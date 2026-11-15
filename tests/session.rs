use frameharbor::acquisition::ReadLimits;
use frameharbor::checkpoint::{CheckpointLimits, CheckpointStore};
use frameharbor::contracts::*;
use frameharbor::control::{CaseConfig, CaseState};
use frameharbor::ledger::Ledger;
use frameharbor::session::*;
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "frameharbor-session-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("image"), b"abcdefghijklmnop").unwrap();
        fs::write(root.join("map"), b"0 + 1\n0 16 +\n").unwrap();
        Self { root }
    }
    fn input(&self) -> SessionInput {
        SessionInput {
            image: self.root.join("image"),
            map: self.root.join("map"),
            binding: CoordinateBinding::new(
                ByteRange::new(0, 16).unwrap(),
                0,
                16,
                BindingEvidence::OperatorAttested,
            )
            .unwrap(),
            limits: ReadLimits {
                per_read: 16,
                total: 1024,
                receipts: 1024,
            },
        }
    }
    fn ledger(&self) -> Ledger {
        Ledger::open(&self.root.join("case.sqlite"), 4 * 1024 * 1024).unwrap()
    }
    fn store(&self) -> CheckpointStore {
        CheckpointStore::create(&self.root.join("frontiers"), CheckpointLimits::default()).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn config(read_bytes: u64) -> CaseConfig {
    CaseConfig {
        total_millis: 60000,
        shutdown_reserve_millis: 1000,
        caps: ResourceCaps {
            read_bytes,
            memory_bytes: 1024 * 1024,
            index_bytes: 1024 * 1024,
            output_bytes: 1024 * 1024,
            visited_postings: 0,
            transitions: 0,
            decoder_seconds: 0,
        },
    }
}
fn range(a: u64, b: u64) -> ByteRange {
    ByteRange::new(a, b).unwrap()
}

#[test]
fn pause_resume_revalidates_consumed_spans_and_continues_without_replaying_work() {
    let fixture = Fixture::new();
    let mut store = fixture.store();
    let mut session = CaseSession::create(fixture.ledger(), fixture.input(), config(1024)).unwrap();
    let generation = session.source_generation();
    let first = session
        .with_read(range(0, 8), |bytes| bytes.to_vec())
        .unwrap();
    session.pause(&mut store).unwrap();
    assert_eq!(session.snapshot().unwrap().state, CaseState::Paused);
    let before = session.snapshot().unwrap().used.read_bytes;
    drop(session);
    let mut resumed =
        CaseSession::resume(fixture.ledger(), fixture.input(), generation, &store).unwrap();
    assert_eq!(resumed.image_bytes_read_this_run(), 8);
    assert_eq!(
        resumed.snapshot().unwrap().used.read_bytes,
        before + fs::metadata(fixture.root.join("map")).unwrap().len() + 8
    );
    let second = resumed
        .with_read(range(8, 16), |bytes| bytes.to_vec())
        .unwrap();
    assert_eq!(
        [first, second].concat(),
        fs::read(fixture.root.join("image")).unwrap()
    );
    resumed.checkpoint(&mut store).unwrap();
    // A regular periodic checkpoint does not consume the stop reserve/state.
    assert_eq!(resumed.snapshot().unwrap().state, CaseState::Running);
}

#[test]
fn failed_ledger_checkpoint_link_preserves_resume_and_settled_history_limits() {
    for historical_manifests in [0, 2] {
        let fixture = Fixture::new();
        let limits = CheckpointLimits {
            historical_manifests,
            ..CheckpointLimits::default()
        };
        let root = fixture.root.join("frontiers");
        let mut store = CheckpointStore::create(&root, limits).unwrap();
        let mut session =
            CaseSession::create(fixture.ledger(), fixture.input(), config(4096)).unwrap();
        let generation = session.source_generation();
        session.with_read(range(0, 4), |_| ()).unwrap();
        let linked = session.checkpoint(&mut store).unwrap();
        let database = rusqlite::Connection::open(fixture.root.join("case.sqlite")).unwrap();
        database
            .execute_batch(
                "CREATE TRIGGER fail_checkpoint_link BEFORE UPDATE OF checkpoint ON case_control
                 BEGIN SELECT RAISE(FAIL,'injected checkpoint link failure'); END;",
            )
            .unwrap();
        session.with_read(range(4, 8), |_| ()).unwrap();
        assert!(session.checkpoint(&mut store).is_err());
        assert_ne!(store.load_current().unwrap().unwrap().id, linked);
        let before = session.snapshot().unwrap();
        // A retry and ordinary reconciliation must not discard SQLite's frontier.
        assert!(session.checkpoint(&mut store).is_err());
        store.reconcile().unwrap();
        assert!(store.load(linked).is_ok());
        assert_eq!(session.snapshot().unwrap().checkpoint, before.checkpoint);
        let charged = session.snapshot().unwrap().used;
        drop(session);
        drop(store);
        database
            .execute_batch("DROP TRIGGER fail_checkpoint_link")
            .unwrap();
        drop(database);

        let mut store = CheckpointStore::open(&root, limits).unwrap();
        let mut resumed =
            CaseSession::resume(fixture.ledger(), fixture.input(), generation, &store).unwrap();
        assert_eq!(resumed.snapshot().unwrap().checkpoint, before.checkpoint);
        assert_eq!(resumed.image_bytes_read_this_run(), 4);
        assert!(resumed.snapshot().unwrap().used.read_bytes > charged.read_bytes);
        assert!(resumed.snapshot().unwrap().used.index_bytes >= charged.index_bytes);
        resumed.with_read(range(8, 12), |_| ()).unwrap();
        resumed.checkpoint(&mut store).unwrap();
        assert!(!root.join("RETAINED").exists());
        assert!(store.history().unwrap().len() <= historical_manifests + 1);
        let manifests = fs::read_dir(&root)
            .unwrap()
            .filter(|entry| {
                entry
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("manifest-")
            })
            .count();
        assert_eq!(manifests, store.history().unwrap().len());
    }
}

#[test]
fn corrupt_retained_checkpoint_blocks_resume_without_discarding_evidence() {
    let fixture = Fixture::new();
    let limits = CheckpointLimits {
        historical_manifests: 0,
        ..CheckpointLimits::default()
    };
    let root = fixture.root.join("frontiers");
    let mut store = CheckpointStore::create(&root, limits).unwrap();
    let mut session = CaseSession::create(fixture.ledger(), fixture.input(), config(4096)).unwrap();
    session.with_read(range(0, 4), |_| ()).unwrap();
    session.checkpoint(&mut store).unwrap();
    let database = rusqlite::Connection::open(fixture.root.join("case.sqlite")).unwrap();
    database
        .execute_batch(
            "CREATE TRIGGER fail_checkpoint_link BEFORE UPDATE OF checkpoint ON case_control
             BEGIN SELECT RAISE(FAIL,'injected checkpoint link failure'); END;",
        )
        .unwrap();
    session.with_read(range(4, 8), |_| ()).unwrap();
    assert!(session.checkpoint(&mut store).is_err());
    drop(session);
    drop(store);
    let retained = root.join("RETAINED");
    fs::write(&retained, b"corrupt retained checkpoint").unwrap();
    assert!(CheckpointStore::open(&root, limits).is_err());
    assert_eq!(fs::read(retained).unwrap(), b"corrupt retained checkpoint");
}

#[test]
fn durable_global_read_cap_blocks_io_after_reopen() {
    let fixture = Fixture::new();
    let mut store = fixture.store();
    let metadata = fs::metadata(fixture.root.join("map")).unwrap().len();
    let mut session =
        CaseSession::create(fixture.ledger(), fixture.input(), config(metadata * 2 + 8)).unwrap();
    let generation = session.source_generation();
    session.with_read(range(0, 4), |_| ()).unwrap();
    session.pause(&mut store).unwrap();
    drop(session);
    let mut resumed =
        CaseSession::resume(fixture.ledger(), fixture.input(), generation, &store).unwrap();
    assert!(
        resumed
            .with_read(range(4, 5), |_| panic!("quota must block callback"))
            .is_err()
    );
    assert_eq!(resumed.image_bytes_read_this_run(), 4);
    assert_eq!(resumed.snapshot().unwrap().state, CaseState::BudgetStopped);
    // Reserve still permits durable frontier persistence after normal work stops.
    resumed.checkpoint(&mut store).unwrap();
}

#[test]
fn source_change_prevents_adoption_and_preserves_checkpoint() {
    let fixture = Fixture::new();
    let mut store = fixture.store();
    let mut session = CaseSession::create(fixture.ledger(), fixture.input(), config(1024)).unwrap();
    let generation = session.source_generation();
    session.with_read(range(0, 8), |_| ()).unwrap();
    session.pause(&mut store).unwrap();
    drop(session);
    let frontier = store.load_current().unwrap().unwrap().id;
    fs::write(fixture.root.join("image"), b"ponmlkjihgfedcba").unwrap();
    assert!(matches!(
        CaseSession::resume(fixture.ledger(), fixture.input(), generation, &store),
        Err(SessionError::SourceChanged)
    ));
    assert_eq!(store.load_current().unwrap().unwrap().id, frontier);
}

#[test]
fn cancellation_is_durable_and_stale_map_bytes_never_reach_callback() {
    let fixture = Fixture::new();
    fs::write(fixture.root.join("map"), b"0 + 1\n0 8 +\n8 8 -\n").unwrap();
    let mut store = fixture.store();
    let mut session = CaseSession::create(fixture.ledger(), fixture.input(), config(1024)).unwrap();
    let generation = session.source_generation();
    assert!(
        session
            .with_read(range(8, 16), |_| panic!(
                "unavailable acquisition must never reach callback"
            ))
            .is_err()
    );
    assert_eq!(session.image_bytes_read_this_run(), 0);
    session.cancel(&mut store).unwrap();
    drop(session);
    assert!(CaseSession::resume(fixture.ledger(), fixture.input(), generation, &store).is_err());
    assert_eq!(
        fs::read(fixture.root.join("image")).unwrap(),
        b"abcdefghijklmnop"
    );
}

#[test]
fn crash_before_first_frontier_can_resume_without_adopting_uncheckpointed_work() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let mut session = CaseSession::create(fixture.ledger(), fixture.input(), config(1024)).unwrap();
    let generation = session.source_generation();
    session.with_read(range(0, 4), |_| ()).unwrap();
    let prior = session.snapshot().unwrap().used.read_bytes;
    drop(session);
    let mut resumed =
        CaseSession::resume(fixture.ledger(), fixture.input(), generation, &store).unwrap();
    assert_eq!(resumed.image_bytes_read_this_run(), 0);
    assert_eq!(
        resumed.snapshot().unwrap().used.read_bytes,
        prior + fs::metadata(fixture.root.join("map")).unwrap().len()
    );
    assert_eq!(
        resumed
            .with_read(range(0, 4), |bytes| bytes.to_vec())
            .unwrap(),
        b"abcd"
    );
}

