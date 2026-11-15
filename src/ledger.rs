//! Bounded SQLite semantic state for case controls, work, and immutable indexes.
//! Media claims use the independent sealed validation/publication protocol;
//! generic work transitions cannot create validation or publication evidence.
use crate::contracts::{SourceGeneration, WorkState};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fmt, path::Path, time::Duration};

const APPLICATION_ID: i64 = 0x46484252;
const SCHEMA_VERSION: i64 = 1;
const MAX_TEXT: usize = 4096;
const MAX_RECEIPT: usize = 64 * 1024;
pub(crate) const MEDIA_OUTCOME: &str = "qualified-media-outcome-v2";
pub(crate) const PORTABLE_ARTIFACT: &str = "portable-artifact-v1";

#[derive(Debug)]
pub enum LedgerError {
    Sql(rusqlite::Error),
    InvalidInput,
    WrongDatabase,
    InvalidTransition,
    MissingWork,
    SourceInvalidated,
    Conflict,
    PublicationNotImplemented,
}
impl fmt::Display for LedgerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for LedgerError {}
impl From<rusqlite::Error> for LedgerError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sql(e)
    }
}
type Result<T> = std::result::Result<T, LedgerError>;

/// Only semantic parameters belong here. Runtime concurrency, timestamps and ETA
/// are not part of the type and must not be inserted into `parameters`.
#[derive(Debug, Clone)]
pub struct WorkSpec {
    pub source: SourceGeneration,
    pub algorithm: String,
    pub stage: String,
    pub parameters: BTreeMap<String, String>,
}
impl WorkSpec {
    pub fn id(&self) -> Result<String> {
        check_text(&self.source.0)?;
        check_text(&self.algorithm)?;
        check_text(&self.stage)?;
        if self.parameters.len() > 128 {
            return Err(LedgerError::InvalidInput);
        }
        let mut h = Sha256::new();
        h.update(b"FrameHarbor semantic work\0");
        h.update(SCHEMA_VERSION.to_be_bytes());
        for s in [&self.source.0, &self.algorithm, &self.stage] {
            hash_field(&mut h, s);
        }
        h.update((self.parameters.len() as u64).to_be_bytes());
        for (k, v) in &self.parameters {
            check_text(k)?;
            check_text(v)?;
            hash_field(&mut h, k);
            hash_field(&mut h, v);
        }
        Ok(format!("{:x}", h.finalize()))
    }
}
fn check_text(s: &str) -> Result<()> {
    if s.is_empty() || s.len() > MAX_TEXT {
        Err(LedgerError::InvalidInput)
    } else {
        Ok(())
    }
}
fn hash_field(h: &mut Sha256, s: &str) {
    h.update((s.len() as u64).to_be_bytes());
    h.update(s.as_bytes());
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkRecord {
    pub state: WorkState,
    pub source_valid: bool,
    pub elapsed_millis: u64,
    pub read_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexRecord {
    pub id: String,
    pub path: String,
    pub digest: String,
    pub bytes: u64,
}

pub struct Ledger {
    pub(crate) connection: Connection,
}
impl Ledger {
    /// Observe existing semantic state without creating files or changing SQLite
    /// pragmas. Mutating methods on this handle fail through SQLite READONLY.
    pub fn open_readonly(path: &Path, max_bytes: u64) -> Result<Self> {
        if max_bytes < 1024 * 1024 || max_bytes > i64::MAX as u64 {
            return Err(LedgerError::InvalidInput);
        }
        let meta = std::fs::symlink_metadata(path).map_err(|_| LedgerError::WrongDatabase)?;
        if !meta.is_file() || meta.len() > max_bytes {
            return Err(LedgerError::WrongDatabase);
        }
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        connection.busy_timeout(Duration::from_millis(250))?;
        let app: i64 = connection.query_row("PRAGMA application_id", [], |r| r.get(0))?;
        let version: i64 = connection.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        let page: i64 = connection.query_row("PRAGMA page_size", [], |r| r.get(0))?;
        let pages: i64 = connection.query_row("PRAGMA page_count", [], |r| r.get(0))?;
        if app != APPLICATION_ID
            || version != SCHEMA_VERSION
            || page <= 0
            || pages < 0
            || pages as u64 > max_bytes / page as u64
        {
            return Err(LedgerError::WrongDatabase);
        }
        Ok(Self { connection })
    }
    /// Bootstrap persisted job configuration without creating or changing a
    /// database, its quota, journal mode, or case state.
    pub fn read_case_manifest(path: &Path) -> Result<Vec<u8>> {
        let meta = std::fs::symlink_metadata(path).map_err(|_| LedgerError::WrongDatabase)?;
        if !meta.is_file() || meta.len() > 1024 * 1024 * 1024 {
            return Err(LedgerError::WrongDatabase);
        }
        let c = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        c.busy_timeout(Duration::from_millis(250))?;
        let app: i64 = c.query_row("PRAGMA application_id", [], |r| r.get(0))?;
        let version: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if app != APPLICATION_ID || version != SCHEMA_VERSION {
            return Err(LedgerError::WrongDatabase);
        }
        c.query_row("SELECT payload FROM case_manifest WHERE singleton=1 AND length(payload) BETWEEN 1 AND 65536",[],|r|r.get(0)).optional()?.ok_or(LedgerError::WrongDatabase)
    }
    /// `max_bytes` bounds database pages, excluding the bounded rollback journal.
    /// The case coordinator must reserve journal and destination space separately.
    pub fn open(path: &Path, max_bytes: u64) -> Result<Self> {
        if max_bytes < 1024 * 1024 || max_bytes > i64::MAX as u64 {
            return Err(LedgerError::InvalidInput);
        }
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        connection.busy_timeout(Duration::from_millis(250))?;
        let app: i64 = connection.query_row("PRAGMA application_id", [], |r| r.get(0))?;
        let version: i64 = connection.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        let tables: i64 = connection.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
            [],
            |r| r.get(0),
        )?;
        if (app != 0 && app != APPLICATION_ID)
            || (app == 0 && (tables != 0 || version != 0))
            || (app == APPLICATION_ID && version != SCHEMA_VERSION)
        {
            return Err(LedgerError::WrongDatabase);
        }
        connection.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=DELETE; PRAGMA synchronous=EXTRA; PRAGMA fullfsync=ON;")?;
        let page_size: i64 = connection.query_row("PRAGMA page_size", [], |r| r.get(0))?;
        let pages: i64 = connection.query_row("PRAGMA page_count", [], |r| r.get(0))?;
        if pages as u64 > max_bytes / page_size as u64 {
            return Err(LedgerError::InvalidInput);
        }
        connection.pragma_update(
            None,
            "max_page_count",
            (max_bytes / page_size as u64) as i64,
        )?;
        let mut ledger = Self { connection };
        if app == 0 {
            ledger.initialize()?;
        }
        Ok(ledger)
    }
    fn initialize(&mut self) -> Result<()> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch("CREATE TABLE sources(id TEXT PRIMARY KEY, valid INTEGER NOT NULL CHECK(valid IN (0,1)));
            CREATE TABLE work(id TEXT PRIMARY KEY,source TEXT NOT NULL REFERENCES sources(id),state TEXT NOT NULL,elapsed INTEGER NOT NULL DEFAULT 0 CHECK(elapsed>=0),reads INTEGER NOT NULL DEFAULT 0 CHECK(reads>=0));
            CREATE INDEX work_source ON work(source);
            CREATE TABLE source_receipts(sequence INTEGER PRIMARY KEY,source TEXT NOT NULL REFERENCES sources(id),payload BLOB NOT NULL);
            CREATE TABLE receipts(sequence INTEGER PRIMARY KEY,work TEXT NOT NULL REFERENCES work(id),kind TEXT NOT NULL,payload BLOB NOT NULL);
            CREATE TABLE charges(work TEXT NOT NULL REFERENCES work(id),event TEXT NOT NULL,elapsed INTEGER NOT NULL,reads INTEGER NOT NULL,PRIMARY KEY(work,event));
            CREATE TABLE index_runs(id TEXT PRIMARY KEY,work TEXT NOT NULL REFERENCES work(id),path TEXT NOT NULL,digest TEXT NOT NULL,bytes INTEGER NOT NULL CHECK(bytes>=0));
            CREATE TRIGGER receipts_no_update BEFORE UPDATE ON receipts BEGIN SELECT RAISE(ABORT,'append-only receipts'); END;
            CREATE TRIGGER receipts_no_delete BEFORE DELETE ON receipts BEGIN SELECT RAISE(ABORT,'append-only receipts'); END;
            CREATE TRIGGER source_receipts_no_update BEFORE UPDATE ON source_receipts BEGIN SELECT RAISE(ABORT,'append-only source receipts'); END;
            CREATE TRIGGER source_receipts_no_delete BEFORE DELETE ON source_receipts BEGIN SELECT RAISE(ABORT,'append-only source receipts'); END;
            CREATE TRIGGER indexes_no_update BEFORE UPDATE ON index_runs BEGIN SELECT RAISE(ABORT,'immutable index'); END;
            CREATE TRIGGER indexes_no_delete BEFORE DELETE ON index_runs BEGIN SELECT RAISE(ABORT,'immutable index'); END;
            PRAGMA application_id=1179140690; PRAGMA user_version=1;")?;
        tx.commit()?;
        Ok(())
    }
    pub fn submit(&mut self, spec: &WorkSpec) -> Result<String> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let id = Self::submit_in_connection(&tx, spec)?;
        tx.commit()?;
        Ok(id)
    }
    /// Shared insertion inside a caller-owned case transaction. Only crate
    /// components may compose it with their own typed, guarded state updates.
    pub(crate) fn submit_in_connection(connection: &Connection, spec: &WorkSpec) -> Result<String> {
        let id = spec.id()?;
        connection.execute(
            "INSERT OR IGNORE INTO sources(id,valid) VALUES(?1,1)",
            [&spec.source.0],
        )?;
        let valid: bool = connection.query_row(
            "SELECT valid FROM sources WHERE id=?1",
            [&spec.source.0],
            |r| r.get(0),
        )?;
        if !valid {
            return Err(LedgerError::SourceInvalidated);
        }
        connection.execute(
            "INSERT OR IGNORE INTO work(id,source,state) VALUES(?1,?2,'pending')",
            params![id, spec.source.0],
        )?;
        Ok(id)
    }
    pub fn get(&self, id: &str) -> Result<WorkRecord> {
        let row:Option<(String,bool,i64,i64)>=self.connection.query_row("SELECT w.state,s.valid,w.elapsed,w.reads FROM work w JOIN sources s ON s.id=w.source WHERE w.id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
        let (state, source_valid, elapsed_millis, read_bytes) =
            row.ok_or(LedgerError::MissingWork)?;
        Ok(WorkRecord {
            state: parse_state(&state)?,
            source_valid,
            elapsed_millis: u64::try_from(elapsed_millis)
                .map_err(|_| LedgerError::WrongDatabase)?,
            read_bytes: u64::try_from(read_bytes).map_err(|_| LedgerError::WrongDatabase)?,
        })
    }
    pub fn work_source(&self, id: &str) -> Result<SourceGeneration> {
        self.connection
            .query_row("SELECT source FROM work WHERE id=?1", [id], |r| {
                r.get::<_, String>(0)
            })
            .optional()?
            .map(SourceGeneration)
            .ok_or(LedgerError::MissingWork)
    }
    /// Transition and its evidence are committed together. No raw SQL escape is
    /// exposed; validated/published states require later independent protocols.
    pub fn transition(
        &mut self,
        id: &str,
        expected: WorkState,
        next: WorkState,
        evidence: &[u8],
    ) -> Result<()> {
        if evidence.is_empty() || evidence.len() > MAX_RECEIPT {
            return Err(LedgerError::InvalidInput);
        }
        if matches!(
            next,
            WorkState::Validated | WorkState::Publishing | WorkState::Complete
        ) {
            return Err(LedgerError::PublicationNotImplemented);
        }
        expected
            .transition(next)
            .map_err(|_| LedgerError::InvalidTransition)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let valid: bool = tx
            .query_row(
                "SELECT s.valid FROM work w JOIN sources s ON s.id=w.source WHERE w.id=?1",
                [id],
                |r| r.get(0),
            )
            .optional()?
            .ok_or(LedgerError::MissingWork)?;
        if !valid {
            return Err(LedgerError::SourceInvalidated);
        }
        if tx.execute(
            "UPDATE work SET state=?1 WHERE id=?2 AND state=?3",
            params![state_name(next), id, state_name(expected)],
        )? != 1
        {
            return Err(LedgerError::Conflict);
        }
        tx.execute(
            "INSERT INTO receipts(work,kind,payload) VALUES(?1,'transition',?2)",
            params![id, evidence],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub fn append_receipt(&mut self, id: &str, kind: &str, payload: &[u8]) -> Result<()> {
        self.append_receipt_batch(&[(id, kind, payload)])
    }
    /// Commit one bounded set of completed observations atomically. A failed
    /// row leaves every receipt in this batch uncommitted.
    pub fn append_receipt_batch(&mut self, rows: &[(&str, &str, &[u8])]) -> Result<()> {
        if rows.is_empty() || rows.len() > 128 {
            return Err(LedgerError::InvalidInput);
        }
        let mut total = 0usize;
        for (id, kind, payload) in rows {
            check_text(id)?;
            check_text(kind)?;
            if payload.is_empty() || payload.len() > MAX_RECEIPT {
                return Err(LedgerError::InvalidInput);
            }
            total = total
                .checked_add(id.len())
                .and_then(|n| n.checked_add(kind.len()))
                .and_then(|n| n.checked_add(payload.len()))
                .ok_or(LedgerError::InvalidInput)?;
            if total > 1024 * 1024 {
                return Err(LedgerError::InvalidInput);
            }
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        for (id, kind, payload) in rows {
            tx.execute(
                "INSERT INTO receipts(work,kind,payload) VALUES(?1,?2,?3)",
                params![id, kind, payload],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn receipt_count(&self, id: &str) -> Result<u64> {
        Ok(self
            .connection
            .query_row("SELECT (SELECT count(*) FROM receipts WHERE work=?1)+(SELECT count(*) FROM source_receipts WHERE source=(SELECT source FROM work WHERE id=?1))", [id], |r| {
                r.get::<_,i64>(0)
            })? as u64)
    }
    /// One immutable manifest per case, stored in this same authority.
    pub fn set_case_manifest(&mut self, payload: &[u8]) -> Result<()> {
        if payload.is_empty() || payload.len() > MAX_RECEIPT {
            return Err(LedgerError::InvalidInput);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch("CREATE TABLE IF NOT EXISTS case_manifest(singleton INTEGER PRIMARY KEY CHECK(singleton=1),payload BLOB NOT NULL); CREATE TRIGGER IF NOT EXISTS manifest_no_update BEFORE UPDATE ON case_manifest BEGIN SELECT RAISE(ABORT,'immutable manifest'); END; CREATE TRIGGER IF NOT EXISTS manifest_no_delete BEFORE DELETE ON case_manifest BEGIN SELECT RAISE(ABORT,'immutable manifest'); END;")?;
        let prior: Option<Vec<u8>> = tx
            .query_row(
                "SELECT payload FROM case_manifest WHERE singleton=1",
                [],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(prior) = prior {
            return if prior == payload {
                Ok(())
            } else {
                Err(LedgerError::Conflict)
            };
        }
        tx.execute("INSERT INTO case_manifest VALUES(1,?1)", [payload])?;
        tx.commit()?;
        Ok(())
    }
    pub fn case_manifest(&self) -> Result<Option<Vec<u8>>> {
        let exists: bool = self.connection.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='case_manifest')", [], |r|r.get(0))?;
        if !exists {
            return Ok(None);
        }
        let row: Option<Vec<u8>> = self
            .connection
            .query_row(
                "SELECT payload FROM case_manifest WHERE singleton=1 AND length(payload)<=65536",
                [],
                |r| r.get(0),
            )
            .optional()?;
        Ok(row)
    }
    /// Bounded immutable index receipts for deterministic restart adoption.
    pub fn indexes(&self, work: &str, maximum: usize) -> Result<Vec<IndexRecord>> {
        if maximum == 0 || maximum > 4096 {
            return Err(LedgerError::InvalidInput);
        }
        let mut stmt = self.connection.prepare(
            "SELECT id,path,digest,bytes FROM index_runs WHERE work=?1 ORDER BY id LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![work, (maximum + 1) as i64], |r| {
            let bytes: i64 = r.get(3)?;
            Ok(IndexRecord {
                id: r.get(0)?,
                path: r.get(1)?,
                digest: r.get(2)?,
                bytes: u64::try_from(bytes).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        3,
                        rusqlite::types::Type::Integer,
                        Box::new(e),
                    )
                })?,
            })
        })?;
        let out = rows.collect::<std::result::Result<Vec<_>, _>>()?;
        if out.len() > maximum
            || out
                .iter()
                .any(|r| r.path.len() > MAX_TEXT || r.id.len() > MAX_TEXT || r.digest.len() != 64)
        {
            return Err(LedgerError::InvalidInput);
        }
        Ok(out)
    }
    pub fn receipts(&self, work: &str, kind: &str, maximum: usize) -> Result<Vec<Vec<u8>>> {
        check_text(kind)?;
        if maximum == 0 || maximum > 4096 {
            return Err(LedgerError::InvalidInput);
        }
        let mut stmt = self.connection.prepare("SELECT payload FROM receipts WHERE work=?1 AND kind=?2 AND length(payload)<=65536 ORDER BY sequence LIMIT ?3")?;
        let rows = stmt.query_map(params![work, kind, (maximum + 1) as i64], |r| r.get(0))?;
        let out = rows.collect::<std::result::Result<Vec<Vec<u8>>, _>>()?;
        if out.len() > maximum {
            return Err(LedgerError::InvalidInput);
        }
        Ok(out)
    }
    /// One unique event per completed accounting batch. Retries adopt exactly the
    /// same batch; conflicting retries fail instead of double-counting or hiding work.
    pub fn account(
        &mut self,
        id: &str,
        event: &str,
        elapsed_millis: u64,
        read_bytes: u64,
    ) -> Result<()> {
        check_text(event)?;
        let elapsed = i64::try_from(elapsed_millis).map_err(|_| LedgerError::InvalidInput)?;
        let reads = i64::try_from(read_bytes).map_err(|_| LedgerError::InvalidInput)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let prior: Option<(i64, i64)> = tx
            .query_row(
                "SELECT elapsed,reads FROM charges WHERE work=?1 AND event=?2",
                params![id, event],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some(p) = prior {
            return if p == (elapsed, reads) {
                Ok(())
            } else {
                Err(LedgerError::Conflict)
            };
        }
        let totals: Option<(i64, i64)> = tx
            .query_row("SELECT elapsed,reads FROM work WHERE id=?1", [id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?;
        let (old_elapsed, old_reads) = totals.ok_or(LedgerError::MissingWork)?;
        let total_elapsed = old_elapsed
            .checked_add(elapsed)
            .ok_or(LedgerError::InvalidInput)?;
        let total_reads = old_reads
            .checked_add(reads)
            .ok_or(LedgerError::InvalidInput)?;
        tx.execute(
            "INSERT INTO charges(work,event,elapsed,reads) VALUES(?1,?2,?3,?4)",
            params![id, event, elapsed, reads],
        )?;
        tx.execute(
            "UPDATE work SET elapsed=?1,reads=?2 WHERE id=?3",
            params![total_elapsed, total_reads, id],
        )?;
        tx.commit()?;
        Ok(())
    }
    /// Registers a case-relative immutable index receipt. Caller must first flush
    /// and hash the file. Registration does not itself certify or read an index.
    pub fn register_index(
        &mut self,
        id: &str,
        index_id: &str,
        path: &str,
        digest: &str,
        bytes: u64,
    ) -> Result<()> {
        check_text(index_id)?;
        check_text(path)?;
        if Path::new(path)
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
            || digest.len() != 64
            || !digest.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(LedgerError::InvalidInput);
        }
        let bytes = i64::try_from(bytes).map_err(|_| LedgerError::InvalidInput)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let valid: bool = tx
            .query_row(
                "SELECT s.valid FROM work w JOIN sources s ON s.id=w.source WHERE w.id=?1",
                [id],
                |r| r.get(0),
            )
            .optional()?
            .ok_or(LedgerError::MissingWork)?;
        if !valid {
            return Err(LedgerError::SourceInvalidated);
        }
        let prior: Option<(String, String, String, i64)> = tx
            .query_row(
                "SELECT work,path,digest,bytes FROM index_runs WHERE id=?1",
                [index_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        if let Some(p) = prior {
            return if p == (id.into(), path.into(), digest.into(), bytes) {
                Ok(())
            } else {
                Err(LedgerError::Conflict)
            };
        }
        tx.execute(
            "INSERT INTO index_runs(id,work,path,digest,bytes) VALUES(?1,?2,?3,?4,?5)",
            params![index_id, id, path, digest, bytes],
        )?;
        tx.commit()?;
        Ok(())
    }
    /// All work depending on this generation becomes unusable. Evidence remains.
    pub fn invalidate_source(&mut self, source: &SourceGeneration, evidence: &[u8]) -> Result<u64> {
        if evidence.is_empty() || evidence.len() > MAX_RECEIPT {
            return Err(LedgerError::InvalidInput);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if tx.execute("UPDATE sources SET valid=0 WHERE id=?1", [&source.0])? != 1 {
            return Err(LedgerError::MissingWork);
        }
        tx.execute(
            "INSERT INTO source_receipts(source,payload) VALUES(?1,?2)",
            params![source.0, evidence],
        )?;
        let count: i64 = tx.query_row(
            "SELECT count(*) FROM work WHERE source=?1",
            [&source.0],
            |r| r.get(0),
        )?;
        tx.commit()?;
        Ok(count as u64)
    }
}
fn state_name(s: WorkState) -> &'static str {
    match s {
        WorkState::Pending => "pending",
        WorkState::Running => "running",
        WorkState::Checkpointed => "checkpointed",
        WorkState::Validated => "validated",
        WorkState::Publishing => "publishing",
        WorkState::Complete => "complete",
        WorkState::Paused => "paused",
        WorkState::Cancelled => "cancelled",
        WorkState::Unresolved => "unresolved",
        WorkState::Failed => "failed",
    }
}
fn parse_state(s: &str) -> Result<WorkState> {
    match s {
        "pending" => Ok(WorkState::Pending),
        "running" => Ok(WorkState::Running),
        "checkpointed" => Ok(WorkState::Checkpointed),
        "validated" => Ok(WorkState::Validated),
        "publishing" => Ok(WorkState::Publishing),
        "complete" => Ok(WorkState::Complete),
        "paused" => Ok(WorkState::Paused),
        "cancelled" => Ok(WorkState::Cancelled),
        "unresolved" => Ok(WorkState::Unresolved),
        "failed" => Ok(WorkState::Failed),
        _ => Err(LedgerError::WrongDatabase),
    }
}
