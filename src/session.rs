//! Serial source work admitted by the durable per-image authority. This is a
//! bounded acquisition session, not a filesystem or media recovery job.
use crate::acquisition::{
    AcquisitionError, ConsumedRange, MapInterval, MapLimits, RangeSource, ReadLimits,
};
use crate::checkpoint::CheckpointStore;
use crate::contracts::{Budget, ByteRange, CoordinateBinding, ResourceCaps, SourceGeneration};
use crate::control::{Admission, CaseConfig, CaseControl, CaseSnapshot, ControlError};
use crate::ledger::{Ledger, LedgerError};
use crate::source_archive::{Header as ArchiveHeader, SourceArchive};
use std::fs::{File, OpenOptions};
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MAGIC: &[u8] = b"FHSESSION1\n";
const MAX_DEPENDENCIES: usize = 1024;
static ATTEMPT: AtomicU64 = AtomicU64::new(0);

#[derive(Clone)]
pub struct SessionInput {
    pub image: PathBuf,
    pub map: PathBuf,
    pub binding: CoordinateBinding,
    pub limits: ReadLimits,
}
#[derive(Debug)]
pub enum SessionError {
    Io(io::Error),
    Acquisition(AcquisitionError),
    Control(ControlError),
    InvalidCheckpoint,
    SourceChanged,
    ResourceLimit,
    ReplayedAdmission,
    Stopped,
    CaseBusy,
    ExistingCase,
    InvalidEpoch,
}
impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for SessionError {}
impl From<io::Error> for SessionError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<AcquisitionError> for SessionError {
    fn from(e: AcquisitionError) -> Self {
        Self::Acquisition(e)
    }
}
impl From<ControlError> for SessionError {
    fn from(e: ControlError) -> Self {
        Self::Control(e)
    }
}
type Result<T> = std::result::Result<T, SessionError>;

struct Clock {
    epoch: u64,
    started: Instant,
}
impl Clock {
    fn from_start(started: Instant) -> Result<Self> {
        // Sample wall time first so rounding cannot place the monotonic anchor
        // ahead of an external controller's wall-clock observation.
        let wall = SystemTime::now();
        let elapsed = Instant::now()
            .checked_duration_since(started)
            .ok_or(SessionError::ResourceLimit)?;
        let millis = wall
            .checked_sub(elapsed)
            .ok_or(SessionError::ResourceLimit)?
            .duration_since(UNIX_EPOCH)
            .map_err(|_| SessionError::ResourceLimit)?
            .as_millis();
        Ok(Self {
            epoch: u64::try_from(millis).map_err(|_| SessionError::ResourceLimit)?,
            started,
        })
    }
    fn now(&self) -> u64 {
        self.epoch.saturating_add(
            self.started
                .elapsed()
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX),
        )
    }
    fn attempt(&self, phase: &str) -> String {
        format!(
            "{phase}:{}:{}:{}:{}",
            self.epoch,
            std::process::id(),
            self.started.elapsed().as_nanos(),
            ATTEMPT.fetch_add(1, Ordering::Relaxed)
        )
    }
}
fn charge(read_bytes: u64, memory_bytes: u64, index_bytes: u64) -> ResourceCaps {
    ResourceCaps {
        read_bytes,
        memory_bytes,
        index_bytes,
        output_bytes: 0,
        visited_postings: 0,
        transitions: 0,
        decoder_seconds: 0,
    }
}
fn new_admission(admission: Admission) -> Result<()> {
    if admission == Admission::New {
        Ok(())
    } else {
        Err(SessionError::ReplayedAdmission)
    }
}
fn observed_identity(source: &RangeSource) -> Result<(SourceGeneration, u64)> {
    let manifest = source.manifest()?;
    Ok((
        SourceGeneration(
            manifest
                .generation_id()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
        ),
        manifest.image_size(),
    ))
}

