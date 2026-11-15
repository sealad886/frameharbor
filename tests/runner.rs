#![cfg(target_os = "macos")]
#[allow(dead_code)]
mod common;
use common::{
    filesystems::{Damage, Fixture, FsKind, Layout},
    media::{Codec, MediaFixture, Moov, Timing},
};
use frameharbor::{
    acquisition::ReadLimits,
    contracts::*,
    control::{CaseConfig, CaseState},
    filesystem::FilesystemProfile,
    jobs::{HelperPin, JobManifest},
    runner::Runner,
};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::{OnceLock, atomic::AtomicBool},
};
fn helpers() -> &'static (PathBuf, PathBuf) {
    static H: OnceLock<(PathBuf, PathBuf)> = OnceLock::new();
    H.get_or_init(|| {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let tsk = root.join(format!("target/runner-tsk-{}", std::process::id()));
        let media = root.join(format!("target/runner-media-{}", std::process::id()));
        let s = Command::new("/usr/bin/cc")
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
            .arg(&tsk)
            .status()
            .unwrap();
        assert!(s.success());
        let s = Command::new("/usr/bin/cc")
            .args([
                "-std=c11",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-I/opt/homebrew/opt/ffmpeg-full/include",
            ])
            .arg(root.join("native/ffmpeg_helper.c"))
            .args([
                "-L/opt/homebrew/opt/ffmpeg-full/lib",
                "-Wl,-rpath,/opt/homebrew/opt/ffmpeg-full/lib",
                "-lavformat",
                "-lavcodec",
                "-lavutil",
                "-o",
            ])
            .arg(&media)
            .status()
            .unwrap();
        assert!(s.success());
        (tsk, media)
    })
}
fn manifest(f: &Fixture) -> JobManifest {
    let root = f.dir.canonicalize().unwrap();
    let size = f.ground_truth.logical_image_bytes;
    let (tsk, media) = helpers();
    let pin = |p: &PathBuf| HelperPin {
        path: p.clone(),
        sha256: Sha256::digest(fs::read(p).unwrap()).into(),
    };
    JobManifest {
        image: f.image.clone(),
        map: f.map.clone(),
        case_dir: root.join("runner-case"),
        output_dir: root.join("exports"),
        binding: CoordinateBinding::new(
            ByteRange::new(0, size).unwrap(),
            0,
            size,
            BindingEvidence::OperatorAttested,
        )
        .unwrap(),
        profile: FilesystemProfile::Fat32,
        fs_start: 0,
        filesystem_helper: pin(tsk),
        media_helper: pin(media),
        read_limits: ReadLimits {
            per_read: 1024 * 1024,
            total: 2 * 1024 * 1024 * 1024,
            receipts: 4096,
        },
        case_config: CaseConfig {
            total_millis: 120000,
            shutdown_reserve_millis: 1000,
            caps: ResourceCaps {
                read_bytes: 2 * 1024 * 1024 * 1024,
                memory_bytes: 512 * 1024 * 1024,
                index_bytes: 128 * 1024 * 1024,
                output_bytes: 16 * 1024 * 1024,
                visited_postings: 0,
                transitions: 0,
                decoder_seconds: 600,
            },
        },
        ledger_bytes: 16 * 1024 * 1024,
        index_bytes: 4 * 1024 * 1024,
        checkpoint_history: 2,
    }
}
#[test]
fn real_controller_exports_exact_media_and_adopts_completed_index_and_publication_on_resume() {
    let media = MediaFixture::generate(Codec::Avc, Moov::Tail, Timing::Vfr).unwrap();
    let mut f = Fixture::build(FsKind::Fat32, Layout::Fragmented, Damage::None).unwrap();
    f.replace_payload(&media.padded_bytes(32768).unwrap())
        .unwrap();
    let mut m = manifest(&f);
    m.index_bytes = 64 * 1024 * 1024;
    let case = m.case_dir.clone();
    let measured = frameharbor::performance::measure_new(m, &AtomicBool::new(false)).unwrap();
    assert!(measured.wall.as_nanos() > 0);
    assert!(measured.source_allocated_bytes < measured.source_logical_bytes);
    assert!(measured.case_and_output_allocated_bytes > 0);
    assert!(measured.report.image_read_bytes_this_run.unwrap() > 0);
    let first = measured.report;
    assert!(first.discovery_complete);
    assert_eq!(first.artifacts.len(), 1);
    assert_eq!(
        fs::read(&first.artifacts[0].path).unwrap(),
        f.ground_truth.payload
    );
    assert_eq!(first.snapshot.state, CaseState::Paused);
    let mut runner = Runner::open(&case).unwrap();
    let second = runner.run(&AtomicBool::new(false)).unwrap();
    assert_eq!(second.artifacts, first.artifacts);
    assert!(first.snapshot.used.index_bytes < 4 * 1024 * 1024);
    assert!(second.snapshot.used.index_bytes - first.snapshot.used.index_bytes < 1024 * 1024);
    assert!(second.discovery_complete);
    assert_eq!(second.qualified_views, first.qualified_views);
    // No new validation staging or second publication after restart.
    assert_eq!(
        fs::read_dir(&case)
            .unwrap()
            .filter(|e| e
                .as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("stage-"))
            .count(),
        1
    );
    drop(runner);
    assert_eq!(Runner::inspect(&case).unwrap().artifacts.len(), 1);
    Runner::cancel(&case).unwrap();
    assert_eq!(
        Runner::inspect(&case).unwrap().snapshot.state,
        CaseState::Cancelled
    );
    assert!(
        Runner::open(&case)
            .unwrap()
            .run(&AtomicBool::new(false))
            .is_err()
    );
}
#[test]
fn unsupported_target_remains_durable_unresolved_and_existing_case_is_never_overwritten() {
    let f = Fixture::build(FsKind::Fat32, Layout::Contiguous, Damage::None).unwrap();
    let m = manifest(&f);
    let case = m.case_dir.clone();
    let mut runner = Runner::create(m.clone()).unwrap();
    assert!(Runner::create(m).is_err());
    let report = runner.run(&AtomicBool::new(false)).unwrap();
    assert!(report.discovery_complete);
    assert!(report.artifacts.is_empty());
    assert!(!report.unresolved.is_empty());
    drop(runner);
    let inspected = Runner::pause(&case).unwrap();
    assert_eq!(inspected.snapshot.state, CaseState::Paused);
    assert_eq!(inspected.unresolved, report.unresolved);
}

