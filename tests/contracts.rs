use frameharbor::contracts::*;
use std::time::Duration;

#[test]
fn ranges_are_half_open_checked_and_intersections_never_invent_bytes() {
    assert!(ByteRange::new(4, 4).is_err());
    assert!(ByteRange::new(5, 4).is_err());
    assert!(ByteRange::from_len(u64::MAX, 1).is_err());
    let a = ByteRange::new(2, 8).unwrap();
    assert_eq!(a.len(), 6);
    assert_eq!(a.intersection(ByteRange::new(8, 10).unwrap()), None);
    assert_eq!(
        a.intersection(ByteRange::new(5, 9).unwrap()),
        Some(ByteRange::new(5, 8).unwrap())
    );
    for start in 0..32 {
        for len in 1..32 {
            let r = ByteRange::from_len(start, len).unwrap();
            assert!(r.contains(r));
            assert_eq!(r.intersection(a), a.intersection(r));
        }
    }
}

#[test]
fn explicit_relocation_roundtrips_and_rejects_underflow_or_out_of_image() {
    for displacement in [-100i128, -7, 0, 13, 100] {
        let b = CoordinateBinding::new(
            ByteRange::new(100, 200).unwrap(),
            displacement,
            1000,
            BindingEvidence::OperatorAttested,
        )
        .unwrap();
        for i in 100..199 {
            let r = ByteRange::new(i, i + 1).unwrap();
            assert_eq!(b.image_to_input(b.input_to_image(r).unwrap()).unwrap(), r);
        }
    }
    assert!(
        CoordinateBinding::new(
            ByteRange::new(0, 8).unwrap(),
            -1,
            20,
            BindingEvidence::OperatorAttested
        )
        .is_err()
    );
    assert!(
        CoordinateBinding::new(
            ByteRange::new(0, 8).unwrap(),
            20,
            20,
            BindingEvidence::OperatorAttested
        )
        .is_err()
    );
    assert!(
        CoordinateBinding::new(
            ByteRange::new(0, 8).unwrap(),
            i128::MAX,
            20,
            BindingEvidence::OperatorAttested
        )
        .is_err()
    );
    let b = CoordinateBinding::new(
        ByteRange::new(0, 8).unwrap(),
        0,
        8,
        BindingEvidence::Unverified,
    )
    .unwrap();
    assert_eq!(b.require_supported(), Err(ContractError::UnverifiedBinding));
    assert!(b.input_to_image(ByteRange::new(7, 9).unwrap()).is_err());
}

#[test]
fn historical_partition_offset_is_never_added_to_partition_image_reads() {
    let fs = FilesystemCoordinates {
        start_in_image: 0,
        length: 4096,
        original_partition_start: Some(2048 * 512),
    };
    let r = ByteRange::new(512, 1024).unwrap();
    assert_eq!(fs.to_image(r, 4096).unwrap(), r);
    assert_eq!(
        fs.to_original_device(r).unwrap().unwrap().start(),
        2048 * 512 + 512
    );
    assert!(
        fs.to_image(ByteRange::new(4090, 4100).unwrap(), 4096)
            .is_err()
    );
}

#[test]
fn only_finished_acquisition_bytes_are_available() {
    for (c, available) in [
        ('+', true),
        ('?', false),
        ('*', false),
        ('/', false),
        ('-', false),
    ] {
        assert_eq!(
            AcquisitionStatus::from_map_char(c).unwrap().available(),
            available
        );
    }
    assert!(!AcquisitionStatus::Unknown.available());
    assert_eq!(AcquisitionStatus::from_map_char('x'), None);
}

fn exact_claims() -> Claims {
    Claims {
        bytes: ByteProvenance::AcquisitionSupported,
        ownership: Ownership::MetadataSupported,
        structure: Structure::Valid,
        decode: Decode::StrictPass,
        temporal: Temporal::OriginalMappingSupported,
        coverage: Coverage::CompleteWithinKnownLayout { samples: 10 },
    }
}
fn verification() -> SampleVerification {
    SampleVerification {
        bytes: true,
        identity: true,
        order: true,
        timing: true,
        configuration: true,
        audio: true,
    }
}

