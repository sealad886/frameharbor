//! Conservative closed-IDR segment plans and bounded encoded-preserving MP4 muxing.
//! Plans do not certify playable output; staged bytes still require independent
//! source mapping, structural comparison and complete strict decoder validation.
use crate::{
    bmff::{
        self, BmffError, LayoutSample, ParseLimits, ReadAt, Result, Sample, SampleAvailability,
        SampleMap, Track,
    },
    contracts::{ByteRange, LogicalOwner},
    session::SessionError,
};
use sha2::{Digest, Sha256};
use std::io::Write;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioGapReason {
    UnalignedStart,
    UnavailablePayload,
    PartialTail,
    UnsupportedConfiguration,
    NoSamples,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransformLabel {
    SynthesizedHeaders,
    TimestampRebase {
        origin_ticks: u64,
        timescale: u32,
    },
    DroppedTargetEdits,
    MissingAudio {
        track_id: u32,
        reason: AudioGapReason,
    },
}
#[derive(Debug, Clone, Copy)]
pub struct SegmentLimits {
    pub segments: usize,
    pub per_segment_samples: usize,
    pub per_segment_spans: usize,
    pub receipt_bytes: u64,
    pub samples: usize,
    pub sample_bytes: u64,
    pub read_bytes: u64,
    pub output_bytes: u64,
    pub memory_bytes: u64,
}
impl Default for SegmentLimits {
    fn default() -> Self {
        Self {
            segments: 16,
            per_segment_samples: 1024,
            per_segment_spans: 2048,
            receipt_bytes: 768 * 1024,
            samples: 100000,
            sample_bytes: 32 * 1024 * 1024,
            read_bytes: 512 * 1024 * 1024,
            output_bytes: 512 * 1024 * 1024,
            memory_bytes: 256 * 1024 * 1024,
        }
    }
}
#[derive(Debug, Clone)]
pub struct TransformationPlan {
    owner: Option<LogicalOwner>,
    source_counts: Vec<(u32, u64)>,
    selected: SampleMap,
    expected: SampleMap,
    labels: Vec<TransformLabel>,
    header: Vec<u8>,
    copies: Vec<CopyRange>,
}
#[derive(Debug, Clone)]
struct CopyRange {
    source: Sample,
    output: ByteRange,
}
impl TransformationPlan {
    pub fn owner(&self) -> Option<&LogicalOwner> {
        self.owner.as_ref()
    }
    pub fn source_counts(&self) -> &[(u32, u64)] {
        &self.source_counts
    }
    pub fn selected_source(&self) -> &SampleMap {
        &self.selected
    }
    pub fn expected_output(&self) -> &SampleMap {
        &self.expected
    }
    pub fn labels(&self) -> &[TransformLabel] {
        &self.labels
    }
    /// Conservative owned-plan plus streaming writer envelope. Fresh source
    /// parsing/native validation require their own additional admission.
    pub fn memory_bound(&self) -> u64 {
        fn map_memory(map: &SampleMap) -> u64 {
            let mut n = std::mem::size_of::<SampleMap>() as u64
                + map.tracks.capacity() as u64 * std::mem::size_of::<Track>() as u64
                + map.container_boxes.capacity() as u64
                    * std::mem::size_of::<bmff::BoxInfo>() as u64;
            for track in &map.tracks {
                n += track.samples.capacity() as u64 * std::mem::size_of::<Sample>() as u64
                    + track.descriptions.capacity() as u64
                        * std::mem::size_of::<bmff::SampleDescription>() as u64
                    + track.edits.capacity() as u64 * std::mem::size_of::<bmff::Edit>() as u64
                    + track.sample_groups.capacity() as u64
                        * std::mem::size_of::<bmff::SampleGroup>() as u64;
                for sample in &track.samples {
                    n += sample.source_spans.capacity() as u64
                        * std::mem::size_of::<bmff::SourceSpan>() as u64;
                }
                for d in &track.descriptions {
                    n += (d.sample_entry.capacity() + d.configuration.capacity()) as u64;
                }
                for g in &track.sample_groups {
                    n += g.descriptions.capacity() as u64 * std::mem::size_of::<Vec<u8>>() as u64
                        + g.assignments.capacity() as u64 * 8
                        + g.descriptions
                            .iter()
                            .map(|d| d.capacity() as u64)
                            .sum::<u64>();
                }
            }
            n
        }
        let owner = self.owner.as_ref().map_or(0, |o| {
            o.source.0.capacity() + o.filesystem.capacity() + o.file_identity.capacity()
        });
        std::mem::size_of::<Self>() as u64
            + map_memory(&self.selected)
            + map_memory(&self.expected)
            + self.header.capacity() as u64
            + self.copies.capacity() as u64 * std::mem::size_of::<CopyRange>() as u64
            + self
                .copies
                .iter()
                .map(|c| {
                    c.source.source_spans.capacity() as u64
                        * std::mem::size_of::<bmff::SourceSpan>() as u64
                })
                .sum::<u64>()
            + self.source_counts.capacity() as u64 * 16
            + self.labels.capacity() as u64 * std::mem::size_of::<TransformLabel>() as u64
            + owner as u64
            + 256 * 1024
    }
    pub fn output_bytes(&self) -> u64 {
        self.expected.file_size
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnresolvedSamples {
    pub track_id: u32,
    pub total: u64,
    pub selected: u64,
}
#[derive(Debug)]
pub struct SegmentPlans {
    pub plans: Vec<TransformationPlan>,
    pub remainder: Vec<UnresolvedSamples>,
}
fn unresolved(reason: &'static str) -> BmffError {
    BmffError::Unresolved(reason)
}
fn available(sample: &LayoutSample) -> Option<Sample> {
    let SampleAvailability::Available { sha256 } = sample.availability else {
        return None;
    };
    Some(Sample {
        ordinal: sample.ordinal,
        description_index: sample.description_index,
        logical: sample.logical,
        source_spans: sample.source_spans.clone(),
        sha256,
        dts: sample.dts,
        pts: sample.pts,
        duration: sample.duration,
        is_sync: sample.is_sync,
        dependency: sample.dependency,
    })
}
fn read_sample(reader: &mut impl ReadAt, sample: &Sample, cap: u64) -> Result<Vec<u8>> {
    if sample.logical.len() > cap {
        return Err(BmffError::ResourceLimit);
    }
    let mut bytes = vec![0; sample.logical.len() as usize];
    for (i, part) in bytes.chunks_mut(65536).enumerate() {
        reader.read_exact(sample.logical.start() + i as u64 * 65536, part)?;
    }
    if Sha256::digest(&bytes).as_slice() != sample.sha256 {
        return Err(BmffError::Session(SessionError::SourceChanged));
    }
    Ok(bytes)
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Access {
    ClosedIdr,
    Following,
    Barrier,
}
fn access(bytes: &[u8], codec: [u8; 4], width: u8, sync: bool) -> Access {
    if ![1, 2, 4].contains(&width) {
        return Access::Barrier;
    }
    let mut p = 0usize;
    let mut vcl = false;
    let mut idr = false;
    let mut ordinary = false;
    while p < bytes.len() {
        let Some(prefix) = bytes.get(p..p + width as usize) else {
            return Access::Barrier;
        };
        p += width as usize;
        let n = prefix
            .iter()
            .fold(0usize, |a, b| (a << 8) | usize::from(*b));
        let Some(end) = p.checked_add(n) else {
            return Access::Barrier;
        };
        let Some(nal) = bytes.get(p..end).filter(|n| !n.is_empty()) else {
            return Access::Barrier;
        };
        p = end;
        if nal[0] & 0x80 != 0 {
            return Access::Barrier;
        }
        if codec == *b"avc1" {
            match nal[0] & 31 {
                5 if nal[0] & 0x60 != 0 => {
                    vcl = true;
                    idr = true
                }
                1 => {
                    vcl = true;
                    ordinary = true
                }
                6 | 9 | 12 => {}
                _ => return Access::Barrier,
            }
        } else if codec == *b"hvc1" {
            if nal.len() < 2 || nal[1] & 7 == 0 || nal[0] & 1 != 0 || nal[1] >> 3 != 0 {
                return Access::Barrier;
            }
            match (nal[0] >> 1) & 63 {
                20 => {
                    vcl = true;
                    idr = true
                }
                0..=5 => {
                    vcl = true;
                    ordinary = true
                }
                35 | 38 | 39 | 40 => {}
                // CRA/BLA/IDR_W_RADL, leading pictures and in-band config stay
                // outside this first declared closed-IDR_N_LP profile.
                _ => return Access::Barrier,
            }
        } else {
            return Access::Barrier;
        }
    }
    if !vcl || idr && ordinary {
        Access::Barrier
    } else if idr && sync {
        Access::ClosedIdr
    } else if ordinary && !sync {
        Access::Following
    } else {
        Access::Barrier
    }
}
/// Target metadata is not authenticated merely by passing it here. The staging
/// boundary must independently reparse the qualified source and compare selected
/// original identities. Payloads are hashed here and again while writing.
pub fn plan(
    reader: &mut impl ReadAt,
    target: &SampleMap<LayoutSample>,
    limits: SegmentLimits,
) -> Result<SegmentPlans> {
    let result = {
        let mut bounded = PlanningReader {
            reader,
            remaining: limits.read_bytes,
        };
        plan_inner(&mut bounded, target, limits)
    };
    reader.finish()?;
    result
}
struct PlanningReader<'a, R: ReadAt> {
    reader: &'a mut R,
    remaining: u64,
}
impl<R: ReadAt> ReadAt for PlanningReader<'_, R> {
    fn len(&self) -> u64 {
        self.reader.len()
    }
    fn logical_owner(&self) -> Option<&LogicalOwner> {
        self.reader.logical_owner()
    }
    fn source_spans(&self, range: ByteRange) -> Result<Vec<bmff::SourceSpan>> {
        self.reader.source_spans(range)
    }
    fn reserve(&mut self, memory: u64) -> Result<()> {
        self.reader.reserve(memory)
    }
    fn read_exact(&mut self, offset: u64, bytes: &mut [u8]) -> Result<()> {
        self.remaining = self
            .remaining
            .checked_sub(bytes.len() as u64)
            .ok_or(BmffError::ResourceLimit)?;
        self.reader.read_exact(offset, bytes)
    }
}
fn plan_inner(
    reader: &mut impl ReadAt,
    target: &SampleMap<LayoutSample>,
    limits: SegmentLimits,
) -> Result<SegmentPlans> {
    if limits.segments == 0
        || limits.segments > 64
        || limits.per_segment_samples == 0
        || limits.per_segment_samples > 4096
        || limits.per_segment_spans == 0
        || limits.per_segment_spans > 16384
        || limits.receipt_bytes == 0
        || limits.receipt_bytes > 768 * 1024
        || limits.samples == 0
        || limits.samples > 1000000
        || limits.sample_bytes == 0
        || limits.sample_bytes > 64 * 1024 * 1024
        || limits.memory_bytes > 1024 * 1024 * 1024
        || limits.memory_bytes < limits.sample_bytes * 2
        || reader.len() != target.file_size
    {
        return Err(BmffError::ResourceLimit);
    }
    if target.movie_timescale == 0 || target.tracks.iter().any(|t| t.timescale == 0) {
        return Err(unresolved("missing target timebase"));
    }
    let mut entries = target
        .tracks
        .iter()
        .flat_map(|t| &t.descriptions)
        .try_fold(0u64, |n, d| {
            n.checked_add((d.sample_entry.len() + d.configuration.len()) as u64)
                .ok_or(BmffError::ResourceLimit)
        })?;
    for track in &target.tracks {
        entries = entries
            .checked_add(track.edits.len() as u64 * 24)
            .ok_or(BmffError::ResourceLimit)?;
        for group in &track.sample_groups {
            let group_bytes = group.descriptions.iter().try_fold(0u64, |n, d| {
                n.checked_add(d.capacity() as u64 + 24)
                    .ok_or(BmffError::ResourceLimit)
            })?;
            entries = entries
                .checked_add(group_bytes)
                .and_then(|n| n.checked_add(group.assignments.capacity() as u64 * 8))
                .ok_or(BmffError::ResourceLimit)?;
        }
    }
    let sample_memory = target
        .tracks
        .iter()
        .flat_map(|t| &t.samples)
        .try_fold(0u64, |n, s| {
            n.checked_add(1024 + s.source_spans.len() as u64 * 128)
                .ok_or(BmffError::ResourceLimit)
        })?;
    if sample_memory
        .checked_add(entries.saturating_mul(limits.segments as u64 * 4))
        .and_then(|n| n.checked_add(limits.sample_bytes * 2))
        .is_none_or(|n| n > limits.memory_bytes)
    {
        return Err(BmffError::ResourceLimit);
    }
    reader.reserve(limits.memory_bytes)?;
    let owner = reader.logical_owner().cloned();
    let videos: Vec<_> = target
        .tracks
        .iter()
        .filter(|t| t.handler == *b"vide")
        .collect();
    if videos.len() != 1 {
        return Err(unresolved(
            "segment profile requires one established video track",
        ));
    }
    let video = videos[0];
    if !video.sample_groups.is_empty() {
        return Err(BmffError::Unsupported(
            "video dependency groups in segment profile",
        ));
    }
    let total: usize = target.tracks.iter().map(|t| t.samples.len()).sum();
    if total > limits.samples {
        return Err(BmffError::ResourceLimit);
    }
    let mut read = 0u64;
    let mut intervals = Vec::new();
    let mut closed_starts = Vec::new();
    let mut start = None;
    let mut closed_end = None;
    let mut description = None;
    let mut expected_dts = None;
    for (i, sample) in video.samples.iter().enumerate() {
        if i % 128 == 0 {
            reader.reserve(limits.memory_bytes)?;
        }
        let class = if let Some(exact) = available(sample) {
            read = read
                .checked_add(exact.logical.len())
                .filter(|n| *n <= limits.read_bytes)
                .ok_or(BmffError::ResourceLimit)?;
            let desc = video
                .descriptions
                .iter()
                .find(|d| d.index == sample.description_index)
                .ok_or_else(|| unresolved("missing target sample configuration"))?;
            let bytes = read_sample(reader, &exact, limits.sample_bytes)?;
            access(
                &bytes,
                desc.codec,
                desc.nal_length_size.unwrap_or(0),
                sample.is_sync,
            )
        } else {
            Access::Barrier
        };
        let continuous = expected_dts.is_none_or(|dts| dts == sample.dts)
            && description.is_none_or(|d| d == sample.description_index);
        if !continuous || class == Access::Barrier {
            if let (Some(a), Some(b)) = (start, closed_end) {
                intervals.push((a, b));
            }
            start = None;
            closed_end = None;
            description = None;
            expected_dts = None;
        }
        if class == Access::ClosedIdr {
            closed_starts.push(i);
            if start.is_some() {
                closed_end = Some(i)
            } else {
                start = Some(i)
            }
            description = Some(sample.description_index);
        }
        if start.is_some() {
            expected_dts = sample.dts.checked_add(u64::from(sample.duration));
        }
    }
    // EOF is target-table EOF, not an arbitrary truncation before a hole.
    if let Some(a) = start {
        intervals.push((a, video.samples.len()));
    }
    let mut bounded_intervals = Vec::new();
    'intervals: for (a, b) in intervals {
        let lo = closed_starts.partition_point(|i| *i < a);
        let hi = closed_starts.partition_point(|i| *i < b);
        let mut boundaries = closed_starts[lo..hi].to_vec();
        boundaries.push(b);
        let mut current: Option<(usize, usize)> = None;
        for (batch, pair) in boundaries.windows(2).enumerate() {
            if batch % 128 == 0 {
                reader.reserve(limits.memory_bytes)?;
            }
            let (gop_start, gop_end) = (pair[0], pair[1]);
            if !receipt_fits(target, video, gop_start, gop_end, limits)? {
                if let Some(interval) = current.take() {
                    bounded_intervals.push(interval);
                }
                // An oversized single GOP stays unresolved, never partially copied.
                continue;
            }
            match current {
                Some((start, end))
                    if end == gop_start && receipt_fits(target, video, start, gop_end, limits)? =>
                {
                    current = Some((start, gop_end))
                }
                Some(interval) => {
                    bounded_intervals.push(interval);
                    current = Some((gop_start, gop_end));
                }
                None => current = Some((gop_start, gop_end)),
            }
            if bounded_intervals.len() >= limits.segments {
                break 'intervals;
            }
        }
        if let Some(interval) = current {
            bounded_intervals.push(interval);
        }
        if bounded_intervals.len() >= limits.segments {
            break;
        }
    }
    let counts = target
        .tracks
        .iter()
        .map(|t| (t.id, t.samples.len() as u64))
        .collect::<Vec<_>>();
    let mut plans = Vec::new();
    let mut chosen = std::collections::BTreeMap::<u32, u64>::new();
    let mut aggregate = 0u64;
    for (a, b) in bounded_intervals {
        if a >= b {
            continue;
        }
        if plans.len() >= limits.segments {
            break;
        }
        let samples = video.samples[a..b]
            .iter()
            .map(|s| available(s).ok_or_else(|| unresolved("unavailable selected sample")))
            .collect::<Result<Vec<_>>>()?;
        let origin = samples[0].dts;
        let end = samples
            .last()
            .unwrap()
            .dts
            .checked_add(u64::from(samples.last().unwrap().duration))
            .ok_or(BmffError::ResourceLimit)?;
        let mut selected = Vec::new();
        selected.push(Track {
            id: video.id,
            timescale: video.timescale,
            media_duration: video.media_duration,
            handler: video.handler,
            descriptions: video.descriptions.clone(),
            edits: video.edits.clone(),
            samples,
            sample_groups: video.sample_groups.clone(),
        });
        let mut labels = vec![
            TransformLabel::SynthesizedHeaders,
            TransformLabel::TimestampRebase {
                origin_ticks: origin,
                timescale: video.timescale,
            },
        ];
        if target.tracks.iter().any(|t| !t.edits.is_empty()) {
            labels.push(TransformLabel::DroppedTargetEdits)
        }
        let (audio_tracks, audio_labels) = audio_selection(
            target,
            video,
            origin,
            end,
            limits.per_segment_samples - (b - a),
        )?;
        selected.extend(audio_tracks);
        labels.extend(audio_labels);
        for t in &selected {
            *chosen.entry(t.id).or_default() += t.samples.len() as u64;
        }
        let selected = SampleMap {
            file_size: target.file_size,
            movie_timescale: target.movie_timescale,
            tracks: selected,
            container_boxes: Vec::new(),
        };
        let output = build(
            reader,
            selected,
            owner.clone(),
            counts.clone(),
            labels,
            limits,
        )?;
        aggregate = aggregate
            .checked_add(output.output_bytes())
            .filter(|n| *n <= limits.output_bytes)
            .ok_or(BmffError::ResourceLimit)?;
        plans.push(output);
    }
    let remainder = counts
        .iter()
        .map(|(id, total)| UnresolvedSamples {
            track_id: *id,
            total: *total,
            selected: chosen.get(id).copied().unwrap_or(0),
        })
        .collect();
    Ok(SegmentPlans { plans, remainder })
}
/// Source parsers establish positive, nonoverlapping decode intervals. Binary
/// searches locate only the contained target slice; no whole-track rescan.
fn audio_window(
    audio: &Track<LayoutSample>,
    video_scale: u32,
    origin: u64,
    end: u64,
) -> (&[LayoutSample], Option<AudioGapReason>) {
    let start_tick = u128::from(origin) * u128::from(audio.timescale);
    let end_tick = u128::from(end) * u128::from(audio.timescale);
    let lo = audio
        .samples
        .partition_point(|s| u128::from(s.dts) * u128::from(video_scale) < start_tick);
    let hi = audio.samples.partition_point(|s| {
        (u128::from(s.dts) + u128::from(s.duration)) * u128::from(video_scale) <= end_tick
    });
    let within = &audio.samples[lo..hi.max(lo)];
    let reason = if within.is_empty() {
        Some(AudioGapReason::NoSamples)
    } else if u128::from(within[0].dts) * u128::from(video_scale) != start_tick {
        Some(AudioGapReason::UnalignedStart)
    } else if within[0].ordinal != 0
        || audio.descriptions.len() != 1
        || audio.descriptions[0].codec != *b"mp4a"
        || !audio.sample_groups.is_empty()
    {
        Some(AudioGapReason::UnsupportedConfiguration)
    } else {
        None
    };
    (within, reason)
}
fn receipt_fits(
    target: &SampleMap<LayoutSample>,
    video: &Track<LayoutSample>,
    a: usize,
    b: usize,
    limits: SegmentLimits,
) -> Result<bool> {
    let mut count = b - a;
    if count > limits.per_segment_samples {
        return Ok(false);
    }
    let start = video.samples[a].dts;
    let last = &video.samples[b - 1];
    let end = last
        .dts
        .checked_add(u64::from(last.duration))
        .ok_or(BmffError::ResourceLimit)?;
    let mut spans = video.samples[a..b]
        .iter()
        .map(|s| s.source_spans.len())
        .sum::<usize>();
    if spans > limits.per_segment_spans {
        return Ok(false);
    }
    let mut configs = video.descriptions.len();
    for audio in target.tracks.iter().filter(|t| t.handler == *b"soun") {
        let (within, reason) = audio_window(audio, video.timescale, start, end);
        if reason.is_some() {
            continue;
        }
        // Conservative preflight precedes scanning/copying candidate metadata.
        // An oversized audio window leaves this GOP unresolved even if an
        // unavailable marker might otherwise allow omitting that audio.
        if within.len() > limits.per_segment_samples - count {
            return Ok(false);
        }
        if within
            .iter()
            .any(|s| !matches!(s.availability, SampleAvailability::Available { .. }))
        {
            continue;
        }
        count += within.len();
        spans += within.iter().map(|s| s.source_spans.len()).sum::<usize>();
        if spans > limits.per_segment_spans {
            return Ok(false);
        }
        configs += audio.descriptions.len();
    }
    let estimate = 4096u64
        .saturating_add(count as u64 * 512)
        .saturating_add(spans as u64 * 128)
        .saturating_add(configs as u64 * 256);
    Ok(estimate <= limits.receipt_bytes)
}
fn audio_selection(
    target: &SampleMap<LayoutSample>,
    video: &Track<LayoutSample>,
    origin: u64,
    end: u64,
    sample_limit: usize,
) -> Result<(Vec<Track>, Vec<TransformLabel>)> {
    let mut selected = Vec::new();
    let mut labels = Vec::new();
    let mut count = 0;
    for audio in target.tracks.iter().filter(|t| t.handler == *b"soun") {
        let (within, mut reason) = audio_window(audio, video.timescale, origin, end);
        if reason.is_none() {
            if within.len() > sample_limit - count {
                return Err(unresolved(
                    "audio selection exceeds bounded segment mappings",
                ));
            }
            if within
                .iter()
                .any(|s| !matches!(s.availability, SampleAvailability::Available { .. }))
            {
                reason = Some(AudioGapReason::UnavailablePayload);
            }
        }
        if let Some(reason) = reason {
            labels.push(TransformLabel::MissingAudio {
                track_id: audio.id,
                reason,
            });
            continue;
        }
        count += within.len();
        let samples = within
            .iter()
            .map(|s| available(s).unwrap())
            .collect::<Vec<_>>();
        let last = samples.last().unwrap();
        if (u128::from(last.dts) + u128::from(last.duration)) * u128::from(video.timescale)
            != u128::from(end) * u128::from(audio.timescale)
        {
            labels.push(TransformLabel::MissingAudio {
                track_id: audio.id,
                reason: AudioGapReason::PartialTail,
            });
        }
        selected.push(Track {
            id: audio.id,
            timescale: audio.timescale,
            media_duration: audio.media_duration,
            handler: audio.handler,
            descriptions: audio.descriptions.clone(),
            edits: audio.edits.clone(),
            samples,
            sample_groups: Vec::new(),
        });
    }
    Ok((selected, labels))
}
/// Recompute audio selection and omission labels against freshly parsed source
/// metadata. A caller-supplied unavailable marker cannot authenticate omission.
pub fn verify_audio_selection(
    target: &SampleMap<LayoutSample>,
    plan: &TransformationPlan,
) -> Result<()> {
    let video = plan
        .selected
        .tracks
        .iter()
        .find(|t| t.handler == *b"vide")
        .ok_or_else(|| unresolved("missing planned video"))?;
    let source = target
        .tracks
        .iter()
        .find(|t| t.id == video.id && t.handler == *b"vide")
        .ok_or_else(|| unresolved("missing target video"))?;
    let first = video
        .samples
        .first()
        .ok_or_else(|| unresolved("empty planned video"))?;
    let last = video.samples.last().unwrap();
    let end = last
        .dts
        .checked_add(u64::from(last.duration))
        .ok_or(BmffError::ResourceLimit)?;
    let (tracks, labels) = audio_selection(target, source, first.dts, end, 4096)?;
    if !plan
        .selected
        .tracks
        .iter()
        .filter(|t| t.handler == *b"soun")
        .eq(tracks.iter())
        || !plan
            .labels
            .iter()
            .filter(|l| matches!(l, TransformLabel::MissingAudio { .. }))
            .eq(labels.iter())
    {
        return Err(unresolved(
            "audio selection or omission reason differs from target",
        ));
    }
    Ok(())
}

fn box_(kind: &[u8; 4], payload: Vec<u8>) -> Result<Vec<u8>> {
    let size = u32::try_from(
        payload
            .len()
            .checked_add(8)
            .ok_or(BmffError::ResourceLimit)?,
    )
    .map_err(|_| BmffError::ResourceLimit)?;
    let mut b = size.to_be_bytes().to_vec();
    b.extend(kind);
    b.extend(payload);
    Ok(b)
}
fn full(version: u8, flags: u32, body: Vec<u8>) -> Vec<u8> {
    let mut b = flags.to_be_bytes().to_vec();
    b[0] = version;
    b.extend(body);
    b
}
fn words(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|n| n.to_be_bytes()).collect()
}
fn table(kind: &[u8; 4], body: Vec<u8>) -> Result<Vec<u8>> {
    box_(kind, full(0, 0, body))
}
fn identity_matrix(bytes: &mut [u8], offset: usize) {
    for (i, n) in [0x10000u32, 0, 0, 0, 0x10000, 0, 0, 0, 0x40000000]
        .iter()
        .enumerate()
    {
        bytes[offset + i * 4..offset + i * 4 + 4].copy_from_slice(&n.to_be_bytes())
    }
}
fn duration_movie(duration: u64, scale: u32, movie: u32) -> Result<u64> {
    u64::try_from((u128::from(duration) * u128::from(movie)).div_ceil(u128::from(scale)))
        .map_err(|_| BmffError::ResourceLimit)
}
fn track_header(track: &Track, movie: u32, offset: u64) -> Result<Vec<u8>> {
    let n = u32::try_from(track.samples.len()).map_err(|_| BmffError::ResourceLimit)?;
    let desc = &track.descriptions[0];
    let mut stsd = words(&[1]);
    stsd.extend(&desc.sample_entry);
    let mut stbl = table(b"stsd", stsd)?;
    let mut stts = words(&[n]);
    for s in &track.samples {
        stts.extend(words(&[1, s.duration]));
    }
    stbl.extend(table(b"stts", stts)?);
    let mut ctts = words(&[n]);
    for s in &track.samples {
        let delta = i32::try_from(s.pts - i128::from(s.dts))
            .map_err(|_| BmffError::Unsupported("segment composition offset range"))?;
        ctts.extend(words(&[1, delta as u32]));
    }
    stbl.extend(box_(b"ctts", full(1, 0, ctts))?);
    stbl.extend(table(b"stsc", words(&[1, 1, n, 1]))?);
    let mut sizes = words(&[0, n]);
    for s in &track.samples {
        sizes.extend(
            u32::try_from(s.logical.len())
                .map_err(|_| BmffError::ResourceLimit)?
                .to_be_bytes(),
        );
    }
    stbl.extend(table(b"stsz", sizes)?);
    let mut co64 = words(&[1]);
    co64.extend(offset.to_be_bytes());
    stbl.extend(table(b"co64", co64)?);
    let sync = track
        .samples
        .iter()
        .enumerate()
        .filter(|(_, s)| s.is_sync)
        .map(|(i, _)| i as u32 + 1)
        .collect::<Vec<_>>();
    let mut stss = words(&[sync.len() as u32]);
    stss.extend(words(&sync));
    stbl.extend(table(b"stss", stss)?);
    let deps = track.samples.iter().map(|s| s.dependency).collect();
    stbl.extend(box_(b"sdtp", full(0, 0, deps))?);
    let mut references = words(&[u32::from(desc.data_reference)]);
    for _ in 0..desc.data_reference {
        references.extend(box_(b"url ", full(0, 1, Vec::new()))?);
    }
    let dinf = box_(b"dinf", table(b"dref", references)?)?;
    let mut minf = if track.handler == *b"vide" {
        box_(b"vmhd", full(0, 1, vec![0; 8]))?
    } else {
        box_(b"smhd", full(0, 0, vec![0; 4]))?
    };
    minf.extend(dinf);
    minf.extend(box_(b"stbl", stbl)?);
    let duration = track
        .samples
        .iter()
        .map(|s| u64::from(s.duration))
        .sum::<u64>();
    let mut mdhd = vec![0; 36];
    mdhd[0] = 1;
    mdhd[20..24].copy_from_slice(&track.timescale.to_be_bytes());
    mdhd[24..32].copy_from_slice(&duration.to_be_bytes());
    mdhd[32..34].copy_from_slice(&0x55c4u16.to_be_bytes());
    let mut hdlr = vec![0; 24];
    hdlr[8..12].copy_from_slice(&track.handler);
    hdlr.extend(b"FrameHarbor\0");
    let mut mdia = box_(b"mdhd", mdhd)?;
    mdia.extend(box_(b"hdlr", hdlr)?);
    mdia.extend(box_(b"minf", minf)?);
    let mut tkhd = vec![0; 96];
    tkhd[0] = 1;
    tkhd[3] = 7;
    tkhd[20..24].copy_from_slice(&track.id.to_be_bytes());
    tkhd[28..36].copy_from_slice(&duration_movie(duration, track.timescale, movie)?.to_be_bytes());
    identity_matrix(&mut tkhd, 52);
    if track.handler == *b"vide" {
        let entry_header = if desc.sample_entry.get(..4) == Some(&1u32.to_be_bytes()) {
            16
        } else {
            8
        };
        let dimensions = desc
            .sample_entry
            .get(entry_header + 24..entry_header + 28)
            .ok_or_else(|| unresolved("video sample entry dimensions"))?;
        let width = u32::from(u16::from_be_bytes([dimensions[0], dimensions[1]])) << 16;
        let height = u32::from(u16::from_be_bytes([dimensions[2], dimensions[3]])) << 16;
        tkhd[88..92].copy_from_slice(&width.to_be_bytes());
        tkhd[92..96].copy_from_slice(&height.to_be_bytes());
    }
    if track.handler == *b"soun" {
        tkhd[48..50].copy_from_slice(&0x100u16.to_be_bytes());
    }
    let mut trak = box_(b"tkhd", tkhd)?;
    trak.extend(box_(b"mdia", mdia)?);
    box_(b"trak", trak)
}
fn headers(tracks: &[Track], movie: u32, base: u64) -> Result<Vec<u8>> {
    let duration = tracks
        .iter()
        .map(|t| duration_movie(t.media_duration, t.timescale, movie))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .max()
        .unwrap_or(0);
    let mut mvhd = vec![0; 112];
    mvhd[0] = 1;
    mvhd[20..24].copy_from_slice(&movie.to_be_bytes());
    mvhd[24..32].copy_from_slice(&duration.to_be_bytes());
    mvhd[32..36].copy_from_slice(&0x10000u32.to_be_bytes());
    mvhd[36..38].copy_from_slice(&0x100u16.to_be_bytes());
    identity_matrix(&mut mvhd, 48);
    mvhd[108..112].copy_from_slice(
        &tracks
            .iter()
            .map(|t| t.id)
            .max()
            .unwrap_or(0)
            .saturating_add(1)
            .to_be_bytes(),
    );
    let mut moov = box_(b"mvhd", mvhd)?;
    let mut offset = base;
    for t in tracks {
        moov.extend(track_header(t, movie, offset)?);
        offset = offset
            .checked_add(t.samples.iter().map(|s| s.logical.len()).sum::<u64>())
            .ok_or(BmffError::ResourceLimit)?;
    }
    let mut header = box_(
        b"ftyp",
        [b"isom".as_slice(), &[0; 4], b"isom", b"iso6", b"mp41"].concat(),
    )?;
    header.extend(box_(b"moov", moov)?);
    Ok(header)
}
fn build(
    reader: &mut impl ReadAt,
    selected: SampleMap,
    owner: Option<LogicalOwner>,
    source_counts: Vec<(u32, u64)>,
    labels: Vec<TransformLabel>,
    limits: SegmentLimits,
) -> Result<TransformationPlan> {
    let mut output = Vec::new();
    for source in &selected.tracks {
        let first = source
            .samples
            .first()
            .ok_or_else(|| unresolved("empty selected track"))?;
        let description = source
            .descriptions
            .iter()
            .find(|d| d.index == first.description_index)
            .ok_or_else(|| unresolved("missing output configuration"))?;
        if source
            .samples
            .iter()
            .any(|s| s.description_index != first.description_index)
        {
            return Err(BmffError::Unsupported("segment description transition"));
        }
        let mut desc = description.clone();
        desc.index = 1;
        let mut samples = source.samples.clone();
        let mut dts = 0u64;
        for (i, s) in samples.iter_mut().enumerate() {
            if s.dts != first.dts.checked_add(dts).ok_or(BmffError::ResourceLimit)? {
                return Err(unresolved("selected decode-time gap"));
            }
            s.ordinal = i as u64;
            s.description_index = 1;
            s.dts = dts;
            s.pts -= i128::from(first.dts);
            dts = dts
                .checked_add(u64::from(s.duration))
                .ok_or(BmffError::ResourceLimit)?;
        }
        output.push(Track {
            id: source.id,
            timescale: source.timescale,
            media_duration: dts,
            handler: source.handler,
            descriptions: vec![desc],
            edits: Vec::new(),
            samples,
            sample_groups: Vec::new(),
        });
    }
    let preliminary = headers(&output, selected.movie_timescale, 0)?;
    let payload = selected
        .tracks
        .iter()
        .flat_map(|t| &t.samples)
        .try_fold(0u64, |n, s| {
            n.checked_add(s.logical.len())
                .ok_or(BmffError::ResourceLimit)
        })?;
    let mdat_size = u32::try_from(payload.checked_add(8).ok_or(BmffError::ResourceLimit)?)
        .map_err(|_| BmffError::ResourceLimit)?;
    let base = preliminary.len() as u64 + 8;
    let mut header = headers(&output, selected.movie_timescale, base)?;
    header.extend(mdat_size.to_be_bytes());
    header.extend(b"mdat");
    let size = base
        .checked_add(payload)
        .filter(|n| *n <= limits.output_bytes)
        .ok_or(BmffError::ResourceLimit)?;
    if header.len() as u64 != base || header.len() as u64 > limits.memory_bytes / 4 {
        return Err(BmffError::ResourceLimit);
    }
    let mut copies = Vec::new();
    let mut offset = base;
    for track in &selected.tracks {
        for source in &track.samples {
            let range = ByteRange::from_len(offset, source.logical.len())
                .map_err(|_| BmffError::ResourceLimit)?;
            copies.push(CopyRange {
                source: source.clone(),
                output: range,
            });
            offset = range.end();
        }
    }
    let parse = ParseLimits {
        samples: limits.samples,
        table_entries: limits.samples.max(64),
        memory_bytes: limits.memory_bytes.min(1024 * 1024 * 1024),
        sample_bytes: limits.sample_bytes,
        payload_hash_bytes: limits.read_bytes,
        ..ParseLimits::default()
    };
    let mut virtual_file = VirtualOutput {
        reader,
        header: &header,
        copies: &copies,
        size,
    };
    let expected = bmff::inspect(&mut virtual_file, parse)?;
    for (actual, copy) in expected.tracks.iter().flat_map(|t| &t.samples).zip(&copies) {
        if actual.sha256 != copy.source.sha256 {
            return Err(BmffError::Session(SessionError::SourceChanged));
        }
    }
    Ok(TransformationPlan {
        owner,
        source_counts,
        selected,
        expected,
        labels,
        header,
        copies,
    })
}
struct VirtualOutput<'a, R: ReadAt> {
    reader: &'a mut R,
    header: &'a [u8],
    copies: &'a [CopyRange],
    size: u64,
}
impl<R: ReadAt> ReadAt for VirtualOutput<'_, R> {
    fn len(&self) -> u64 {
        self.size
    }
    fn reserve(&mut self, memory: u64) -> Result<()> {
        self.reader.reserve(memory)
    }
    fn read_exact(&mut self, offset: u64, bytes: &mut [u8]) -> Result<()> {
        let mut p = offset;
        let end = offset
            .checked_add(bytes.len() as u64)
            .filter(|n| *n <= self.size)
            .ok_or(BmffError::ResourceLimit)?;
        while p < end {
            let dest = (p - offset) as usize;
            if p < self.header.len() as u64 {
                let n = (end - p).min(self.header.len() as u64 - p) as usize;
                bytes[dest..dest + n].copy_from_slice(&self.header[p as usize..p as usize + n]);
                p += n as u64;
            } else {
                let copy = self
                    .copies
                    .get(self.copies.partition_point(|c| c.output.end() <= p))
                    .filter(|c| c.output.start() <= p)
                    .ok_or_else(|| unresolved("missing output payload mapping"))?;
                let n = (end - p).min(copy.output.end() - p).min(65536) as usize;
                self.reader.read_exact(
                    copy.source.logical.start() + p - copy.output.start(),
                    &mut bytes[dest..dest + n],
                )?;
                p += n as u64;
            }
        }
        Ok(())
    }
    fn source_spans(&self, range: ByteRange) -> Result<Vec<bmff::SourceSpan>> {
        let copy = self
            .copies
            .get(
                self.copies
                    .partition_point(|c| c.output.start() < range.start()),
            )
            .filter(|c| c.output == range)
            .ok_or_else(|| unresolved("output sample mapping"))?;
        Ok(copy.source.source_spans.clone())
    }
}
/// Copy only known selected encoded payloads. The caller owns the staging file,
/// quota, cleanup and independent validation; partial writes produce no permit.
pub fn write(
    reader: &mut impl ReadAt,
    plan: &TransformationPlan,
    writer: &mut impl Write,
) -> Result<()> {
    let result = (|| {
        if reader.len() != plan.selected.file_size || reader.logical_owner() != plan.owner.as_ref()
        {
            return Err(BmffError::Session(SessionError::SourceChanged));
        }
        reader.reserve(plan.memory_bound())?;
        writer.write_all(&plan.header)?;
        let mut buffer = vec![0; 65536];
        for copy in &plan.copies {
            if reader.source_spans(copy.source.logical)? != copy.source.source_spans {
                return Err(unresolved("selected physical provenance changed"));
            }
            let mut hash = Sha256::new();
            let mut p = copy.source.logical.start();
            while p < copy.source.logical.end() {
                let n = (copy.source.logical.end() - p).min(buffer.len() as u64) as usize;
                reader.read_exact(p, &mut buffer[..n])?;
                hash.update(&buffer[..n]);
                writer.write_all(&buffer[..n])?;
                p += n as u64;
            }
            if hash.finalize().as_slice() != copy.source.sha256 {
                return Err(BmffError::Session(SessionError::SourceChanged));
            }
        }
        Ok(())
    })();
    reader.finish()?;
    result
}
