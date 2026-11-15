#![cfg(target_os = "macos")]
#[allow(dead_code)]
mod common;
use common::filesystems::{Damage, Fixture, FsKind, Layout};
use frameharbor::{
    acquisition::ReadLimits,
    contracts::{BindingEvidence, ByteRange, CoordinateBinding, ResourceCaps},
    control::{CaseConfig, CaseControl},
    filesystem::*,
    ledger::Ledger,
    session::{CaseSession, SessionInput},
};
use std::{
    path::PathBuf,
    process::Command,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
static PROCESS_TESTS: Mutex<()> = Mutex::new(());

fn compile_helper(mode: usize) -> &'static PathBuf {
    static HELPERS: OnceLock<Vec<PathBuf>> = OnceLock::new();
    &HELPERS.get_or_init(|| {
        (0..4)
            .map(|mode| {
                let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
                let output = root.join(format!(
                    "target/tsk-helper-fixture-{}-{mode}",
                    std::process::id()
                ));
                let status = Command::new("/usr/bin/cc")
                    .args([
                        "-std=c11",
                        "-Wall",
                        "-Wextra",
                        "-Werror",
                        "-I/usr/local/include",
                    ])
                    .arg(format!("-DFRAMEHARBOR_TEST_MODE={mode}"))
                    .arg(root.join("native/tsk_helper.c"))
                    .arg("/usr/local/lib/libtsk.23.dylib")
                    .arg("-o")
                    .arg(&output)
                    .status()
                    .unwrap();
                assert!(status.success(), "selected TSK helper failed to compile");
                output
            })
            .collect()
    })[mode]
}
fn session(f: &Fixture) -> CaseSession {
    let size = f.ground_truth.logical_image_bytes;
    CaseSession::create(
        Ledger::open(&f.dir.join("case.sqlite"), 8 * 1024 * 1024).unwrap(),
        SessionInput {
            image: f.image.clone(),
            map: f.map.clone(),
            binding: CoordinateBinding::new(
                ByteRange::new(0, size).unwrap(),
                0,
                size,
                BindingEvidence::OperatorAttested,
            )
            .unwrap(),
            limits: ReadLimits {
                per_read: 1024 * 1024,
                total: 128 * 1024 * 1024,
                receipts: 1024,
            },
        },
        CaseConfig {
            total_millis: 60000,
            shutdown_reserve_millis: 1000,
            caps: ResourceCaps {
                read_bytes: 128 * 1024 * 1024,
                memory_bytes: 512 * 1024 * 1024,
                index_bytes: 1024 * 1024,
                output_bytes: 1024 * 1024,
                visited_postings: 0,
                transitions: 0,
                decoder_seconds: 0,
            },
        },
    )
    .unwrap()
}
fn profile(kind: FsKind) -> FilesystemProfile {
    match kind {
        FsKind::Fat32 => FilesystemProfile::Fat32,
        FsKind::ExFat => FilesystemProfile::ExFat,
    }
}
fn run(f: &Fixture, session: &mut CaseSession, kind: FsKind) -> FilesystemReport {
    inspect(
        session,
        compile_helper(0),
        f.ground_truth.logical_image_bytes,
        0,
        profile(kind),
        AdapterLimits::default(),
        &AtomicBool::new(false),
    )
    .unwrap()
}

