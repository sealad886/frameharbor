#[allow(dead_code)]
mod common;
use common::filesystems::{Damage, Fixture as ImageFixture, FsKind, Layout};
use common::media::{Codec, MediaFixture, Moov, Timing};
use frameharbor::bmff::*;
use sha2::{Digest, Sha256};

fn cache_source(
    f: &ImageFixture,
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
                memory_bytes: 128 * 1024 * 1024,
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

fn box_(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut b = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
    b.extend(kind);
    b.extend(payload);
    b
}
fn cat(boxes: impl IntoIterator<Item = Vec<u8>>) -> Vec<u8> {
    boxes.into_iter().flatten().collect()
}
fn full(version: u8, body: &[u8]) -> Vec<u8> {
    let mut b = vec![version, 0, 0, 0];
    b.extend(body);
    b
}
fn words(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_be_bytes()).collect()
}
fn table(kind: &[u8; 4], entries: &[u32]) -> Vec<u8> {
    box_(kind, &full(0, &words(entries)))
}
fn synthetic() -> Vec<u8> {
    synthetic_tables(false, false, false)
}
fn synthetic_tables(compact: bool, wide_offsets: bool, constant: bool) -> Vec<u8> {
    let ftyp = box_(
        b"ftyp",
        &cat([b"isom".to_vec(), vec![0; 4], b"isom".to_vec()]),
    );
    let sample_bytes = b"abcdefgh123456789XYZ5678";
    let mdat = box_(b"mdat", sample_bytes);
    let base = ftyp.len() as u32 + 8;
    let mut mvhd = vec![0; 100];
    mvhd[12..16].copy_from_slice(&1000u32.to_be_bytes());
    let mut tkhd = vec![0; 84];
    tkhd[12..16].copy_from_slice(&1u32.to_be_bytes());
    let mut mdhd = vec![0; 24];
    mdhd[12..16].copy_from_slice(&1000u32.to_be_bytes());
    mdhd[16..20].copy_from_slice(&60u32.to_be_bytes());
    let mut hdlr = vec![0; 24];
    hdlr[8..12].copy_from_slice(b"vide");
    let url = box_(b"url ", &[0, 0, 0, 1]);
    let dref = box_(b"dref", &full(0, &cat([words(&[1]), url])));
    let dinf = box_(b"dinf", &dref);
    let mut entry = vec![0; 78];
    entry[6..8].copy_from_slice(&1u16.to_be_bytes());
    entry.extend(box_(
        b"avcC",
        &[1, 66, 0, 30, 255, 225, 0, 2, 0x67, 0, 1, 0, 1, 0x68],
    ));
    let stsd = box_(
        b"stsd",
        &full(0, &cat([words(&[1]), box_(b"avc1", &entry)])),
    );
    let stsz = if compact {
        box_(
            b"stz2",
            &cat([vec![0, 0, 0, 0, 0, 0, 0, 4], words(&[3]), vec![0x89, 0x70]]),
        )
    } else if constant {
        table(b"stsz", &[8, 3])
    } else {
        table(b"stsz", &[0, 3, 8, 9, 7])
    };
    let stts = table(b"stts", &[3, 1, 10, 1, 20, 1, 30]);
    let ctts = box_(
        b"ctts",
        &full(1, &words(&[3, 1, 20, 1, (-10i32) as u32, 1, 0])),
    );
    let stsc = table(b"stsc", &[2, 1, 2, 1, 2, 1, 1]);
    let second = base + if constant { 16 } else { 17 };
    let stco = if wide_offsets {
        box_(
            b"co64",
            &full(
                0,
                &cat([
                    words(&[2]),
                    u64::from(base).to_be_bytes().to_vec(),
                    u64::from(second).to_be_bytes().to_vec(),
                ]),
            ),
        )
    } else {
        table(b"stco", &[2, base, second])
    };
    let stss = table(b"stss", &[1, 1]);
    let sdtp = box_(b"sdtp", &full(0, &[0x20, 0x10, 0x10]));
    let stbl = box_(
        b"stbl",
        &cat([stsd, stsz, stts, ctts, stsc, stco, stss, sdtp]),
    );
    let minf = box_(b"minf", &cat([dinf, stbl]));
    let mdia = box_(
        b"mdia",
        &cat([box_(b"mdhd", &mdhd), box_(b"hdlr", &hdlr), minf]),
    );
    let trak = box_(b"trak", &cat([box_(b"tkhd", &tkhd), mdia]));
    cat([
        ftyp,
        mdat,
        box_(b"moov", &cat([box_(b"mvhd", &mvhd), trak])),
    ])
}
fn locate(bytes: &[u8], kind: &[u8; 4]) -> usize {
    bytes.windows(4).position(|w| w == kind).unwrap() - 4
}
fn parse(bytes: &[u8]) -> Result<SampleMap> {
    inspect(&mut SliceReader::new(bytes), ParseLimits::default())
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn exact_sample_tables_preserve_byte_identity_decode_order_vfr_and_signed_composition() {
    let bytes = synthetic();
    let map = parse(&bytes).unwrap();
    let track = &map.tracks[0];
    assert_eq!(track.id, 1);
    assert_eq!(track.timescale, 1000);
    assert_eq!(track.samples.len(), 3);
    assert_eq!(
        track
            .samples
            .iter()
            .map(|s| (s.ordinal, s.dts, s.pts, s.duration, s.is_sync))
            .collect::<Vec<_>>(),
        vec![
            (0, 0, 20, 10, true),
            (1, 10, 0, 20, false),
            (2, 30, 30, 30, false)
        ]
    );
    for sample in &track.samples {
        let expected: &[u8] =
            &bytes[sample.logical.start() as usize..sample.logical.end() as usize];
        assert_eq!(sample.sha256, <[u8; 32]>::from(Sha256::digest(expected)));
    }
    assert_eq!(track.descriptions[0].nal_length_size, Some(4));
    assert_eq!(track.descriptions[0].codec, *b"avc1");
}

#[test]
fn box_size_parent_containment_overflow_and_count_budgets_fail_closed() {
    let valid = synthetic();
    for bad in [
        vec![0, 0, 0, 4, b'f', b'r', b'e', b'e'],
        vec![
            0, 0, 0, 1, b'm', b'd', b'a', b't', 255, 255, 255, 255, 255, 255, 255, 255,
        ],
        vec![0, 0, 0, 1, b'f', b'r', b'e', b'e', 0, 0, 0],
    ] {
        assert!(parse(&bad).is_err());
    }
    let mut mutated = valid.clone();
    let stsz = locate(&mutated, b"stsz");
    mutated[stsz..stsz + 4].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(parse(&mutated).is_err());
    assert!(matches!(
        inspect(
            &mut SliceReader::new(&valid),
            ParseLimits {
                boxes: 1,
                ..ParseLimits::default()
            }
        ),
        Err(BmffError::ResourceLimit)
    ));
    let mut nested = box_(b"free", &[]);
    for _ in 0..20 {
        nested = box_(b"moov", &nested);
    }
    assert!(matches!(parse(&nested), Err(BmffError::ResourceLimit)));
}

#[test]
fn missing_tables_external_references_and_wrong_cardinality_never_become_exact() {
    for kind in [b"moov", b"stsz", b"stts", b"stco"] {
        let mut b = synthetic();
        let p = locate(&b, kind);
        b[p + 4..p + 8].copy_from_slice(b"free");
        assert!(parse(&b).is_err());
    }
    let mut b = synthetic();
    let p = locate(&b, b"url ");
    b[p + 11] = 0;
    assert!(matches!(
        parse(&b),
        Err(BmffError::Unsupported("external data reference"))
    ));
    let mut b = synthetic();
    let p = locate(&b, b"stts");
    b[p + 16..p + 20].copy_from_slice(&4u32.to_be_bytes());
    assert!(parse(&b).is_err());
    let mut b = synthetic();
    let p = locate(&b, b"stsc");
    b[p + 16..p + 20].copy_from_slice(&2u32.to_be_bytes());
    assert!(parse(&b).is_err());
    let mut b = synthetic();
    let p = locate(&b, b"stco");
    b[p + 16..p + 20].copy_from_slice(&0u32.to_be_bytes());
    assert!(parse(&b).is_err());
    let mut b = synthetic();
    let p = locate(&b, b"stsz");
    b[p + 16..p + 20].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(matches!(parse(&b), Err(BmffError::ResourceLimit)));
}

#[test]
fn legal_extended_and_end_of_parent_boxes_are_bounded_without_payload_buffering() {
    let mut bytes = synthetic();
    let original = bytes.len();
    bytes.extend([0, 0, 0, 1, b'f', b'r', b'e', b'e']);
    bytes.extend(24u64.to_be_bytes());
    bytes.extend([0; 8]);
    bytes.extend([0, 0, 0, 0, b'f', b'r', b'e', b'e']);
    bytes.extend([0; 7]);
    let map = parse(&bytes).unwrap();
    assert_eq!(map.file_size, (original + 39) as u64);
    assert_eq!(map.container_boxes.last().unwrap().size, 15);
}

#[test]
fn compact_constant_sizes_and_64bit_offsets_resolve_the_same_target_samples() {
    let regular = parse(&synthetic()).unwrap();
    for (compact, wide, constant) in [
        (true, false, false),
        (false, true, false),
        (true, true, false),
        (false, false, true),
    ] {
        let bytes = synthetic_tables(compact, wide, constant);
        let map = parse(&bytes).unwrap();
        let samples = &map.tracks[0].samples;
        assert_eq!(samples.len(), 3);
        if !constant {
            assert_eq!(samples, &regular.tracks[0].samples);
        } else {
            assert_eq!(
                samples.iter().map(|s| s.logical.len()).collect::<Vec<_>>(),
                vec![8, 8, 8]
            );
        }
    }
}

#[test]
fn conflicting_sizes_and_media_duration_metadata_are_rejected() {
    let mut bytes = synthetic();
    let p = locate(&bytes, b"mdhd");
    bytes[p + 24..p + 28].copy_from_slice(&61u32.to_be_bytes());
    assert!(matches!(
        parse(&bytes),
        Err(BmffError::Malformed(
            "media duration and decode table disagree"
        ))
    ));
    let mut bytes = synthetic();
    let p = locate(&bytes, b"stco");
    let first = bytes[p + 16..p + 20].to_vec();
    bytes[p + 20..p + 24].copy_from_slice(&first);
    assert!(matches!(
        parse(&bytes),
        Err(BmffError::Malformed("overlapping sample payloads"))
    ));
}

#[test]
fn warm_logical_windows_avoid_repeated_source_admissions_and_remain_generation_bound() {
    use std::{
        fs::OpenOptions,
        io::{Seek, SeekFrom, Write},
    };
    let f = ImageFixture::build(FsKind::ExFat, Layout::Fragmented, Damage::None).unwrap();
    let (mut session, view) = cache_source(&f);
    let before = session.image_bytes_read_this_run();
    let stats;
    {
        let mut reader = LogicalReader::new(&mut session, &view);
        reader.reserve(1024 * 1024).unwrap();
        let mut data = [0u8; 8];
        for offset in 0..100u64 {
            reader.read_exact(offset * 8, &mut data).unwrap();
            assert_eq!(
                data,
                &f.ground_truth.payload[offset as usize * 8..offset as usize * 8 + 8]
            );
        }
        stats = reader.stats();
        assert_eq!(stats.logical_requests, 100);
        assert!(stats.cache_hits >= 99);
        assert_eq!(stats.cache_fills, 1);
        assert_eq!(stats.source_bytes, 65536);
        let mut changed = OpenOptions::new().write(true).open(&f.image).unwrap();
        changed
            .seek(SeekFrom::Start(f.ground_truth.spans[0].image_offset + 8))
            .unwrap();
        changed.write_all(b"changed!").unwrap();
        changed.sync_all().unwrap();
        assert!(matches!(
            reader.read_exact(8, &mut data),
            Err(BmffError::Session(
                frameharbor::session::SessionError::SourceChanged
            ))
        ));
    }
    assert_eq!(
        session.image_bytes_read_this_run() - before,
        stats.source_bytes
    );
}

#[test]
fn unavailable_prefetch_bytes_do_not_poison_available_precise_metadata_reads() {
    let f = ImageFixture::build(FsKind::ExFat, Layout::Fragmented, Damage::None).unwrap();
    let hole = f.ground_truth.spans[0].image_offset + 32;
    let size = f.ground_truth.logical_image_bytes;
    std::fs::write(
        &f.map,
        format!(
            "0 + 1\n0 {hole} +\n{hole} 4 -\n{} {} +\n",
            hole + 4,
            size - hole - 4
        ),
    )
    .unwrap();
    let (mut session, view) = cache_source(&f);
    let before = session.image_bytes_read_this_run();
    let stats;
    {
        let mut reader = LogicalReader::new(&mut session, &view);
        reader.reserve(1024 * 1024).unwrap();
        let mut data = [0u8; 8];
        reader.read_exact(0, &mut data).unwrap();
        assert_eq!(data, &f.ground_truth.payload[..8]);
        assert!(reader.read_exact(32, &mut data[..4]).is_err());
        reader.read_exact(40, &mut data).unwrap();
        assert_eq!(data, &f.ground_truth.payload[40..48]);
        reader.finish().unwrap();
        stats = reader.stats();
    }
    assert_eq!(stats.cache_fills, 0);
    assert_eq!(stats.precise_reads, 2);
    assert_eq!(stats.source_bytes, 16);
    assert_eq!(session.image_bytes_read_this_run() - before, 16);
}

#[test]
fn parser_settles_and_terminal_authority_takes_precedence_over_parse_failure() {
    struct Settled<'a> {
        reader: SliceReader<'a>,
        finished: bool,
    }
    impl ReadAt for Settled<'_> {
        fn len(&self) -> u64 {
            self.reader.len()
        }
        fn read_exact(&mut self, offset: u64, bytes: &mut [u8]) -> Result<()> {
            self.reader.read_exact(offset, bytes)
        }
        fn finish(&mut self) -> Result<()> {
            self.finished = true;
            Err(BmffError::ResourceLimit)
        }
    }
    let bytes = synthetic();
    let mut reader = Settled {
        reader: SliceReader::new(&bytes),
        finished: false,
    };
    assert!(matches!(
        inspect(&mut reader, ParseLimits::default()),
        Err(BmffError::ResourceLimit)
    ));
    assert!(reader.finished);
    let mut reader = Settled {
        reader: SliceReader::new(&[0; 4]),
        finished: false,
    };
    assert!(matches!(
        inspect(&mut reader, ParseLimits::default()),
        Err(BmffError::ResourceLimit)
    ));
    assert!(reader.finished);
}

