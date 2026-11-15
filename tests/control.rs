use frameharbor::{
    checkpoint::{CheckpointLimits, CheckpointStore},
    contracts::{ResourceCaps, SourceGeneration},
    control::*,
    ledger::Ledger,
};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "frameharbor-control-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&p).unwrap();
        Self(p)
    }
    fn ledger(&self) -> Ledger {
        Ledger::open(&self.0.join("case.sqlite"), 8 * 1024 * 1024).unwrap()
    }
    fn create(&self) -> CaseControl {
        CaseControl::create(self.ledger(), source(), config()).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn source() -> SourceGeneration {
    SourceGeneration("synthetic-image-1".into())
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
        total_millis: 1000,
        shutdown_reserve_millis: 100,
        caps: caps(100),
    }
}
fn reads(n: u64) -> ResourceCaps {
    ResourceCaps {
        read_bytes: n,
        ..caps(0)
    }
}

#[test]
fn per_image_processing_budget_survives_resume_and_excludes_paused_time() {
    let f = Fixture::new();
    let mut c = f.create();
    c.resume_at(1000).unwrap();
    c.authorize_at(1200, "intake", "batch1", reads(20)).unwrap();
    c.pause_at(1300).unwrap();
    assert_eq!(c.snapshot().unwrap().elapsed_millis, 300);
    drop(c);
    let mut c = CaseControl::adopt(f.ledger(), source()).unwrap();
    c.resume_at(10000).unwrap();
    assert_eq!(c.snapshot().unwrap().elapsed_millis, 300);
    c.authorize_at(10100, "validation", "batch2", reads(30))
        .unwrap();
    assert_eq!(c.snapshot().unwrap().elapsed_millis, 400);
    assert_eq!(c.snapshot().unwrap().used.read_bytes, 50);
    assert_eq!(c.phase_totals("intake").unwrap().read_bytes, 20);
    assert_eq!(c.phase_totals("validation").unwrap().read_bytes, 30);
}
#[test]
fn deadline_stops_new_work_and_durably_retains_partial_totals() {
    let f = Fixture::new();
    let mut c = f.create();
    c.resume_at(0).unwrap();
    c.authorize_at(899, "intake", "done", reads(5)).unwrap();
    assert!(matches!(
        c.authorize_at(900, "intake", "late", reads(1)),
        Err(ControlError::BudgetExhausted)
    ));
    let s = c.snapshot().unwrap();
    assert_eq!(s.state, CaseState::BudgetStopped);
    assert_eq!(s.elapsed_millis, 900);
    assert_eq!(s.used.read_bytes, 5);
    drop(c);
    let mut c = CaseControl::adopt(f.ledger(), source()).unwrap();
    assert!(matches!(
        c.resume_at(2000),
        Err(ControlError::BudgetExhausted)
    ));
    assert_eq!(c.snapshot().unwrap().used.read_bytes, 5);
}
#[test]
fn paused_and_cancelled_cases_cannot_authorize_work() {
    let f = Fixture::new();
    let mut c = f.create();
    assert!(matches!(
        c.authorize_at(10, "intake", "paused", reads(1)),
        Err(ControlError::NotRunning)
    ));
    c.resume_at(10).unwrap();
    c.cancel_at(20).unwrap();
    drop(c);
    let mut c = CaseControl::adopt(f.ledger(), source()).unwrap();
    assert_eq!(c.snapshot().unwrap().state, CaseState::Cancelled);
    assert!(matches!(c.resume_at(100), Err(ControlError::Cancelled)));
    assert!(matches!(
        c.authorize_at(100, "intake", "cancelled", reads(1)),
        Err(ControlError::Cancelled)
    ));
    assert_eq!(c.snapshot().unwrap().elapsed_millis, 10);
}
#[test]
fn resource_batch_is_atomic_and_retry_safe_across_restart() {
    let f = Fixture::new();
    let mut c = f.create();
    c.resume_at(0).unwrap();
    c.authorize_at(10, "intake", "same", reads(70)).unwrap();
    drop(c);
    let mut c = CaseControl::adopt(f.ledger(), source()).unwrap();
    c.authorize_at(20, "intake", "same", reads(70)).unwrap();
    assert_eq!(c.snapshot().unwrap().used.read_bytes, 70);
    assert_eq!(c.snapshot().unwrap().elapsed_millis, 20);
    assert!(matches!(
        c.authorize_at(21, "intake", "same", reads(71)),
        Err(ControlError::Conflict)
    ));
    assert_eq!(c.snapshot().unwrap().used.read_bytes, 70);
    let charge = ResourceCaps {
        read_bytes: 31,
        output_bytes: 1,
        ..caps(0)
    };
    assert!(matches!(
        c.authorize_at(30, "intake", "too-big", charge),
        Err(ControlError::BudgetExhausted)
    ));
    let s = c.snapshot().unwrap();
    assert_eq!(s.used.read_bytes, 70);
    assert_eq!(s.used.output_bytes, 0);
    assert_eq!(s.elapsed_millis, 30);
}
#[test]
fn unpaused_crash_gap_is_conservatively_accounted_and_backward_clock_fails_closed() {
    let f = Fixture::new();
    let mut c = f.create();
    c.resume_at(100).unwrap();
    drop(c);
    let mut c = CaseControl::adopt(f.ledger(), source()).unwrap();
    c.authorize_at(400, "intake", "after-crash", reads(1))
        .unwrap();
    assert_eq!(c.snapshot().unwrap().elapsed_millis, 300);
    assert!(matches!(
        c.authorize_at(399, "intake", "bad-clock", reads(1)),
        Err(ControlError::ClockReversed)
    ));
    assert_eq!(c.snapshot().unwrap().used.read_bytes, 1);
}
#[test]
fn source_invalidation_blocks_resume_and_new_charges() {
    let f = Fixture::new();
    let c = f.create();
    let mut ledger = c.into_ledger();
    ledger
        .invalidate_source(&source(), b"source changed")
        .unwrap();
    let mut c = CaseControl::adopt(ledger, source()).unwrap();
    assert!(matches!(
        c.resume_at(0),
        Err(ControlError::SourceInvalidated)
    ));
    assert_eq!(c.snapshot().unwrap().state, CaseState::Paused);
}
#[test]
fn frontier_adoption_requires_reopened_committed_digest() {
    let f = Fixture::new();
    let mut store =
        CheckpointStore::create(&f.0.join("frontiers"), CheckpointLimits::default()).unwrap();
    store.publish(b"frontier one", &[]).unwrap();
    let mut c = f.create();
    c.link_checkpoint(&store).unwrap();
    drop(c);
    let mut c = CaseControl::adopt(f.ledger(), source()).unwrap();
    assert!(matches!(
        c.resume_at(0),
        Err(ControlError::CheckpointMismatch)
    ));
    c.verify_checkpoint(&store).unwrap();
    c.resume_at(0).unwrap();
    c.pause_at(20).unwrap();
    drop(c);
    store.publish(b"frontier two", &[]).unwrap();
    let mut c = CaseControl::adopt(f.ledger(), source()).unwrap();
    let linked = c.snapshot().unwrap().checkpoint;
    c.verify_checkpoint(&store).unwrap();
    assert_eq!(c.snapshot().unwrap().checkpoint, linked);
    c.resume_at(30).unwrap();
    c.pause_at(40).unwrap();
    drop(c);
    store.publish(b"frontier three", &[]).unwrap();
    store.publish(b"frontier four", &[]).unwrap();
    let mut c = CaseControl::adopt(f.ledger(), source()).unwrap();
    assert!(matches!(
        c.verify_checkpoint(&store),
        Err(ControlError::CheckpointMismatch)
    ));
    assert!(matches!(
        c.resume_at(50),
        Err(ControlError::CheckpointMismatch)
    ));
}

