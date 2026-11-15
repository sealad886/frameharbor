#[allow(dead_code)]
mod common;
use common::media::{Codec, MediaFixture, Moov, Timing};
use frameharbor::{
    bmff::{self, ParseLimits, ReadAt, SampleAvailability, SliceReader},
    segments::{self, SegmentLimits, TransformLabel},
};
use sha2::{Digest, Sha256};
fn output_map(bytes: &[u8]) -> bmff::SampleMap {
    bmff::inspect(&mut SliceReader::new(bytes), ParseLimits::default()).unwrap()
}
#[test]
fn bounded_mux_preserves_selected_encoded_samples_timing_configuration_and_common_origin() {
    for codec in [Codec::Avc, Codec::Hevc] {
        let fixture = if codec == Codec::Hevc {
            MediaFixture::generate_closed_idr(codec, Moov::Head, Timing::Vfr)
        } else {
            MediaFixture::generate(codec, Moov::Head, Timing::Vfr)
        }
        .unwrap();
        let target = bmff::inspect_layout(
            &mut SliceReader::new(&fixture.bytes),
            ParseLimits::default(),
        )
        .unwrap();
        let planned = segments::plan(
            &mut SliceReader::new(&fixture.bytes),
            &target,
            SegmentLimits::default(),
        )
        .unwrap();
        assert!(
            !planned.plans.is_empty(),
            "no closed-IDR interval for {codec:?}"
        );
        for plan in &planned.plans {
            let mut bytes = Vec::new();
            segments::write(&mut SliceReader::new(&fixture.bytes), plan, &mut bytes).unwrap();
            assert_eq!(bytes.len() as u64, plan.output_bytes());
            let observed = output_map(&bytes);
            assert_eq!(observed, *plan.expected_output());
            assert!(plan.labels().contains(&TransformLabel::SynthesizedHeaders));
            for (source, output) in plan.selected_source().tracks.iter().zip(&observed.tracks) {
                assert_eq!(source.samples.len(), output.samples.len());
                let origin = source.samples[0].dts;
                for (original, remuxed) in source.samples.iter().zip(&output.samples) {
                    assert_eq!(remuxed.sha256, original.sha256);
                    assert_eq!(
                        (remuxed.dts, remuxed.pts, remuxed.duration),
                        (
                            original.dts - origin,
                            original.pts - i128::from(origin),
                            original.duration
                        )
                    );
                    assert_eq!(
                        &bytes[remuxed.logical.start() as usize..remuxed.logical.end() as usize],
                        &fixture.bytes
                            [original.logical.start() as usize..original.logical.end() as usize]
                    );
                }
            }
            // Independent native strict decoder accepts the synthesized MP4.
            let path = fixture.dir.join(format!(
                "segment-{}.mp4",
                plan.selected_source().tracks[0].samples[0].ordinal
            ));
            std::fs::write(&path, &bytes).unwrap();
            let mut child = std::process::Command::new("/opt/homebrew/opt/ffmpeg-full/bin/ffmpeg")
                .args([
                    "-hide_banner",
                    "-nostdin",
                    "-v",
                    "error",
                    "-xerror",
                    "-err_detect",
                    "explode",
                    "-i",
                ])
                .arg(path)
                .args(["-f", "null", "-"])
                .spawn()
                .unwrap();
            let started = std::time::Instant::now();
            let status = loop {
                if let Some(status) = child.try_wait().unwrap() {
                    break status;
                }
                if started.elapsed() > std::time::Duration::from_secs(10) {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("strict fixture decoder deadline");
                }
                std::thread::yield_now();
            };
            assert!(status.success());
        }
    }
}
#[test]
fn hole_discards_incomplete_gop_and_salvages_next_closed_idr_with_original_identity() {
    let fixture = MediaFixture::generate(Codec::Avc, Moov::Head, Timing::Cfr).unwrap();
    let mut target = bmff::inspect_layout(
        &mut SliceReader::new(&fixture.bytes),
        ParseLimits::default(),
    )
    .unwrap();
    let video = target
        .tracks
        .iter_mut()
        .find(|t| t.handler == *b"vide")
        .unwrap();
    assert!(video.samples.iter().filter(|s| s.is_sync).count() >= 2);
    let second_idr = video
        .samples
        .iter()
        .position(|s| s.ordinal > 0 && s.is_sync)
        .unwrap();
    let hole = video.samples[2].logical;
    video.samples[2].availability = SampleAvailability::Unavailable {
        reason: bmff::AcquisitionMissing { image: hole },
    };
    struct NoHoleRead<'a> {
        reader: SliceReader<'a>,
        hole: frameharbor::contracts::ByteRange,
    }
    impl ReadAt for NoHoleRead<'_> {
        fn len(&self) -> u64 {
            self.reader.len()
        }
        fn read_exact(&mut self, offset: u64, bytes: &mut [u8]) -> bmff::Result<()> {
            assert!(
                frameharbor::contracts::ByteRange::from_len(offset, bytes.len() as u64)
                    .unwrap()
                    .intersection(self.hole)
                    .is_none(),
                "planner read unavailable bytes"
            );
            self.reader.read_exact(offset, bytes)
        }
    }
    let mut reader = NoHoleRead {
        reader: SliceReader::new(&fixture.bytes),
        hole,
    };
    let plans = segments::plan(&mut reader, &target, SegmentLimits::default()).unwrap();
    assert_eq!(plans.plans.len(), 1);
    let plan = &plans.plans[0];
    assert_eq!(
        plan.selected_source().tracks[0].samples[0].ordinal,
        second_idr as u64
    );
    assert!(
        plan.selected_source().tracks[0]
            .samples
            .iter()
            .all(|s| s.ordinal >= second_idr as u64)
    );
    let mut bytes = Vec::new();
    segments::write(&mut reader, plan, &mut bytes).unwrap();
    let remuxed = output_map(&bytes);
    assert_eq!(remuxed.tracks[0].samples[0].dts, 0);
    assert_eq!(
        remuxed.tracks[0].samples[0].sha256,
        plan.selected_source().tracks[0].samples[0].sha256
    );
    assert!(plans.remainder.iter().any(|r| r.selected < r.total));
    // Mutated known payload cannot be written under an old sealed plan.
    let mut changed = fixture.bytes.clone();
    let offset = plan.selected_source().tracks[0].samples[0].logical.start() as usize;
    changed[offset] ^= 1;
    assert!(segments::write(&mut SliceReader::new(&changed), plan, &mut Vec::new()).is_err());
    assert_ne!(Sha256::digest(&changed), Sha256::digest(&fixture.bytes));
}