#[test]
fn procedural_avc_hevc_audio_bframes_vfr_and_head_tail_moov_match_independent_packet_truth() {
    for codec in [Codec::Avc, Codec::Hevc] {
        for moov in [Moov::Head, Moov::Tail] {
            for timing in [Timing::Cfr, Timing::Vfr] {
                let fixture = MediaFixture::generate(codec, moov, timing).unwrap();
                let map = parse(&fixture.bytes).unwrap();
                assert_eq!(map.tracks.len(), fixture.streams.len());
                for truth in &fixture.streams {
                    let track = map.tracks.iter().find(|t| t.id == truth.id).unwrap();
                    assert_eq!(i64::from(track.timescale), truth.time_base.1);
                    assert_eq!(truth.time_base.0, 1);
                    if truth.codec_type == "audio" {
                        assert_eq!(track.descriptions[0].audio_channels, Some(1));
                        assert_eq!(track.descriptions[0].audio_sample_rate, Some(48000));
                    }
                    assert_eq!(
                        hex(&track.descriptions[0].configuration_sha256),
                        truth.extradata_sha256
                    );
                    let packets = fixture
                        .packets
                        .iter()
                        .filter(|p| p.stream_index == truth.index)
                        .collect::<Vec<_>>();
                    assert_eq!(track.samples.len(), packets.len());
                    for (sample, packet) in track.samples.iter().zip(packets) {
                        assert_eq!(sample.ordinal, packet.ordinal as u64);
                        assert_eq!(sample.logical.start(), packet.pos);
                        assert_eq!(sample.logical.len(), packet.size);
                        assert_eq!(sample.dts as i64, packet.dts);
                        assert_eq!(sample.pts, i128::from(packet.pts));
                        assert_eq!(i64::from(sample.duration), packet.duration);
                        assert_eq!(hex(&sample.sha256), packet.sha256);
                    }
                }
            }
        }
    }
}

