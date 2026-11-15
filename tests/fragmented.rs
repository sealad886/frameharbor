#[allow(dead_code)]
mod common;
use common::media::{Codec, MediaFixture, Moov, Timing};
use frameharbor::{
    bmff::{self, BmffError, ParseLimits, SampleAvailability, SliceReader},
    fragmented,
};
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
#[test]
fn generated_fragments_match_independent_native_packet_truth_without_cross_file_joining() {
    for codec in [Codec::Avc, Codec::Hevc] {
        let fixture = MediaFixture::generate(codec, Moov::Fragmented, Timing::Vfr).unwrap();
        let map = fragmented::inspect(
            &mut SliceReader::new(&fixture.bytes),
            ParseLimits::default(),
        )
        .unwrap();
        assert!(map.fragments.len() >= 2);
        assert!(matches!(
            bmff::inspect(
                &mut SliceReader::new(&fixture.bytes),
                ParseLimits::default()
            ),
            Err(BmffError::Unsupported(_))
        ));
        for stream in &fixture.streams {
            let track = map.init.tracks.iter().find(|t| t.id == stream.id).unwrap();
            assert_eq!(track.timescale as i64, stream.time_base.1);
            assert_eq!(
                hex(&track.descriptions[0].configuration_sha256),
                stream.extradata_sha256
            );
            let samples: Vec<_> = map
                .fragments
                .iter()
                .flat_map(|f| f.tracks.iter())
                .filter(|t| t.track_id == stream.id)
                .flat_map(|t| &t.samples)
                .collect();
            let truth: Vec<_> = fixture
                .packets
                .iter()
                .filter(|p| p.stream_index == stream.index)
                .collect();
            assert_eq!(samples.len(), truth.len());
            for (sample, packet) in samples.iter().zip(truth) {
                assert_eq!(sample.ordinal, packet.ordinal as u64);
                assert_eq!(
                    (sample.logical.start(), sample.logical.len()),
                    (packet.pos, packet.size)
                );
                assert_eq!(
                    (sample.dts as i64, sample.pts as i64, sample.duration as i64),
                    (packet.dts, packet.pts, packet.duration)
                );
                let SampleAvailability::Available { sha256 } = sample.availability else {
                    panic!("generated payload unavailable")
                };
                assert_eq!(hex(&sha256), packet.sha256);
                assert_eq!(sample.is_sync, packet.flags.contains('K'));
            }
        }
        let fragment_ranges = map.fragments.iter().map(|f| f.logical).collect::<Vec<_>>();
        let layout = map.into_layout().unwrap();
        let retained_moofs = layout
            .container_boxes
            .iter()
            .filter(|b| b.kind == *b"moof")
            .collect::<Vec<_>>();
        assert_eq!(retained_moofs.len(), fragment_ranges.len());
        for (retained, original) in retained_moofs.iter().zip(fragment_ranges) {
            assert_eq!(
                (retained.start, retained.size),
                (original.start(), original.len())
            );
            assert_eq!(
                &fixture.bytes[retained.start as usize + 4..retained.start as usize + 8],
                b"moof"
            );
        }
    }
}