#[test]
fn live_source_change_immediately_invalidates_durable_dependents() {
    let fixture = Fixture::new();
    let mut store = fixture.store();
    let mut session = CaseSession::create(fixture.ledger(), fixture.input(), config(1024)).unwrap();
    let generation = session.source_generation();
    session.with_read(range(0, 4), |_| ()).unwrap();
    session.checkpoint(&mut store).unwrap();
    fs::write(fixture.root.join("image"), b"ponmlkjihgfedcba").unwrap();
    assert!(matches!(
        session.with_read(range(4, 8), |_| panic!(
            "changed source must not reach callback"
        )),
        Err(SessionError::SourceChanged)
    ));
    assert!(matches!(
        session.with_read(range(4, 8), |_| ()),
        Err(SessionError::Control(
            frameharbor::control::ControlError::SourceInvalidated
        ))
    ));
    drop(session);
    assert!(matches!(
        CaseSession::resume(fixture.ledger(), fixture.input(), generation, &store),
        Err(SessionError::Control(
            frameharbor::control::ControlError::SourceInvalidated
        ))
    ));
}

#[test]
fn managed_memory_admission_includes_metadata_and_payload_before_io() {
    let fixture = Fixture::new();
    fs::write(fixture.root.join("image"), vec![3; 1024]).unwrap();
    fs::write(fixture.root.join("map"), b"0 + 1\n0 1024 +\n").unwrap();
    let mut input = fixture.input();
    input.binding =
        CoordinateBinding::new(range(0, 1024), 0, 1024, BindingEvidence::OperatorAttested).unwrap();
    input.limits.per_read = 1024;
    let mut limits = config(2048);
    limits.caps.memory_bytes = 66000;
    let mut session = CaseSession::create(fixture.ledger(), input, limits).unwrap();
    let initial = session.snapshot().unwrap().used.memory_bytes;
    assert!(initial > 65536 && initial <= 66000);
    assert!(
        session
            .with_read(range(0, 1024), |_| panic!(
                "memory admission must precede payload IO"
            ))
            .is_err()
    );
    assert_eq!(session.image_bytes_read_this_run(), 0);
    assert_eq!(session.snapshot().unwrap().used.memory_bytes, initial);
}