fn cache_source(
    f: &common::filesystems::Fixture,
) -> (
    frameharbor::session::CaseSession,
    frameharbor::filesystem::LogicalFileView,
) {
    use frameharbor::{
        acquisition::ReadLimits,
        contracts::{BindingEvidence, ByteRange, CoordinateBinding, LogicalOwner, ResourceCaps},
        control::CaseConfig,
        filesystem::{ExtentEvidence, ExtentRun, LogicalFileView},
        ledger::Ledger,
        session::{CaseSession, SessionInput},
    };
    let size = f.ground_truth.logical_image_bytes;
    let session = CaseSession::create(
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
                total: 16 * 1024 * 1024,
                receipts: 1024,
            },
        },
        CaseConfig {
            total_millis: 60000,
            shutdown_reserve_millis: 1000,
            caps: ResourceCaps {
                read_bytes: 16 * 1024 * 1024,
                memory_bytes: 512 * 1024 * 1024,
                index_bytes: 1024 * 1024,
                output_bytes: 0,
                visited_postings: 0,
                transitions: 0,
                decoder_seconds: 0,
            },
        },
    )
    .unwrap();
    let view = LogicalFileView {
        owner: LogicalOwner {
            source: session.source_generation(),
            filesystem: "synthetic-certified-geometry".into(),
            file_identity: "ground-truth-file".into(),
        },
        size: f.ground_truth.payload.len() as u64,
        metadata_flags: 5,
        name_flags: 1,
        attribute_flags: 3,
        initialized_size: f.ground_truth.payload.len() as u64,
        skip_length: 0,
        runs: f
            .ground_truth
            .spans
            .iter()
            .map(|s| ExtentRun {
                logical: ByteRange::from_len(s.logical_offset, s.len).unwrap(),
                image: Some(ByteRange::from_len(s.image_offset, s.len).unwrap()),
                raw_flags: 0,
                allocation_consistent: true,
            })
            .collect(),
        evidence: ExtentEvidence::MetadataAllocated,
    };
    (session, view)
}