fn intake_limits(input: &SessionInput, config: CaseConfig) -> Result<(u64, MapLimits, u64)> {
    let bytes = std::fs::metadata(&input.map)?.len();
    let defaults = MapLimits::default();
    let per_interval = (std::mem::size_of::<MapInterval>() as u64) * 4;
    let fit = config.caps.memory_bytes.saturating_sub(65536) / per_interval;
    let intervals = fit.min(defaults.intervals as u64).min(bytes / 5 + 1) as usize;
    if bytes == 0 || bytes > defaults.bytes || intervals == 0 || config.caps.memory_bytes < 65536 {
        return Err(SessionError::ResourceLimit);
    }
    Ok((
        bytes,
        MapLimits {
            bytes,
            line_bytes: defaults.line_bytes,
            intervals,
        },
        65536 + intervals as u64 * per_interval,
    ))
}
fn lease(ledger: &Ledger) -> Result<File> {
    let path = ledger
        .connection
        .path()
        .ok_or(SessionError::ResourceLimit)?;
    let canonical = std::fs::canonicalize(path)?;
    let mut lock_path = canonical.into_os_string();
    lock_path.push(".session-lock");
    let lock_path = PathBuf::from(lock_path);
    match std::fs::symlink_metadata(&lock_path) {
        Ok(metadata) if !metadata.file_type().is_file() => return Err(SessionError::ResourceLimit),
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error.into()),
        _ => {}
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => Err(SessionError::CaseBusy),
        Err(std::fs::TryLockError::Error(error)) => Err(error.into()),
    }
}
fn require_fresh_case(ledger: &Ledger) -> Result<()> {
    let exists: bool = ledger
        .connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='case_control')",
            [],
            |row| row.get(0),
        )
        .map_err(|e| SessionError::Control(ControlError::Sql(e)))?;
    if exists {
        let count: i64 = ledger
            .connection
            .query_row("SELECT count(*) FROM case_control", [], |row| row.get(0))
            .map_err(|e| SessionError::Control(ControlError::Sql(e)))?;
        if count != 0 {
            return Err(SessionError::ExistingCase);
        }
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
struct DatabaseIdentity {
    path: PathBuf,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}
impl DatabaseIdentity {
    fn observe(ledger: &Ledger) -> std::result::Result<Self, LedgerError> {
        let path =
            std::fs::canonicalize(ledger.connection.path().ok_or(LedgerError::InvalidInput)?)
                .map_err(|_| LedgerError::InvalidInput)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let metadata = std::fs::metadata(&path).map_err(|_| LedgerError::InvalidInput)?;
            Ok(Self {
                path,
                device: metadata.dev(),
                inode: metadata.ino(),
            })
        }
        #[cfg(not(unix))]
        {
            Ok(Self { path })
        }
    }
}
/// Source observations for one immutable native discovery batch have reached a
/// verified frontier in this exact case. Only the session can construct a token.
#[derive(Debug)]
pub struct SealedDiscoveryBatch {
    database: DatabaseIdentity,
    source: SourceGeneration,
    checkpoint: String,
    digest: [u8; 32],
    acquisition_bytes: u64,
}
impl SealedDiscoveryBatch {
    pub(crate) fn validate(
        self,
        ledger: &Ledger,
        source: &SourceGeneration,
        digest: [u8; 32],
    ) -> std::result::Result<(), LedgerError> {
        if self.source != *source
            || self.digest != digest
            || self.database != DatabaseIdentity::observe(ledger)?
        {
            return Err(LedgerError::Conflict);
        }
        let valid: bool = ledger.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM case_control c JOIN sources s ON s.id=c.source WHERE c.source=?1 AND c.checkpoint=?2 AND c.state='running' AND s.valid=1 AND coalesce((SELECT used FROM case_phases WHERE source=c.source AND phase='acquisition' AND resource=0),0)=?3)",
            rusqlite::params![self.source.0, self.checkpoint, self.acquisition_bytes as i64], |r| r.get(0))?;
        if !valid {
            return Err(LedgerError::Conflict);
        }
        Ok(())
    }
}

/// One bounded group of serial parser operations. Only CPU/memory permission is
/// shared; each actual source read still receives its own durable admission.
#[derive(Debug)]
pub struct CpuBatch {
    permit: CpuPermit,
    remaining_operations: u16,
}
#[derive(Debug, Clone)]
pub(crate) struct CpuPermit {
    session: u64,
    batch: u64,
    source: SourceGeneration,
    memory: u64,
    started: Instant,
    allowance: Duration,
    phase: &'static str,
}

/// A single in-memory media-work boundary. It cannot clear dependencies itself.
/// Only verified durable publication evidence can retire subsequent reads.
#[derive(Debug)]
pub struct ReadEpoch {
    id: u64,
    source: SourceGeneration,
    dependencies: usize,
    consumed: usize,
}