#[test]
fn truncated_composition_headers_and_reserved_table_flags_fail_without_panics() {
    for length in 1..=3 {
        for version in [0u8, 1] {
            let mut bytes = synthetic();
            let at = locate(&bytes, b"ctts");
            let old = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
            let removed = old - 8 - length;
            bytes.drain(at + 8 + length..at + old);
            bytes[at..at + 4].copy_from_slice(&((8 + length) as u32).to_be_bytes());
            bytes[at + 8] = version;
            for kind in [b"moov", b"trak", b"mdia", b"minf", b"stbl"] {
                let ancestor = locate(&bytes, kind);
                let size = u32::from_be_bytes(bytes[ancestor..ancestor + 4].try_into().unwrap());
                bytes[ancestor..ancestor + 4]
                    .copy_from_slice(&(size - removed as u32).to_be_bytes());
            }
            let outcome = std::panic::catch_unwind(|| parse(&bytes));
            assert!(outcome.is_ok());
            assert!(outcome.unwrap().is_err());
        }
    }
    for kind in [b"stts", b"stsc", b"stco", b"stss"] {
        let mut bytes = synthetic();
        let at = locate(&bytes, kind);
        bytes[at + 11] = 1;
        assert!(matches!(parse(&bytes), Err(BmffError::Unsupported(_))));
    }
}

