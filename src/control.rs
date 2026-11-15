//! Durable per-image control and budget accounting in the semantic SQLite ledger.
//! `_at` timestamps are milliseconds on one nondecreasing epoch clock, sampled
//! immediately before operations. Backward clock movement fails closed. A crash
//! while Running conservatively counts downtime until the next observation.
//! This module does not stop native children or implement media publication.
use crate::{
    contracts::{Budget, ResourceCaps, SourceGeneration},
    ledger::{Ledger, LedgerError},
};
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};
use std::{fmt, time::Duration};

#[derive(Debug)]
pub enum ControlError {
    Ledger(LedgerError),
    Sql(rusqlite::Error),
    InvalidInput,
    MissingCase,
    Conflict,
    NotRunning,
    Cancelled,
    BudgetExhausted,
    ClockReversed,
    SourceInvalidated,
    CheckpointMismatch,
}
impl fmt::Display for ControlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for ControlError {}
impl From<rusqlite::Error> for ControlError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sql(e)
    }
}
impl From<LedgerError> for ControlError {
    fn from(e: LedgerError) -> Self {
        Self::Ledger(e)
    }
}
type Result<T> = std::result::Result<T, ControlError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseState {
    Paused,
    Running,
    Cancelled,
    BudgetStopped,
}
/// A repeated reservation is evidence that the budget was already charged, not
/// permission to execute I/O again. Callers must execute only `New` admissions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    New,
    AlreadyRecorded,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaseConfig {
    pub total_millis: u64,
    pub shutdown_reserve_millis: u64,
    pub caps: ResourceCaps,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaseSnapshot {
    pub state: CaseState,
    pub elapsed_millis: u64,
    pub used: ResourceCaps,
    pub checkpoint: Option<String>,
}

pub struct CaseControl {
    ledger: Ledger,
    source: SourceGeneration,
    verified_checkpoint: Option<String>,
}
impl CaseControl {
    pub(crate) fn with_ledger<T>(
        &mut self,
        action: impl FnOnce(&mut Ledger) -> std::result::Result<T, LedgerError>,
    ) -> std::result::Result<T, LedgerError> {
        action(&mut self.ledger)
    }
    /// Read-only guard for already-reserved CPU/cache work. It does not grant
    /// new resource consumption; admission/phase settlement remains durable.
    pub fn check_running_at(&self, now: u64) -> Result<()> {
        let (state,valid,checkpoint,elapsed,last,total,reserve):(String,bool,Option<String>,i64,Option<i64>,i64,i64)=self.ledger.connection.query_row("SELECT c.state,s.valid,c.checkpoint,c.elapsed,c.last_clock,c.total,c.reserve FROM case_control c JOIN sources s ON s.id=c.source WHERE c.source=?1",[&self.source.0],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?)))?;
        if total <= 0 || reserve < 0 || reserve >= total || elapsed < 0 {
            return Err(ControlError::InvalidInput);
        }
        if !valid {
            return Err(ControlError::SourceInvalidated);
        }
        if checkpoint != self.verified_checkpoint {
            return Err(ControlError::CheckpointMismatch);
        }
        let state = parse_state(&state)?;
        if state != CaseState::Running {
            return Err(if state == CaseState::Cancelled {
                ControlError::Cancelled
            } else {
                ControlError::NotRunning
            });
        }
        let now = i64::try_from(now).map_err(|_| ControlError::InvalidInput)?;
        let last = last.ok_or(ControlError::InvalidInput)?;
        if now < last {
            return Err(ControlError::ClockReversed);
        }
        if elapsed
            .checked_add(now - last)
            .ok_or(ControlError::InvalidInput)?
            >= total - reserve
        {
            return Err(ControlError::BudgetExhausted);
        }
        Ok(())
    }
    pub fn source(&self) -> &SourceGeneration {
        &self.source
    }
    /// Persist live source-change evidence and invalidate every dependent work
    /// item without consuming the active controller.
    pub fn invalidate_source(&mut self, evidence: &[u8]) -> Result<()> {
        self.ledger.invalidate_source(&self.source, evidence)?;
        Ok(())
    }
    pub fn config(&self) -> Result<CaseConfig> {
        let (total, reserve): (i64, i64) = self.ledger.connection.query_row(
            "SELECT total,reserve FROM case_control WHERE source=?1",
            [&self.source.0],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let mut caps = [0u64; 7];
        for (i, cap) in caps.iter_mut().enumerate() {
            let n: i64 = self.ledger.connection.query_row(
                "SELECT cap FROM case_resources WHERE source=?1 AND resource=?2",
                params![self.source.0, i as i64],
                |r| r.get(0),
            )?;
            *cap = u64::try_from(n).map_err(|_| ControlError::InvalidInput)?;
        }
        Ok(CaseConfig {
            total_millis: total as u64,
            shutdown_reserve_millis: reserve as u64,
            caps: from_resources(caps),
        })
    }
    pub fn create(
        mut ledger: Ledger,
        source: SourceGeneration,
        config: CaseConfig,
    ) -> Result<Self> {
        if source.0.is_empty() || source.0.len() > 4096 {
            return Err(ControlError::InvalidInput);
        }
        Budget::new(
            Duration::from_millis(config.total_millis),
            Duration::from_millis(config.shutdown_reserve_millis),
            config.caps,
        )
        .map_err(|_| ControlError::InvalidInput)?;
        let values = resources(config.caps);
        if values.iter().any(|v| *v > i64::MAX as u64) {
            return Err(ControlError::InvalidInput);
        }
        let tx = ledger
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch("CREATE TABLE IF NOT EXISTS case_control(source TEXT PRIMARY KEY REFERENCES sources(id),total INTEGER NOT NULL,reserve INTEGER NOT NULL,elapsed INTEGER NOT NULL DEFAULT 0,last_clock INTEGER,state TEXT NOT NULL,checkpoint TEXT);
            CREATE TABLE IF NOT EXISTS case_resources(source TEXT NOT NULL REFERENCES case_control(source),resource INTEGER NOT NULL,cap INTEGER NOT NULL,used INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(source,resource));
            CREATE TABLE IF NOT EXISTS case_batches(source TEXT NOT NULL REFERENCES case_control(source),id TEXT NOT NULL,phase TEXT NOT NULL,charges TEXT NOT NULL,PRIMARY KEY(source,id));
            CREATE TABLE IF NOT EXISTS case_phases(source TEXT NOT NULL REFERENCES case_control(source),phase TEXT NOT NULL,resource INTEGER NOT NULL,used INTEGER NOT NULL,PRIMARY KEY(source,phase,resource));
            CREATE TABLE IF NOT EXISTS case_events(id INTEGER PRIMARY KEY,source TEXT NOT NULL REFERENCES case_control(source),kind TEXT NOT NULL,elapsed INTEGER NOT NULL);
            CREATE TRIGGER IF NOT EXISTS case_events_no_update BEFORE UPDATE ON case_events BEGIN SELECT RAISE(ABORT,'append-only control events'); END;
            CREATE TRIGGER IF NOT EXISTS case_events_no_delete BEFORE DELETE ON case_events BEGIN SELECT RAISE(ABORT,'append-only control events'); END;")?;
        tx.execute(
            "INSERT OR IGNORE INTO sources(id,valid) VALUES(?1,1)",
            [&source.0],
        )?;
        require_source(&tx, &source)?;
        if tx
            .query_row(
                "SELECT 1 FROM case_control WHERE source=?1",
                [&source.0],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
            .is_some()
        {
            return Err(ControlError::Conflict);
        }
        tx.execute(
            "INSERT INTO case_control(source,total,reserve,state) VALUES(?1,?2,?3,'paused')",
            params![
                source.0,
                config.total_millis as i64,
                config.shutdown_reserve_millis as i64
            ],
        )?;
        for (i, v) in values.iter().enumerate() {
            tx.execute(
                "INSERT INTO case_resources(source,resource,cap) VALUES(?1,?2,?3)",
                params![source.0, i as i64, *v as i64],
            )?;
        }
        tx.commit()?;
        Ok(Self {
            ledger,
            source,
            verified_checkpoint: None,
        })
    }
    pub fn adopt(ledger: Ledger, source: SourceGeneration) -> Result<Self> {
        let this = Self {
            ledger,
            source,
            verified_checkpoint: None,
        };
        this.snapshot()?;
        Ok(this)
    }
    pub fn into_ledger(self) -> Ledger {
        self.ledger
    }
    pub fn snapshot(&self) -> Result<CaseSnapshot> {
        let (state, elapsed, checkpoint): (String, i64, Option<String>) = self
            .ledger
            .connection
            .query_row(
                "SELECT state,elapsed,checkpoint FROM case_control WHERE source=?1",
                [&self.source.0],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?
            .ok_or(ControlError::MissingCase)?;
        let mut vals = [0u64; 7];
        for (i, v) in vals.iter_mut().enumerate() {
            let n: i64 = self.ledger.connection.query_row(
                "SELECT used FROM case_resources WHERE source=?1 AND resource=?2",
                params![self.source.0, i as i64],
                |r| r.get(0),
            )?;
            *v = u64::try_from(n).map_err(|_| ControlError::InvalidInput)?;
        }
        Ok(CaseSnapshot {
            state: parse_state(&state)?,
            elapsed_millis: u64::try_from(elapsed).map_err(|_| ControlError::InvalidInput)?,
            used: from_resources(vals),
            checkpoint,
        })
    }
    pub fn resume_at(&mut self, now_millis: u64) -> Result<()> {
        self.change_state(now_millis, CaseState::Running)
    }
    pub fn pause_at(&mut self, now_millis: u64) -> Result<()> {
        self.change_state(now_millis, CaseState::Paused)
    }
    pub fn cancel_at(&mut self, now_millis: u64) -> Result<()> {
        self.change_state(now_millis, CaseState::Cancelled)
    }
    fn change_state(&mut self, now: u64, next: CaseState) -> Result<()> {
        if next == CaseState::Running && self.snapshot()?.checkpoint != self.verified_checkpoint {
            return Err(ControlError::CheckpointMismatch);
        }
        let tx = self
            .ledger
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_source(&tx, &self.source)?;
        let (state, elapsed, total, reserve) = observe(&tx, &self.source, now)?;
        if state == CaseState::Cancelled {
            return Err(ControlError::Cancelled);
        }
        if next == CaseState::Running
            && (state == CaseState::BudgetStopped || elapsed >= total - reserve)
        {
            set_state(&tx, &self.source, CaseState::BudgetStopped, now, elapsed)?;
            tx.commit()?;
            return Err(ControlError::BudgetExhausted);
        }
        set_state(&tx, &self.source, next, now, elapsed)?;
        tx.commit()?;
        Ok(())
    }
    /// Authorize one bounded resource batch before work. Identical retries adopt
    /// the charge; elapsed time is still observed afresh on every attempt.
    pub fn authorize_at(
        &mut self,
        now: u64,
        phase: &str,
        batch: &str,
        charge: ResourceCaps,
    ) -> Result<Admission> {
        self.authorize_mode(now, phase, batch, charge, false)
    }
    /// Reserve bounded checkpoint/shutdown work inside the total deadline.
    /// This does not resume normal work. Search and decoder charges are forbidden.
    /// After a budget stop, the gap until shutdown observation is conservatively
    /// counted as active shutdown time, including crash downtime.
    pub fn authorize_shutdown_at(
        &mut self,
        now: u64,
        batch: &str,
        charge: ResourceCaps,
    ) -> Result<Admission> {
        if charge.visited_postings != 0 || charge.transitions != 0 || charge.decoder_seconds != 0 {
            return Err(ControlError::InvalidInput);
        }
        self.authorize_mode(now, "shutdown", batch, charge, true)
    }
    fn authorize_mode(
        &mut self,
        now: u64,
        phase: &str,
        batch: &str,
        charge: ResourceCaps,
        shutdown: bool,
    ) -> Result<Admission> {
        if phase.is_empty() || phase.len() > 128 || batch.is_empty() || batch.len() > 4096 {
            return Err(ControlError::InvalidInput);
        }
        if self.snapshot()?.checkpoint != self.verified_checkpoint {
            return Err(ControlError::CheckpointMismatch);
        }
        let values = resources(charge);
        if values.iter().any(|v| *v > i64::MAX as u64) {
            return Err(ControlError::InvalidInput);
        }
        let encoded = values
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let tx = self
            .ledger
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_source(&tx, &self.source)?;
        let (state, elapsed, total, reserve) = observe_mode(&tx, &self.source, now, shutdown)?;
        if state != CaseState::Running && !(shutdown && state == CaseState::BudgetStopped) {
            return Err(if state == CaseState::Cancelled {
                ControlError::Cancelled
            } else {
                ControlError::NotRunning
            });
        }
        if elapsed >= if shutdown { total } else { total - reserve } {
            set_state(&tx, &self.source, CaseState::BudgetStopped, now, elapsed)?;
            tx.commit()?;
            return Err(ControlError::BudgetExhausted);
        }
        let prior: Option<(String, String)> = tx
            .query_row(
                "SELECT phase,charges FROM case_batches WHERE source=?1 AND id=?2",
                params![self.source.0, batch],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some(p) = prior {
            if p != (phase.into(), encoded) {
                return Err(ControlError::Conflict);
            }
            tx.commit()?;
            return Ok(Admission::AlreadyRecorded);
        }
        for (i, amount) in values.iter().enumerate() {
            let (used, cap): (i64, i64) = tx.query_row(
                "SELECT used,cap FROM case_resources WHERE source=?1 AND resource=?2",
                params![self.source.0, i as i64],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let next = if i == 1 {
                Some(used.max(*amount as i64))
            } else {
                used.checked_add(*amount as i64)
            };
            if next.is_none_or(|n| n > cap) {
                set_state(&tx, &self.source, CaseState::BudgetStopped, now, elapsed)?;
                tx.commit()?;
                return Err(ControlError::BudgetExhausted);
            }
        }
        for (i, amount) in values.iter().enumerate() {
            // Memory is observed peak for synchronous operations. Other resource
            // fields are cumulative charges. Parallel scheduling is unqualified.
            tx.execute(
                "UPDATE case_resources SET used=CASE WHEN resource=1 THEN max(used,?1) ELSE used+?1 END WHERE source=?2 AND resource=?3",
                params![*amount as i64, self.source.0, i as i64],
            )?;
            tx.execute("INSERT INTO case_phases(source,phase,resource,used) VALUES(?1,?2,?3,?4) ON CONFLICT(source,phase,resource) DO UPDATE SET used=CASE WHEN resource=1 THEN max(used,excluded.used) ELSE used+excluded.used END",params![self.source.0,phase,i as i64,*amount as i64])?;
        }
        tx.execute(
            "INSERT INTO case_batches(source,id,phase,charges) VALUES(?1,?2,?3,?4)",
            params![self.source.0, batch, phase, encoded],
        )?;
        tx.commit()?;
        Ok(Admission::New)
    }
    pub fn phase_totals(&self, phase: &str) -> Result<ResourceCaps> {
        let mut values = [0u64; 7];
        for (i, v) in values.iter_mut().enumerate() {
            let n: Option<i64> = self
                .ledger
                .connection
                .query_row(
                    "SELECT used FROM case_phases WHERE source=?1 AND phase=?2 AND resource=?3",
                    params![self.source.0, phase, i as i64],
                    |r| r.get(0),
                )
                .optional()?;
            *v = n.unwrap_or(0) as u64;
        }
        Ok(from_resources(values))
    }
    /// Link only the reopened committed frontier; callers cannot assert a digest.
    pub fn link_checkpoint(&mut self, store: &crate::checkpoint::CheckpointStore) -> Result<()> {
        let frontier = store
            .load_current()
            .map_err(|_| ControlError::CheckpointMismatch)?
            .ok_or(ControlError::CheckpointMismatch)?;
        let id = frontier
            .id
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let (valid, linked): (bool, Option<String>) = self.ledger.connection.query_row(
            "SELECT s.valid,c.checkpoint FROM case_control c JOIN sources s ON s.id=c.source WHERE c.source=?1",
            [&self.source.0], |r| Ok((r.get(0)?,r.get(1)?)))?;
        if !valid {
            return Err(ControlError::SourceInvalidated);
        }
        if linked.as_deref() == Some(id.as_str()) {
            self.verified_checkpoint = Some(id);
            return Ok(());
        }
        let tx = self
            .ledger
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_source(&tx, &self.source)?;
        tx.execute(
            "UPDATE case_control SET checkpoint=?1 WHERE source=?2",
            params![id, self.source.0],
        )?;
        tx.commit()?;
        self.verified_checkpoint = Some(id);
        Ok(())
    }
    pub fn verify_checkpoint(&mut self, store: &crate::checkpoint::CheckpointStore) -> Result<()> {
        let expected = self
            .snapshot()?
            .checkpoint
            .ok_or(ControlError::CheckpointMismatch)?;
        let linked = store
            .history()
            .map_err(|_| ControlError::CheckpointMismatch)?
            .into_iter()
            .chain(
                store
                    .retained_checkpoint()
                    .map_err(|_| ControlError::CheckpointMismatch)?,
            )
            .find(|id| id.iter().map(|b| format!("{b:02x}")).collect::<String>() == expected)
            .ok_or(ControlError::CheckpointMismatch)?;
        store
            .load(linked)
            .map_err(|_| ControlError::CheckpointMismatch)?;
        self.verified_checkpoint = Some(expected);
        Ok(())
    }
}
fn require_source(tx: &Transaction<'_>, source: &SourceGeneration) -> Result<()> {
    let valid: bool = tx.query_row("SELECT valid FROM sources WHERE id=?1", [&source.0], |r| {
        r.get(0)
    })?;
    if valid {
        Ok(())
    } else {
        Err(ControlError::SourceInvalidated)
    }
}
fn observe(
    tx: &Transaction<'_>,
    source: &SourceGeneration,
    now: u64,
) -> Result<(CaseState, u64, u64, u64)> {
    observe_mode(tx, source, now, false)
}
fn observe_mode(
    tx: &Transaction<'_>,
    source: &SourceGeneration,
    now: u64,
    shutdown: bool,
) -> Result<(CaseState, u64, u64, u64)> {
    let now = i64::try_from(now).map_err(|_| ControlError::InvalidInput)?;
    let (state, elapsed, last, total, reserve): (String, i64, Option<i64>, i64, i64) = tx
        .query_row(
            "SELECT state,elapsed,last_clock,total,reserve FROM case_control WHERE source=?1",
            [&source.0],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )?;
    let state = parse_state(&state)?;
    if last.is_some_and(|last| now < last) {
        return Err(ControlError::ClockReversed);
    }
    let added = if state == CaseState::Running || (shutdown && state == CaseState::BudgetStopped) {
        now - last.ok_or(ControlError::InvalidInput)?
    } else {
        0
    };
    let elapsed = elapsed
        .checked_add(added)
        .ok_or(ControlError::InvalidInput)?;
    tx.execute(
        "UPDATE case_control SET elapsed=?1,last_clock=?2 WHERE source=?3",
        params![elapsed, now, source.0],
    )?;
    Ok((state, elapsed as u64, total as u64, reserve as u64))
}
fn set_state(
    tx: &Transaction<'_>,
    source: &SourceGeneration,
    state: CaseState,
    now: u64,
    elapsed: u64,
) -> Result<()> {
    let name = match state {
        CaseState::Paused => "paused",
        CaseState::Running => "running",
        CaseState::Cancelled => "cancelled",
        CaseState::BudgetStopped => "budget_stopped",
    };
    tx.execute(
        "UPDATE case_control SET state=?1,last_clock=?2 WHERE source=?3",
        params![name, now as i64, source.0],
    )?;
    tx.execute(
        "INSERT INTO case_events(source,kind,elapsed) VALUES(?1,?2,?3)",
        params![source.0, name, elapsed as i64],
    )?;
    Ok(())
}
fn parse_state(s: &str) -> Result<CaseState> {
    match s {
        "paused" => Ok(CaseState::Paused),
        "running" => Ok(CaseState::Running),
        "cancelled" => Ok(CaseState::Cancelled),
        "budget_stopped" => Ok(CaseState::BudgetStopped),
        _ => Err(ControlError::InvalidInput),
    }
}
fn resources(r: ResourceCaps) -> [u64; 7] {
    [
        r.read_bytes,
        r.memory_bytes,
        r.index_bytes,
        r.output_bytes,
        r.visited_postings,
        r.transitions,
        r.decoder_seconds,
    ]
}
fn from_resources(r: [u64; 7]) -> ResourceCaps {
    ResourceCaps {
        read_bytes: r[0],
        memory_bytes: r[1],
        index_bytes: r[2],
        output_bytes: r[3],
        visited_postings: r[4],
        transitions: r[5],
        decoder_seconds: r[6],
    }
}
