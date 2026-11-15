use frameharbor::checkpoint::*;
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!(
            "frameharbor-checkpoint-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )))
    }
    fn create(&self) -> CheckpointStore {
        CheckpointStore::create(&self.0, CheckpointLimits::default()).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn retains_current_plus_two_histories_without_copying_payloads() {
    let fixture = Fixture::new();
    let mut store = fixture.create();
    let refs = [[7; 32], [9; 32]];
    for i in 0..20 {
        store.publish(&[i], &refs).unwrap();
        let reopened = CheckpointStore::open(&fixture.0, CheckpointLimits::default()).unwrap();
        let current = reopened.load_current().unwrap().unwrap();
        assert_eq!(current.bytes, [i]);
        assert_eq!(current.payloads, refs);
        assert_eq!(reopened.history().unwrap().len(), usize::from(i + 1).min(3));
        let manifest_count = fs::read_dir(&fixture.0)
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("manifest-")
            })
            .count();
        assert_eq!(manifest_count, usize::from(i + 1).min(3));
    }
    // Repeated checkpoint at identical work is idempotent.
    let before = store.history().unwrap();
    store.publish(&[19], &refs).unwrap();
    assert_eq!(store.history().unwrap(), before);
    // No payload file exists: references stay small regardless of payload size.
    let allocated_logical: u64 = fs::read_dir(&fixture.0)
        .unwrap()
        .map(|e| e.unwrap().metadata().unwrap().len())
        .sum();
    assert!(allocated_logical < 4096);
}

#[test]
fn interrupted_publication_reopens_old_or_new_committed_frontier() {
    for boundary in [
        CheckpointBoundary::ManifestSynced,
        CheckpointBoundary::HistorySynced,
        CheckpointBoundary::HistoryRenamed,
        CheckpointBoundary::DirectorySynced,
    ] {
        let fixture = Fixture::new();
        let mut store = fixture.create();
        store.publish(b"old", &[[1; 32]]).unwrap();
        let result = store.publish_observed(b"new", &[[2; 32]], |at| {
            if at == boundary {
                Err(std::io::Error::other("injected boundary failure"))
            } else {
                Ok(())
            }
        });
        assert!(result.is_err());
        let mut reopened = CheckpointStore::open(&fixture.0, CheckpointLimits::default()).unwrap();
        let expected = match boundary {
            CheckpointBoundary::ManifestSynced | CheckpointBoundary::HistorySynced => b"old",
            _ => b"new",
        };
        assert_eq!(reopened.load_current().unwrap().unwrap().bytes, expected);
        reopened.reconcile().unwrap();
        reopened.publish(b"continued", &[[3; 32]]).unwrap();
        assert_eq!(
            reopened.load_current().unwrap().unwrap().bytes,
            b"continued"
        );
    }
}

#[test]
fn corruption_and_limits_fail_closed_without_changing_current() {
    let fixture = Fixture::new();
    let mut store = fixture.create();
    let id = store.publish(b"valid", &[]).unwrap();
    assert!(store.publish(&vec![0; 65537], &[]).is_err());
    assert_eq!(store.load_current().unwrap().unwrap().id, id);
    let current = fs::read(fixture.0.join("CURRENT")).unwrap();
    fs::write(fixture.0.join("NEXT"), b"partial interrupted write").unwrap();
    store.reconcile().unwrap();
    assert_eq!(fs::read(fixture.0.join("CURRENT")).unwrap(), current);
    let failure = fs::read_dir(&fixture.0)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("failed-staging-")
        })
        .unwrap();
    assert_eq!(fs::read(failure).unwrap(), b"partial interrupted write");
    assert!(!fixture.0.join("NEXT").exists());
    store.publish(b"other", &[]).unwrap();
    fs::write(fixture.0.join("CURRENT"), b"corrupt").unwrap();
    assert!(CheckpointStore::open(&fixture.0, CheckpointLimits::default()).is_err());
}

#[test]
fn partial_manifest_write_is_preserved_and_does_not_block_continuation() {
    let fixture = Fixture::new();
    let mut store = fixture.create();
    store.publish(b"old", &[]).unwrap();
    fs::write(fixture.0.join("MANIFEST-NEXT"), b"FHCP1\npartial payload").unwrap();
    let mut reopened = CheckpointStore::open(&fixture.0, CheckpointLimits::default()).unwrap();
    assert_eq!(reopened.load_current().unwrap().unwrap().bytes, b"old");
    reopened.publish(b"new", &[]).unwrap();
    assert_eq!(reopened.load_current().unwrap().unwrap().bytes, b"new");
    let failure = fs::read_dir(&fixture.0)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("failed-staging-")
        })
        .unwrap();
    assert_eq!(fs::read(failure).unwrap(), b"FHCP1\npartial payload");
}

#[test]
fn checksummed_foreign_staging_is_not_treated_as_history() {
    use sha2::{Digest, Sha256};
    let fixture = Fixture::new();
    let mut store = fixture.create();
    store.publish(b"old", &[]).unwrap();
    let mut foreign = b"different format with valid checksum".to_vec();
    foreign.extend(Sha256::digest(&foreign));
    fs::write(fixture.0.join("NEXT"), &foreign).unwrap();
    store.reconcile().unwrap();
    let failure = fs::read_dir(&fixture.0)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("failed-staging-")
        })
        .unwrap();
    assert_eq!(fs::read(failure).unwrap(), foreign);
    assert_eq!(store.load_current().unwrap().unwrap().bytes, b"old");
}

#[test]
fn refuses_existing_unowned_directory_and_symlink_inputs() {
    let fixture = Fixture::new();
    fs::create_dir(&fixture.0).unwrap();
    fs::write(fixture.0.join("user-data"), b"preserved").unwrap();
    assert!(CheckpointStore::create(&fixture.0, CheckpointLimits::default()).is_err());
    assert!(CheckpointStore::open(&fixture.0, CheckpointLimits::default()).is_err());
    assert_eq!(fs::read(fixture.0.join("user-data")).unwrap(), b"preserved");
}

#[test]
fn modified_immutable_manifest_is_rejected() {
    let fixture = Fixture::new();
    let mut store = fixture.create();
    store.publish(b"frontier", &[]).unwrap();
    let path = fs::read_dir(&fixture.0)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("manifest-")
        })
        .unwrap();
    let mut bytes = fs::read(&path).unwrap();
    bytes[20] ^= 1;
    fs::write(path, bytes).unwrap();
    assert!(CheckpointStore::open(&fixture.0, CheckpointLimits::default()).is_err());
}

#[cfg(unix)]
#[test]
fn symlink_marker_cannot_adopt_another_directory() {
    let fixture = Fixture::new();
    fs::create_dir(&fixture.0).unwrap();
    let outside = fixture.0.join("outside");
    fs::write(&outside, b"FrameHarbor frontier store v1\n").unwrap();
    std::os::unix::fs::symlink(&outside, fixture.0.join("OWNER")).unwrap();
    assert!(CheckpointStore::open(&fixture.0, CheckpointLimits::default()).is_err());
}

#[test]
fn unchanged_frontier_reuses_verified_current_without_a_new_durable_write() {
    let fixture = Fixture::new();
    let mut store = fixture.create();
    let id = store.publish(b"same-work", &[[4; 32]]).unwrap();
    let repeated = store
        .publish_observed(b"same-work", &[[4; 32]], |_| {
            Err(std::io::Error::other(
                "unexpected write for unchanged frontier",
            ))
        })
        .unwrap();
    assert_eq!(repeated, id);
    assert_eq!(store.history().unwrap(), [id]);
}