#[test]
fn repeated_chunk_geometry_is_rejected_before_rehash_or_span_expansion() {
    struct Counted<'a> {
        inner: SliceReader<'a>,
        data: frameharbor::contracts::ByteRange,
        payload: usize,
        spans: std::cell::Cell<usize>,
    }
    impl ReadAt for Counted<'_> {
        fn len(&self) -> u64 {
            self.inner.len()
        }
        fn read_exact(&mut self, at: u64, b: &mut [u8]) -> Result<()> {
            if at >= self.data.start() && at + b.len() as u64 <= self.data.end() {
                self.payload += b.len();
            }
            self.inner.read_exact(at, b)
        }
        fn source_spans(&self, _: frameharbor::contracts::ByteRange) -> Result<Vec<SourceSpan>> {
            self.spans.set(self.spans.get() + 1);
            Ok(Vec::new())
        }
    }
    let mut bytes = synthetic();
    let offset = locate(&bytes, b"stco");
    let first = bytes[offset + 16..offset + 20].to_vec();
    bytes[offset + 20..offset + 24].copy_from_slice(&first);
    let mdat = locate(&bytes, b"mdat");
    let mut reader = Counted {
        inner: SliceReader::new(&bytes),
        data: frameharbor::contracts::ByteRange::from_len(mdat as u64 + 8, 24).unwrap(),
        payload: 0,
        spans: std::cell::Cell::new(0),
    };
    assert!(matches!(
        inspect(&mut reader, ParseLimits::default()),
        Err(BmffError::Malformed("overlapping sample payloads"))
    ));
    assert_eq!(reader.payload, 17);
    assert_eq!(reader.spans.get(), 2);
}

