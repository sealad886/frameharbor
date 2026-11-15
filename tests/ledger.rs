use frameharbor::{
    contracts::{SourceGeneration, WorkState},
    ledger::{Ledger, LedgerError, WorkSpec},
};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Case(PathBuf);
impl Case {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "frameharbor-ledger-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&p).unwrap();
        Self(p)
    }
    fn db(&self) -> PathBuf {
        self.0.join("case.sqlite")
    }
}
impl Drop for Case {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn spec() -> WorkSpec {
    WorkSpec {
        source: SourceGeneration("synthetic-generation-1".into()),
        algorithm: "intake-v1".into(),
        stage: "intake".into(),
        parameters: BTreeMap::from([
            ("domain".into(), "0:1024".into()),
            ("profile".into(), "partition".into()),
        ]),
    }
}
fn open(c: &Case) -> Ledger {
    Ledger::open(&c.db(), 4 * 1024 * 1024).unwrap()
}

#[test]
fn semantic_ids_are_canonical_domain_separated_and_source_sensitive() {
    let a = spec();
    let mut b = a.clone();
    b.parameters = BTreeMap::from([
        ("profile".into(), "partition".into()),
        ("domain".into(), "0:1024".into()),
    ]);
    assert_eq!(a.id().unwrap(), b.id().unwrap());
    b.source.0 = "synthetic-generation-2".into();
    assert_ne!(a.id().unwrap(), b.id().unwrap());
    b = a.clone();
    b.algorithm.push('2');
    assert_ne!(a.id().unwrap(), b.id().unwrap());
    b = a.clone();
    b.parameters.insert("domain".into(), "0:1025".into());
    assert_ne!(a.id().unwrap(), b.id().unwrap());
    let mut x = spec();
    let mut y = spec();
    x.algorithm = "ab".into();
    x.stage = "c".into();
    y.algorithm = "a".into();
    y.stage = "bc".into();
    assert_ne!(x.id().unwrap(), y.id().unwrap());
}
#[test]
fn duplicate_submission_and_noop_reopen_adopt_completed_semantic_work() {
    let c = Case::new();
    let id;
    {
        let mut l = open(&c);
        id = l.submit(&spec()).unwrap();
        assert_eq!(l.submit(&spec()).unwrap(), id);
        l.transition(&id, WorkState::Pending, WorkState::Running, b"scheduled")
            .unwrap();
        l.transition(
            &id,
            WorkState::Running,
            WorkState::Checkpointed,
            b"frontier retained",
        )
        .unwrap();
        l.account(&id, "batch1", 120, 1000).unwrap();
    }
    let mut l = open(&c);
    assert_eq!(l.submit(&spec()).unwrap(), id);
    let r = l.get(&id).unwrap();
    assert_eq!(r.state, WorkState::Checkpointed);
    assert_eq!((r.elapsed_millis, r.read_bytes), (120, 1000));
    assert_eq!(l.receipt_count(&id).unwrap(), 2);
}
#[test]
fn transition_preconditions_and_evidence_commit_atomically() {
    let c = Case::new();
    let mut l = open(&c);
    let id = l.submit(&spec()).unwrap();
    assert!(
        l.transition(
            &id,
            WorkState::Running,
            WorkState::Paused,
            b"wrong expected state"
        )
        .is_err()
    );
    assert_eq!(l.get(&id).unwrap().state, WorkState::Pending);
    assert_eq!(l.receipt_count(&id).unwrap(), 0);
    assert!(
        l.transition(&id, WorkState::Pending, WorkState::Running, b"")
            .is_err()
    );
    assert_eq!(l.get(&id).unwrap().state, WorkState::Pending);
    l.transition(&id, WorkState::Pending, WorkState::Running, b"scheduled")
        .unwrap();
    for state in [
        WorkState::Validated,
        WorkState::Publishing,
        WorkState::Complete,
    ] {
        assert!(matches!(
            l.transition(
                &id,
                WorkState::Running,
                state,
                b"not independent validation"
            ),
            Err(LedgerError::PublicationNotImplemented)
        ));
    }
    assert_eq!(l.get(&id).unwrap().state, WorkState::Running);
    assert_eq!(l.receipt_count(&id).unwrap(), 1);
}
#[test]
fn accounting_retries_do_not_duplicate_or_disguise_consumed_work() {
    let c = Case::new();
    let mut l = open(&c);
    let id = l.submit(&spec()).unwrap();
    l.account(&id, "one", 20, 50).unwrap();
    l.account(&id, "one", 20, 50).unwrap();
    assert!(matches!(
        l.account(&id, "one", 21, 50),
        Err(LedgerError::Conflict)
    ));
    l.account(&id, "two", 30, 70).unwrap();
    assert_eq!(
        (
            l.get(&id).unwrap().elapsed_millis,
            l.get(&id).unwrap().read_bytes
        ),
        (50, 120)
    );
    assert!(l.account(&id, "overflow", u64::MAX, 0).is_err());
    assert_eq!(l.get(&id).unwrap().elapsed_millis, 50);
}
#[test]
fn source_invalidation_blocks_adoption_and_preserves_evidence() {
    let c = Case::new();
    let mut l = open(&c);
    let id = l.submit(&spec()).unwrap();
    l.append_receipt(&id, "observation", b"original receipt")
        .unwrap();
    assert_eq!(
        l.invalidate_source(&spec().source, b"consumed range hash changed")
            .unwrap(),
        1
    );
    assert!(!l.get(&id).unwrap().source_valid);
    assert_eq!(l.receipt_count(&id).unwrap(), 2);
    assert!(matches!(
        l.submit(&spec()),
        Err(LedgerError::SourceInvalidated)
    ));
    assert!(matches!(
        l.transition(&id, WorkState::Pending, WorkState::Running, b"resume"),
        Err(LedgerError::SourceInvalidated)
    ));
    let mut replacement = spec();
    replacement.source.0.push_str("-new");
    let fresh = l.submit(&replacement).unwrap();
    assert_ne!(fresh, id);
    assert!(l.get(&fresh).unwrap().source_valid);
}
#[test]
fn immutable_index_identity_rejects_overwrite_and_escaping_paths() {
    let c = Case::new();
    let mut l = open(&c);
    let id = l.submit(&spec()).unwrap();
    let digest = "0".repeat(64);
    l.register_index(&id, "run1", "indexes/one.bin", &digest, 12)
        .unwrap();
    l.register_index(&id, "run1", "indexes/one.bin", &digest, 12)
        .unwrap();
    assert!(matches!(
        l.register_index(&id, "run1", "indexes/one.bin", &digest, 13),
        Err(LedgerError::Conflict)
    ));
    for path in [
        "../source.img",
        "/tmp/source.img",
        "indexes/../../source.img",
    ] {
        assert!(l.register_index(&id, "bad", path, &digest, 12).is_err());
    }
    l.invalidate_source(&spec().source, b"changed").unwrap();
    assert!(
        l.register_index(&id, "new", "indexes/new.bin", &digest, 12)
            .is_err()
    );
}
#[test]
fn append_only_receipts_and_indexes_survive_direct_sql_mutation_attempts() {
    let c = Case::new();
    let mut l = open(&c);
    let id = l.submit(&spec()).unwrap();
    l.append_receipt(&id, "original", b"keep failure evidence")
        .unwrap();
    l.register_index(&id, "one", "indexes/one", &"0".repeat(64), 1)
        .unwrap();
    drop(l);
    let conn = rusqlite::Connection::open(c.db()).unwrap();
    assert!(conn.execute("DELETE FROM receipts", []).is_err());
    assert!(
        conn.execute("UPDATE receipts SET payload=x'00'", [])
            .is_err()
    );
    assert!(conn.execute("DELETE FROM index_runs", []).is_err());
    drop(conn);
    assert_eq!(open(&c).receipt_count(&id).unwrap(), 1);
}
#[test]
fn bounded_inputs_and_wrong_database_are_rejected() {
    let c = Case::new();
    let conn = rusqlite::Connection::open(c.db()).unwrap();
    conn.execute("CREATE TABLE unrelated(v)", []).unwrap();
    drop(conn);
    assert!(matches!(
        Ledger::open(&c.db(), 4 * 1024 * 1024),
        Err(LedgerError::WrongDatabase)
    ));
    let c = Case::new();
    let mut l = open(&c);
    let id = l.submit(&spec()).unwrap();
    assert!(l.append_receipt(&id, "too-big", &vec![0; 65537]).is_err());
    let mut s = spec();
    s.algorithm = "x".repeat(4097);
    assert!(s.id().is_err());
    assert!(Ledger::open(&c.0.join("small.sqlite"), 100).is_err());
}

#[test]
fn failed_receipt_write_rolls_back_transition_and_reopen_retains_prior_state() {
    let c = Case::new();
    let id;
    {
        let mut l = open(&c);
        id = l.submit(&spec()).unwrap();
        let injector = rusqlite::Connection::open(c.db()).unwrap();
        injector.execute_batch("CREATE TRIGGER injected_receipt_failure BEFORE INSERT ON receipts BEGIN SELECT RAISE(ABORT,'injected storage failure'); END;").unwrap();
        assert!(
            l.transition(&id, WorkState::Pending, WorkState::Running, b"scheduled")
                .is_err()
        );
        assert_eq!(l.get(&id).unwrap().state, WorkState::Pending);
        assert_eq!(l.receipt_count(&id).unwrap(), 0);
    }
    assert_eq!(open(&c).get(&id).unwrap().state, WorkState::Pending);
}

#[test]
fn dropped_uncommitted_sql_transaction_is_not_adopted_on_reopen() {
    let c = Case::new();
    let id;
    {
        let mut l = open(&c);
        id = l.submit(&spec()).unwrap();
    }
    {
        let conn = rusqlite::Connection::open(c.db()).unwrap();
        conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        conn.execute(
            "UPDATE work SET state='running',elapsed=500 WHERE id=?1",
            [&id],
        )
        .unwrap();
    }
    let l = open(&c);
    let r = l.get(&id).unwrap();
    assert_eq!(r.state, WorkState::Pending);
    assert_eq!(r.elapsed_millis, 0);
}

#[test]
fn bounded_receipt_batch_is_atomic_and_adoptable_after_reopen() {
    let c = Case::new();
    let mut ledger = open(&c);
    let first = ledger.submit(&spec()).unwrap();
    let mut second = spec();
    second
        .parameters
        .insert("domain".into(), "1024:2048".into());
    let second = ledger.submit(&second).unwrap();
    let baseline = ledger.receipt_count(&first).unwrap();
    assert!(
        ledger
            .append_receipt_batch(&[
                (&first, "probe", b"first"),
                ("missing-work", "probe", b"second")
            ])
            .is_err()
    );
    assert_eq!(ledger.receipt_count(&first).unwrap(), baseline);
    ledger
        .append_receipt_batch(&[(&first, "probe", b"first"), (&second, "probe", b"second")])
        .unwrap();
    assert!(
        ledger
            .append_receipt_batch(&vec![(&first[..], "probe", &b"x"[..]); 129])
            .is_err()
    );
    drop(ledger);
    let reopened = open(&c);
    assert_eq!(
        reopened.receipts(&first, "probe", 1).unwrap(),
        vec![b"first".to_vec()]
    );
    assert_eq!(
        reopened.receipts(&second, "probe", 1).unwrap(),
        vec![b"second".to_vec()]
    );
}