#[test]
fn repeated_admission_is_not_new_execution_and_memory_is_observed_peak() {
    let f = Fixture::new();
    let mut c = f.create();
    c.resume_at(0).unwrap();
    let charge = ResourceCaps {
        read_bytes: 20,
        memory_bytes: 80,
        ..caps(0)
    };
    assert_eq!(
        c.authorize_at(1, "intake", "first", charge).unwrap(),
        Admission::New
    );
    assert_eq!(
        c.authorize_at(2, "intake", "first", charge).unwrap(),
        Admission::AlreadyRecorded
    );
    assert_eq!(
        c.authorize_at(3, "intake", "second", charge).unwrap(),
        Admission::New
    );
    assert_eq!(c.snapshot().unwrap().used.memory_bytes, 80);
    assert_eq!(c.phase_totals("intake").unwrap().memory_bytes, 80);
    assert_eq!(c.snapshot().unwrap().used.read_bytes, 40);
    assert_eq!(c.config().unwrap(), config());
    assert_eq!(c.source(), &source());
}

#[test]
fn shutdown_uses_reserved_time_without_reopening_normal_work() {
    let f = Fixture::new();
    let mut c = f.create();
    c.resume_at(0).unwrap();
    assert!(matches!(
        c.authorize_at(900, "intake", "stop", reads(1)),
        Err(ControlError::BudgetExhausted)
    ));
    let charge = ResourceCaps {
        index_bytes: 5,
        ..caps(0)
    };
    assert_eq!(
        c.authorize_shutdown_at(950, "frontier", charge).unwrap(),
        Admission::New
    );
    let s = c.snapshot().unwrap();
    assert_eq!(s.elapsed_millis, 950);
    assert_eq!(s.state, CaseState::BudgetStopped);
    assert_eq!(s.used.index_bytes, 5);
    assert!(
        c.authorize_at(960, "intake", "still-stopped", reads(1))
            .is_err()
    );
    assert!(matches!(
        c.authorize_shutdown_at(
            960,
            "search",
            ResourceCaps {
                transitions: 1,
                ..caps(0)
            }
        ),
        Err(ControlError::InvalidInput)
    ));
    assert!(matches!(
        c.authorize_shutdown_at(1000, "too-late", charge),
        Err(ControlError::BudgetExhausted)
    ));
    assert_eq!(c.snapshot().unwrap().elapsed_millis, 1000);
    assert_eq!(c.snapshot().unwrap().used.index_bytes, 5);
}

