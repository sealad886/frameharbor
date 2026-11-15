#![cfg(target_os = "macos")]
#[allow(dead_code)]
mod common;
use common::filesystems::{Damage, Fixture, FsKind, Layout};
use frameharbor::{
    acquisition::ReadLimits,
    checkpoint::{CheckpointLimits, CheckpointStore},
    contracts::{BindingEvidence, ByteRange, CoordinateBinding, ResourceCaps},
    control::CaseConfig,
    discovery::{DiscoveryLimits, DiscoveryQueue},
    filesystem::{self, DirectoryBatchLimits, DirectoryEvent, ExtentEvidence, FilesystemProfile},
    ledger::{Ledger, WorkSpec},
    session::{CaseSession, SessionInput},
};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};
fn helper() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let path = root.join(format!(
        "target/discovery-tsk-helper-{}",
        std::process::id()
    ));
    let mut child = std::process::Command::new("/usr/bin/cc")
        .args([
            "-std=c11",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-I/usr/local/include",
        ])
        .arg(root.join("native/tsk_helper.c"))
        .arg("/usr/local/lib/libtsk.23.dylib")
        .arg("-o")
        .arg(&path)
        .spawn()
        .unwrap();
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if start.elapsed() > Duration::from_secs(20) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("native fixture compiler deadline")
        }
        std::thread::yield_now();
    }
    path
}
#[test]
fn actual_native_batches_archive_before_cursor_commit_and_reopen_idempotently() {
    let fixture = Fixture::build(FsKind::Fat32, Layout::Fragmented, Damage::None).unwrap();
    let size = fixture.ground_truth.logical_image_bytes;
    let input = SessionInput {
        image: fixture.image.clone(),
        map: fixture.map.clone(),
        binding: CoordinateBinding::new(
            ByteRange::new(0, size).unwrap(),
            0,
            size,
            BindingEvidence::OperatorAttested,
        )
        .unwrap(),
        limits: ReadLimits {
            per_read: 1024 * 1024,
            total: 256 * 1024 * 1024,
            receipts: 1024,
        },
    };
    let mut session = CaseSession::create(
        Ledger::open(&fixture.dir.join("case.sqlite"), 32 * 1024 * 1024).unwrap(),
        input.clone(),
        CaseConfig {
            total_millis: 120000,
            shutdown_reserve_millis: 5000,
            caps: ResourceCaps {
                read_bytes: 512 * 1024 * 1024,
                memory_bytes: 512 * 1024 * 1024,
                index_bytes: 64 * 1024 * 1024,
                output_bytes: 1024 * 1024,
                visited_postings: 0,
                transitions: 0,
                decoder_seconds: 0,
            },
        },
    )
    .unwrap();
    let spec = WorkSpec {
        source: session.source_generation(),
        algorithm: "actual-native-directory-index-v1".into(),
        stage: "filesystem-discovery".into(),
        parameters: BTreeMap::from([("helper".into(), "compiled-selected-tsk4.14".into())]),
    };
    let limits = DiscoveryLimits {
        entries_per_batch: 2,
        index_bytes: 16 * 1024 * 1024,
        ..DiscoveryLimits::default()
    };
    let queue = session
        .with_ledger(65536, |l| {
            Ok(DiscoveryQueue::open(
                &fixture.dir,
                l,
                &spec,
                FilesystemProfile::Fat32,
                0,
                limits,
            ))
        })
        .unwrap()
        .unwrap();
    let mut checkpoints =
        CheckpointStore::create(&fixture.dir.join("frontiers"), CheckpointLimits::default())
            .unwrap();
    let helper = helper();
    let mut last = None;
    let mut batches = 0;
    loop {
        let cursor = session
            .with_ledger(65536, |l| Ok(queue.next(l)))
            .unwrap()
            .unwrap();
        let Some(cursor) = cursor else { break };
        let batch = filesystem::inspect_directory_batch(
            &mut session,
            &helper,
            size,
            0,
            FilesystemProfile::Fat32,
            cursor,
            DirectoryBatchLimits {
                entries: 2,
                ..DirectoryBatchLimits::default()
            },
            &AtomicBool::new(false),
            |s, event| {
                if let DirectoryEvent::BeforeRead { delivered, .. } = event {
                    s.prepare_discovery_read(delivered, &mut checkpoints)
                        .map_err(filesystem::FilesystemError::Session)?;
                }
                Ok(())
            },
        )
        .unwrap();
        assert!(
            batch
                .files()
                .iter()
                .all(|f| f.view.evidence != ExtentEvidence::MetadataAllocated)
        );
        let proof = session
            .seal_discovery_batch(&mut checkpoints, batch.digest())
            .unwrap();
        let bound = queue.write_bound(&batch).unwrap();
        session
            .with_ledger_resources(bound, |l| Ok(queue.commit_batch(l, &batch, proof)))
            .unwrap()
            .unwrap();
        batches += 1;
        last = Some(batch);
    }
    assert!(batches >= 2);
    let catalog_bound = session
        .with_ledger(65536, |l| Ok(queue.catalog_read_bound(l)))
        .unwrap()
        .unwrap();
    session
        .with_ledger_resources(catalog_bound, |l| Ok(queue.verify_catalog(l)))
        .unwrap()
        .unwrap();
    assert!(
        session
            .with_ledger(65536, |l| Ok(queue.finish(l)))
            .unwrap()
            .unwrap()
    );
    let bound = session
        .with_ledger(65536, |l| Ok(queue.page_read_bound(l, None, 16)))
        .unwrap()
        .unwrap();
    let views = session
        .with_ledger_resources(bound, |l| Ok(queue.qualified_page(l, None, 16)))
        .unwrap()
        .unwrap();
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].view.evidence, ExtentEvidence::MetadataAllocated);
    let source = session.source_generation();
    session.pause(&mut checkpoints).unwrap();
    drop(session);
    let mut resumed = CaseSession::resume(
        Ledger::open(&fixture.dir.join("case.sqlite"), 32 * 1024 * 1024).unwrap(),
        input,
        source,
        &checkpoints,
    )
    .unwrap();
    let queue = resumed
        .with_ledger(65536, |l| {
            Ok(DiscoveryQueue::open(
                &fixture.dir,
                l,
                &spec,
                FilesystemProfile::Fat32,
                0,
                limits,
            ))
        })
        .unwrap()
        .unwrap();
    let batch = last.unwrap();
    let proof = resumed
        .seal_discovery_batch(&mut checkpoints, batch.digest())
        .unwrap();
    let replay = resumed
        .with_ledger(65536, |l| Ok(queue.commit_batch(l, &batch, proof)))
        .unwrap()
        .unwrap();
    assert!(replay.adopted);
    let readonly =
        Ledger::open_readonly(&fixture.dir.join("case.sqlite"), 32 * 1024 * 1024).unwrap();
    let observed = DiscoveryQueue::open_readonly(
        &fixture.dir,
        &readonly,
        &spec,
        FilesystemProfile::Fat32,
        0,
        limits,
    )
    .unwrap()
    .unwrap();
    let status = observed.status(&readonly).unwrap();
    assert!(status.complete);
    assert_eq!(
        status.namespace_policy,
        "allocated-reachable-native-names-v1"
    );
    assert!(status.excluded_virtual_files + status.excluded_virtual_directories > 0);
}
