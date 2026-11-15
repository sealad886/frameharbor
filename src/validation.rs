//! Selected, quota-bound materialization and independent staged-artifact checks.
//! Native decoding receives bytes through metered seekable IPC, never a path.
pub use crate::supervision::ChildCleanup;
use crate::{
    acquisition::{Identity, SourceManifest, identity},
    bmff::{self, BmffError, LogicalReader, ParseLimits, ReadAt, SampleMap},
    contracts::{
        ByteProvenance, ByteRange, Claims, Coverage, Decode, LogicalOwner, Ownership, ResourceCaps,
        SampleVerification, Structure, Temporal,
    },
    filesystem::{ExtentEvidence, LogicalFileView},
    session::{CaseSession, SessionError},
    supervision::{self, Event, SupervisionError, check_control, send},
};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
const OWNER: &[u8] = b"FrameHarbor selected media staging v1\n";
const BUFFER: usize = 65536;
const FMT: u64 = (63 << 16) | (1 << 8) | 102;
const UTIL: u64 = (61 << 16) | (1 << 8) | 102;
const STRICT: u64 = 65551;
#[derive(Debug)]
pub enum ValidationError {
    Io(io::Error),
    Session(SessionError),
    Bmff(BmffError),
    Protocol,
    VersionMismatch,
    ResourceLimit,
    Unowned,
    Changed,
    SampleMismatch(&'static str),
    Unsupported,
    Cancelled,
    TimedOut,
    NativeFailure,
    NativeError {
        phase: u64,
        code: i64,
        warnings: u64,
        errors: u64,
    },
}
impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for ValidationError {}
impl From<io::Error> for ValidationError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<SessionError> for ValidationError {
    fn from(e: SessionError) -> Self {
        Self::Session(e)
    }
}
impl From<BmffError> for ValidationError {
    fn from(e: BmffError) -> Self {
        Self::Bmff(e)
    }
}
impl From<SupervisionError> for ValidationError {
    fn from(e: SupervisionError) -> Self {
        match e {
            SupervisionError::Io(e) => Self::Io(e),
            SupervisionError::Protocol => Self::Protocol,
            SupervisionError::ResourceLimit => Self::ResourceLimit,
            SupervisionError::Cancelled => Self::Cancelled,
            SupervisionError::TimedOut => Self::TimedOut,
            SupervisionError::NativeFailure => Self::NativeFailure,
        }
    }
}
pub type Result<T> = std::result::Result<T, ValidationError>;

#[derive(Clone, Copy, Debug)]
pub struct ValidationLimits {
    pub artifact_bytes: u64,
    pub read_bytes: u64,
    pub protocol_bytes: usize,
    pub wall: Duration,
    pub helper_memory_bytes: u64,
    pub parse: ParseLimits,
}
impl Default for ValidationLimits {
    fn default() -> Self {
        Self {
            artifact_bytes: 64 * 1024 * 1024,
            read_bytes: 512 * 1024 * 1024,
            protocol_bytes: 16 * 1024 * 1024,
            wall: Duration::from_secs(60),
            helper_memory_bytes: 256 * 1024 * 1024,
            parse: ParseLimits::default(),
        }
    }
}
impl ValidationLimits {
    fn check(self) -> Result<Self> {
        if self.parse.memory_bytes == 0
            || self.parse.memory_bytes > 1024 * 1024 * 1024
            || self.artifact_bytes == 0
            || self.artifact_bytes > 512 * 1024 * 1024
            || self.read_bytes == 0
            || self.read_bytes > 4 * 1024 * 1024 * 1024
            || self.protocol_bytes == 0
            || self.protocol_bytes > 64 * 1024 * 1024
            || self.wall.is_zero()
            || self.wall > Duration::from_secs(120)
            || self.helper_memory_bytes < 256 * 1024 * 1024
            || self.helper_memory_bytes > 1024 * 1024 * 1024
        {
            return Err(ValidationError::ResourceLimit);
        }
        Ok(self)
    }
}
fn resources(read: u64, memory: u64, output: u64, decoder: u64) -> ResourceCaps {
    ResourceCaps {
        read_bytes: read,
        memory_bytes: memory,
        index_bytes: 0,
        output_bytes: output,
        visited_postings: 0,
        transitions: 0,
        decoder_seconds: decoder,
    }
}

pub struct StagingArea {
    root: PathBuf,
    quota: u64,
    artifacts: usize,
}
impl StagingArea {
    pub fn create(root: &Path, quota: u64, artifacts: usize) -> Result<Self> {
        if quota == 0 || artifacts == 0 || artifacts > 1024 {
            return Err(ValidationError::ResourceLimit);
        }
        fs::create_dir(root)?;
        let mut owner = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(root.join("OWNER"))?;
        owner.write_all(OWNER)?;
        owner.sync_all()?;
        File::open(root)?.sync_all()?;
        if let Some(parent) = root.parent() {
            File::open(parent)?.sync_all()?;
        }
        Ok(Self {
            root: root.into(),
            quota,
            artifacts,
        })
    }
    fn reserve(&self, size: u64) -> Result<()> {
        let marker = self.root.join("OWNER");
        if !fs::symlink_metadata(&marker)?.file_type().is_file() {
            return Err(ValidationError::Unowned);
        }
        let mut marker_bytes = Vec::new();
        File::open(&marker)?
            .take(OWNER.len() as u64 + 1)
            .read_to_end(&mut marker_bytes)?;
        if !fs::symlink_metadata(&self.root)?.file_type().is_dir() || marker_bytes != OWNER {
            return Err(ValidationError::Unowned);
        }
        let mut used = 0u64;
        let mut count = 0usize;
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            if entry.file_name() == "OWNER" || entry.file_name() == ".DS_Store" {
                continue;
            }
            count += 1;
            if count >= self.artifacts {
                return Err(ValidationError::ResourceLimit);
            }
            let m = fs::symlink_metadata(entry.path())?;
            if !m.is_file() {
                return Err(ValidationError::Unowned);
            }
            used = used
                .checked_add(m.len())
                .ok_or(ValidationError::ResourceLimit)?;
        }
        if used.checked_add(size).is_none_or(|n| n > self.quota) {
            return Err(ValidationError::ResourceLimit);
        }
        Ok(())
    }
    /// Copies an entire supported logical view unchanged. No header rewrite,
    /// donor substitution, framing transform or timestamp normalization occurs.
    pub fn stage_exact(
        &mut self,
        session: &mut CaseSession,
        view: &LogicalFileView,
        expected: SampleMap,
        limits: ValidationLimits,
    ) -> Result<StagedArtifact> {
        let limits = limits.check()?;
        if view.evidence != ExtentEvidence::MetadataAllocated
            || view.owner.source != session.source_generation()
        {
            return Err(ValidationError::Unsupported);
        }
        if view.size == 0
            || view.size != expected.file_size
            || view.size > limits.artifact_bytes
            || expected.tracks.is_empty()
            || expected.tracks.len() > 8
        {
            return Err(ValidationError::ResourceLimit);
        }
        self.reserve(view.size)?;
        session.admit_work(
            "materialization",
            resources(
                0,
                limits.parse.memory_bytes + 3 * BUFFER as u64,
                view.size,
                0,
            ),
        )?;
        {
            let reader = LogicalReader::new(session, view);
            for track in &expected.tracks {
                for sample in &track.samples {
                    if sample.source_spans.is_empty()
                        || reader.source_spans(sample.logical)? != sample.source_spans
                    {
                        return Err(ValidationError::SampleMismatch("sample map"));
                    }
                }
            }
        }
        let path = self.root.join(format!(
            "stage-{}-{}.mp4",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        let mut hash = Sha256::new();
        let mut cursor = 0;
        while cursor < view.size {
            let n = (view.size - cursor).min(BUFFER as u64);
            let range =
                ByteRange::from_len(cursor, n).map_err(|_| ValidationError::ResourceLimit)?;
            view.with_read(session, range, BUFFER as u64, |bytes| -> io::Result<()> {
                output.write_all(bytes)?;
                hash.update(bytes);
                Ok(())
            })
            .map_err(BmffError::from)??;
            cursor += n;
        }
        output.sync_all()?;
        File::open(&self.root)?.sync_all()?;
        let observed = identity(&fs::metadata(&path)?);
        let source_manifest = session.source_snapshot()?;
        Ok(StagedArtifact {
            path,
            bytes: view.size,
            sha256: hash.finalize().into(),
            owner: view.owner.clone(),
            expected,
            identity: observed,
            layout: view.clone(),
            source_manifest,
            transform: ArtifactTransform::OriginalFile,
        })
    }
    /// Materialize a sample-preserving segment from authenticated target tables.
    /// A fresh source parse checks the sealed plan before any output is written.
    pub fn stage_segment(
        &mut self,
        session: &mut CaseSession,
        view: &LogicalFileView,
        plan: &crate::segments::TransformationPlan,
        limits: ValidationLimits,
    ) -> Result<StagedArtifact> {
        let limits = limits.check()?;
        if view.evidence != ExtentEvidence::MetadataAllocated
            || view.owner.source != session.source_generation()
            || plan.owner() != Some(&view.owner)
            || plan.output_bytes() == 0
            || plan.output_bytes() > limits.artifact_bytes
        {
            return Err(ValidationError::Unsupported);
        }
        session.admit_work(
            "segment-plan-verification",
            resources(
                0,
                plan.memory_bound()
                    .checked_add(
                        limits
                            .parse
                            .memory_bytes
                            .checked_mul(3)
                            .ok_or(ValidationError::ResourceLimit)?,
                    )
                    .ok_or(ValidationError::ResourceLimit)?,
                0,
                0,
            ),
        )?;
        let target =
            bmff::inspect_target_layout(&mut LogicalReader::new(session, view), limits.parse)?;
        let evidence = verify_segment_plan(&target, plan)?;
        self.reserve(plan.output_bytes())?;
        session.admit_work(
            "segment-materialization",
            resources(
                0,
                plan.memory_bound() + limits.parse.memory_bytes * 3 + BUFFER as u64,
                plan.output_bytes(),
                0,
            ),
        )?;
        let path = self.root.join(format!(
            "segment-{}-{}.mp4",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        let sha256 = {
            let mut writer = SegmentWriter {
                file: &mut output,
                hash: Sha256::new(),
                bytes: 0,
                limit: plan.output_bytes(),
            };
            crate::segments::write(&mut LogicalReader::new(session, view), plan, &mut writer)?;
            if writer.bytes != plan.output_bytes() {
                return Err(ValidationError::SampleMismatch("segment output size"));
            }
            writer.hash.finalize().into()
        };
        output.sync_all()?;
        File::open(&self.root)?.sync_all()?;
        let observed = identity(&fs::metadata(&path)?);
        let source_manifest = session.source_snapshot()?;
        Ok(StagedArtifact {
            path,
            bytes: plan.output_bytes(),
            sha256,
            owner: view.owner.clone(),
            expected: plan.expected_output().clone(),
            identity: observed,
            layout: view.clone(),
            source_manifest,
            transform: ArtifactTransform::SamplePreserving(evidence),
        })
    }
}

struct SegmentWriter<'a> {
    file: &'a mut File,
    hash: Sha256,
    bytes: u64,
    limit: u64,
}
impl Write for SegmentWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self
            .bytes
            .checked_add(bytes.len() as u64)
            .is_none_or(|n| n > self.limit)
        {
            return Err(io::Error::other("segment output exceeds plan"));
        }
        let n = self.file.write(bytes)?;
        self.hash.update(&bytes[..n]);
        self.bytes += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}
fn verify_segment_plan(
    target: &SampleMap<bmff::LayoutSample>,
    plan: &crate::segments::TransformationPlan,
) -> Result<SegmentEvidence> {
    use crate::segments::TransformLabel;
    crate::segments::verify_audio_selection(target, plan)?;
    let selected = plan.selected_source();
    let output = plan.expected_output();
    let counts: Vec<_> = target
        .tracks
        .iter()
        .map(|t| (t.id, t.samples.len() as u64))
        .collect();
    if counts != plan.source_counts()
        || selected.tracks.is_empty()
        || selected.tracks.len() != output.tracks.len()
        || output.file_size != plan.output_bytes()
    {
        return Err(ValidationError::SampleMismatch("segment source counts"));
    }
    let origins: Vec<_> = plan
        .labels()
        .iter()
        .filter_map(|label| match label {
            TransformLabel::TimestampRebase {
                origin_ticks,
                timescale,
            } => Some((*origin_ticks, *timescale)),
            _ => None,
        })
        .collect();
    if origins.len() != 1
        || origins[0].1 == 0
        || !plan.labels().contains(&TransformLabel::SynthesizedHeaders)
    {
        return Err(ValidationError::SampleMismatch(
            "segment timestamp declaration",
        ));
    }
    let (origin, origin_scale) = origins[0];
    let dropped_edits = target.tracks.iter().any(|t| !t.edits.is_empty());
    if dropped_edits != plan.labels().contains(&TransformLabel::DroppedTargetEdits) {
        return Err(ValidationError::SampleMismatch("segment edit declaration"));
    }
    let mut mappings = Vec::new();
    for (source, staged) in selected.tracks.iter().zip(&output.tracks) {
        let original = target
            .tracks
            .iter()
            .find(|t| t.id == source.id)
            .ok_or(ValidationError::SampleMismatch("segment track identity"))?;
        if source.id != staged.id
            || source.timescale != original.timescale
            || staged.timescale != original.timescale
            || source.handler != original.handler
            || staged.handler != original.handler
            || source.sample_groups != original.sample_groups
            || (original.handler == *b"vide" && !original.sample_groups.is_empty())
            || (original.handler == *b"soun" && !original.sample_groups.is_empty())
            || source.descriptions != original.descriptions
            || staged.descriptions != original.descriptions
            || !staged.edits.is_empty()
            || source.samples.is_empty()
            || source.samples.len() != staged.samples.len()
        {
            return Err(ValidationError::SampleMismatch("segment configuration"));
        }
        let mut previous = None;
        for (n, (sample, out)) in source.samples.iter().zip(&staged.samples).enumerate() {
            if previous.is_some_and(|old| old >= sample.ordinal) {
                return Err(ValidationError::SampleMismatch("segment source order"));
            }
            previous = Some(sample.ordinal);
            let actual = original
                .samples
                .get(usize::try_from(sample.ordinal).map_err(|_| ValidationError::ResourceLimit)?)
                .ok_or(ValidationError::SampleMismatch("segment source ordinal"))?;
            if actual.ordinal != sample.ordinal
                || actual.logical != sample.logical
                || actual.source_spans != sample.source_spans
                || actual.description_index != sample.description_index
                || actual.dts != sample.dts
                || actual.pts != sample.pts
                || actual.duration != sample.duration
                || actual.is_sync != sample.is_sync
                || actual.dependency != sample.dependency
                || actual.availability
                    != (bmff::SampleAvailability::Available {
                        sha256: sample.sha256,
                    })
            {
                return Err(ValidationError::SampleMismatch("segment target evidence"));
            }
            let shift = sample
                .dts
                .checked_sub(out.dts)
                .ok_or(ValidationError::SampleMismatch("segment timestamp shift"))?;
            if u128::from(shift) * u128::from(origin_scale)
                != u128::from(origin) * u128::from(source.timescale)
                || sample.pts - out.pts != i128::from(shift)
                || out.ordinal != n as u64
                || out.sha256 != sample.sha256
                || out.logical.len() != sample.logical.len()
                || out.source_spans != sample.source_spans
                || out.description_index != sample.description_index
                || out.duration != sample.duration
                || out.is_sync != sample.is_sync
                || out.dependency != sample.dependency
            {
                return Err(ValidationError::SampleMismatch("segment output mapping"));
            }
            let description = original
                .descriptions
                .iter()
                .find(|d| d.index == sample.description_index)
                .ok_or(ValidationError::SampleMismatch(
                    "segment description identity",
                ))?;
            mappings.push(SampleOrigin {
                configuration_sha256: description.configuration_sha256,
                sample_entry_sha256: description.sample_entry_sha256,
                track_id: source.id,
                timescale: source.timescale,
                original: sample.clone(),
                output_ordinal: out.ordinal,
                output_dts: out.dts,
                output_pts: out.pts,
            });
        }
    }
    for track in &target.tracks {
        let included = selected.tracks.iter().any(|s| s.id == track.id);
        let declared_missing = plan.labels().iter().any(
            |l| matches!(l, TransformLabel::MissingAudio { track_id, .. } if *track_id == track.id),
        );
        let declared_partial_tail = plan.labels().iter().any(|l| matches!(l, TransformLabel::MissingAudio { track_id, reason: crate::segments::AudioGapReason::PartialTail } if *track_id == track.id));
        if (!included && (track.handler != *b"soun" || !declared_missing))
            || (included
                && declared_missing
                && (track.handler != *b"soun" || !declared_partial_tail))
        {
            return Err(ValidationError::SampleMismatch("segment audio declaration"));
        }
    }
    Ok(SegmentEvidence {
        mappings,
        source_counts: counts,
        labels: plan.labels().to_vec(),
    })
}

/// A private-constructor token binds staged bytes to source ownership, the
/// asserted sample map and explicit transforms. It is not a playable result.
pub struct StagedArtifact {
    path: PathBuf,
    bytes: u64,
    sha256: [u8; 32],
    owner: LogicalOwner,
    expected: SampleMap,
    identity: Identity,
    layout: LogicalFileView,
    source_manifest: SourceManifest,
    transform: ArtifactTransform,
}
impl StagedArtifact {
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn bytes(&self) -> u64 {
        self.bytes
    }
    pub fn sha256(&self) -> [u8; 32] {
        self.sha256
    }
    pub fn expected(&self) -> &SampleMap {
        &self.expected
    }
    pub fn owner(&self) -> &LogicalOwner {
        &self.owner
    }
    fn memory_bound(&self) -> u64 {
        let samples = self
            .expected
            .tracks
            .iter()
            .flat_map(|t| &t.samples)
            .map(|s| 1024 + s.source_spans.len() as u64 * 128)
            .sum::<u64>();
        let descriptions = self
            .expected
            .tracks
            .iter()
            .flat_map(|t| &t.descriptions)
            .map(|d| 1024 + (d.configuration.len() + d.sample_entry.len()) as u64 * 2)
            .sum::<u64>();
        let groups = self
            .expected
            .tracks
            .iter()
            .flat_map(|t| &t.sample_groups)
            .map(|g| {
                1024 + g.assignments.len() as u64 * 32
                    + g.descriptions
                        .iter()
                        .map(|d| 64 + d.len() as u64 * 2)
                        .sum::<u64>()
            })
            .sum::<u64>();
        let transform = match &self.transform {
            ArtifactTransform::OriginalFile => 0,
            ArtifactTransform::SamplePreserving(e) => {
                e.mappings
                    .iter()
                    .map(|m| 2048 + m.original.source_spans.len() as u64 * 256)
                    .sum::<u64>()
                    + e.labels.len() as u64 * 128
                    + e.source_counts.len() as u64 * 64
            }
        };
        65536
            + samples
            + descriptions
            + groups
            + transform * 2
            + self.layout.runs.len() as u64 * 256
            + self.source_manifest.consumed().len() as u64 * 256
            + self.source_manifest.archive_refs().len() as u64 * 128
    }
    fn open(&self) -> Result<File> {
        if !fs::symlink_metadata(&self.path)?.file_type().is_file() {
            return Err(ValidationError::Changed);
        }
        let file = File::open(&self.path)?;
        self.check(&file)?;
        Ok(file)
    }
    fn check(&self, file: &File) -> Result<()> {
        if identity(&file.metadata()?) != self.identity
            || identity(&fs::metadata(&self.path)?) != self.identity
        {
            return Err(ValidationError::Changed);
        }
        Ok(())
    }
}
struct StageReader<'a> {
    artifact: &'a StagedArtifact,
    file: File,
    session: &'a mut CaseSession,
    read: u64,
    cap: u64,
    memory: u64,
    cache: Vec<u8>,
    cache_start: u64,
    deadline: Option<(Instant, Duration)>,
}
impl<'a> StageReader<'a> {
    fn new(
        artifact: &'a StagedArtifact,
        session: &'a mut CaseSession,
        cap: u64,
        memory: u64,
    ) -> Result<Self> {
        Ok(Self {
            file: artifact.open()?,
            artifact,
            session,
            read: 0,
            cap,
            memory,
            cache: Vec::new(),
            cache_start: 0,
            deadline: None,
        })
    }
    fn read_into(&mut self, offset: u64, bytes: &mut [u8]) -> Result<()> {
        if offset
            .checked_add(bytes.len() as u64)
            .is_none_or(|end| end > self.artifact.bytes)
            || bytes.len() > 1024 * 1024
        {
            return Err(ValidationError::ResourceLimit);
        }
        let total = self
            .read
            .checked_add(bytes.len() as u64)
            .filter(|v| *v <= self.cap)
            .ok_or(ValidationError::ResourceLimit)?;
        self.artifact.check(&self.file)?;
        self.session.admit_work(
            "validation-read",
            resources(bytes.len() as u64, self.memory, 0, 0),
        )?;
        self.read = total;
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.read_exact(bytes)?;
        self.artifact.check(&self.file)?;
        Ok(())
    }
    fn cached_read(&mut self, offset: u64, bytes: &mut [u8]) -> Result<()> {
        if offset
            .checked_add(bytes.len() as u64)
            .is_none_or(|n| n > self.artifact.bytes)
        {
            return Err(ValidationError::ResourceLimit);
        }
        self.session.check_source()?;
        self.artifact.check(&self.file)?;
        if let Some((started, allowed)) = self.deadline
            && started.elapsed() >= allowed
        {
            return Err(ValidationError::TimedOut);
        }
        let mut done = 0;
        while done < bytes.len() {
            let at = offset + done as u64;
            if self.cache.is_empty()
                || at < self.cache_start
                || at >= self.cache_start + self.cache.len() as u64
            {
                let start = at / BUFFER as u64 * BUFFER as u64;
                let n = (self.artifact.bytes - start).min(BUFFER as u64) as usize;
                let mut cache = std::mem::take(&mut self.cache);
                cache.resize(n, 0);
                self.read_into(start, &mut cache)?;
                self.cache = cache;
                self.cache_start = start;
            }
            let local = (at - self.cache_start) as usize;
            let n = (self.cache.len() - local).min(bytes.len() - done);
            bytes[done..done + n].copy_from_slice(&self.cache[local..local + n]);
            done += n;
        }
        Ok(())
    }
}
impl ReadAt for StageReader<'_> {
    fn len(&self) -> u64 {
        self.artifact.bytes
    }
    fn read_exact(&mut self, offset: u64, bytes: &mut [u8]) -> bmff::Result<()> {
        self.cached_read(offset, bytes)
            .map_err(|e| BmffError::Io(io::Error::other(e)))
    }
    fn reserve(&mut self, memory: u64) -> bmff::Result<()> {
        self.memory = self.memory.max(memory.saturating_mul(3));
        let allowed = self
            .session
            .admit_external("validation-reparse", self.memory)?;
        self.deadline = Some((Instant::now(), allowed));
        Ok(())
    }
    fn finish(&mut self) -> bmff::Result<()> {
        self.artifact
            .check(&self.file)
            .map_err(|e| BmffError::Io(io::Error::other(e)))?;
        self.session
            .admit_external("validation-reparse-complete", self.memory)?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SampleOrigin {
    pub configuration_sha256: [u8; 32],
    pub sample_entry_sha256: [u8; 32],
    pub track_id: u32,
    pub timescale: u32,
    pub original: bmff::Sample,
    pub output_ordinal: u64,
    pub output_dts: u64,
    pub output_pts: i128,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentEvidence {
    pub mappings: Vec<SampleOrigin>,
    pub source_counts: Vec<(u32, u64)>,
    pub labels: Vec<crate::segments::TransformLabel>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactTransform {
    OriginalFile,
    SamplePreserving(SegmentEvidence),
}
#[derive(Debug, Clone)]
pub struct ValidationReceipt {
    pub transform: ArtifactTransform,
    pub helper_sha256: [u8; 32],
    pub helper_read_bytes: u64,
    pub elapsed: Duration,
    pub claims: Claims,
    pub samples: u64,
    pub packets: u64,
    pub frames: u64,
    pub native_reads: u64,
    pub staged_read_bytes: u64,
    pub max_rss_bytes: u64,
    pub decoder_version: &'static str,
    pub ignore_editlist: bool,
    pub strict_error_flags: u64,
    pub original_file_bytes_preserved: bool,
    pub audio_present: bool,
}
/// Only full validation creates this token. A decoder pass cannot construct or
/// upgrade source ownership, original timing or completeness independently.
pub struct ValidatedArtifact {
    staged: StagedArtifact,
    receipt: ValidationReceipt,
}
impl ValidatedArtifact {
    pub(crate) fn source_layout(&self) -> &LogicalFileView {
        &self.staged.layout
    }
    pub(crate) fn source_manifest(&self) -> &SourceManifest {
        &self.staged.source_manifest
    }
    pub(crate) fn open_verified(&self) -> Result<File> {
        self.staged.open()
    }
    pub(crate) fn check_file(&self, file: &File) -> Result<()> {
        self.staged.check(file)
    }
    pub fn path(&self) -> &Path {
        self.staged.path()
    }
    pub fn bytes(&self) -> u64 {
        self.staged.bytes()
    }
    pub fn sha256(&self) -> [u8; 32] {
        self.staged.sha256()
    }
    pub fn owner(&self) -> &LogicalOwner {
        self.staged.owner()
    }
    pub fn receipt(&self) -> &ValidationReceipt {
        &self.receipt
    }
}
fn unsigned(s: &str) -> Result<u64> {
    s.parse().map_err(|_| ValidationError::Protocol)
}
fn signed(s: &str) -> Result<i128> {
    if s == "NOPTS" {
        return Err(ValidationError::Unsupported);
    }
    s.parse().map_err(|_| ValidationError::Protocol)
}
fn digest(s: &str) -> Result<[u8; 32]> {
    if s.len() != 64 || !s.is_ascii() {
        return Err(ValidationError::Protocol);
    }
    let mut out = [0; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).map_err(|_| ValidationError::Protocol)?;
    }
    Ok(out)
}

pub fn validate(
    session: &mut CaseSession,
    artifact: StagedArtifact,
    helper: &Path,
    limits: ValidationLimits,
    cancel: &AtomicBool,
) -> Result<ValidatedArtifact> {
    validate_observed(session, artifact, helper, limits, cancel, |_| {})
}
pub fn validate_observed(
    session: &mut CaseSession,
    artifact: StagedArtifact,
    helper: &Path,
    limits: ValidationLimits,
    cancel: &AtomicBool,
    cleanup: impl FnOnce(ChildCleanup),
) -> Result<ValidatedArtifact> {
    let started = Instant::now();
    let limits = limits.check()?;
    if artifact.bytes > limits.artifact_bytes
        || artifact.owner.source != session.source_generation()
    {
        return Err(ValidationError::Unsupported);
    }
    let mut total_reads = 0;
    // The actual reopened stage is independently parsed, not merely its plan.
    {
        let mut reader = StageReader::new(
            &artifact,
            session,
            limits.read_bytes,
            limits.parse.memory_bytes * 3 + artifact.memory_bound() + 3 * BUFFER as u64,
        )?;
        let mut observed = bmff::inspect(&mut reader, limits.parse)?;
        total_reads += reader.read;
        if observed.tracks.len() != artifact.expected.tracks.len() {
            return Err(ValidationError::SampleMismatch("sample map"));
        }
        for (actual, expected) in observed.tracks.iter_mut().zip(&artifact.expected.tracks) {
            if actual.samples.len() != expected.samples.len() {
                return Err(ValidationError::SampleMismatch("sample map"));
            }
            for (a, e) in actual.samples.iter_mut().zip(&expected.samples) {
                a.source_spans = e.source_spans.clone();
            }
        }
        if observed != artifact.expected {
            return Err(ValidationError::SampleMismatch("sample map"));
        }
    }
    let mut cleaned = false;
    let decoded = decode(
        session,
        &artifact,
        helper,
        limits,
        limits.read_bytes.saturating_sub(total_reads),
        cancel,
        |receipt| {
            cleaned = true;
            cleanup(receipt)
        },
    );
    if cleaned {
        let settled = session.admit_external("validation-exit", 0);
        if decoded.is_ok() {
            settled?;
        }
    }
    let mut receipt = decoded?;
    total_reads += receipt.staged_read_bytes;
    // Rehash the exact stage after all native seeks/reads; ordinary edits and
    // path replacements are also checked around every read.
    {
        let mut reader = StageReader::new(
            &artifact,
            session,
            limits.read_bytes.saturating_sub(total_reads),
            limits.parse.memory_bytes + artifact.memory_bound() + BUFFER as u64,
        )?;
        let mut hash = Sha256::new();
        let mut bytes = vec![0; BUFFER];
        let mut cursor = 0;
        while cursor < artifact.bytes {
            let n = (artifact.bytes - cursor).min(BUFFER as u64) as usize;
            reader.read_into(cursor, &mut bytes[..n])?;
            hash.update(&bytes[..n]);
            cursor += n as u64;
        }
        if <[u8; 32]>::from(hash.finalize()) != artifact.sha256 {
            return Err(ValidationError::Changed);
        }
        total_reads += reader.read;
    }
    receipt.staged_read_bytes = total_reads;
    session.admit_external("validation-complete", 0)?;
    receipt.elapsed = started.elapsed();
    match &artifact.transform {
        ArtifactTransform::OriginalFile => {
            receipt
                .claims
                .require_exact_samples(SampleVerification {
                    bytes: true,
                    identity: true,
                    order: true,
                    timing: true,
                    configuration: true,
                    audio: true,
                })
                .map_err(|_| ValidationError::SampleMismatch("sample map"))?;
        }
        ArtifactTransform::SamplePreserving(evidence) => {
            let listed = evidence
                .source_counts
                .iter()
                .try_fold(0u64, |n, (_, count)| n.checked_add(*count))
                .ok_or(ValidationError::ResourceLimit)?;
            let recovered = evidence.mappings.len() as u64;
            if recovered == 0 || recovered > listed {
                return Err(ValidationError::SampleMismatch("segment coverage"));
            }
            receipt.original_file_bytes_preserved = false;
            receipt.claims.bytes = ByteProvenance::Synthesized;
            receipt.claims.structure = Structure::RepairedWithRecordedEdits;
            receipt.claims.temporal = Temporal::ConstrainedHypothesis;
            receipt.claims.coverage = if recovered == listed {
                Coverage::CompleteWithinKnownLayout { samples: listed }
            } else {
                Coverage::KnownGaps { recovered, listed }
            };
        }
    }
    receipt
        .claims
        .require_playable()
        .map_err(|_| ValidationError::SampleMismatch("sample map"))?;
    Ok(ValidatedArtifact {
        staged: artifact,
        receipt,
    })
}

#[derive(Default)]
struct StreamState {
    track: usize,
    packets: usize,
    frames: usize,
}
fn decode(
    session: &mut CaseSession,
    artifact: &StagedArtifact,
    helper: &Path,
    limits: ValidationLimits,
    read_cap: u64,
    cancel: &AtomicBool,
    cleanup: impl FnOnce(ChildCleanup),
) -> Result<ValidationReceipt> {
    let sample_count: usize = artifact
        .expected
        .tracks
        .iter()
        .map(|t| t.samples.len())
        .sum();
    for track in &artifact.expected.tracks {
        let Some(first) = track.descriptions.first() else {
            return Err(ValidationError::Unsupported);
        };
        if track
            .descriptions
            .iter()
            .any(|d| d.codec != first.codec || d.configuration != first.configuration)
        {
            return Err(ValidationError::Unsupported);
        }
    }
    let memory = limits.helper_memory_bytes
        + artifact.memory_bound()
        + limits.parse.memory_bytes
        + 4 * 1024 * 1024
        + 3 * BUFFER as u64
        + sample_count as u64 * 32;
    let helper = fs::canonicalize(helper)?;
    let mut executable = File::open(&helper)?;
    let metadata = executable.metadata()?;
    if !metadata.is_file() || metadata.len() > 16 * 1024 * 1024 {
        return Err(ValidationError::Unsupported);
    }
    let tool_identity = identity(&metadata);
    session.admit_work("validator-tool", resources(metadata.len(), memory, 0, 0))?;
    let mut fingerprint = Sha256::new();
    let mut buffer = vec![0; BUFFER];
    let mut remaining = metadata.len();
    while remaining > 0 {
        let n = remaining.min(BUFFER as u64) as usize;
        executable.read_exact(&mut buffer[..n])?;
        fingerprint.update(&buffer[..n]);
        remaining -= n as u64;
    }
    if identity(&executable.metadata()?) != tool_identity
        || identity(&fs::metadata(&helper)?) != tool_identity
    {
        return Err(ValidationError::Changed);
    }
    let helper_sha256 = fingerprint.finalize().into();
    let start = Instant::now();
    let remaining = session.admit_work(
        "strict-decode",
        resources(
            0,
            memory,
            0,
            limits
                .wall
                .as_secs()
                .saturating_add(u64::from(limits.wall.subsec_nanos() != 0)),
        ),
    )?;
    let allowance = limits.wall.min(remaining);
    check_control(cancel, start, allowance)?;
    let mut command = Command::new(&helper);
    command.arg(artifact.bytes.to_string());
    let (mut worker, events) = supervision::spawn(&mut command, limits.protocol_bytes, cleanup)?;
    let mut reader = StageReader::new(artifact, session, read_cap, memory)?;
    let mut hello = false;
    let mut policy = false;
    let mut streams: Vec<StreamState> = Vec::new();
    let mut seen_tracks = Vec::new();
    let mut positions: Vec<Vec<i128>> = Vec::new();
    for track in &artifact.expected.tracks {
        let mut pts = track.samples.iter().map(|s| s.pts).collect::<Vec<_>>();
        pts.sort();
        positions.push(pts);
    }
    let (mut packets, mut frames, mut native_reads) = (0u64, 0u64, 0u64);
    let mut summary = None;
    loop {
        check_control(cancel, start, allowance)?;
        let event = match events.recv_timeout(Duration::from_millis(10)) {
            Ok(event) => event,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => return Err(ValidationError::NativeFailure),
        };
        let line = match event {
            Event::Line(line) => line,
            Event::Invalid => return Err(ValidationError::ResourceLimit),
            _ => return Err(ValidationError::NativeFailure),
        };
        reader.session.check_source()?;
        let text = std::str::from_utf8(&line).map_err(|_| ValidationError::Protocol)?;
        let f = text.split_ascii_whitespace().collect::<Vec<_>>();
        let Some(tag) = f.first() else {
            return Err(ValidationError::Protocol);
        };
        if !hello && *tag != "HELLO" {
            return Err(ValidationError::Protocol);
        }
        match *tag {
            "HELLO" => {
                if hello || f.len() != 9 || f[1] != "FFMPEG" || f[2] != "9.0.2" {
                    return Err(ValidationError::VersionMismatch);
                }
                let nums = f[3..]
                    .iter()
                    .map(|s| unsigned(s))
                    .collect::<Result<Vec<_>>>()?;
                if nums != [FMT, FMT, FMT, FMT, UTIL, UTIL] {
                    return Err(ValidationError::VersionMismatch);
                }
                hello = true;
            }
            "POLICY" => {
                if policy
                    || f.len() != 4
                    || unsigned(f[1])? != 1
                    || unsigned(f[2])? != STRICT
                    || unsigned(f[3])? != 1
                {
                    return Err(ValidationError::Protocol);
                }
                policy = true;
            }
            "READ" => {
                if !policy || f.len() != 3 {
                    return Err(ValidationError::Protocol);
                }
                let offset = unsigned(f[1])?;
                let n = unsigned(f[2])?;
                if n == 0 || n > BUFFER as u64 {
                    return Err(ValidationError::ResourceLimit);
                }
                let mut response = format!("OK {n}\n").into_bytes();
                let header = response.len();
                response.resize(header + n as usize, 0);
                reader.read_into(offset, &mut response[header..])?;
                native_reads += 1;
                send(&worker, response, cancel, start, allowance)?;
            }
            "STREAM" => {
                if !policy
                    || f.len() != 14
                    || unsigned(f[1])? != streams.len() as u64
                    || streams.len() >= artifact.expected.tracks.len()
                {
                    return Err(ValidationError::Protocol);
                }
                let id = unsigned(f[2])?;
                let track = artifact
                    .expected
                    .tracks
                    .iter()
                    .position(|t| u64::from(t.id) == id)
                    .ok_or(ValidationError::SampleMismatch(
                        "native stream configuration",
                    ))?;
                if seen_tracks.contains(&track) {
                    return Err(ValidationError::SampleMismatch(
                        "native stream configuration",
                    ));
                }
                let expected = &artifact.expected.tracks[track];
                let description = &expected.descriptions[0];
                let (codec, kind) = match &description.codec {
                    b"avc1" => (27, 0),
                    b"hvc1" => (172, 0),
                    b"mp4a" => (86018, 1),
                    _ => return Err(ValidationError::Unsupported),
                };
                if unsigned(f[3])? != codec
                    || unsigned(f[4])? != u64::from(u32::from_le_bytes(description.codec))
                    || unsigned(f[5])? != kind
                    || unsigned(f[6])?.checked_mul(u64::from(expected.timescale))
                        != Some(unsigned(f[7])?)
                    || unsigned(f[8])? != description.configuration.len() as u64
                    || digest(f[9])? != description.configuration_sha256
                {
                    return Err(ValidationError::SampleMismatch(
                        "native stream configuration",
                    ));
                }
                if unsigned(f[10])? > 4096 || unsigned(f[11])? > 2160 || unsigned(f[13])? > 8 {
                    return Err(ValidationError::Unsupported);
                }
                if kind == 1
                    && (Some(unsigned(f[12])?) != description.audio_sample_rate.map(u64::from)
                        || Some(unsigned(f[13])?) != description.audio_channels.map(u64::from))
                {
                    return Err(ValidationError::SampleMismatch(
                        "AAC configuration interpretation",
                    ));
                }
                seen_tracks.push(track);
                streams.push(StreamState {
                    track,
                    ..Default::default()
                });
            }
            "PACKET" => {
                if f.len() != 10 || summary.is_some() {
                    return Err(ValidationError::Protocol);
                }
                let index = unsigned(f[1])? as usize;
                let state = streams.get_mut(index).ok_or(ValidationError::Protocol)?;
                let expected = artifact.expected.tracks[state.track]
                    .samples
                    .get(state.packets)
                    .ok_or(ValidationError::SampleMismatch(
                        "native packet identity or timing",
                    ))?;
                if unsigned(f[2])? != state.packets as u64
                    || signed(f[3])? != i128::from(expected.logical.start())
                    || unsigned(f[4])? != expected.logical.len()
                    || signed(f[5])? != i128::from(expected.dts)
                    || signed(f[6])? != expected.pts
                    || unsigned(f[7])? != u64::from(expected.duration)
                    || unsigned(f[8])? & 2 != 0
                    || digest(f[9])? != expected.sha256
                {
                    return Err(ValidationError::SampleMismatch(
                        "native packet identity or timing",
                    ));
                }
                state.packets += 1;
                packets += 1;
            }
            "FRAME" => {
                if f.len() != 10 || summary.is_some() {
                    return Err(ValidationError::Protocol);
                }
                let index = unsigned(f[1])? as usize;
                let state = streams.get_mut(index).ok_or(ValidationError::Protocol)?;
                if unsigned(f[2])? != state.frames as u64
                    || positions[state.track].get(state.frames) != Some(&signed(f[3])?)
                    || unsigned(f[6])? & 1 != 0
                    || signed(f[7])? != 0
                    || unsigned(f[8])? > 4096
                    || unsigned(f[9])? > 2160
                {
                    return Err(ValidationError::SampleMismatch(
                        "decoded frame identity or timing",
                    ));
                }
                state.frames += 1;
                frames += 1;
            }
            "SUMMARY" => {
                if f.len() != 6
                    || summary.is_some()
                    || unsigned(f[1])? != packets
                    || unsigned(f[2])? != frames
                    || unsigned(f[3])? != 0
                    || unsigned(f[4])? != 0
                {
                    return Err(ValidationError::SampleMismatch("native decode summary"));
                }
                let rss = unsigned(f[5])?;
                if rss == 0 || rss > limits.helper_memory_bytes {
                    return Err(ValidationError::ResourceLimit);
                }
                summary = Some(rss);
            }
            "ERROR" => {
                if f.len() != 5 {
                    return Err(ValidationError::Protocol);
                }
                return Err(ValidationError::NativeError {
                    phase: unsigned(f[1])?,
                    code: i64::try_from(signed(f[2])?).map_err(|_| ValidationError::Protocol)?,
                    warnings: unsigned(f[3])?,
                    errors: unsigned(f[4])?,
                });
            }
            "DONE" => {
                if f.len() != 1
                    || summary.is_none()
                    || streams.len() != artifact.expected.tracks.len()
                    || packets != sample_count as u64
                {
                    return Err(ValidationError::SampleMismatch("native decode coverage"));
                }
                for state in &streams {
                    let n = artifact.expected.tracks[state.track].samples.len();
                    if state.packets != n || state.frames != n {
                        return Err(ValidationError::SampleMismatch("native decode coverage"));
                    }
                }
                loop {
                    check_control(cancel, start, allowance)?;
                    match worker.child.try_wait()? {
                        Some(status) if status.success() => break,
                        Some(_) => return Err(ValidationError::NativeFailure),
                        None => thread::sleep(Duration::from_millis(2)),
                    }
                }
                break;
            }
            _ => return Err(ValidationError::Protocol),
        }
    }
    Ok(ValidationReceipt {
        transform: artifact.transform.clone(),
        helper_sha256,
        helper_read_bytes: metadata.len(),
        elapsed: Duration::ZERO,
        claims: Claims {
            bytes: ByteProvenance::AcquisitionSupported,
            ownership: Ownership::MetadataSupported,
            structure: Structure::Valid,
            decode: Decode::StrictPass,
            temporal: Temporal::OriginalMappingSupported,
            coverage: Coverage::CompleteWithinKnownLayout {
                samples: sample_count as u64,
            },
        },
        samples: sample_count as u64,
        packets,
        frames,
        native_reads,
        staged_read_bytes: reader.read,
        max_rss_bytes: summary.unwrap(),
        decoder_version: "9.0.2",
        ignore_editlist: true,
        strict_error_flags: STRICT,
        original_file_bytes_preserved: true,
        audio_present: artifact
            .expected
            .tracks
            .iter()
            .any(|t| t.handler == *b"soun"),
    })
}
