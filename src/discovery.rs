//! Durable directory cursors and global extent ownership in the case Ledger.
//! A batch is never adopted before its source observations reach a sealed frontier.
use crate::{
    contracts::{ResourceCaps, SourceGeneration},
    filesystem::{DirectoryBatch, DirectoryCursor, ExtentEvidence, FilesystemProfile},
    jobs::{self, IndexedView, ViewStatus},
    ledger::{Ledger, LedgerError, WorkSpec},
    session::SealedDiscoveryBatch,
};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use sha2::{Digest, Sha256};
use std::{
    cell::Cell,
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
#[derive(Debug)]
pub enum DiscoveryError {
    Io(io::Error),
    Ledger(LedgerError),
    Sql(rusqlite::Error),
    Invalid,
    Conflict,
    Incomplete,
    ResourceLimit,
    Corrupt,
}
impl fmt::Display for DiscoveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for DiscoveryError {}
impl From<io::Error> for DiscoveryError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<LedgerError> for DiscoveryError {
    fn from(e: LedgerError) -> Self {
        Self::Ledger(e)
    }
}
impl From<rusqlite::Error> for DiscoveryError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sql(e)
    }
}
pub type Result<T> = std::result::Result<T, DiscoveryError>;
#[derive(Debug, Clone, Copy)]
pub struct DiscoveryLimits {
    pub entries_per_batch: usize,
    pub runs_per_batch: usize,
    pub posting_cap: usize,
    pub page_size: usize,
    pub index_bytes: u64,
    pub geometry_visits: u64,
}
impl Default for DiscoveryLimits {
    fn default() -> Self {
        Self {
            entries_per_batch: 256,
            runs_per_batch: 1024,
            posting_cap: 256,
            page_size: 128,
            index_bytes: 1024 * 1024 * 1024,
            geometry_visits: 10_000_000,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryStatus {
    pub geometry_visits: u64,
    pub geometry_conflicts: u64,
    pub namespace_policy: &'static str,
    pub excluded_virtual_files: u64,
    pub excluded_virtual_directories: u64,
    pub pending_directories: u64,
    pub deferred_entries: u64,
    pub directories: u64,
    pub views: u64,
    pub batches: u64,
    pub index_bytes: u64,
    pub complete: bool,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchAdoption {
    pub id: String,
    pub adopted: bool,
    pub files: u64,
    pub postings: u64,
    pub conflicts: u64,
}
pub struct DiscoveryQueue {
    root: PathBuf,
    domain: String,
    work: String,
    source: SourceGeneration,
    profile: FilesystemProfile,
    fs_start: u64,
    limits: DiscoveryLimits,
    verified: Cell<bool>,
    verification_epoch: String,
}
static VERIFICATION_EPOCH: AtomicU64 = AtomicU64::new(0);
fn fresh_epoch() -> String {
    hash(format!(
        "{:?}:{}:{}",
        std::time::SystemTime::now(),
        std::process::id(),
        VERIFICATION_EPOCH.fetch_add(1, Ordering::Relaxed)
    ))
}
fn stamp(meta: &fs::Metadata) -> String {
    hash(format!("{:?}", crate::acquisition::identity(meta)))
}
fn verify_payload(path: &Path, bytes: u64, digest: &str) -> Result<String> {
    let before = fs::symlink_metadata(path)?;
    if !before.is_file() || before.len() != bytes {
        return Err(DiscoveryError::Corrupt);
    }
    let identity = stamp(&before);
    let mut file = File::open(path)?;
    if stamp(&file.metadata()?) != identity {
        return Err(DiscoveryError::Corrupt);
    }
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    let mut remaining = bytes;
    while remaining > 0 {
        let n = remaining.min(buffer.len() as u64) as usize;
        file.read_exact(&mut buffer[..n])?;
        hash.update(&buffer[..n]);
        remaining -= n as u64;
    }
    if format!("{:x}", hash.finalize()) != digest
        || stamp(&file.metadata()?) != identity
        || stamp(&fs::symlink_metadata(path)?) != identity
    {
        return Err(DiscoveryError::Corrupt);
    }
    Ok(identity)
}
fn hash(b: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(b.as_ref()))
}
fn number(n: u64) -> Result<i64> {
    i64::try_from(n).map_err(|_| DiscoveryError::ResourceLimit)
}
fn profile(p: FilesystemProfile) -> &'static str {
    match p {
        FilesystemProfile::Fat32 => "fat32",
        FilesystemProfile::ExFat => "exfat",
    }
}
fn inode(n: u64) -> [u8; 8] {
    n.to_be_bytes()
}
fn decode_inode(n: &[u8]) -> Result<u64> {
    Ok(u64::from_be_bytes(
        n.try_into().map_err(|_| DiscoveryError::Corrupt)?,
    ))
}
fn require_source(ledger: &Ledger, work: &str, source: &SourceGeneration) -> Result<()> {
    if ledger.work_source(work)? != *source || !ledger.get(work)?.source_valid {
        return Err(DiscoveryError::Conflict);
    }
    Ok(())
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewQualification {
    pub unchecked_runs: Vec<u64>,
    pub observed_allocation_negative: bool,
    pub shared_geometry: bool,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaProgress {
    pub views: u64,
    pub pending: u64,
    pub completed: u64,
    pub unresolved: u64,
    pub examples: Vec<String>,
}
fn domain_key(
    work_id: &str,
    profile_value: FilesystemProfile,
    fs_start: u64,
    limits: DiscoveryLimits,
) -> String {
    hash(format!(
        "FrameHarbor discovery v1:allocated-reachable-native-names-v1:{work_id}:{}:{fs_start}:{}:{}:{}:{}:{}:{}",
        profile(profile_value),
        limits.entries_per_batch,
        limits.runs_per_batch,
        limits.posting_cap,
        limits.page_size,
        limits.index_bytes,
        limits.geometry_visits
    ))
}
impl DiscoveryQueue {
    pub fn open(
        case_dir: &Path,
        ledger: &mut Ledger,
        work: &WorkSpec,
        profile_value: FilesystemProfile,
        fs_start: u64,
        limits: DiscoveryLimits,
    ) -> Result<Self> {
        if limits.entries_per_batch == 0
            || limits.entries_per_batch > 1024
            || limits.runs_per_batch == 0
            || limits.runs_per_batch > 1024
            || limits.posting_cap == 0
            || limits.posting_cap > 4096
            || limits.page_size == 0
            || limits.page_size > 1024
            || limits.index_bytes == 0
            || limits.index_bytes > 16 * 1024 * 1024 * 1024
            || limits.geometry_visits == 0
            || limits.geometry_visits > 1_000_000_000
        {
            return Err(DiscoveryError::Invalid);
        }
        if fs::canonicalize(ledger.connection.path().ok_or(DiscoveryError::Invalid)?)?.parent()
            != Some(case_dir)
        {
            return Err(DiscoveryError::Invalid);
        }
        if fs::canonicalize(case_dir)? != case_dir || !fs::symlink_metadata(case_dir)?.is_dir() {
            return Err(DiscoveryError::Invalid);
        }
        let work_id = ledger.submit(work)?;
        let domain = domain_key(&work_id, profile_value, fs_start, limits);
        let root = case_dir.join("discovery-indexes");
        match fs::create_dir(&root) {
            Ok(()) => File::open(case_dir)?.sync_all()?,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
        if fs::canonicalize(&root)? != root || !fs::symlink_metadata(&root)?.is_dir() {
            return Err(DiscoveryError::Invalid);
        }
        ledger.connection.execute_batch("CREATE TABLE IF NOT EXISTS discovery_domains(id TEXT PRIMARY KEY,work TEXT NOT NULL REFERENCES work(id),geometry BLOB,state TEXT NOT NULL DEFAULT 'active',index_bytes INTEGER NOT NULL DEFAULT 0,bucket INTEGER,excluded_files INTEGER NOT NULL DEFAULT 0,excluded_dirs INTEGER NOT NULL DEFAULT 0,geometry_visits INTEGER NOT NULL DEFAULT 0,geometry_conflicts INTEGER NOT NULL DEFAULT 0);
            CREATE TABLE IF NOT EXISTS discovery_storage(singleton INTEGER PRIMARY KEY CHECK(singleton=1),used INTEGER NOT NULL,quota INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS discovery_dirs(domain TEXT NOT NULL REFERENCES discovery_domains(id),inode BLOB NOT NULL,sequence INTEGER PRIMARY KEY AUTOINCREMENT,next_entry INTEGER NOT NULL DEFAULT 0,total INTEGER,state TEXT NOT NULL DEFAULT 'pending',UNIQUE(domain,inode));
            CREATE INDEX IF NOT EXISTS discovery_dir_pending ON discovery_dirs(domain,state,sequence);
            CREATE TABLE IF NOT EXISTS discovery_payloads(batch TEXT PRIMARY KEY,domain TEXT NOT NULL REFERENCES discovery_domains(id),digest TEXT NOT NULL,bytes INTEGER NOT NULL,committed INTEGER NOT NULL DEFAULT 0,verified_identity TEXT,verified_epoch TEXT);
            CREATE TABLE IF NOT EXISTS discovery_batches(id TEXT PRIMARY KEY,domain TEXT NOT NULL REFERENCES discovery_domains(id),directory BLOB NOT NULL,start INTEGER NOT NULL,next INTEGER NOT NULL,digest TEXT NOT NULL,postings INTEGER NOT NULL,conflicts INTEGER NOT NULL,UNIQUE(domain,directory,start));
            CREATE TABLE IF NOT EXISTS discovery_views(domain TEXT NOT NULL,owner TEXT NOT NULL,id TEXT NOT NULL,batch TEXT NOT NULL,offset INTEGER NOT NULL,bytes INTEGER NOT NULL,digest TEXT NOT NULL,base INTEGER NOT NULL,shared INTEGER NOT NULL DEFAULT 0,unchecked BLOB NOT NULL,allocation_negative INTEGER NOT NULL,media_work TEXT REFERENCES work(id),PRIMARY KEY(domain,owner));
            CREATE INDEX IF NOT EXISTS discovery_views_page ON discovery_views(domain,id);
            CREATE INDEX IF NOT EXISTS discovery_outcome_work ON receipts(work,kind);
            CREATE TABLE IF NOT EXISTS discovery_extents(id INTEGER PRIMARY KEY,domain TEXT NOT NULL,owner TEXT NOT NULL,logical INTEGER NOT NULL,start INTEGER NOT NULL,end INTEGER NOT NULL,UNIQUE(domain,owner,logical,start,end));
            CREATE VIRTUAL TABLE IF NOT EXISTS discovery_geometry USING rtree_i32(id,domain_lo,domain_hi,lo,hi);
            CREATE TABLE IF NOT EXISTS discovery_aliases(domain TEXT NOT NULL,parent BLOB NOT NULL,entry INTEGER NOT NULL,child BLOB NOT NULL,PRIMARY KEY(domain,parent,entry));
            CREATE TABLE IF NOT EXISTS discovery_deferred(domain TEXT NOT NULL,parent BLOB NOT NULL,entry INTEGER NOT NULL,reason TEXT NOT NULL,PRIMARY KEY(domain,parent,entry));")?;
        ledger.connection.execute(
            "INSERT OR IGNORE INTO discovery_storage(singleton,used,quota)VALUES(1,0,?1)",
            [number(limits.index_bytes)?],
        )?;
        let stored_quota: i64 = ledger.connection.query_row(
            "SELECT quota FROM discovery_storage WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        if stored_quota != number(limits.index_bytes)? {
            return Err(DiscoveryError::Conflict);
        }
        ledger.connection.execute(
            "INSERT OR IGNORE INTO discovery_domains(id,work)VALUES(?1,?2)",
            params![domain, work_id],
        )?;
        Ok(Self {
            root,
            domain,
            work: work_id,
            source: work.source.clone(),
            profile: profile_value,
            fs_start,
            limits,
            verified: Cell::new(false),
            verification_epoch: fresh_epoch(),
        })
    }
    /// Observe already registered discovery state without creating files/tables.
    pub fn open_readonly(
        case_dir: &Path,
        ledger: &Ledger,
        work: &WorkSpec,
        profile_value: FilesystemProfile,
        fs_start: u64,
        limits: DiscoveryLimits,
    ) -> Result<Option<Self>> {
        if fs::canonicalize(case_dir)? != case_dir
            || fs::canonicalize(ledger.connection.path().ok_or(DiscoveryError::Invalid)?)?.parent()
                != Some(case_dir)
        {
            return Err(DiscoveryError::Invalid);
        }
        let work_id = work.id()?;
        let domain = domain_key(&work_id, profile_value, fs_start, limits);
        let exists:bool=ledger.connection.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='discovery_domains')",[],|r|r.get(0))?;
        if !exists {
            return Ok(None);
        }
        let registered: bool = ledger.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM discovery_domains WHERE id=?1 AND work=?2)",
            params![domain, work_id],
            |r| r.get(0),
        )?;
        if !registered {
            return Ok(None);
        }
        let root = case_dir.join("discovery-indexes");
        if fs::canonicalize(&root)? != root || !fs::symlink_metadata(&root)?.is_dir() {
            return Err(DiscoveryError::Corrupt);
        }
        if ledger.work_source(&work_id)? != work.source {
            return Err(DiscoveryError::Conflict);
        }
        Ok(Some(Self {
            root,
            domain,
            work: work_id,
            source: work.source.clone(),
            profile: profile_value,
            fs_start,
            limits,
            verified: Cell::new(false),
            verification_epoch: fresh_epoch(),
        }))
    }
    pub fn page_read_bound(
        &self,
        ledger: &Ledger,
        after: Option<&str>,
        limit: usize,
    ) -> Result<ResourceCaps> {
        self.page_read_bound_filtered(ledger, after, limit, false)
    }
    pub fn pending_page_read_bound(
        &self,
        ledger: &Ledger,
        after: Option<&str>,
        limit: usize,
    ) -> Result<ResourceCaps> {
        self.page_read_bound_filtered(ledger, after, limit, true)
    }
    fn page_read_bound_filtered(
        &self,
        ledger: &Ledger,
        after: Option<&str>,
        limit: usize,
        pending: bool,
    ) -> Result<ResourceCaps> {
        if limit == 0 || limit > self.limits.page_size {
            return Err(DiscoveryError::Invalid);
        }
        require_source(ledger, &self.work, &self.source)?;
        let (rows,bytes):(i64,i64)=ledger.connection.query_row("SELECT count(*),coalesce(sum(bytes),0) FROM(SELECT bytes FROM discovery_views v WHERE domain=?1 AND id>?2 AND (?4=0 OR NOT EXISTS(SELECT 1 FROM receipts r WHERE r.work=v.media_work AND r.kind=?5)) ORDER BY id LIMIT ?3)",params![self.domain,after.unwrap_or(""),limit as i64,i64::from(pending),crate::ledger::MEDIA_OUTCOME],|r|Ok((r.get(0)?,r.get(1)?)))?;
        if rows < 0 || bytes < 0 || bytes as u64 > limit as u64 * 128 * 1024 {
            return Err(DiscoveryError::Corrupt);
        }
        Ok(ResourceCaps {
            read_bytes: bytes as u64 + rows as u64 * 1024 + 65536,
            memory_bytes: bytes as u64 * 4 + rows as u64 * 1024 + 65536,
            index_bytes: 0,
            output_bytes: 0,
            visited_postings: 0,
            transitions: 0,
            decoder_seconds: 0,
        })
    }
    pub fn bind_media_work(&self, ledger: &mut Ledger, view_id: &str, work_id: &str) -> Result<()> {
        require_source(ledger, &self.work, &self.source)?;
        require_source(ledger, work_id, &self.source)?;
        self.bind_media_link(&ledger.connection, view_id, work_id)
    }
    fn bind_media_link(
        &self,
        connection: &rusqlite::Connection,
        view_id: &str,
        work_id: &str,
    ) -> Result<()> {
        let prior: Option<Option<String>> = connection
            .query_row(
                "SELECT media_work FROM discovery_views WHERE domain=?1 AND id=?2",
                params![self.domain, view_id],
                |r| r.get(0),
            )
            .optional()?;
        match prior {
            None => Err(DiscoveryError::Invalid),
            Some(Some(prior)) if prior != work_id => Err(DiscoveryError::Conflict),
            Some(Some(_)) => Ok(()),
            Some(None) => {
                if connection.execute("UPDATE discovery_views SET media_work=?3 WHERE domain=?1 AND id=?2 AND media_work IS NULL",params![self.domain,view_id,work_id])?!=1 {return Err(DiscoveryError::Conflict)}
                Ok(())
            }
        }
    }
    /// One admitted page operation. Work registration and every view link commit
    /// together; any missing view or conflicting link rolls back the whole page.
    pub fn bind_media_work_batch(
        &self,
        ledger: &mut Ledger,
        rows: &[(&str, &WorkSpec)],
    ) -> Result<Vec<String>> {
        if rows.len() > 128 {
            return Err(DiscoveryError::ResourceLimit);
        }
        require_source(ledger, &self.work, &self.source)?;
        let mut text_bytes = 0usize;
        let mut seen = std::collections::BTreeSet::new();
        for (view, spec) in rows {
            if spec.source != self.source || !seen.insert(*view) {
                return Err(DiscoveryError::Conflict);
            }
            for part in [
                *view,
                spec.source.0.as_str(),
                spec.algorithm.as_str(),
                spec.stage.as_str(),
            ]
            .into_iter()
            .chain(
                spec.parameters
                    .iter()
                    .flat_map(|(k, v)| [k.as_str(), v.as_str()]),
            ) {
                text_bytes = text_bytes
                    .checked_add(part.len())
                    .filter(|n| *n <= 1024 * 1024)
                    .ok_or(DiscoveryError::ResourceLimit)?;
            }
        }
        let tx = ledger
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut ids = Vec::with_capacity(rows.len());
        for (view, spec) in rows {
            let id = Ledger::submit_in_connection(&tx, spec)?;
            self.bind_media_link(&tx, view, &id)?;
            ids.push(id);
        }
        tx.commit()?;
        Ok(ids)
    }
    pub fn qualification(&self, ledger: &Ledger, view_id: &str) -> Result<ViewQualification> {
        let (bytes,negative,shared):(Vec<u8>,bool,bool)=ledger.connection.query_row("SELECT unchecked,allocation_negative,shared FROM discovery_views WHERE domain=?1 AND id=?2 AND length(unchecked)<=8192",params![self.domain,view_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
        let (chunks, remainder) = bytes.as_chunks::<8>();
        if !remainder.is_empty() {
            return Err(DiscoveryError::Corrupt);
        }
        Ok(ViewQualification {
            unchecked_runs: chunks.iter().map(|b| u64::from_be_bytes(*b)).collect(),
            observed_allocation_negative: negative,
            shared_geometry: shared,
        })
    }
    /// Scalar namespace counts plus bounded examples, never all view payloads.
    pub fn media_progress(&self, ledger: &Ledger, example_limit: usize) -> Result<MediaProgress> {
        if example_limit > 128 {
            return Err(DiscoveryError::Invalid);
        }
        let (views,completed,unresolved):(i64,i64,i64)=ledger.connection.query_row("SELECT count(*),coalesce(sum(EXISTS(SELECT 1 FROM receipts r WHERE r.work=v.media_work AND r.kind=?2)),0),coalesce(sum(EXISTS(SELECT 1 FROM receipts r WHERE r.work=v.media_work AND r.kind=?2 AND substr(CAST(r.payload AS TEXT),1,9)!='exported ')),0) FROM discovery_views v WHERE v.domain=?1",params![self.domain,crate::ledger::MEDIA_OUTCOME],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
        if views < 0
            || completed < 0
            || completed > views
            || unresolved < 0
            || unresolved > completed
        {
            return Err(DiscoveryError::Corrupt);
        }
        let mut q=ledger.connection.prepare("SELECT substr(r.payload,1,4096) FROM discovery_views v JOIN receipts r ON r.work=v.media_work AND r.kind=?2 WHERE v.domain=?1 AND substr(CAST(r.payload AS TEXT),1,9)!='exported ' ORDER BY v.id LIMIT ?3")?;
        let examples = q
            .query_map(
                params![
                    self.domain,
                    crate::ledger::MEDIA_OUTCOME,
                    example_limit as i64
                ],
                |r| r.get::<_, Vec<u8>>(0),
            )?
            .map(|r| r.map(|b| String::from_utf8_lossy(&b).into_owned()))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(MediaProgress {
            views: views as u64,
            pending: (views - completed) as u64,
            completed: completed as u64,
            unresolved: unresolved as u64,
            examples,
        })
    }
    pub fn domain(&self) -> &str {
        &self.domain
    }
    pub fn status(&self, ledger: &Ledger) -> Result<DiscoveryStatus> {
        if ledger.work_source(&self.work)? != self.source {
            return Err(DiscoveryError::Conflict);
        }
        let (state, bytes): (String, i64) = ledger.connection.query_row(
            "SELECT state,index_bytes FROM discovery_domains WHERE id=?1",
            [&self.domain],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let count = |sql: &str| -> Result<u64> {
            let n: i64 = ledger
                .connection
                .query_row(sql, [&self.domain], |r| r.get(0))?;
            u64::try_from(n).map_err(|_| DiscoveryError::Corrupt)
        };
        let (excluded_files, excluded_dirs): (i64, i64) = ledger.connection.query_row(
            "SELECT excluded_files,excluded_dirs FROM discovery_domains WHERE id=?1",
            [&self.domain],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let (visits, conflicts): (i64, i64) = ledger.connection.query_row(
            "SELECT geometry_visits,geometry_conflicts FROM discovery_domains WHERE id=?1",
            [&self.domain],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        Ok(DiscoveryStatus {
            geometry_visits: u64::try_from(visits).map_err(|_| DiscoveryError::Corrupt)?,
            geometry_conflicts: u64::try_from(conflicts).map_err(|_| DiscoveryError::Corrupt)?,
            namespace_policy: "allocated-reachable-native-names-v1",
            excluded_virtual_files: u64::try_from(excluded_files)
                .map_err(|_| DiscoveryError::Corrupt)?,
            excluded_virtual_directories: u64::try_from(excluded_dirs)
                .map_err(|_| DiscoveryError::Corrupt)?,
            pending_directories: count(
                "SELECT count(*) FROM discovery_dirs WHERE domain=?1 AND state='pending'",
            )?,
            deferred_entries: count("SELECT count(*) FROM discovery_deferred WHERE domain=?1")?,
            directories: count("SELECT count(*) FROM discovery_dirs WHERE domain=?1")?,
            views: count("SELECT count(*) FROM discovery_views WHERE domain=?1")?,
            batches: count("SELECT count(*) FROM discovery_batches WHERE domain=?1")?,
            index_bytes: u64::try_from(bytes).map_err(|_| DiscoveryError::Corrupt)?,
            complete: state == "complete",
        })
    }
    pub fn next(&self, ledger: &Ledger) -> Result<Option<DirectoryCursor>> {
        require_source(ledger, &self.work, &self.source)?;
        let (state, geometry): (String, Option<Vec<u8>>) = ledger.connection.query_row(
            "SELECT state,geometry FROM discovery_domains WHERE id=?1",
            [&self.domain],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if state != "active" {
            return Ok(None);
        }
        if geometry.is_none() {
            return Ok(Some(DirectoryCursor {
                directory_inode: None,
                next_entry: 0,
            }));
        }
        let row:Option<(Vec<u8>,i64)>=ledger.connection.query_row("SELECT inode,next_entry FROM discovery_dirs WHERE domain=?1 AND state='pending' ORDER BY sequence LIMIT 1",[&self.domain],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        row.map(|(n, c)| {
            Ok(DirectoryCursor {
                directory_inode: Some(decode_inode(&n)?),
                next_entry: u64::try_from(c).map_err(|_| DiscoveryError::Corrupt)?,
            })
        })
        .transpose()
    }
    fn batch_id(&self, b: &DirectoryBatch) -> String {
        hash(format!(
            "FrameHarbor directory batch:{}:{}:{}",
            self.domain,
            b.directory_inode(),
            b.start_entry()
        ))
    }
    fn validate_batch(&self, b: &DirectoryBatch) -> Result<()> {
        let g = b.geometry();
        if b.namespace_policy() != "allocated-reachable-native-names-v1" {
            return Err(DiscoveryError::Invalid);
        }
        if g.profile != self.profile
            || g.start_in_image != self.fs_start
            || g.block_size == 0
            || !g.block_size.is_power_of_two()
            || b.next_entry() < b.start_entry()
            || b.next_entry() > b.total_entries()
            || b.next_entry() - b.start_entry() > self.limits.entries_per_batch as u64
            || b.directory_complete() != (b.next_entry() == b.total_entries())
            || b.files().len() + b.children().len() + b.deferred().len()
                > self.limits.entries_per_batch
        {
            return Err(DiscoveryError::Invalid);
        }
        if b.next_entry() == b.start_entry() && !b.directory_complete() {
            return Err(DiscoveryError::Invalid);
        }
        let mut entries = std::collections::BTreeSet::new();
        let mut runs = 0usize;
        for entry in b
            .files()
            .iter()
            .map(|f| f.entry_index)
            .chain(b.children().iter().map(|c| c.entry_index))
            .chain(b.deferred().iter().map(|d| d.entry_index))
        {
            if entry < b.start_entry() || entry >= b.next_entry() || !entries.insert(entry) {
                return Err(DiscoveryError::Invalid);
            }
        }
        for file in b.files() {
            let v = &file.view;
            if v.owner.source != self.source
                || v.owner.filesystem != format!("{}:{}", profile(self.profile), self.fs_start)
            {
                return Err(DiscoveryError::Invalid);
            }
            if file.unchecked_runs.len() > v.runs.len()
                || file.unchecked_runs.windows(2).any(|w| w[0] >= w[1])
                || file
                    .unchecked_runs
                    .iter()
                    .any(|i| *i >= v.runs.len() as u64)
            {
                return Err(DiscoveryError::Invalid);
            }
            jobs::view_bytes(v).map_err(|_| DiscoveryError::Invalid)?;
            runs = runs
                .checked_add(v.runs.len())
                .filter(|n| *n <= self.limits.runs_per_batch)
                .ok_or(DiscoveryError::ResourceLimit)?;
            for run in &v.runs {
                if let Some(image) = run.image {
                    number(image.start())?;
                    number(image.end())?;
                    number(run.logical.start())?;
                }
            }
        }
        Ok(())
    }
    /// Includes bounded returned bucket postings BEFORE exact overlap filtering,
    /// immutable payload bytes and conservative per-record/index/journal work.
    pub fn write_bound(&self, b: &DirectoryBatch) -> Result<ResourceCaps> {
        self.validate_batch(b)?;
        let bytes = b.files().iter().try_fold(
            256u64 + b.children().len() as u64 * 40 + b.deferred().len() as u64 * 80,
            |n, f| {
                n.checked_add(
                    jobs::view_bytes(&f.view).map_err(|_| DiscoveryError::Invalid)?
                        + 80
                        + f.unchecked_runs.len() as u64 * 8,
                )
                .ok_or(DiscoveryError::ResourceLimit)
            },
        )?;
        let runs = b
            .files()
            .iter()
            .map(|f| f.view.runs.len() as u64)
            .sum::<u64>();
        let postings = runs.saturating_mul(self.limits.posting_cap as u64 + 1);
        let rows = b.files().len() as u64
            + b.children().len() as u64
            + b.deferred().len() as u64
            + runs * 3;
        Ok(ResourceCaps {
            read_bytes: postings.saturating_mul(192).saturating_add(bytes),
            memory_bytes: bytes
                .saturating_mul(4)
                .saturating_add((self.limits.posting_cap as u64 + 1) * 256)
                .saturating_add(65536),
            index_bytes: bytes
                .saturating_add(rows.saturating_mul(512))
                .saturating_add(16384),
            output_bytes: 0,
            visited_postings: 0,
            transitions: 0,
            decoder_seconds: 0,
        })
    }
    pub fn commit_batch(
        &self,
        ledger: &mut Ledger,
        b: &DirectoryBatch,
        proof: SealedDiscoveryBatch,
    ) -> Result<BatchAdoption> {
        self.validate_batch(b)?;
        self.verified.set(false);
        proof.validate(ledger, &self.source, b.digest())?;
        require_source(ledger, &self.work, &self.source)?;
        let id = self.batch_id(b);
        let semantic = hash(b.digest());
        if let Some((digest, postings, conflicts)) = ledger
            .connection
            .query_row(
                "SELECT digest,postings,conflicts FROM discovery_batches WHERE id=?1",
                [&id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()?
        {
            if digest != semantic {
                return Err(DiscoveryError::Conflict);
            }
            let (size, payload_hash): (i64, String) = ledger.connection.query_row(
                "SELECT bytes,digest FROM discovery_payloads WHERE batch=?1 AND committed=1",
                [&id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            verify_payload(
                &self.root.join(format!("{id}.batch")),
                u64::try_from(size).map_err(|_| DiscoveryError::Corrupt)?,
                &payload_hash,
            )?;
            return Ok(BatchAdoption {
                id,
                adopted: true,
                files: b.files().len() as u64,
                postings: postings as u64,
                conflicts: conflicts as u64,
            });
        }
        self.commit_records(ledger, b, id, semantic)
    }
    fn commit_records(
        &self,
        ledger: &mut Ledger,
        b: &DirectoryBatch,
        id: String,
        semantic: String,
    ) -> Result<BatchAdoption> {
        let mut payload = b"FHDISC1\0".to_vec();
        payload.extend(b.digest());
        let g = b.geometry();
        for n in [
            g.block_size,
            g.block_count,
            g.root_inode,
            g.start_in_image,
            b.directory_inode(),
            b.start_entry(),
            b.total_entries(),
            b.next_entry(),
            u64::from(b.directory_complete()),
            b.exclusions().virtual_files,
            b.exclusions().virtual_directories,
        ] {
            payload.extend(n.to_be_bytes());
        }
        payload.extend((b.namespace_policy().len() as u64).to_be_bytes());
        payload.extend(b.namespace_policy().as_bytes());
        payload.extend((b.children().len() as u64).to_be_bytes());
        for c in b.children() {
            for n in [c.entry_index, c.inode, c.name_flags, c.metadata_flags] {
                payload.extend(n.to_be_bytes());
            }
        }
        payload.extend((b.deferred().len() as u64).to_be_bytes());
        for d in b.deferred() {
            for n in [
                d.entry_index,
                u64::from(d.inode.is_some()),
                d.inode.unwrap_or(0),
            ] {
                payload.extend(n.to_be_bytes());
            }
            let reason = format!("{:?}", d.reason);
            payload.extend((reason.len() as u64).to_be_bytes());
            payload.extend(reason.as_bytes());
        }
        payload.extend((b.files().len() as u64).to_be_bytes());
        let mut records = Vec::new();
        for file in b.files() {
            let mut view = file.view.clone();
            if view.evidence == ExtentEvidence::MetadataAllocated {
                view.evidence = ExtentEvidence::MetadataCandidate;
            }
            let encoded = jobs::encode_view(&view, ViewStatus::Provisional)
                .map_err(|_| DiscoveryError::Invalid)?;
            let owner = hash(format!(
                "{}:{}:{}",
                view.owner.source.0, view.owner.filesystem, view.owner.file_identity
            ));
            let digest = hash(&encoded);
            let view_id = hash(format!("{}:{owner}:{digest}", self.domain));
            payload.extend(file.entry_index.to_be_bytes());
            payload.extend((file.unchecked_runs.len() as u64).to_be_bytes());
            for i in &file.unchecked_runs {
                payload.extend(i.to_be_bytes());
            }
            payload.extend((encoded.len() as u64).to_be_bytes());
            let offset = payload.len() as u64;
            payload.extend(&encoded);
            records.push((owner, view_id, digest, offset, encoded.len() as u64));
        }
        let payload_digest = hash(&payload);
        let size = payload.len() as u64;
        // Reservation survives a crash before file creation/final cursor commit.
        // Thus unregistered files cannot evade the configured index quota.
        {
            let tx = ledger
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            let existing: Option<(String, i64)> = tx
                .query_row(
                    "SELECT digest,bytes FROM discovery_payloads WHERE batch=?1",
                    [&id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            if let Some((d, n)) = existing {
                if d != payload_digest || n != number(size)? {
                    return Err(DiscoveryError::Conflict);
                }
            } else {
                let used: i64 = tx.query_row(
                    "SELECT used FROM discovery_storage WHERE singleton=1",
                    [],
                    |r| r.get(0),
                )?;
                if (used as u64)
                    .checked_add(size)
                    .is_none_or(|n| n > self.limits.index_bytes)
                {
                    return Err(DiscoveryError::ResourceLimit);
                }
                tx.execute(
                    "INSERT INTO discovery_payloads(batch,domain,digest,bytes)VALUES(?1,?2,?3,?4)",
                    params![id, self.domain, payload_digest, number(size)?],
                )?;
                tx.execute(
                    "UPDATE discovery_storage SET used=used+?1 WHERE singleton=1",
                    [number(size)?],
                )?;
                tx.execute(
                    "UPDATE discovery_domains SET index_bytes=index_bytes+?2 WHERE id=?1",
                    params![self.domain, number(size)?],
                )?;
            }
            tx.commit()?;
        }
        let path = self.root.join(format!("{id}.batch"));
        match fs::symlink_metadata(&path) {
            Ok(m) => {
                if !m.is_file() || m.len() != size || hash(fs::read(&path)?) != payload_digest {
                    return Err(DiscoveryError::Corrupt);
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let mut f = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)?;
                f.write_all(&payload)?;
                f.sync_all()?;
                File::open(&self.root)?.sync_all()?
            }
            Err(e) => return Err(e.into()),
        }
        let tx = ledger
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let prior_visits: i64 = tx.query_row(
            "SELECT geometry_visits FROM discovery_domains WHERE id=?1",
            [&self.domain],
            |r| r.get(0),
        )?;
        let mut postings = 0u64;
        let mut conflicts = 0u64;
        tx.execute_batch("SAVEPOINT discovery_records")?;
        let result = (|| -> Result<BatchAdoption> {
            let (state, geometry, old_bucket): (String, Option<Vec<u8>>, Option<i64>) = tx
                .query_row(
                    "SELECT state,geometry,bucket FROM discovery_domains WHERE id=?1",
                    [&self.domain],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )?;
            if state != "active" {
                return Err(DiscoveryError::Conflict);
            }
            let g = b.geometry();
            let geometry_bytes = [
                g.block_size.to_be_bytes(),
                g.block_count.to_be_bytes(),
                g.root_inode.to_be_bytes(),
                g.start_in_image.to_be_bytes(),
            ]
            .concat();
            let domain_end = g
                .block_count
                .checked_mul(g.block_size)
                .and_then(|n| n.checked_add(g.start_in_image))
                .ok_or(DiscoveryError::ResourceLimit)?;
            number(domain_end)?;
            let bucket = if let Some(old) = old_bucket {
                old as u64
            } else {
                let min = (domain_end / (i32::MAX as u64))
                    .saturating_add(1)
                    .max(g.block_size);
                min.checked_next_power_of_two()
                    .ok_or(DiscoveryError::ResourceLimit)?
            };
            if let Some(old) = geometry {
                if old != geometry_bytes {
                    return Err(DiscoveryError::Conflict);
                }
            } else {
                if b.directory_inode() != g.root_inode || b.start_entry() != 0 {
                    return Err(DiscoveryError::Conflict);
                }
                tx.execute(
                    "UPDATE discovery_domains SET geometry=?2,bucket=?3 WHERE id=?1",
                    params![self.domain, geometry_bytes, number(bucket)?],
                )?;
                tx.execute(
                    "INSERT INTO discovery_dirs(domain,inode)VALUES(?1,?2)",
                    params![self.domain, inode(g.root_inode).as_slice()],
                )?;
            }
            let (next, total, dir_state): (i64, Option<i64>, String) = tx.query_row(
                "SELECT next_entry,total,state FROM discovery_dirs WHERE domain=?1 AND inode=?2",
                params![self.domain, inode(b.directory_inode()).as_slice()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?;
            if dir_state != "pending"
                || next != number(b.start_entry())?
                || total.is_some_and(|n| n != b.total_entries() as i64)
            {
                return Err(DiscoveryError::Conflict);
            }
            let domain_number: i64 = tx.query_row(
                "SELECT rowid FROM discovery_domains WHERE id=?1",
                [&self.domain],
                |r| r.get(0),
            )?;
            if domain_number > i32::MAX as i64 {
                return Err(DiscoveryError::ResourceLimit);
            }
            for (file, (owner, view_id, digest, offset, bytes)) in b.files().iter().zip(&records) {
                let prior: Option<(String, Vec<u8>)> = tx
                    .query_row(
                        "SELECT digest,unchecked FROM discovery_views WHERE domain=?1 AND owner=?2",
                        params![self.domain, owner],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?;
                if let Some((prior, prior_unchecked)) = prior {
                    let unchecked = file
                        .unchecked_runs
                        .iter()
                        .flat_map(|n| n.to_be_bytes())
                        .collect::<Vec<_>>();
                    if prior != *digest || prior_unchecked != unchecked {
                        return Err(DiscoveryError::Conflict);
                    }
                    continue;
                }
                let mut local = file.view.clone();
                if local.evidence == ExtentEvidence::MetadataCandidate {
                    crate::filesystem::classify(&mut local);
                }
                if !file.unchecked_runs.is_empty()
                    && local.evidence == ExtentEvidence::MetadataAllocated
                {
                    local.evidence = ExtentEvidence::MetadataCandidate;
                }
                let unchecked = file
                    .unchecked_runs
                    .iter()
                    .flat_map(|i| i.to_be_bytes())
                    .collect::<Vec<_>>();
                let allocation_negative = file.view.runs.iter().enumerate().any(|(i, r)| {
                    !r.allocation_consistent && !file.unchecked_runs.contains(&(i as u64))
                });
                tx.execute("INSERT INTO discovery_views(domain,owner,id,batch,offset,bytes,digest,base,unchecked,allocation_negative)VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![self.domain,owner,view_id,id,number(*offset)?,number(*bytes)?,digest,evidence(local.evidence),unchecked,allocation_negative])?;
                for run in &file.view.runs {
                    let Some(image) = run.image else { continue };
                    if image.is_empty() {
                        return Err(DiscoveryError::Invalid);
                    }
                    let lo = number(image.start() / bucket)?;
                    let hi = number((image.end() - 1) / bucket)?;
                    if hi > i32::MAX as i64 {
                        return Err(DiscoveryError::ResourceLimit);
                    }
                    // Materialize only capped BUCKET candidates; exact filtering is
                    // Rust work after the posting cap, never a post-filter SQL LIMIT.
                    let remaining = self
                        .limits
                        .geometry_visits
                        .checked_sub(prior_visits as u64)
                        .and_then(|n| n.checked_sub(postings))
                        .ok_or(DiscoveryError::ResourceLimit)?;
                    if remaining == 0 {
                        return Err(DiscoveryError::ResourceLimit);
                    }
                    let query_cap = (self.limits.posting_cap as u64 + 1).min(remaining) as usize;
                    let candidates = {
                        let mut q=tx.prepare("SELECT e.owner,e.start,e.end FROM discovery_geometry r JOIN discovery_extents e ON e.id=r.id WHERE r.lo<=?1 AND r.hi>=?2 AND r.domain_lo<=?3 AND r.domain_hi>=?3 LIMIT ?4")?;
                        q.query_map(params![hi, lo, domain_number, query_cap as i64], |r| {
                            Ok((
                                r.get::<_, String>(0)?,
                                r.get::<_, i64>(1)?,
                                r.get::<_, i64>(2)?,
                            ))
                        })?
                        .collect::<std::result::Result<Vec<_>, _>>()?
                    };
                    postings += candidates.len() as u64;
                    if candidates.len() > self.limits.posting_cap
                        || (query_cap < self.limits.posting_cap + 1
                            && candidates.len() == query_cap)
                    {
                        return Err(DiscoveryError::ResourceLimit);
                    }
                    for (other, start, end) in candidates {
                        if (start as u64) < image.end() && (end as u64) > image.start() {
                            conflicts += 1;
                            tx.execute("UPDATE discovery_views SET shared=1 WHERE domain=?1 AND owner IN(?2,?3)",params![self.domain,owner,other])?;
                        }
                    }
                    tx.execute("INSERT INTO discovery_extents(domain,owner,logical,start,end)VALUES(?1,?2,?3,?4,?5)",params![self.domain,owner,number(run.logical.start())?,number(image.start())?,number(image.end())?])?;
                    tx.execute(
                    "INSERT INTO discovery_geometry(id,domain_lo,domain_hi,lo,hi)VALUES(?1,?2,?2,?3,?4)",
                    params![tx.last_insert_rowid(), domain_number, lo, hi],
                )?;
                }
            }
            for child in b.children() {
                tx.execute(
                    "INSERT INTO discovery_aliases(domain,parent,entry,child)VALUES(?1,?2,?3,?4)",
                    params![
                        self.domain,
                        inode(b.directory_inode()).as_slice(),
                        number(child.entry_index)?,
                        inode(child.inode).as_slice()
                    ],
                )?;
                let prior: Option<Vec<u8>> = tx
                    .query_row(
                        "SELECT inode FROM discovery_dirs WHERE domain=?1 AND inode=?2",
                        params![self.domain, inode(child.inode).as_slice()],
                        |r| r.get(0),
                    )
                    .optional()?;
                if prior.is_some() {
                    tx.execute("INSERT INTO discovery_deferred(domain,parent,entry,reason)VALUES(?1,?2,?3,'directory-alias-or-cycle')",params![self.domain,inode(b.directory_inode()).as_slice(),number(child.entry_index)?])?;
                } else {
                    tx.execute(
                        "INSERT INTO discovery_dirs(domain,inode)VALUES(?1,?2)",
                        params![self.domain, inode(child.inode).as_slice()],
                    )?;
                }
            }
            for d in b.deferred() {
                tx.execute(
                    "INSERT INTO discovery_deferred(domain,parent,entry,reason)VALUES(?1,?2,?3,?4)",
                    params![
                        self.domain,
                        inode(b.directory_inode()).as_slice(),
                        number(d.entry_index)?,
                        format!("{:?}", d.reason)
                    ],
                )?;
            }
            tx.execute("UPDATE discovery_dirs SET next_entry=?3,total=?4,state=?5 WHERE domain=?1 AND inode=?2",params![self.domain,inode(b.directory_inode()).as_slice(),number(b.next_entry())?,number(b.total_entries())?,if b.directory_complete(){"complete"}else{"pending"}])?;
            tx.execute("UPDATE discovery_domains SET excluded_files=excluded_files+?2,excluded_dirs=excluded_dirs+?3 WHERE id=?1",params![self.domain,number(b.exclusions().virtual_files)?,number(b.exclusions().virtual_directories)?])?;
            tx.execute("INSERT INTO discovery_batches(id,domain,directory,start,next,digest,postings,conflicts)VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![id,self.domain,inode(b.directory_inode()).as_slice(),number(b.start_entry())?,number(b.next_entry())?,semantic,number(postings)?,number(conflicts)?])?;
            tx.execute(
                "INSERT INTO index_runs(id,work,path,digest,bytes)VALUES(?1,?2,?3,?4,?5)",
                params![
                    id,
                    self.work,
                    format!("discovery-indexes/{id}.batch"),
                    payload_digest,
                    number(size)?
                ],
            )?;
            tx.execute(
                "UPDATE discovery_payloads SET committed=1 WHERE batch=?1",
                [&id],
            )?;
            Ok(BatchAdoption {
                id,
                adopted: false,
                files: b.files().len() as u64,
                postings,
                conflicts,
            })
        })();
        if result.is_err() {
            tx.execute_batch("ROLLBACK TO discovery_records")?;
        }
        tx.execute_batch("RELEASE discovery_records")?;
        // Failed bounded probes consumed work too. Savepoint rollback removes
        // candidate/index/cursor mutations, while this outer transaction retains
        // actual attempt counters so retries cannot reset the global visit cap.
        tx.execute("UPDATE discovery_domains SET geometry_visits=geometry_visits+?2,geometry_conflicts=geometry_conflicts+?3 WHERE id=?1",params![self.domain,number(postings)?,number(conflicts)?])?;
        tx.commit()?;
        result
    }
    /// Full committed-payload verification plus bounded same-Ledger identity
    /// cache writes. Call once per controller run before completion or paging.
    pub fn catalog_read_bound(&self, ledger: &Ledger) -> Result<ResourceCaps> {
        require_source(ledger, &self.work, &self.source)?;
        let (count,bytes):(i64,i64)=ledger.connection.query_row("SELECT count(*),coalesce(sum(p.bytes),0) FROM discovery_batches b LEFT JOIN discovery_payloads p ON p.batch=b.id AND p.committed=1 WHERE b.domain=?1",[&self.domain],|r|Ok((r.get(0)?,r.get(1)?)))?;
        if count < 0 || bytes < 0 || bytes as u64 > self.limits.index_bytes {
            return Err(DiscoveryError::Corrupt);
        }
        Ok(ResourceCaps {
            read_bytes: bytes as u64 + count as u64 * 1024 + 65536,
            memory_bytes: 2 * 1024 * 1024,
            index_bytes: count as u64 * 256 + 16384,
            output_bytes: 0,
            visited_postings: 0,
            transitions: 0,
            decoder_seconds: 0,
        })
    }
    pub fn verify_catalog(&self, ledger: &Ledger) -> Result<()> {
        self.verified.set(false);
        require_source(ledger, &self.work, &self.source)?;
        let mut after = String::new();
        loop {
            let rows = {
                let mut q=ledger.connection.prepare("SELECT b.id,p.bytes,p.digest,i.bytes,i.digest,i.path FROM discovery_batches b LEFT JOIN discovery_payloads p ON p.batch=b.id AND p.committed=1 LEFT JOIN index_runs i ON i.id=b.id WHERE b.domain=?1 AND b.id>?2 ORDER BY b.id LIMIT 128")?;
                q.query_map(params![self.domain, after], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<i64>>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, Option<i64>>(3)?,
                        r.get::<_, Option<String>>(4)?,
                        r.get::<_, Option<String>>(5)?,
                    ))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?
            };
            if rows.is_empty() {
                break;
            }
            for (id, size, digest, registered_size, registered_digest, path) in rows {
                if id.len() != 64
                    || !id.bytes().all(|b| b.is_ascii_hexdigit())
                    || size != registered_size
                    || digest != registered_digest
                    || path.as_deref() != Some(format!("discovery-indexes/{id}.batch").as_str())
                {
                    return Err(DiscoveryError::Corrupt);
                }
                let size = u64::try_from(size.ok_or(DiscoveryError::Corrupt)?)
                    .map_err(|_| DiscoveryError::Corrupt)?;
                let digest = digest.ok_or(DiscoveryError::Corrupt)?;
                let identity =
                    verify_payload(&self.root.join(format!("{id}.batch")), size, &digest)?;
                ledger.connection.execute("UPDATE discovery_payloads SET verified_identity=?2,verified_epoch=?3 WHERE batch=?1",params![id,identity,self.verification_epoch])?;
                after = id;
            }
        }
        self.verified.set(true);
        Ok(())
    }
    pub fn finish(&self, ledger: &mut Ledger) -> Result<bool> {
        require_source(ledger, &self.work, &self.source)?;
        if !self.verified.get() {
            return Err(DiscoveryError::Incomplete);
        }
        let status = self.status(ledger)?;
        if status.complete {
            return Ok(true);
        }
        if status.batches == 0 || status.pending_directories != 0 || status.deferred_entries != 0 {
            return Ok(false);
        }
        let tx = ledger
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "UPDATE discovery_domains SET state='complete' WHERE id=?1 AND state='active'",
            [&self.domain],
        )?;
        tx.execute("INSERT INTO receipts(work,kind,payload)VALUES(?1,'directory-discovery-complete-v1',?2)",params![self.work,format!("{}:{}:{}:{}",status.namespace_policy,self.domain,status.excluded_virtual_files,status.excluded_virtual_directories).as_bytes()])?;
        tx.commit()?;
        Ok(true)
    }
    pub fn qualified_page(
        &self,
        ledger: &Ledger,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<IndexedView>> {
        self.qualified_page_filtered(ledger, after, limit, false)
    }
    pub fn pending_qualified_page(
        &self,
        ledger: &Ledger,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<IndexedView>> {
        self.qualified_page_filtered(ledger, after, limit, true)
    }
    fn qualified_page_filtered(
        &self,
        ledger: &Ledger,
        after: Option<&str>,
        limit: usize,
        pending: bool,
    ) -> Result<Vec<IndexedView>> {
        if limit == 0 || limit > self.limits.page_size {
            return Err(DiscoveryError::Invalid);
        }
        require_source(ledger, &self.work, &self.source)?;
        if !self.verified.get() {
            return Err(DiscoveryError::Incomplete);
        }
        let complete: bool = ledger.connection.query_row(
            "SELECT state='complete' FROM discovery_domains WHERE id=?1",
            [&self.domain],
            |r| r.get(0),
        )?;
        if !complete {
            return Err(DiscoveryError::Incomplete);
        }
        let rows = {
            let mut s=ledger.connection.prepare("SELECT v.id,v.batch,v.offset,v.bytes,v.digest,v.base,v.shared,p.bytes,p.verified_identity,p.verified_epoch FROM discovery_views v JOIN discovery_payloads p ON p.batch=v.batch WHERE v.domain=?1 AND v.id>?2 AND (?4=0 OR NOT EXISTS(SELECT 1 FROM receipts r WHERE r.work=v.media_work AND r.kind=?5)) ORDER BY v.id LIMIT ?3")?;
            s.query_map(
                params![
                    self.domain,
                    after.unwrap_or(""),
                    limit as i64,
                    i64::from(pending),
                    crate::ledger::MEDIA_OUTCOME
                ],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, i64>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, i64>(5)?,
                        r.get::<_, bool>(6)?,
                        r.get::<_, i64>(7)?,
                        r.get::<_, Option<String>>(8)?,
                        r.get::<_, Option<String>>(9)?,
                    ))
                },
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?
        };
        let mut out = Vec::new();
        for (
            id,
            batch,
            offset,
            bytes,
            digest,
            base,
            shared,
            total,
            verified_identity,
            verified_epoch,
        ) in rows
        {
            if !(0..=128 * 1024).contains(&bytes)
                || offset < 8
                || offset.checked_add(bytes).is_none_or(|n| n > total)
            {
                return Err(DiscoveryError::Corrupt);
            }
            let path = self.root.join(format!("{batch}.batch"));
            let meta = fs::symlink_metadata(&path)?;
            if !meta.is_file() || meta.len() != total as u64 {
                return Err(DiscoveryError::Corrupt);
            }
            if verified_epoch.as_deref() != Some(self.verification_epoch.as_str())
                || verified_identity.as_deref() != Some(stamp(&meta).as_str())
            {
                return Err(DiscoveryError::Corrupt);
            }
            let expected_identity = verified_identity.unwrap();
            let mut f = File::open(&path)?;
            if stamp(&f.metadata()?) != expected_identity {
                return Err(DiscoveryError::Corrupt);
            }
            f.seek(SeekFrom::Start(offset as u64))?;
            let mut encoded = vec![0; bytes as usize];
            f.read_exact(&mut encoded)?;
            if stamp(&f.metadata()?) != expected_identity
                || stamp(&fs::symlink_metadata(&path)?) != expected_identity
                || hash(&encoded) != digest
            {
                return Err(DiscoveryError::Corrupt);
            }
            let (mut view, status) =
                jobs::decode_view(&encoded).map_err(|_| DiscoveryError::Corrupt)?;
            if status != ViewStatus::Provisional || view.owner.source != self.source {
                return Err(DiscoveryError::Corrupt);
            }
            view.evidence = if shared {
                ExtentEvidence::SharedGeometry
            } else {
                decode_evidence(base)?
            };
            out.push(IndexedView {
                id,
                view,
                status: ViewStatus::Qualified,
            });
        }
        Ok(out)
    }
}
fn evidence(e: ExtentEvidence) -> i64 {
    match e {
        ExtentEvidence::MetadataAllocated => 0,
        ExtentEvidence::MetadataCandidate => 1,
        ExtentEvidence::DeletedOrRecovery => 2,
        ExtentEvidence::SharedGeometry => 3,
        ExtentEvidence::Partial => 4,
        ExtentEvidence::Unsupported => 5,
    }
}
fn decode_evidence(n: i64) -> Result<ExtentEvidence> {
    Ok(match n {
        0 => ExtentEvidence::MetadataAllocated,
        1 => ExtentEvidence::MetadataCandidate,
        2 => ExtentEvidence::DeletedOrRecovery,
        3 => ExtentEvidence::SharedGeometry,
        4 => ExtentEvidence::Partial,
        5 => ExtentEvidence::Unsupported,
        _ => return Err(DiscoveryError::Corrupt),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        acquisition::ReadLimits,
        checkpoint::{CheckpointLimits, CheckpointStore},
        contracts::{BindingEvidence, ByteRange, CoordinateBinding, LogicalOwner},
        control::CaseConfig,
        filesystem::{
            DirectoryCounters, DirectoryFile, ExtentRun, FilesystemGeometry, LogicalFileView,
        },
        session::{CaseSession, SessionInput},
    };
    use std::{
        collections::BTreeMap,
        sync::atomic::{AtomicU64, Ordering},
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture {
        root: PathBuf,
        session: CaseSession,
        store: CheckpointStore,
        queue: DiscoveryQueue,
    }
    impl Fixture {
        fn new(limits: DiscoveryLimits) -> Self {
            let root = std::env::temp_dir().canonicalize().unwrap().join(format!(
                "frameharbor-discovery-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            fs::write(root.join("image"), b"abcdefghijklmnop").unwrap();
            fs::write(root.join("map"), b"0 + 1\n0 16 +\n").unwrap();
            let ledger = Ledger::open(&root.join("case.sqlite"), 64 * 1024 * 1024).unwrap();
            let mut session = CaseSession::create(
                ledger,
                SessionInput {
                    image: root.join("image"),
                    map: root.join("map"),
                    binding: CoordinateBinding::new(
                        ByteRange::new(0, 16).unwrap(),
                        0,
                        16,
                        BindingEvidence::OperatorAttested,
                    )
                    .unwrap(),
                    limits: ReadLimits {
                        per_read: 16,
                        total: 1024 * 1024,
                        receipts: 1024,
                    },
                },
                CaseConfig {
                    total_millis: 60000,
                    shutdown_reserve_millis: 1000,
                    caps: ResourceCaps {
                        read_bytes: 1024 * 1024 * 1024,
                        memory_bytes: 512 * 1024 * 1024,
                        index_bytes: 1024 * 1024 * 1024,
                        output_bytes: 1024 * 1024,
                        visited_postings: 10000000,
                        transitions: 10000000,
                        decoder_seconds: 0,
                    },
                },
            )
            .unwrap();
            let spec = WorkSpec {
                source: session.source_generation(),
                algorithm: "discovery-synthetic-v1".into(),
                stage: "filesystem-discovery".into(),
                parameters: BTreeMap::from([("helper-sha256".into(), "pinned-synthetic".into())]),
            };
            let queue = session
                .with_ledger(65536, |l| {
                    Ok(DiscoveryQueue::open(
                        &root,
                        l,
                        &spec,
                        FilesystemProfile::Fat32,
                        0,
                        limits,
                    ))
                })
                .unwrap()
                .unwrap();
            let store =
                CheckpointStore::create(&root.join("frontiers"), CheckpointLimits::default())
                    .unwrap();
            Self {
                root,
                session,
                store,
                queue,
            }
        }
        fn file(&self, entry: u64, owner: u64, physical: u64) -> DirectoryFile {
            DirectoryFile {
                entry_index: entry,
                unchecked_runs: Vec::new(),
                view: LogicalFileView {
                    owner: LogicalOwner {
                        source: self.session.source_generation(),
                        filesystem: "fat32:0".into(),
                        file_identity: format!("file-{owner}"),
                    },
                    size: 16,
                    metadata_flags: 5,
                    name_flags: 1,
                    attribute_flags: 3,
                    initialized_size: 16,
                    skip_length: 0,
                    runs: vec![ExtentRun {
                        logical: ByteRange::from_len(0, 16).unwrap(),
                        image: Some(ByteRange::from_len(physical, 16).unwrap()),
                        raw_flags: 0,
                        allocation_consistent: true,
                    }],
                    evidence: ExtentEvidence::MetadataCandidate,
                },
            }
        }
        fn batch(&self, start: u64, total: u64, files: Vec<DirectoryFile>) -> DirectoryBatch {
            let next = start + files.len() as u64;
            DirectoryBatch::from_parts(
                FilesystemGeometry {
                    profile: FilesystemProfile::Fat32,
                    block_size: 512,
                    block_count: 1024 * 1024,
                    root_inode: 2,
                    start_in_image: 0,
                },
                2,
                start,
                total,
                next,
                next == total,
                files,
                Vec::new(),
                Vec::new(),
                DirectoryCounters {
                    entries_completed: next - start,
                    ..DirectoryCounters::default()
                },
            )
        }
        fn commit(&mut self, b: &DirectoryBatch) -> Result<BatchAdoption> {
            let proof = self
                .session
                .seal_discovery_batch(&mut self.store, b.digest())
                .unwrap();
            let bound = self.queue.write_bound(b).unwrap();
            self.session
                .with_ledger_resources(bound, |l| Ok(self.queue.commit_batch(l, b, proof)))
                .unwrap()
        }
        fn status(&mut self) -> DiscoveryStatus {
            self.session
                .with_ledger_resources(
                    ResourceCaps {
                        read_bytes: 65536,
                        memory_bytes: 65536,
                        index_bytes: 0,
                        output_bytes: 0,
                        visited_postings: 0,
                        transitions: 0,
                        decoder_seconds: 0,
                    },
                    |l| Ok(self.queue.status(l)),
                )
                .unwrap()
                .unwrap()
        }
        fn finish(&mut self) -> bool {
            let bound = self
                .session
                .with_ledger(65536, |l| Ok(self.queue.catalog_read_bound(l)))
                .unwrap()
                .unwrap();
            self.session
                .with_ledger_resources(bound, |l| Ok(self.queue.verify_catalog(l)))
                .unwrap()
                .unwrap();
            self.session
                .with_ledger(65536, |l| Ok(self.queue.finish(l)))
                .unwrap()
                .unwrap()
        }
        fn page(&mut self, after: Option<&str>) -> Vec<IndexedView> {
            self.session
                .with_ledger_resources(
                    ResourceCaps {
                        read_bytes: 1024 * 1024,
                        memory_bytes: 1024 * 1024,
                        index_bytes: 0,
                        output_bytes: 0,
                        visited_postings: 0,
                        transitions: 0,
                        decoder_seconds: 0,
                    },
                    |l| Ok(self.queue.qualified_page(l, after, 128)),
                )
                .unwrap()
                .unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
    #[test]
    fn more_than_1024_views_adopt_in_bounded_batches_and_keyset_pages() {
        let mut f = Fixture::new(DiscoveryLimits::default());
        let mut start = 0;
        while start < 1100 {
            let n = (1100 - start).min(256);
            let files = (start..start + n)
                .map(|i| f.file(i, i, 4096 + i * 512))
                .collect();
            let b = f.batch(start, 1100, files);
            f.commit(&b).unwrap();
            assert!(!f.status().complete);
            start += n;
        }
        assert_eq!(f.status().views, 1100);
        assert!(f.finish());
        let mut after = None;
        let mut count = 0;
        loop {
            let page = f.page(after.as_deref());
            if page.is_empty() {
                break;
            }
            assert!(page.len() <= 128);
            assert!(
                page.iter()
                    .all(|v| v.view.evidence == ExtentEvidence::MetadataAllocated)
            );
            count += page.len();
            after = Some(page.last().unwrap().id.clone());
        }
        assert_eq!(count, 1100);
    }
    #[test]
    fn cross_batch_alias_replay_and_same_owner_physical_reuse_are_distinguished() {
        let mut f = Fixture::new(DiscoveryLimits::default());
        let b = f.batch(0, 4, vec![f.file(0, 0, 4096)]);
        f.commit(&b).unwrap();
        let before = f.status();
        assert!(f.commit(&b).unwrap().adopted);
        assert_eq!(f.status(), before);
        let second = f.batch(1, 4, vec![f.file(1, 1, 4096), f.file(2, 0, 4096)]);
        let outcome = f.commit(&second).unwrap();
        assert_eq!(outcome.conflicts, 1);
        assert_eq!(f.status().views, 2);
        let mut reused = f.file(3, 2, 8192);
        reused.view.size = 32;
        reused.view.initialized_size = 32;
        reused.view.runs.push(ExtentRun {
            logical: ByteRange::new(16, 32).unwrap(),
            image: Some(ByteRange::from_len(8200, 16).unwrap()),
            raw_flags: 0,
            allocation_consistent: true,
        });
        let last = f.batch(3, 4, vec![reused]);
        assert_eq!(f.commit(&last).unwrap().conflicts, 1);
        assert!(f.finish());
        assert!(
            f.page(None)
                .iter()
                .all(|v| v.view.evidence == ExtentEvidence::SharedGeometry)
        );
    }
    #[test]
    fn bucket_posting_cap_precedes_exact_filter_and_rolls_back_cursor_and_geometry() {
        let mut f = Fixture::new(DiscoveryLimits {
            posting_cap: 2,
            ..DiscoveryLimits::default()
        });
        let files = (0..4)
            .map(|i| {
                let mut v = f.file(i, i, 8192 + i * 2);
                v.view.size = 1;
                v.view.initialized_size = 1;
                v.view.runs[0].logical = ByteRange::from_len(0, 1).unwrap();
                v.view.runs[0].image = Some(ByteRange::from_len(8192 + i * 2, 1).unwrap());
                v
            })
            .collect();
        let b = f.batch(0, 4, files);
        assert!(matches!(f.commit(&b), Err(DiscoveryError::ResourceLimit)));
        let status = f.status();
        assert_eq!(
            (status.views, status.batches, status.directories),
            (0, 0, 0)
        );
        assert!(status.index_bytes > 0);
        assert!(!f.finish());
        let next = f
            .session
            .with_ledger_resources(
                ResourceCaps {
                    read_bytes: 65536,
                    memory_bytes: 65536,
                    index_bytes: 0,
                    output_bytes: 0,
                    visited_postings: 0,
                    transitions: 0,
                    decoder_seconds: 0,
                },
                |l| Ok(f.queue.next(l)),
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            next,
            Some(DirectoryCursor {
                directory_inode: None,
                next_entry: 0
            })
        );
    }
    #[test]
    fn wrong_source_batch_proof_cannot_create_payload_or_advance_cursor() {
        let mut f = Fixture::new(DiscoveryLimits::default());
        let b = f.batch(0, 1, vec![f.file(0, 0, 4096)]);
        let proof = f
            .session
            .seal_discovery_batch(&mut f.store, [99; 32])
            .unwrap();
        let result = f
            .session
            .with_ledger(65536, |l| Ok(f.queue.commit_batch(l, &b, proof)))
            .unwrap();
        assert!(result.is_err());
        assert_eq!(f.status().index_bytes, 0);
        assert_eq!(
            fs::read_dir(f.root.join("discovery-indexes"))
                .unwrap()
                .count(),
            0
        );
        assert_eq!(f.status().batches, 0);
    }
    #[test]
    fn post_seal_source_read_and_different_case_proof_cannot_adopt_batch() {
        let mut f = Fixture::new(DiscoveryLimits::default());
        let b = f.batch(0, 1, vec![f.file(0, 0, 4096)]);
        let proof = f
            .session
            .seal_discovery_batch(&mut f.store, b.digest())
            .unwrap();
        f.session
            .with_read(ByteRange::new(0, 1).unwrap(), |_| ())
            .unwrap();
        assert!(
            f.session
                .with_ledger(65536, |l| Ok(f.queue.commit_batch(l, &b, proof)))
                .unwrap()
                .is_err()
        );
        assert_eq!(f.status().batches, 0);
        assert_eq!(f.status().index_bytes, 0);
        // A second Ledger using the exact same source inputs obtains the same
        // generation and checkpoint payload, but cannot authorize this Ledger.
        let other_root = f.root.join("other-case");
        fs::create_dir(&other_root).unwrap();
        let mut other = CaseSession::create(
            Ledger::open(&other_root.join("case.sqlite"), 4 * 1024 * 1024).unwrap(),
            SessionInput {
                image: f.root.join("image"),
                map: f.root.join("map"),
                binding: CoordinateBinding::new(
                    ByteRange::new(0, 16).unwrap(),
                    0,
                    16,
                    BindingEvidence::OperatorAttested,
                )
                .unwrap(),
                limits: ReadLimits {
                    per_read: 16,
                    total: 1024 * 1024,
                    receipts: 1024,
                },
            },
            CaseConfig {
                total_millis: 60000,
                shutdown_reserve_millis: 1000,
                caps: ResourceCaps {
                    read_bytes: 1024 * 1024,
                    memory_bytes: 64 * 1024 * 1024,
                    index_bytes: 64 * 1024 * 1024,
                    output_bytes: 1024 * 1024,
                    visited_postings: 0,
                    transitions: 0,
                    decoder_seconds: 0,
                },
            },
        )
        .unwrap();
        assert_eq!(other.source_generation(), f.session.source_generation());
        other
            .with_read(ByteRange::new(0, 1).unwrap(), |_| ())
            .unwrap();
        let _current = f
            .session
            .seal_discovery_batch(&mut f.store, b.digest())
            .unwrap();
        let mut other_store =
            CheckpointStore::create(&other_root.join("frontiers"), CheckpointLimits::default())
                .unwrap();
        let foreign = other
            .seal_discovery_batch(&mut other_store, b.digest())
            .unwrap();
        assert_eq!(
            other.snapshot().unwrap().checkpoint,
            f.session.snapshot().unwrap().checkpoint
        );
        assert!(
            f.session
                .with_ledger(65536, |l| Ok(f.queue.commit_batch(l, &b, foreign)))
                .unwrap()
                .is_err()
        );
        assert_eq!(f.status().views, 0);
    }
    #[test]
    fn unperformed_allocation_checks_remain_distinct_from_observed_negative_evidence() {
        let mut f = Fixture::new(DiscoveryLimits::default());
        let mut unknown = f.file(0, 0, 4096);
        unknown.unchecked_runs.push(0);
        unknown.view.runs[0].allocation_consistent = false;
        let mut negative = f.file(1, 1, 8192);
        negative.view.runs[0].allocation_consistent = false;
        let b = f.batch(0, 2, vec![unknown, negative]);
        f.commit(&b).unwrap();
        assert!(f.finish());
        let views = f.page(None);
        assert!(
            views
                .iter()
                .all(|v| v.view.evidence == ExtentEvidence::MetadataCandidate)
        );
        for v in views {
            let q = f
                .session
                .with_ledger(65536, |l| Ok(f.queue.qualification(l, &v.id)))
                .unwrap()
                .unwrap();
            if v.view.owner.file_identity == "file-0" {
                assert_eq!(q.unchecked_runs, vec![0]);
                assert!(!q.observed_allocation_negative);
            } else {
                assert!(q.unchecked_runs.is_empty());
                assert!(q.observed_allocation_negative);
            }
        }
    }
    #[test]
    fn report_pages_and_media_work_binding_are_bounded_and_historical_status_survives_invalidation()
    {
        let mut f = Fixture::new(DiscoveryLimits::default());
        let b = f.batch(0, 3, (0..3).map(|i| f.file(i, i, 4096 + i * 512)).collect());
        f.commit(&b).unwrap();
        assert!(f.finish());
        let views = f.page(None);
        for (i, v) in views.iter().take(2).enumerate() {
            let spec = WorkSpec {
                source: f.session.source_generation(),
                algorithm: "media-test".into(),
                stage: "media-export".into(),
                parameters: BTreeMap::from([("view".into(), v.id.clone())]),
            };
            let work = f.session.with_ledger(65536, |l| l.submit(&spec)).unwrap();
            f.session
                .with_ledger(65536, |l| Ok(f.queue.bind_media_work(l, &v.id, &work)))
                .unwrap()
                .unwrap();
            f.session
                .with_ledger(65536, |l| Ok(f.queue.bind_media_work(l, &v.id, &work)))
                .unwrap()
                .unwrap();
            f.session
                .with_ledger(65536, |l| {
                    l.append_receipt(
                        &work,
                        crate::ledger::MEDIA_OUTCOME,
                        if i == 0 {
                            b"exported fixture"
                        } else {
                            b"unresolved fixture"
                        },
                    )
                })
                .unwrap();
        }
        let progress = f
            .session
            .with_ledger(65536, |l| Ok(f.queue.media_progress(l, 1)))
            .unwrap()
            .unwrap();
        assert_eq!(
            (
                progress.views,
                progress.completed,
                progress.pending,
                progress.unresolved
            ),
            (3, 2, 1, 1)
        );
        assert_eq!(progress.examples.len(), 1);
        let pending_bound = f
            .session
            .with_ledger(65536, |l| Ok(f.queue.pending_page_read_bound(l, None, 128)))
            .unwrap()
            .unwrap();
        let pending = f
            .session
            .with_ledger_resources(pending_bound, |l| {
                Ok(f.queue.pending_qualified_page(l, None, 128))
            })
            .unwrap()
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, views[2].id);
        let source = f.session.source_generation();
        assert!(
            f.session
                .with_ledger(65536, |l| l
                    .invalidate_source(&source, b"test invalidation"))
                .is_err()
        );
        let readonly =
            Ledger::open_readonly(&f.root.join("case.sqlite"), 64 * 1024 * 1024).unwrap();
        assert!(f.queue.status(&readonly).unwrap().complete);
        assert!(f.queue.qualified_page(&readonly, None, 128).is_err());
    }
    #[test]
    fn deferred_real_directories_prevent_global_qualification() {
        use crate::filesystem::{DeferredEntry, DeferredReason};
        let mut f = Fixture::new(DiscoveryLimits::default());
        let mut b = f.batch(0, 1, vec![f.file(0, 0, 4096)]);
        b = DirectoryBatch::from_parts(
            b.geometry().clone(),
            2,
            0,
            1,
            1,
            true,
            Vec::new(),
            Vec::new(),
            vec![DeferredEntry {
                entry_index: 0,
                inode: Some(99),
                reason: DeferredReason::MetadataUnavailable,
            }],
            DirectoryCounters::default(),
        );
        f.commit(&b).unwrap();
        assert!(!f.finish());
        assert_eq!(f.status().deferred_entries, 1);
    }
    #[test]
    fn actual_global_geometry_visit_cap_survives_failed_batch_and_retry() {
        let mut f = Fixture::new(DiscoveryLimits {
            geometry_visits: 3,
            posting_cap: 256,
            ..DiscoveryLimits::default()
        });
        let files = (0..4)
            .map(|i| {
                let mut v = f.file(i, i, 8192 + i * 2);
                v.view.size = 1;
                v.view.initialized_size = 1;
                v.view.runs[0].logical = ByteRange::from_len(0, 1).unwrap();
                v.view.runs[0].image = Some(ByteRange::from_len(8192 + i * 2, 1).unwrap());
                v
            })
            .collect();
        let b = f.batch(0, 4, files);
        assert!(matches!(f.commit(&b), Err(DiscoveryError::ResourceLimit)));
        let before = f.status();
        assert_eq!(before.geometry_visits, 3);
        assert_eq!(before.geometry_conflicts, 0);
        assert_eq!(before.views, 0);
        assert!(matches!(f.commit(&b), Err(DiscoveryError::ResourceLimit)));
        assert_eq!(f.status().geometry_visits, 3);
        assert!(!f.finish());
    }
    #[test]
    fn unchecked_allocation_never_erases_deleted_partial_or_unsupported_qualifiers() {
        let mut f = Fixture::new(DiscoveryLimits::default());
        let qualifiers = [
            ExtentEvidence::DeletedOrRecovery,
            ExtentEvidence::Partial,
            ExtentEvidence::Unsupported,
        ];
        let files = qualifiers
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let mut file = f.file(i as u64, i as u64, 4096 + i as u64 * 512);
                file.unchecked_runs = vec![0];
                file.view.runs[0].allocation_consistent = false;
                file.view.evidence = *e;
                file
            })
            .collect();
        let b = f.batch(0, 3, files);
        f.commit(&b).unwrap();
        assert!(f.finish());
        let views = f.page(None);
        for view in views {
            let i = view
                .view
                .owner
                .file_identity
                .strip_prefix("file-")
                .unwrap()
                .parse::<usize>()
                .unwrap();
            assert_eq!(view.view.evidence, qualifiers[i]);
        }
    }
    #[test]
    fn full_payload_header_corruption_is_rejected_by_pages_and_duplicate_adoption() {
        let mut f = Fixture::new(DiscoveryLimits::default());
        let b = f.batch(0, 1, vec![f.file(0, 0, 4096)]);
        let committed = f.commit(&b).unwrap();
        assert!(f.finish());
        let path = f
            .root
            .join("discovery-indexes")
            .join(format!("{}.batch", committed.id));
        let mut file = OpenOptions::new().write(true).open(&path).unwrap();
        file.write_all(b"X").unwrap();
        file.sync_all().unwrap();
        assert!(
            f.session
                .with_ledger(65536, |l| Ok(f.queue.qualified_page(l, None, 128)))
                .unwrap()
                .is_err()
        );
        assert!(f.commit(&b).is_err());
        assert_eq!(f.status().batches, 1);
    }
    #[test]
    fn missing_child_only_batch_prevents_catalog_verification_and_completion() {
        use crate::filesystem::ChildDirectory;
        let mut f = Fixture::new(DiscoveryLimits::default());
        let template = f.batch(0, 0, Vec::new());
        let g = template.geometry().clone();
        let root = DirectoryBatch::from_parts(
            g.clone(),
            2,
            0,
            1,
            1,
            true,
            Vec::new(),
            vec![ChildDirectory {
                entry_index: 0,
                inode: 3,
                name_flags: 1,
                metadata_flags: 5,
            }],
            Vec::new(),
            DirectoryCounters::default(),
        );
        let adopted = f.commit(&root).unwrap();
        let leaf = DirectoryBatch::from_parts(
            g,
            3,
            0,
            0,
            0,
            true,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            DirectoryCounters::default(),
        );
        f.commit(&leaf).unwrap();
        fs::remove_file(
            f.root
                .join("discovery-indexes")
                .join(format!("{}.batch", adopted.id)),
        )
        .unwrap();
        let bound = f
            .session
            .with_ledger(65536, |l| Ok(f.queue.catalog_read_bound(l)))
            .unwrap()
            .unwrap();
        assert!(
            f.session
                .with_ledger_resources(bound, |l| Ok(f.queue.verify_catalog(l)))
                .unwrap()
                .is_err()
        );
        assert!(
            f.session
                .with_ledger(65536, |l| Ok(f.queue.finish(l)))
                .unwrap()
                .is_err()
        );
        assert!(!f.status().complete);
    }
    #[test]
    fn media_work_batch_binding_adopts_existing_ids_and_rolls_back_missing_or_conflicting_views() {
        let mut f = Fixture::new(DiscoveryLimits::default());
        let b = f.batch(0, 3, (0..3).map(|i| f.file(i, i, 4096 + i * 512)).collect());
        f.commit(&b).unwrap();
        assert!(f.finish());
        let views = f.page(None);
        let source = f.session.source_generation();
        let spec = |view: &str, variant: &str| WorkSpec {
            source: source.clone(),
            algorithm: "batch-media-test".into(),
            stage: "media-export".into(),
            parameters: BTreeMap::from([
                ("view".into(), view.to_owned()),
                ("variant".into(), variant.to_owned()),
            ]),
        };
        let existing = spec(&views[0].id, "existing");
        let existing_id = f
            .session
            .with_ledger(4096, |l| l.submit(&existing))
            .unwrap();
        f.session
            .with_ledger(4096, |l| {
                Ok(f.queue.bind_media_work(l, &views[0].id, &existing_id))
            })
            .unwrap()
            .unwrap();
        let second = spec(&views[1].id, "new");
        let rows = [
            (views[0].id.as_str(), &existing),
            (views[1].id.as_str(), &second),
        ];
        let ids = f
            .session
            .with_ledger(65536, |l| Ok(f.queue.bind_media_work_batch(l, &rows)))
            .unwrap()
            .unwrap();
        assert_eq!(ids, vec![existing_id, second.id().unwrap()]);
        assert_eq!(
            f.session
                .with_ledger(65536, |l| Ok(f.queue.bind_media_work_batch(l, &rows)))
                .unwrap()
                .unwrap(),
            ids
        );
        let third = spec(&views[2].id, "rollback");
        let missing = spec("missing-view", "rollback");
        let rows = [(views[2].id.as_str(), &third), ("missing-view", &missing)];
        assert!(
            f.session
                .with_ledger(65536, |l| Ok(f.queue.bind_media_work_batch(l, &rows)))
                .unwrap()
                .is_err()
        );
        f.session
            .with_ledger(4096, |l| {
                assert!(matches!(l.get(&third.id()?), Err(LedgerError::MissingWork)));
                assert!(matches!(
                    l.get(&missing.id()?),
                    Err(LedgerError::MissingWork)
                ));
                Ok(())
            })
            .unwrap();
        let replacement = spec(&views[2].id, "after-rollback");
        let conflict = spec(&views[0].id, "conflicting");
        let rows = [
            (views[2].id.as_str(), &replacement),
            (views[0].id.as_str(), &conflict),
        ];
        assert!(matches!(
            f.session
                .with_ledger(65536, |l| Ok(f.queue.bind_media_work_batch(l, &rows)))
                .unwrap(),
            Err(DiscoveryError::Conflict)
        ));
        f.session
            .with_ledger(4096, |l| {
                assert!(matches!(
                    l.get(&replacement.id()?),
                    Err(LedgerError::MissingWork)
                ));
                assert!(matches!(
                    l.get(&conflict.id()?),
                    Err(LedgerError::MissingWork)
                ));
                Ok(())
            })
            .unwrap();
        f.session
            .with_ledger(4096, |l| {
                Ok(f.queue
                    .bind_media_work_batch(l, &[(views[2].id.as_str(), &replacement)]))
            })
            .unwrap()
            .unwrap();
        assert!(matches!(
            f.session
                .with_ledger(4096, |l| Ok(f.queue.bind_media_work_batch(
                    l,
                    &vec![(views[2].id.as_str(), &replacement); 129]
                )))
                .unwrap(),
            Err(DiscoveryError::ResourceLimit)
        ));
        let parameters = (0..128)
            .map(|i| (format!("key-{i}"), "x".repeat(4096)))
            .collect::<BTreeMap<_, _>>();
        let mut large1 = spec(&views[0].id, "large");
        large1.parameters = parameters.clone();
        let mut large2 = spec(&views[1].id, "large");
        large2.parameters = parameters;
        assert!(matches!(
            f.session
                .with_ledger(4096, |l| Ok(f.queue.bind_media_work_batch(
                    l,
                    &[
                        (views[0].id.as_str(), &large1),
                        (views[1].id.as_str(), &large2)
                    ]
                )))
                .unwrap(),
            Err(DiscoveryError::ResourceLimit)
        ));
    }
}