#[test]
fn resumed_and_uninterrupted_work_produce_identical_semantic_frontier() {
    let fixture = Fixture::new();
    let mut resumed_store = fixture.store();
    let mut first = CaseSession::create(fixture.ledger(), fixture.input(), config(4096)).unwrap();
    let generation = first.source_generation();
    first.with_read(range(0, 8), |_| ()).unwrap();
    first.pause(&mut resumed_store).unwrap();
    drop(first);
    let mut resumed = CaseSession::resume(
        fixture.ledger(),
        fixture.input(),
        generation,
        &resumed_store,
    )
    .unwrap();
    resumed.with_read(range(8, 16), |_| ()).unwrap();
    let resumed_id = resumed.checkpoint(&mut resumed_store).unwrap();
    let ledger = Ledger::open(&fixture.root.join("uninterrupted.sqlite"), 4 * 1024 * 1024).unwrap();
    let mut uninterrupted = CaseSession::create(ledger, fixture.input(), config(4096)).unwrap();
    uninterrupted.with_read(range(0, 8), |_| ()).unwrap();
    uninterrupted.with_read(range(8, 16), |_| ()).unwrap();
    let mut store = CheckpointStore::create(
        &fixture.root.join("uninterrupted-frontiers"),
        CheckpointLimits::default(),
    )
    .unwrap();
    assert_eq!(uninterrupted.checkpoint(&mut store).unwrap(), resumed_id);
    assert_eq!(
        store.load_current().unwrap().unwrap(),
        resumed_store.load_current().unwrap().unwrap()
    );
}

