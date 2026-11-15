//! One no-clobber durable publication protocol for synthetic fixtures and sealed
//! validated media. Hard-link + directory flush + unlink avoids rename overwrite.
//! Physical power-loss behavior still requires host/filesystem qualification.
use crate::{
    contracts::*,
    session::{CaseSession, SessionError},
    validation::{ValidatedArtifact, ValidationReceipt},
};
use rusqlite::{Connection, OpenFlags, params};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    time::Duration,
};
const OWNER: &[u8] = b"FrameHarbor publication v2\n";
const APP: i64 = 0x46485055;
const SYNTHETIC: &[u8] = b"synthetic-byte-comparison-v1";
const MAX_EVIDENCE: usize = 1024 * 1024;
const BUFFER: usize = 65536;
type Result<T> = std::result::Result<T, PublicationError>;
#[derive(Debug)]
pub enum PublicationError {
    Io(io::Error),
    Sql(rusqlite::Error),
    Session(SessionError),
    Invalid,
    Unowned,
    Conflict,
    Quota,
    Corrupt,
    SourceMismatch,
    Changed,
    Injected(FaultPoint),
    UnsupportedFilesystem,
}
impl std::fmt::Display for PublicationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for PublicationError {}
impl From<io::Error> for PublicationError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<rusqlite::Error> for PublicationError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sql(e)
    }
}
impl From<SessionError> for PublicationError {
    fn from(e: SessionError) -> Self {
        Self::Session(e)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultPoint {
    None,
    StageFlushed,
    IntentCommitted,
    FinalLinked,
    DirectoryFlushed,
    PublishedCommitted,
}
#[derive(Debug, Clone, Copy)]
pub struct PublicationLimits {
    pub artifact_bytes: u64,
    pub output_bytes: u64,
    pub verification_bytes: u64,
    pub database_bytes: u64,
    pub artifacts: usize,
}
impl Default for PublicationLimits {
    fn default() -> Self {
        Self {
            artifact_bytes: 1024 * 1024,
            output_bytes: 16 * 1024 * 1024,
            verification_bytes: 64 * 1024 * 1024,
            database_bytes: 1024 * 1024,
            artifacts: 1024,
        }
    }
}
#[derive(Debug, Clone)]
pub struct SyntheticFixtureReceipt {
    hash: String,
    size: u64,
}
impl SyntheticFixtureReceipt {
    pub fn from_fixture(bytes: &[u8]) -> Self {
        Self {
            hash: hex(&Sha256::digest(bytes)),
            size: bytes.len() as u64,
        }
    }
}
/// Stored facts are inspectable, but cannot construct a ValidatedArtifact token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedValidationReceipt {
    pub transform: crate::validation::ArtifactTransform,
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
    pub decoder_version: String,
    pub ignore_editlist: bool,
    pub strict_error_flags: u64,
    pub original_file_bytes_preserved: bool,
    pub audio_present: bool,
}
impl From<&ValidationReceipt> for PersistedValidationReceipt {
    fn from(r: &ValidationReceipt) -> Self {
        Self {
            transform: r.transform.clone(),
            helper_sha256: r.helper_sha256,
            helper_read_bytes: r.helper_read_bytes,
            elapsed: r.elapsed,
            claims: r.claims.clone(),
            samples: r.samples,
            packets: r.packets,
            frames: r.frames,
            native_reads: r.native_reads,
            staged_read_bytes: r.staged_read_bytes,
            max_rss_bytes: r.max_rss_bytes,
            decoder_version: r.decoder_version.into(),
            ignore_editlist: r.ignore_editlist,
            strict_error_flags: r.strict_error_flags,
            original_file_bytes_preserved: r.original_file_bytes_preserved,
            audio_present: r.audio_present,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provenance {
    pub generation: [u8; 32],
    pub image_size: u64,
    pub map_sha256: [u8; 32],
    pub input_domain: ByteRange,
    pub image_domain: ByteRange,
    pub binding: BindingEvidence,
    /// Content-addressed immutable source-dependency blocks retained by the case.
    pub archive_refs: Vec<[u8; 32]>,
    pub consumed: Vec<crate::acquisition::ConsumedRange>,
    pub layout: crate::filesystem::LogicalFileView,
}
/// Constructed only after a committed portable artifact and same-Ledger work
/// links have been verified. A caller-supplied manifest cannot retire reads.
pub(crate) struct RetainedSourceProof {
    generation: [u8; 32],
    image_size: u64,
    map_sha256: [u8; 32],
    input_domain: ByteRange,
    image_domain: ByteRange,
    binding: BindingEvidence,
    consumed: Vec<crate::acquisition::ConsumedRange>,
    archive_refs: Vec<[u8; 32]>,
}
impl RetainedSourceProof {
    pub(crate) fn generation(&self) -> [u8; 32] {
        self.generation
    }
    pub(crate) fn image_size(&self) -> u64 {
        self.image_size
    }
    pub(crate) fn map_sha256(&self) -> [u8; 32] {
        self.map_sha256
    }
    pub(crate) fn input_domain(&self) -> ByteRange {
        self.input_domain
    }
    pub(crate) fn image_domain(&self) -> ByteRange {
        self.image_domain
    }
    pub(crate) fn binding(&self) -> BindingEvidence {
        self.binding
    }
    pub(crate) fn consumed(&self) -> &[crate::acquisition::ConsumedRange] {
        &self.consumed
    }
    pub(crate) fn archive_refs(&self) -> &[[u8; 32]] {
        &self.archive_refs
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaEvidence {
    pub provenance: Provenance,
    pub owner: LogicalOwner,
    pub receipt: PersistedValidationReceipt,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactKind {
    SyntheticFixture,
    ValidatedMedia(Box<MediaEvidence>),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    pub id: String,
    pub path: PathBuf,
    pub bytes: u64,
    pub sha256: String,
    pub kind: ArtifactKind,
}
struct Record {
    id: String,
    hash: String,
    size: u64,
    state: String,
    kind: u8,
    evidence: Vec<u8>,
}
pub struct Publisher {
    root: PathBuf,
    db: Connection,
    limits: PublicationLimits,
    verified: u64,
}
fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
fn valid_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|x| x.is_ascii_hexdigit() && !x.is_ascii_uppercase())
}
fn safe_path(path: &Path) -> Result<()> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut p = PathBuf::new();
    for c in absolute.components() {
        match c {
            Component::RootDir | Component::Prefix(_) => p.push(c),
            Component::Normal(n) => p.push(n),
            _ => return Err(PublicationError::Invalid),
        };
        match fs::symlink_metadata(&p) {
            Ok(m) if m.file_type().is_symlink() => return Err(PublicationError::Unowned),
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
fn regular(p: &Path) -> Result<()> {
    let m = fs::symlink_metadata(p)?;
    if !m.is_file() || m.file_type().is_symlink() {
        return Err(PublicationError::Unowned);
    }
    Ok(())
}
fn unchanged_file(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        a.is_file()
            && b.is_file()
            && a.dev() == b.dev()
            && a.ino() == b.ino()
            && a.len() == b.len()
            && a.mtime() == b.mtime()
            && a.mtime_nsec() == b.mtime_nsec()
            && a.ctime() == b.ctime()
            && a.ctime_nsec() == b.ctime_nsec()
    }
    #[cfg(not(unix))]
    {
        a.is_file() && b.is_file() && a.len() == b.len() && a.modified().ok() == b.modified().ok()
    }
}
fn same_inode(a: &Path, b: &Path) -> Result<()> {
    regular(a)?;
    regular(b)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let a = fs::metadata(a)?;
        let b = fs::metadata(b)?;
        if a.dev() != b.dev() || a.ino() != b.ino() {
            return Err(PublicationError::Conflict);
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        Err(PublicationError::UnsupportedFilesystem)
    }
}
fn sync_dir(p: &Path) -> Result<()> {
    File::open(p)?.sync_all()?;
    Ok(())
}
fn fault(actual: FaultPoint, wanted: FaultPoint) -> Result<()> {
    if actual == wanted {
        Err(PublicationError::Injected(actual))
    } else {
        Ok(())
    }
}
fn guard(
    session: &mut Option<&mut CaseSession>,
    read: u64,
    output: u64,
    index: u64,
    memory: u64,
) -> Result<()> {
    if let Some(s) = session.as_deref_mut() {
        s.check_source()?;
        s.admit_work(
            "publication",
            ResourceCaps {
                read_bytes: read,
                output_bytes: output,
                index_bytes: index,
                memory_bytes: memory,
                visited_postings: 0,
                transitions: 0,
                decoder_seconds: 0,
            },
        )?;
    }
    Ok(())
}
fn source_guard(session: &mut Option<&mut CaseSession>, owner: &LogicalOwner) -> Result<()> {
    let s = session
        .as_deref_mut()
        .ok_or(PublicationError::SourceMismatch)?;
    s.check_source()?;
    if s.source_generation() != owner.source {
        return Err(PublicationError::SourceMismatch);
    }
    Ok(())
}
fn field(b: &mut Vec<u8>, s: &str) -> Result<()> {
    if s.is_empty() || s.len() > 4096 {
        return Err(PublicationError::Invalid);
    }
    b.extend_from_slice(&(s.len() as u32).to_be_bytes());
    b.extend_from_slice(s.as_bytes());
    Ok(())
}
fn media_id(
    owner: &LogicalOwner,
    hash: &str,
    transform: &crate::validation::ArtifactTransform,
) -> Result<String> {
    if !valid_id(hash) {
        return Err(PublicationError::Invalid);
    }
    let mut b = b"FrameHarbor validated media identity v2\0".to_vec();
    for s in [
        &owner.source.0,
        &owner.filesystem,
        &owner.file_identity,
        hash,
    ] {
        field(&mut b, s)?;
    }
    let mut encoded = Vec::new();
    encode_transform(&mut encoded, transform)?;
    b.extend(Sha256::digest(encoded));
    Ok(hex(&Sha256::digest(b)))
}
fn segment_shape(e: &crate::validation::SegmentEvidence) -> bool {
    e.mappings.len() <= 4096
        && e.source_counts.len() <= 64
        && e.labels.len() <= 128
        && e.mappings
            .iter()
            .try_fold(0usize, |n, m| n.checked_add(m.original.source_spans.len()))
            .is_some_and(|n| n <= 16384)
}
fn segment_valid(e: &crate::validation::SegmentEvidence, r: &PersistedValidationReceipt) -> bool {
    use crate::segments::TransformLabel;
    if !segment_shape(e) || e.mappings.len() as u64 != r.samples || e.source_counts.is_empty() {
        return false;
    }
    let mut counts = std::collections::BTreeMap::new();
    let mut listed = 0u64;
    for &(id, count) in &e.source_counts {
        if id == 0 || counts.insert(id, count).is_some() {
            return false;
        }
        let Some(n) = listed.checked_add(count) else {
            return false;
        };
        listed = n;
    }
    let expected = if listed == r.samples {
        Coverage::CompleteWithinKnownLayout { samples: r.samples }
    } else {
        Coverage::KnownGaps {
            recovered: r.samples,
            listed,
        }
    };
    if listed < r.samples || r.claims.coverage != expected {
        return false;
    }
    let mut headers = 0;
    let mut origin = None;
    let mut dropped = 0;
    let mut missing = std::collections::BTreeMap::new();
    for label in &e.labels {
        match label {
            TransformLabel::SynthesizedHeaders => headers += 1,
            TransformLabel::TimestampRebase {
                origin_ticks,
                timescale,
            } => {
                if *timescale == 0 || origin.replace((*origin_ticks, *timescale)).is_some() {
                    return false;
                }
            }
            TransformLabel::DroppedTargetEdits => dropped += 1,
            TransformLabel::MissingAudio { track_id, reason } => {
                if !counts.contains_key(track_id) || missing.insert(*track_id, reason).is_some() {
                    return false;
                }
            }
        }
    }
    if headers != 1 || dropped > 1 {
        return false;
    }
    let Some((origin_ticks, origin_scale)) = origin else {
        return false;
    };
    let mut tracks = std::collections::BTreeMap::<u32, (u64, u64, u64, u32, u64)>::new();
    for m in &e.mappings {
        let s = &m.original;
        let Some(count) = counts.get(&m.track_id) else {
            return false;
        };
        if m.timescale == 0
            || s.ordinal >= *count
            || s.duration == 0
            || s.description_index == 0
            || s.logical.is_empty()
            || s.source_spans.is_empty()
            || missing
                .get(&m.track_id)
                .is_some_and(|reason| **reason != crate::segments::AudioGapReason::PartialTail)
            || s.dts < m.output_dts
        {
            return false;
        }
        let shift = s.dts - m.output_dts;
        if u128::from(shift) * u128::from(origin_scale)
            != u128::from(origin_ticks) * u128::from(m.timescale)
            || s.pts.checked_sub(m.output_pts) != Some(i128::from(shift))
        {
            return false;
        }
        let Some(next_output) = m.output_dts.checked_add(u64::from(s.duration)) else {
            return false;
        };
        let Some(next_source) = s.dts.checked_add(u64::from(s.duration)) else {
            return false;
        };
        let entry =
            tracks
                .entry(m.track_id)
                .or_insert((0, m.output_dts, s.dts, m.timescale, s.ordinal));
        if m.output_ordinal != entry.0
            || m.output_dts != entry.1
            || s.dts != entry.2
            || m.timescale != entry.3
            || s.ordinal != entry.4
        {
            return false;
        }
        entry.0 += 1;
        entry.1 = next_output;
        entry.2 = next_source;
        let Some(next_ordinal) = s.ordinal.checked_add(1) else {
            return false;
        };
        entry.4 = next_ordinal;
    }
    true
}
fn encode_transform(
    b: &mut Vec<u8>,
    transform: &crate::validation::ArtifactTransform,
) -> Result<()> {
    use crate::{
        segments::{AudioGapReason, TransformLabel},
        validation::ArtifactTransform,
    };
    match transform {
        ArtifactTransform::OriginalFile => b.push(0),
        ArtifactTransform::SamplePreserving(e) => {
            if !segment_shape(e) {
                return Err(PublicationError::Invalid);
            }
            b.push(1);
            b.extend((e.source_counts.len() as u64).to_be_bytes());
            for &(id, count) in &e.source_counts {
                b.extend(u64::from(id).to_be_bytes());
                b.extend(count.to_be_bytes());
            }
            b.extend((e.labels.len() as u64).to_be_bytes());
            for label in &e.labels {
                match label {
                    TransformLabel::SynthesizedHeaders => b.push(0),
                    TransformLabel::TimestampRebase {
                        origin_ticks,
                        timescale,
                    } => {
                        b.push(1);
                        b.extend(origin_ticks.to_be_bytes());
                        b.extend(u64::from(*timescale).to_be_bytes());
                    }
                    TransformLabel::DroppedTargetEdits => b.push(2),
                    TransformLabel::MissingAudio { track_id, reason } => {
                        b.push(3);
                        b.extend(u64::from(*track_id).to_be_bytes());
                        b.push(match reason {
                            AudioGapReason::UnalignedStart => 0,
                            AudioGapReason::UnavailablePayload => 1,
                            AudioGapReason::PartialTail => 2,
                            AudioGapReason::UnsupportedConfiguration => 3,
                            AudioGapReason::NoSamples => 4,
                        });
                    }
                }
            }
            b.extend((e.mappings.len() as u64).to_be_bytes());
            for m in &e.mappings {
                let s = &m.original;
                for n in [
                    u64::from(m.track_id),
                    u64::from(m.timescale),
                    s.ordinal,
                    u64::from(s.description_index),
                    s.logical.start(),
                    s.logical.end(),
                    s.dts,
                    u64::from(s.duration),
                    m.output_ordinal,
                    m.output_dts,
                ] {
                    b.extend(n.to_be_bytes());
                }
                b.extend(s.pts.to_be_bytes());
                b.extend(m.output_pts.to_be_bytes());
                b.extend(s.sha256);
                b.extend(m.configuration_sha256);
                b.extend(m.sample_entry_sha256);
                b.extend([u8::from(s.is_sync), s.dependency]);
                b.extend((s.source_spans.len() as u64).to_be_bytes());
                for span in &s.source_spans {
                    for n in [
                        span.logical.start(),
                        span.logical.end(),
                        span.image.start(),
                        span.image.end(),
                    ] {
                        b.extend(n.to_be_bytes());
                    }
                }
                if b.len() > MAX_EVIDENCE - 32 {
                    return Err(PublicationError::Invalid);
                }
            }
        }
    }
    Ok(())
}
fn decode_transform(c: &mut Cursor<'_>) -> Result<crate::validation::ArtifactTransform> {
    use crate::{
        segments::{AudioGapReason, TransformLabel},
        validation::{ArtifactTransform, SampleOrigin, SegmentEvidence},
    };
    match c.take(1)? {
        [0] => Ok(ArtifactTransform::OriginalFile),
        [1] => {
            let count = c.n()?;
            if count > 64 {
                return Err(PublicationError::Corrupt);
            }
            let mut source_counts = Vec::new();
            for _ in 0..count {
                source_counts.push((c.u32()?, c.n()?));
            }
            let count = c.n()?;
            if count > 128 {
                return Err(PublicationError::Corrupt);
            }
            let mut labels = Vec::new();
            for _ in 0..count {
                labels.push(match c.take(1)? {
                    [0] => TransformLabel::SynthesizedHeaders,
                    [1] => TransformLabel::TimestampRebase {
                        origin_ticks: c.n()?,
                        timescale: c.u32()?,
                    },
                    [2] => TransformLabel::DroppedTargetEdits,
                    [3] => {
                        let track_id = c.u32()?;
                        let reason = match c.take(1)? {
                            [0] => AudioGapReason::UnalignedStart,
                            [1] => AudioGapReason::UnavailablePayload,
                            [2] => AudioGapReason::PartialTail,
                            [3] => AudioGapReason::UnsupportedConfiguration,
                            [4] => AudioGapReason::NoSamples,
                            _ => return Err(PublicationError::Corrupt),
                        };
                        TransformLabel::MissingAudio { track_id, reason }
                    }
                    _ => return Err(PublicationError::Corrupt),
                });
            }
            let count = c.n()?;
            if count > 4096 {
                return Err(PublicationError::Corrupt);
            }
            let mut mappings = Vec::new();
            let mut spans = 0u64;
            for _ in 0..count {
                let track_id = c.u32()?;
                let timescale = c.u32()?;
                let ordinal = c.n()?;
                let description_index = c.u32()?;
                let logical = c.range()?;
                let dts = c.n()?;
                let duration = c.u32()?;
                let output_ordinal = c.n()?;
                let output_dts = c.n()?;
                let pts = c.i128()?;
                let output_pts = c.i128()?;
                let sha256 = c.take(32)?.try_into().unwrap();
                let configuration_sha256 = c.take(32)?.try_into().unwrap();
                let sample_entry_sha256 = c.take(32)?.try_into().unwrap();
                let is_sync = c.boolean()?;
                let dependency = c.take(1)?[0];
                let count = c.n()?;
                spans = spans.checked_add(count).ok_or(PublicationError::Corrupt)?;
                if spans > 16384 {
                    return Err(PublicationError::Corrupt);
                }
                let mut source_spans = Vec::new();
                for _ in 0..count {
                    source_spans.push(crate::bmff::SourceSpan {
                        logical: c.range()?,
                        image: c.range()?,
                    });
                }
                mappings.push(SampleOrigin {
                    track_id,
                    timescale,
                    original: crate::bmff::Sample {
                        ordinal,
                        description_index,
                        logical,
                        source_spans,
                        sha256,
                        dts,
                        pts,
                        duration,
                        is_sync,
                        dependency,
                    },
                    output_ordinal,
                    output_dts,
                    output_pts,
                    configuration_sha256,
                    sample_entry_sha256,
                });
            }
            Ok(ArtifactTransform::SamplePreserving(SegmentEvidence {
                mappings,
                source_counts,
                labels,
            }))
        }
        _ => Err(PublicationError::Corrupt),
    }
}

fn receipt_valid(r: &PersistedValidationReceipt) -> bool {
    r.claims.validate().is_ok()
        && r.claims.ownership == Ownership::MetadataSupported
        && r.claims.decode == Decode::StrictPass
        && r.samples > 0
        && r.packets == r.samples
        && r.frames == r.samples
        && r.helper_read_bytes > 0
        && r.native_reads > 0
        && r.staged_read_bytes > 0
        && r.decoder_version == "9.0.2"
        && r.ignore_editlist
        && r.strict_error_flags == 65551
        && match &r.transform {
            crate::validation::ArtifactTransform::OriginalFile => {
                r.original_file_bytes_preserved
                    && r.claims.bytes == ByteProvenance::AcquisitionSupported
                    && r.claims.structure == Structure::Valid
                    && r.claims.temporal == Temporal::OriginalMappingSupported
                    && r.claims.coverage
                        == Coverage::CompleteWithinKnownLayout { samples: r.samples }
            }
            crate::validation::ArtifactTransform::SamplePreserving(e) => {
                !r.original_file_bytes_preserved
                    && r.claims.bytes == ByteProvenance::Synthesized
                    && r.claims.structure == Structure::RepairedWithRecordedEdits
                    && r.claims.temporal == Temporal::ConstrainedHypothesis
                    && segment_valid(e, r)
            }
        }
}
fn provenance_valid(
    p: &Provenance,
    owner: &LogicalOwner,
    transform: &crate::validation::ArtifactTransform,
) -> bool {
    if p.archive_refs.len() > 1024
        || p.archive_refs
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != p.archive_refs.len()
        || hex(&p.generation) != owner.source.0
        || p.layout.owner != *owner
        || p.binding == BindingEvidence::Unverified
        || p.image_domain.end() > p.image_size
        || p.input_domain.len() != p.image_domain.len()
        || p.layout.evidence != crate::filesystem::ExtentEvidence::MetadataAllocated
        || p.layout.size == 0
        || p.layout.metadata_flags & 1 == 0
        || p.layout.initialized_size != p.layout.size
        || p.layout.skip_length != 0
    {
        return false;
    }
    let mut covered = p.consumed.iter().map(|c| c.image).collect::<Vec<_>>();
    covered.sort();
    if covered.iter().any(|r| r.end() > p.image_size) {
        return false;
    }
    let mut logical = 0;
    for run in &p.layout.runs {
        let Some(image) = run.image else { return false };
        if run.logical.start() != logical
            || run.logical.len() != image.len()
            || run.raw_flags != 0
            || !run.allocation_consistent
            || !p.image_domain.contains(image)
        {
            return false;
        }
        if matches!(
            transform,
            crate::validation::ArtifactTransform::OriginalFile
        ) && !covered_range(&covered, image)
        {
            return false;
        }
        logical = run.logical.end();
    }
    if logical != p.layout.size {
        return false;
    }
    if let crate::validation::ArtifactTransform::SamplePreserving(e) = transform {
        for mapping in &e.mappings {
            let sample = &mapping.original;
            let mut cursor = sample.logical.start();
            for span in &sample.source_spans {
                if span.logical.start() != cursor
                    || span.logical.len() != span.image.len()
                    || span.logical.end() > sample.logical.end()
                    || !covered_range(&covered, span.image)
                {
                    return false;
                }
                let expected = p
                    .layout
                    .runs
                    .iter()
                    .find(|r| r.logical.contains(span.logical))
                    .and_then(|r| r.image.map(|i| (r.logical, i)));
                let Some((logical, image)) = expected else {
                    return false;
                };
                if image
                    .start()
                    .checked_add(span.logical.start() - logical.start())
                    != Some(span.image.start())
                {
                    return false;
                }
                cursor = span.logical.end();
            }
            if cursor != sample.logical.end() {
                return false;
            }
        }
    }
    true
}
fn covered_range(covered: &[ByteRange], range: ByteRange) -> bool {
    let mut cursor = range.start();
    for span in covered {
        if span.start() > cursor {
            break;
        }
        cursor = cursor.max(span.end());
        if cursor >= range.end() {
            return true;
        }
    }
    false
}
fn encode(e: &MediaEvidence) -> Result<Vec<u8>> {
    if !receipt_valid(&e.receipt)
        || !provenance_valid(&e.provenance, &e.owner, &e.receipt.transform)
    {
        return Err(PublicationError::Invalid);
    }
    let mut b = b"FH-MEDIA-RECEIPT-3\0".to_vec();
    for s in [
        &e.owner.source.0,
        &e.owner.filesystem,
        &e.owner.file_identity,
    ] {
        field(&mut b, s)?;
    }
    let p = &e.provenance;
    b.extend(p.generation);
    b.extend(p.image_size.to_be_bytes());
    b.extend(p.map_sha256);
    for range in [p.input_domain, p.image_domain] {
        b.extend(range.start().to_be_bytes());
        b.extend(range.end().to_be_bytes());
    }
    b.push(match p.binding {
        BindingEvidence::Unverified => 0,
        BindingEvidence::OperatorAttested => 1,
        BindingEvidence::TrustedManifest => 2,
    });
    if p.binding == BindingEvidence::Unverified
        || p.layout.owner != e.owner
        || p.consumed.len() > 16384
        || p.layout.runs.len() > 1024
    {
        return Err(PublicationError::Invalid);
    }
    b.extend((p.archive_refs.len() as u64).to_be_bytes());
    for reference in &p.archive_refs {
        b.extend(reference);
    }
    b.extend((p.consumed.len() as u64).to_be_bytes());
    for consumed in &p.consumed {
        b.extend(consumed.image.start().to_be_bytes());
        b.extend(consumed.image.end().to_be_bytes());
        b.extend(consumed.sha256);
    }
    for value in [
        p.layout.size,
        p.layout.metadata_flags,
        p.layout.name_flags,
        p.layout.attribute_flags,
        p.layout.initialized_size,
        p.layout.skip_length,
        p.layout.runs.len() as u64,
    ] {
        b.extend(value.to_be_bytes());
    }
    for run in &p.layout.runs {
        let image = run.image.ok_or(PublicationError::Invalid)?;
        for value in [
            run.logical.start(),
            run.logical.end(),
            image.start(),
            image.end(),
            run.raw_flags,
            u64::from(run.allocation_consistent),
        ] {
            b.extend(value.to_be_bytes());
        }
    }
    let r = &e.receipt;
    encode_transform(&mut b, &r.transform)?;
    b.extend(r.helper_sha256);
    b.extend(r.helper_read_bytes.to_be_bytes());
    encode_claims(&mut b, &r.claims);
    for n in [
        r.samples,
        r.elapsed.as_secs(),
        u64::from(r.elapsed.subsec_nanos()),
        r.packets,
        r.frames,
        r.native_reads,
        r.staged_read_bytes,
        r.max_rss_bytes,
        r.strict_error_flags,
    ] {
        b.extend_from_slice(&n.to_be_bytes());
    }
    field(&mut b, &r.decoder_version)?;
    b.extend([
        u8::from(r.ignore_editlist),
        u8::from(r.original_file_bytes_preserved),
        u8::from(r.audio_present),
    ]);
    let digest = Sha256::digest(&b);
    b.extend_from_slice(&digest);
    if b.len() > MAX_EVIDENCE {
        return Err(PublicationError::Invalid);
    }
    Ok(b)
}
struct Cursor<'a> {
    b: &'a [u8],
    at: usize,
}
fn encode_claims(b: &mut Vec<u8>, c: &Claims) {
    b.extend([
        match c.bytes {
            ByteProvenance::AcquisitionSupported => 0,
            ByteProvenance::Synthesized => 1,
            ByteProvenance::Unavailable => 2,
            ByteProvenance::BindingUnverified => 3,
        },
        match c.ownership {
            Ownership::MetadataSupported => 0,
            Ownership::SharedGeometry => 1,
            Ownership::HistoricalConflict => 2,
            Ownership::Unresolved => 3,
        },
        match c.structure {
            Structure::Valid => 0,
            Structure::RepairedWithRecordedEdits => 1,
            Structure::Invalid => 2,
            Structure::Untested => 3,
        },
        match c.decode {
            Decode::StrictPass => 0,
            Decode::Failure => 1,
            Decode::Untested => 2,
            Decode::TimedOut => 3,
            Decode::Unsupported => 4,
        },
        match c.temporal {
            Temporal::OriginalMappingSupported => 0,
            Temporal::ConstrainedHypothesis => 1,
            Temporal::Unresolved => 2,
        },
    ]);
    let (tag, a, d) = match c.coverage {
        Coverage::CompleteWithinKnownLayout { samples } => (0, samples, 0),
        Coverage::KnownGaps { recovered, listed } => (1, recovered, listed),
        Coverage::UnknownTotal => (2, 0, 0),
    };
    b.push(tag);
    b.extend(a.to_be_bytes());
    b.extend(d.to_be_bytes());
}
fn decode_claims(c: &mut Cursor<'_>) -> Result<Claims> {
    let bytes = match c.take(1)? {
        [0] => ByteProvenance::AcquisitionSupported,
        [1] => ByteProvenance::Synthesized,
        [2] => ByteProvenance::Unavailable,
        [3] => ByteProvenance::BindingUnverified,
        _ => return Err(PublicationError::Corrupt),
    };
    let ownership = match c.take(1)? {
        [0] => Ownership::MetadataSupported,
        [1] => Ownership::SharedGeometry,
        [2] => Ownership::HistoricalConflict,
        [3] => Ownership::Unresolved,
        _ => return Err(PublicationError::Corrupt),
    };
    let structure = match c.take(1)? {
        [0] => Structure::Valid,
        [1] => Structure::RepairedWithRecordedEdits,
        [2] => Structure::Invalid,
        [3] => Structure::Untested,
        _ => return Err(PublicationError::Corrupt),
    };
    let decode = match c.take(1)? {
        [0] => Decode::StrictPass,
        [1] => Decode::Failure,
        [2] => Decode::Untested,
        [3] => Decode::TimedOut,
        [4] => Decode::Unsupported,
        _ => return Err(PublicationError::Corrupt),
    };
    let temporal = match c.take(1)? {
        [0] => Temporal::OriginalMappingSupported,
        [1] => Temporal::ConstrainedHypothesis,
        [2] => Temporal::Unresolved,
        _ => return Err(PublicationError::Corrupt),
    };
    let tag = c.take(1)?[0];
    let a = c.n()?;
    let d = c.n()?;
    let coverage = match (tag, a, d) {
        (0, samples, 0) => Coverage::CompleteWithinKnownLayout { samples },
        (1, recovered, listed) => Coverage::KnownGaps { recovered, listed },
        (2, 0, 0) => Coverage::UnknownTotal,
        _ => return Err(PublicationError::Corrupt),
    };
    let claims = Claims {
        bytes,
        ownership,
        structure,
        decode,
        temporal,
        coverage,
    };
    claims.validate().map_err(|_| PublicationError::Corrupt)?;
    Ok(claims)
}
impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.at.checked_add(n).ok_or(PublicationError::Corrupt)?;
        let x = self.b.get(self.at..end).ok_or(PublicationError::Corrupt)?;
        self.at = end;
        Ok(x)
    }
    fn u32(&mut self) -> Result<u32> {
        u32::try_from(self.n()?).map_err(|_| PublicationError::Corrupt)
    }
    fn i128(&mut self) -> Result<i128> {
        Ok(i128::from_be_bytes(self.take(16)?.try_into().unwrap()))
    }
    fn n(&mut self) -> Result<u64> {
        Ok(u64::from_be_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn text(&mut self) -> Result<String> {
        let n = u32::from_be_bytes(self.take(4)?.try_into().unwrap()) as usize;
        if n == 0 || n > 4096 {
            return Err(PublicationError::Corrupt);
        }
        String::from_utf8(self.take(n)?.to_vec()).map_err(|_| PublicationError::Corrupt)
    }
    fn range(&mut self) -> Result<ByteRange> {
        let start = self.n()?;
        let end = self.n()?;
        ByteRange::new(start, end).map_err(|_| PublicationError::Corrupt)
    }
    fn boolean(&mut self) -> Result<bool> {
        match self.take(1)? {
            [0] => Ok(false),
            [1] => Ok(true),
            _ => Err(PublicationError::Corrupt),
        }
    }
}
fn decode(b: &[u8]) -> Result<MediaEvidence> {
    if b.len() > MAX_EVIDENCE || b.len() < 32 {
        return Err(PublicationError::Corrupt);
    }
    let (body, digest) = b.split_at(b.len() - 32);
    if Sha256::digest(body).as_slice() != digest {
        return Err(PublicationError::Corrupt);
    }
    let mut c = Cursor { b: body, at: 0 };
    if c.take(19)? != b"FH-MEDIA-RECEIPT-3\0" {
        return Err(PublicationError::Corrupt);
    }
    let owner = LogicalOwner {
        source: SourceGeneration(c.text()?),
        filesystem: c.text()?,
        file_identity: c.text()?,
    };
    let generation = c.take(32)?.try_into().unwrap();
    let image_size = c.n()?;
    let map_sha256 = c.take(32)?.try_into().unwrap();
    let input_domain = c.range()?;
    let image_domain = c.range()?;
    let binding = match c.take(1)? {
        [1] => BindingEvidence::OperatorAttested,
        [2] => BindingEvidence::TrustedManifest,
        _ => return Err(PublicationError::Corrupt),
    };
    let archive_count = c.n()?;
    if archive_count > 1024 {
        return Err(PublicationError::Corrupt);
    }
    let mut archive_refs = Vec::with_capacity(archive_count as usize);
    for _ in 0..archive_count {
        archive_refs.push(c.take(32)?.try_into().unwrap());
    }
    let count = c.n()?;
    if count > 16384 {
        return Err(PublicationError::Corrupt);
    }
    let mut consumed = Vec::new();
    for _ in 0..count {
        consumed.push(crate::acquisition::ConsumedRange {
            image: c.range()?,
            sha256: c.take(32)?.try_into().unwrap(),
        });
    }
    let size = c.n()?;
    let metadata_flags = c.n()?;
    let name_flags = c.n()?;
    let attribute_flags = c.n()?;
    let initialized_size = c.n()?;
    let skip_length = c.n()?;
    let count = c.n()?;
    if count > 1024 {
        return Err(PublicationError::Corrupt);
    }
    let mut runs = Vec::new();
    for _ in 0..count {
        let logical = c.range()?;
        let image = Some(c.range()?);
        let raw_flags = c.n()?;
        let allocation_consistent = match c.n()? {
            0 => false,
            1 => true,
            _ => return Err(PublicationError::Corrupt),
        };
        runs.push(crate::filesystem::ExtentRun {
            logical,
            image,
            raw_flags,
            allocation_consistent,
        });
    }
    let provenance = Provenance {
        generation,
        image_size,
        map_sha256,
        input_domain,
        image_domain,
        binding,
        archive_refs,
        consumed,
        layout: crate::filesystem::LogicalFileView {
            owner: owner.clone(),
            size,
            metadata_flags,
            name_flags,
            attribute_flags,
            initialized_size,
            skip_length,
            runs,
            evidence: crate::filesystem::ExtentEvidence::MetadataAllocated,
        },
    };
    let transform = decode_transform(&mut c)?;
    let helper_sha256 = c.take(32)?.try_into().unwrap();
    let helper_read_bytes = c.n()?;
    let claims = decode_claims(&mut c)?;
    let samples = c.n()?;
    let secs = c.n()?;
    let nanos = c.n()?;
    if nanos >= 1_000_000_000 {
        return Err(PublicationError::Corrupt);
    }
    let r = PersistedValidationReceipt {
        transform,
        helper_sha256,
        helper_read_bytes,
        elapsed: Duration::new(secs, nanos as u32),
        claims,
        samples,
        packets: c.n()?,
        frames: c.n()?,
        native_reads: c.n()?,
        staged_read_bytes: c.n()?,
        max_rss_bytes: c.n()?,
        strict_error_flags: c.n()?,
        decoder_version: c.text()?,
        ignore_editlist: c.boolean()?,
        original_file_bytes_preserved: c.boolean()?,
        audio_present: c.boolean()?,
    };
    if c.at != body.len()
        || !receipt_valid(&r)
        || !provenance_valid(&provenance, &owner, &r.transform)
    {
        return Err(PublicationError::Corrupt);
    }
    Ok(MediaEvidence {
        owner,
        receipt: r,
        provenance,
    })
}
impl Publisher {
    /// Retire one source-read epoch only after fresh portable-payload verification
    /// and durable work links. False retains all reads when this artifact does
    /// not cover the entire epoch. Invalid proof attempts preserve dependencies.
    pub fn retire_source_epoch(
        &mut self,
        session: &mut CaseSession,
        epoch: crate::session::ReadEpoch,
        artifact: &Artifact,
        work: &str,
    ) -> Result<bool> {
        let proof = self.retained_source_proof(session, artifact, work);
        match proof {
            Ok(proof) => session
                .retire_read_epoch(epoch, &proof)
                .map_err(PublicationError::Session),
            Err(error) => {
                // Invalid publication evidence must not strand a valid epoch or
                // permit a later caller to discard any of its dependencies.
                let _ = session.retain_read_epoch(epoch);
                Err(error)
            }
        }
    }
    fn retained_source_proof(
        &mut self,
        session: &mut CaseSession,
        artifact: &Artifact,
        work: &str,
    ) -> Result<RetainedSourceProof> {
        let size = fs::metadata(self.root.join("publication.sqlite"))?.len();
        if size > self.limits.database_bytes {
            return Err(PublicationError::Quota);
        }
        let mut active = Some(&mut *session);
        guard(
            &mut active,
            size,
            0,
            0,
            size.saturating_mul(2)
                .saturating_add((MAX_EVIDENCE as u64) * 6),
        )?;
        let record = self
            .existing(&artifact.id)?
            .ok_or(PublicationError::Invalid)?;
        if record.kind != 1 || record.state != "published" || self.artifact(&record)? != *artifact {
            return Err(PublicationError::Conflict);
        }
        let ArtifactKind::ValidatedMedia(e) = self.kind(&record)? else {
            return Err(PublicationError::Invalid);
        };
        source_guard(&mut active, &e.owner)?;
        let published_identity = fs::symlink_metadata(&artifact.path)?;
        self.verify(&artifact.path, record.size, &record.hash, &mut active)?;
        let id = artifact.id.as_bytes();
        let linked=session.with_ledger_resources(ResourceCaps{read_bytes:65536,memory_bytes:196608,index_bytes:0,output_bytes:0,visited_postings:0,transitions:0,decoder_seconds:0},|ledger|{
            if ledger.work_source(work)?!=e.owner.source || !ledger.get(work)?.source_valid {return Ok(false);}
            let outcomes=ledger.receipts(work,crate::ledger::MEDIA_OUTCOME,1)?;
            if outcomes.len()!=1 || outcomes[0].is_empty(){return Ok(false);}
            let (count,max_len,found):(i64,i64,bool)=ledger.connection.query_row("SELECT count(*),coalesce(max(length(payload)),0),coalesce(max(payload=?3),0) FROM receipts WHERE work=?1 AND kind=?2",params![work,crate::ledger::PORTABLE_ARTIFACT,id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(crate::ledger::LedgerError::from)?;
            Ok(count>0 && count<=64 && max_len==64 && found)
        })?;
        if !linked {
            return Err(PublicationError::Conflict);
        }
        if !unchanged_file(&published_identity, &fs::symlink_metadata(&artifact.path)?) {
            return Err(PublicationError::Changed);
        }
        let p = e.provenance;
        Ok(RetainedSourceProof {
            generation: p.generation,
            image_size: p.image_size,
            map_sha256: p.map_sha256,
            input_domain: p.input_domain,
            image_domain: p.image_domain,
            binding: p.binding,
            archive_refs: p.archive_refs,
            consumed: p.consumed,
        })
    }
    /// Metadata foundation shared by synthetic tests. Media operations additionally
    /// require the case authority. Reserve database and rollback-journal headroom.
    pub fn create(path: &Path, limits: PublicationLimits) -> Result<Self> {
        safe_path(path)?;
        fs::create_dir(path)?;
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path.join("OWNER"))?;
        f.write_all(OWNER)?;
        f.sync_all()?;
        sync_dir(path)?;
        if let Some(parent) = path.parent() {
            sync_dir(parent)?;
        }
        let db = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path.join("publication.sqlite"))?;
        db.sync_all()?;
        let this = Self::connect(path, limits, true, false)?;
        this.db.execute_batch("BEGIN IMMEDIATE;CREATE TABLE artifacts(id TEXT PRIMARY KEY,hash TEXT NOT NULL,size INTEGER NOT NULL CHECK(size>=0),state TEXT NOT NULL CHECK(state IN ('intent','published')),kind INTEGER NOT NULL CHECK(kind IN (0,1)),evidence BLOB NOT NULL CHECK(length(evidence)<=1048576));CREATE TABLE failures(sequence INTEGER PRIMARY KEY,id TEXT NOT NULL,reason TEXT NOT NULL);PRAGMA application_id=1179144277;PRAGMA user_version=2;COMMIT;")?;
        sync_dir(path)?;
        Ok(this)
    }
    pub fn open(path: &Path, limits: PublicationLimits) -> Result<Self> {
        Self::connect(path, limits, false, false)
    }
    pub fn open_readonly(path: &Path, limits: PublicationLimits) -> Result<Self> {
        Self::connect(path, limits, false, true)
    }
    fn connect(path: &Path, limits: PublicationLimits, new: bool, readonly: bool) -> Result<Self> {
        if limits.artifact_bytes == 0
            || limits.output_bytes == 0
            || limits.verification_bytes == 0
            || limits.artifacts == 0
            || limits.database_bytes < 1024 * 1024
            || limits.database_bytes > i64::MAX as u64
        {
            return Err(PublicationError::Invalid);
        }
        safe_path(path)?;
        regular(&path.join("OWNER"))?;
        if fs::metadata(path.join("OWNER"))?.len() != OWNER.len() as u64
            || fs::read(path.join("OWNER"))? != OWNER
        {
            return Err(PublicationError::Unowned);
        }
        let dbpath = path.join("publication.sqlite");
        regular(&dbpath)?;
        for name in [
            "publication.sqlite-journal",
            "publication.sqlite-wal",
            "publication.sqlite-shm",
        ] {
            let p = path.join(name);
            if fs::symlink_metadata(&p).is_ok() {
                regular(&p)?;
            }
        }
        let db = Connection::open_with_flags(
            dbpath,
            (if readonly {
                OpenFlags::SQLITE_OPEN_READ_ONLY
            } else {
                OpenFlags::SQLITE_OPEN_READ_WRITE
            }) | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        let app: i64 = db.query_row("PRAGMA application_id", [], |r| r.get(0))?;
        let version: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if (!new && (app != APP || version != 2)) || (new && (app != 0 || version != 0)) {
            return Err(PublicationError::Unowned);
        }
        if !readonly {
            db.execute_batch(
                "PRAGMA journal_mode=DELETE;PRAGMA synchronous=EXTRA;PRAGMA fullfsync=ON;",
            )?;
        }
        let page: i64 = db.query_row("PRAGMA page_size", [], |r| r.get(0))?;
        let pages: i64 = db.query_row("PRAGMA page_count", [], |r| r.get(0))?;
        if pages as u64 > limits.database_bytes / page as u64 {
            return Err(PublicationError::Quota);
        }
        if !readonly {
            db.pragma_update(
                None,
                "max_page_count",
                (limits.database_bytes / page as u64) as i64,
            )?;
        }
        Ok(Self {
            root: path.into(),
            db,
            limits,
            verified: 0,
        })
    }
    fn file(&self, id: &str, stage: bool, kind: u8) -> Result<PathBuf> {
        if !valid_id(id) || kind > 1 {
            return Err(PublicationError::Corrupt);
        }
        Ok(self.root.join(format!(
            "{id}.{}",
            if stage {
                "stage"
            } else if kind == 0 {
                "synthetic"
            } else {
                "mp4"
            }
        )))
    }
    fn kind(&self, r: &Record) -> Result<ArtifactKind> {
        if !valid_id(&r.id) || !valid_id(&r.hash) || r.size > self.limits.artifact_bytes {
            return Err(PublicationError::Corrupt);
        }
        match r.kind {
            0 if r.id == r.hash && r.evidence == SYNTHETIC => Ok(ArtifactKind::SyntheticFixture),
            1 => {
                let e = decode(&r.evidence)?;
                if media_id(&e.owner, &r.hash, &e.receipt.transform)? != r.id {
                    return Err(PublicationError::Corrupt);
                }
                Ok(ArtifactKind::ValidatedMedia(Box::new(e)))
            }
            _ => Err(PublicationError::Corrupt),
        }
    }
    fn verify(
        &mut self,
        path: &Path,
        size: u64,
        hash: &str,
        session: &mut Option<&mut CaseSession>,
    ) -> Result<()> {
        if size > self.limits.artifact_bytes {
            return Err(PublicationError::Quota);
        }
        guard(session, 0, 0, 0, BUFFER as u64)?;
        regular(path)?;
        let observed = fs::symlink_metadata(path)?;
        if observed.len() != size {
            return Err(PublicationError::Corrupt);
        }
        if self
            .verified
            .checked_add(size)
            .is_none_or(|n| n > self.limits.verification_bytes)
        {
            return Err(PublicationError::Quota);
        }
        self.verified += size;
        let mut f = File::open(path)?;
        if !unchanged_file(&observed, &f.metadata()?) {
            return Err(PublicationError::Changed);
        }
        let mut h = Sha256::new();
        let mut b = [0u8; BUFFER];
        let mut left = size;
        while left > 0 {
            let n = left.min(BUFFER as u64) as usize;
            guard(session, n as u64, 0, 0, BUFFER as u64)?;
            f.read_exact(&mut b[..n])?;
            h.update(&b[..n]);
            left -= n as u64;
        }
        guard(session, 0, 0, 0, BUFFER as u64)?;
        if !unchanged_file(&observed, &f.metadata()?)
            || !unchanged_file(&observed, &fs::symlink_metadata(path)?)
        {
            return Err(PublicationError::Changed);
        }
        if hex(&h.finalize()) != hash {
            return Err(PublicationError::Corrupt);
        }
        Ok(())
    }
    fn rows(&self, kind: Option<u8>, published: bool) -> Result<Vec<Record>> {
        let count: i64 = self
            .db
            .query_row("SELECT count(*) FROM artifacts", [], |r| r.get(0))?;
        if count < 0 || count as u64 > self.limits.artifacts as u64 {
            return Err(PublicationError::Quota);
        }
        let mut q=self.db.prepare("SELECT id,hash,size,state,kind,evidence FROM artifacts WHERE (?1 IS NULL OR kind=?1) AND (?2=0 OR state='published') ORDER BY id")?;
        let raw = q
            .query_map(params![kind.map(i64::from), i64::from(published)], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, u8>(4)?,
                    r.get::<_, Vec<u8>>(5)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        raw.into_iter()
            .map(|(id, hash, size, state, kind, evidence)| {
                if evidence.len() > MAX_EVIDENCE
                    || !matches!(state.as_str(), "intent" | "published")
                {
                    return Err(PublicationError::Corrupt);
                }
                Ok(Record {
                    id,
                    hash,
                    size: u64::try_from(size).map_err(|_| PublicationError::Corrupt)?,
                    state,
                    kind,
                    evidence,
                })
            })
            .collect()
    }
    fn allocation(&self) -> Result<(u64, u64)> {
        let mut payloads = std::collections::BTreeMap::<String, u64>::new();
        let mut physical = std::collections::BTreeMap::<String, PathBuf>::new();
        for r in self.rows(None, false)? {
            self.kind(&r)?;
            payloads.insert(r.id, r.size);
        }
        let mut entries = 0usize;
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            if entry.file_name() == ".DS_Store" {
                continue;
            }
            entries += 1;
            if entries > self.limits.artifacts.saturating_mul(2).saturating_add(8) {
                return Err(PublicationError::Quota);
            }
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| PublicationError::Unowned)?;
            if ["OWNER", "publication.sqlite", "publication.sqlite-journal"]
                .contains(&name.as_str())
            {
                regular(&entry.path())?;
                continue;
            }
            let (id, ext) = name.rsplit_once('.').ok_or(PublicationError::Unowned)?;
            if !valid_id(id) || !["stage", "synthetic", "mp4"].contains(&ext) {
                return Err(PublicationError::Unowned);
            }
            regular(&entry.path())?;
            if let Some(previous) = physical.get(id) {
                same_inode(previous, &entry.path())?;
            } else {
                physical.insert(id.into(), entry.path());
            }
            let size = entry.metadata()?.len();
            if size > self.limits.artifact_bytes {
                return Err(PublicationError::Quota);
            }
            let prior = payloads.entry(id.into()).or_insert(size);
            if *prior != size {
                return Err(PublicationError::Corrupt);
            }
        }
        let total = payloads.values().try_fold(0u64, |a, b| {
            a.checked_add(*b).ok_or(PublicationError::Quota)
        })?;
        Ok((payloads.len() as u64, total))
    }
    fn admit_new(&self, r: &Record) -> Result<()> {
        self.kind(r)?;
        let (count, total) = self.allocation()?;
        if count >= self.limits.artifacts as u64
            || total
                .checked_add(r.size)
                .is_none_or(|n| n > self.limits.output_bytes)
        {
            return Err(PublicationError::Quota);
        }
        if fs::symlink_metadata(self.file(&r.id, false, r.kind)?).is_ok() {
            return Err(PublicationError::Conflict);
        }
        Ok(())
    }
    fn existing(&self, id: &str) -> Result<Option<Record>> {
        let present: bool = self.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM artifacts WHERE id=?1)",
            [id],
            |r| r.get(0),
        )?;
        if !present {
            return Ok(None);
        }
        Ok(self.rows(None, false)?.into_iter().find(|r| r.id == id))
    }
    fn intent(&self, r: &Record) -> Result<()> {
        self.db.execute("INSERT INTO artifacts(id,hash,size,state,kind,evidence)VALUES(?1,?2,?3,'intent',?4,?5)",params![r.id,r.hash,i64::try_from(r.size).map_err(|_|PublicationError::Invalid)?,r.kind,r.evidence])?;
        Ok(())
    }
    fn artifact(&self, r: &Record) -> Result<Artifact> {
        Ok(Artifact {
            id: r.id.clone(),
            path: self.file(&r.id, false, r.kind)?,
            bytes: r.size,
            sha256: r.hash.clone(),
            kind: self.kind(r)?,
        })
    }
    fn reconcile_one(
        &mut self,
        r: &Record,
        at: FaultPoint,
        session: &mut Option<&mut CaseSession>,
    ) -> Result<()> {
        let kind = self.kind(r)?;
        if let ArtifactKind::ValidatedMedia(e) = kind
            && r.state != "published"
        {
            source_guard(session, &e.owner)?;
        }
        let stage = self.file(&r.id, true, r.kind)?;
        let final_path = self.file(&r.id, false, r.kind)?;
        guard(session, 0, 0, 0, BUFFER as u64)?;
        if fs::symlink_metadata(&final_path).is_ok() {
            if r.state == "intent" {
                same_inode(&stage, &final_path)?;
            }
            self.verify(&final_path, r.size, &r.hash, session)?;
        } else {
            if r.state == "published" {
                return Err(PublicationError::Corrupt);
            }
            self.verify(&stage, r.size, &r.hash, session)?;
            guard(session, 0, 0, 0, BUFFER as u64)?;
            fs::hard_link(&stage, &final_path).map_err(|e| {
                if e.kind() == io::ErrorKind::Unsupported {
                    PublicationError::UnsupportedFilesystem
                } else {
                    e.into()
                }
            })?;
        }
        fault(FaultPoint::FinalLinked, at)?;
        guard(session, 0, 0, 0, BUFFER as u64)?;
        sync_dir(&self.root)?;
        fault(FaultPoint::DirectoryFlushed, at)?;
        if r.state != "published" {
            guard(session, 0, 0, 0, BUFFER as u64)?;
            self.db.execute(
                "UPDATE artifacts SET state='published' WHERE id=?1 AND state='intent'",
                [&r.id],
            )?;
        }
        fault(FaultPoint::PublishedCommitted, at)?;
        guard(session, 0, 0, 0, BUFFER as u64)?;
        if fs::symlink_metadata(&stage).is_ok() {
            self.verify(&stage, r.size, &r.hash, session)?;
            same_inode(&stage, &final_path)?;
            fs::remove_file(stage)?;
            sync_dir(&self.root)?;
        }
        Ok(())
    }
    pub fn verification_bytes(&self) -> u64 {
        self.verified
    }
    pub fn publish_synthetic(
        &mut self,
        receipt: &SyntheticFixtureReceipt,
        bytes: &[u8],
        at: FaultPoint,
    ) -> Result<Artifact> {
        if receipt.size != bytes.len() as u64 || receipt.hash != hex(&Sha256::digest(bytes)) {
            return Err(PublicationError::Invalid);
        }
        let r = Record {
            id: receipt.hash.clone(),
            hash: receipt.hash.clone(),
            size: receipt.size,
            state: "intent".into(),
            kind: 0,
            evidence: SYNTHETIC.into(),
        };
        if let Some(prior) = self.existing(&r.id)? {
            if prior.kind != 0 || prior.size != r.size {
                return Err(PublicationError::Conflict);
            }
            self.reconcile_one(&prior, FaultPoint::None, &mut None)?;
            return self.artifact(&prior);
        }
        self.admit_new(&r)?;
        let stage = self.file(&r.id, true, 0)?;
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&stage)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        sync_dir(&self.root)?;
        fault(FaultPoint::StageFlushed, at)?;
        self.verify(&stage, r.size, &r.hash, &mut None)?;
        self.intent(&r)?;
        fault(FaultPoint::IntentCommitted, at)?;
        self.reconcile_one(&r, at, &mut None)?;
        self.artifact(&r)
    }
    pub fn publish_media(
        &mut self,
        session: &mut CaseSession,
        validated: &ValidatedArtifact,
        at: FaultPoint,
    ) -> Result<Artifact> {
        let mut session = Some(session);
        source_guard(&mut session, validated.owner())?;
        let manifest = validated.source_manifest();
        if manifest.consumed().len() > 16384
            || manifest.archive_refs().len() > 1024
            || validated.source_layout().runs.len() > 1024
        {
            return Err(PublicationError::Invalid);
        }
        if let crate::validation::ArtifactTransform::SamplePreserving(e) =
            &validated.receipt().transform
            && !segment_shape(e)
        {
            return Err(PublicationError::Invalid);
        }
        guard(
            &mut session,
            0,
            0,
            0,
            self.limits
                .database_bytes
                .saturating_mul(2)
                .saturating_add((MAX_EVIDENCE as u64).saturating_mul(6)),
        )?;
        let evidence = MediaEvidence {
            provenance: Provenance {
                generation: manifest.generation_id(),
                image_size: manifest.image_size(),
                map_sha256: manifest.map_digest(),
                input_domain: manifest.binding().input_domain(),
                image_domain: manifest.binding().image_domain(),
                binding: manifest.binding().evidence(),
                archive_refs: manifest.archive_refs().to_vec(),
                consumed: manifest.consumed().to_vec(),
                layout: validated.source_layout().clone(),
            },
            owner: validated.owner().clone(),
            receipt: validated.receipt().into(),
        };
        let hash = hex(&validated.sha256());
        let r = Record {
            id: media_id(validated.owner(), &hash, &evidence.receipt.transform)?,
            hash,
            size: validated.bytes(),
            state: "intent".into(),
            kind: 1,
            evidence: encode(&evidence)?,
        };
        guard(
            &mut session,
            0,
            0,
            0,
            self.limits.database_bytes.saturating_mul(2),
        )?;
        if let Some(prior) = self.existing(&r.id)? {
            if prior.kind != 1
                || prior.hash != r.hash
                || prior.size != r.size
                || decode(&prior.evidence)?.owner != evidence.owner
            {
                return Err(PublicationError::Conflict);
            }
            self.reconcile_one(&prior, FaultPoint::None, &mut session)?;
            return self.artifact(&prior);
        }
        self.admit_new(&r)?;
        guard(&mut session, 0, r.size, 0, BUFFER as u64)?;
        let mut source = validated
            .open_verified()
            .map_err(|_| PublicationError::Changed)?;
        let stage = self.file(&r.id, true, 1)?;
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&stage)?;
        let mut b = [0u8; BUFFER];
        let mut left = r.size;
        while left > 0 {
            let n = left.min(BUFFER as u64) as usize;
            guard(&mut session, n as u64, 0, 0, BUFFER as u64)?;
            validated
                .check_file(&source)
                .map_err(|_| PublicationError::Changed)?;
            source.read_exact(&mut b[..n])?;
            guard(&mut session, 0, 0, 0, BUFFER as u64)?;
            f.write_all(&b[..n])?;
            left -= n as u64;
        }
        validated
            .check_file(&source)
            .map_err(|_| PublicationError::Changed)?;
        guard(&mut session, 0, 0, 0, BUFFER as u64)?;
        f.sync_all()?;
        sync_dir(&self.root)?;
        fault(FaultPoint::StageFlushed, at)?;
        self.verify(&stage, r.size, &r.hash, &mut session)?;
        source_guard(&mut session, &evidence.owner)?;
        validated
            .check_file(&source)
            .map_err(|_| PublicationError::Changed)?;
        guard(&mut session, 0, 0, r.evidence.len() as u64, BUFFER as u64)?;
        self.intent(&r)?;
        fault(FaultPoint::IntentCommitted, at)?;
        self.reconcile_one(&r, at, &mut session)?;
        self.artifact(&r)
    }
    fn reconcile_kind(
        &mut self,
        kind: u8,
        session: &mut Option<&mut CaseSession>,
    ) -> Result<usize> {
        guard(
            session,
            0,
            0,
            0,
            self.limits.database_bytes.saturating_mul(2),
        )?;
        let rows = self.rows(Some(kind), false)?;
        let mut n = 0;
        for r in rows {
            match self.reconcile_one(&r, FaultPoint::None, session) {
                Ok(()) => n += 1,
                Err(e) => {
                    guard(session, 0, 0, 0, BUFFER as u64)?;
                    self.db.execute(
                        "INSERT INTO failures(id,reason)VALUES(?1,?2)",
                        params![r.id, e.to_string()],
                    )?;
                    return Err(e);
                }
            }
        }
        Ok(n)
    }
    pub fn reconcile_synthetic(&mut self) -> Result<usize> {
        self.reconcile_kind(0, &mut None)
    }
    pub fn reconcile_media(&mut self, session: &mut CaseSession) -> Result<usize> {
        self.reconcile_kind(1, &mut Some(session))
    }
    fn published_kind(
        &mut self,
        kind: u8,
        session: &mut Option<&mut CaseSession>,
    ) -> Result<Vec<Artifact>> {
        guard(
            session,
            0,
            0,
            0,
            self.limits.database_bytes.saturating_mul(2),
        )?;
        let rows = self.rows(Some(kind), true)?;
        let mut out = Vec::new();
        for r in rows {
            let path = self.file(&r.id, false, r.kind)?;
            self.verify(&path, r.size, &r.hash, session)?;
            out.push(self.artifact(&r)?);
        }
        Ok(out)
    }
    pub fn published_synthetic(&mut self) -> Result<Vec<Artifact>> {
        self.published_kind(0, &mut None)
    }
    pub fn published_media(&mut self) -> Result<Vec<Artifact>> {
        self.published_kind(1, &mut None)
    }
    /// Fresh payload verification charged to the active per-image authority.
    pub fn published_media_with_session(
        &mut self,
        session: &mut CaseSession,
    ) -> Result<Vec<Artifact>> {
        self.published_kind(1, &mut Some(session))
    }
    /// Inspect committed, bounded stored receipts only. This performs no payload
    /// reads and makes no fresh claim about present file bytes or existence.
    pub fn media_receipts(&self) -> Result<Vec<Artifact>> {
        self.rows(Some(1), true)?
            .iter()
            .map(|r| self.artifact(r))
            .collect()
    }
}
