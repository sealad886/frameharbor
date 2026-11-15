use frameharbor::acquisition::*;
use frameharbor::contracts::*;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    dir: PathBuf,
    image: PathBuf,
    map: PathBuf,
}
impl Fixture {
    fn new(bytes: &[u8], map: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "frameharbor-acquisition-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        let image = dir.join("image.bin");
        let map_path = dir.join("image.map");
        fs::write(&image, bytes).unwrap();
        fs::write(&map_path, map).unwrap();
        Self {
            dir,
            image,
            map: map_path,
        }
    }
    fn open(
        &self,
        domain: ByteRange,
        displacement: i128,
        evidence: BindingEvidence,
        limits: ReadLimits,
    ) -> RangeSource {
        let binding = CoordinateBinding::new(
            domain,
            displacement,
            fs::metadata(&self.image).unwrap().len(),
            evidence,
        )
        .unwrap();
        RangeSource::open(&self.image, &self.map, binding, limits).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.image);
        let _ = fs::remove_file(&self.map);
        let _ = fs::remove_dir(&self.dir);
    }
}
fn r(a: u64, b: u64) -> ByteRange {
    ByteRange::new(a, b).unwrap()
}
#[test]
fn every_nonfinished_status_and_unknown_gap_block_stale_video_bytes() {
    let fixture = Fixture::new(
        b"ftypmoovmdatavc1",
        "0 + 1\n0 2 +\n2 2 ?\n4 2 *\n6 2 /\n8 2 -\n12 4 +\n",
    );
    let mut source = fixture.open(
        r(0, 16),
        0,
        BindingEvidence::OperatorAttested,
        ReadLimits::default(),
    );
    for range in [r(2, 4), r(4, 6), r(6, 8), r(8, 10), r(10, 12), r(0, 16)] {
        assert!(matches!(
            source.read(range),
            Err(AcquisitionError::Unavailable(_))
        ));
    }
    assert_eq!(source.bytes_read(), 0);
    assert_eq!(source.read(r(12, 16)).unwrap().bytes, b"avc1");
    assert_eq!(fs::read(&fixture.image).unwrap(), b"ftypmoovmdatavc1");
}
#[test]
fn map_coverage_cannot_authenticate_a_wrong_same_size_pair() {
    let fixture = Fixture::new(b"wrong image!", "0 +\n0 12 +\n");
    let mut source = fixture.open(
        r(0, 12),
        0,
        BindingEvidence::Unverified,
        ReadLimits::default(),
    );
    assert!(matches!(
        source.read(r(0, 12)),
        Err(AcquisitionError::BindingUnverified)
    ));
    assert_eq!(source.bytes_read(), 0);
}
#[test]
fn nonzero_input_relocation_and_rescue_domain_are_explicit() {
    let fixture = Fixture::new(b"abcdefgh", "4096 + 1\n4096 8 +\n");
    let mut source = fixture.open(
        r(4096, 4104),
        -4096,
        BindingEvidence::OperatorAttested,
        ReadLimits::default(),
    );
    assert_eq!(source.read(r(2, 6)).unwrap().bytes, b"cdef");
    assert!(source.read(r(8, 9)).is_err());
    let mismatch =
        CoordinateBinding::new(r(0, 8), 0, 8, BindingEvidence::OperatorAttested).unwrap();
    let mut wrong = RangeSource::open(
        &fixture.image,
        &fixture.map,
        mismatch,
        ReadLimits::default(),
    )
    .unwrap();
    assert!(matches!(
        wrong.read(r(0, 8)),
        Err(AcquisitionError::Unavailable(_))
    ));
}
#[test]
fn malformed_maps_and_resource_exhaustion_fail_before_reads() {
    for map in [
        "",
        "0 + 1\n",
        "0 + 1\n0 0 +",
        "0 + 1\n0 10 +\n9 1 -",
        "0 + 1\n10 1 +\n0 1 +",
        "0 + 1\n18446744073709551615 2 +",
        "0 + 1\n0 3 X",
        "0 + 1\n0 3 + trailing",
    ] {
        assert!(
            RescueMap::parse(map.as_bytes(), MapLimits::default()).is_err(),
            "{map}"
        );
    }
    assert!(
        RescueMap::parse(
            "0 + 1\n0 9 +\n".as_bytes(),
            MapLimits {
                bytes: 4,
                line_bytes: 4096,
                intervals: 10
            }
        )
        .is_err()
    );
    assert!(
        RescueMap::parse(
            format!("#{}", "x".repeat(10000)).as_bytes(),
            MapLimits {
                bytes: 20000,
                line_bytes: 32,
                intervals: 10
            }
        )
        .is_err()
    );
    let fixture = Fixture::new(b"12345678", "0 + 1\n0 8 +\n");
    let mut source = fixture.open(
        r(0, 8),
        0,
        BindingEvidence::OperatorAttested,
        ReadLimits {
            per_read: 4,
            total: 6,
            receipts: 2,
        },
    );
    assert!(source.read(r(0, 5)).is_err());
    assert_eq!(source.bytes_read(), 0);
    source.read(r(0, 4)).unwrap();
    assert!(source.read(r(4, 8)).is_err());
    assert_eq!(source.bytes_read(), 4);
}
#[test]
fn source_and_map_changes_or_replacement_invalidate_generation() {
    for map_change in [false, true] {
        let fixture = Fixture::new(b"12345678", "0 + 1\n0 8 +\n");
        let mut source = fixture.open(
            r(0, 8),
            0,
            BindingEvidence::OperatorAttested,
            ReadLimits::default(),
        );
        source.read(r(0, 2)).unwrap();
        if map_change {
            fs::write(&fixture.map, "0 + 1\n0 8 -\n").unwrap();
        } else {
            fs::write(&fixture.image, b"87654321").unwrap();
        }
        assert!(matches!(
            source.read(r(2, 4)),
            Err(AcquisitionError::SourceChanged)
        ));
    }
    let fixture = Fixture::new(b"12345678", "0 + 1\n0 8 +\n");
    let mut source = fixture.open(
        r(0, 8),
        0,
        BindingEvidence::OperatorAttested,
        ReadLimits::default(),
    );
    let replacement = fixture.dir.join("replacement");
    fs::write(&replacement, b"12345678").unwrap();
    fs::rename(replacement, &fixture.image).unwrap();
    assert!(matches!(
        source.read(r(0, 1)),
        Err(AcquisitionError::SourceChanged)
    ));
}
#[test]
fn consumed_hash_revalidation_is_separately_metered() {
    let fixture = Fixture::new(b"abcdefgh", "0 + 1\n0 8 +\n");
    let mut source = fixture.open(
        r(0, 8),
        0,
        BindingEvidence::OperatorAttested,
        ReadLimits::default(),
    );
    let receipt = source.read(r(0, 8)).unwrap();
    assert_eq!(
        receipt.sha256,
        [
            0x9c, 0x56, 0xcc, 0x51, 0xb3, 0x74, 0xc3, 0xba, 0x18, 0x92, 0x10, 0xd5, 0xb6, 0xd4,
            0xbf, 0x57, 0x79, 0x0d, 0x35, 0x1c, 0x96, 0xc4, 0x7c, 0x02, 0x19, 0x0e, 0xcf, 0x1e,
            0x43, 0x06, 0x35, 0xab
        ]
    );
    source.revalidate().unwrap();
    assert_eq!(source.verification_bytes(), 8);
    assert_eq!(source.bytes_read(), 16);
    assert_eq!(source.consumed().len(), 1);
}
#[test]
fn sparse_zero_bytes_need_finished_status_and_never_imply_video() {
    let fixture = Fixture::new(b"", "0 + 1\n0 8192 +\n");
    fs::OpenOptions::new()
        .write(true)
        .open(&fixture.image)
        .unwrap()
        .set_len(8192)
        .unwrap();
    let mut source = fixture.open(
        r(0, 8192),
        0,
        BindingEvidence::OperatorAttested,
        ReadLimits::default(),
    );
    let receipt = source.read(r(0, 8192)).unwrap();
    assert!(receipt.bytes.iter().all(|b| *b == 0));
    // The receipt contains no structural, decode, temporal, or recording claims.
    assert_eq!(receipt.image, r(0, 8192));
}
#[test]
fn pilot_is_stratified_bounded_and_deadline_zero_schedules_nothing() {
    let fixture = Fixture::new(&vec![42; 1000], "0 + 1\n0 1000 +\n");
    let mut source = fixture.open(
        r(0, 1000),
        0,
        BindingEvidence::OperatorAttested,
        ReadLimits {
            per_read: 10,
            total: 1000,
            receipts: 100,
        },
    );
    let zero = pilot(&mut source, 40, Duration::ZERO, 4).unwrap();
    assert_eq!(zero.bytes, 0);
    let receipt = pilot(&mut source, 40, Duration::from_secs(1), 4).unwrap();
    assert_eq!(receipt.bytes, 40);
    assert_eq!(receipt.windows, 4);
    assert_eq!(
        source
            .consumed()
            .iter()
            .map(|r| r.image.start())
            .collect::<Vec<_>>(),
        [0, 250, 500, 750]
    );
    assert!(
        pilot(
            &mut source,
            9 * 1024 * 1024 * 1024,
            Duration::from_secs(1),
            4
        )
        .is_err()
    );
}
#[test]
fn bounded_malformed_input_fuzz_never_panics() {
    let mut state = 13u64;
    for size in 0..256 {
        let bytes: Vec<_> = (0..size)
            .map(|_| {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                (state >> 32) as u8
            })
            .collect();
        let _ = RescueMap::parse(
            bytes.as_slice(),
            MapLimits {
                bytes: 256,
                line_bytes: 128,
                intervals: 8,
            },
        );
    }
}