#[test]
fn pause_and_cancel_stop_new_reads_even_when_checkpoint_storage_fails() {
    for cancel in [false, true] {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        let mut session =
            CaseSession::create(fixture.ledger(), fixture.input(), config(1024)).unwrap();
        session.with_read(range(0, 4), |_| ()).unwrap();
        session.checkpoint(&mut store).unwrap();
        fs::write(fixture.root.join("frontiers/CURRENT"), b"corrupt storage").unwrap();
        let result = if cancel {
            session.cancel(&mut store)
        } else {
            session.pause(&mut store)
        };
        assert!(result.is_err());
        assert_eq!(
            session.snapshot().unwrap().state,
            if cancel {
                CaseState::Cancelled
            } else {
                CaseState::Paused
            }
        );
        let reads = session.image_bytes_read_this_run();
        assert!(matches!(
            session.with_read(range(4, 8), |_| panic!(
                "stop must precede checkpoint success"
            )),
            Err(SessionError::Stopped)
        ));
        assert_eq!(session.image_bytes_read_this_run(), reads);
    }
}

#[test]
fn native_steps_require_global_memory_admission_and_return_deadline_allowance() {
    let fixture = Fixture::new();
    let mut session = CaseSession::create(fixture.ledger(), fixture.input(), config(1024)).unwrap();
    let remaining = session
        .admit_external("filesystem-helper", 128 * 1024)
        .unwrap();
    assert!(
        remaining > std::time::Duration::ZERO && remaining <= std::time::Duration::from_secs(59)
    );
    assert!(session.snapshot().unwrap().used.memory_bytes > 128 * 1024);
    assert!(
        session
            .admit_external("filesystem-helper", 1024 * 1024)
            .is_err()
    );
    assert_eq!(session.image_bytes_read_this_run(), 0);
    assert_eq!(session.snapshot().unwrap().state, CaseState::BudgetStopped);
}