fn box_(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = ((body.len() + 8) as u32).to_be_bytes().to_vec();
    out.extend(kind);
    out.extend(body);
    out
}
fn full(version: u8, flags: u32, words: &[u32]) -> Vec<u8> {
    let mut b = flags.to_be_bytes().to_vec();
    b[0] = version;
    b.extend(words.iter().flat_map(|w| w.to_be_bytes()));
    b
}
fn positions(bytes: &[u8], kind: &[u8; 4]) -> Vec<usize> {
    bytes
        .windows(4)
        .enumerate()
        .filter(|(i, w)| *i >= 4 && *w == kind)
        .map(|(i, _)| i)
        .collect()
}
fn manual_fragment(
    sequence: u32,
    time: u64,
    track: u32,
    absolute_moof: u64,
    explicit: bool,
) -> Vec<u8> {
    let build = |offset: i32| {
        let tfhd = if explicit {
            let mut b = full(0, 1, &[track]);
            b.extend(absolute_moof.to_be_bytes());
            box_(b"tfhd", &b)
        } else {
            box_(b"tfhd", &full(0, 0x020000, &[track]))
        };
        let mut time_body = full(1, 0, &[]);
        time_body.extend(time.to_be_bytes());
        let mut traf = tfhd;
        traf.extend(box_(b"tfdt", &time_body));
        traf.extend(box_(
            b"trun",
            &full(1, 0x801, &[1, offset as u32, (-7i32) as u32]),
        ));
        // No data offset means continuation of this traf's previous run.
        traf.extend(box_(b"trun", &full(0, 0, &[1])));
        let mut moof = box_(b"mfhd", &full(0, 0, &[sequence]));
        moof.extend(box_(b"traf", &traf));
        box_(b"moof", &moof)
    };
    let mut result = build((build(0).len() + 8) as i32);
    result.extend(box_(b"mdat", b"abcdef"));
    result
}
fn manual_file(explicit: bool) -> (Vec<u8>, u32) {
    let fixture = MediaFixture::generate(Codec::Avc, Moov::Fragmented, Timing::Cfr).unwrap();
    let cut = positions(&fixture.bytes, b"moof")[0] - 4;
    let track = fixture
        .streams
        .iter()
        .find(|s| s.codec_type == "video")
        .unwrap()
        .id;
    let mut init = fixture.bytes[..cut].to_vec();
    for pos in positions(&init, b"trex") {
        if u32::from_be_bytes(init[pos + 8..pos + 12].try_into().unwrap()) == track {
            init[pos + 16..pos + 20].copy_from_slice(&11u32.to_be_bytes());
            init[pos + 20..pos + 24].copy_from_slice(&3u32.to_be_bytes());
            init[pos + 24..pos + 28].copy_from_slice(&0x02000000u32.to_be_bytes());
        }
    }
    let fragment = manual_fragment(1, 0, track, init.len() as u64, explicit);
    init.extend(fragment);
    (init, track)
}
#[test]
fn trex_defaults_explicit_or_moof_base_run_continuation_and_signed_composition_resolve() {
    for explicit in [false, true] {
        let (bytes, track) = manual_file(explicit);
        let map =
            fragmented::inspect(&mut SliceReader::new(&bytes), ParseLimits::default()).unwrap();
        let samples = &map.fragments[0].tracks[0].samples;
        assert_eq!(map.fragments[0].tracks[0].track_id, track);
        assert_eq!(map.fragments[0].tracks[0].runs.len(), 2);
        assert_eq!(
            (
                samples[0].ordinal,
                samples[0].dts,
                samples[0].pts,
                samples[0].duration
            ),
            (0, 0, -7, 11)
        );
        assert_eq!(
            (
                samples[1].ordinal,
                samples[1].dts,
                samples[1].pts,
                samples[1].duration
            ),
            (1, 11, 11, 11)
        );
        assert_eq!(samples[0].logical.end(), samples[1].logical.start());
        for (sample, original) in samples.iter().zip([b"abc", b"def"]) {
            let SampleAvailability::Available { sha256 } = sample.availability else {
                panic!("unexpected hole")
            };
            use sha2::{Digest, Sha256};
            assert_eq!(sha256.as_slice(), Sha256::digest(original).as_slice());
        }
    }
}
#[test]
fn missing_init_defaults_timestamps_bad_bases_and_sequence_resets_stay_unresolved() {
    let (bytes, track) = manual_file(false);
    let parse = |b: &[u8]| fragmented::inspect(&mut SliceReader::new(b), ParseLimits::default());
    let mut missing = bytes.clone();
    let pos = positions(&missing, b"moov")[0];
    missing[pos..pos + 4].copy_from_slice(b"free");
    assert!(matches!(parse(&missing), Err(BmffError::Unresolved(_))));
    let mut missing = bytes.clone();
    let pos = positions(&missing, b"tfdt")[0];
    missing[pos..pos + 4].copy_from_slice(b"free");
    assert!(matches!(parse(&missing), Err(BmffError::Unresolved(_))));
    let mut missing = bytes.clone();
    for pos in positions(&missing, b"trex") {
        if u32::from_be_bytes(missing[pos + 8..pos + 12].try_into().unwrap()) == track {
            missing[pos + 16..pos + 20].copy_from_slice(&0u32.to_be_bytes());
        }
    }
    assert!(matches!(parse(&missing), Err(BmffError::Unresolved(_))));
    let mut wrong = bytes.clone();
    let pos = positions(&wrong, b"trun")[0];
    wrong[pos + 12..pos + 16].copy_from_slice(&i32::MIN.to_be_bytes());
    assert!(parse(&wrong).is_err());
    let mut wrong = bytes.clone();
    let pos = positions(&wrong, b"tfhd")[0];
    wrong[pos + 8..pos + 12].copy_from_slice(&777u32.to_be_bytes());
    assert!(matches!(parse(&wrong), Err(BmffError::Unresolved(_))));
    let mut wrong = bytes.clone();
    let pos = positions(&wrong, b"tfhd")[0];
    wrong[pos + 4..pos + 8].fill(0);
    assert!(matches!(parse(&wrong), Err(BmffError::Unsupported(_))));
    let mut repeat = bytes.clone();
    repeat.extend(manual_fragment(1, 22, track, repeat.len() as u64, false));
    assert!(matches!(parse(&repeat), Err(BmffError::Unresolved(_))));
    let mut reset = bytes.clone();
    reset.extend(manual_fragment(2, 0, track, reset.len() as u64, false));
    assert!(matches!(parse(&reset), Err(BmffError::Unresolved(_))));
    let mut gap = bytes.clone();
    gap.extend(manual_fragment(2, 25, track, gap.len() as u64, false));
    assert!(matches!(parse(&gap), Err(BmffError::Unresolved(_))));
    let mut consecutive = bytes.clone();
    consecutive.extend(manual_fragment(
        2,
        22,
        track,
        consecutive.len() as u64,
        false,
    ));
    assert_eq!(
        parse(&consecutive).unwrap().fragments[1].tracks[0].samples[0].ordinal,
        2
    );
}

