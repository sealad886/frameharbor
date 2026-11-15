#![cfg(target_os = "macos")]
#[allow(dead_code)]
mod common;
use common::{
    filesystems::{Damage, Fixture, FsKind, Layout},
    media::{Codec, MediaFixture, Moov, Timing},
};
use frameharbor::publication::*;
use frameharbor::{
    acquisition::ReadLimits,
    bmff::{self, LogicalReader, ParseLimits, SampleMap},
    contracts::*,
    control::CaseConfig,
    filesystem::{ExtentEvidence, ExtentRun, LogicalFileView},
    ledger::Ledger,
    session::{CaseSession, SessionInput},
    validation::*,
};
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::{OnceLock, atomic::AtomicBool},
};

fn helper(mode: usize) -> &'static PathBuf {
    static HELPERS: OnceLock<Vec<PathBuf>> = OnceLock::new();
    &HELPERS.get_or_init(|| {
        (0..4)
            .map(|mode| {
                let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
                let out = root.join(format!(
                    "target/ffmpeg-validation-{}-{mode}",
                    std::process::id()
                ));
                let status = Command::new("/usr/bin/cc")
                    .args([
                        "-std=c11",
                        "-Wall",
                        "-Wextra",
                        "-Werror",
                        "-I/opt/homebrew/opt/ffmpeg-full/include",
                    ])
                    .arg(format!("-DFRAMEHARBOR_TEST_MODE={mode}"))
                    .arg(root.join("native/ffmpeg_helper.c"))
                    .args([
                        "-L/opt/homebrew/opt/ffmpeg-full/lib",
                        "-Wl,-rpath,/opt/homebrew/opt/ffmpeg-full/lib",
                        "-lavformat",
                        "-lavcodec",
                        "-lavutil",
                        "-o",
                    ])
                    .arg(&out)
                    .status()
                    .unwrap();
                assert!(status.success());
                out
            })
            .collect()
    })[mode]
}
fn prepared(
    codec: Codec,
    moov: Moov,
    timing: Timing,
) -> (
    Fixture,
    CaseSession,
    LogicalFileView,
    SampleMap,
    MediaFixture,
) {
    let media = MediaFixture::generate(codec, moov, timing).unwrap();
    let mut f = Fixture::build(FsKind::Fat32, Layout::Fragmented, Damage::None).unwrap();
    let padded = media.padded_bytes(32768).unwrap();
    f.replace_payload(&padded).unwrap();
    let size = f.ground_truth.logical_image_bytes;
    let mut case = CaseSession::create(
        Ledger::open(&f.dir.join("case.sqlite"), 16 * 1024 * 1024).unwrap(),
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
                total: 512 * 1024 * 1024,
                receipts: 4096,
            },
        },
        CaseConfig {
            total_millis: 120000,
            shutdown_reserve_millis: 1000,
            caps: ResourceCaps {
                read_bytes: 512 * 1024 * 1024,
                memory_bytes: 512 * 1024 * 1024,
                index_bytes: 16 * 1024 * 1024,
                output_bytes: 16 * 1024 * 1024,
                visited_postings: 0,
                transitions: 0,
                decoder_seconds: 600,
            },
        },
    )
    .unwrap();
    // This fixture-owned map isolates validator behavior. Native filesystem-to-
    // sample-map integration is separately exercised in bmff_extents.rs.
    let view = LogicalFileView {
        owner: LogicalOwner {
            source: case.source_generation(),
            filesystem: "fixture-fat32".into(),
            file_identity: "original".into(),
        },
        size: padded.len() as u64,
        metadata_flags: 5,
        name_flags: 1,
        attribute_flags: 3,
        initialized_size: padded.len() as u64,
        skip_length: 0,
        runs: f
            .ground_truth
            .spans
            .iter()
            .map(|span| ExtentRun {
                logical: ByteRange::from_len(span.logical_offset, span.len).unwrap(),
                image: Some(ByteRange::from_len(span.image_offset, span.len).unwrap()),
                raw_flags: 0,
                allocation_consistent: true,
            })
            .collect(),
        evidence: ExtentEvidence::MetadataAllocated,
    };
    let map = bmff::inspect(
        &mut LogicalReader::new(&mut case, &view),
        ParseLimits::default(),
    )
    .unwrap();
    (f, case, view, map, media)
}
fn stage(
    f: &Fixture,
    case: &mut CaseSession,
    view: &LogicalFileView,
    map: SampleMap,
) -> StagedArtifact {
    let mut area = StagingArea::create(&f.dir.join("staging"), 16 * 1024 * 1024, 16).unwrap();
    area.stage_exact(case, view, map, ValidationLimits::default())
        .unwrap()
}