#[test]
fn pilot_finds_eligible_islands_and_never_duplicates_tiny_ranges() {
    let fixture = Fixture::new(
        &vec![42; 1000],
        "0 + 1\n100 50 +\n350 50 +\n600 50 +\n850 50 +\n",
    );
    let mut source = fixture.open(
        r(0, 1000),
        0,
        BindingEvidence::OperatorAttested,
        ReadLimits {
            per_read: 10,
            total: 1000,
            receipts: 100,
        },
    );
    let receipt = pilot(&mut source, 40, Duration::from_secs(1), 4).unwrap();
    assert_eq!(receipt.bytes, 40);
    assert_eq!(receipt.eligible_bytes, 200);
    assert_eq!(receipt.image_domain_bytes, 1000);
    assert_eq!(
        source
            .consumed()
            .iter()
            .map(|r| r.image.start())
            .collect::<Vec<_>>(),
        [100, 350, 600, 850]
    );
    let fixture = Fixture::new(b"ab", "0 + 1\n0 2 +\n");
    let mut source = fixture.open(
        r(0, 2),
        0,
        BindingEvidence::OperatorAttested,
        ReadLimits::default(),
    );
    let receipt = pilot(&mut source, 100, Duration::from_secs(1), 20).unwrap();
    assert_eq!(receipt.bytes, 2);
    assert_eq!(receipt.windows, 2);
    assert_eq!(
        source
            .consumed()
            .iter()
            .map(|r| r.image)
            .collect::<Vec<_>>(),
        [r(0, 1), r(1, 2)]
    );
}
#[test]
fn reader_uses_resumed_cumulative_budget_and_rejects_expired_work() {
    let fixture = Fixture::new(b"abcd", "0 + 1\n0 4 +\n");
    let binding = CoordinateBinding::new(r(0, 4), 0, 4, BindingEvidence::OperatorAttested).unwrap();
    let caps = ResourceCaps {
        read_bytes: 100,
        memory_bytes: 100,
        index_bytes: 0,
        output_bytes: 0,
        visited_postings: 0,
        transitions: 0,
        decoder_seconds: 0,
    };
    let mut budget = Budget::new(Duration::from_secs(60), Duration::from_secs(10), caps).unwrap();
    budget.account_elapsed(Duration::from_secs(50)).unwrap();
    assert!(matches!(
        RangeSource::open_with_budget(
            &fixture.image,
            &fixture.map,
            binding.clone(),
            ReadLimits::default(),
            budget
        ),
        Err(AcquisitionError::Stopped(TerminalReason::BudgetExhausted))
    ));
    let mut budget = Budget::new(Duration::from_secs(60), Duration::from_secs(10), caps).unwrap();
    budget.account_elapsed(Duration::from_secs(20)).unwrap();
    let mut source = RangeSource::open_with_budget(
        &fixture.image,
        &fixture.map,
        binding,
        ReadLimits::default(),
        budget,
    )
    .unwrap();
    source.read(r(0, 4)).unwrap();
    let budget = source.into_budget();
    assert!(budget.elapsed() >= Duration::from_secs(20));
    assert_eq!(budget.used().read_bytes, 4);
}
#[test]
fn immutable_manifest_records_consumed_dependencies_without_full_image_claim() {
    let fixture = Fixture::new(b"abcdefgh", "0 + 1\n0 8 +\n");
    let mut source = fixture.open(
        r(0, 8),
        0,
        BindingEvidence::OperatorAttested,
        ReadLimits::default(),
    );
    let first = source.manifest().unwrap();
    assert!(!first.is_full_image_hash());
    assert!(first.consumed().is_empty());
    source.read(r(0, 4)).unwrap();
    let second = source.manifest().unwrap();
    assert_eq!(first.generation_id(), second.generation_id());
    assert_eq!(second.consumed().len(), 1);
    assert_eq!(second.image_size(), 8);
    assert!(first.consumed().is_empty());
    assert_eq!(
        source.map_bytes_read(),
        fs::metadata(&fixture.map).unwrap().len()
    );
}
#[test]
fn custom_case_memory_cap_bounds_owned_read_allocations() {
    let fixture = Fixture::new(b"abcdefgh", "0 + 1\n0 8 +\n");
    let binding = CoordinateBinding::new(r(0, 8), 0, 8, BindingEvidence::OperatorAttested).unwrap();
    let caps = ResourceCaps {
        read_bytes: 100,
        memory_bytes: 2,
        index_bytes: 0,
        output_bytes: 0,
        visited_postings: 0,
        transitions: 0,
        decoder_seconds: 0,
    };
    let budget = Budget::new(Duration::from_secs(60), Duration::from_secs(10), caps).unwrap();
    let mut source = RangeSource::open_with_budget(
        &fixture.image,
        &fixture.map,
        binding,
        ReadLimits::default(),
        budget,
    )
    .unwrap();
    assert!(matches!(
        source.read(r(0, 8)),
        Err(AcquisitionError::Limit("read buffer memory"))
    ));
    assert_eq!(source.bytes_read(), 0);
    source.read(r(0, 2)).unwrap();
    source.revalidate().unwrap();
    assert_eq!(source.verification_bytes(), 2);
}
#[test]
fn masked_reads_return_quality_boundaries_without_stale_destination_bytes() {
    let fixture = Fixture::new(b"ftypmoovmdat", "0 + 1\n0 4 +\n4 4 -\n10 2 +\n");
    let mut source = fixture.open(
        r(0, 12),
        0,
        BindingEvidence::OperatorAttested,
        ReadLimits::default(),
    );
    let pieces = source.read_masked(r(0, 12)).unwrap();
    assert_eq!(pieces.len(), 4);
    assert!(matches!(&pieces[0],MaskedPiece::Available(receipt) if receipt.bytes==b"ftyp"));
    assert!(
        matches!(&pieces[1],MaskedPiece::Unavailable{image,input,status:AcquisitionStatus::BadSector} if *image==r(4,8)&&*input==r(4,8))
    );
    assert!(
        matches!(&pieces[2],MaskedPiece::Unavailable{image,input,status:AcquisitionStatus::Unknown} if *image==r(8,10)&&*input==r(8,10))
    );
    assert!(matches!(&pieces[3],MaskedPiece::Available(receipt) if receipt.bytes==b"at"));
    assert_eq!(source.bytes_read(), 6);
    assert_eq!(source.consumed().len(), 2);
}
#[test]
fn coordinator_map_limits_reject_growth_and_interval_fanout_before_reads() {
    let fixture = Fixture::new(b"abcdefgh", "0 + 1\n0 2 +\n2 2 +\n4 4 +\n");
    let binding = CoordinateBinding::new(r(0, 8), 0, 8, BindingEvidence::OperatorAttested).unwrap();
    let caps = ResourceCaps {
        read_bytes: 100,
        memory_bytes: 65536,
        index_bytes: 0,
        output_bytes: 0,
        visited_postings: 0,
        transitions: 0,
        decoder_seconds: 0,
    };
    let budget = || Budget::new(Duration::from_secs(60), Duration::from_secs(10), caps).unwrap();
    assert!(matches!(
        RangeSource::open_with_map_limits(
            &fixture.image,
            &fixture.map,
            binding.clone(),
            ReadLimits::default(),
            budget(),
            MapLimits {
                bytes: 10,
                line_bytes: 4096,
                intervals: 100
            }
        ),
        Err(AcquisitionError::Limit(_))
    ));
    assert!(matches!(
        RangeSource::open_with_map_limits(
            &fixture.image,
            &fixture.map,
            binding,
            ReadLimits::default(),
            budget(),
            MapLimits {
                bytes: 100,
                line_bytes: 4096,
                intervals: 2
            }
        ),
        Err(AcquisitionError::Limit("map intervals"))
    ));
}
#[test]
fn every_quality_partition_matches_independent_byte_status_ground_truth() {
    let map = RescueMap::parse(
        "0 + 1\n0 3 +\n5 2 -\n7 2 +\n".as_bytes(),
        MapLimits::default(),
    )
    .unwrap();
    let truth = [
        AcquisitionStatus::Finished,
        AcquisitionStatus::Finished,
        AcquisitionStatus::Finished,
        AcquisitionStatus::Unknown,
        AcquisitionStatus::Unknown,
        AcquisitionStatus::BadSector,
        AcquisitionStatus::BadSector,
        AcquisitionStatus::Finished,
        AcquisitionStatus::Finished,
        AcquisitionStatus::Unknown,
        AcquisitionStatus::Unknown,
        AcquisitionStatus::Unknown,
    ];
    for start in 0..12 {
        for end in start + 1..=12 {
            let mut cursor = start;
            let mut actual = Vec::new();
            for piece in map.quality_iter(r(start, end)) {
                assert_eq!(piece.input.start(), cursor);
                cursor = piece.input.end();
                actual.extend(std::iter::repeat_n(
                    piece.status,
                    piece.input.len() as usize,
                ));
            }
            assert_eq!(cursor, end);
            assert_eq!(actual, &truth[start as usize..end as usize]);
        }
    }
}

