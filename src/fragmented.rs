//! Declared fMP4 profile within one established logical file. Sequence numbers,
//! track IDs and configuration hashes never establish cross-file recording identity.
//! No playable artifact, ownership upgrade or original-order claim is produced.
use crate::{
    bmff::{
        self, BmffError, LayoutSample, Node, ParseLimits, Parser, ReadAt, Result, Track, required,
        u32be, u64be, unique,
    },
    contracts::{ByteRange, LogicalOwner},
};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trex {
    pub track_id: u32,
    pub description_index: u32,
    pub duration: u32,
    pub size: u32,
    pub flags: u32,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitMap {
    pub movie_timescale: u32,
    /// Initialization headers/configuration only; no donor sample tables.
    pub tracks: Vec<Track<LayoutSample>>,
    pub defaults: Vec<Trex>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FragmentTrack {
    pub track_id: u32,
    pub decode_time: u64,
    /// Ordinals count samples of this track in this logical file; grouping below
    /// retains original moof/traf/run positions independently of ordinals.
    pub samples: Vec<LayoutSample>,
    /// Full target sample flags, aligned with samples; retained for future transforms.
    pub sample_flags: Vec<u32>,
    pub runs: Vec<FragmentRun>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FragmentRun {
    pub ordinal: u64,
    pub first_sample: usize,
    pub sample_count: usize,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fragment {
    pub ordinal: u64,
    pub logical: ByteRange,
    pub sequence: u32,
    pub tracks: Vec<FragmentTrack>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FragmentMap {
    pub source_owner: Option<LogicalOwner>,
    pub file_size: u64,
    pub container_boxes: Vec<bmff::BoxInfo>,
    pub init: InitMap,
    pub fragments: Vec<Fragment>,
}
fn bad(reason: &'static str) -> BmffError {
    BmffError::Malformed(reason)
}
fn unresolved(reason: &'static str) -> BmffError {
    BmffError::Unresolved(reason)
}
fn full(bytes: &[u8], versions: &[u8], allowed: u32) -> Result<(u8, u32)> {
    if bytes.len() < 4 {
        return Err(bad("truncated fragment FullBox"));
    }
    let flags = u32::from_be_bytes([0, bytes[1], bytes[2], bytes[3]]);
    if !versions.contains(&bytes[0]) || flags & !allowed != 0 {
        return Err(BmffError::Unsupported("fragment version or flags"));
    }
    Ok((bytes[0], flags))
}
fn take32(bytes: &[u8], cursor: &mut usize) -> Result<u32> {
    let value = u32be(bytes, *cursor)?;
    *cursor = cursor.checked_add(4).ok_or(BmffError::ResourceLimit)?;
    Ok(value)
}
fn init<R: ReadAt>(
    parser: &mut Parser<'_, R>,
    roots: &[Node],
    limits: ParseLimits,
) -> Result<InitMap> {
    let moov = required(roots, b"moov")?;
    let mvex = required(&moov.children, b"mvex")?;
    let movie_timescale = parser.timescale(required(&moov.children, b"mvhd")?)?;
    let mut tracks = Vec::new();
    for node in moov.children.iter().filter(|n| n.info.kind == *b"trak") {
        if tracks.len() >= limits.tracks {
            return Err(BmffError::ResourceLimit);
        }
        let track = parser.track_header(node)?;
        if tracks
            .iter()
            .any(|t: &Track<LayoutSample>| t.id == track.id)
        {
            return Err(bad("duplicate initialization track"));
        }
        // Hybrid ordinary/fragmented sample tables require a separate profile.
        let mdia = required(&node.children, b"mdia")?;
        let minf = required(&mdia.children, b"minf")?;
        let stbl = required(&minf.children, b"stbl")?;
        for kind in [b"stts", b"stsc", b"stco", b"co64"] {
            if let Some(table) = unique(&stbl.children, kind)? {
                let b = parser.body(table)?;
                full(&b, &[0], 0)?;
                if u32be(&b, 4)? != 0 || b.len() != 8 {
                    return Err(BmffError::Unsupported(
                        "hybrid initialization sample tables",
                    ));
                }
            }
        }
        if let Some(sizes) = unique(&stbl.children, b"stsz")? {
            let b = parser.body(sizes)?;
            full(&b, &[0], 0)?;
            if b.len() != 12 || u32be(&b, 8)? != 0 {
                return Err(BmffError::Unsupported("hybrid initialization sample sizes"));
            }
        }
        for kind in [
            b"stz2", b"ctts", b"stss", b"sdtp", b"sgpd", b"sbgp", b"senc", b"saiz", b"saio",
        ] {
            if unique(&stbl.children, kind)?.is_some() {
                return Err(BmffError::Unsupported(
                    "initialization dependency or hybrid table",
                ));
            }
        }
        tracks.push(track);
    }
    if tracks.is_empty() {
        return Err(unresolved("missing initialization tracks"));
    }
    let mut defaults = Vec::new();
    for node in mvex.children.iter().filter(|n| n.info.kind == *b"trex") {
        if defaults.len() >= limits.tracks {
            return Err(BmffError::ResourceLimit);
        }
        let b = parser.body(node)?;
        full(&b, &[0], 0)?;
        if b.len() != 24 {
            return Err(bad("fragment defaults length"));
        }
        let trex = Trex {
            track_id: u32be(&b, 4)?,
            description_index: u32be(&b, 8)?,
            duration: u32be(&b, 12)?,
            size: u32be(&b, 16)?,
            flags: u32be(&b, 20)?,
        };
        if !tracks.iter().any(|t| t.id == trex.track_id)
            || defaults.iter().any(|t: &Trex| t.track_id == trex.track_id)
        {
            return Err(bad("fragment defaults track identity"));
        }
        defaults.push(trex);
    }
    if defaults.len() != tracks.len() {
        return Err(unresolved("incomplete fragment defaults"));
    }
    Ok(InitMap {
        movie_timescale,
        tracks,
        defaults,
    })
}

/// Resolve the declared explicit-base/default-base-is-moof profile. tfdt required;
/// repeated/reset sequence or discontinuous per-track DTS stays unresolved.
/// Payload acquisition holes retain target metadata and physical provenance.
pub fn inspect(reader: &mut impl ReadAt, limits: ParseLimits) -> Result<FragmentMap> {
    let file_size = reader.len();
    let source_owner = reader.logical_owner().cloned();
    bmff::with_container(reader, limits, |parser, roots| {
        parser.file_type(roots)?;
        let init = init(parser, roots, limits)?;
        let mdats = roots
            .iter()
            .filter(|n| n.info.kind == *b"mdat")
            .map(|n| {
                ByteRange::from_len(
                    n.info.start + n.info.header_size,
                    n.info.size - n.info.header_size,
                )
                .map_err(|_| bad("mdat range"))
            })
            .collect::<Result<Vec<_>>>()?;
        let mut fragments = Vec::new();
        let mut cursors = BTreeMap::<u32, (u64, u64)>::new();
        let mut total = 0usize;
        let mut previous_sequence: Option<u32> = None;
        for moof in roots.iter().filter(|n| n.info.kind == *b"moof") {
            let header = parser.body(required(&moof.children, b"mfhd")?)?;
            full(&header, &[0], 0)?;
            if header.len() != 8 {
                return Err(bad("movie fragment header length"));
            }
            let sequence = u32be(&header, 4)?;
            if previous_sequence.is_some_and(|old| old.checked_add(1) != Some(sequence)) {
                return Err(unresolved("fragment sequence gap repeat reset or wrap"));
            }
            previous_sequence = Some(sequence);
            let mut tracks = Vec::new();
            for traf in moof.children.iter().filter(|n| n.info.kind == *b"traf") {
                if tracks.len() >= limits.tracks {
                    return Err(BmffError::ResourceLimit);
                }
                for kind in [b"senc", b"saiz", b"saio", b"sbgp", b"sgpd", b"sdtp"] {
                    if unique(&traf.children, kind)?.is_some() {
                        return Err(BmffError::Unsupported(
                            "fragment encryption or dependency group",
                        ));
                    }
                }
                let b = parser.body(required(&traf.children, b"tfhd")?)?;
                let (_, flags) = full(&b, &[0], 0x02003b)?;
                let track_id = u32be(&b, 4)?;
                if tracks
                    .iter()
                    .any(|t: &FragmentTrack| t.track_id == track_id)
                {
                    return Err(bad("duplicate fragment track"));
                }
                let track = init
                    .tracks
                    .iter()
                    .find(|t| t.id == track_id)
                    .ok_or_else(|| unresolved("uninitialized fragment track"))?;
                let defaults = init
                    .defaults
                    .iter()
                    .find(|t| t.track_id == track_id)
                    .ok_or_else(|| unresolved("missing track defaults"))?;
                let mut p = 8;
                let base = if flags & 1 != 0 {
                    if flags & 0x020000 != 0 {
                        return Err(bad("conflicting fragment bases"));
                    }
                    let value = u64be(&b, p)?;
                    p += 8;
                    value
                } else if flags & 0x020000 != 0 {
                    moof.info.start
                } else {
                    return Err(BmffError::Unsupported("implicit previous-traf data base"));
                };
                let description = if flags & 2 != 0 {
                    take32(&b, &mut p)?
                } else {
                    defaults.description_index
                };
                let duration = if flags & 8 != 0 {
                    take32(&b, &mut p)?
                } else {
                    defaults.duration
                };
                let size = if flags & 16 != 0 {
                    take32(&b, &mut p)?
                } else {
                    defaults.size
                };
                let sample_flags = if flags & 32 != 0 {
                    take32(&b, &mut p)?
                } else {
                    defaults.flags
                };
                if p != b.len() {
                    return Err(bad("track fragment header length"));
                }
                if description == 0 || description as usize > track.descriptions.len() {
                    return Err(unresolved("fragment sample description"));
                }
                let time = parser.body(required(&traf.children, b"tfdt")?)?;
                let (version, _) = full(&time, &[0, 1], 0)?;
                let decode_time = if version == 0 {
                    if time.len() != 8 {
                        return Err(bad("decode timestamp length"));
                    }
                    u64::from(u32be(&time, 4)?)
                } else {
                    if time.len() != 12 {
                        return Err(bad("decode timestamp length"));
                    }
                    u64be(&time, 4)?
                };
                let (mut ordinal, mut dts) =
                    cursors.get(&track_id).copied().unwrap_or((0, decode_time));
                if dts != decode_time {
                    return Err(unresolved("fragment decode timestamp gap overlap or reset"));
                }
                let mut samples = Vec::new();
                let mut resolved_flags = Vec::new();
                let mut runs = Vec::new();
                let mut previous_data_end = None;
                for trun in traf.children.iter().filter(|n| n.info.kind == *b"trun") {
                    let b = parser.body(trun)?;
                    let (version, flags) = full(&b, &[0, 1], 0x000f05)?;
                    if flags & 4 != 0 && flags & 0x400 != 0 {
                        return Err(bad("conflicting first sample flags"));
                    }
                    let count = u32be(&b, 4)? as usize;
                    total = total
                        .checked_add(count)
                        .filter(|n| *n <= limits.samples)
                        .ok_or(BmffError::ResourceLimit)?;
                    if count == 0 || count > limits.table_entries {
                        return Err(unresolved("empty fragment run"));
                    }
                    let mut p = 8;
                    let mut offset = if flags & 1 != 0 {
                        let delta = take32(&b, &mut p)? as i32;
                        base.checked_add_signed(i64::from(delta))
                            .ok_or_else(|| bad("fragment data offset overflow"))?
                    } else {
                        previous_data_end.unwrap_or(base)
                    };
                    let first_flags = if flags & 4 != 0 {
                        Some(take32(&b, &mut p)?)
                    } else {
                        None
                    };
                    let first_sample = samples.len();
                    for i in 0..count {
                        let duration = if flags & 0x100 != 0 {
                            take32(&b, &mut p)?
                        } else {
                            duration
                        };
                        let size = if flags & 0x200 != 0 {
                            take32(&b, &mut p)?
                        } else {
                            size
                        };
                        let sf = if flags & 0x400 != 0 {
                            take32(&b, &mut p)?
                        } else if i == 0 {
                            first_flags.unwrap_or(sample_flags)
                        } else {
                            sample_flags
                        };
                        let cts = if flags & 0x800 != 0 {
                            let raw = take32(&b, &mut p)?;
                            if version == 0 {
                                i64::from(raw)
                            } else {
                                i64::from(raw as i32)
                            }
                        } else {
                            0
                        };
                        if duration == 0 || size == 0 {
                            return Err(unresolved("missing fragment sample duration or size"));
                        }
                        let logical = ByteRange::from_len(offset, u64::from(size))
                            .map_err(|_| bad("fragment sample range"))?;
                        if !mdats.iter().any(|m| m.contains(logical)) {
                            return Err(unresolved(
                                "fragment sample outside available mdat layout",
                            ));
                        }
                        let (source_spans, availability) = parser.payload(logical)?;
                        if sf & 0xf0000000 != 0 {
                            return Err(bad("reserved fragment sample flags"));
                        }
                        resolved_flags.push(sf);
                        samples.push(LayoutSample {
                            ordinal,
                            description_index: description,
                            logical,
                            source_spans,
                            availability,
                            dts,
                            pts: i128::from(dts) + i128::from(cts),
                            duration,
                            is_sync: sf & 0x10000 == 0,
                            dependency: ((sf >> 20) & 0xff) as u8,
                        });
                        ordinal = ordinal.checked_add(1).ok_or(BmffError::ResourceLimit)?;
                        dts = dts
                            .checked_add(u64::from(duration))
                            .ok_or_else(|| bad("fragment time overflow"))?;
                        offset = logical.end();
                    }
                    if p != b.len() {
                        return Err(bad("fragment run length"));
                    }
                    runs.push(FragmentRun {
                        ordinal: runs.len() as u64,
                        first_sample,
                        sample_count: count,
                    });
                    previous_data_end = Some(offset);
                }
                if samples.is_empty() {
                    return Err(unresolved("missing fragment runs"));
                }
                cursors.insert(track_id, (ordinal, dts));
                tracks.push(FragmentTrack {
                    track_id,
                    decode_time,
                    samples,
                    sample_flags: resolved_flags,
                    runs,
                });
            }
            if tracks.is_empty() {
                return Err(unresolved("missing fragment tracks"));
            }
            fragments.push(Fragment {
                ordinal: fragments.len() as u64,
                logical: ByteRange::from_len(moof.info.start, moof.info.size)
                    .map_err(|_| bad("moof range"))?,
                sequence,
                tracks,
            });
        }
        if fragments.is_empty() {
            return Err(unresolved("missing target fragments"));
        }
        let mut container_boxes = Vec::new();
        bmff::flatten_boxes(roots, &mut container_boxes);
        Ok(FragmentMap {
            source_owner,
            file_size,
            container_boxes,
            init,
            fragments,
        })
    })
}

impl FragmentMap {
    /// Flatten only this already established logical file. Fragment sequence,
    /// track/configuration matches never join evidence from separate files.
    pub fn into_layout(self) -> Result<bmff::SampleMap<LayoutSample>> {
        let mut tracks = self.init.tracks;
        for fragment in self.fragments {
            for fragment_track in fragment.tracks {
                let track = tracks
                    .iter_mut()
                    .find(|t| t.id == fragment_track.track_id)
                    .ok_or_else(|| unresolved("uninitialized flattened track"))?;
                for sample in fragment_track.samples {
                    if sample.ordinal != track.samples.len() as u64 {
                        return Err(unresolved("fragment original ordinal discontinuity"));
                    }
                    track.media_duration = sample
                        .dts
                        .checked_add(u64::from(sample.duration))
                        .ok_or(BmffError::ResourceLimit)?;
                    track.samples.push(sample);
                }
            }
        }
        Ok(bmff::SampleMap {
            file_size: self.file_size,
            movie_timescale: self.init.movie_timescale,
            tracks,
            container_boxes: self.container_boxes,
        })
    }
}