#[test]
fn real_acquisition_mask_hole_salvages_later_closed_gop_without_reading_stale_bytes() {
    use common::filesystems::{Damage, Fixture, FsKind, Layout};
    let media = MediaFixture::generate(Codec::Avc, Moov::Head, Timing::Cfr).unwrap();
    let intact =
        bmff::inspect_layout(&mut SliceReader::new(&media.bytes), ParseLimits::default()).unwrap();
    let video = intact
        .tracks
        .iter()
        .find(|t| t.handler == *b"vide")
        .unwrap();
    let missing = video.samples[2].logical;
    let idr = video
        .samples
        .iter()
        .find(|s| s.ordinal > 0 && s.is_sync)
        .unwrap()
        .ordinal;
    let mut image = Fixture::build(FsKind::Fat32, Layout::Fragmented, Damage::None).unwrap();
    image
        .replace_payload(&media.padded_bytes(32768).unwrap())
        .unwrap();
    let span = image
        .ground_truth
        .spans
        .iter()
        .find(|s| s.logical_offset <= missing.start() && s.logical_offset + s.len >= missing.end())
        .unwrap();
    let start = span.image_offset + missing.start() - span.logical_offset;
    let end = start + missing.len();
    let size = image.ground_truth.logical_image_bytes;
    std::fs::write(
        &image.map,
        format!(
            "0 + 1\n0 {start} +\n{start} {} -\n{end} {} +\n",
            end - start,
            size - end
        ),
    )
    .unwrap();
    let (mut session, view) = cache_source(&image);
    let mut reader = bmff::LogicalReader::new(&mut session, &view);
    let target = bmff::inspect_layout(&mut reader, ParseLimits::default()).unwrap();
    assert!(matches!(
        target
            .tracks
            .iter()
            .find(|t| t.handler == *b"vide")
            .unwrap()
            .samples[2]
            .availability,
        SampleAvailability::Unavailable { .. }
    ));
    let plans = segments::plan(&mut reader, &target, SegmentLimits::default()).unwrap();
    assert_eq!(plans.plans.len(), 1);
    let plan = &plans.plans[0];
    assert_eq!(plan.owner(), Some(&view.owner));
    assert_eq!(plan.selected_source().tracks[0].samples[0].ordinal, idr);
    assert!(
        !plan.selected_source().tracks[0].samples[0]
            .source_spans
            .is_empty()
    );
    let mut bytes = Vec::new();
    segments::write(&mut reader, plan, &mut bytes).unwrap();
    let output = output_map(&bytes);
    assert_eq!(
        output.tracks[0].samples.len(),
        plan.selected_source().tracks[0].samples.len()
    );
    for (actual, original) in output.tracks[0]
        .samples
        .iter()
        .zip(&plan.selected_source().tracks[0].samples)
    {
        assert_eq!(actual.sha256, original.sha256);
    }
}