#[test]
fn a_case_has_one_live_session_and_create_cannot_reset_an_existing_case() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let session = CaseSession::create(fixture.ledger(), fixture.input(), config(1024)).unwrap();
    let generation = session.source_generation();
    let mut missing = fixture.input();
    missing.image = fixture.root.join("does-not-exist");
    assert!(matches!(
        CaseSession::resume(fixture.ledger(), missing, generation.clone(), &store),
        Err(SessionError::CaseBusy)
    ));
    assert!(matches!(
        CaseSession::create(fixture.ledger(), fixture.input(), config(1024)),
        Err(SessionError::CaseBusy)
    ));
    drop(session);
    assert!(matches!(
        CaseSession::create(fixture.ledger(), fixture.input(), config(1024)),
        Err(SessionError::ExistingCase)
    ));
    assert!(CaseSession::resume(fixture.ledger(), fixture.input(), generation, &store).is_ok());
}

#[test]
fn readonly_semantic_loads_charge_reads_without_consuming_index_write_allowance() {
    let fixture = Fixture::new();
    let mut session = CaseSession::create(fixture.ledger(), fixture.input(), config(1024)).unwrap();
    let before = session.snapshot().unwrap().used;
    for _ in 0..3 {
        let found = session
            .with_ledger_resources(
                ResourceCaps {
                    read_bytes: 32,
                    memory_bytes: 65536,
                    index_bytes: 0,
                    output_bytes: 0,
                    visited_postings: 0,
                    transitions: 0,
                    decoder_seconds: 0,
                },
                |ledger| ledger.case_manifest(),
            )
            .unwrap();
        assert!(found.is_none());
    }
    let after = session.snapshot().unwrap().used;
    assert_eq!(after.read_bytes - before.read_bytes, 96);
    assert_eq!(after.index_bytes, before.index_bytes);
}

#[test]
fn repeated_exact_reads_preserve_frontier_identity_without_replenishing_read_budget() {
    let fixture = Fixture::new();
    let mut input = fixture.input();
    input.limits.receipts = 1;
    let mut session = CaseSession::create(fixture.ledger(), input, config(1024)).unwrap();
    let mut store = fixture.store();
    session
        .with_read(range(0, 4), |bytes| assert_eq!(bytes, b"abcd"))
        .unwrap();
    let first = session.checkpoint(&mut store).unwrap();
    let before = session.snapshot().unwrap().used.read_bytes;
    for _ in 0..10 {
        session
            .with_read(range(0, 4), |bytes| assert_eq!(bytes, b"abcd"))
            .unwrap();
    }
    assert_eq!(session.snapshot().unwrap().used.read_bytes - before, 40);
    assert_eq!(session.checkpoint(&mut store).unwrap(), first);
    assert!(session.with_read(range(4, 8), |_| ()).is_err());
}

#[test]
fn an_unpublished_epoch_cannot_retire_reads_or_nest_work_boundaries() {
    let fixture = Fixture::new();
    let mut input = fixture.input();
    input.limits.receipts = 1;
    let mut session = CaseSession::create(fixture.ledger(), input.clone(), config(1024)).unwrap();
    let epoch = session.begin_read_epoch().unwrap();
    assert!(matches!(
        session.begin_read_epoch(),
        Err(SessionError::InvalidEpoch)
    ));
    session.with_read(range(0, 4), |_| ()).unwrap();
    session.retain_read_epoch(epoch).unwrap();
    let next = session.begin_read_epoch().unwrap();
    // Closing an unsupported epoch retains the source receipt cap and evidence.
    assert!(session.with_read(range(4, 8), |_| ()).is_err());
    session.retain_read_epoch(next).unwrap();
    let mut store = fixture.store();
    let generation = session.source_generation();
    session.pause(&mut store).unwrap();
    drop(session);
    let mut resumed = CaseSession::resume(fixture.ledger(), input, generation, &store).unwrap();
    assert!(resumed.with_read(range(4, 8), |_| ()).is_err());
}

