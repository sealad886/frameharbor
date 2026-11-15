#![cfg(target_os = "macos")]
#[allow(dead_code)]
mod common;
use common::{
    filesystems::{Damage, Fixture, FsKind, Layout},
    media::{Codec, MediaFixture, Moov, Timing},
};
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
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{OnceLock, atomic::AtomicBool},
    time::{Duration, Instant},
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
    prepared_media(MediaFixture::generate(codec, moov, timing).unwrap())
}
fn prepared_media(
    media: MediaFixture,
) -> (
    Fixture,
    CaseSession,
    LogicalFileView,
    SampleMap,
    MediaFixture,
) {
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
    let map = bmff::inspect_target_layout(
        &mut LogicalReader::new(&mut case, &view),
        ParseLimits::default(),
    )
    .unwrap()
    .into_exact()
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
fn actual_staged_avc_hevc_audio_and_timing_match_source_then_decode_in_full() {
    for codec in [Codec::Avc, Codec::Hevc] {
        for moov in [Moov::Head, Moov::Tail] {
            for timing in [Timing::Cfr, Timing::Vfr] {
                let (f, mut case, view, map, media) = prepared(codec, moov, timing);
                let expected = media.packets.len() as u64;
                let map_before = fs::read(&f.map).unwrap();
                let modified_before = fs::metadata(&f.image).unwrap().modified().unwrap();
                let artifact = stage(&f, &mut case, &view, map);
                assert_eq!(
                    artifact.sha256(),
                    <[u8; 32]>::from(Sha256::digest(&f.ground_truth.payload))
                );
                assert_eq!(fs::read(artifact.path()).unwrap(), f.ground_truth.payload);
                let mut cleanup = None;
                let validated = validate_observed(
                    &mut case,
                    artifact,
                    helper(0),
                    ValidationLimits::default(),
                    &AtomicBool::new(false),
                    |receipt| cleanup = Some(receipt),
                )
                .unwrap();
                assert!(cleanup.unwrap().reaped);
                let receipt = validated.receipt();
                assert_eq!(receipt.samples, expected);
                assert_eq!(receipt.packets, expected);
                assert_eq!(receipt.frames, expected);
                assert!(receipt.native_reads > 0);
                assert!(receipt.staged_read_bytes >= validated.bytes());
                assert!(receipt.audio_present);
                assert!(receipt.original_file_bytes_preserved);
                assert_eq!(receipt.claims.temporal, Temporal::OriginalMappingSupported);
                assert_eq!(
                    receipt.claims.coverage,
                    Coverage::CompleteWithinKnownLayout { samples: expected }
                );
                assert!(receipt.claims.require_playable().is_ok());
                assert_eq!(fs::read(&f.map).unwrap(), map_before);
                assert_eq!(
                    fs::metadata(&f.image).unwrap().modified().unwrap(),
                    modified_before
                );
            }
        }
    }
}

#[test]
fn a_decodable_stage_cannot_validate_wrong_asserted_order_timing_configuration_or_audio() {
    for mutation in 0..5 {
        let (f, mut case, view, mut map, _) = prepared(Codec::Avc, Moov::Head, Timing::Vfr);
        match mutation {
            0 => map.tracks[0].samples[0].sha256[0] ^= 1,
            1 => map.tracks[0].samples[0].pts += 1,
            2 => map.tracks[0].samples[1].ordinal = 0,
            3 => map.tracks[0].descriptions[0].configuration[0] ^= 1,
            _ => map.tracks.retain(|track| track.handler != *b"soun"),
        }
        let artifact = stage(&f, &mut case, &view, map);
        assert!(matches!(
            validate(
                &mut case,
                artifact,
                Path::new("/no-helper-needed"),
                ValidationLimits::default(),
                &AtomicBool::new(false)
            ),
            Err(ValidationError::SampleMismatch(_))
        ));
    }
}

#[test]
fn modified_stage_and_output_quota_fail_before_native_publication() {
    let (f, mut case, view, map, _) = prepared(Codec::Avc, Moov::Tail, Timing::Cfr);
    let mut area = StagingArea::create(&f.dir.join("small-staging"), view.size - 1, 1).unwrap();
    assert!(
        area.stage_exact(&mut case, &view, map.clone(), ValidationLimits::default())
            .is_err()
    );
    assert_eq!(
        fs::read_dir(f.dir.join("small-staging")).unwrap().count(),
        1
    );
    let artifact = stage(&f, &mut case, &view, map);
    let mut bytes = fs::read(artifact.path()).unwrap();
    let n = bytes.len();
    bytes[n - 1] ^= 1;
    fs::write(artifact.path(), bytes).unwrap();
    assert!(matches!(
        validate(
            &mut case,
            artifact,
            Path::new("/no-helper-needed"),
            ValidationLimits::default(),
            &AtomicBool::new(false)
        ),
        Err(ValidationError::Changed)
    ));
}

#[test]
fn native_validation_timeout_and_failure_reap_the_actual_helper() {
    for mode in [1, 2, 3] {
        let executable = helper(mode);
        let (f, mut case, view, map, _) = prepared(Codec::Avc, Moov::Head, Timing::Cfr);
        let artifact = stage(&f, &mut case, &view, map);
        let limits = ValidationLimits {
            wall: Duration::from_secs(2),
            ..ValidationLimits::default()
        };
        let mut cleanup = None;
        let started = Instant::now();
        let result = validate_observed(
            &mut case,
            artifact,
            executable,
            limits,
            &AtomicBool::new(false),
            |receipt| cleanup = Some(receipt),
        );
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_secs(5));
        let receipt = cleanup.expect("actual native helper must spawn");
        assert!(receipt.reaped);
        assert!(receipt.wait_error.is_none());
    }
}