#[test]
fn physical_span_vectors_consume_the_declared_parser_memory_budget() {
    struct Fragmented<'a>(SliceReader<'a>);
    impl ReadAt for Fragmented<'_> {
        fn len(&self) -> u64 {
            self.0.len()
        }
        fn read_exact(&mut self, at: u64, b: &mut [u8]) -> Result<()> {
            self.0.read_exact(at, b)
        }
        fn source_spans(
            &self,
            range: frameharbor::contracts::ByteRange,
        ) -> Result<Vec<SourceSpan>> {
            Ok((range.start()..range.end())
                .map(|at| SourceSpan {
                    logical: frameharbor::contracts::ByteRange::from_len(at, 1).unwrap(),
                    image: frameharbor::contracts::ByteRange::from_len(4096 + at * 2, 1).unwrap(),
                })
                .collect())
        }
    }
    let bytes = synthetic();
    let mut reader = Fragmented(SliceReader::new(&bytes));
    let limits = ParseLimits {
        samples: 3,
        metadata_bytes: 4096,
        memory_bytes: 3 * 384 + 4096,
        ..ParseLimits::default()
    };
    assert!(matches!(
        inspect(&mut reader, limits),
        Err(BmffError::ResourceLimit)
    ));
}

#[test]
fn acquisition_hole_preserves_target_identity_geometry_and_other_sample_hashes() {
    use frameharbor::contracts::ByteRange;
    let bytes = synthetic();
    let exact = inspect(&mut SliceReader::new(&bytes), ParseLimits::default()).unwrap();
    let mut payload = bytes.clone();
    payload.extend(box_(b"free", &vec![0; 17000]));
    let mut fixture = ImageFixture::build(FsKind::Fat32, Layout::Fragmented, Damage::None).unwrap();
    fixture.replace_payload(&payload).unwrap();
    let missing_sample = &exact.tracks[0].samples[1];
    let start = fixture.ground_truth.spans[0].image_offset + missing_sample.logical.start();
    let end = start + missing_sample.logical.len();
    let size = fixture.ground_truth.logical_image_bytes;
    std::fs::write(
        &fixture.map,
        format!(
            "0 + 1\n0 {start} +\n{start} {} -\n{end} {} +\n",
            end - start,
            size - end
        ),
    )
    .unwrap();
    // Underlying destination retains original nonzero bytes, but map forbids them.
    let (mut session, view) = cache_source(&fixture);
    let layout = inspect_layout(
        &mut LogicalReader::new(&mut session, &view),
        ParseLimits::default(),
    )
    .unwrap();
    for (actual, expected) in layout.tracks[0]
        .samples
        .iter()
        .zip(&exact.tracks[0].samples)
    {
        assert_eq!(
            (
                actual.ordinal,
                actual.logical,
                actual.dts,
                actual.pts,
                actual.duration
            ),
            (
                expected.ordinal,
                expected.logical,
                expected.dts,
                expected.pts,
                expected.duration
            )
        );
        assert!(!actual.source_spans.is_empty());
        if actual.ordinal == 1 {
            assert_eq!(
                actual.availability,
                SampleAvailability::Unavailable {
                    reason: AcquisitionMissing {
                        image: ByteRange::new(start, end).unwrap()
                    }
                }
            );
        } else {
            assert_eq!(
                actual.availability,
                SampleAvailability::Available {
                    sha256: expected.sha256
                }
            );
        }
    }
    assert!(
        inspect(
            &mut LogicalReader::new(&mut session, &view),
            ParseLimits::default()
        )
        .is_err()
    );
}

