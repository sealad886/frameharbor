mod common;
use common::filesystems::*;
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
};
fn read(path: &std::path::Path, offset: u64, len: usize) -> Vec<u8> {
    let mut f = File::open(path).unwrap();
    f.seek(SeekFrom::Start(offset)).unwrap();
    let mut b = vec![0; len];
    f.read_exact(&mut b).unwrap();
    b
}
#[test]
fn fixture_physical_spans_reconstruct_independent_original_bytes() {
    for kind in [FsKind::Fat32, FsKind::ExFat] {
        for layout in [Layout::Fragmented, Layout::Contiguous] {
            for damage in [
                Damage::None,
                Damage::Deleted,
                Damage::PartialChain,
                Damage::Crosslink,
                Damage::BitmapFree,
                Damage::AcquisitionHole,
                Damage::MetadataHole,
            ] {
                if kind == FsKind::Fat32 && damage == Damage::BitmapFree
                    || kind == FsKind::ExFat
                        && layout == Layout::Contiguous
                        && damage == Damage::PartialChain
                {
                    continue;
                }
                let f = Fixture::build(kind, layout, damage).unwrap();
                let t = &f.ground_truth;
                let mut actual = Vec::new();
                for span in &t.spans {
                    assert_eq!(actual.len() as u64, span.logical_offset);
                    actual.extend(read(&f.image, span.image_offset, span.len as usize))
                }
                assert_eq!(actual, t.payload);
                assert_eq!(
                    std::fs::read(f.dir.join("original.bin")).unwrap(),
                    t.payload
                );
                // Host allocation granularity is observed, not inferred from set_len.
                assert!(f.allocated_bytes().unwrap() < 16 * 1024 * 1024);
                assert_eq!(
                    t.cluster_size,
                    if kind == FsKind::Fat32 { 16384 } else { 131072 }
                );
                assert_eq!(
                    t.second_cluster,
                    if layout == Layout::Fragmented { 9 } else { 6 }
                );
                assert!(t.root_offset < t.spans[0].image_offset);
                assert!(t.fat_offset < t.root_offset);
                assert_eq!(
                    t.ownership,
                    match damage {
                        Damage::Deleted | Damage::BitmapFree =>
                            ExpectedOwnership::HistoricalConflict,
                        Damage::PartialChain => ExpectedOwnership::Unresolved,
                        Damage::Crosslink => ExpectedOwnership::SharedGeometry,
                        _ => ExpectedOwnership::MetadataSupported,
                    }
                );
                assert_eq!(
                    t.complete,
                    !matches!(
                        damage,
                        Damage::PartialChain | Damage::BitmapFree | Damage::AcquisitionHole
                    )
                );
            }
        }
    }
}
#[test]
fn fat32_geometry_is_above_fat32_cluster_threshold_and_fragmented_chain_is_explicit() {
    let f = Fixture::build(FsKind::Fat32, Layout::Fragmented, Damage::None).unwrap();
    let b = read(&f.image, 0, 512);
    let u16at = |o| u16::from_le_bytes(b[o..o + 2].try_into().unwrap()) as u64;
    let u32at = |o| u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as u64;
    let heap = u16at(14) + b[16] as u64 * u32at(36);
    let count = (u32at(32) - heap) / b[13] as u64;
    assert!(count >= 65525);
    assert_eq!(count, 65536);
    assert_eq!(
        read(&f.image, f.ground_truth.fat_offset + 20, 4),
        9u32.to_le_bytes()
    );
    assert_eq!(&b[510..], [0x55, 0xaa]);
}
#[test]
fn exfat_boot_regions_and_directory_sets_have_independently_verified_checksums() {
    for damage in [Damage::None, Damage::Deleted, Damage::Crosslink] {
        let f = Fixture::build(FsKind::ExFat, Layout::Fragmented, damage).unwrap();
        let b = read(&f.image, 0, 12288);
        assert_eq!(&b[..6144], &b[6144..]);
        let mut checksum = 0u32;
        for (i, v) in b[..5632].iter().enumerate() {
            if i != 106 && i != 107 && i != 112 {
                checksum = ((checksum & 1) << 31) + (checksum >> 1);
                checksum = checksum.wrapping_add(*v as u32)
            }
        }
        assert!(
            b[5632..6144]
                .as_chunks::<4>()
                .0
                .iter()
                .all(|x| u32::from_le_bytes(*x) == checksum)
        );
        let root = read(&f.image, f.ground_truth.root_offset, 256);
        for start in [64, 160] {
            if root[start] == 0 {
                continue;
            }
            let set = &root[start..start + 96];
            let mut sum = 0u16;
            for (i, v) in set.iter().enumerate() {
                if i != 2 && i != 3 {
                    sum = ((sum & 1) << 15) + (sum >> 1);
                    sum = sum.wrapping_add(*v as u16)
                }
            }
            assert_eq!(sum, u16::from_le_bytes(set[2..4].try_into().unwrap()));
        }
    }
}
#[test]
fn acquisition_holes_are_declared_in_map_while_stale_image_bytes_remain() {
    for damage in [Damage::AcquisitionHole, Damage::MetadataHole] {
        let f = Fixture::build(FsKind::ExFat, Layout::Fragmented, damage).unwrap();
        let map = std::fs::read_to_string(&f.map).unwrap();
        assert!(map.lines().any(|x| x.ends_with(" -")));
        let second = &f.ground_truth.spans[1];
        assert_eq!(
            read(&f.image, second.image_offset, second.len as usize),
            f.ground_truth.payload[f.ground_truth.cluster_size as usize..]
        );
    }
}
