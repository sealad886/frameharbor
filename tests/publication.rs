use frameharbor::publication::*;
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    base: PathBuf,
    dir: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let base = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "frameharbor-publication-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&base).unwrap();
        let dir = base.join("publication");
        Self { base, dir }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}
#[test]
fn synthetic_publication_reopens_and_duplicate_adoption_is_idempotent() {
    let f = Fixture::new();
    let bytes = b"generated synthetic contract fixture";
    let r = SyntheticFixtureReceipt::from_fixture(bytes);
    let mut p = Publisher::create(&f.dir, PublicationLimits::default()).unwrap();
    let a = p.publish_synthetic(&r, bytes, FaultPoint::None).unwrap();
    assert_eq!(fs::read(&a.path).unwrap(), bytes);
    assert_eq!(p.published_synthetic().unwrap(), vec![a.clone()]);
    drop(p);
    let mut p = Publisher::open(&f.dir, PublicationLimits::default()).unwrap();
    assert_eq!(p.publish_synthetic(&r, bytes, FaultPoint::None).unwrap(), a);
    assert_eq!(p.published_synthetic().unwrap().len(), 1);
}
#[test]
fn every_durable_boundary_reconciles_without_uncommitted_enumeration() {
    for boundary in [
        FaultPoint::StageFlushed,
        FaultPoint::IntentCommitted,
        FaultPoint::FinalLinked,
        FaultPoint::DirectoryFlushed,
        FaultPoint::PublishedCommitted,
    ] {
        let f = Fixture::new();
        let bytes = b"synthetic crash boundary";
        let r = SyntheticFixtureReceipt::from_fixture(bytes);
        let mut p = Publisher::create(&f.dir, PublicationLimits::default()).unwrap();
        assert!(
            matches!(p.publish_synthetic(&r,bytes,boundary),Err(PublicationError::Injected(b))if b==boundary)
        );
        assert_eq!(
            p.published_synthetic().unwrap().len(),
            usize::from(boundary == FaultPoint::PublishedCommitted)
        );
        drop(p);
        let mut p = Publisher::open(&f.dir, PublicationLimits::default()).unwrap();
        p.reconcile_synthetic().unwrap();
        if boundary == FaultPoint::StageFlushed {
            assert!(p.published_synthetic().unwrap().is_empty());
            assert!(
                fs::read_dir(&f.dir).unwrap().any(|e| e
                    .unwrap()
                    .path()
                    .extension()
                    .is_some_and(|x| x == "stage"))
            );
        } else {
            let a = p.published_synthetic().unwrap();
            assert_eq!(a.len(), 1);
            assert_eq!(fs::read(&a[0].path).unwrap(), bytes);
            assert_eq!(
                p.publish_synthetic(&r, bytes, FaultPoint::None).unwrap(),
                a[0]
            );
        }
    }
}
#[test]
fn corruption_after_intent_is_preserved_and_cannot_publish() {
    let f = Fixture::new();
    let bytes = b"expected synthetic bytes";
    let mut p = Publisher::create(&f.dir, PublicationLimits::default()).unwrap();
    let r = SyntheticFixtureReceipt::from_fixture(bytes);
    assert!(
        p.publish_synthetic(&r, bytes, FaultPoint::IntentCommitted)
            .is_err()
    );
    let stage = fs::read_dir(&f.dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|x| x == "stage"))
        .unwrap();
    fs::write(&stage, b"corruption").unwrap();
    assert!(matches!(
        p.reconcile_synthetic(),
        Err(PublicationError::Corrupt)
    ));
    assert!(p.published_synthetic().unwrap().is_empty());
    assert!(stage.exists());
    let c = rusqlite::Connection::open(f.dir.join("publication.sqlite")).unwrap();
    let failures: i64 = c
        .query_row("SELECT count(*) FROM failures", [], |r| r.get(0))
        .unwrap();
    assert_eq!(failures, 1);
}
#[test]
fn stage_and_total_output_and_verification_quotas_are_enforced() {
    let f = Fixture::new();
    let limits = PublicationLimits {
        artifact_bytes: 8,
        output_bytes: 10,
        verification_bytes: 100,
        ..PublicationLimits::default()
    };
    let mut p = Publisher::create(&f.dir, limits).unwrap();
    assert!(
        p.publish_synthetic(
            &SyntheticFixtureReceipt::from_fixture(b"123456789"),
            b"123456789",
            FaultPoint::None
        )
        .is_err()
    );
    p.publish_synthetic(
        &SyntheticFixtureReceipt::from_fixture(b"123456"),
        b"123456",
        FaultPoint::None,
    )
    .unwrap();
    assert!(matches!(
        p.publish_synthetic(
            &SyntheticFixtureReceipt::from_fixture(b"abcdef"),
            b"abcdef",
            FaultPoint::None
        ),
        Err(PublicationError::Quota)
    ));
    let f = Fixture::new();
    let limits = PublicationLimits {
        verification_bytes: 2,
        ..PublicationLimits::default()
    };
    let mut p = Publisher::create(&f.dir, limits).unwrap();
    assert!(matches!(
        p.publish_synthetic(
            &SyntheticFixtureReceipt::from_fixture(b"123"),
            b"123",
            FaultPoint::None
        ),
        Err(PublicationError::Quota)
    ));
    assert!(p.published_synthetic().unwrap().is_empty());
}
#[test]
fn unowned_directory_database_and_final_name_are_never_overwritten() {
    let f = Fixture::new();
    fs::create_dir(&f.dir).unwrap();
    fs::write(f.dir.join("keep"), b"mine").unwrap();
    assert!(Publisher::create(&f.dir, PublicationLimits::default()).is_err());
    assert!(Publisher::open(&f.dir, PublicationLimits::default()).is_err());
    assert_eq!(fs::read(f.dir.join("keep")).unwrap(), b"mine");
    let f = Fixture::new();
    let mut p = Publisher::create(&f.dir, PublicationLimits::default()).unwrap();
    let a = p
        .publish_synthetic(
            &SyntheticFixtureReceipt::from_fixture(b"one"),
            b"one",
            FaultPoint::None,
        )
        .unwrap();
    fs::write(&a.path, b"changed").unwrap();
    assert!(matches!(
        p.published_synthetic(),
        Err(PublicationError::Corrupt)
    ));
    drop(p);
    let c = rusqlite::Connection::open(f.dir.join("publication.sqlite")).unwrap();
    c.execute_batch("PRAGMA application_id=123").unwrap();
    drop(c);
    assert!(matches!(
        Publisher::open(&f.dir, PublicationLimits::default()),
        Err(PublicationError::Unowned)
    ));
}
#[cfg(unix)]
#[test]
fn symlink_roots_and_destinations_are_rejected_without_touching_targets() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let real = f.base.join("real");
    fs::create_dir(&real).unwrap();
    let link = f.base.join("link");
    symlink(&real, &link).unwrap();
    assert!(matches!(
        Publisher::create(&link.join("child"), PublicationLimits::default()),
        Err(PublicationError::Unowned)
    ));
    assert!(!real.join("child").exists());
    let mut p = Publisher::create(&f.dir, PublicationLimits::default()).unwrap();
    let bytes = b"synthetic symlink test";
    let receipt = SyntheticFixtureReceipt::from_fixture(bytes);
    assert!(
        p.publish_synthetic(&receipt, bytes, FaultPoint::IntentCommitted)
            .is_err()
    );
    let stage = fs::read_dir(&f.dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|x| x == "stage"))
        .unwrap();
    let target = f.base.join("target");
    fs::write(&target, b"unowned").unwrap();
    let final_path = stage.with_extension("synthetic");
    symlink(&target, &final_path).unwrap();
    assert!(p.reconcile_synthetic().is_err());
    assert_eq!(fs::read(target).unwrap(), b"unowned");
    assert!(p.published_synthetic().unwrap().is_empty());
}
#[test]
fn orphan_stages_consume_quota_and_missing_flushed_stage_cannot_be_adopted() {
    let f = Fixture::new();
    let limits = PublicationLimits {
        output_bytes: 10,
        ..PublicationLimits::default()
    };
    let mut p = Publisher::create(&f.dir, limits).unwrap();
    assert!(
        p.publish_synthetic(
            &SyntheticFixtureReceipt::from_fixture(b"123456"),
            b"123456",
            FaultPoint::StageFlushed
        )
        .is_err()
    );
    assert!(matches!(
        p.publish_synthetic(
            &SyntheticFixtureReceipt::from_fixture(b"abcdef"),
            b"abcdef",
            FaultPoint::None
        ),
        Err(PublicationError::Quota)
    ));
    assert!(p.published_synthetic().unwrap().is_empty());
    let f = Fixture::new();
    let mut p = Publisher::create(&f.dir, PublicationLimits::default()).unwrap();
    assert!(
        p.publish_synthetic(
            &SyntheticFixtureReceipt::from_fixture(b"lost stage"),
            b"lost stage",
            FaultPoint::IntentCommitted
        )
        .is_err()
    );
    let stage = fs::read_dir(&f.dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|x| x == "stage"))
        .unwrap();
    fs::remove_file(stage).unwrap();
    assert!(p.reconcile_synthetic().is_err());
    assert!(p.published_synthetic().unwrap().is_empty());
}
#[test]
fn equal_unowned_destination_after_intent_is_not_adopted() {
    let f = Fixture::new();
    let mut p = Publisher::create(&f.dir, PublicationLimits::default()).unwrap();
    let bytes = b"same bytes distinct owner";
    assert!(
        p.publish_synthetic(
            &SyntheticFixtureReceipt::from_fixture(bytes),
            bytes,
            FaultPoint::IntentCommitted
        )
        .is_err()
    );
    let stage = fs::read_dir(&f.dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|x| x == "stage"))
        .unwrap();
    let final_path = stage.with_extension("synthetic");
    fs::write(&final_path, bytes).unwrap();
    assert!(matches!(
        p.reconcile_synthetic(),
        Err(PublicationError::Conflict)
    ));
    assert_eq!(fs::read(final_path).unwrap(), bytes);
    assert!(p.published_synthetic().unwrap().is_empty());
}