#[test]
fn allocated_fat32_and_exfat_views_match_retained_fragmented_ground_truth() {
    for kind in [FsKind::Fat32, FsKind::ExFat] {
        for layout in [Layout::Fragmented, Layout::Contiguous] {
            let f = Fixture::build(kind, layout, Damage::None).unwrap();
            let mut session = session(&f);
            let report = run(&f, &mut session, kind);
            assert_eq!(
                report.geometry.block_size, 512,
                "TSK run units are sectors, not filesystem clusters"
            );
            let view = report
                .files
                .iter()
                .find(|v| v.size == f.ground_truth.payload.len() as u64)
                .expect("fixture logical file missing");
            assert_eq!(
                view.evidence,
                ExtentEvidence::MetadataAllocated,
                "{kind:?}/{layout:?}: {view:?}"
            );
            // Equivalent neighboring extents may be coalesced by the backend.
            // Verify the logical-to-physical mapping, not the chosen run encoding.
            for expected in &f.ground_truth.spans {
                let expected_logical =
                    ByteRange::from_len(expected.logical_offset, expected.len).unwrap();
                let mut covered = 0;
                for actual in &view.runs {
                    if let Some(overlap) = actual.logical.intersection(expected_logical) {
                        assert_eq!(
                            actual.image.unwrap().start() + overlap.start()
                                - actual.logical.start(),
                            expected.image_offset + overlap.start() - expected.logical_offset
                        );
                        covered += overlap.len();
                    }
                }
                assert_eq!(covered, expected.len);
            }
            view.with_read(
                &mut session,
                ByteRange::new(0, view.size).unwrap(),
                1024 * 1024,
                |bytes| assert_eq!(bytes, f.ground_truth.payload),
            )
            .unwrap();
            let boundary = f.ground_truth.cluster_size;
            view.with_read(
                &mut session,
                ByteRange::new(boundary - 2, boundary + 5).unwrap(),
                1024,
                |bytes| {
                    assert_eq!(
                        bytes,
                        &f.ground_truth.payload[boundary as usize - 2..boundary as usize + 5]
                    )
                },
            )
            .unwrap();
            assert!(report.callback_reads > 0);
            assert!(report.callback_bytes >= 65536);
            assert!(report.max_rss_bytes.unwrap() > 0);
        }
    }
}

#[test]
fn deleted_partial_crosslinked_and_bitmap_free_metadata_never_promote_owner_exactness() {
    for kind in [FsKind::Fat32, FsKind::ExFat] {
        for damage in [Damage::Deleted, Damage::PartialChain, Damage::Crosslink] {
            let f = Fixture::build(kind, Layout::Fragmented, damage).unwrap();
            let mut session = session(&f);
            let result = inspect(
                &mut session,
                compile_helper(0),
                f.ground_truth.logical_image_bytes,
                0,
                profile(kind),
                AdapterLimits::default(),
                &AtomicBool::new(false),
            );
            if let Ok(report) = result {
                for view in report
                    .files
                    .iter()
                    .filter(|v| v.size == f.ground_truth.payload.len() as u64)
                {
                    assert_ne!(
                        view.evidence,
                        ExtentEvidence::MetadataAllocated,
                        "{kind:?}/{damage:?}: {view:?}"
                    );
                }
            }
        }
    }
    let f = Fixture::build(FsKind::ExFat, Layout::Contiguous, Damage::BitmapFree).unwrap();
    let mut session = session(&f);
    if let Ok(report) = inspect(
        &mut session,
        compile_helper(0),
        f.ground_truth.logical_image_bytes,
        0,
        FilesystemProfile::ExFat,
        AdapterLimits::default(),
        &AtomicBool::new(false),
    ) {
        for view in report
            .files
            .iter()
            .filter(|v| v.size == f.ground_truth.payload.len() as u64)
        {
            assert_ne!(view.evidence, ExtentEvidence::MetadataAllocated);
        }
    }
}

#[test]
fn acquisition_holes_inside_metadata_or_payload_are_not_substituted_with_stale_bytes() {
    for damage in [Damage::MetadataHole, Damage::AcquisitionHole] {
        let f = Fixture::build(FsKind::ExFat, Layout::Fragmented, damage).unwrap();
        let mut session = session(&f);
        let report = inspect(
            &mut session,
            compile_helper(0),
            f.ground_truth.logical_image_bytes,
            0,
            FilesystemProfile::ExFat,
            AdapterLimits::default(),
            &AtomicBool::new(false),
        );
        if let Ok(report) = report {
            for view in report
                .files
                .iter()
                .filter(|v| v.size == f.ground_truth.payload.len() as u64)
            {
                assert!(
                    view.with_read(
                        &mut session,
                        ByteRange::new(0, view.size).unwrap(),
                        1024 * 1024,
                        |_| ()
                    )
                    .is_err()
                );
            }
        }
    }
}

