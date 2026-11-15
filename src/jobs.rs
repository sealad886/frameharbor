//! Restartable job inputs and the canonical bounded logical-view codec.
//! Partial discovery records retain geometry only: full enumeration must finish
//! conflict qualification before any record can authorize media publication.
use crate::{
    acquisition::ReadLimits,
    contracts::{BindingEvidence, ByteRange, CoordinateBinding, LogicalOwner, SourceGeneration},
    control::CaseConfig,
    filesystem::{ExtentEvidence, ExtentRun, FilesystemProfile, LogicalFileView},
    ledger::{Ledger, LedgerError, WorkSpec},
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{self, Read},
    path::{Component, Path, PathBuf},
};
const MAX_MANIFEST: usize = 65536;
const MAX_VIEW: usize = 128 * 1024;
#[derive(Debug)]
pub enum JobError {
    Io(io::Error),
    Ledger(LedgerError),
    Invalid,
    Conflict,
    Missing,
    Corrupt,
    SourceMismatch,
    ResourceLimit,
}
impl std::fmt::Display for JobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for JobError {}
impl From<io::Error> for JobError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<LedgerError> for JobError {
    fn from(e: LedgerError) -> Self {
        Self::Ledger(e)
    }
}
type Result<T> = std::result::Result<T, JobError>;
fn hash(b: &[u8]) -> String {
    format!("{:x}", Sha256::digest(b))
}
fn number(b: &mut Vec<u8>, n: u64) {
    b.extend(n.to_be_bytes());
}
fn text(b: &mut Vec<u8>, s: &str) -> Result<()> {
    if s.is_empty() || s.len() > 4096 {
        return Err(JobError::Invalid);
    }
    number(b, s.len() as u64);
    b.extend(s.as_bytes());
    Ok(())
}
fn path(b: &mut Vec<u8>, p: &Path) -> Result<()> {
    if !p.is_absolute()
        || p.components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(JobError::Invalid);
    }
    text(b, p.to_str().ok_or(JobError::Invalid)?)
}
fn finish(mut b: Vec<u8>, max: usize) -> Result<Vec<u8>> {
    if b.len() + 32 > max {
        return Err(JobError::ResourceLimit);
    }
    b.extend(Sha256::digest(&b));
    Ok(b)
}
struct Cursor<'a> {
    b: &'a [u8],
    at: usize,
}
impl<'a> Cursor<'a> {
    fn new(b: &'a [u8], magic: &[u8], max: usize) -> Result<Self> {
        if b.len() > max
            || b.len() < magic.len() + 32
            || !b.starts_with(magic)
            || Sha256::digest(&b[..b.len() - 32]).as_slice() != &b[b.len() - 32..]
        {
            return Err(JobError::Corrupt);
        }
        Ok(Self {
            b: &b[..b.len() - 32],
            at: magic.len(),
        })
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.at.checked_add(n).ok_or(JobError::Corrupt)?;
        let b = self.b.get(self.at..end).ok_or(JobError::Corrupt)?;
        self.at = end;
        Ok(b)
    }
    fn n(&mut self) -> Result<u64> {
        Ok(u64::from_be_bytes(
            self.take(8)?.try_into().map_err(|_| JobError::Corrupt)?,
        ))
    }
    fn text(&mut self) -> Result<String> {
        let n = self.n()?;
        if n == 0 || n > 4096 {
            return Err(JobError::Corrupt);
        }
        String::from_utf8(self.take(n as usize)?.to_vec()).map_err(|_| JobError::Corrupt)
    }
    fn end(self) -> Result<()> {
        if self.at == self.b.len() {
            Ok(())
        } else {
            Err(JobError::Corrupt)
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelperPin {
    pub path: PathBuf,
    pub sha256: [u8; 32],
}
#[derive(Debug, Clone)]
pub struct JobManifest {
    pub image: PathBuf,
    pub map: PathBuf,
    pub case_dir: PathBuf,
    pub output_dir: PathBuf,
    pub binding: CoordinateBinding,
    pub profile: FilesystemProfile,
    pub fs_start: u64,
    pub filesystem_helper: HelperPin,
    pub media_helper: HelperPin,
    pub read_limits: ReadLimits,
    pub case_config: CaseConfig,
    pub ledger_bytes: u64,
    pub index_bytes: u64,
    pub checkpoint_history: usize,
}
impl JobManifest {
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.binding.evidence() == BindingEvidence::Unverified
            || self.fs_start >= self.binding.image_domain().end()
            || self.ledger_bytes < 1024 * 1024
            || self.ledger_bytes > 1024 * 1024 * 1024
            || self.index_bytes == 0
            || self.index_bytes > 1024 * 1024 * 1024
            || self.checkpoint_history > 64
            || self.read_limits.per_read == 0
            || self.read_limits.per_read > 1024 * 1024
            || self.read_limits.receipts == 0
            || self.read_limits.receipts > 16384
            || self.read_limits.total == 0
            || self.case_config.total_millis == 0
            || self.case_config.shutdown_reserve_millis >= self.case_config.total_millis
        {
            return Err(JobError::Invalid);
        }
        let mut b = b"FHJOB2\0".to_vec();
        for p in [
            &self.image,
            &self.map,
            &self.case_dir,
            &self.output_dir,
            &self.filesystem_helper.path,
            &self.media_helper.path,
        ] {
            path(&mut b, p)?;
        }
        for pin in [&self.filesystem_helper, &self.media_helper] {
            b.extend(pin.sha256);
        }
        let input = self.binding.input_domain();
        let image = self.binding.image_domain();
        for n in [
            input.start(),
            input.end(),
            image.start(),
            image.end(),
            match self.binding.evidence() {
                BindingEvidence::OperatorAttested => 1,
                BindingEvidence::TrustedManifest => 2,
                _ => 0,
            },
            match self.profile {
                FilesystemProfile::Fat32 => 0,
                FilesystemProfile::ExFat => 1,
            },
            self.fs_start,
            self.read_limits.per_read,
            self.read_limits.total,
            self.read_limits.receipts as u64,
            self.case_config.total_millis,
            self.case_config.shutdown_reserve_millis,
            self.ledger_bytes,
            self.index_bytes,
            self.checkpoint_history as u64,
        ] {
            number(&mut b, n);
        }
        let c = self.case_config.caps;
        for n in [
            c.read_bytes,
            c.memory_bytes,
            c.index_bytes,
            c.output_bytes,
            c.visited_postings,
            c.transitions,
            c.decoder_seconds,
        ] {
            number(&mut b, n);
        }
        finish(b, MAX_MANIFEST)
    }
    pub fn decode(b: &[u8]) -> Result<Self> {
        let mut c = Cursor::new(b, b"FHJOB2\0", MAX_MANIFEST)?;
        let image = PathBuf::from(c.text()?);
        let map = PathBuf::from(c.text()?);
        let case_dir = PathBuf::from(c.text()?);
        let output_dir = PathBuf::from(c.text()?);
        let fs_path = PathBuf::from(c.text()?);
        let media_path = PathBuf::from(c.text()?);
        let fs_hash = c.take(32)?.try_into().map_err(|_| JobError::Corrupt)?;
        let media_hash = c.take(32)?.try_into().map_err(|_| JobError::Corrupt)?;
        let input = ByteRange::new(c.n()?, c.n()?).map_err(|_| JobError::Corrupt)?;
        let image_domain = ByteRange::new(c.n()?, c.n()?).map_err(|_| JobError::Corrupt)?;
        let evidence = match c.n()? {
            1 => BindingEvidence::OperatorAttested,
            2 => BindingEvidence::TrustedManifest,
            _ => return Err(JobError::Corrupt),
        };
        let binding = CoordinateBinding::new(
            input,
            i128::from(image_domain.start()) - i128::from(input.start()),
            image_domain.end(),
            evidence,
        )
        .map_err(|_| JobError::Corrupt)?;
        if binding.image_domain() != image_domain {
            return Err(JobError::Corrupt);
        }
        let profile = match c.n()? {
            0 => FilesystemProfile::Fat32,
            1 => FilesystemProfile::ExFat,
            _ => return Err(JobError::Corrupt),
        };
        let fs_start = c.n()?;
        let read_limits = ReadLimits {
            per_read: c.n()?,
            total: c.n()?,
            receipts: usize::try_from(c.n()?).map_err(|_| JobError::Corrupt)?,
        };
        let total_millis = c.n()?;
        let shutdown_reserve_millis = c.n()?;
        let ledger_bytes = c.n()?;
        let index_bytes = c.n()?;
        let checkpoint_history = usize::try_from(c.n()?).map_err(|_| JobError::Corrupt)?;
        let caps = crate::contracts::ResourceCaps {
            read_bytes: c.n()?,
            memory_bytes: c.n()?,
            index_bytes: c.n()?,
            output_bytes: c.n()?,
            visited_postings: c.n()?,
            transitions: c.n()?,
            decoder_seconds: c.n()?,
        };
        c.end()?;
        let m = Self {
            image,
            map,
            case_dir,
            output_dir,
            binding,
            profile,
            fs_start,
            filesystem_helper: HelperPin {
                path: fs_path,
                sha256: fs_hash,
            },
            media_helper: HelperPin {
                path: media_path,
                sha256: media_hash,
            },
            read_limits,
            case_config: CaseConfig {
                total_millis,
                shutdown_reserve_millis,
                caps,
            },
            ledger_bytes,
            index_bytes,
            checkpoint_history,
        };
        if m.encode()? != b {
            return Err(JobError::Corrupt);
        }
        Ok(m)
    }
    pub fn persist(&self, ledger: &mut Ledger) -> Result<()> {
        ledger.set_case_manifest(&self.encode()?)?;
        Ok(())
    }
    pub fn load(ledger: &Ledger) -> Result<Self> {
        Self::decode(&ledger.case_manifest()?.ok_or(JobError::Missing)?)
    }
    pub fn load_from_case(case_dir: &Path) -> Result<Self> {
        directory(case_dir)?;
        Self::decode(&Ledger::read_case_manifest(&case_dir.join("case.sqlite"))?)
    }
    pub fn discovery_spec(&self, source: SourceGeneration) -> Result<WorkSpec> {
        Ok(WorkSpec {
            source,
            algorithm: "tsk-4.14.0-directory-cursor-v2".into(),
            stage: "filesystem-discovery".into(),
            parameters: BTreeMap::from([("manifest".into(), hash(&self.encode()?))]),
        })
    }
    /// Returns actual bytes hashed. Caller must admit the 32 MiB maximum before
    /// invoking this bounded startup check through the shared case authority.
    pub fn verify_helpers(&self) -> Result<u64> {
        let mut bytes = 0;
        for pin in [&self.filesystem_helper, &self.media_helper] {
            let meta = fs::symlink_metadata(&pin.path)?;
            if !meta.is_file() || meta.len() == 0 || meta.len() > 16 * 1024 * 1024 {
                return Err(JobError::Invalid);
            }
            let mut f = File::open(&pin.path)?;
            if !same_file(&meta, &f.metadata()?) {
                return Err(JobError::Conflict);
            }
            let mut hasher = Sha256::new();
            let mut buf = [0u8; 65536];
            let mut left = meta.len();
            while left > 0 {
                let n =
                    usize::try_from(left.min(buf.len() as u64)).map_err(|_| JobError::Invalid)?;
                f.read_exact(&mut buf[..n])?;
                hasher.update(&buf[..n]);
                left -= n as u64;
            }
            if !same_file(&meta, &f.metadata()?)
                || !same_file(&meta, &fs::symlink_metadata(&pin.path)?)
                || hasher.finalize().as_slice() != pin.sha256
            {
                return Err(JobError::Conflict);
            }
            bytes += meta.len();
        }
        Ok(bytes)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewStatus {
    Provisional,
    Qualified,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedView {
    pub id: String,
    pub view: LogicalFileView,
    pub status: ViewStatus,
}
impl IndexedView {
    pub fn media_spec(&self) -> Result<WorkSpec> {
        if self.status != ViewStatus::Qualified {
            return Err(JobError::Invalid);
        }
        Ok(WorkSpec {
            source: self.view.owner.source.clone(),
            algorithm: "qualified-media-export-v2".into(),
            stage: "media-export".into(),
            parameters: BTreeMap::from([("view-index".into(), self.id.clone())]),
        })
    }
}
fn directory(p: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(p)?;
    if !meta.is_dir() || fs::canonicalize(p)? != p {
        return Err(JobError::Invalid);
    }
    Ok(())
}
fn same_file(a: &fs::Metadata, b: &fs::Metadata) -> bool {
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
fn evidence(e: ExtentEvidence) -> u64 {
    match e {
        ExtentEvidence::MetadataAllocated => 0,
        ExtentEvidence::MetadataCandidate => 1,
        ExtentEvidence::DeletedOrRecovery => 2,
        ExtentEvidence::SharedGeometry => 3,
        ExtentEvidence::Partial => 4,
        ExtentEvidence::Unsupported => 5,
    }
}
pub(crate) fn view_bytes(v: &LogicalFileView) -> Result<u64> {
    if v.runs.len() > 1024 {
        return Err(JobError::ResourceLimit);
    }
    // Magic + three length prefixes + nine scalar fields + checksum.
    let mut bytes = 136u64;
    for text in [
        &v.owner.source.0,
        &v.owner.filesystem,
        &v.owner.file_identity,
    ] {
        if text.is_empty() || text.len() > 4096 {
            return Err(JobError::Invalid);
        }
        bytes = bytes
            .checked_add(text.len() as u64)
            .ok_or(JobError::ResourceLimit)?;
    }
    let mut end = 0;
    for run in &v.runs {
        if run.logical.start() < end
            || run.logical.end() > v.size
            || run.image.is_some_and(|i| i.len() != run.logical.len())
        {
            return Err(JobError::Invalid);
        }
        end = run.logical.end();
        bytes = bytes.checked_add(56).ok_or(JobError::ResourceLimit)?;
    }
    if bytes > MAX_VIEW as u64 {
        return Err(JobError::ResourceLimit);
    }
    Ok(bytes)
}
pub(crate) fn encode_view(v: &LogicalFileView, status: ViewStatus) -> Result<Vec<u8>> {
    view_bytes(v)?;
    if v.runs.len() > 1024 {
        return Err(JobError::ResourceLimit);
    }
    let mut b = b"FHVIEW1\0".to_vec();
    for t in [
        &v.owner.source.0,
        &v.owner.filesystem,
        &v.owner.file_identity,
    ] {
        text(&mut b, t)?;
    }
    for n in [
        if status == ViewStatus::Qualified {
            1
        } else {
            0
        },
        evidence(v.evidence),
        v.size,
        v.metadata_flags,
        v.name_flags,
        v.attribute_flags,
        v.initialized_size,
        v.skip_length,
        v.runs.len() as u64,
    ] {
        number(&mut b, n);
    }
    let mut end = 0;
    for r in &v.runs {
        if r.logical.start() < end
            || r.logical.end() > v.size
            || r.image.is_some_and(|i| i.len() != r.logical.len())
        {
            return Err(JobError::Invalid);
        }
        end = r.logical.end();
        for n in [
            r.logical.start(),
            r.logical.end(),
            u64::from(r.image.is_some()),
            r.image.map_or(0, |i| i.start()),
            r.image.map_or(0, |i| i.end()),
            r.raw_flags,
            u64::from(r.allocation_consistent),
        ] {
            number(&mut b, n);
        }
    }
    finish(b, MAX_VIEW)
}
pub(crate) fn decode_view(b: &[u8]) -> Result<(LogicalFileView, ViewStatus)> {
    let mut c = Cursor::new(b, b"FHVIEW1\0", MAX_VIEW)?;
    let owner = LogicalOwner {
        source: SourceGeneration(c.text()?),
        filesystem: c.text()?,
        file_identity: c.text()?,
    };
    let status = match c.n()? {
        0 => ViewStatus::Provisional,
        1 => ViewStatus::Qualified,
        _ => return Err(JobError::Corrupt),
    };
    let evidence = match c.n()? {
        0 => ExtentEvidence::MetadataAllocated,
        1 => ExtentEvidence::MetadataCandidate,
        2 => ExtentEvidence::DeletedOrRecovery,
        3 => ExtentEvidence::SharedGeometry,
        4 => ExtentEvidence::Partial,
        5 => ExtentEvidence::Unsupported,
        _ => return Err(JobError::Corrupt),
    };
    let size = c.n()?;
    let metadata_flags = c.n()?;
    let name_flags = c.n()?;
    let attribute_flags = c.n()?;
    let initialized_size = c.n()?;
    let skip_length = c.n()?;
    let count = c.n()?;
    if count > 1024 {
        return Err(JobError::ResourceLimit);
    }
    let mut runs = Vec::new();
    for _ in 0..count {
        let logical = ByteRange::new(c.n()?, c.n()?).map_err(|_| JobError::Corrupt)?;
        let has = c.n()?;
        let start = c.n()?;
        let end = c.n()?;
        let image = match has {
            0 if start == 0 && end == 0 => None,
            1 => Some(ByteRange::new(start, end).map_err(|_| JobError::Corrupt)?),
            _ => return Err(JobError::Corrupt),
        };
        let raw_flags = c.n()?;
        let allocation_consistent = match c.n()? {
            0 => false,
            1 => true,
            _ => return Err(JobError::Corrupt),
        };
        runs.push(ExtentRun {
            logical,
            image,
            raw_flags,
            allocation_consistent,
        });
    }
    c.end()?;
    let v = LogicalFileView {
        owner,
        size,
        metadata_flags,
        name_flags,
        attribute_flags,
        initialized_size,
        skip_length,
        runs,
        evidence,
    };
    if encode_view(&v, status)? != b {
        return Err(JobError::Corrupt);
    }
    Ok((v, status))
}