#[test]
fn explicit_base_supports_a_negative_signed_data_offset_without_guessing() {
    let (mut bytes, _) = manual_file(true);
    let sample_start = (positions(&bytes, b"mdat")[0] + 4) as u64;
    let tfhd = positions(&bytes, b"tfhd")[0];
    bytes[tfhd + 12..tfhd + 20].copy_from_slice(&(sample_start + 6).to_be_bytes());
    let trun = positions(&bytes, b"trun")[0];
    bytes[trun + 12..trun + 16].copy_from_slice(&(-6i32).to_be_bytes());
    let map = fragmented::inspect(&mut SliceReader::new(&bytes), ParseLimits::default()).unwrap();
    assert_eq!(
        map.fragments[0].tracks[0].samples[0].logical.start(),
        sample_start
    );
    assert_eq!(map.fragments[0].tracks[0].sample_flags, vec![0x02000000; 2]);
}

#[test]
fn missing_fragment_payload_keeps_owner_ordinals_flags_and_known_sample_hashes() {
    use bmff::{ReadAt, Result, SourceSpan};
    use frameharbor::{
        acquisition::AcquisitionError,
        contracts::{ByteRange, LogicalOwner, SourceGeneration},
        filesystem::FilesystemError,
        session::SessionError,
    };
    struct Hole<'a> {
        reader: SliceReader<'a>,
        hole: ByteRange,
        owner: LogicalOwner,
    }
    impl ReadAt for Hole<'_> {
        fn len(&self) -> u64 {
            self.reader.len()
        }
        fn logical_owner(&self) -> Option<&LogicalOwner> {
            Some(&self.owner)
        }
        fn read_exact(&mut self, offset: u64, bytes: &mut [u8]) -> Result<()> {
            let request = ByteRange::from_len(offset, bytes.len() as u64).unwrap();
            if request.intersection(self.hole).is_some() {
                return Err(BmffError::Filesystem(FilesystemError::Session(
                    SessionError::Acquisition(AcquisitionError::Unavailable(
                        ByteRange::from_len(1_000_000 + self.hole.start(), self.hole.len())
                            .unwrap(),
                    )),
                )));
            }
            self.reader.read_exact(offset, bytes)
        }
        fn source_spans(&self, logical: ByteRange) -> Result<Vec<SourceSpan>> {
            Ok(vec![SourceSpan {
                logical,
                image: ByteRange::from_len(1_000_000 + logical.start(), logical.len()).unwrap(),
            }])
        }
    }
    let (bytes, _) = manual_file(false);
    let intact =
        fragmented::inspect(&mut SliceReader::new(&bytes), ParseLimits::default()).unwrap();
    let hole = intact.fragments[0].tracks[0].samples[0].logical;
    let owner = LogicalOwner {
        source: SourceGeneration("bound-fixture".into()),
        filesystem: "qualified-test".into(),
        file_identity: "target-file".into(),
    };
    let map = fragmented::inspect(
        &mut Hole {
            reader: SliceReader::new(&bytes),
            hole,
            owner: owner.clone(),
        },
        ParseLimits::default(),
    )
    .unwrap();
    assert_eq!(map.source_owner, Some(owner));
    assert_eq!(map.fragments[0].tracks[0].samples[0].logical, hole);
    assert!(
        !map.fragments[0].tracks[0].samples[0]
            .source_spans
            .is_empty()
    );
    assert!(matches!(
        map.fragments[0].tracks[0].samples[0].availability,
        SampleAvailability::Unavailable { .. }
    ));
    assert_eq!(
        map.fragments[0].tracks[0].samples[1].availability,
        intact.fragments[0].tracks[0].samples[1].availability
    );
    assert_eq!(
        map.fragments[0].tracks[0].sample_flags,
        intact.fragments[0].tracks[0].sample_flags
    );
    let limited = ParseLimits {
        samples: 1,
        ..ParseLimits::default()
    };
    assert!(matches!(
        fragmented::inspect(&mut SliceReader::new(&bytes), limited),
        Err(BmffError::ResourceLimit)
    ));
}