fn archive_config() -> CaseConfig {
    let mut c = config(4 * 1024 * 1024);
    c.caps.index_bytes = 8 * 1024 * 1024;
    c
}

#[test]
fn immutable_discovery_blocks_resume_under_the_same_live_dependency_cap() {
    let fixture = Fixture::new();
    let mut input = fixture.input();
    input.limits.receipts = 2;
    let mut session =
        CaseSession::create(fixture.ledger(), input.clone(), archive_config()).unwrap();
    let generation = session.source_generation();
    let mut store = fixture.store();
    session.with_read(range(0, 4), |_| ()).unwrap();
    session.with_read(range(4, 8), |_| ()).unwrap();
    assert!(session.with_read(range(8, 12), |_| ()).is_err());
    let first = session.archive_dependencies(&mut store).unwrap().unwrap();
    session.with_read(range(8, 12), |_| ()).unwrap();
    session.with_read(range(12, 16), |_| ()).unwrap();
    let second = session.archive_dependencies(&mut store).unwrap().unwrap();
    assert_ne!(first, second);
    session.with_read(range(0, 4), |_| ()).unwrap();
    session.with_read(range(4, 8), |_| ()).unwrap();
    assert_eq!(
        session.archive_dependencies(&mut store).unwrap(),
        Some(first)
    );
    let manifest = session.source_snapshot().unwrap();
    assert_eq!(manifest.archive_refs(), &[first, second]);
    assert!(manifest.consumed().is_empty());
    assert_eq!(session.image_bytes_read_this_run(), 24);
    assert_eq!(
        store.load_current().unwrap().unwrap().payloads,
        [first, second]
    );
    session.pause(&mut store).unwrap();
    let charged = session.snapshot().unwrap().used.read_bytes;
    drop(session);
    let mut resumed = CaseSession::resume(fixture.ledger(), input, generation, &store).unwrap();
    assert_eq!(resumed.image_bytes_read_this_run(), 16);
    assert!(resumed.snapshot().unwrap().used.read_bytes > charged + 16);
    assert_eq!(
        resumed.source_snapshot().unwrap().archive_refs(),
        &[first, second]
    );
    resumed.with_read(range(0, 4), |_| ()).unwrap();
    resumed.with_read(range(4, 8), |_| ()).unwrap();
    let block_dir = fixture.root.join("frontiers/source-dependencies");
    assert_eq!(
        fs::read_dir(block_dir)
            .unwrap()
            .filter(|e| e
                .as_ref()
                .unwrap()
                .path()
                .extension()
                .is_some_and(|s| s == "dep"))
            .count(),
        2
    );
}

