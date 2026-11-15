//! Isolated, acquisition-masked TSK 4.14.0 logical extent adapter.
//! The helper receives geometry only, never image/map paths or descriptors.
//! Native RSS limits remain advisory on macOS; the reserved process envelope is
//! conservative admission accounting, not a measured hard RSS guarantee.
pub use crate::supervision::ChildCleanup;
use crate::supervision::{self, Event, SupervisionError, check_control, send};
use crate::{
    contracts::{ByteRange, LogicalOwner},
    session::{CaseSession, SessionError},
};
use std::{
    fmt, io,
    path::Path,
    process::Command,
    sync::{atomic::AtomicBool, mpsc},
    thread,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilesystemProfile {
    Fat32,
    ExFat,
}
impl FilesystemProfile {
    fn arg(self) -> &'static str {
        match self {
            Self::Fat32 => "fat32",
            Self::ExFat => "exfat",
        }
    }
    fn tsk_type(self) -> u64 {
        match self {
            Self::Fat32 => 8,
            Self::ExFat => 10,
        }
    }
}
#[derive(Debug, Clone, Copy)]
pub struct AdapterLimits {
    pub wall: Duration,
    pub callback_bytes: u64,
    pub total_read_bytes: u64,
    pub files: usize,
    pub runs: usize,
    pub output_bytes: usize,
    pub helper_memory_bytes: u64,
}
impl Default for AdapterLimits {
    fn default() -> Self {
        Self {
            wall: Duration::from_secs(30),
            callback_bytes: 1024 * 1024,
            total_read_bytes: 64 * 1024 * 1024,
            files: 1024,
            runs: 1024,
            output_bytes: 1024 * 1024,
            helper_memory_bytes: 256 * 1024 * 1024,
        }
    }
}
#[derive(Debug)]
pub enum FilesystemError {
    Io(io::Error),
    Session(SessionError),
    Protocol,
    VersionMismatch,
    ResourceLimit,
    Cancelled,
    TimedOut,
    NativeFailure,
    NativeError(u64),
    Unsupported,
    Unresolved,
    SourceMismatch,
}
impl fmt::Display for FilesystemError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for FilesystemError {}
impl From<io::Error> for FilesystemError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<SessionError> for FilesystemError {
    fn from(e: SessionError) -> Self {
        Self::Session(e)
    }
}
impl From<SupervisionError> for FilesystemError {
    fn from(error: SupervisionError) -> Self {
        match error {
            SupervisionError::Io(error) => Self::Io(error),
            SupervisionError::Protocol => Self::Protocol,
            SupervisionError::ResourceLimit => Self::ResourceLimit,
            SupervisionError::Cancelled => Self::Cancelled,
            SupervisionError::TimedOut => Self::TimedOut,
            SupervisionError::NativeFailure => Self::NativeFailure,
        }
    }
}
type Result<T> = std::result::Result<T, FilesystemError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilesystemGeometry {
    pub profile: FilesystemProfile,
    pub block_size: u64,
    pub block_count: u64,
    pub root_inode: u64,
    pub start_in_image: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtentEvidence {
    MetadataAllocated,
    MetadataCandidate,
    DeletedOrRecovery,
    SharedGeometry,
    Partial,
    Unsupported,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtentRun {
    pub logical: ByteRange,
    pub image: Option<ByteRange>,
    pub raw_flags: u64,
    pub allocation_consistent: bool,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogicalFileView {
    pub owner: LogicalOwner,
    pub size: u64,
    pub metadata_flags: u64,
    pub name_flags: u64,
    pub attribute_flags: u64,
    pub initialized_size: u64,
    pub skip_length: u64,
    pub runs: Vec<ExtentRun>,
    pub evidence: ExtentEvidence,
}
impl LogicalFileView {
    /// Reads only evidenced physical spans. Unknown/sparse/filler/uninitialized
    /// regions are never silently filled with zeros. The callback owns any copies
    /// it retains; the adapter's temporary logical buffer is dropped on return.
    pub fn with_read<T>(
        &self,
        session: &mut CaseSession,
        range: ByteRange,
        maximum: u64,
        consume: impl FnOnce(&[u8]) -> T,
    ) -> Result<T> {
        if self.owner.source != session.source_generation() {
            return Err(FilesystemError::SourceMismatch);
        }
        if range.end() > self.size || range.len() > maximum || range.len() > 1024 * 1024 {
            return Err(FilesystemError::ResourceLimit);
        }
        if self.evidence != ExtentEvidence::MetadataAllocated {
            return Err(FilesystemError::Unresolved);
        }
        let memory = range
            .len()
            .checked_mul(2)
            .and_then(|v| v.checked_add(self.runs.len() as u64 * 128))
            .ok_or(FilesystemError::ResourceLimit)?;
        session.admit_external("filesystem-logical", memory)?;
        let mut bytes = vec![0; range.len() as usize];
        let mut cursor = range.start();
        for run in &self.runs {
            let Some(part) = run.logical.intersection(range) else {
                continue;
            };
            if part.start() != cursor {
                return Err(FilesystemError::Unresolved);
            }
            let image = run.image.ok_or(FilesystemError::Unresolved)?;
            let physical = ByteRange::from_len(
                image.start() + part.start() - run.logical.start(),
                part.len(),
            )
            .map_err(|_| FilesystemError::Protocol)?;
            let dest = (part.start() - range.start()) as usize;
            session.with_read(physical, |source| {
                bytes[dest..dest + source.len()].copy_from_slice(source)
            })?;
            cursor = part.end();
        }
        if cursor != range.end() {
            return Err(FilesystemError::Unresolved);
        }
        Ok(consume(&bytes))
    }
}
#[derive(Debug, Clone)]
pub struct FilesystemReport {
    pub geometry: FilesystemGeometry,
    pub files: Vec<LogicalFileView>,
    pub callback_reads: u64,
    /// Requested bytes include TSK speculative cache windows.
    pub callback_requested_bytes: u64,
    /// Actual acquired prefix bytes delivered; no unavailable suffix is sent.
    pub callback_bytes: u64,
    pub max_rss_bytes: Option<u64>,
    pub helper_version: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectoryCursor {
    pub directory_inode: Option<u64>,
    pub next_entry: u64,
}
#[derive(Debug, Clone, Copy)]
pub struct DirectoryBatchLimits {
    pub adapter: AdapterLimits,
    pub entries: usize,
}
impl Default for DirectoryBatchLimits {
    fn default() -> Self {
        Self {
            adapter: AdapterLimits::default(),
            entries: 256,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryFile {
    pub entry_index: u64,
    pub view: LogicalFileView,
    pub unchecked_runs: Vec<u64>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildDirectory {
    pub entry_index: u64,
    pub inode: u64,
    pub name_flags: u64,
    pub metadata_flags: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeferredReason {
    MetadataUnavailable,
    DeletedDirectory,
    UnsupportedType,
    VirtualDirectory,
    DirectoryCycle,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeferredEntry {
    pub entry_index: u64,
    pub inode: Option<u64>,
    pub reason: DeferredReason,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DirectoryCounters {
    pub requests: u64,
    pub requested_bytes: u64,
    pub delivered_bytes: u64,
    pub allocation_checks: u64,
    pub entries_completed: u64,
    pub ignored_entries: u64,
    pub max_rss_bytes: Option<u64>,
}
pub const NAMESPACE_POLICY: &str = "allocated-reachable-native-names-v1";
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NamespaceExclusions {
    pub virtual_files: u64,
    pub virtual_directories: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchStopReason {
    DirectoryComplete,
    EntryLimit,
    RunLimit,
    AllocationBudget,
}
#[derive(Debug, Clone)]
pub struct DirectoryBatch {
    geometry: FilesystemGeometry,
    directory_inode: u64,
    start_entry: u64,
    total_entries: u64,
    next_entry: u64,
    directory_complete: bool,
    files: Vec<DirectoryFile>,
    children: Vec<ChildDirectory>,
    deferred: Vec<DeferredEntry>,
    counters: DirectoryCounters,
    exclusions: NamespaceExclusions,
    run_limited: bool,
    allocation_limited: bool,
}
impl DirectoryBatch {
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_parts(
        geometry: FilesystemGeometry,
        directory_inode: u64,
        start_entry: u64,
        total_entries: u64,
        next_entry: u64,
        directory_complete: bool,
        files: Vec<DirectoryFile>,
        children: Vec<ChildDirectory>,
        deferred: Vec<DeferredEntry>,
        counters: DirectoryCounters,
    ) -> Self {
        Self {
            geometry,
            directory_inode,
            start_entry,
            total_entries,
            next_entry,
            directory_complete,
            files,
            children,
            deferred,
            counters,
            exclusions: NamespaceExclusions::default(),
            run_limited: false,
            allocation_limited: false,
        }
    }
    pub fn geometry(&self) -> &FilesystemGeometry {
        &self.geometry
    }
    pub fn directory_inode(&self) -> u64 {
        self.directory_inode
    }
    pub fn start_entry(&self) -> u64 {
        self.start_entry
    }
    pub fn total_entries(&self) -> u64 {
        self.total_entries
    }
    pub fn next_entry(&self) -> u64 {
        self.next_entry
    }
    pub fn directory_complete(&self) -> bool {
        self.directory_complete
    }
    pub fn files(&self) -> &[DirectoryFile] {
        &self.files
    }
    pub fn children(&self) -> &[ChildDirectory] {
        &self.children
    }
    pub fn deferred(&self) -> &[DeferredEntry] {
        &self.deferred
    }
    pub fn counters(&self) -> &DirectoryCounters {
        &self.counters
    }
    pub fn namespace_policy(&self) -> &'static str {
        NAMESPACE_POLICY
    }
    pub fn exclusions(&self) -> &NamespaceExclusions {
        &self.exclusions
    }
    pub fn ignored_virtual_entries(&self) -> u64 {
        self.exclusions.virtual_files + self.exclusions.virtual_directories
    }
    pub fn stop_reason(&self) -> BatchStopReason {
        if self.directory_complete {
            BatchStopReason::DirectoryComplete
        } else if self.allocation_limited {
            BatchStopReason::AllocationBudget
        } else if self.run_limited {
            BatchStopReason::RunLimit
        } else {
            BatchStopReason::EntryLimit
        }
    }
    /// Semantic digest excludes variable resource observations on retry.
    pub fn digest(&self) -> [u8; 32] {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(b"FrameHarbor directory batch v1\0");
        h.update(NAMESPACE_POLICY.as_bytes());
        h.update(self.exclusions.virtual_files.to_be_bytes());
        h.update(self.exclusions.virtual_directories.to_be_bytes());
        h.update(self.counters.ignored_entries.to_be_bytes());
        h.update([u8::from(self.run_limited)]);
        h.update([u8::from(self.allocation_limited)]);
        let numbers = |h: &mut Sha256, values: &[u64]| {
            for n in values {
                h.update(n.to_be_bytes());
            }
        };
        let text = |h: &mut Sha256, s: &str| {
            h.update((s.len() as u64).to_be_bytes());
            h.update(s.as_bytes());
        };
        numbers(
            &mut h,
            &[
                match self.geometry.profile {
                    FilesystemProfile::Fat32 => 0,
                    FilesystemProfile::ExFat => 1,
                },
                self.geometry.block_size,
                self.geometry.block_count,
                self.geometry.root_inode,
                self.geometry.start_in_image,
                self.directory_inode,
                self.start_entry,
                self.total_entries,
                self.next_entry,
                u64::from(self.directory_complete),
                self.files.len() as u64,
            ],
        );
        for file in &self.files {
            let v = &file.view;
            numbers(&mut h, &[file.entry_index]);
            for s in [
                &v.owner.source.0,
                &v.owner.filesystem,
                &v.owner.file_identity,
            ] {
                text(&mut h, s);
            }
            numbers(
                &mut h,
                &[
                    v.size,
                    v.metadata_flags,
                    v.name_flags,
                    v.attribute_flags,
                    v.initialized_size,
                    v.skip_length,
                    match v.evidence {
                        ExtentEvidence::MetadataAllocated => 0,
                        ExtentEvidence::MetadataCandidate => 1,
                        ExtentEvidence::DeletedOrRecovery => 2,
                        ExtentEvidence::SharedGeometry => 3,
                        ExtentEvidence::Partial => 4,
                        ExtentEvidence::Unsupported => 5,
                    },
                    v.runs.len() as u64,
                ],
            );
            for run in &v.runs {
                numbers(
                    &mut h,
                    &[
                        run.logical.start(),
                        run.logical.end(),
                        u64::from(run.image.is_some()),
                        run.image.map_or(0, |i| i.start()),
                        run.image.map_or(0, |i| i.end()),
                        run.raw_flags,
                        u64::from(run.allocation_consistent),
                    ],
                );
            }
            numbers(&mut h, &[file.unchecked_runs.len() as u64]);
            numbers(&mut h, &file.unchecked_runs);
        }
        numbers(&mut h, &[self.children.len() as u64]);
        for child in &self.children {
            numbers(
                &mut h,
                &[
                    child.entry_index,
                    child.inode,
                    child.name_flags,
                    child.metadata_flags,
                ],
            );
        }
        numbers(&mut h, &[self.deferred.len() as u64]);
        for d in &self.deferred {
            numbers(
                &mut h,
                &[
                    d.entry_index,
                    u64::from(d.inode.is_some()),
                    d.inode.unwrap_or(0),
                    match d.reason {
                        DeferredReason::MetadataUnavailable => 0,
                        DeferredReason::DeletedDirectory => 1,
                        DeferredReason::UnsupportedType => 2,
                        DeferredReason::VirtualDirectory => 3,
                        DeferredReason::DirectoryCycle => 4,
                    },
                ],
            );
        }
        h.finalize().into()
    }
}

pub enum DirectoryEvent<'a> {
    BeforeRead {
        requested: ByteRange,
        delivered: ByteRange,
    },
    File(&'a DirectoryFile),
    Child(&'a ChildDirectory),
    Deferred(&'a DeferredEntry),
    EntryComplete {
        directory_inode: u64,
        next_entry: u64,
    },
}

fn numbers(fields: &[&str]) -> Result<Vec<u64>> {
    fields
        .iter()
        .map(|s| s.parse().map_err(|_| FilesystemError::Protocol))
        .collect()
}
/// Return only the maximal finished prefix of a bounded native request. TSK's
/// public image reader accepts a positive short cache fill and never receives
/// fabricated suffix bytes. A hole at the start remains an immediate failure.
fn acquired_prefix(session: &mut CaseSession, range: ByteRange) -> Result<ByteRange> {
    if session.range_available(range)? {
        return Ok(range);
    }
    let mut low = 0u64;
    // The complete range was already rejected, so exclude that length. This
    // keeps the binary search to at most 20 mask probes for a 1 MiB request.
    let mut high = range.len() - 1;
    while low < high {
        let middle = low + (high - low).div_ceil(2);
        let prefix =
            ByteRange::from_len(range.start(), middle).map_err(|_| FilesystemError::Protocol)?;
        if session.range_available(prefix)? {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    if low == 0 {
        // Preserve the precise acquisition error, using a bounded one-byte read
        // that cannot succeed or supply native data at this missing position.
        return session
            .with_read(
                ByteRange::from_len(range.start(), 1).map_err(|_| FilesystemError::Protocol)?,
                |_| (),
            )
            .map(|_| range)
            .map_err(FilesystemError::Session);
    }
    ByteRange::from_len(range.start(), low).map_err(|_| FilesystemError::Protocol)
}

/// Inspect a bounded small namespace through the canonical directory-batch
/// engine. Production discovery uses its durable paged Ledger queue instead.
/// All helper READ positions are image-relative bytes. Only filesystem RUN
/// addresses add `fs_start`; the historical boot partition offset is never added.
pub fn inspect(
    session: &mut CaseSession,
    helper: &Path,
    image_size: u64,
    fs_start: u64,
    profile: FilesystemProfile,
    limits: AdapterLimits,
    cancel: &AtomicBool,
) -> Result<FilesystemReport> {
    inspect_observed(
        session,
        helper,
        image_size,
        fs_start,
        profile,
        limits,
        cancel,
        |_| {},
    )
}
/// The observer receives one actual cleanup result per owned child, including
/// parser, deadline or cancellation failure. Preflight failures spawn no child
/// and emit no lifecycle event. No aggregated child status is synthesized.
#[allow(clippy::too_many_arguments)]
pub fn inspect_observed(
    session: &mut CaseSession,
    helper: &Path,
    image_size: u64,
    fs_start: u64,
    profile: FilesystemProfile,
    limits: AdapterLimits,
    cancel: &AtomicBool,
    cleanup: impl FnMut(ChildCleanup),
) -> Result<FilesystemReport> {
    inspect_progress_observed(
        session,
        helper,
        image_size,
        fs_start,
        profile,
        limits,
        cancel,
        |_, _| Ok(()),
        cleanup,
    )
}
/// Completed records survive later helper/cap failure through this fallible
/// callback. Records remain provisional until full-report conflict qualification.
#[allow(clippy::too_many_arguments)]
pub fn inspect_progress(
    session: &mut CaseSession,
    helper: &Path,
    image_size: u64,
    fs_start: u64,
    profile: FilesystemProfile,
    limits: AdapterLimits,
    cancel: &AtomicBool,
    file: impl FnMut(&mut CaseSession, &LogicalFileView) -> Result<()>,
) -> Result<FilesystemReport> {
    inspect_progress_observed(
        session,
        helper,
        image_size,
        fs_start,
        profile,
        limits,
        cancel,
        file,
        |_| {},
    )
}
#[allow(clippy::too_many_arguments)]
fn inspect_progress_observed(
    session: &mut CaseSession,
    helper: &Path,
    image_size: u64,
    fs_start: u64,
    profile: FilesystemProfile,
    limits: AdapterLimits,
    cancel: &AtomicBool,
    mut file: impl FnMut(&mut CaseSession, &LogicalFileView) -> Result<()>,
    mut cleanup: impl FnMut(ChildCleanup),
) -> Result<FilesystemReport> {
    // Small-namespace diagnostics use the same direct-cursor engine as Runner.
    // Production keeps its cursor/geometry queue in the Ledger instead.
    let started = Instant::now();
    let mut pending = std::collections::VecDeque::from([DirectoryCursor {
        directory_inode: None,
        next_entry: 0,
    }]);
    let mut seen = std::collections::BTreeSet::new();
    let mut files = Vec::new();
    let mut runs = 0usize;
    let mut requested = 0u64;
    let mut reads = 0u64;
    let mut delivered = 0u64;
    let mut max_rss = None::<u64>;
    let mut geometry = None::<FilesystemGeometry>;
    while let Some(cursor) = pending.pop_front() {
        let mut bounded = limits;
        bounded.wall = limits.wall.saturating_sub(started.elapsed());
        if bounded.wall.is_zero() {
            return Err(FilesystemError::TimedOut);
        }
        bounded.total_read_bytes = limits.total_read_bytes.saturating_sub(requested);
        let batch = inspect_directory_batch_observed(
            session,
            helper,
            image_size,
            fs_start,
            profile,
            cursor,
            DirectoryBatchLimits {
                adapter: bounded,
                entries: 256,
            },
            cancel,
            |case, event| {
                match event {
                    DirectoryEvent::BeforeRead {
                        requested: range, ..
                    } => {
                        requested = requested
                            .checked_add(range.len())
                            .filter(|n| *n <= limits.total_read_bytes)
                            .ok_or(FilesystemError::ResourceLimit)?;
                    }
                    DirectoryEvent::File(observed) => {
                        if files.len() >= limits.files {
                            return Err(FilesystemError::ResourceLimit);
                        }
                        runs = runs
                            .checked_add(observed.view.runs.len())
                            .filter(|n| *n <= limits.runs)
                            .ok_or(FilesystemError::ResourceLimit)?;
                        file(case, &observed.view)?;
                        files.push(observed.view.clone());
                    }
                    _ => {}
                }
                Ok(())
            },
            &mut cleanup,
        )?;
        if let Some(g) = &geometry {
            if *g != *batch.geometry() {
                return Err(FilesystemError::Protocol);
            }
        } else {
            geometry = Some(batch.geometry().clone());
            seen.insert(batch.directory_inode());
        }
        reads = reads
            .checked_add(batch.counters().requests)
            .ok_or(FilesystemError::ResourceLimit)?;
        delivered = delivered
            .checked_add(batch.counters().delivered_bytes)
            .ok_or(FilesystemError::ResourceLimit)?;
        if let Some(rss) = batch.counters().max_rss_bytes {
            max_rss = Some(max_rss.unwrap_or(0).max(rss));
        }
        if !batch.deferred().is_empty() {
            return Err(FilesystemError::Unresolved);
        }
        for child in batch.children() {
            if !seen.insert(child.inode) {
                return Err(FilesystemError::Unresolved);
            }
            if seen.len() > 1024 {
                return Err(FilesystemError::ResourceLimit);
            }
            pending.push_back(DirectoryCursor {
                directory_inode: Some(child.inode),
                next_entry: 0,
            });
        }
        if !batch.directory_complete() {
            pending.push_front(DirectoryCursor {
                directory_inode: Some(batch.directory_inode()),
                next_entry: batch.next_entry(),
            });
        }
    }
    for view in &mut files {
        classify(view);
    }
    qualify_conflicts(&mut files);
    Ok(FilesystemReport {
        geometry: geometry.ok_or(FilesystemError::Protocol)?,
        files,
        callback_reads: reads,
        callback_requested_bytes: requested,
        callback_bytes: delivered,
        max_rss_bytes: max_rss,
        helper_version: "4.14.0".into(),
    })
}
#[allow(clippy::too_many_arguments)]
pub fn inspect_directory_batch(
    session: &mut CaseSession,
    helper: &Path,
    image_size: u64,
    fs_start: u64,
    profile: FilesystemProfile,
    cursor: DirectoryCursor,
    limits: DirectoryBatchLimits,
    cancel: &AtomicBool,
    mut event: impl FnMut(&mut CaseSession, DirectoryEvent<'_>) -> Result<()>,
) -> Result<DirectoryBatch> {
    inspect_directory_batch_observed(
        session,
        helper,
        image_size,
        fs_start,
        profile,
        cursor,
        limits,
        cancel,
        &mut event,
        |_| {},
    )
}
#[allow(clippy::too_many_arguments)]
pub fn inspect_directory_batch_observed(
    session: &mut CaseSession,
    helper: &Path,
    image_size: u64,
    fs_start: u64,
    profile: FilesystemProfile,
    cursor: DirectoryCursor,
    limits: DirectoryBatchLimits,
    cancel: &AtomicBool,
    mut event: impl FnMut(&mut CaseSession, DirectoryEvent<'_>) -> Result<()>,
    cleanup: impl FnOnce(ChildCleanup),
) -> Result<DirectoryBatch> {
    if limits.entries == 0
        || limits.entries > 1024
        || cursor.next_entry > 1000000
        || cursor.directory_inode.is_none() && cursor.next_entry != 0
    {
        return Err(FilesystemError::ResourceLimit);
    }
    let mut cleaned = false;
    let result = inspect_batch_inner(
        session,
        helper,
        image_size,
        fs_start,
        profile,
        limits.adapter,
        cancel,
        |receipt| {
            cleaned = true;
            cleanup(receipt)
        },
        cursor,
        limits.entries,
        &mut event,
    );
    if cleaned {
        let settled = session.admit_external("filesystem-helper-exit", 0);
        if result.is_ok() {
            settled?;
        }
    }
    result
}
#[allow(clippy::too_many_arguments)]
fn inspect_batch_inner(
    session: &mut CaseSession,
    helper: &Path,
    image_size: u64,
    fs_start: u64,
    profile: FilesystemProfile,
    limits: AdapterLimits,
    cancel: &AtomicBool,
    cleanup: impl FnOnce(ChildCleanup),
    cursor: DirectoryCursor,
    entries: usize,
    observer: &mut dyn FnMut(&mut CaseSession, DirectoryEvent<'_>) -> Result<()>,
) -> Result<DirectoryBatch> {
    if image_size != session.image_size() {
        return Err(FilesystemError::SourceMismatch);
    }
    if image_size == 0
        || fs_start >= image_size
        || limits.wall.is_zero()
        || limits.wall > Duration::from_secs(120)
        || limits.callback_bytes == 0
        || limits.callback_bytes > 1024 * 1024
        || limits.total_read_bytes > 1024 * 1024 * 1024
        || limits.files == 0
        || limits.files > 1024
        || limits.runs == 0
        || limits.runs > 1024
        || limits.output_bytes == 0
        || limits.output_bytes > 4 * 1024 * 1024
        || limits.helper_memory_bytes < 256 * 1024 * 1024
        || limits.helper_memory_bytes > 512 * 1024 * 1024
    {
        return Err(FilesystemError::ResourceLimit);
    }
    // A BeforeRead callback may flush discovery archives/checkpoints while the
    // native child and parser buffers remain resident. Reserve that overlap now.
    let memory = (2*1024*1024+entries as u64*1024+limits.runs as u64*256) + limits.helper_memory_bytes
        + 4 * 1024 * 1024 // two Rust supervisor thread stacks
        + 3 * limits.callback_bytes
        + limits.files as u64 * 512
        + limits.runs as u64 * 256
        + limits.output_bytes as u64;
    let started = Instant::now();
    let remaining = session.admit_external("filesystem-helper", memory)?;
    let allowance = limits.wall.min(remaining);
    check_control(cancel, started, allowance)?;
    let mut command = Command::new(helper);
    command
        .arg(image_size.to_string())
        .arg(fs_start.to_string())
        .arg(profile.arg());
    {
        command
            .arg("batch")
            .arg(
                cursor
                    .directory_inode
                    .map_or_else(|| "root".into(), |n| n.to_string()),
            )
            .arg(cursor.next_entry.to_string())
            .arg(entries.to_string())
            .arg(limits.runs.to_string());
    }
    let (mut worker, events) = supervision::spawn(&mut command, limits.output_bytes, cleanup)?;
    let mut hello = false;
    let mut geometry = None::<FilesystemGeometry>;
    let mut emitted_files = 0u64;
    let mut current: Option<(LogicalFileView, usize)> = None;
    let mut reads = 0u64;
    let mut read_bytes = 0u64;
    let mut requested_bytes = 0u64;
    let mut total_runs = 0usize;
    let mut max_rss = None;
    let mut directory = None::<(u64, u64, u64)>;
    let mut entry = None::<u64>;
    let mut entry_handled = false;
    let mut next_entry = cursor.next_entry;
    let mut batch_ended = false;
    let mut run_limited = false;
    let mut allocation_limited = false;
    let mut batch_files = Vec::new();
    let mut children = Vec::new();
    let mut deferred = Vec::new();
    let mut unchecked_runs = Vec::new();
    let mut ignored_entries = 0u64;
    let mut exclusions = NamespaceExclusions::default();
    let mut allocation_checks = 0;
    loop {
        check_control(cancel, started, allowance)?;
        // Parser buffers and protocol volume were reserved before spawn. Guard
        // cached CPU work without a durable SQLite write per protocol record;
        // actual reads/archives and final elapsed settlement remain admitted.
        session.check_source()?;
        let event = match events.recv_timeout(Duration::from_millis(10)) {
            Ok(v) => v,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => return Err(FilesystemError::NativeFailure),
        };
        let line = match event {
            Event::Line(v) => v,
            Event::Invalid => return Err(FilesystemError::ResourceLimit),
            Event::Io => return Err(FilesystemError::NativeFailure),
            Event::End => return Err(FilesystemError::NativeFailure),
        };
        let text = std::str::from_utf8(&line).map_err(|_| FilesystemError::Protocol)?;
        let fields = text.split_ascii_whitespace().collect::<Vec<_>>();
        let Some(command) = fields.first() else {
            return Err(FilesystemError::Protocol);
        };
        if !hello && *command != "HELLO" {
            return Err(FilesystemError::Protocol);
        }
        match *command {
            "DIRECTORY" => {
                if fields.len() != 4 || directory.is_some() || geometry.is_none() {
                    return Err(FilesystemError::Protocol);
                }
                let n = numbers(&fields[1..])?;
                let geo = geometry.as_ref().ok_or(FilesystemError::Protocol)?;
                if n[0] != cursor.directory_inode.unwrap_or(geo.root_inode)
                    || n[1] > 1000000
                    || n[2] != cursor.next_entry
                    || n[2] > n[1]
                {
                    return Err(FilesystemError::Protocol);
                }
                directory = Some((n[0], n[1], n[2]));
            }
            "ENTRY" => {
                let cap = entries;
                let (_, total, start) = directory.ok_or(FilesystemError::Protocol)?;
                if fields.len() != 2 || entry.is_some() || current.is_some() || batch_ended {
                    return Err(FilesystemError::Protocol);
                }
                let n = numbers(&fields[1..])?[0];
                if n != next_entry || n >= total || n - start >= cap as u64 {
                    return Err(FilesystemError::Protocol);
                }
                entry = Some(n);
                entry_handled = false;
            }
            "CHILD" => {
                if fields.len() != 4 || entry_handled || current.is_some() {
                    return Err(FilesystemError::Protocol);
                }
                let n = numbers(&fields[1..])?;
                if n[1] != 1 || n[2] & 1 == 0 || n[2] & 2 != 0 {
                    return Err(FilesystemError::Protocol);
                }
                let child = ChildDirectory {
                    entry_index: entry.ok_or(FilesystemError::Protocol)?,
                    inode: n[0],
                    name_flags: n[1],
                    metadata_flags: n[2],
                };
                observer(session, DirectoryEvent::Child(&child))?;
                children.push(child);
                entry_handled = true;
            }
            "DEFERRED" => {
                if fields.len() != 3 || entry_handled || current.is_some() {
                    return Err(FilesystemError::Protocol);
                }
                let n = numbers(&fields[1..])?;
                let reason = match n[1] {
                    0 => DeferredReason::MetadataUnavailable,
                    1 => DeferredReason::DeletedDirectory,
                    2 => DeferredReason::UnsupportedType,
                    3 => DeferredReason::VirtualDirectory,
                    _ => return Err(FilesystemError::Protocol),
                };
                let item = DeferredEntry {
                    entry_index: entry.ok_or(FilesystemError::Protocol)?,
                    inode: Some(n[0]),
                    reason,
                };
                observer(session, DirectoryEvent::Deferred(&item))?;
                deferred.push(item);
                entry_handled = true;
            }
            "IGNORED" => {
                if fields.len() != 1 || entry.is_none() || entry_handled || current.is_some() {
                    return Err(FilesystemError::Protocol);
                }
                ignored_entries += 1;
                entry_handled = true;
            }
            "EXCLUDED" => {
                if fields.len() != 2 || entry.is_none() || entry_handled || current.is_some() {
                    return Err(FilesystemError::Protocol);
                }
                match numbers(&fields[1..])?[0] {
                    0 => exclusions.virtual_files += 1,
                    1 => exclusions.virtual_directories += 1,
                    _ => return Err(FilesystemError::Protocol),
                }
                entry_handled = true;
            }
            "ENTRY_END" => {
                if fields.len() != 2 || !entry_handled || current.is_some() {
                    return Err(FilesystemError::Protocol);
                }
                let old = entry.take().ok_or(FilesystemError::Protocol)?;
                let next = numbers(&fields[1..])?[0];
                if old.checked_add(1) != Some(next) {
                    return Err(FilesystemError::Protocol);
                }
                next_entry = next;
                observer(
                    session,
                    DirectoryEvent::EntryComplete {
                        directory_inode: directory.ok_or(FilesystemError::Protocol)?.0,
                        next_entry,
                    },
                )?;
            }
            "BATCH_END" => {
                let cap = entries;
                let (_, total, start) = directory.ok_or(FilesystemError::Protocol)?;
                if fields.len() != 5 || batch_ended || entry.is_some() || current.is_some() {
                    return Err(FilesystemError::Protocol);
                }
                let n = numbers(&fields[1..])?;
                if n[0] != next_entry
                    || n[0] > total.min(start + cap as u64)
                    || n[1] != u64::from(next_entry == total)
                    || n[2] > 65536
                {
                    return Err(FilesystemError::Protocol);
                }
                match n[3] {
                    0 if next_entry == total => {}
                    1 if next_entry < total && next_entry == start + cap as u64 => {}
                    2 if next_entry < total && next_entry < start + cap as u64 => {
                        if next_entry == start {
                            return Err(FilesystemError::ResourceLimit);
                        }
                        run_limited = true;
                    }
                    3 if next_entry < total && next_entry < start + cap as u64 => {
                        if next_entry == start {
                            return Err(FilesystemError::ResourceLimit);
                        }
                        allocation_limited = true;
                    }
                    _ => return Err(FilesystemError::Protocol),
                }
                allocation_checks = n[2];
                batch_ended = true;
            }
            "HELLO" => {
                if hello || fields.len() != 3 || fields[1] != "4.14.0" || fields[2] != "4.14.0" {
                    return Err(FilesystemError::VersionMismatch);
                }
                hello = true;
            }
            "READ" => {
                if fields.len() != 3 {
                    return Err(FilesystemError::Protocol);
                }
                let n = numbers(&fields[1..])?;
                let range =
                    ByteRange::from_len(n[0], n[1]).map_err(|_| FilesystemError::Protocol)?;
                if range.is_empty()
                    || range.end() > image_size
                    || range.len() > limits.callback_bytes
                {
                    return Err(FilesystemError::ResourceLimit);
                }
                requested_bytes = requested_bytes
                    .checked_add(range.len())
                    .filter(|n| *n <= limits.total_read_bytes)
                    .ok_or(FilesystemError::ResourceLimit)?;
                reads += 1;
                session.check_source()?;
                let delivered = acquired_prefix(session, range)?;
                observer(
                    session,
                    DirectoryEvent::BeforeRead {
                        requested: range,
                        delivered,
                    },
                )?;
                read_bytes = read_bytes
                    .checked_add(delivered.len())
                    .ok_or(FilesystemError::ResourceLimit)?;
                let response = session.with_read(delivered, |bytes| {
                    let mut response = format!("OK {}\n", bytes.len()).into_bytes();
                    response.extend_from_slice(bytes);
                    response
                })?;
                send(&worker, response, cancel, started, allowance)?;
            }
            "GEOMETRY" => {
                if geometry.is_some() || current.is_some() || fields.len() != 5 {
                    return Err(FilesystemError::Protocol);
                }
                let n = numbers(&fields[1..])?;
                if n[0] != profile.tsk_type()
                    || !n[1].is_power_of_two()
                    || n[1] < 512
                    || n[1] > 128 * 1024
                    || n[2] == 0
                    || n[1]
                        .checked_mul(n[2])
                        .and_then(|v| v.checked_add(fs_start))
                        .is_none_or(|end| end > image_size)
                {
                    return Err(FilesystemError::Protocol);
                }
                geometry = Some(FilesystemGeometry {
                    profile,
                    block_size: n[1],
                    block_count: n[2],
                    root_inode: n[3],
                    start_in_image: fs_start,
                });
            }
            "FILE" => {
                if entry.is_none() || entry_handled || batch_ended {
                    return Err(FilesystemError::Protocol);
                }
                unchecked_runs.clear();
                if geometry.is_none() || current.is_some() || fields.len() != 9 {
                    return Err(FilesystemError::Protocol);
                }
                if emitted_files >= limits.files as u64 {
                    return Err(FilesystemError::ResourceLimit);
                }
                let n = numbers(&fields[1..])?;
                if n[5] as usize > limits.runs || n[6] > n[1] {
                    return Err(FilesystemError::ResourceLimit);
                }
                let owner = LogicalOwner {
                    source: session.source_generation(),
                    filesystem: format!("{}:{fs_start}", profile.arg()),
                    file_identity: n[0].to_string(),
                };
                current = Some((
                    LogicalFileView {
                        owner,
                        size: n[1],
                        metadata_flags: n[2],
                        name_flags: n[3],
                        attribute_flags: n[4],
                        initialized_size: n[6],
                        skip_length: n[7],
                        runs: Vec::new(),
                        evidence: ExtentEvidence::MetadataCandidate,
                    },
                    n[5] as usize,
                ));
            }
            "RUN" => {
                if fields.len() != 5 && fields.len() != 6 {
                    return Err(FilesystemError::Protocol);
                }
                let n = numbers(&fields[1..])?;
                if n.get(4).is_some_and(|v| *v > 2) {
                    return Err(FilesystemError::Protocol);
                }
                let geo = geometry.as_ref().ok_or(FilesystemError::Protocol)?;
                let (view, count) = current.as_mut().ok_or(FilesystemError::Protocol)?;
                if view.runs.len() >= *count || total_runs >= limits.runs || n[2] == 0 {
                    return Err(FilesystemError::ResourceLimit);
                }
                total_runs += 1;
                let begin = n[0]
                    .checked_mul(geo.block_size)
                    .ok_or(FilesystemError::Protocol)?;
                let length = n[2]
                    .checked_mul(geo.block_size)
                    .ok_or(FilesystemError::Protocol)?;
                let end = begin.checked_add(length).ok_or(FilesystemError::Protocol)?;
                let skipped = view.skip_length;
                if end <= skipped {
                    continue;
                }
                let clipped_begin = begin.max(skipped);
                let logical_start = clipped_begin - skipped;
                let logical_end = (end - skipped).min(view.size);
                if logical_start >= logical_end {
                    continue;
                }
                let logical = ByteRange::new(logical_start, logical_end)
                    .map_err(|_| FilesystemError::Protocol)?;
                let image = if n[3] == 0 {
                    let start = n[1]
                        .checked_mul(geo.block_size)
                        .and_then(|v| v.checked_add(fs_start))
                        .and_then(|v| v.checked_add(clipped_begin - begin))
                        .ok_or(FilesystemError::Protocol)?;
                    let range = ByteRange::from_len(start, logical.len())
                        .map_err(|_| FilesystemError::Protocol)?;
                    if range.end() > image_size
                        || n[1].checked_add(n[2]).is_none_or(|v| v > geo.block_count)
                    {
                        return Err(FilesystemError::Protocol);
                    }
                    Some(range)
                } else {
                    None
                };
                if n.get(4) == Some(&2) {
                    unchecked_runs.push(view.runs.len() as u64);
                }
                view.runs.push(ExtentRun {
                    logical,
                    image,
                    raw_flags: n[3],
                    allocation_consistent: n.get(4) == Some(&1),
                });
            }
            "END_FILE" => {
                if fields.len() != 1 {
                    return Err(FilesystemError::Protocol);
                }
                let (mut view, expected) = current.take().ok_or(FilesystemError::Protocol)?;
                if view.runs.len() != expected {
                    return Err(FilesystemError::Protocol);
                }
                classify(&mut view);
                if view.evidence == ExtentEvidence::MetadataAllocated {
                    view.evidence = ExtentEvidence::MetadataCandidate;
                }
                let observed = DirectoryFile {
                    entry_index: entry.ok_or(FilesystemError::Protocol)?,
                    view,
                    unchecked_runs: std::mem::take(&mut unchecked_runs),
                };
                observer(session, DirectoryEvent::File(&observed))?;
                batch_files.push(observed);
                entry_handled = true;
                emitted_files += 1;
            }
            "METRICS" => {
                if fields.len() != 2 || max_rss.is_some() {
                    return Err(FilesystemError::Protocol);
                }
                let observed = numbers(&fields[1..])?[0];
                if observed > limits.helper_memory_bytes {
                    return Err(FilesystemError::ResourceLimit);
                }
                max_rss = Some(observed);
            }
            "ERROR" => {
                if fields.len() != 2 {
                    return Err(FilesystemError::Protocol);
                }
                return Err(FilesystemError::NativeError(numbers(&fields[1..])?[0]));
            }
            "DONE" => {
                if fields.len() != 3 || current.is_some() || !batch_ended || entry.is_some() {
                    return Err(FilesystemError::Protocol);
                }
                let n = numbers(&fields[1..])?;
                if n[0] != emitted_files || n[1] != reads {
                    return Err(FilesystemError::Protocol);
                }
                loop {
                    check_control(cancel, started, allowance)?;
                    match worker.child.try_wait()? {
                        Some(status) if status.success() => break,
                        Some(_) => return Err(FilesystemError::NativeFailure),
                        None => thread::sleep(Duration::from_millis(2)),
                    }
                }
                break;
            }
            _ => return Err(FilesystemError::Protocol),
        }
    }
    let geometry = geometry.ok_or(FilesystemError::Protocol)?;
    let (inode, total, start) = directory.ok_or(FilesystemError::Protocol)?;
    Ok(DirectoryBatch {
        geometry,
        directory_inode: inode,
        start_entry: start,
        total_entries: total,
        next_entry,
        directory_complete: next_entry == total,
        files: batch_files,
        children,
        deferred,
        exclusions,
        run_limited,
        allocation_limited,
        counters: DirectoryCounters {
            requests: reads,
            requested_bytes,
            delivered_bytes: read_bytes,
            allocation_checks,
            entries_completed: next_entry - start,
            ignored_entries,
            max_rss_bytes: max_rss,
        },
    })
}

pub(crate) fn classify(view: &mut LogicalFileView) {
    view.evidence = if view.metadata_flags & 2 != 0
        || view.name_flags & 2 != 0
        || view.attribute_flags & 0x80 != 0
    {
        ExtentEvidence::DeletedOrRecovery
    // TSK 4.14.0: META_ALLOC=0x01, META_USED=0x04; positive ALLOC
    // evidence is mandatory. Absence of UNALLOC is not allocation evidence.
    } else if view.metadata_flags & 1 == 0
        || view.metadata_flags & !5 != 0
        || view.name_flags != 1
        || view.attribute_flags != 3
        || view.skip_length != 0
        || view.runs.iter().any(|r| r.raw_flags != 0)
    {
        ExtentEvidence::Unsupported
    } else {
        let mut cursor = 0;
        let mut complete = view.initialized_size == view.size;
        for run in &view.runs {
            complete &= run.logical.start() == cursor && run.image.is_some();
            cursor = run.logical.end();
        }
        complete &= cursor == view.size;
        if !complete {
            ExtentEvidence::Partial
        } else if view.runs.iter().all(|r| r.allocation_consistent) {
            ExtentEvidence::MetadataAllocated
        } else {
            ExtentEvidence::MetadataCandidate
        }
    };
}
fn qualify_conflicts(files: &mut [LogicalFileView]) {
    let mut spans = files
        .iter()
        .enumerate()
        .flat_map(|(owner, file)| {
            file.runs
                .iter()
                .filter_map(move |run| run.image.map(|r| (r, owner)))
        })
        .collect::<Vec<_>>();
    spans.sort_by_key(|(r, _)| (r.start(), r.end()));
    let mut maximum: Option<(u64, usize)> = None;
    for (range, owner) in spans {
        if let Some((end, previous)) = maximum {
            if range.start() < end {
                files[owner].evidence = ExtentEvidence::SharedGeometry;
                files[previous].evidence = ExtentEvidence::SharedGeometry;
            }
            if range.end() > end {
                maximum = Some((range.end(), owner));
            }
        } else {
            maximum = Some((range.end(), owner));
        }
    }
}
