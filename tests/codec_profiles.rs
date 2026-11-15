#![cfg(target_os = "macos")]
#[allow(dead_code)]
mod common;
use common::media::{Codec, MediaFixture};
use frameharbor::{
    acquisition::ReadLimits,
    bmff::{self, LogicalReader, ParseLimits},
    contracts::*,
    control::CaseConfig,
    filesystem::{ExtentEvidence, ExtentRun, LogicalFileView},
    ledger::Ledger,
    session::{CaseSession, SessionInput},
    validation::{self, StagingArea, ValidationLimits},
};
use std::{
    fs, io::Write, path::PathBuf, process::Command, sync::atomic::AtomicBool, time::Instant,
};
fn helper() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let out = root.join(format!(
        "target/codec-profile-helper-{}",
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
}
#[test]
fn exposed_720p_1080p_avc_hevc_stereo_controls_validate_under_existing_limits() {
    let helper = helper();
    let report_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
        "target/codec-profile-observations-{}.tsv",
        std::process::id()
    ));
    let mut report = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&report_path)
        .unwrap();
    writeln!(report,"scope\texposed mathematical codec controls; no filesystem, camera, physical-I/O or blind qualification").unwrap();
    writeln!(report,"codec\twidth\theight\tencoded_bytes\tvideo_samples\taudio_samples\tvalidation_millis\tstage_validation_millis\tdecoder_max_rss_bytes\timage_api_read_bytes\tstaged_api_read_bytes").unwrap();
    for (width, height) in [(1280, 720), (1920, 1080)] {
        for codec in [Codec::Avc, Codec::Hevc] {
            let media = MediaFixture::generate_codec_control(codec, width, height).unwrap();
            media.verify_packet_bytes().unwrap();
            let video_truth = media
                .streams
                .iter()
                .find(|s| s.codec_type == "video")
                .unwrap();
            assert_eq!(
                (video_truth.width, video_truth.height),
                (Some(width), Some(height))
            );
            let audio_truth = media
                .streams
                .iter()
                .find(|s| s.codec_type == "audio")
                .unwrap();
            assert_eq!(
                (audio_truth.sample_rate, audio_truth.channels),
                (Some(48000), Some(2))
            );
            let size = media.bytes.len() as u64;
            let map = media.dir.join("source.map");
            fs::write(&map, format!("0 + 1\n0 {size} +\n")).unwrap();
            let mut session = CaseSession::create(
                Ledger::open(&media.dir.join("case.sqlite"), 16 * 1024 * 1024).unwrap(),
                SessionInput {
                    image: media.file.clone(),
                    map,
                    binding: CoordinateBinding::new(
                        ByteRange::new(0, size).unwrap(),
                        0,
                        size,
                        BindingEvidence::OperatorAttested,
                    )
                    .unwrap(),
                    limits: ReadLimits {
                        per_read: 1024 * 1024,
                        total: 1024 * 1024 * 1024,
                        receipts: 4096,
                    },
                },
                CaseConfig {
                    total_millis: 120000,
                    shutdown_reserve_millis: 1000,
                    caps: ResourceCaps {
                        read_bytes: 1024 * 1024 * 1024,
                        memory_bytes: 512 * 1024 * 1024,
                        index_bytes: 16 * 1024 * 1024,
                        output_bytes: 64 * 1024 * 1024,
                        visited_postings: 0,
                        transitions: 0,
                        decoder_seconds: 120,
                    },
                },
            )
            .unwrap();
            // An explicit synthetic byte-layout oracle isolates the codec stage.
            // Native filesystem/ownership acceptance has separate disk fixtures.
            let view = LogicalFileView {
                owner: LogicalOwner {
                    source: session.source_generation(),
                    filesystem: "synthetic-codec-control".into(),
                    file_identity: "mathematical-original".into(),
                },
                size,
                metadata_flags: 1,
                name_flags: 1,
                attribute_flags: 3,
                initialized_size: size,
                skip_length: 0,
                runs: vec![ExtentRun {
                    logical: ByteRange::new(0, size).unwrap(),
                    image: Some(ByteRange::new(0, size).unwrap()),
                    raw_flags: 0,
                    allocation_consistent: true,
                }],
                evidence: ExtentEvidence::MetadataAllocated,
            };
            let expected = bmff::inspect(
                &mut LogicalReader::new(&mut session, &view),
                ParseLimits::default(),
            )
            .unwrap();
            let video = expected
                .tracks
                .iter()
                .find(|t| t.handler == *b"vide")
                .unwrap();
            let video_samples = video.samples.len();
            assert_eq!(video_samples, 12);
            let audio = expected
                .tracks
                .iter()
                .find(|t| t.handler == *b"soun")
                .unwrap();
            let audio_samples = audio.samples.len();
            assert_eq!(audio.descriptions[0].audio_channels, Some(2));
            assert_eq!(audio.descriptions[0].audio_sample_rate, Some(48000));
            let mut area =
                StagingArea::create(&media.dir.join("stage"), 64 * 1024 * 1024, 2).unwrap();
            let started = Instant::now();
            let staged = area
                .stage_exact(&mut session, &view, expected, ValidationLimits::default())
                .unwrap();
            let validated = validation::validate(
                &mut session,
                staged,
                &helper,
                ValidationLimits::default(),
                &AtomicBool::new(false),
            )
            .unwrap();
            let stage_validation = started.elapsed();
            let r = validated.receipt();
            assert_eq!(r.claims.decode, Decode::StrictPass);
            assert!(r.original_file_bytes_preserved && r.audio_present);
            assert_eq!(r.packets, media.packets.len() as u64);
            assert_eq!(r.samples, r.frames);
            assert_eq!(fs::read(validated.path()).unwrap(), media.bytes);
            writeln!(report,"{codec:?}\t{width}\t{height}\t{size}\t{video_samples}\t{audio_samples}\t{}\t{}\t{}\t{}\t{}",r.elapsed.as_millis(),stage_validation.as_millis(),r.max_rss_bytes,session.image_bytes_read_this_run(),r.staged_read_bytes).unwrap();
            report.flush().unwrap();
        }
    }
    report.sync_all().unwrap();
    eprintln!("codec observations: {}", report_path.display());
}