#[test]
fn missing_or_corrupt_archived_dependencies_prevent_resume_adoption() {
    for remove in [false, true] {
        let fixture = Fixture::new();
        let mut session =
            CaseSession::create(fixture.ledger(), fixture.input(), archive_config()).unwrap();
        let generation = session.source_generation();
        let mut store = fixture.store();
        session.with_read(range(0, 8), |_| ()).unwrap();
        session.archive_dependencies(&mut store).unwrap();
        session.pause(&mut store).unwrap();
        drop(session);
        let block = fs::read_dir(fixture.root.join("frontiers/source-dependencies"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p.extension().is_some_and(|s| s == "dep"))
            .unwrap();
        if remove {
            fs::remove_file(block).unwrap();
        } else {
            fs::write(block, b"corrupt immutable source block").unwrap();
        }
        assert!(
            CaseSession::resume(fixture.ledger(), fixture.input(), generation, &store).is_err()
        );
    }
}

#[test]
fn interrupted_unlinked_archive_initialization_is_preserved_and_retried() {
    let fixture = Fixture::new();
    let mut session =
        CaseSession::create(fixture.ledger(), fixture.input(), archive_config()).unwrap();
    let mut store = fixture.store();
    let staged = fixture.root.join("frontiers/source-dependencies-NEXT");
    fs::create_dir(&staged).unwrap();
    fs::write(staged.join("OWNER"), b"partial owner").unwrap();
    session.with_read(range(0, 8), |_| ()).unwrap();
    session.archive_dependencies(&mut store).unwrap();
    assert_eq!(
        fs::read(
            fixture
                .root
                .join("frontiers/source-dependencies-failed-0/OWNER")
        )
        .unwrap(),
        b"partial owner"
    );
    assert_eq!(session.source_snapshot().unwrap().archive_refs().len(), 1);
}

#[test]
fn failed_archive_frontier_link_retains_live_reads_until_a_durable_retry() {
    let fixture = Fixture::new();
    let mut input = fixture.input();
    input.limits.receipts = 1;
    let mut session = CaseSession::create(fixture.ledger(), input, archive_config()).unwrap();
    let mut store = fixture.store();
    session.with_read(range(0, 4), |_| ()).unwrap();
    let blocker = fixture.root.join("frontiers/CURRENT");
    fs::create_dir(&blocker).unwrap();
    assert!(session.archive_dependencies(&mut store).is_err());
    assert_eq!(session.source_snapshot().unwrap().consumed().len(), 1);
    assert!(session.with_read(range(4, 8), |_| ()).is_err());
    fs::remove_dir(blocker).unwrap();
    session.archive_dependencies(&mut store).unwrap();
    session.with_read(range(4, 8), |_| ()).unwrap();
    assert_eq!(session.image_bytes_read_this_run(), 8);
}

#[test]
fn source_archive_references_cannot_change_inside_a_media_epoch() {
    let fixture = Fixture::new();
    let mut session =
        CaseSession::create(fixture.ledger(), fixture.input(), archive_config()).unwrap();
    let mut store = fixture.store();
    let epoch = session.begin_read_epoch().unwrap();
    session.with_read(range(0, 4), |_| ()).unwrap();
    assert!(matches!(
        session.archive_dependencies(&mut store),
        Err(SessionError::InvalidEpoch)
    ));
    session.retain_read_epoch(epoch).unwrap();
    assert!(session.archive_dependencies(&mut store).unwrap().is_some());
}

#[cfg(unix)]
#[test]
fn archive_initialization_preserves_a_broken_symlink_at_its_owned_name() {
    let fixture = Fixture::new();
    let mut session =
        CaseSession::create(fixture.ledger(), fixture.input(), archive_config()).unwrap();
    let mut store = fixture.store();
    let root = fixture.root.join("frontiers/source-dependencies");
    std::os::unix::fs::symlink("absent-owned-target", &root).unwrap();
    session.with_read(range(0, 4), |_| ()).unwrap();
    assert!(session.archive_dependencies(&mut store).is_err());
    assert_eq!(
        fs::read_link(&root).unwrap(),
        PathBuf::from("absent-owned-target")
    );
    assert_eq!(session.source_snapshot().unwrap().consumed().len(), 1);
}

#[test]
fn archiving_reserves_the_retained_block_and_largest_resume_buffer_together() {
    let fixture = Fixture::new();
    let size = 512 * 1024;
    fs::OpenOptions::new()
        .write(true)
        .open(fixture.root.join("image"))
        .unwrap()
        .set_len(size)
        .unwrap();
    fs::write(fixture.root.join("map"), format!("0 + 1\n0 {size} +\n")).unwrap();
    let mut input = fixture.input();
    input.binding =
        CoordinateBinding::new(range(0, size), 0, size, BindingEvidence::OperatorAttested).unwrap();
    input.limits.per_read = size;
    input.limits.total = 4 * 1024 * 1024;
    let mut session = CaseSession::create(fixture.ledger(), input, archive_config()).unwrap();
    session.with_read(range(0, size), |_| ()).unwrap();
    let mut store = fixture.store();
    assert!(session.archive_dependencies(&mut store).is_err());
    assert!(!fixture.root.join("frontiers/source-dependencies").exists());
    assert_eq!(session.image_bytes_read_this_run(), size);
}

#[test]
fn discovery_read_preflight_archives_only_when_new_geometry_would_fill_the_live_cap() {
    let fixture = Fixture::new();
    let mut input = fixture.input();
    input.limits.receipts = 2;
    let mut session = CaseSession::create(fixture.ledger(), input, archive_config()).unwrap();
    let mut store = fixture.store();
    session.with_read(range(0, 4), |_| ()).unwrap();
    session.with_read(range(4, 8), |_| ()).unwrap();
    session
        .prepare_discovery_read(range(0, 4), &mut store)
        .unwrap();
    assert!(session.source_snapshot().unwrap().archive_refs().is_empty());
    session
        .prepare_discovery_read(range(8, 12), &mut store)
        .unwrap();
    assert_eq!(session.source_snapshot().unwrap().archive_refs().len(), 1);
    assert_eq!(session.image_bytes_read_this_run(), 8);
    session.with_read(range(8, 12), |_| ()).unwrap();
    assert_eq!(session.image_bytes_read_this_run(), 12);
}

fn logical_view(session: &CaseSession) -> frameharbor::filesystem::LogicalFileView {
    use frameharbor::filesystem::{ExtentEvidence, ExtentRun, LogicalFileView};
    LogicalFileView {
        owner: LogicalOwner {
            source: session.source_generation(),
            filesystem: "fixture".into(),
            file_identity: "bytes".into(),
        },
        size: 16,
        metadata_flags: 1,
        name_flags: 1,
        attribute_flags: 3,
        initialized_size: 16,
        skip_length: 0,
        runs: vec![ExtentRun {
            logical: range(0, 16),
            image: Some(range(0, 16)),
            raw_flags: 0,
            allocation_consistent: true,
        }],
        evidence: ExtentEvidence::MetadataAllocated,
    }
}

#[test]
fn scoped_parser_memory_keeps_actual_reads_durable_and_bounds_operations() {
    use frameharbor::bmff::{LogicalReader, ReadAt};
    let fixture = Fixture::new();
    let mut cfg = config(128);
    cfg.caps.memory_bytes = 8 * 1024 * 1024;
    let mut session = CaseSession::create(fixture.ledger(), fixture.input(), cfg).unwrap();
    let view = logical_view(&session);
    let mut scope = session
        .begin_cpu_batch("probe-batch", 4 * 1024 * 1024, 2)
        .unwrap();
    let before = session.snapshot().unwrap().used.read_bytes;
    for (offset, expected) in [(0, b"abcd"), (4, b"efgh")] {
        let mut reader = LogicalReader::in_cpu_batch(&mut session, &view, &mut scope).unwrap();
        reader.reserve(128).unwrap();
        let mut bytes = [0; 4];
        reader.read_exact(offset, &mut bytes).unwrap();
        assert_eq!(&bytes, expected);
        reader.finish().unwrap();
    }
    assert!(LogicalReader::in_cpu_batch(&mut session, &view, &mut scope).is_err());
    // Each fresh reader legitimately prefetched the 16-byte logical view.
    assert_eq!(session.snapshot().unwrap().used.read_bytes - before, 32);
    assert_eq!(session.image_bytes_read_this_run(), 32);
    session.finish_cpu_batch(scope).unwrap();
}

#[test]
fn scoped_parser_grants_cannot_cross_sessions_or_expand_memory() {
    use frameharbor::bmff::{LogicalReader, ReadAt};
    let a = Fixture::new();
    let b = Fixture::new();
    let mut cfg = config(1024);
    cfg.caps.memory_bytes = 8 * 1024 * 1024;
    let mut first = CaseSession::create(a.ledger(), a.input(), cfg).unwrap();
    let mut second = CaseSession::create(b.ledger(), b.input(), cfg).unwrap();
    let mut scope = first
        .begin_cpu_batch("probe-batch", 4 * 1024 * 1024, 2)
        .unwrap();
    assert!(first.begin_cpu_batch("nested", 1, 1).is_err());
    let other_view = logical_view(&second);
    assert!(LogicalReader::in_cpu_batch(&mut second, &other_view, &mut scope).is_err());
    let view = logical_view(&first);
    let mut reader = LogicalReader::in_cpu_batch(&mut first, &view, &mut scope).unwrap();
    assert!(reader.reserve(8 * 1024 * 1024).is_err());
    drop(reader);
    assert_eq!(first.image_bytes_read_this_run(), 0);
    first.finish_cpu_batch(scope).unwrap();
}
