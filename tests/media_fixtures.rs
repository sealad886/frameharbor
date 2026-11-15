mod common;
use common::{filesystems::*, media::*};
use std::collections::BTreeSet;
fn boxes(bytes: &[u8]) -> Vec<([u8; 4], usize)> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 8 <= bytes.len() {
        let size = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        assert!(size >= 8 && at + size <= bytes.len());
        out.push((bytes[at + 4..at + 8].try_into().unwrap(), at));
        at += size;
    }
    assert_eq!(at, bytes.len());
    out
}
#[test]
fn mathematical_avc_hevc_audio_oracles_preserve_raw_bytes_reordering_vfr_and_moov_placement() {
    for (codec, moov, timing) in [
        (Codec::Avc, Moov::Head, Timing::Cfr),
        (Codec::Avc, Moov::Tail, Timing::Vfr),
        (Codec::Hevc, Moov::Head, Timing::Vfr),
        (Codec::Hevc, Moov::Tail, Timing::Cfr),
    ] {
        let f = MediaFixture::generate(codec, moov, timing).unwrap();
        f.verify_packet_bytes().unwrap();
        assert_eq!(std::fs::read(&f.file).unwrap(), f.bytes);
        let video = f.streams.iter().find(|s| s.codec_type == "video").unwrap();
        let audio = f.streams.iter().find(|s| s.codec_type == "audio").unwrap();
        assert_eq!(
            video.codec_name,
            if codec == Codec::Avc { "h264" } else { "hevc" }
        );
        assert_eq!(audio.codec_name, "aac");
        assert_ne!(video.id, audio.id);
        assert!(
            f.streams
                .iter()
                .all(|s| s.time_base.0 == 1 && s.time_base.1 > 0 && s.extradata_sha256.len() == 64)
        );
        let samples: Vec<_> = f
            .packets
            .iter()
            .filter(|p| p.stream_index == video.index)
            .collect();
        assert!(samples.iter().any(|p| p.dts != p.pts));
        assert!(samples.iter().enumerate().all(|(i, p)| p.ordinal == i));
        assert!(samples.iter().all(|p| p.duration > 0));
        let mut pts: Vec<_> = samples.iter().map(|p| p.pts).collect();
        pts.sort_unstable();
        let intervals: BTreeSet<_> = pts.windows(2).map(|x| x[1] - x[0]).collect();
        assert_eq!(intervals.len() > 1, timing == Timing::Vfr);
        let mut physical = f.packets.clone();
        physical.sort_by_key(|p| p.pos);
        assert!(
            physical
                .windows(2)
                .filter(|x| x[0].stream_index != x[1].stream_index)
                .count()
                > 2
        );
        assert!(
            physical
                .windows(2)
                .all(|x| x[0].pos + x[0].size <= x[1].pos)
        );
        assert!(
            f.packets
                .iter()
                .any(|p| p.stream_index == audio.index && p.size > 0)
        );
        let b = boxes(&f.bytes);
        let movie = b.iter().find(|(k, _)| k == b"moov").unwrap().1;
        let media = b.iter().find(|(k, _)| k == b"mdat").unwrap().1;
        assert_eq!(movie < media, moov == Moov::Head);
        assert!(!b.iter().any(|(k, _)| k == b"moof"));
    }
}
#[test]
fn generated_media_survives_independent_fragmented_filesystem_placement() {
    let media = MediaFixture::generate(Codec::Avc, Moov::Tail, Timing::Vfr).unwrap();
    media.verify_packet_bytes().unwrap();
    for kind in [FsKind::Fat32, FsKind::ExFat] {
        let mut f = Fixture::build(kind, Layout::Fragmented, Damage::None).unwrap();
        let padded = media.padded_bytes(f.ground_truth.payload.len()).unwrap();
        f.replace_payload(&padded).unwrap();
        assert_eq!(f.ground_truth.payload, padded);
        for p in &media.packets {
            let original = &media.bytes[p.pos as usize..(p.pos + p.size) as usize];
            let stored = &f.ground_truth.payload[p.pos as usize..(p.pos + p.size) as usize];
            assert_eq!(original, stored);
        }
        assert_eq!(&padded[..media.bytes.len()], media.bytes.as_slice());
    }
}
#[test]
fn actual_avc_hevc_nal_prefixes_cross_both_filesystem_cluster_sizes() {
    use std::io::{Read, Seek, SeekFrom};
    for (kind, boundary) in [(FsKind::Fat32, 16384u64), (FsKind::ExFat, 131072u64)] {
        for codec in [Codec::Avc, Codec::Hevc] {
            let media = MediaFixture::generate_prefix_split(codec, Timing::Vfr, boundary).unwrap();
            media.verify_packet_bytes().unwrap();
            let video = media
                .streams
                .iter()
                .find(|s| s.codec_type == "video")
                .unwrap();
            let first = media
                .packets
                .iter()
                .find(|p| p.stream_index == video.index)
                .unwrap();
            assert_eq!(first.pos, boundary - 1);
            let mut f = Fixture::build(kind, Layout::Fragmented, Damage::None).unwrap();
            f.replace_payload(&media.bytes).unwrap();
            let mut source = std::fs::File::open(&f.image).unwrap();
            let mut actual = [0u8; 4];
            source
                .seek(SeekFrom::Start(
                    f.ground_truth.spans[0].image_offset + boundary - 1,
                ))
                .unwrap();
            source.read_exact(&mut actual[..1]).unwrap();
            source
                .seek(SeekFrom::Start(f.ground_truth.spans[1].image_offset))
                .unwrap();
            source.read_exact(&mut actual[1..]).unwrap();
            assert_eq!(
                actual,
                media.bytes[(boundary - 1) as usize..(boundary + 3) as usize]
            );
            assert!(u32::from_be_bytes(actual) > 0);
            assert_ne!(
                f.ground_truth.spans[0].image_offset + boundary,
                f.ground_truth.spans[1].image_offset
            );
            assert_eq!(
                std::fs::read(f.dir.join("original.bin")).unwrap(),
                media.bytes
            );
        }
    }
}
