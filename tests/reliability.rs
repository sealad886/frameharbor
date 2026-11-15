//! Synthetic storage-exhaustion checks. SQLite page quotas produce real
//! SQLITE_FULL responses without exhausting the destination filesystem.
use frameharbor::{
    checkpoint::{CheckpointLimits, CheckpointStore},
    contracts::{ResourceCaps, SourceGeneration, WorkState},
    control::{CaseConfig, CaseControl},
    ledger::{Ledger, LedgerError, WorkSpec},
};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
const QUOTA: u64 = 1024 * 1024;
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "frameharbor-reliability-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn database(&self) -> PathBuf {
        self.0.join("case.sqlite")
    }
    fn ledger(&self) -> Ledger {
        Ledger::open(&self.database(), QUOTA).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn source() -> SourceGeneration {
    SourceGeneration("reliability-synthetic-image".into())
}
fn spec() -> WorkSpec {
    WorkSpec {
        source: source(),
        algorithm: "reliability-v1".into(),
        stage: "intake".into(),
        parameters: BTreeMap::new(),
    }
}
fn caps(n: u64) -> ResourceCaps {
    ResourceCaps {
        read_bytes: n,
        memory_bytes: n,
        index_bytes: n,
        output_bytes: n,
        visited_postings: n,
        transitions: n,
        decoder_seconds: n,
    }
}
fn config() -> CaseConfig {
    CaseConfig {
        total_millis: 10_000,
        shutdown_reserve_millis: 100,
        caps: caps(10_000),
    }
}
fn assert_database_full(error: &rusqlite::Error) {
    assert!(
        matches!(error,rusqlite::Error::SqliteFailure(code,_) if code.code==rusqlite::ErrorCode::DiskFull),
        "expected SQLite page-quota exhaustion, received {error:?}"
    );
}

#[test]
fn sqlite_full_rolls_back_state_and_preserves_prior_frontier_after_reopen() {
    let f = Fixture::new();
    let mut ledger = f.ledger();
    let id = ledger.submit(&spec()).unwrap();
    let mut control = CaseControl::create(ledger, source(), config()).unwrap();
    let mut store =
        CheckpointStore::create(&f.0.join("frontiers"), CheckpointLimits::default()).unwrap();
    let frontier = store
        .publish(b"completed source-backed frontier", &[[7; 32]])
        .unwrap();
    control.link_checkpoint(&store).unwrap();
    let before = control.snapshot().unwrap();
    let mut ledger = control.into_ledger();
    let payload = vec![42; 65536];
    let mut committed = 0;
    // The one-MiB page quota and 64-KiB receipts bound this loop independently
    // of actual available disk space. Retained successful receipts are the oracle.
    let mut exhausted = false;
    for _ in 0..32 {
        match ledger.append_receipt(&id, "completed-observation", &payload) {
            Ok(()) => committed += 1,
            Err(LedgerError::Sql(error)) => {
                assert_database_full(&error);
                exhausted = true;
                break;
            }
            Err(error) => panic!("unexpected failure: {error:?}"),
        }
    }
    assert!(exhausted, "SQLite page cap did not bound receipt growth");
    assert!(committed > 0);
    let error = ledger
        .transition(&id, WorkState::Pending, WorkState::Running, &payload)
        .unwrap_err();
    match error {
        LedgerError::Sql(error) => assert_database_full(&error),
        other => panic!("unexpected transition failure: {other:?}"),
    }
    assert_eq!(ledger.get(&id).unwrap().state, WorkState::Pending);
    assert_eq!(ledger.receipt_count(&id).unwrap(), committed);
    drop(ledger);
    let reopened = f.ledger();
    assert_eq!(reopened.get(&id).unwrap().state, WorkState::Pending);
    assert_eq!(reopened.receipt_count(&id).unwrap(), committed);
    let mut control = CaseControl::adopt(reopened, source()).unwrap();
    let store = CheckpointStore::open(&f.0.join("frontiers"), CheckpointLimits::default()).unwrap();
    control.verify_checkpoint(&store).unwrap();
    assert_eq!(control.snapshot().unwrap(), before);
    assert_eq!(store.load_current().unwrap().unwrap().id, frontier);
    assert_eq!(
        store.load_current().unwrap().unwrap().bytes,
        b"completed source-backed frontier"
    );
    assert!(std::fs::metadata(f.database()).unwrap().len() <= QUOTA);
}

#[test]
fn sqlite_full_mid_admission_rolls_back_time_and_all_resource_fields() {
    let f = Fixture::new();
    let mut control = CaseControl::create(f.ledger(), source(), config()).unwrap();
    control.resume_at(100).unwrap();
    control
        .authorize_at(
            110,
            "intake",
            "completed-batch",
            ResourceCaps {
                read_bytes: 7,
                ..caps(0)
            },
        )
        .unwrap();
    let before = control.snapshot().unwrap();
    // Force the failure after resource-row updates but before the batch receipt
    // commits. Allocation stays bounded by the active ledger connection's quota.
    let injector = rusqlite::Connection::open(f.database()).unwrap();
    injector.execute_batch("CREATE TABLE injected_quota_fill(payload BLOB); CREATE TRIGGER injected_full_admission BEFORE INSERT ON case_batches BEGIN INSERT INTO injected_quota_fill VALUES(zeroblob(2097152)); END;").unwrap();
    let error = control
        .authorize_at(
            120,
            "validation",
            "uncommitted-batch",
            ResourceCaps {
                read_bytes: 10,
                index_bytes: 20,
                output_bytes: 30,
                ..caps(0)
            },
        )
        .unwrap_err();
    match error {
        frameharbor::control::ControlError::Sql(error) => assert_database_full(&error),
        other => panic!("unexpected admission failure: {other:?}"),
    }
    assert_eq!(control.snapshot().unwrap(), before);
    assert_eq!(control.phase_totals("validation").unwrap(), caps(0));
    drop(control);
    drop(injector);
    let control = CaseControl::adopt(f.ledger(), source()).unwrap();
    assert_eq!(control.snapshot().unwrap(), before);
    let inspector = rusqlite::Connection::open(f.database()).unwrap();
    let partial: i64 = inspector
        .query_row("SELECT count(*) FROM injected_quota_fill", [], |r| r.get(0))
        .unwrap();
    assert_eq!(partial, 0);
    assert!(std::fs::metadata(f.database()).unwrap().len() <= QUOTA);
}