#[test]
fn source_invalidation_keeps_portable_receipts_inspectable_without_payload_io() {
    use std::io::{Seek, SeekFrom, Write};
    let media = MediaFixture::generate(Codec::Hevc, Moov::Head, Timing::Cfr).unwrap();
    let mut f = Fixture::build(FsKind::Fat32, Layout::Fragmented, Damage::None).unwrap();
    f.replace_payload(&media.padded_bytes(32768).unwrap())
        .unwrap();
    let m = manifest(&f);
    let case = m.case_dir.clone();
    let mut runner = Runner::create(m).unwrap();
    let before = runner.run(&AtomicBool::new(false)).unwrap();
    drop(runner);
    let mut image = fs::OpenOptions::new().write(true).open(&f.image).unwrap();
    image
        .seek(SeekFrom::Start(f.ground_truth.spans[0].image_offset))
        .unwrap();
    image.write_all(&[255]).unwrap();
    image.sync_all().unwrap();
    drop(image);
    assert!(
        Runner::open(&case)
            .unwrap()
            .run(&AtomicBool::new(false))
            .is_err()
    );
    // Stored receipt inspection survives both source invalidation and payload
    // loss; it explicitly makes no fresh verification/existence claim.
    fs::remove_file(&before.artifacts[0].path).unwrap();
    let bytes = fs::read(case.join("case.sqlite")).unwrap();
    let report = Runner::inspect(&case).unwrap();
    assert!(!report.source_valid);
    assert!(!report.payloads_reverified);
    assert_eq!(report.artifacts, before.artifacts);
    assert!(!report.unresolved.is_empty());
    assert_eq!(fs::read(case.join("case.sqlite")).unwrap(), bytes);
}