#[test]
fn native_helper_hang_crash_and_malformed_output_are_scoped_bounded_failures() {
    let _exclusive = PROCESS_TESTS.lock().unwrap_or_else(|e| e.into_inner());
    for mode in [1, 2, 3] {
        let helper = compile_helper(mode);
        let f = Fixture::build(FsKind::ExFat, Layout::Fragmented, Damage::None).unwrap();
        let mut session = session(&f);
        let start = Instant::now();
        let mut cleanup = None;
        let result = inspect_observed(
            &mut session,
            helper,
            f.ground_truth.logical_image_bytes,
            0,
            FilesystemProfile::ExFat,
            AdapterLimits {
                wall: Duration::from_secs(2),
                ..AdapterLimits::default()
            },
            &AtomicBool::new(false),
            |receipt| cleanup = Some(receipt),
        );
        match mode {
            1 => assert!(
                matches!(result, Err(FilesystemError::TimedOut)),
                "{result:?}"
            ),
            2 => assert!(
                matches!(result, Err(FilesystemError::NativeFailure)),
                "{result:?}"
            ),
            _ => assert!(
                matches!(result, Err(FilesystemError::Protocol)),
                "{result:?}"
            ),
        }
        assert!(
            session.image_bytes_read_this_run() > 0,
            "test must reach actual helper READ callback"
        );
        assert!(start.elapsed() < Duration::from_secs(4));
        let cleanup = cleanup.expect("owned child cleanup receipt missing");
        assert!(cleanup.reaped, "{cleanup:?}");
        assert!(cleanup.status.is_some());
        assert!(cleanup.wait_error.is_none());
        if mode == 2 {
            assert_eq!(cleanup.status.unwrap().code(), Some(77));
        }
    }
}

