//! Small, checksummed frontier snapshots. Payload digests are references only:
//! this store never reads, copies, or deletes media/index payloads.
//!
//! Requires a single writer and a case-owned directory. Filesystem flush calls
//! are exercised by fault tests; those tests do not certify physical power loss.
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

const OWNER: &[u8] = b"FrameHarbor frontier store v1\n";
const SNAPSHOT: &[u8] = b"FHCP1\n";
const HISTORY: &[u8] = b"FHCH1\n";

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[derive(Clone, Copy, Debug)]
pub struct CheckpointLimits {
    pub frontier_bytes: usize,
    pub payload_references: usize,
    pub historical_manifests: usize,
}
impl Default for CheckpointLimits {
    fn default() -> Self {
        Self {
            frontier_bytes: 64 * 1024,
            payload_references: 1024,
            historical_manifests: 2,
        }
    }
}
impl CheckpointLimits {
    fn validate(self) -> io::Result<Self> {
        if self.frontier_bytes > 16 * 1024 * 1024
            || self.payload_references > 65536
            || self.historical_manifests > 64
        {
            return Err(invalid("checkpoint limits exceed implementation envelope"));
        }
        Ok(self)
    }
    fn maximum_bytes(self) -> usize {
        SNAPSHOT.len() + 8 + 4 + self.frontier_bytes + self.payload_references * 32 + 32
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frontier {
    pub id: [u8; 32],
    pub bytes: Vec<u8>,
    pub payloads: Vec<[u8; 32]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckpointBoundary {
    ManifestSynced,
    HistorySynced,
    HistoryRenamed,
    DirectorySynced,
}

pub struct CheckpointStore {
    root: PathBuf,
    limits: CheckpointLimits,
}

fn checksum(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}
fn seal(mut bytes: Vec<u8>) -> Vec<u8> {
    let hash = checksum(&bytes);
    bytes.extend(hash);
    bytes
}
fn checked_body(bytes: &[u8]) -> io::Result<&[u8]> {
    let n = bytes
        .len()
        .checked_sub(32)
        .ok_or_else(|| invalid("truncated checksum"))?;
    if checksum(&bytes[..n]).as_slice() != &bytes[n..] {
        return Err(invalid("checkpoint checksum mismatch"));
    }
    Ok(&bytes[..n])
}
fn hex(id: &[u8; 32]) -> String {
    id.iter().map(|b| format!("{b:02x}")).collect()
}
fn read_small(path: &Path, maximum: usize) -> io::Result<Vec<u8>> {
    if !fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(invalid("checkpoint path is not a regular owned file"));
    }
    let mut bytes = Vec::new();
    File::open(path)?
        .take(maximum as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > maximum {
        return Err(invalid("checkpoint file exceeds limit"));
    }
    Ok(bytes)
}

impl CheckpointStore {
    /// Case-owned immutable source blocks live outside frontier retention.
    pub(crate) fn source_archive_root(&self) -> io::Result<PathBuf> {
        Ok(fs::canonicalize(&self.root)?.join("source-dependencies"))
    }
    /// Conservative small-buffer reservation; referenced payloads are not read.
    pub fn working_memory_bound(&self) -> u64 {
        (self.limits.maximum_bytes() as u64) * 4 + 4096
    }
    /// Creates a fresh subdirectory; never adopts or overwrites an existing path.
    pub fn create(root: &Path, limits: CheckpointLimits) -> io::Result<Self> {
        let limits = limits.validate()?;
        fs::create_dir(root)?;
        let mut marker = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(root.join("OWNER"))?;
        marker.write_all(OWNER)?;
        marker.sync_all()?;
        File::open(root)?.sync_all()?;
        if let Some(parent) = root.parent() {
            File::open(parent)?.sync_all()?;
        }
        Ok(Self {
            root: root.into(),
            limits,
        })
    }
    pub fn open(root: &Path, limits: CheckpointLimits) -> io::Result<Self> {
        if !fs::symlink_metadata(root)?.file_type().is_dir() {
            return Err(invalid("checkpoint root is not an owned directory"));
        }
        let store = Self {
            root: root.into(),
            limits: limits.validate()?,
        };
        store.check_owner()?;
        // CURRENT alone is authoritative. NEXT is an interrupted staging file,
        // never an accepted frontier merely because its checksum is valid.
        for id in store.history()? {
            store.load(id)?;
        }
        store.retained_checkpoint()?;
        Ok(store)
    }
    fn check_owner(&self) -> io::Result<()> {
        if read_small(&self.root.join("OWNER"), OWNER.len())? != OWNER {
            return Err(invalid("checkpoint ownership marker mismatch"));
        }
        Ok(())
    }
    fn manifest_path(&self, id: [u8; 32]) -> PathBuf {
        self.root.join(format!("manifest-{}", hex(&id)))
    }
    pub fn history(&self) -> io::Result<Vec<[u8; 32]>> {
        let bytes = match read_small(&self.root.join("CURRENT"), HISTORY.len() + 4 + 65 * 32 + 32) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        self.decode_history(&bytes)
    }
    fn decode_history(&self, bytes: &[u8]) -> io::Result<Vec<[u8; 32]>> {
        let body = checked_body(bytes)?;
        if !body.starts_with(HISTORY) || body.len() < HISTORY.len() + 4 {
            return Err(invalid("invalid checkpoint history"));
        }
        let count =
            u32::from_le_bytes(body[HISTORY.len()..HISTORY.len() + 4].try_into().unwrap()) as usize;
        if count == 0
            || count > self.limits.historical_manifests + 1
            || body.len() != HISTORY.len() + 4 + count * 32
        {
            return Err(invalid("checkpoint history count exceeds limit"));
        }
        Ok(body[HISTORY.len() + 4..].as_chunks::<32>().0.to_vec())
    }
    pub fn load_current(&self) -> io::Result<Option<Frontier>> {
        self.history()?.first().map(|id| self.load(*id)).transpose()
    }
    /// One pending Ledger link may retain a frontier outside normal history.
    pub(crate) fn retained_checkpoint(&self) -> io::Result<Option<[u8; 32]>> {
        let bytes = match read_small(&self.root.join("RETAINED"), self.limits.maximum_bytes()) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let id = checksum(&bytes);
        self.load(id)?;
        Ok(Some(id))
    }
    pub(crate) fn retain_linked(&mut self, id: [u8; 32]) -> io::Result<()> {
        self.check_owner()?;
        self.load(id)?;
        if let Some(retained) = self.retained_checkpoint()? {
            if retained == id {
                return Ok(());
            }
            fs::remove_file(self.root.join("RETAINED"))?;
        }
        fs::hard_link(self.manifest_path(id), self.root.join("RETAINED"))?;
        File::open(&self.root)?.sync_all()
    }
    pub(crate) fn release_retained(&mut self) -> io::Result<()> {
        if self.retained_checkpoint()?.is_some() {
            fs::remove_file(self.root.join("RETAINED"))?;
            File::open(&self.root)?.sync_all()?;
            self.reconcile()?;
        }
        Ok(())
    }
    /// Retires only verified case-owned frontier manifests outside committed
    /// history and complete interrupted history staging. Payloads are untouched.
    /// Interrupted staging is preserved as failure evidence, never adopted.
    /// Corruption in committed history blocks reuse.
    pub fn reconcile(&mut self) -> io::Result<()> {
        self.check_owner()?;
        let history = self.history()?;
        let retained = self.retained_checkpoint()?;
        for id in &history {
            self.load(*id)?;
        }
        let mut changed = false;
        let next = self.root.join("NEXT");
        if next.try_exists()? {
            let bytes = read_small(&next, HISTORY.len() + 4 + 65 * 32 + 32)?;
            if self.decode_history(&bytes).is_ok() {
                fs::remove_file(&next)?;
                changed = true;
            } else {
                self.preserve_interrupted(&next, &bytes)?;
            }
        }
        let staged = self.root.join("MANIFEST-NEXT");
        if staged.try_exists()? {
            let bytes = read_small(&staged, self.limits.maximum_bytes())?;
            self.preserve_interrupted(&staged, &bytes)?;
        }
        // Work is bounded by the normal retention plus one interrupted write.
        // Extra unknown files are neither traversed nor deleted.
        let mut visited = 0;
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            if entry.file_name() == ".DS_Store" {
                continue;
            }
            visited += 1;
            if visited > 132 {
                return Err(invalid("checkpoint directory exceeds entry limit"));
            }
            let name = entry.file_name();
            let Some(name) = name
                .to_str()
                .and_then(|name| name.strip_prefix("manifest-"))
            else {
                continue;
            };
            if name.len() != 64
                || !name
                    .bytes()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
            {
                return Err(invalid("invalid owned manifest name"));
            }
            let mut id = [0u8; 32];
            for (i, byte) in id.iter_mut().enumerate() {
                *byte = u8::from_str_radix(&name[i * 2..i * 2 + 2], 16)
                    .map_err(|_| invalid("invalid manifest identity"))?;
            }
            if !history.contains(&id) && retained != Some(id) {
                self.load(id)?;
                fs::remove_file(entry.path())?;
                changed = true;
            }
        }
        if changed {
            File::open(&self.root)?.sync_all()?;
        }
        Ok(())
    }
    fn preserve_interrupted(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        let failure = self
            .root
            .join(format!("failed-staging-{}", hex(&checksum(bytes))));
        if failure.try_exists()? {
            if read_small(&failure, self.limits.maximum_bytes())? != bytes {
                return Err(invalid("interrupted evidence identity mismatch"));
            }
            fs::remove_file(path)?;
        } else {
            File::open(path)?.sync_all()?;
            fs::rename(path, failure)?;
        }
        File::open(&self.root)?.sync_all()
    }
    pub fn load(&self, id: [u8; 32]) -> io::Result<Frontier> {
        let bytes = read_small(&self.manifest_path(id), self.limits.maximum_bytes())?;
        if checksum(&bytes) != id {
            return Err(invalid("checkpoint identity mismatch"));
        }
        let body = checked_body(&bytes)?;
        if !body.starts_with(SNAPSHOT) || body.len() < SNAPSHOT.len() + 12 {
            return Err(invalid("invalid frontier header"));
        }
        let offset = SNAPSHOT.len();
        let length = usize::try_from(u64::from_le_bytes(
            body[offset..offset + 8].try_into().unwrap(),
        ))
        .map_err(|_| invalid("frontier length overflow"))?;
        let count = u32::from_le_bytes(body[offset + 8..offset + 12].try_into().unwrap()) as usize;
        if length > self.limits.frontier_bytes || count > self.limits.payload_references {
            return Err(invalid("frontier content exceeds limit"));
        }
        let start = offset + 12;
        if body.len() != start + length + count * 32 {
            return Err(invalid("frontier content length mismatch"));
        }
        Ok(Frontier {
            id,
            bytes: body[start..start + length].to_vec(),
            payloads: body[start + length..].as_chunks::<32>().0.to_vec(),
        })
    }
    pub fn publish(&mut self, frontier: &[u8], payloads: &[[u8; 32]]) -> io::Result<[u8; 32]> {
        self.publish_observed(frontier, payloads, |_| Ok(()))
    }
    /// Fault hook supports reproducible boundary tests. An error preserves either
    /// the old or new authoritative history; callers reopen before continuing.
    pub fn publish_observed(
        &mut self,
        frontier: &[u8],
        payloads: &[[u8; 32]],
        boundary: impl FnMut(CheckpointBoundary) -> io::Result<()>,
    ) -> io::Result<[u8; 32]> {
        self.publish_retaining(frontier, payloads, None, boundary)
    }
    pub(crate) fn publish_linked(
        &mut self,
        frontier: &[u8],
        payloads: &[[u8; 32]],
        linked: Option<[u8; 32]>,
    ) -> io::Result<[u8; 32]> {
        self.publish_retaining(frontier, payloads, linked, |_| Ok(()))
    }
    fn publish_retaining(
        &mut self,
        frontier: &[u8],
        payloads: &[[u8; 32]],
        linked: Option<[u8; 32]>,
        mut boundary: impl FnMut(CheckpointBoundary) -> io::Result<()>,
    ) -> io::Result<[u8; 32]> {
        self.check_owner()?;
        if frontier.len() > self.limits.frontier_bytes
            || payloads.len() > self.limits.payload_references
        {
            return Err(invalid("frontier exceeds limit"));
        }
        self.reconcile()?;
        let previous = self.history()?;
        let mut bytes = SNAPSHOT.to_vec();
        bytes.extend((frontier.len() as u64).to_le_bytes());
        bytes.extend((payloads.len() as u32).to_le_bytes());
        bytes.extend(frontier);
        for id in payloads {
            bytes.extend(id);
        }
        let bytes = seal(bytes);
        let id = checksum(&bytes);
        let path = self.manifest_path(id);
        if path.try_exists()? {
            self.load(id)?;
            if previous.first() == Some(&id) {
                return Ok(id);
            }
        }
        if let Some(linked) = linked {
            self.retain_linked(linked)?;
        }
        if !path.try_exists()? {
            let staged = self.root.join("MANIFEST-NEXT");
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&staged)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            fs::rename(staged, path)?;
        }
        File::open(&self.root)?.sync_all()?;
        boundary(CheckpointBoundary::ManifestSynced)?;
        let mut history = vec![id];
        history.extend(
            previous
                .iter()
                .copied()
                .filter(|old| *old != id)
                .take(self.limits.historical_manifests),
        );
        let mut index = HISTORY.to_vec();
        index.extend((history.len() as u32).to_le_bytes());
        for id in &history {
            index.extend(id);
        }
        let index = seal(index);
        let next = self.root.join("NEXT");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&next)?;
        file.write_all(&index)?;
        file.sync_all()?;
        boundary(CheckpointBoundary::HistorySynced)?;
        fs::rename(&next, self.root.join("CURRENT"))?;
        boundary(CheckpointBoundary::HistoryRenamed)?;
        File::open(&self.root)?.sync_all()?;
        boundary(CheckpointBoundary::DirectorySynced)?;
        let retained = self.retained_checkpoint()?;
        for old in previous {
            if !history.contains(&old) && retained != Some(old) {
                fs::remove_file(self.manifest_path(old))?;
            }
        }
        File::open(&self.root)?.sync_all()?;
        Ok(id)
    }
}