#[test]
fn transformed_avc_hevc_segments_retain_source_identity_and_disclose_synthesis() {
    use frameharbor::segments::{self, SegmentLimits};
    for codec in [Codec::Avc, Codec::Hevc] {
        let (f, mut case, view, _map, _) = prepared_media(
            MediaFixture::generate_closed_idr(codec, Moov::Tail, Timing::Vfr).unwrap(),
        );
        let layout = bmff::inspect_layout(
            &mut LogicalReader::new(&mut case, &view),
            ParseLimits::default(),
        )
        .unwrap();
        let plans = segments::plan(
            &mut LogicalReader::new(&mut case, &view),
            &layout,
            SegmentLimits::default(),
        )
        .unwrap();
        assert!(!plans.plans.is_empty(), "{codec:?}");
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
        let receipt = validated.receipt();
        assert!(!receipt.original_file_bytes_preserved);
        assert_eq!(receipt.claims.bytes, ByteProvenance::Synthesized);
        assert_eq!(
            receipt.claims.structure,
            Structure::RepairedWithRecordedEdits
        );
        assert_eq!(receipt.claims.temporal, Temporal::ConstrainedHypothesis);
        let ArtifactTransform::SamplePreserving(evidence) = &receipt.transform else {
            panic!("missing transform provenance")
        };
        assert_eq!(evidence.mappings.len() as u64, receipt.samples);
        for mapping in &evidence.mappings {
            let target = layout
                .tracks
                .iter()
                .find(|t| t.id == mapping.track_id)
                .unwrap();
            let sample = &target.samples[mapping.original.ordinal as usize];
            assert_eq!(mapping.original.logical, sample.logical);
            assert_eq!(mapping.original.source_spans, sample.source_spans);
            assert_eq!(
                sample.availability,
                bmff::SampleAvailability::Available {
                    sha256: mapping.original.sha256
                }
            );
        }
    }
}