#[test]
fn conservative_hevc_profile_leaves_leading_picture_idr_and_open_gop_unresolved() {
    let fixture = MediaFixture::generate(Codec::Hevc, Moov::Head, Timing::Vfr).unwrap();
    let target = bmff::inspect_layout(
        &mut SliceReader::new(&fixture.bytes),
        ParseLimits::default(),
    )
    .unwrap();
    let plans = segments::plan(
        &mut SliceReader::new(&fixture.bytes),
        &target,
        SegmentLimits::default(),
    )
    .unwrap();
    assert!(plans.plans.is_empty());
    assert!(plans.remainder.iter().all(|r| r.selected == 0));
    let tight = SegmentLimits {
        read_bytes: 1,
        ..SegmentLimits::default()
    };
    assert!(matches!(
        segments::plan(&mut SliceReader::new(&fixture.bytes), &target, tight),
        Err(bmff::BmffError::ResourceLimit)
    ));
}

#[test]
fn receipt_limits_split_only_at_closed_idr_and_leave_oversized_gop_unresolved() {
    let fixture = MediaFixture::generate(Codec::Avc, Moov::Head, Timing::Cfr).unwrap();
    let target = bmff::inspect_layout(
        &mut SliceReader::new(&fixture.bytes),
        ParseLimits::default(),
    )
    .unwrap();
    let limits = SegmentLimits {
        per_segment_samples: 12,
        ..SegmentLimits::default()
    };
    let planned = segments::plan(&mut SliceReader::new(&fixture.bytes), &target, limits).unwrap();
    assert_eq!(planned.plans.len(), 2);
    assert_eq!(
        planned.plans[0].selected_source().tracks[0].samples[0].ordinal,
        0
    );
    assert_eq!(
        planned.plans[1].selected_source().tracks[0].samples[0].ordinal,
        12
    );
    for plan in &planned.plans {
        assert!(
            plan.expected_output()
                .tracks
                .iter()
                .map(|t| t.samples.len())
                .sum::<usize>()
                <= 12
        );
    }
    let tiny = SegmentLimits {
        per_segment_samples: 11,
        ..SegmentLimits::default()
    };
    let remainder = segments::plan(&mut SliceReader::new(&fixture.bytes), &target, tiny).unwrap();
    assert!(remainder.plans.is_empty());
    assert!(remainder.remainder.iter().all(|r| r.selected == 0));
}
#[test]
fn fresh_audio_selection_rejects_forged_omission_evidence() {
    let fixture = MediaFixture::generate(Codec::Avc, Moov::Fragmented, Timing::Cfr).unwrap();
    let target = bmff::inspect_target_layout(
        &mut SliceReader::new(&fixture.bytes),
        ParseLimits::default(),
    )
    .unwrap();
    let mut forged = target.clone();
    let audio = forged
        .tracks
        .iter_mut()
        .find(|t| t.handler == *b"soun")
        .unwrap();
    let first = audio.samples[0].logical;
    audio.samples[0].availability = SampleAvailability::Unavailable {
        reason: bmff::AcquisitionMissing { image: first },
    };
    let planned = segments::plan(
        &mut SliceReader::new(&fixture.bytes),
        &forged,
        SegmentLimits::default(),
    )
    .unwrap();
    assert!(!planned.plans.is_empty());
    assert!(
        planned
            .plans
            .iter()
            .any(|plan| segments::verify_audio_selection(&target, plan).is_err())
    );
    for plan in &planned.plans {
        segments::verify_audio_selection(&forged, plan).unwrap();
    }
}