#[test]
fn sealed_media_receipt_retains_full_source_provenance_and_survives_source_loss() {
    let (f, mut case, view, map, _) = prepared(Codec::Avc, Moov::Head, Timing::Vfr);
    let staged = stage(&f, &mut case, &view, map);
    let validated = validate(
        &mut case,
        staged,
        helper(0),
        ValidationLimits::default(),
        &AtomicBool::new(false),
    )
    .unwrap();
    let expected_hash = validated.sha256();
    let root = f.dir.join("published");
    let mut publisher = Publisher::create(&root, PublicationLimits::default()).unwrap();
    let artifact = publisher
        .publish_media(&mut case, &validated, FaultPoint::None)
        .unwrap();
    assert_eq!(fs::read(&artifact.path).unwrap(), f.ground_truth.payload);
    assert_eq!(
        publisher
            .publish_media(&mut case, &validated, FaultPoint::None)
            .unwrap()
            .id,
        artifact.id
    );
    let before = case.snapshot().unwrap().used.read_bytes;
    assert_eq!(
        publisher.published_media_with_session(&mut case).unwrap(),
        std::slice::from_ref(&artifact)
    );
    assert!(case.snapshot().unwrap().used.read_bytes >= before + artifact.bytes);
    let ArtifactKind::ValidatedMedia(e) = &artifact.kind else {
        panic!("typed media receipt required")
    };
    assert_eq!(e.owner, view.owner);
    assert_eq!(e.provenance.layout, view);
    assert!(!e.provenance.consumed.is_empty());
    assert_eq!(e.provenance.binding, BindingEvidence::OperatorAttested);
    assert_eq!(e.receipt.helper_sha256, validated.receipt().helper_sha256);
    assert!(e.receipt.helper_read_bytes > 0);
    assert_ne!(artifact.id, artifact.sha256);
    assert_eq!(
        artifact.sha256,
        expected_hash
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    drop(publisher);
    drop(validated);
    drop(case);
    fs::remove_file(&f.image).unwrap();
    let mut reopened = Publisher::open(&root, PublicationLimits::default()).unwrap();
    let outputs = reopened.published_media().unwrap();
    assert_eq!(outputs, std::slice::from_ref(&artifact));
    fs::write(&artifact.path, b"changed portable payload").unwrap();
    assert_eq!(reopened.media_receipts().unwrap(), [artifact]);
    assert!(reopened.published_media().is_err());
}
#[test]
fn media_intents_reconcile_only_to_committed_hash_verified_results() {
    for point in [
        FaultPoint::IntentCommitted,
        FaultPoint::FinalLinked,
        FaultPoint::DirectoryFlushed,
        FaultPoint::PublishedCommitted,
    ] {
        let (f, mut case, view, map, _) = prepared(Codec::Hevc, Moov::Tail, Timing::Cfr);
        let staged = stage(&f, &mut case, &view, map);
        let validated = validate(
            &mut case,
            staged,
            helper(0),
            ValidationLimits::default(),
            &AtomicBool::new(false),
        )
        .unwrap();
        let root = f.dir.join("published");
        let mut publisher = Publisher::create(&root, PublicationLimits::default()).unwrap();
        assert!(matches!(
            publisher.publish_media(&mut case, &validated, point),
            Err(PublicationError::Injected(_))
        ));
        if point != FaultPoint::PublishedCommitted {
            assert!(publisher.published_media().unwrap().is_empty());
        }
        drop(publisher);
        let mut reopened = Publisher::open(&root, PublicationLimits::default()).unwrap();
        reopened.reconcile_media(&mut case).unwrap();
        let outputs = reopened.published_media().unwrap();
        assert_eq!(outputs.len(), 1);
        assert_eq!(fs::read(&outputs[0].path).unwrap(), f.ground_truth.payload);
    }
}

#[test]
fn changed_source_blocks_pending_media_intent_adoption() {
    use std::io::{Seek, SeekFrom, Write};
    let (f, mut case, view, map, _) = prepared(Codec::Avc, Moov::Tail, Timing::Cfr);
    let staged = stage(&f, &mut case, &view, map);
    let validated = validate(
        &mut case,
        staged,
        helper(0),
        ValidationLimits::default(),
        &AtomicBool::new(false),
    )
    .unwrap();
    let root = f.dir.join("published");
    let mut publisher = Publisher::create(&root, PublicationLimits::default()).unwrap();
    assert!(matches!(
        publisher.publish_media(&mut case, &validated, FaultPoint::IntentCommitted),
        Err(PublicationError::Injected(_))
    ));
    let mut image = fs::OpenOptions::new().write(true).open(&f.image).unwrap();
    image.seek(SeekFrom::Start(0)).unwrap();
    image.write_all(b"X").unwrap();
    image.sync_all().unwrap();
    assert!(publisher.reconcile_media(&mut case).is_err());
    assert!(publisher.published_media().unwrap().is_empty());
}

fn transformed() -> (Fixture, CaseSession, ValidatedArtifact) {
    let (f, mut case, view, _, _) = prepared(Codec::Avc, Moov::Head, Timing::Cfr);
    let layout = bmff::inspect_layout(
        &mut LogicalReader::new(&mut case, &view),
        ParseLimits::default(),
    )
    .unwrap();
    let plans = frameharbor::segments::plan(
        &mut LogicalReader::new(&mut case, &view),
        &layout,
        frameharbor::segments::SegmentLimits::default(),
    )
    .unwrap();
    assert!(!plans.plans.is_empty());
    let mut area =
        StagingArea::create(&f.dir.join("segment-staging"), 16 * 1024 * 1024, 16).unwrap();
    let staged = area
        .stage_segment(
            &mut case,
            &view,
            &plans.plans[0],
            ValidationLimits::default(),
        )
        .unwrap();
    let validated = validate(
        &mut case,
        staged,
        helper(0),
        ValidationLimits::default(),
        &AtomicBool::new(false),
    )
    .unwrap();
    (f, case, validated)
}

#[test]
fn transformed_receipt_roundtrips_sample_origins_without_exact_original_claims() {
    let (f, mut case, validated) = transformed();
    let ArtifactTransform::SamplePreserving(expected) = &validated.receipt().transform else {
        panic!("segment transform required")
    };
    assert!(!expected.mappings.is_empty());
    let mut publisher = Publisher::create(
        &f.dir.join("transformed-export"),
        PublicationLimits::default(),
    )
    .unwrap();
    let artifact = publisher
        .publish_media(&mut case, &validated, FaultPoint::None)
        .unwrap();
    let ArtifactKind::ValidatedMedia(e) = &artifact.kind else {
        panic!("typed media receipt required")
    };
    assert_eq!(e.receipt.transform, validated.receipt().transform);
    assert_eq!(e.receipt.claims, validated.receipt().claims);
    assert_eq!(e.receipt.claims.bytes, ByteProvenance::Synthesized);
    assert_eq!(
        e.receipt.claims.structure,
        Structure::RepairedWithRecordedEdits
    );
    assert_eq!(e.receipt.claims.temporal, Temporal::ConstrainedHypothesis);
    assert!(!e.receipt.original_file_bytes_preserved);
    assert_eq!(
        fs::read(&artifact.path).unwrap(),
        fs::read(validated.path()).unwrap()
    );
    assert_ne!(fs::read(&artifact.path).unwrap(), f.ground_truth.payload);
    drop(publisher);
    drop(case);
    let reopened = Publisher::open_readonly(
        &f.dir.join("transformed-export"),
        PublicationLimits::default(),
    )
    .unwrap();
    assert_eq!(reopened.media_receipts().unwrap(), [artifact]);
}

#[test]
fn transformed_publication_preserves_mapping_across_durable_boundaries() {
    for point in [
        FaultPoint::IntentCommitted,
        FaultPoint::FinalLinked,
        FaultPoint::DirectoryFlushed,
        FaultPoint::PublishedCommitted,
    ] {
        let (f, mut case, validated) = transformed();
        let root = f.dir.join("transformed-export");
        let mut publisher = Publisher::create(&root, PublicationLimits::default()).unwrap();
        assert!(matches!(
            publisher.publish_media(&mut case, &validated, point),
            Err(PublicationError::Injected(_))
        ));
        if point != FaultPoint::PublishedCommitted {
            assert!(publisher.media_receipts().unwrap().is_empty());
        }
        drop(publisher);
        let mut publisher = Publisher::open(&root, PublicationLimits::default()).unwrap();
        publisher.reconcile_media(&mut case).unwrap();
        let artifacts = publisher.published_media_with_session(&mut case).unwrap();
        assert_eq!(artifacts.len(), 1);
        let ArtifactKind::ValidatedMedia(e) = &artifacts[0].kind else {
            panic!("typed receipt required")
        };
        assert_eq!(e.receipt.transform, validated.receipt().transform);
    }
}

#[test]
fn changed_source_blocks_transformed_intent_adoption() {
    use std::io::{Seek, SeekFrom, Write};
    let (f, mut case, validated) = transformed();
    let root = f.dir.join("transformed-export");
    let mut publisher = Publisher::create(&root, PublicationLimits::default()).unwrap();
    assert!(matches!(
        publisher.publish_media(&mut case, &validated, FaultPoint::IntentCommitted),
        Err(PublicationError::Injected(_))
    ));
    let mut source = fs::OpenOptions::new().write(true).open(&f.image).unwrap();
    source.seek(SeekFrom::Start(0)).unwrap();
    source.write_all(b"changed").unwrap();
    source.sync_all().unwrap();
    assert!(publisher.reconcile_media(&mut case).is_err());
    assert!(publisher.media_receipts().unwrap().is_empty());
}

#[test]
fn transformed_export_retains_known_gap_without_requiring_missing_source_bytes() {
    let (f, case, mut view, map, _) = prepared(Codec::Avc, Moov::Head, Timing::Cfr);
    let video = map.tracks.iter().find(|t| t.handler == *b"vide").unwrap();
    let lost = &video.samples[1];
    let hole = lost.source_spans[0].image.start();
    drop(case);
    let gap_map = f.dir.join("segment-gap.map");
    let size = f.ground_truth.logical_image_bytes;
    fs::write(
        &gap_map,
        format!(
            "0x0 + 1\n0x0 0x{hole:x} +\n0x{hole:x} 0x1 -\n0x{:x} 0x{:x} +\n",
            hole + 1,
            size - hole - 1
        ),
    )
    .unwrap();
    let mut case = CaseSession::create(
        Ledger::open(&f.dir.join("segment-gap.sqlite"), 16 * 1024 * 1024).unwrap(),
        SessionInput {
            image: f.image.clone(),
            map: gap_map,
            binding: CoordinateBinding::new(
                ByteRange::new(0, size).unwrap(),
                0,
                size,
                BindingEvidence::OperatorAttested,
            )
            .unwrap(),
            limits: ReadLimits {
                per_read: 1024 * 1024,
                total: 512 * 1024 * 1024,
                receipts: 4096,
            },
        },
        CaseConfig {
            total_millis: 120000,
            shutdown_reserve_millis: 1000,
            caps: ResourceCaps {
                read_bytes: 512 * 1024 * 1024,
                memory_bytes: 512 * 1024 * 1024,
                index_bytes: 16 * 1024 * 1024,
                output_bytes: 16 * 1024 * 1024,
                visited_postings: 0,
                transitions: 0,
                decoder_seconds: 600,
            },
        },
    )
    .unwrap();
    view.owner.source = case.source_generation();
    let layout = bmff::inspect_layout(
        &mut LogicalReader::new(&mut case, &view),
        ParseLimits::default(),
    )
    .unwrap();
    let plans = frameharbor::segments::plan(
        &mut LogicalReader::new(&mut case, &view),
        &layout,
        frameharbor::segments::SegmentLimits::default(),
    )
    .unwrap();
    assert!(!plans.plans.is_empty());
    let mut area =
        StagingArea::create(&f.dir.join("gapped-segment-staging"), 16 * 1024 * 1024, 16).unwrap();
    let staged = area
        .stage_segment(
            &mut case,
            &view,
            &plans.plans[0],
            ValidationLimits::default(),
        )
        .unwrap();
    let validated = validate(
        &mut case,
        staged,
        helper(0),
        ValidationLimits::default(),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(
        matches!(validated.receipt().claims.coverage,Coverage::KnownGaps{recovered,listed} if recovered<listed)
    );
    let mut publisher =
        Publisher::create(&f.dir.join("gapped-export"), PublicationLimits::default()).unwrap();
    let artifact = publisher
        .publish_media(&mut case, &validated, FaultPoint::None)
        .unwrap();
    let ArtifactKind::ValidatedMedia(e) = artifact.kind else {
        panic!("typed media receipt required")
    };
    let ArtifactTransform::SamplePreserving(segment) = e.receipt.transform else {
        panic!("segment evidence required")
    };
    assert!(
        segment
            .mappings
            .iter()
            .filter(|m| m.track_id == video.id)
            .all(|m| m.original.ordinal > lost.ordinal)
    );
    assert!(segment.mappings.iter().all(|m| {
        m.original
            .source_spans
            .iter()
            .all(|s| !s.image.contains(ByteRange::from_len(hole, 1).unwrap()))
    }));
}

fn portable_work(case: &mut CaseSession, artifact: &Artifact, include_link: bool) -> String {
    let spec = frameharbor::ledger::WorkSpec {
        source: case.source_generation(),
        algorithm: "qualified-media-export-v2".into(),
        stage: "media-export".into(),
        parameters: std::collections::BTreeMap::from([("artifact".into(), artifact.id.clone())]),
    };
    case.with_ledger(65536, |ledger| {
        let work = ledger.submit(&spec)?;
        ledger.append_receipt(
            &work,
            "qualified-media-outcome-v2",
            b"bounded media work finished with explicit remainder",
        )?;
        if include_link {
            ledger.append_receipt(&work, "portable-artifact-v1", artifact.id.as_bytes())?;
        }
        Ok(work)
    })
    .unwrap()
}

#[test]
fn sealed_retirement_releases_only_portably_retained_epoch_reads_without_resetting_charges() {
    let (f, mut case, view, map, _) = prepared(Codec::Avc, Moov::Head, Timing::Cfr);
    let baseline = case.source_snapshot().unwrap().consumed().len();
    let before = case.snapshot().unwrap().used;
    let epoch = case.begin_read_epoch().unwrap();
    for position in 0..900 {
        case.with_read(ByteRange::from_len(position, 1).unwrap(), |_| ())
            .unwrap();
    }
    assert!(case.source_snapshot().unwrap().consumed().len() > baseline);
    let staged = stage(&f, &mut case, &view, map);
    let validated = validate(
        &mut case,
        staged,
        helper(0),
        ValidationLimits::default(),
        &AtomicBool::new(false),
    )
    .unwrap();
    let mut publisher = Publisher::create(
        &f.dir.join("retirement-export"),
        PublicationLimits::default(),
    )
    .unwrap();
    let artifact = publisher
        .publish_media(&mut case, &validated, FaultPoint::None)
        .unwrap();
    let work = portable_work(&mut case, &artifact, true);
    assert!(
        publisher
            .retire_source_epoch(&mut case, epoch, &artifact, &work)
            .unwrap()
    );
    assert_eq!(case.source_snapshot().unwrap().consumed().len(), baseline);
    let after = case.snapshot().unwrap().used;
    assert!(after.read_bytes > before.read_bytes);
    assert!(after.output_bytes > before.output_bytes);
    let ArtifactKind::ValidatedMedia(e) = &artifact.kind else {
        panic!("typed receipt required")
    };
    assert!(
        e.provenance
            .consumed
            .iter()
            .filter(|r| r.image.len() == 1 && r.image.start() < 900)
            .count()
            >= 900
    );
    let epoch = case.begin_read_epoch().unwrap();
    for position in 1000..1900 {
        case.with_read(ByteRange::from_len(position, 1).unwrap(), |_| ())
            .unwrap();
    }
    assert!(case.source_snapshot().unwrap().consumed().len() >= baseline + 900);
    assert!(case.snapshot().unwrap().used.read_bytes >= after.read_bytes + 900);
    case.retain_read_epoch(epoch).unwrap();
}

#[test]
fn retirement_retains_reads_not_covered_by_committed_artifact_evidence() {
    let (f, mut case, validated) = transformed();
    let mut publisher = Publisher::create(
        &f.dir.join("retirement-export"),
        PublicationLimits::default(),
    )
    .unwrap();
    let artifact = publisher
        .publish_media(&mut case, &validated, FaultPoint::None)
        .unwrap();
    let work = portable_work(&mut case, &artifact, true);
    let epoch = case.begin_read_epoch().unwrap();
    case.with_read(ByteRange::new(0, 32).unwrap(), |_| ())
        .unwrap();
    let observed = case.source_snapshot().unwrap().consumed().to_vec();
    assert!(
        !publisher
            .retire_source_epoch(&mut case, epoch, &artifact, &work)
            .unwrap()
    );
    assert_eq!(case.source_snapshot().unwrap().consumed(), observed);
    let epoch = case.begin_read_epoch().unwrap();
    case.retain_read_epoch(epoch).unwrap();
}

#[test]
fn retirement_rejects_missing_work_links_forged_artifacts_pending_rows_and_corruption() {
    let (f, mut case, validated) = transformed();
    let root = f.dir.join("retirement-export");
    let mut publisher = Publisher::create(&root, PublicationLimits::default()).unwrap();
    let artifact = publisher
        .publish_media(&mut case, &validated, FaultPoint::None)
        .unwrap();
    let work = portable_work(&mut case, &artifact, false);
    for mode in 0..5 {
        let epoch = case.begin_read_epoch().unwrap();
        case.with_read(ByteRange::from_len(32 + mode, 1).unwrap(), |_| ())
            .unwrap();
        let observed = case.source_snapshot().unwrap().consumed().to_vec();
        let mut attempted = artifact.clone();
        let mut attempted_work = work.clone();
        if mode == 1 {
            case.with_ledger(65536, |l| {
                l.append_receipt(&work, "portable-artifact-v1", artifact.id.as_bytes())
            })
            .unwrap();
            attempted.bytes += 1;
        } else if mode == 2 {
            attempted_work = case
                .with_ledger(65536, |l| {
                    let wrong = frameharbor::ledger::WorkSpec {
                        source: SourceGeneration("other-generation".into()),
                        algorithm: "qualified-media-export-v2".into(),
                        stage: "media-export".into(),
                        parameters: std::collections::BTreeMap::new(),
                    };
                    let id = l.submit(&wrong)?;
                    l.append_receipt(&id, "qualified-media-outcome-v2", b"completed")?;
                    l.append_receipt(&id, "portable-artifact-v1", artifact.id.as_bytes())?;
                    Ok(id)
                })
                .unwrap();
        } else if mode == 3 {
            let db = rusqlite::Connection::open(root.join("publication.sqlite")).unwrap();
            db.execute(
                "UPDATE artifacts SET state='intent' WHERE id=?1",
                [&artifact.id],
            )
            .unwrap();
        } else if mode == 4 {
            let db = rusqlite::Connection::open(root.join("publication.sqlite")).unwrap();
            db.execute(
                "UPDATE artifacts SET state='published' WHERE id=?1",
                [&artifact.id],
            )
            .unwrap();
            fs::write(&artifact.path, b"corrupted portable bytes").unwrap();
        }
        assert!(
            publisher
                .retire_source_epoch(&mut case, epoch, &attempted, &attempted_work)
                .is_err()
        );
        assert_eq!(case.source_snapshot().unwrap().consumed(), observed);
    }
    let epoch = case.begin_read_epoch().unwrap();
    case.retain_read_epoch(epoch).unwrap();
}

#[test]
fn portable_receipt_retains_checkpointed_source_archive_references() {
    use frameharbor::checkpoint::{CheckpointLimits, CheckpointStore};
    let (f, mut case, view, map, _) = prepared(Codec::Avc, Moov::Head, Timing::Cfr);
    let mut frontiers = CheckpointStore::create(
        &f.dir.join("archived-frontiers"),
        CheckpointLimits::default(),
    )
    .unwrap();
    let archive = case.archive_dependencies(&mut frontiers).unwrap().unwrap();
    assert_eq!(case.source_snapshot().unwrap().archive_refs(), &[archive]);
    let staged = stage(&f, &mut case, &view, map);
    let validated = validate(
        &mut case,
        staged,
        helper(0),
        ValidationLimits::default(),
        &AtomicBool::new(false),
    )
    .unwrap();
    let root = f.dir.join("archive-linked-export");
    let mut publisher = Publisher::create(&root, PublicationLimits::default()).unwrap();
    let artifact = publisher
        .publish_media(&mut case, &validated, FaultPoint::None)
        .unwrap();
    let ArtifactKind::ValidatedMedia(e) = &artifact.kind else {
        panic!("media evidence")
    };
    assert_eq!(e.provenance.archive_refs, vec![archive]);
    drop(publisher);
    drop(validated);
    drop(case);
    let reopened = Publisher::open_readonly(&root, PublicationLimits::default()).unwrap();
    let stored = reopened.media_receipts().unwrap();
    assert_eq!(stored, vec![artifact]);
}