pub struct CaseSession {
    source: RangeSource,
    image_size: u64,
    control: CaseControl,
    clock: Clock,
    // Work dependencies exclude resume-verification rereads.
    dependencies: Vec<(ByteRange, [u8; 32])>,
    halted: bool,
    active_epoch: Option<u64>,
    session_id: u64,
    active_cpu_batch: Option<u64>,
    archived: Vec<[u8; 32]>,
    // Keep the single-controller lease until source and control handles drop.
    _lease: File,
}
impl CaseSession {
    pub fn begin_cpu_batch(
        &mut self,
        phase: &'static str,
        memory: u64,
        operations: u16,
    ) -> Result<CpuBatch> {
        if self.active_cpu_batch.is_some() || operations == 0 || operations > 128 {
            return Err(SessionError::InvalidEpoch);
        }
        let started = Instant::now();
        // Reserve bounded controller/dependency growth once rather than writing
        // a new peak-memory receipt for every tiny read in the group.
        let admission_memory = memory
            .checked_add(256 * 1024)
            .ok_or(SessionError::ResourceLimit)?;
        let allowance = self.admit_external(phase, admission_memory)?;
        let batch = ATTEMPT.fetch_add(1, Ordering::Relaxed);
        self.active_cpu_batch = Some(batch);
        Ok(CpuBatch {
            remaining_operations: operations,
            permit: CpuPermit {
                session: self.session_id,
                batch,
                source: self.source_generation(),
                memory,
                started,
                allowance,
                phase,
            },
        })
    }
    pub(crate) fn cpu_batch_permit(&mut self, batch: &mut CpuBatch) -> Result<CpuPermit> {
        self.check_cpu_permit(&batch.permit, 0)?;
        if batch.remaining_operations == 0 {
            return Err(SessionError::ResourceLimit);
        }
        batch.remaining_operations -= 1;
        Ok(batch.permit.clone())
    }
    pub(crate) fn check_cpu_permit(
        &mut self,
        permit: &CpuPermit,
        requested: u64,
    ) -> Result<Duration> {
        if permit.session != self.session_id
            || self.active_cpu_batch != Some(permit.batch)
            || permit.source != self.source_generation()
        {
            return Err(SessionError::InvalidEpoch);
        }
        if requested > permit.memory {
            return Err(SessionError::ResourceLimit);
        }
        self.check_source()?;
        let total = self
            .metadata_memory_bound()
            .checked_add(permit.memory)
            .ok_or(SessionError::ResourceLimit)?;
        let config = self.control.config()?;
        if total > config.caps.memory_bytes {
            return Err(SessionError::ResourceLimit);
        }
        if total > self.snapshot()?.used.memory_bytes {
            self.admit_external(permit.phase, permit.memory)?;
        }
        permit
            .allowance
            .checked_sub(permit.started.elapsed())
            .filter(|d| !d.is_zero())
            .ok_or(SessionError::ResourceLimit)
    }
    /// Settle elapsed parser work before its outcomes can be committed. A valid
    /// consumed token always closes, including on pause/deadline/source failure.
    pub fn finish_cpu_batch(&mut self, batch: CpuBatch) -> Result<()> {
        if batch.permit.session != self.session_id
            || self.active_cpu_batch != Some(batch.permit.batch)
        {
            return Err(SessionError::InvalidEpoch);
        }
        self.active_cpu_batch = None;
        self.admit_external(batch.permit.phase, 0).map(|_| ())
    }
    /// Begin one media-work epoch after discovery dependencies are checkpointed.
    /// Nested epochs are refused; unsupported or failed work retains its reads.
    pub fn begin_read_epoch(&mut self) -> Result<ReadEpoch> {
        if self.active_epoch.is_some() {
            return Err(SessionError::InvalidEpoch);
        }
        self.admit_external("source-epoch-begin", 256)?;
        let id = ATTEMPT.fetch_add(1, Ordering::Relaxed);
        let epoch = ReadEpoch {
            id,
            source: self.source_generation(),
            dependencies: self.dependencies.len(),
            consumed: self.source.consumed().len(),
        };
        self.active_epoch = Some(id);
        Ok(epoch)
    }
    /// Finish a work boundary without discarding any source dependencies.
    pub fn retain_read_epoch(&mut self, epoch: ReadEpoch) -> Result<()> {
        self.check_epoch(&epoch)?;
        self.active_epoch = None;
        Ok(())
    }
    fn check_epoch(&self, epoch: &ReadEpoch) -> Result<()> {
        if self.active_epoch != Some(epoch.id)
            || self.source_generation() != epoch.source
            || epoch.dependencies > self.dependencies.len()
            || epoch.consumed > self.source.consumed().len()
        {
            return Err(SessionError::InvalidEpoch);
        }
        Ok(())
    }
    pub(crate) fn retire_read_epoch(
        &mut self,
        epoch: ReadEpoch,
        proof: &crate::publication::RetainedSourceProof,
    ) -> Result<bool> {
        self.check_epoch(&epoch)?;
        let result = self.retire_read_epoch_inner(&epoch, proof);
        // A consumed valid token always closes its epoch. Every failure before
        // the final mutation retains all dependencies for a subsequent checkpoint.
        self.active_epoch = None;
        result
    }
    fn retire_read_epoch_inner(
        &mut self,
        epoch: &ReadEpoch,
        proof: &crate::publication::RetainedSourceProof,
    ) -> Result<bool> {
        let memory = 65536 + proof.consumed().len() as u64 * 256;
        self.admit_external("source-epoch-retirement", memory)?;
        let result = self.source.manifest();
        let manifest = self.source_result(result)?;
        if manifest.generation_id() != proof.generation()
            || manifest.image_size() != proof.image_size()
            || manifest.map_digest() != proof.map_sha256()
            || manifest.binding().input_domain() != proof.input_domain()
            || manifest.binding().image_domain() != proof.image_domain()
            || manifest.binding().evidence() != proof.binding()
            || self.archived != proof.archive_refs()
        {
            return Err(SessionError::InvalidEpoch);
        }
        let mut retained = std::collections::BTreeMap::new();
        for item in proof.consumed() {
            if retained
                .insert(item.image, item.sha256)
                .is_some_and(|old| old != item.sha256)
            {
                return Err(SessionError::InvalidEpoch);
            }
        }
        let covered = self.dependencies[epoch.dependencies..]
            .iter()
            .all(|(range, hash)| retained.get(range) == Some(hash))
            && self.source.consumed()[epoch.consumed..]
                .iter()
                .all(|item| retained.get(&item.image) == Some(&item.sha256));
        self.admit_external("source-epoch-retirement-complete", memory)?;
        if covered {
            self.source.retire_consumed_after(epoch.consumed)?;
            self.dependencies.truncate(epoch.dependencies);
            self.dependencies.shrink_to_fit();
        }
        Ok(covered)
    }
    /// One bounded semantic-state operation under the existing case authority.
    /// The closure cannot retain the ledger reference beyond this call.
    pub fn with_ledger<T>(
        &mut self,
        bytes_bound: u64,
        action: impl FnOnce(&mut Ledger) -> std::result::Result<T, crate::ledger::LedgerError>,
    ) -> Result<T> {
        let memory = bytes_bound
            .checked_mul(3)
            .ok_or(SessionError::ResourceLimit)?;
        self.with_ledger_resources(charge(0, memory, bytes_bound), action)
    }
    /// Execute a bounded semantic-state operation with explicit read/write
    /// accounting. Read-only loads consume read bytes and memory, not the
    /// cumulative index-write allowance. Caller must bound the closure's work.
    pub fn with_ledger_resources<T>(
        &mut self,
        resources: ResourceCaps,
        action: impl FnOnce(&mut Ledger) -> std::result::Result<T, crate::ledger::LedgerError>,
    ) -> Result<T> {
        self.admit_work("case-state", resources)?;
        let result = self
            .control
            .with_ledger(action)
            .map_err(ControlError::Ledger);
        let settled = self.admit_external("case-state-complete", 0);
        match result {
            Ok(value) => {
                settled?;
                Ok(value)
            }
            Err(error) => Err(error.into()),
        }
    }
    /// Initial intake has no prior source identity. It uses the bounded range
    /// engine, then persists its actual start time and map-read admission before
    /// this constructor can return an operational session.
    pub fn create(ledger: Ledger, input: SessionInput, config: CaseConfig) -> Result<Self> {
        Self::create_started(ledger, input, config, Instant::now())
    }
    pub(crate) fn create_started(
        ledger: Ledger,
        input: SessionInput,
        config: CaseConfig,
        started: Instant,
    ) -> Result<Self> {
        let clock = Clock::from_start(started)?;
        let lease = lease(&ledger)?;
        require_fresh_case(&ledger)?;
        let (metadata_bytes, map_limits, metadata_memory) = intake_limits(&input, config)?;
        if metadata_bytes > config.caps.read_bytes {
            return Err(SessionError::ResourceLimit);
        }
        let mut budget = Budget::new(
            Duration::from_millis(config.total_millis),
            Duration::from_millis(config.shutdown_reserve_millis),
            config.caps,
        )
        .map_err(|_| SessionError::ResourceLimit)?;
        budget
            .account_elapsed(clock.started.elapsed())
            .map_err(|_| SessionError::ResourceLimit)?;
        if !budget.may_schedule() {
            return Err(ControlError::BudgetExhausted.into());
        }
        let source = RangeSource::open_with_map_limits(
            &input.image,
            &input.map,
            input.binding,
            input.limits,
            budget,
            map_limits,
        )?;
        let (generation, image_size) = observed_identity(&source)?;
        let mut control = CaseControl::create(ledger, generation, config)?;
        control.resume_at(clock.epoch)?;
        new_admission(control.authorize_at(
            clock.now(),
            "intake",
            &clock.attempt("intake"),
            charge(source.map_bytes_read(), metadata_memory, 0),
        )?)?;
        Ok(Self {
            source,
            image_size,
            control,
            clock,
            dependencies: Vec::new(),
            halted: false,
            active_epoch: None,
            session_id: ATTEMPT.fetch_add(1, Ordering::Relaxed),
            active_cpu_batch: None,
            archived: Vec::new(),
            _lease: lease,
        })
    }
    /// Resume requires the expected generation from the durable case. A linked
    /// retained frontier is verified before source I/O, then every consumed span
    /// is rehashed under fresh, durable read admissions.
    pub fn resume(
        ledger: Ledger,
        input: SessionInput,
        expected: SourceGeneration,
        store: &CheckpointStore,
    ) -> Result<Self> {
        Self::resume_started(ledger, input, expected, store, Instant::now())
    }
    pub(crate) fn resume_started(
        ledger: Ledger,
        input: SessionInput,
        expected: SourceGeneration,
        store: &CheckpointStore,
        started: Instant,
    ) -> Result<Self> {
        let clock = Clock::from_start(started)?;
        let lease = lease(&ledger)?;
        let mut control = CaseControl::adopt(ledger, expected.clone())?;
        let config = control.config()?;
        let (metadata_bytes, map_limits, metadata_memory) = intake_limits(&input, config)?;
        let snapshot = control.snapshot()?;
        if store.working_memory_bound() > config.caps.memory_bytes {
            return Err(SessionError::ResourceLimit);
        }
        let (dependencies, archived) = if let Some(linked) = snapshot.checkpoint.as_deref() {
            control.verify_checkpoint(store)?;
            let frontier = store.load(parse_id(linked)?)?;
            if frontier.payloads.len() > 1024
                || frontier
                    .payloads
                    .iter()
                    .copied()
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    != frontier.payloads.len()
            {
                return Err(SessionError::InvalidCheckpoint);
            }
            (
                decode_frontier(&frontier.bytes, &expected)?,
                frontier.payloads,
            )
        } else {
            // No frontier means no completed semantic work is adopted.
            (Vec::new(), Vec::new())
        };
        let resume_memory = metadata_memory
            .checked_add(
                dependencies.capacity() as u64
                    * std::mem::size_of::<(ByteRange, [u8; 32])>() as u64,
            )
            .and_then(|n| n.checked_add(archived.capacity() as u64 * 32))
            .ok_or(SessionError::ResourceLimit)?;
        if resume_memory > config.caps.memory_bytes {
            return Err(SessionError::ResourceLimit);
        }
        control.resume_at(clock.epoch)?;
        new_admission(control.authorize_at(
            clock.now(),
            "intake",
            &clock.attempt("resume-intake"),
            charge(metadata_bytes, resume_memory, 0),
        )?)?;
        let mut budget = Budget::new(
            Duration::from_millis(config.total_millis),
            Duration::from_millis(config.shutdown_reserve_millis),
            config.caps,
        )
        .map_err(|_| SessionError::ResourceLimit)?;
        budget
            .account_elapsed(Duration::from_millis(control.snapshot()?.elapsed_millis))
            .map_err(|_| SessionError::ResourceLimit)?;
        let source = match RangeSource::open_with_map_limits(
            &input.image,
            &input.map,
            input.binding,
            input.limits,
            budget,
            map_limits,
        ) {
            Ok(source) => source,
            Err(AcquisitionError::SourceChanged) => {
                control.invalidate_source(b"source changed during resume intake")?;
                return Err(SessionError::SourceChanged);
            }
            Err(error) => return Err(error.into()),
        };
        let (observed, image_size) = match observed_identity(&source) {
            Err(SessionError::Acquisition(AcquisitionError::SourceChanged)) => {
                control.invalidate_source(b"source changed during generation observation")?;
                return Err(SessionError::SourceChanged);
            }
            result => result?,
        };
        if observed != expected {
            control.invalidate_source(b"session observed changed source generation")?;
            return Err(SessionError::SourceChanged);
        }
        let mut session = Self {
            source,
            image_size,
            control,
            clock,
            dependencies,
            halted: false,
            active_epoch: None,
            session_id: ATTEMPT.fetch_add(1, Ordering::Relaxed),
            active_cpu_batch: None,
            archived,
            _lease: lease,
        };
        if !session.archived.is_empty() {
            session.admit_work("source-archive-open", charge(256, 512 * 1024, 0))?;
            let manifest = session.source_snapshot()?;
            let archive = SourceArchive::open(
                &store.source_archive_root()?,
                ArchiveHeader::from_manifest(&manifest)?,
            )?;
            for index in 0..session.archived.len() {
                session.admit_work("source-archive-load", charge(2 * 65536, 512 * 1024, 0))?;
                let block = archive.load(session.archived[index])?;
                let largest = block
                    .iter()
                    .map(|(range, _)| range.len())
                    .max()
                    .unwrap_or(0);
                let overlap = largest
                    .checked_add(512 * 1024)
                    .ok_or(SessionError::ResourceLimit)?;
                // Include the retained decoded block and growing verification
                // receipts while the largest source read buffer is alive.
                session.admit_external("source-archive-verification", overlap)?;
                for (range, expected_hash) in block {
                    session.admit_read(range, "resume-archive-verification")?;
                    let result = session.source.read(range);
                    let actual = session.source_result(result)?;
                    if actual.sha256 != expected_hash {
                        session
                            .control
                            .invalidate_source(b"archived dependency hash mismatch")?;
                        return Err(SessionError::SourceChanged);
                    }
                }
                // The verified immutable block remains referenced by the linked
                // frontier; keep only one block's receipt buffers in memory.
                session.source.retire_consumed_after(0)?;
                session.admit_external("source-archive-verified", 512 * 1024)?;
            }
        }
        for i in 0..session.dependencies.len() {
            let (range, expected_hash) = session.dependencies[i];
            session.admit_read(range, "resume-verification")?;
            let result = session.source.read(range);
            let actual = session.source_result(result)?;
            if actual.sha256 != expected_hash {
                session
                    .control
                    .invalidate_source(b"resume dependency hash mismatch")?;
                return Err(SessionError::SourceChanged);
            }
        }
        Ok(session)
    }
    pub fn source_generation(&self) -> SourceGeneration {
        self.control.source().clone()
    }
    /// Size of the observed bound generation; every actual operation rechecks
    /// generation identity before this geometry can authorize source reads.
    pub fn image_size(&self) -> u64 {
        self.image_size
    }
    pub fn source_snapshot(&mut self) -> Result<crate::acquisition::SourceManifest> {
        self.check_source()?;
        let result = self.source.manifest();
        self.source_result(result)
            .map(|manifest| manifest.with_archives(&self.archived))
    }
    pub fn snapshot(&self) -> Result<CaseSnapshot> {
        Ok(self.control.snapshot()?)
    }
    pub fn image_bytes_read_this_run(&self) -> u64 {
        self.source.bytes_read()
    }
    /// Guard previously admitted cache/CPU work without a new SQLite commit.
    /// Source conflicts still invalidate durable dependencies immediately.
    pub fn check_source(&mut self) -> Result<()> {
        if self.halted {
            return Err(SessionError::Stopped);
        }
        self.control.check_running_at(self.clock.now())?;
        let result = self.source.check_generation();
        self.source_result(result)
    }
    /// Number of live (not archived/retired) source geometries. This exposes no
    /// clearing authority and avoids copying a manifest just to plan a boundary.
    pub fn live_dependency_count(&self) -> usize {
        self.dependencies.len()
    }
    /// Read-only mask query used to avoid speculative prefetch across holes.
    /// Callers check_source before using it; actual reads independently recheck.
    pub fn range_available(&self, image: ByteRange) -> Result<bool> {
        self.source
            .binding()
            .require_supported()
            .map_err(|_| SessionError::Acquisition(AcquisitionError::BindingUnverified))?;
        let input = self
            .source
            .binding()
            .image_to_input(image)
            .map_err(|_| SessionError::Acquisition(AcquisitionError::InvalidRange))?;
        Ok(self
            .source
            .map()
            .quality_iter(input)
            .all(|part| part.status.available()))
    }
    /// Admit one supervised native protocol step. `memory_bound` includes the
    /// selected child memory reservation and overlapping IPC/read buffers. The
    /// returned allowance must also cap the supervisor's next wait.
    pub fn admit_external(&mut self, phase: &str, memory_bound: u64) -> Result<Duration> {
        self.admit_work(phase, charge(0, memory_bound, 0))
    }
    /// Admit non-source work through the same durable per-image authority.
    /// Reservations are conservative; actual stage/native read/write receipts
    /// must remain separate from these admission counters.
    pub fn admit_work(&mut self, phase: &str, mut resources: ResourceCaps) -> Result<Duration> {
        if self.halted {
            return Err(SessionError::Stopped);
        }
        let result = self.source.check_generation();
        self.source_result(result)?;
        resources.memory_bytes = self
            .metadata_memory_bound()
            .checked_add(resources.memory_bytes)
            .ok_or(SessionError::ResourceLimit)?;
        new_admission(self.control.authorize_at(
            self.clock.now(),
            phase,
            &self.clock.attempt("work-step"),
            resources,
        )?)?;
        let config = self.control.config()?;
        let elapsed = self.control.snapshot()?.elapsed_millis;
        Ok(Duration::from_millis(
            config
                .total_millis
                .saturating_sub(config.shutdown_reserve_millis)
                .saturating_sub(elapsed),
        ))
    }
    /// The engine-owned buffer is dropped before another read can begin. Memory
    /// allocated or retained independently by a caller's callback is its own work.
    pub fn with_read<T>(
        &mut self,
        range: ByteRange,
        consume: impl FnOnce(&[u8]) -> T,
    ) -> Result<T> {
        if self.halted {
            return Err(SessionError::Stopped);
        }
        let previous = self
            .dependencies
            .iter()
            .find(|(r, _)| *r == range)
            .map(|(_, hash)| *hash);
        if previous.is_none() && self.dependencies.len() >= MAX_DEPENDENCIES {
            return Err(SessionError::ResourceLimit);
        }
        self.admit_read(range, "acquisition")?;
        let result = self.source.read(range);
        let receipt = self.source_result(result)?;
        if previous.is_some_and(|old| old != receipt.sha256) {
            self.control
                .invalidate_source(b"repeated source range hash changed")?;
            return Err(SessionError::SourceChanged);
        }
        let result = consume(&receipt.bytes);
        if previous.is_none() {
            self.dependencies.push((range, receipt.sha256));
        }
        Ok(result)
    }
    fn admit_read(&mut self, range: ByteRange, phase: &str) -> Result<()> {
        new_admission(self.control.authorize_at(
            self.clock.now(),
            phase,
            &self.clock.attempt(phase),
            charge(
                range.len(),
                self.metadata_memory_bound().saturating_add(range.len()),
                0,
            ),
        )?)
    }
    fn source_result<T>(&mut self, result: std::result::Result<T, AcquisitionError>) -> Result<T> {
        match result {
            Err(AcquisitionError::SourceChanged) => {
                self.control
                    .invalidate_source(b"live source dependency changed")?;
                Err(SessionError::SourceChanged)
            }
            Err(error) => Err(error.into()),
            Ok(value) => Ok(value),
        }
    }
    fn metadata_memory_bound(&self) -> u64 {
        65536
            + self.archived.len() as u64 * 64
            + self.source.map().intervals().len() as u64
                * 4
                * std::mem::size_of::<MapInterval>() as u64
            + (self.source.consumed().len() as u64 + 1)
                * 2
                * std::mem::size_of::<ConsumedRange>() as u64
            + (self.dependencies.len() as u64 + 1)
                * 2
                * std::mem::size_of::<(ByteRange, [u8; 32])>() as u64
    }
    /// Make room before a native discovery callback reads a new geometry. The
    /// native child may be waiting on byte IPC while completed observations are
    /// sealed; no cursor can advance until the final batch token is committed.
    pub fn prepare_discovery_read(
        &mut self,
        range: ByteRange,
        store: &mut CheckpointStore,
    ) -> Result<()> {
        if self.active_epoch.is_some() {
            return Err(SessionError::InvalidEpoch);
        }
        self.check_source()?;
        let new_dependency = !self.dependencies.iter().any(|(old, _)| *old == range);
        if (new_dependency && self.dependencies.len() >= MAX_DEPENDENCIES)
            || !self.source.can_retain_read(range)
        {
            self.archive_dependencies(store)?;
        }
        if !self.source.can_retain_read(range) {
            return Err(SessionError::ResourceLimit);
        }
        Ok(())
    }
    /// Bind an immutable native batch digest to durable source observations.
    /// The discovery index consumes the token inside its atomic cursor update.
    pub fn seal_discovery_batch(
        &mut self,
        store: &mut CheckpointStore,
        digest: [u8; 32],
    ) -> Result<SealedDiscoveryBatch> {
        if self.archive_dependencies(store)?.is_none() {
            self.checkpoint(store)?;
        }
        self.admit_external("discovery-batch-seal", 65536)?;
        let checkpoint = self
            .snapshot()?
            .checkpoint
            .ok_or(SessionError::InvalidCheckpoint)?;
        let database = self
            .control
            .with_ledger(|ledger| DatabaseIdentity::observe(ledger))
            .map_err(ControlError::Ledger)?;
        let acquisition_bytes = self.control.phase_totals("acquisition")?.read_bytes;
        Ok(SealedDiscoveryBatch {
            acquisition_bytes,
            database,
            source: self.source_generation(),
            checkpoint,
            digest,
        })
    }
    /// Seal a bounded discovery batch before its index/cursor can commit.
    /// Immutable blocks survive frontier history compaction. No image bytes are
    /// copied, and all read, time and write charges remain cumulative.
    pub fn archive_dependencies(
        &mut self,
        store: &mut CheckpointStore,
    ) -> Result<Option<[u8; 32]>> {
        if self.active_epoch.is_some() {
            return Err(SessionError::InvalidEpoch);
        }
        self.check_source()?;
        if self.dependencies.is_empty() {
            return Ok(None);
        }
        if self.archived.len() >= 1024 {
            return Err(SessionError::ResourceLimit);
        }
        let writes = SourceArchive::write_bound(self.dependencies.len())?;
        let largest = self
            .dependencies
            .iter()
            .map(|(range, _)| range.len())
            .max()
            .unwrap_or(0);
        let working_memory = largest
            .checked_add(512 * 1024)
            .ok_or(SessionError::ResourceLimit)?;
        self.admit_work(
            "source-archive-write",
            charge(2 * 65536, working_memory, writes),
        )?;
        let manifest = self.source_snapshot()?;
        let archive = SourceArchive::open_or_create(
            &store.source_archive_root()?,
            ArchiveHeader::from_manifest(&manifest)?,
        )?;
        let id = archive.put(&self.dependencies)?;
        if !self.archived.contains(&id) {
            self.archived.push(id);
        }
        // Link both the archive and still-live hashes first. Any interruption
        // before this succeeds leaves all in-memory evidence unretired.
        self.checkpoint(store)?;
        self.source.retire_consumed_after(0)?;
        self.dependencies.clear();
        self.dependencies.shrink_to_fit();
        // A failure here still leaves the preceding durable frontier with both
        // representations; resume safely verifies that redundant evidence.
        self.checkpoint(store)?;
        Ok(Some(id))
    }
    /// Frontier writes are admitted from the shutdown reserve. Snapshot payload
    /// references remain source-dependent; they are not portable media artifacts.
    pub fn checkpoint(&mut self, store: &mut CheckpointStore) -> Result<[u8; 32]> {
        let result = self.source.check_generation();
        self.source_result(result)?;
        let bytes = encode_frontier(self.control.source(), &self.dependencies)?;
        // One immutable frontier plus a small history/namespace update. Buffer
        // overlap is reserved separately by the store's working-memory bound;
        // it must not be billed as permanent index growth.
        let write_bound = (bytes.len() as u64)
            .checked_add(self.archived.len() as u64 * 32)
            .and_then(|n| n.checked_add(64))
            .and_then(|n| n.checked_mul(2))
            .and_then(|n| n.checked_add(16384))
            .ok_or(SessionError::ResourceLimit)?;
        let memory_bound = self
            .metadata_memory_bound()
            .saturating_add(store.working_memory_bound())
            .saturating_add(write_bound);
        let batch = self.clock.attempt("checkpoint");
        let admitted = match self.control.authorize_at(
            self.clock.now(),
            "checkpoint",
            &batch,
            charge(0, memory_bound, write_bound),
        ) {
            Ok(admitted) => admitted,
            Err(ControlError::BudgetExhausted | ControlError::NotRunning) => {
                self.control.authorize_shutdown_at(
                    self.clock.now(),
                    &batch,
                    charge(0, memory_bound, write_bound),
                )?
            }
            Err(error) => return Err(error.into()),
        };
        new_admission(admitted)?;
        // CURRENT and SQLite cannot commit atomically; keep the prior linked
        // frontier until its replacement is durably linked, even with history 0.
        let linked = self
            .control
            .snapshot()?
            .checkpoint
            .map(|id| parse_id(&id))
            .transpose()?;
        let id = store.publish_linked(&bytes, &self.archived, linked)?;
        self.control.link_checkpoint(store)?;
        store.release_retained()?;
        Ok(id)
    }
    pub fn pause(&mut self, store: &mut CheckpointStore) -> Result<()> {
        self.halted = true;
        let checkpoint = self.checkpoint(store);
        // Stop admission even if storage cannot persist a new frontier. Report
        // the checkpoint failure while retaining any earlier committed frontier.
        self.control.pause_at(self.clock.now())?;
        checkpoint.map(|_| ())
    }
    pub fn cancel(&mut self, store: &mut CheckpointStore) -> Result<()> {
        self.halted = true;
        let checkpoint = self.checkpoint(store);
        self.control.cancel_at(self.clock.now())?;
        checkpoint.map(|_| ())
    }
}
fn parse_id(text: &str) -> Result<[u8; 32]> {
    if text.len() != 64 || !text.is_ascii() {
        return Err(SessionError::InvalidCheckpoint);
    }
    let mut id = [0; 32];
    for (i, b) in id.iter_mut().enumerate() {
        *b = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16)
            .map_err(|_| SessionError::InvalidCheckpoint)?;
    }
    Ok(id)
}
fn encode_frontier(
    source: &SourceGeneration,
    dependencies: &[(ByteRange, [u8; 32])],
) -> Result<Vec<u8>> {
    let mut bytes = MAGIC.to_vec();
    bytes.extend(parse_id(&source.0)?);
    bytes.extend((dependencies.len() as u32).to_le_bytes());
    for (range, hash) in dependencies {
        bytes.extend(range.start().to_le_bytes());
        bytes.extend(range.end().to_le_bytes());
        bytes.extend(hash);
    }
    Ok(bytes)
}
fn decode_frontier(bytes: &[u8], source: &SourceGeneration) -> Result<Vec<(ByteRange, [u8; 32])>> {
    let offset = MAGIC.len() + 32;
    if bytes.len() < offset + 4
        || !bytes.starts_with(MAGIC)
        || bytes[MAGIC.len()..offset] != parse_id(&source.0)?
    {
        return Err(SessionError::InvalidCheckpoint);
    }
    let n = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
    if n > MAX_DEPENDENCIES || bytes.len() != offset + 4 + n * 48 {
        return Err(SessionError::InvalidCheckpoint);
    }
    bytes[offset + 4..]
        .as_chunks::<48>()
        .0
        .iter()
        .map(|entry| {
            let start = u64::from_le_bytes(entry[..8].try_into().unwrap());
            let end = u64::from_le_bytes(entry[8..16].try_into().unwrap());
            Ok((
                ByteRange::new(start, end).map_err(|_| SessionError::InvalidCheckpoint)?,
                entry[16..].try_into().unwrap(),
            ))
        })
        .collect()
}