#[test]
fn dense_idr_audio_metadata_stays_bounded_and_obeys_post_read_stop_authority() {
    use frameharbor::{contracts::ByteRange, session::SessionError};
    let fixture = MediaFixture::generate(Codec::Avc, Moov::Head, Timing::Cfr).unwrap();
    let mut target = bmff::inspect_layout(
        &mut SliceReader::new(&fixture.bytes),
        ParseLimits::default(),
    )
    .unwrap();
    const GOPS: usize = 50_000;
    let mut bytes = Vec::with_capacity(GOPS * 11);
    let video = target
        .tracks
        .iter_mut()
        .find(|t| t.handler == *b"vide")
        .unwrap();
    let template = video.samples[0].clone();
    video.timescale = 1;
    video.media_duration = (GOPS * 2) as u64;
    video.samples.clear();
    for ordinal in 0..GOPS * 2 {
        let payload = [0, 0, 0, 1, if ordinal % 2 == 0 { 0x65 } else { 0x41 }];
        let mut sample = template.clone();
        sample.ordinal = ordinal as u64;
        sample.dts = ordinal as u64;
        sample.pts = ordinal as i128;
        sample.duration = 1;
        sample.is_sync = ordinal % 2 == 0;
        sample.logical = ByteRange::from_len(bytes.len() as u64, 5).unwrap();
        sample.source_spans.clear();
        sample.availability = SampleAvailability::Available {
            sha256: Sha256::digest(payload).into(),
        };
        bytes.extend(payload);
        video.samples.push(sample);
    }
    let audio = target
        .tracks
        .iter_mut()
        .find(|t| t.handler == *b"soun")
        .unwrap();
    let template = audio.samples[0].clone();
    audio.timescale = 1;
    audio.media_duration = (GOPS * 2) as u64;
    audio.sample_groups.clear();
    audio.samples.clear();
    for ordinal in 0..GOPS {
        let mut sample = template.clone();
        sample.ordinal = ordinal as u64;
        sample.dts = (ordinal * 2) as u64;
        sample.pts = (ordinal * 2) as i128;
        sample.duration = 2;
        sample.logical = ByteRange::from_len(bytes.len() as u64, 1).unwrap();
        sample.source_spans.clear();
        sample.availability = SampleAvailability::Available {
            sha256: Sha256::digest([1]).into(),
        };
        bytes.push(1);
        audio.samples.push(sample);
    }
    target.file_size = bytes.len() as u64;
    struct Budget<'a> {
        reader: SliceReader<'a>,
        reads: usize,
        post_read_checks: usize,
        stop: bool,
    }
    impl ReadAt for Budget<'_> {
        fn len(&self) -> u64 {
            self.reader.len()
        }
        fn read_exact(&mut self, offset: u64, bytes: &mut [u8]) -> bmff::Result<()> {
            self.reads += 1;
            self.reader.read_exact(offset, bytes)
        }
        fn reserve(&mut self, _: u64) -> bmff::Result<()> {
            if self.reads >= GOPS * 2 {
                self.post_read_checks += 1;
                if self.stop {
                    return Err(bmff::BmffError::Session(SessionError::Stopped));
                }
            }
            Ok(())
        }
    }
    let limits = SegmentLimits {
        samples: GOPS * 3,
        per_segment_samples: 1,
        sample_bytes: 16,
        ..SegmentLimits::default()
    };
    let mut reader = Budget {
        reader: SliceReader::new(&bytes),
        reads: 0,
        post_read_checks: 0,
        stop: true,
    };
    assert!(matches!(
        segments::plan(&mut reader, &target, limits),
        Err(bmff::BmffError::Session(SessionError::Stopped))
    ));
    assert!(reader.post_read_checks > 0);
    let mut reader = Budget {
        reader: SliceReader::new(&bytes),
        reads: 0,
        post_read_checks: 0,
        stop: false,
    };
    let start = std::time::Instant::now();
    let plans = segments::plan(&mut reader, &target, limits).unwrap();
    assert!(plans.plans.is_empty());
    assert!(reader.post_read_checks > 0);
    // 50k oversized closed GOPs and 50k AAC packets must not trigger 2.5b
    // audio visits. This generous bound accompanies the explicit stop contract.
    assert!(start.elapsed() < std::time::Duration::from_secs(10));
}