#[test]
fn cancellation_kills_and_reaps_active_native_parser_after_its_masked_read() {
    let _exclusive = PROCESS_TESTS.lock().unwrap_or_else(|e| e.into_inner());
    let helper = compile_helper(1);
    let f = Fixture::build(FsKind::ExFat, Layout::Fragmented, Damage::None).unwrap();
    let mut session = session(&f);
    let cancel = Arc::new(AtomicBool::new(false));
    let signal = cancel.clone();
    let database = f.dir.join("case.sqlite");
    let source_generation = session.source_generation();
    let initial_read_bytes = session.snapshot().unwrap().used.read_bytes;
    let trigger = std::thread::spawn(move || {
        let observer = CaseControl::adopt(
            Ledger::open(&database, 8 * 1024 * 1024).unwrap(),
            source_generation,
        )
        .unwrap();
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(2) {
            if observer
                .snapshot()
                .is_ok_and(|s| s.used.read_bytes > initial_read_bytes)
            {
                signal.store(true, Ordering::Relaxed);
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        signal.store(true, Ordering::Relaxed);
        false
    });
    let started = Instant::now();
    let mut cleanup = None;
    let result = inspect_observed(
        &mut session,
        helper,
        f.ground_truth.logical_image_bytes,
        0,
        FilesystemProfile::ExFat,
        AdapterLimits {
            wall: Duration::from_secs(3),
            ..AdapterLimits::default()
        },
        &cancel,
        |receipt| cleanup = Some(receipt),
    );
    assert!(
        trigger.join().unwrap(),
        "native callback was never admitted"
    );
    assert!(
        matches!(result, Err(FilesystemError::Cancelled)),
        "{result:?}"
    );
    assert!(session.image_bytes_read_this_run() > 0);
    assert!(started.elapsed() < Duration::from_secs(2));
    let cleanup = cleanup.expect("owned native child cleanup receipt missing");
    assert!(cleanup.reaped, "{cleanup:?}");
    assert!(cleanup.status.is_some());
    assert!(cleanup.wait_error.is_none());
    assert!(cleanup.kill_error.is_none());
}

#[test]
fn allocation_name_and_run_flags_cannot_replace_missing_metadata_allocation() {
    use std::os::unix::fs::PermissionsExt;
    for meta_flags in [0, 4] {
        let f = Fixture::build(FsKind::ExFat, Layout::Contiguous, Damage::None).unwrap();
        let helper = f.dir.join("synthetic-protocol-helper");
        // Mutated observation fixture exercises public protocol classification,
        // without claiming that this shell fixture is a certified TSK parser.
        let text = format!(
            "#!/bin/sh\nprintf '%s\\n' 'HELLO 4.14.0 4.14.0' 'GEOMETRY 10 512 {} 2' 'DIRECTORY 2 1 0' 'ENTRY 0' 'FILE 5 512 {meta_flags} 1 3 1 512 0' 'RUN 0 {} 1 0 1' 'END_FILE' 'ENTRY_END 1' 'BATCH_END 1 1 0 0' 'DONE 1 0'\n",
            f.ground_truth.logical_image_bytes / 512,
            f.ground_truth.spans[0].image_offset / 512
        );
        std::fs::write(&helper, text).unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut session = session(&f);
        let report = inspect(
            &mut session,
            &helper,
            f.ground_truth.logical_image_bytes,
            0,
            FilesystemProfile::ExFat,
            AdapterLimits::default(),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(report.files.len(), 1);
        assert_ne!(report.files[0].evidence, ExtentEvidence::MetadataAllocated);
        assert!(
            report.files[0]
                .with_read(&mut session, ByteRange::new(0, 512).unwrap(), 512, |_| ())
                .is_err()
        );
    }
}

#[test]
fn callback_cache_amplification_is_bounded_before_image_reads() {
    let helper = compile_helper(0);
    for limits in [
        AdapterLimits {
            callback_bytes: 32,
            ..AdapterLimits::default()
        },
        AdapterLimits {
            total_read_bytes: 32,
            ..AdapterLimits::default()
        },
    ] {
        let f = Fixture::build(FsKind::ExFat, Layout::Fragmented, Damage::None).unwrap();
        let mut session = session(&f);
        let result = inspect(
            &mut session,
            helper,
            f.ground_truth.logical_image_bytes,
            0,
            FilesystemProfile::ExFat,
            limits,
            &AtomicBool::new(false),
        );
        assert!(
            matches!(result, Err(FilesystemError::ResourceLimit)),
            "{result:?}"
        );
        assert_eq!(session.image_bytes_read_this_run(), 0);
    }
}

#[test]
fn caller_geometry_cannot_expand_the_bound_source_image() {
    let f = Fixture::build(FsKind::ExFat, Layout::Contiguous, Damage::None).unwrap();
    let mut session = session(&f);
    let result = inspect(
        &mut session,
        std::path::Path::new("/nonexistent-helper"),
        f.ground_truth.logical_image_bytes + 512,
        0,
        FilesystemProfile::ExFat,
        AdapterLimits::default(),
        &AtomicBool::new(false),
    );
    assert!(matches!(result, Err(FilesystemError::SourceMismatch)));
    assert_eq!(session.image_bytes_read_this_run(), 0);
}

#[test]
fn malformed_fat_chains_and_boot_geometry_fail_within_owned_child_and_read_bounds() {
    use std::{
        fs::OpenOptions,
        io::{Read, Seek, SeekFrom, Write},
    };
    // These are exposed regression corruptions of independently retained truth,
    // never holdouts. Both FAT copies and both boot copies are mutated together.
    for corruption in [
        "cycle",
        "outside-cluster",
        "oversized-volume",
        "overflow-fat",
        "invalid-cluster-size",
    ] {
        let f = Fixture::build(FsKind::Fat32, Layout::Fragmented, Damage::None).unwrap();
        let mut image = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&f.image)
            .unwrap();
        let mut boot = [0u8; 512];
        image.read_exact(&mut boot).unwrap();
        let fat_bytes = u32::from_le_bytes(boot[36..40].try_into().unwrap()) as u64 * 512;
        match corruption {
            "cycle" | "outside-cluster" => {
                let next: u32 = if corruption == "cycle" {
                    5
                } else {
                    0x000f_ffff
                };
                for copy in 0..2 {
                    image
                        .seek(SeekFrom::Start(
                            f.ground_truth.fat_offset + copy * fat_bytes + 5 * 4,
                        ))
                        .unwrap();
                    image.write_all(&next.to_le_bytes()).unwrap();
                }
            }
            _ => {
                match corruption {
                    "oversized-volume" => boot[32..36].copy_from_slice(&u32::MAX.to_le_bytes()),
                    "overflow-fat" => boot[36..40].copy_from_slice(&u32::MAX.to_le_bytes()),
                    _ => boot[13] = 255,
                };
                for offset in [0, 6 * 512] {
                    image.seek(SeekFrom::Start(offset)).unwrap();
                    image.write_all(&boot).unwrap();
                }
            }
        }
        image.sync_all().unwrap();
        drop(image);
        let mut session = session(&f);
        let limit = AdapterLimits {
            wall: Duration::from_secs(2),
            total_read_bytes: 4 * 1024 * 1024,
            ..AdapterLimits::default()
        };
        let mut cleanup = None;
        let started = Instant::now();
        let result = inspect_observed(
            &mut session,
            compile_helper(0),
            f.ground_truth.logical_image_bytes,
            0,
            FilesystemProfile::Fat32,
            limit,
            &AtomicBool::new(false),
            |receipt| cleanup = Some(receipt),
        );
        if let Ok(report) = &result {
            for view in report
                .files
                .iter()
                .filter(|v| v.size == f.ground_truth.payload.len() as u64)
            {
                assert_ne!(
                    view.evidence,
                    ExtentEvidence::MetadataAllocated,
                    "{corruption}: {view:?}"
                );
            }
            assert!(report.callback_bytes <= limit.total_read_bytes);
        }
        assert!(
            started.elapsed() < Duration::from_secs(4),
            "{corruption} exceeded bounded stop envelope"
        );
        let cleanup = cleanup.expect("malformed regression must execute the native parser");
        assert!(cleanup.reaped, "{corruption}: {cleanup:?}");
        assert!(cleanup.status.is_some());
        assert!(cleanup.wait_error.is_none());
        // The map domain is exactly the image range. The adapter rejects callback
        // ranges outside it before CaseSession can read, and the serial range
        // engine applies the same checked source bounds to every admitted read.
        assert!(
            session.image_bytes_read_this_run() <= limit.total_read_bytes,
            "{corruption} exceeded source read allowance"
        );
        assert!(
            session.image_bytes_read_this_run() > 0,
            "{corruption} did not reach native image parsing: {result:?}"
        );
    }
}

#[test]
fn speculative_native_cache_uses_only_acquired_prefix_before_payload_hole() {
    let f = Fixture::build(FsKind::Fat32, Layout::Fragmented, Damage::None).unwrap();
    let hole = f.ground_truth.spans[0].image_offset + 200;
    let size = f.ground_truth.logical_image_bytes;
    std::fs::write(
        &f.map,
        format!(
            "0 + 1\n0 {hole} +\n{hole} 100 -\n{} {} +\n",
            hole + 100,
            size - hole - 100
        ),
    )
    .unwrap();
    let mut case = session(&f);
    let report = inspect(
        &mut case,
        compile_helper(0),
        size,
        0,
        FilesystemProfile::Fat32,
        AdapterLimits::default(),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(
        report.callback_requested_bytes > report.callback_bytes,
        "actual native helper must encounter a short speculative cache fill"
    );
    let view = report
        .files
        .iter()
        .find(|v| v.size == f.ground_truth.payload.len() as u64)
        .unwrap();
    assert_eq!(view.evidence, ExtentEvidence::MetadataAllocated);
    let missing = ByteRange::from_len(hole, 100).unwrap();
    assert!(
        case.source_snapshot()
            .unwrap()
            .consumed()
            .iter()
            .all(|c| c.image.intersection(missing).is_none())
    );
    assert!(
        view.with_read(
            &mut case,
            ByteRange::new(0, view.size).unwrap(),
            1024 * 1024,
            |_| ()
        )
        .is_err()
    );
}

#[test]
fn native_metadata_read_crossing_a_hole_cannot_adopt_a_short_suffix() {
    for hole_offset in [0, 300] {
        let f = Fixture::build(FsKind::Fat32, Layout::Fragmented, Damage::None).unwrap();
        let hole = f.ground_truth.root_offset + hole_offset;
        let size = f.ground_truth.logical_image_bytes;
        std::fs::write(
            &f.map,
            format!(
                "0 + 1\n0 {hole} +\n{hole} 100 -\n{} {} +\n",
                hole + 100,
                size - hole - 100
            ),
        )
        .unwrap();
        let mut case = session(&f);
        let result = inspect(
            &mut case,
            compile_helper(0),
            size,
            0,
            FilesystemProfile::Fat32,
            AdapterLimits::default(),
            &AtomicBool::new(false),
        );
        assert!(
            result.is_err(),
            "metadata must not be read across acquisition hole: {result:?}"
        );
        let missing = ByteRange::from_len(hole, 100).unwrap();
        assert!(
            case.source_snapshot()
                .unwrap()
                .consumed()
                .iter()
                .all(|c| c.image.intersection(missing).is_none())
        );
    }
}

#[test]
fn malformed_or_zero_native_prefix_response_exits_before_tsk_fallback() {
    use std::io::Write;
    use std::process::Stdio;
    let f = Fixture::build(FsKind::Fat32, Layout::Fragmented, Damage::None).unwrap();
    for response in [b"OK 0\n".as_slice(), b"OK 65537\n", b"BOGUS\n"] {
        let mut child = Command::new(compile_helper(0))
            .args([
                f.ground_truth.logical_image_bytes.to_string(),
                "0".into(),
                "fat32".into(),
                "batch".into(),
                "root".into(),
                "0".into(),
                "256".into(),
                "1024".into(),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(response).unwrap();
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(11));
        let output = String::from_utf8(output.stdout).unwrap();
        assert!(output.contains("ERROR 11\n"));
        assert!(!output.contains("GEOMETRY"));
        assert!(!output.contains("DONE"));
    }
}

#[test]
fn native_directory_batches_resume_direct_entry_cursors_and_keep_views_provisional() {
    let f = Fixture::build(FsKind::Fat32, Layout::Fragmented, Damage::None).unwrap();
    let mut case = session(&f);
    let mut cursor = DirectoryCursor {
        directory_inode: None,
        next_entry: 0,
    };
    let mut files = Vec::new();
    let mut before_reads = 0;
    loop {
        let batch = inspect_directory_batch(
            &mut case,
            compile_helper(0),
            f.ground_truth.logical_image_bytes,
            0,
            FilesystemProfile::Fat32,
            cursor,
            DirectoryBatchLimits {
                entries: 1,
                ..DirectoryBatchLimits::default()
            },
            &AtomicBool::new(false),
            |_, event| {
                if let DirectoryEvent::BeforeRead {
                    requested,
                    delivered,
                } = event
                {
                    assert!(requested.contains(delivered));
                    before_reads += 1;
                }
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(batch.start_entry(), cursor.next_entry);
        assert!(batch.next_entry() > batch.start_entry() || batch.directory_complete());
        for file in batch.files() {
            assert_ne!(file.view.evidence, ExtentEvidence::MetadataAllocated);
            files.push(file.view.owner.clone());
        }
        if batch.directory_complete() {
            break;
        }
        cursor = DirectoryCursor {
            directory_inode: Some(batch.directory_inode()),
            next_entry: batch.next_entry(),
        };
    }
    assert_eq!(files.len(), 1);
    assert!(before_reads > 0);
}