#[test]
fn distinct_equal_sized_stage_and_final_cannot_hide_allocation_from_new_publish() {
    let f = Fixture::new();
    let limits = PublicationLimits {
        output_bytes: 12,
        ..PublicationLimits::default()
    };
    let mut p = Publisher::create(&f.dir, limits).unwrap();
    let bytes = b"123456";
    assert!(
        p.publish_synthetic(
            &SyntheticFixtureReceipt::from_fixture(bytes),
            bytes,
            FaultPoint::IntentCommitted
        )
        .is_err()
    );
    let stage = fs::read_dir(&f.dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|x| x == "stage"))
        .unwrap();
    let final_path = stage.with_extension("synthetic");
    fs::write(&final_path, bytes).unwrap();
    // The two distinct six-byte allocations already consume all output space.
    // A new payload must fail before its stage is created, regardless of equal IDs.
    assert!(matches!(
        p.publish_synthetic(
            &SyntheticFixtureReceipt::from_fixture(b"new"),
            b"new",
            FaultPoint::None
        ),
        Err(PublicationError::Conflict)
    ));
    assert_eq!(fs::read(&stage).unwrap(), bytes);
    assert_eq!(fs::read(&final_path).unwrap(), bytes);
    assert_eq!(
        fs::read_dir(&f.dir)
            .unwrap()
            .filter(|e| e
                .as_ref()
                .unwrap()
                .path()
                .extension()
                .is_some_and(|x| x == "stage"))
            .count(),
        1
    );
}