#[test]
fn successful_decode_cannot_upgrade_order_timing_ownership_or_coverage() {
    let mut claims = exact_claims();
    assert!(claims.require_exact_samples(verification()).is_ok());
    claims.temporal = Temporal::ConstrainedHypothesis;
    claims.coverage = Coverage::UnknownTotal;
    assert!(claims.require_playable().is_ok());
    assert_eq!(
        claims.require_exact_samples(verification()),
        Err(ContractError::MissingEvidence)
    );
    assert_eq!(claims.coverage, Coverage::UnknownTotal);
    for owner in [
        Ownership::SharedGeometry,
        Ownership::HistoricalConflict,
        Ownership::Unresolved,
    ] {
        let mut c = exact_claims();
        c.ownership = owner;
        assert!(c.require_exact_samples(verification()).is_err());
    }
    for bytes in [
        ByteProvenance::Synthesized,
        ByteProvenance::Unavailable,
        ByteProvenance::BindingUnverified,
    ] {
        let mut c = exact_claims();
        c.bytes = bytes;
        assert!(c.require_exact_samples(verification()).is_err());
    }
}

#[test]
fn staged_sample_order_timing_configuration_and_audio_are_independent_requirements() {
    for missing in 0..6 {
        let mut v = verification();
        match missing {
            0 => v.bytes = false,
            1 => v.identity = false,
            2 => v.order = false,
            3 => v.timing = false,
            4 => v.configuration = false,
            _ => v.audio = false,
        }
        assert!(exact_claims().require_exact_samples(v).is_err());
    }
    for decode in [
        Decode::Failure,
        Decode::Untested,
        Decode::TimedOut,
        Decode::Unsupported,
    ] {
        let mut c = exact_claims();
        c.decode = decode;
        assert!(c.require_exact_samples(verification()).is_ok());
        assert!(c.require_playable().is_err());
    }
}

#[test]
fn sample_exactness_does_not_imply_complete_recording() {
    let mut c = exact_claims();
    c.coverage = Coverage::KnownGaps {
        recovered: 4,
        listed: 10,
    };
    assert!(c.require_exact_samples(verification()).is_ok());
    c.coverage = Coverage::KnownGaps {
        recovered: 10,
        listed: 10,
    };
    assert!(c.validate().is_err());
    assert_eq!(
        Metric {
            numerator: 10,
            denominator: None
        }
        .ratio()
        .unwrap(),
        None
    );
    assert!(
        Metric {
            numerator: 0,
            denominator: Some(0)
        }
        .ratio()
        .is_err()
    );
    assert!(
        Metric {
            numerator: 11,
            denominator: Some(10)
        }
        .ratio()
        .is_err()
    );
    assert_eq!(
        Metric {
            numerator: 4,
            denominator: Some(10)
        }
        .ratio()
        .unwrap(),
        Some(0.4)
    );
}

fn caps() -> ResourceCaps {
    ResourceCaps {
        read_bytes: 10,
        memory_bytes: 10,
        index_bytes: 10,
        output_bytes: 10,
        visited_postings: 10,
        transitions: 10,
        decoder_seconds: 10,
    }
}

#[test]
fn reserve_and_cumulative_elapsed_survive_resume_without_reset() {
    let mut b = Budget::new(
        Duration::from_secs(NORMAL_TOTAL_SECONDS),
        Duration::from_secs(60),
        caps(),
    )
    .unwrap();
    b.account_elapsed(Duration::from_secs(NORMAL_TOTAL_SECONDS - 60))
        .unwrap();
    assert!(!b.may_schedule());
    let resumed = b.clone();
    assert_eq!(resumed.elapsed(), b.elapsed());
    assert_eq!(resumed.remaining(), Duration::from_secs(60));
    assert!(
        Budget::new(
            Duration::from_secs(NORMAL_TOTAL_SECONDS + 1),
            Duration::ZERO,
            caps()
        )
        .is_err()
    );
    assert!(Budget::new(Duration::from_secs(60), Duration::from_secs(60), caps()).is_err());
}