#[test]
fn damaged_payload_controller_publishes_later_idr_segment_with_explicit_remainder() {
    use frameharbor::{
        bmff::{self, ParseLimits, SliceReader},
        publication::ArtifactKind,
        validation::ArtifactTransform,
    };
    let media = MediaFixture::generate(Codec::Avc, Moov::Head, Timing::Cfr).unwrap();
    let target =
        bmff::inspect_layout(&mut SliceReader::new(&media.bytes), ParseLimits::default()).unwrap();
    let missing = target
        .tracks
        .iter()
        .find(|t| t.handler == *b"vide")
        .unwrap()
        .samples[2]
        .logical;
    let mut f = Fixture::build(FsKind::Fat32, Layout::Fragmented, Damage::None).unwrap();
    f.replace_payload(&media.padded_bytes(32768).unwrap())
        .unwrap();
    let span = f
        .ground_truth
        .spans
        .iter()
        .find(|s| s.logical_offset <= missing.start() && s.logical_offset + s.len >= missing.end())
        .unwrap();
    let start = span.image_offset + missing.start() - span.logical_offset;
    let end = start + missing.len();
    let size = f.ground_truth.logical_image_bytes;
    fs::write(
        &f.map,
        format!(
            "0 + 1\n0 {start} +\n{start} {} -\n{end} {} +\n",
            end - start,
            size - end
        ),
    )
    .unwrap();
    let m = manifest(&f);
    let case = m.case_dir.clone();
    let mut runner = Runner::create(m).unwrap();
    let report = runner.run(&AtomicBool::new(false)).unwrap();
    assert!(report.discovery_complete);
    assert!(!report.artifacts.is_empty());
    assert!(!report.unresolved.is_empty());
    for a in &report.artifacts {
        let ArtifactKind::ValidatedMedia(e) = &a.kind else {
            panic!("validated media")
        };
        let ArtifactTransform::SamplePreserving(t) = &e.receipt.transform else {
            panic!("transformed segment required")
        };
        assert!(
            t.mappings
                .iter()
                .filter(|s| s.track_id == 1)
                .all(|s| s.original.ordinal >= 12)
        );
        assert!(!e.receipt.original_file_bytes_preserved);
    }
    drop(runner);
    let again = Runner::open(&case)
        .unwrap()
        .run(&AtomicBool::new(false))
        .unwrap();
    assert_eq!(again.artifacts, report.artifacts);
    assert_eq!(again.unresolved, report.unresolved);
}

#[test]
fn fragmented_profile_controller_routes_to_qualified_transformed_outputs() {
    use frameharbor::{publication::ArtifactKind, validation::ArtifactTransform};
    for codec in [Codec::Avc, Codec::Hevc] {
        let media =
            MediaFixture::generate_closed_idr(codec, Moov::Fragmented, Timing::Vfr).unwrap();
        let mut f = Fixture::build(FsKind::Fat32, Layout::Fragmented, Damage::None).unwrap();
        f.replace_payload(&media.padded_bytes(32768).unwrap())
            .unwrap();
        let m = manifest(&f);
        let case = m.case_dir.clone();
        let mut runner = Runner::create(m).unwrap();
        let report = runner.run(&AtomicBool::new(false)).unwrap();
        assert!(report.discovery_complete);
        assert!(
            !report.artifacts.is_empty(),
            "fMP4 {codec:?} did not route to segments: {:?}",
            report.unresolved
        );
        assert!(!report.unresolved.is_empty());
        for artifact in &report.artifacts {
            let ArtifactKind::ValidatedMedia(e) = &artifact.kind else {
                panic!("validated media required")
            };
            assert!(matches!(
                e.receipt.transform,
                ArtifactTransform::SamplePreserving(_)
            ));
            assert!(!e.receipt.original_file_bytes_preserved);
        }
        drop(runner);
        let resumed = Runner::open(&case)
            .unwrap()
            .run(&AtomicBool::new(false))
            .unwrap();
        assert_eq!(resumed.artifacts, report.artifacts);
        assert_eq!(resumed.unresolved, report.unresolved);
    }
}