#[test]
fn donor_never_fills_missing_target_tables_or_overrides_target_timing() {
    let bytes = synthetic();
    let mut donor = inspect(&mut SliceReader::new(&bytes), ParseLimits::default()).unwrap();
    donor.tracks[0].descriptions[0].configuration[1] ^= 1;
    donor.tracks[0].samples[0].dts = 9876;
    let result = inspect_damage(
        &mut SliceReader::new(&bytes),
        ParseLimits::default(),
        Some(&donor),
    )
    .unwrap();
    let DamageRoute::TargetLayout {
        map,
        donor: assessment,
    } = result
    else {
        panic!("target layout lost")
    };
    assert_eq!(assessment, DonorAssessment::ConfigurationRejected);
    assert_eq!(map.tracks[0].samples[0].dts, 0);
    let moov = locate(&bytes, b"moov");
    for target in [&bytes[..moov], &bytes[..bytes.len() - 1]] {
        assert!(matches!(
            inspect_damage(
                &mut SliceReader::new(target),
                ParseLimits::default(),
                Some(&donor)
            )
            .unwrap(),
            DamageRoute::Unresolved {
                donor: DonorAssessment::UnqualifiedWithoutTarget,
                ..
            }
        ));
    }
}

#[test]
fn damage_layout_does_not_downgrade_source_changes_or_upgrade_owner_evidence() {
    struct Changed<'a>(SliceReader<'a>);
    impl ReadAt for Changed<'_> {
        fn len(&self) -> u64 {
            self.0.len()
        }
        fn read_exact(&mut self, offset: u64, bytes: &mut [u8]) -> Result<()> {
            if offset == 28 {
                return Err(BmffError::Filesystem(
                    frameharbor::filesystem::FilesystemError::Session(
                        frameharbor::session::SessionError::SourceChanged,
                    ),
                ));
            }
            self.0.read_exact(offset, bytes)
        }
    }
    let bytes = synthetic();
    assert!(matches!(
        inspect_damage(
            &mut Changed(SliceReader::new(&bytes)),
            ParseLimits::default(),
            None
        ),
        Err(BmffError::Filesystem(
            frameharbor::filesystem::FilesystemError::Session(
                frameharbor::session::SessionError::SourceChanged
            )
        ))
    ));
    let fixture = ImageFixture::build(FsKind::Fat32, Layout::Fragmented, Damage::None).unwrap();
    let (mut session, mut view) = cache_source(&fixture);
    view.owner.source = frameharbor::contracts::SourceGeneration("foreign-generation".into());
    assert!(matches!(
        inspect_damage(
            &mut LogicalReader::new(&mut session, &view),
            ParseLimits::default(),
            None
        ),
        Err(BmffError::Filesystem(
            frameharbor::filesystem::FilesystemError::SourceMismatch
        ))
    ));
    view.owner.source = session.source_generation();
    for evidence in [
        frameharbor::filesystem::ExtentEvidence::DeletedOrRecovery,
        frameharbor::filesystem::ExtentEvidence::SharedGeometry,
    ] {
        view.evidence = evidence;
        assert!(matches!(
            inspect_layout(
                &mut LogicalReader::new(&mut session, &view),
                ParseLimits::default()
            ),
            Err(BmffError::Filesystem(
                frameharbor::filesystem::FilesystemError::Unresolved
            ))
        ));
    }
}

#[test]
fn damaged_metadata_does_not_swallow_finish_source_or_control_faults() {
    struct Guard {
        stopped: bool,
    }
    impl ReadAt for Guard {
        fn len(&self) -> u64 {
            4
        }
        fn read_exact(&mut self, _: u64, _: &mut [u8]) -> Result<()> {
            unreachable!()
        }
        fn finish(&mut self) -> Result<()> {
            Err(BmffError::Session(if self.stopped {
                frameharbor::session::SessionError::Stopped
            } else {
                frameharbor::session::SessionError::SourceChanged
            }))
        }
    }
    assert!(matches!(
        inspect_damage(&mut Guard { stopped: false }, ParseLimits::default(), None),
        Err(BmffError::Session(
            frameharbor::session::SessionError::SourceChanged
        ))
    ));
    assert!(matches!(
        inspect_damage(&mut Guard { stopped: true }, ParseLimits::default(), None),
        Err(BmffError::Session(
            frameharbor::session::SessionError::Stopped
        ))
    ));
}