#[test]
fn live_controller_invalidation_is_durable_and_retains_failure_evidence() {
    let f = Fixture::new();
    let mut c = f.create();
    c.resume_at(0).unwrap();
    c.authorize_at(1, "intake", "before", reads(1)).unwrap();
    c.invalidate_source(b"consumed range digest changed")
        .unwrap();
    assert!(matches!(
        c.authorize_at(2, "intake", "after", reads(1)),
        Err(ControlError::SourceInvalidated)
    ));
    assert!(matches!(
        c.resume_at(2),
        Err(ControlError::SourceInvalidated)
    ));
    assert_eq!(c.snapshot().unwrap().used.read_bytes, 1);
    drop(c);
    let mut c = CaseControl::adopt(f.ledger(), source()).unwrap();
    assert!(matches!(
        c.resume_at(3),
        Err(ControlError::SourceInvalidated)
    ));
    drop(c);
    let connection = rusqlite::Connection::open(f.0.join("case.sqlite")).unwrap();
    let evidence: Vec<u8> = connection
        .query_row(
            "SELECT payload FROM source_receipts WHERE source=?1",
            [&source().0],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(evidence, b"consumed range digest changed");
    assert!(
        connection
            .execute("DELETE FROM source_receipts", [])
            .is_err()
    );
}
#[test]
fn failed_control_event_write_rolls_back_state_and_elapsed() {
    let f = Fixture::new();
    let mut c = f.create();
    c.resume_at(100).unwrap();
    let conn = rusqlite::Connection::open(f.0.join("case.sqlite")).unwrap();
    conn.execute_batch("CREATE TRIGGER injected_control_failure BEFORE INSERT ON case_events BEGIN SELECT RAISE(ABORT,'injected failure'); END;").unwrap();
    assert!(c.pause_at(200).is_err());
    let s = c.snapshot().unwrap();
    assert_eq!(s.state, CaseState::Running);
    assert_eq!(s.elapsed_millis, 0);
    drop(conn);
    drop(c);
    let mut c = CaseControl::adopt(f.ledger(), source()).unwrap();
    c.authorize_at(300, "intake", "next", reads(1)).unwrap();
    assert_eq!(c.snapshot().unwrap().elapsed_millis, 200);
}

#[test]
fn relinking_an_identical_verified_checkpoint_is_readonly_but_checks_source_validity() {
    let fixture = Fixture::new();
    let mut c = fixture.create();
    let mut store =
        CheckpointStore::create(&fixture.0.join("frontiers"), CheckpointLimits::default()).unwrap();
    store.publish(b"stable-work", &[]).unwrap();
    c.link_checkpoint(&store).unwrap();
    let before = std::fs::read(fixture.0.join("case.sqlite")).unwrap();
    c.link_checkpoint(&store).unwrap();
    assert_eq!(
        std::fs::read(fixture.0.join("case.sqlite")).unwrap(),
        before
    );
    c.invalidate_source(b"changed source").unwrap();
    assert!(matches!(
        c.link_checkpoint(&store),
        Err(ControlError::SourceInvalidated)
    ));
}