#[test]
fn paged_namespace_and_unresolved_archives_reach_valid_media_after_1024_nonmedia_files() {
    let media = MediaFixture::generate(Codec::Avc, Moov::Head, Timing::Cfr).unwrap();
    let mut f = Fixture::build(FsKind::Fat32, Layout::Fragmented, Damage::None).unwrap();
    f.replace_payload(&media.padded_bytes(32768).unwrap())
        .unwrap();
    f.add_many_files_before_payload(1100, 8).unwrap();
    let mut m = manifest(&f);
    m.case_config.total_millis = 600000;
    let case = m.case_dir.clone();
    let mut runner = Runner::create(m).unwrap();
    let report = runner.run(&AtomicBool::new(false)).unwrap();
    assert!(report.discovery_complete);
    assert_eq!(report.qualified_views, 1100);
    assert_eq!(report.pending_media, 0);
    assert_eq!(report.unresolved_media, 1099);
    assert!(report.unresolved_examples_truncated);
    assert_eq!(report.artifacts.len(), 1);
    assert_eq!(
        fs::read(&report.artifacts[0].path).unwrap(),
        f.ground_truth.payload
    );
    assert!(report.snapshot.used.index_bytes < 128 * 1024 * 1024);
    assert!(
        report.unresolved.len() < 140,
        "report loaded whole namespace into examples"
    );
    drop(runner);
    let resumed = Runner::open(&case)
        .unwrap()
        .run(&AtomicBool::new(false))
        .unwrap();
    assert_eq!(resumed.artifacts, report.artifacts);
    assert_eq!(resumed.pending_media, 0);
    assert_eq!(resumed.unresolved_media, 1099);
    assert_eq!(resumed.qualified_views, 1100);
}

#[test]
fn missing_target_metadata_remains_unresolved_without_stopping_other_valid_files() {
    let media = MediaFixture::generate(Codec::Avc, Moov::Head, Timing::Cfr).unwrap();
    let mut f = Fixture::build(FsKind::Fat32, Layout::Fragmented, Damage::None).unwrap();
    f.replace_payload(&media.padded_bytes(32768).unwrap())
        .unwrap();
    f.add_many_files_before_payload(2, 512).unwrap();
    let start = f.ground_truth.root_offset + (200 - 2) * f.ground_truth.cluster_size;
    let end = start + 8;
    let size = f.ground_truth.logical_image_bytes;
    fs::write(
        &f.map,
        format!("0 + 1\n0 {start} +\n{start} 8 -\n{end} {} +\n", size - end),
    )
    .unwrap();
    let mut runner = Runner::create(manifest(&f)).unwrap();
    let report = runner.run(&AtomicBool::new(false)).unwrap();
    assert!(report.discovery_complete);
    assert_eq!(report.artifacts.len(), 1);
    assert_eq!(report.unresolved_media, 1);
    assert!(
        report
            .unresolved
            .iter()
            .any(|s| s.contains("target metadata acquisition missing"))
    );
    assert_eq!(
        fs::read(&report.artifacts[0].path).unwrap(),
        f.ground_truth.payload
    );
}

#[test]
fn corrupt_sample_count_is_unresolved_without_stopping_other_valid_files() {
    // A candidate's table envelope must not consume the whole case's authority.
    let media = MediaFixture::generate(Codec::Avc, Moov::Head, Timing::Cfr).unwrap();
    let mut f = Fixture::build(FsKind::Fat32, Layout::Fragmented, Damage::None).unwrap();
    let payload = media.padded_bytes(32768).unwrap();
    f.replace_payload(&payload).unwrap();
    f.add_many_files_before_payload(2, 16384).unwrap();
    let mut damaged = payload[..16384].to_vec();
    let stsz = damaged.windows(4).position(|w| w == b"stsz").unwrap();
    damaged[stsz + 12..stsz + 16].copy_from_slice(&u32::MAX.to_be_bytes());
    let start = f.ground_truth.root_offset + (200 - 2) * f.ground_truth.cluster_size;
    use std::io::{Seek, SeekFrom, Write};
    let mut image = fs::OpenOptions::new().write(true).open(&f.image).unwrap();
    image.seek(SeekFrom::Start(start)).unwrap();
    image.write_all(&damaged).unwrap();
    image.sync_all().unwrap();
    drop(image);
    let mut runner = Runner::create(manifest(&f)).unwrap();
    let report = runner.run(&AtomicBool::new(false)).unwrap();
    assert!(report.discovery_complete);
    assert_eq!(report.pending_media, 0);
    assert_eq!(report.unresolved_media, 1);
    assert_eq!(report.artifacts.len(), 1);
    assert_eq!(
        fs::read(&report.artifacts[0].path).unwrap(),
        f.ground_truth.payload
    );
}