#[test]
fn a_decodable_plan_with_forged_original_timing_cannot_obtain_a_staged_token() {
    use frameharbor::segments::{self, SegmentLimits};
    let (f, mut case, view, _, _) = prepared(Codec::Avc, Moov::Head, Timing::Cfr);
    let mut layout = bmff::inspect_layout(
        &mut LogicalReader::new(&mut case, &view),
        ParseLimits::default(),
    )
    .unwrap();
    // Payload remains decodable, but the caller has changed asserted source PTS.
    for track in &mut layout.tracks {
        for sample in &mut track.samples {
            sample.pts += 1;
        }
    }
    let plans = segments::plan(
        &mut LogicalReader::new(&mut case, &view),
        &layout,
        SegmentLimits::default(),
    )
    .unwrap();
    assert!(!plans.plans.is_empty());
    let mut area =
        StagingArea::create(&f.dir.join("forged-segment-staging"), 16 * 1024 * 1024, 16).unwrap();
    assert!(matches!(
        area.stage_segment(
            &mut case,
            &view,
            &plans.plans[0],
            ValidationLimits::default()
        ),
        Err(ValidationError::SampleMismatch(_))
    ));
    assert_eq!(
        fs::read_dir(f.dir.join("forged-segment-staging"))
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn fragmented_target_segments_are_independently_mapped_and_fully_validated() {
    use frameharbor::segments::{self, SegmentLimits};
    for codec in [Codec::Avc, Codec::Hevc] {
        let (f, mut case, view, _, _) = prepared_media(
            MediaFixture::generate_closed_idr(codec, Moov::Fragmented, Timing::Vfr).unwrap(),
        );
        let layout = bmff::inspect_target_layout(
            &mut LogicalReader::new(&mut case, &view),
            ParseLimits::default(),
        )
        .unwrap();
        let plans = segments::plan(
            &mut LogicalReader::new(&mut case, &view),
            &layout,
            SegmentLimits::default(),
        )
        .unwrap();
        assert!(!plans.plans.is_empty());
        let mut area = StagingArea::create(
            &f.dir.join("fragment-segment-staging"),
            16 * 1024 * 1024,
            16,
        )
        .unwrap();
        let stage = area
            .stage_segment(
                &mut case,
                &view,
                &plans.plans[0],
                ValidationLimits::default(),
            )
            .unwrap();
        let validated = validate(
            &mut case,
            stage,
            helper(0),
            ValidationLimits::default(),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(!validated.receipt().original_file_bytes_preserved);
        assert!(matches!(
            validated.receipt().transform,
            ArtifactTransform::SamplePreserving(_)
        ));
        assert_eq!(validated.receipt().claims.decode, Decode::StrictPass);
    }
}

#[test]
fn clearing_real_audio_preroll_groups_cannot_authorize_a_segment_transform() {
    use frameharbor::segments::{self, SegmentLimits};
    let (f, mut case, view, _, _) = prepared(Codec::Avc, Moov::Head, Timing::Cfr);
    let mut layout = bmff::inspect_target_layout(
        &mut LogicalReader::new(&mut case, &view),
        ParseLimits::default(),
    )
    .unwrap();
    let audio = layout
        .tracks
        .iter_mut()
        .find(|t| t.handler == *b"soun")
        .unwrap();
    assert!(
        !audio.sample_groups.is_empty(),
        "fixture must retain actual target preroll groups"
    );
    audio.sample_groups.clear();
    let plans = segments::plan(
        &mut LogicalReader::new(&mut case, &view),
        &layout,
        SegmentLimits::default(),
    )
    .unwrap();
    assert!(!plans.plans.is_empty());
    let mut area =
        StagingArea::create(&f.dir.join("forged-group-staging"), 16 * 1024 * 1024, 16).unwrap();
    assert!(
        area.stage_segment(
            &mut case,
            &view,
            &plans.plans[0],
            ValidationLimits::default()
        )
        .is_err()
    );
    assert_eq!(
        fs::read_dir(f.dir.join("forged-group-staging"))
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn invalid_parser_memory_envelope_is_rejected_before_arithmetic_or_writes() {
    let (f, mut case, view, map, _) = prepared(Codec::Avc, Moov::Head, Timing::Cfr);
    let mut area = StagingArea::create(
        &f.dir.join("invalid-envelope-staging"),
        16 * 1024 * 1024,
        16,
    )
    .unwrap();
    let limits = ValidationLimits {
        parse: ParseLimits {
            memory_bytes: u64::MAX,
            ..ParseLimits::default()
        },
        ..ValidationLimits::default()
    };
    assert!(matches!(
        area.stage_exact(&mut case, &view, map, limits),
        Err(ValidationError::ResourceLimit)
    ));
    assert_eq!(
        fs::read_dir(f.dir.join("invalid-envelope-staging"))
            .unwrap()
            .count(),
        1
    );
}
