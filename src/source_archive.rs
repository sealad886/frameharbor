//! Immutable, bounded source dependency blocks referenced by case frontiers.
//! Payload bytes are never copied. A block is durable before its frontier link.
use crate::{
    acquisition::{SourceManifest, identity},
    contracts::{BindingEvidence, ByteRange},
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
const MAGIC: &[u8] = b"FrameHarbor source dependency block v1\n";
const MAX_DEPENDENCIES: usize = 1024;
const MAX_BLOCK: usize = 64 * 1024;
const MAX_ENTRIES: usize = 2048;
const MAX_BYTES: u64 = 64 * 1024 * 1024;
static NEXT: AtomicU64 = AtomicU64::new(0);
fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn hex(hash: &[u8; 32]) -> String {
    hash.iter().map(|b| format!("{b:02x}")).collect()
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Header {
    bytes: Vec<u8>,
    domain: ByteRange,
}
impl Header {
    pub(crate) fn from_manifest(m: &SourceManifest) -> io::Result<Self> {
        let mut bytes = MAGIC.to_vec();
        bytes.extend(m.generation_id());
        bytes.extend(m.map_digest());
        for n in [
            m.image_size(),
            m.binding().input_domain().start(),
            m.binding().input_domain().end(),
            m.binding().image_domain().start(),
            m.binding().image_domain().end(),
        ] {
            bytes.extend(n.to_be_bytes());
        }
        bytes.push(match m.binding().evidence() {
            BindingEvidence::OperatorAttested => 1,
            BindingEvidence::TrustedManifest => 2,
            BindingEvidence::Unverified => return Err(invalid("unverified archive binding")),
        });
        Ok(Self {
            bytes,
            domain: m.binding().image_domain(),
        })
    }
}
pub(crate) struct SourceArchive {
    root: PathBuf,
    header: Header,
}
fn read_bounded(path: &Path, cap: usize) -> io::Result<Vec<u8>> {
    if !fs::symlink_metadata(path)?.is_file() {
        return Err(invalid("archive path is not a regular file"));
    }
    let mut file = File::open(path)?;
    let before = identity(&file.metadata()?);
    let mut bytes = Vec::new();
    (&mut file).take(cap as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > cap
        || before != identity(&file.metadata()?)
        || before != identity(&fs::metadata(path)?)
    {
        return Err(invalid("archive file changed or exceeded limit"));
    }
    Ok(bytes)
}
impl SourceArchive {
    pub(crate) fn open(root: &Path, header: Header) -> io::Result<Self> {
        if !fs::symlink_metadata(root)?.is_dir() {
            return Err(invalid("archive root is not an owned directory"));
        }
        let archive = Self {
            root: root.into(),
            header,
        };
        archive.check_owner()?;
        Ok(archive)
    }
    pub(crate) fn open_or_create(root: &Path, header: Header) -> io::Result<Self> {
        match fs::symlink_metadata(root) {
            Ok(_) => return Self::open(root, header),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let parent = root
            .parent()
            .ok_or_else(|| invalid("archive parent missing"))?;
        let staged = parent.join("source-dependencies-NEXT");
        let staged_exists = match fs::symlink_metadata(&staged) {
            Ok(_) => true,
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => return Err(error),
        };
        if staged_exists {
            if !fs::symlink_metadata(&staged)?.is_dir() {
                return Err(invalid("invalid interrupted archive initialization"));
            }
            let mut destination = None;
            for i in 0..8 {
                let candidate = parent.join(format!("source-dependencies-failed-{i}"));
                match fs::symlink_metadata(&candidate) {
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        destination = Some(candidate);
                        break;
                    }
                    Ok(_) => {}
                    Err(error) => return Err(error),
                }
            }
            let failed =
                destination.ok_or_else(|| invalid("archive initialization failure quota"))?;
            fs::rename(&staged, failed)?;
            File::open(parent)?.sync_all()?;
        }
        fs::create_dir(&staged)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(staged.join("OWNER"))?;
        file.write_all(&header.bytes)?;
        file.sync_all()?;
        File::open(&staged)?.sync_all()?;
        fs::rename(&staged, root)?;
        File::open(parent)?.sync_all()?;
        Self::open(root, header)
    }
    fn check_owner(&self) -> io::Result<()> {
        if !fs::symlink_metadata(&self.root)?.is_dir()
            || read_bounded(&self.root.join("OWNER"), 256)? != self.header.bytes
        {
            return Err(invalid("source archive ownership mismatch"));
        }
        Ok(())
    }
    pub(crate) fn write_bound(dependencies: usize) -> io::Result<u64> {
        if dependencies == 0 || dependencies > MAX_DEPENDENCIES {
            return Err(invalid("source archive dependency count"));
        }
        // Header fits the bounded OWNER envelope; reserve both staged content
        // and durable namespace bookkeeping without charging a full-sized block
        // for a small metadata batch.
        Ok((256 + 4 + dependencies as u64 * 48) * 2 + 8192)
    }
    fn encode(&self, deps: &[(ByteRange, [u8; 32])]) -> io::Result<Vec<u8>> {
        Self::write_bound(deps.len())?;
        let mut bytes = self.header.bytes.clone();
        bytes.extend((deps.len() as u32).to_be_bytes());
        let mut seen = BTreeSet::new();
        for (range, hash) in deps {
            if !self.header.domain.contains(*range) || !seen.insert(*range) {
                return Err(invalid("duplicate archived source geometry"));
            }
            bytes.extend(range.start().to_be_bytes());
            bytes.extend(range.end().to_be_bytes());
            bytes.extend(hash);
        }
        if bytes.len() > MAX_BLOCK {
            return Err(invalid("source block exceeds bound"));
        }
        Ok(bytes)
    }
    fn inventory(&self) -> io::Result<u64> {
        let mut entries = 0usize;
        let mut bytes = 0u64;
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let name = entry.file_name();
            if name == "OWNER" || name == ".DS_Store" {
                continue;
            }
            entries += 1;
            if entries > MAX_ENTRIES - 2 {
                return Err(invalid("source archive entry quota"));
            }
            let name = name
                .to_str()
                .ok_or_else(|| invalid("invalid source archive filename"))?;
            let valid_block = name.strip_suffix(".dep").is_some_and(|s| {
                s.len() == 64
                    && s.bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            });
            if !valid_block && !name.starts_with(".next-") {
                return Err(invalid("unknown source archive entry"));
            }
            let m = fs::symlink_metadata(entry.path())?;
            if !m.is_file() || m.len() > MAX_BLOCK as u64 {
                return Err(invalid("invalid source archive entry"));
            }
            bytes = bytes
                .checked_add(m.len())
                .ok_or_else(|| invalid("source archive quota overflow"))?;
            if bytes > MAX_BYTES {
                return Err(invalid("source archive byte quota"));
            }
        }
        Ok(bytes)
    }
    pub(crate) fn put(&self, deps: &[(ByteRange, [u8; 32])]) -> io::Result<[u8; 32]> {
        self.check_owner()?;
        let bytes = self.encode(deps)?;
        let id: [u8; 32] = Sha256::digest(&bytes).into();
        let final_path = self.root.join(format!("{}.dep", hex(&id)));
        if final_path.try_exists()? {
            self.load(id)?;
            return Ok(id);
        }
        let used = self.inventory()?;
        if used
            .checked_add(bytes.len() as u64)
            .is_none_or(|n| n > MAX_BYTES)
        {
            return Err(invalid("source archive byte quota"));
        }
        let staged = self.root.join(format!(
            ".next-{}-{}-{}",
            hex(&id),
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        match fs::hard_link(&staged, &final_path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                self.load(id)?;
            }
            Err(e) => return Err(e),
        }
        File::open(&self.root)?.sync_all()?;
        fs::remove_file(&staged)?;
        File::open(&self.root)?.sync_all()?;
        Ok(id)
    }
    pub(crate) fn load(&self, id: [u8; 32]) -> io::Result<Vec<(ByteRange, [u8; 32])>> {
        self.check_owner()?;
        let bytes = read_bounded(&self.root.join(format!("{}.dep", hex(&id))), MAX_BLOCK)?;
        if <[u8; 32]>::from(Sha256::digest(&bytes)) != id || !bytes.starts_with(&self.header.bytes)
        {
            return Err(invalid("source block hash or binding mismatch"));
        }
        let mut at = self.header.bytes.len();
        let count = u32::from_be_bytes(
            bytes
                .get(at..at + 4)
                .ok_or_else(|| invalid("truncated source block"))?
                .try_into()
                .unwrap(),
        ) as usize;
        at += 4;
        if count == 0 || count > MAX_DEPENDENCIES || bytes.len() != at + count * 48 {
            return Err(invalid("source block length/count mismatch"));
        }
        let mut deps = Vec::with_capacity(count);
        let mut seen = BTreeSet::new();
        for entry in bytes[at..].as_chunks::<48>().0 {
            let range = ByteRange::new(
                u64::from_be_bytes(entry[..8].try_into().unwrap()),
                u64::from_be_bytes(entry[8..16].try_into().unwrap()),
            )
            .map_err(|_| invalid("invalid archived source range"))?;
            if !self.header.domain.contains(range) || !seen.insert(range) {
                return Err(invalid("duplicate archived source geometry"));
            }
            deps.push((range, entry[16..].try_into().unwrap()));
        }
        Ok(deps)
    }
}