#[test]
fn committed_original_adoption_restores_portable_reference_before_outcome() {
    const MEDIA_OUTCOME: &str = "qualified-media-outcome-v2";
    const PORTABLE_ARTIFACT: &str = "portable-artifact-v1";
    let media = MediaFixture::generate(Codec::Avc, Moov::Head, Timing::Cfr).unwrap();
    let mut f = Fixture::build(FsKind::Fat32, Layout::Fragmented, Damage::None).unwrap();
    f.replace_payload(&media.padded_bytes(32768).unwrap())
        .unwrap();
    let m = manifest(&f);
    let case = m.case_dir.clone();
    let mut runner = Runner::create(m).unwrap();
    let first = runner.run(&AtomicBool::new(false)).unwrap();
    drop(runner);
    // Retain publication but restore the durable prefix immediately before the
    // controller linked that artifact and committed its media-work outcome.
    let db = rusqlite::Connection::open(case.join("case.sqlite")).unwrap();
    let triggers = db
        .prepare("SELECT name,sql FROM sqlite_master WHERE type='trigger' AND tbl_name='receipts'")
        .unwrap()
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    // Fixture-only restoration of a committed prefix; production receipts remain
    // append-only. Restore every protection before reopening through Runner.
    for (name, _) in &triggers {
        db.execute_batch(&format!("DROP TRIGGER \"{}\"", name.replace('"', "\"\"")))
            .unwrap();
    }
    db.execute(
        "DELETE FROM receipts WHERE kind IN (?1,?2)",
        [MEDIA_OUTCOME, PORTABLE_ARTIFACT],
    )
    .unwrap();
    for (_, sql) in triggers {
        db.execute_batch(&sql).unwrap();
    }
    drop(db);
    let second = Runner::open(&case)
        .unwrap()
        .run(&AtomicBool::new(false))
        .unwrap();
    assert_eq!(second.artifacts, first.artifacts);
    assert_eq!(second.pending_media, 0);
    let db = rusqlite::Connection::open(case.join("case.sqlite")).unwrap();
    let linked: i64 = db.query_row(
        "SELECT COUNT(*) FROM receipts AS o JOIN receipts AS a ON o.work=a.work WHERE o.kind=?1 AND a.kind=?2 AND a.payload=?3",
        rusqlite::params![MEDIA_OUTCOME, PORTABLE_ARTIFACT, first.artifacts[0].id.as_bytes()],
        |row| row.get(0),
    ).unwrap();
    assert_eq!(linked, 1);
}

#[test]
fn caller_intake_time_is_included_in_durable_case_elapsed() {
    let f = Fixture::build(FsKind::Fat32, Layout::Fragmented, Damage::None).unwrap();
    let m = manifest(&f);
    let case = m.case_dir.clone();
    let started = std::time::Instant::now()
        .checked_sub(std::time::Duration::from_millis(250))
        .unwrap();
    let runner = Runner::create_started(m, started).unwrap();
    let report = Runner::inspect(&case).unwrap();
    assert!(report.snapshot.elapsed_millis >= 250);
    drop(runner);
}

#[test]
fn future_intake_start_is_rejected_before_destination_creation() {
    let f = Fixture::build(FsKind::Fat32, Layout::Fragmented, Damage::None).unwrap();
    let m = manifest(&f);
    let case = m.case_dir.clone();
    let output = m.output_dir.clone();
    let future = std::time::Instant::now()
        .checked_add(std::time::Duration::from_secs(60))
        .unwrap();
    assert!(Runner::create_started(m, future).is_err());
    assert!(!case.exists());
    assert!(!output.exists());
}

#[test]
fn resumed_caller_intake_is_included_without_resetting_prior_elapsed() {
    let f = Fixture::build(FsKind::Fat32, Layout::Fragmented, Damage::None).unwrap();
    let m = manifest(&f);
    let case = m.case_dir.clone();
    let mut runner = Runner::create(m).unwrap();
    let first = runner.run(&AtomicBool::new(false)).unwrap();
    drop(runner);
    // Resume intake starts after the prior pause; backdating across that durable
    // boundary would correctly trigger ClockReversed instead of modelling work.
    let started = std::time::Instant::now();
    std::thread::sleep(std::time::Duration::from_millis(250));
    let second = Runner::open(&case)
        .unwrap()
        .run_started(&AtomicBool::new(false), started)
        .unwrap();
    assert!(second.snapshot.elapsed_millis >= first.snapshot.elapsed_millis + 250);
    assert_eq!(second.pending_media, first.pending_media);
}
