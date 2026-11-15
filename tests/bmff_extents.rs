#![cfg(target_os = "macos")]
mod common;
use common::{
    filesystems::{Damage, Fixture, FsKind, Layout},
    media::{Codec, MediaFixture, Timing},
};
use frameharbor::{
    acquisition::ReadLimits,
    bmff::{self, LogicalReader, ParseLimits},
    contracts::{BindingEvidence, ByteRange, CoordinateBinding, ResourceCaps},
    control::CaseConfig,
    filesystem::{self, AdapterLimits, ExtentEvidence, FilesystemProfile},
    ledger::Ledger,
    session::{CaseSession, SessionInput},
};
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::{OnceLock, atomic::AtomicBool},
};
fn helper() -> &'static PathBuf {
    static HELPER: OnceLock<PathBuf> = OnceLock::new();
    HELPER.get_or_init(|| {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let output = root.join(format!("target/tsk-bmff-extents-{}", std::process::id()));
        let status = Command::new("/usr/bin/cc")
            .args([
                "-std=c11",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-DFRAMEHARBOR_TEST_MODE=0",
                "-I/usr/local/include",
            ])
            .arg(root.join("native/tsk_helper.c"))
            .arg("/usr/local/lib/libtsk.23.dylib")
            .arg("-o")
            .arg(&output)
            .status()
            .unwrap();
        assert!(status.success());
        output
    })
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
                receipts: 4096,
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
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
#[test]
fn encoded_sample_maps_follow_fragmented_extents_and_real_split_nal_prefixes() {
    for (kind, codec, boundary) in [
        (FsKind::Fat32, Codec::Avc, 16384u64),
        (FsKind::ExFat, Codec::Hevc, 131072u64),
    ] {
        let original = MediaFixture::generate_prefix_split(codec, Timing::Vfr, boundary).unwrap();
        original.verify_packet_bytes().unwrap();
        let mut f = Fixture::build(kind, Layout::Fragmented, Damage::None).unwrap();
        f.replace_payload(&original.bytes).unwrap();
        let before_metadata = fs::metadata(&f.image).unwrap();
        let before_map = fs::read(&f.map).unwrap();
        let mut case = session(&f);
        let report = filesystem::inspect(
            &mut case,
            helper(),
            f.ground_truth.logical_image_bytes,
            0,
            match kind {
                FsKind::Fat32 => FilesystemProfile::Fat32,
                FsKind::ExFat => FilesystemProfile::ExFat,
            },
            AdapterLimits::default(),
            &AtomicBool::new(false),
        )
        .unwrap();
        let view = report
            .files
            .iter()
            .find(|v| {
                v.size == original.bytes.len() as u64
                    && v.evidence == ExtentEvidence::MetadataAllocated
            })
            .expect("evidenced generated media view");
        let map = bmff::inspect(
            &mut LogicalReader::new(&mut case, view),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(map.file_size, original.bytes.len() as u64);
        assert_eq!(map.tracks.len(), original.streams.len());
        for truth in &original.streams {
            let track = map.tracks.iter().find(|t| t.id == truth.id).unwrap();
            assert_eq!(truth.time_base, (1, i64::from(track.timescale)));
            assert_eq!(
                track.handler,
                if truth.codec_type == "video" {
                    *b"vide"
                } else {
                    *b"soun"
                }
            );
            assert_eq!(track.descriptions.len(), 1);
            assert_eq!(
                hex(&track.descriptions[0].configuration_sha256),
                truth.extradata_sha256
            );
            let packets: Vec<_> = original
                .packets
                .iter()
                .filter(|p| p.stream_index == truth.index)
                .collect();
            assert_eq!(track.samples.len(), packets.len());
            for (sample, packet) in track.samples.iter().zip(packets) {
                assert_eq!(sample.ordinal, packet.ordinal as u64);
                assert_eq!(sample.logical.start(), packet.pos);
                assert_eq!(sample.logical.len(), packet.size);
                assert_eq!(sample.description_index, 1);
                assert_eq!(hex(&sample.sha256), packet.sha256);
                assert_eq!(i64::try_from(sample.dts).unwrap(), packet.dts);
                assert_eq!(sample.pts, i128::from(packet.pts));
                assert_eq!(i64::from(sample.duration), packet.duration);
                let mut cursor = sample.logical.start();
                for actual in &sample.source_spans {
                    assert_eq!(actual.logical.start(), cursor);
                    assert_eq!(actual.logical.len(), actual.image.len());
                    let mut covered = 0;
                    for expected in &f.ground_truth.spans {
                        let er =
                            ByteRange::from_len(expected.logical_offset, expected.len).unwrap();
                        if let Some(part) = er.intersection(actual.logical) {
                            assert_eq!(
                                actual.image.start() + part.start() - actual.logical.start(),
                                expected.image_offset + part.start() - expected.logical_offset
                            );
                            covered += part.len();
                        }
                    }
                    assert_eq!(covered, actual.logical.len());
                    cursor = actual.logical.end();
                }
                assert_eq!(cursor, sample.logical.end());
            }
        }
        let video = map.tracks.iter().find(|t| t.handler == *b"vide").unwrap();
        let split = &video.samples[0];
        assert_eq!(split.logical.start(), boundary - 1);
        assert!(split.source_spans.len() >= 2);
        assert_eq!(split.source_spans[0].logical.end(), boundary);
        assert_eq!(
            split.source_spans[0].image.start(),
            f.ground_truth.spans[0].image_offset + boundary - 1
        );
        assert_eq!(
            split.source_spans[1].image.start(),
            f.ground_truth.spans[1].image_offset
        );
        assert!(video.samples.iter().any(|s| i128::from(s.dts) != s.pts));
        // The independent packet hashes cover original media bytes; metadata identity
        // and exact map bytes additionally detect source writes during the pipeline.
        let after_metadata = fs::metadata(&f.image).unwrap();
        assert_eq!(before_metadata.len(), after_metadata.len());
        assert_eq!(
            before_metadata.modified().unwrap(),
            after_metadata.modified().unwrap()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(before_metadata.ctime(), after_metadata.ctime());
            assert_eq!(before_metadata.ctime_nsec(), after_metadata.ctime_nsec());
        }
        assert_eq!(fs::read(&f.map).unwrap(), before_map);
        let used = case.snapshot().unwrap().used;
        assert!(used.read_bytes > 0 && used.read_bytes <= 128 * 1024 * 1024);
        assert_eq!(used.decoder_seconds, 0);
    }
}