#[test]
fn every_resource_is_charged_before_work_and_rejected_charge_is_atomic() {
    for resource in [
        Resource::ReadBytes,
        Resource::MemoryBytes,
        Resource::IndexBytes,
        Resource::OutputBytes,
        Resource::VisitedPostings,
        Resource::Transitions,
        Resource::DecoderSeconds,
    ] {
        let mut b = Budget::new(Duration::from_secs(100), Duration::from_secs(10), caps()).unwrap();
        assert_eq!(b.charge(resource, 10), Ok(()));
        let before = b.used();
        assert_eq!(b.charge(resource, 1), Err(TerminalReason::BudgetExhausted));
        assert_eq!(b.used(), before);
        assert!(b.charge(resource, u64::MAX).is_err());
    }
}

#[test]
fn fresh_deadline_authorization_preserves_reserve_and_resource_counters() {
    let mut b = Budget::new(Duration::from_secs(100), Duration::from_secs(10), caps()).unwrap();
    b.authorize_at(Duration::from_secs(89), Resource::ReadBytes, 4)
        .unwrap();
    let before = b.used();
    assert_eq!(
        b.authorize_at(Duration::from_secs(90), Resource::ReadBytes, 1),
        Err(TerminalReason::BudgetExhausted)
    );
    assert_eq!(b.used(), before);
    assert_eq!(b.elapsed(), Duration::from_secs(90));
    assert_eq!(
        b.charge(Resource::ReadBytes, 1),
        Err(TerminalReason::BudgetExhausted)
    );
    assert_eq!(
        b.authorize_at(Duration::ZERO, Resource::ReadBytes, 1),
        Err(TerminalReason::BudgetExhausted)
    );
    assert_eq!(b.elapsed(), Duration::from_secs(90));
    assert_eq!(b.used(), before);
    assert_eq!(
        b.authorize_at(Duration::from_secs(101), Resource::ReadBytes, 0),
        Err(TerminalReason::BudgetExhausted)
    );
    assert_eq!(b.remaining(), Duration::ZERO);
    assert_eq!(b.elapsed(), Duration::from_secs(101));
}

#[test]
fn resource_rejection_still_accounts_for_elapsed_processing_time() {
    let mut b = Budget::new(Duration::from_secs(100), Duration::from_secs(10), caps()).unwrap();
    assert_eq!(
        b.authorize_at(Duration::from_secs(20), Resource::ReadBytes, 11),
        Err(TerminalReason::BudgetExhausted)
    );
    assert_eq!(b.used().read_bytes, 0);
    assert_eq!(b.elapsed(), Duration::from_secs(20));
    b.authorize_at(Duration::from_secs(19), Resource::ReadBytes, 10)
        .unwrap();
    assert_eq!(b.elapsed(), Duration::from_secs(20));
    assert_eq!(b.used().read_bytes, 10);
}

#[test]
fn control_state_cannot_publish_unvalidated_or_restart_completed_work() {
    let s = WorkState::Pending.transition(WorkState::Running).unwrap();
    assert!(s.transition(WorkState::Publishing).is_err());
    let s = s
        .transition(WorkState::Paused)
        .unwrap()
        .transition(WorkState::Running)
        .unwrap();
    let s = s
        .transition(WorkState::Validated)
        .unwrap()
        .transition(WorkState::Publishing)
        .unwrap()
        .transition(WorkState::Complete)
        .unwrap();
    assert!(s.transition(WorkState::Running).is_err());
    assert!(WorkState::Cancelled.transition(WorkState::Running).is_err());
    assert_eq!(DEFAULT_HISTORICAL_FRONTIERS, 2);
}

#[test]
fn equal_content_does_not_collapse_distinct_geometry_or_owners() {
    let source = SourceGeneration("synthetic-1".into());
    let a = PhysicalGeometry {
        source: source.clone(),
        ordered_spans: vec![ByteRange::new(0, 10).unwrap()],
    };
    let b = PhysicalGeometry {
        source: source.clone(),
        ordered_spans: vec![ByteRange::new(20, 30).unwrap()],
    };
    assert_ne!(a, b);
    let owner_a = LogicalOwner {
        source: source.clone(),
        filesystem: "fat32".into(),
        file_identity: "a".into(),
    };
    let owner_b = LogicalOwner {
        source,
        filesystem: "fat32".into(),
        file_identity: "b".into(),
    };
    assert_ne!(owner_a, owner_b);
}