#[test]
fn repeated_exact_geometry_shares_one_dependency_but_each_read_consumes_budget() {
    let fixture = Fixture::new(b"12345678", "0 + 1\n0 8 +\n");
    let mut source = fixture.open(
        r(0, 8),
        0,
        BindingEvidence::OperatorAttested,
        ReadLimits {
            per_read: 4,
            total: 12,
            receipts: 1,
        },
    );
    for _ in 0..2 {
        assert_eq!(source.read(r(0, 4)).unwrap().bytes, b"1234");
    }
    let masked = source.read_masked(r(0, 4)).unwrap();
    assert!(matches!(&masked[0], MaskedPiece::Available(r) if r.bytes == b"1234"));
    assert_eq!(source.consumed().len(), 1);
    assert_eq!(source.bytes_read(), 12);
    assert!(source.read(r(0, 4)).is_err());
    assert!(source.read(r(4, 8)).is_err());
    assert_eq!(source.bytes_read(), 12);
}

#[test]
fn unavailable_ranges_use_image_coordinates_after_nonzero_input_relocation() {
    let fixture = Fixture::new(b"abcdefgh", "4096 + 1\n4096 2 +\n4098 3 -\n4101 3 +\n");
    let mut source = fixture.open(
        r(4096, 4104),
        -4096,
        BindingEvidence::OperatorAttested,
        ReadLimits::default(),
    );
    assert!(
        matches!(source.read(r(0, 8)), Err(AcquisitionError::Unavailable(missing)) if missing == r(2, 5))
    );
    assert_eq!(source.bytes_read(), 0);
    let masked = source.read_masked(r(0, 8)).unwrap();
    assert!(
        matches!(&masked[1], MaskedPiece::Unavailable { image, input, .. } if *image == r(2, 5) && *input == r(4098, 4101))
    );
}
