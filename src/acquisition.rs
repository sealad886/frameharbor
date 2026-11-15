//! Read-only acquisition masks. Map coverage never authenticates a pairing.
use crate::contracts::{
    AcquisitionStatus, Budget, ByteRange, CoordinateBinding, Resource, ResourceCaps, TerminalReason,
};
use sha2::{Digest, Sha256};
use std::fs::{File, Metadata};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

#[derive(Debug)]
pub enum AcquisitionError {
    Malformed {
        line: usize,
        reason: &'static str,
    },
    Limit(&'static str),
    InvalidRange,
    BindingUnverified,
    /// Unavailable physical bytes in image-relative coordinates.
    Unavailable(ByteRange),
    SourceChanged,
    Stopped(TerminalReason),
    Io(io::Error),
}
impl std::fmt::Display for AcquisitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for AcquisitionError {}
impl From<io::Error> for AcquisitionError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MapLimits {
    pub bytes: u64,
    pub line_bytes: usize,
    pub intervals: usize,
}
impl Default for MapLimits {
    fn default() -> Self {
        Self {
            bytes: 64 * 1024 * 1024,
            line_bytes: 4096,
            intervals: 1_000_000,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapInterval {
    pub input: ByteRange,
    pub status: AcquisitionStatus,
}
#[derive(Clone, Debug)]
pub struct RescueMap {
    intervals: Vec<MapInterval>,
    pub digest: [u8; 32],
}
fn integer(s: &str) -> Option<u64> {
    if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(h, 16).ok()
    } else {
        s.parse().ok()
    }
}
impl RescueMap {
    /// Accepts GNU map status line followed by sorted nonoverlapping block rows.
    /// Gaps remain unknown. The parser never allocates in proportion to an offset.
    pub fn parse(reader: impl Read, limits: MapLimits) -> Result<Self, AcquisitionError> {
        if limits.bytes == 0 || limits.line_bytes == 0 || limits.intervals == 0 {
            return Err(AcquisitionError::Limit("map limits"));
        }
        let mut reader = BufReader::new(reader.take(limits.bytes.saturating_add(1)));
        let mut digest = Sha256::new();
        let mut total = 0u64;
        let mut line = Vec::new();
        let mut line_no = 0;
        let mut header = false;
        let mut intervals: Vec<MapInterval> = Vec::new();
        loop {
            line.clear();
            // take bounds a hostile single line before allocation, not after read_until.
            let n = reader
                .by_ref()
                .take((limits.line_bytes as u64).saturating_add(1))
                .read_until(b'\n', &mut line)?;
            if n == 0 {
                break;
            }
            line_no += 1;
            total = total
                .checked_add(n as u64)
                .ok_or(AcquisitionError::Limit("map bytes"))?;
            if total > limits.bytes || n > limits.line_bytes {
                return Err(AcquisitionError::Limit("map bytes or line"));
            }
            digest.update(&line);
            let s = std::str::from_utf8(&line).map_err(|_| AcquisitionError::Malformed {
                line: line_no,
                reason: "non UTF-8 map",
            })?;
            let s = s.split('#').next().unwrap_or("").trim();
            if s.is_empty() {
                continue;
            }
            let fields: Vec<_> = s.split_whitespace().collect();
            let bad = |reason| AcquisitionError::Malformed {
                line: line_no,
                reason,
            };
            if !header {
                if !(fields.len() == 2 || fields.len() == 3)
                    || integer(fields[0]).is_none()
                    || fields[1].len() != 1
                    || !"?*/-+FG".contains(fields[1])
                    || (fields.len() == 3 && integer(fields[2]).filter(|n| *n > 0).is_none())
                {
                    return Err(bad("invalid status line"));
                }
                header = true;
                continue;
            }
            if fields.len() != 3 {
                return Err(bad("expected position size status"));
            }
            let start = integer(fields[0]).ok_or_else(|| bad("invalid position"))?;
            let len = integer(fields[1])
                .filter(|v| *v > 0)
                .ok_or_else(|| bad("invalid size"))?;
            let input = ByteRange::from_len(start, len).map_err(|_| bad("range overflow"))?;
            let status = match fields[2] {
                "+" => AcquisitionStatus::Finished,
                "?" => AcquisitionStatus::NonTried,
                "*" => AcquisitionStatus::NonTrimmed,
                "/" => AcquisitionStatus::NonScraped,
                "-" => AcquisitionStatus::BadSector,
                _ => return Err(bad("invalid block status")),
            };
            if intervals
                .last()
                .is_some_and(|i| i.input.end() > input.start())
            {
                return Err(bad("overlapping or unsorted map intervals"));
            }
            if intervals.len() >= limits.intervals {
                return Err(AcquisitionError::Limit("map intervals"));
            }
            intervals.push(MapInterval { input, status });
        }
        if !header || intervals.is_empty() {
            return Err(AcquisitionError::Malformed {
                line: line_no,
                reason: "missing status or intervals",
            });
        }
        Ok(Self {
            intervals,
            digest: digest.finalize().into(),
        })
    }
    pub fn intervals(&self) -> &[MapInterval] {
        &self.intervals
    }
    pub fn quality(&self, input: ByteRange) -> Vec<MapInterval> {
        self.quality_iter(input).collect()
    }
    /// Streaming quality partition: classification needs no interval-sized copy.
    pub fn quality_iter(&self, input: ByteRange) -> QualityIntervals<'_> {
        QualityIntervals {
            intervals: &self.intervals,
            index: self
                .intervals
                .partition_point(|i| i.input.end() <= input.start()),
            cursor: input.start(),
            end: input.end(),
        }
    }
}

pub struct QualityIntervals<'a> {
    intervals: &'a [MapInterval],
    index: usize,
    cursor: u64,
    end: u64,
}
impl Iterator for QualityIntervals<'_> {
    type Item = MapInterval;
    fn next(&mut self) -> Option<Self::Item> {
        if self.cursor >= self.end {
            return None;
        }
        let (end, status) = match self.intervals.get(self.index) {
            None => (self.end, AcquisitionStatus::Unknown),
            Some(interval) if interval.input.start() > self.cursor => (
                interval.input.start().min(self.end),
                AcquisitionStatus::Unknown,
            ),
            Some(interval) => {
                self.index += 1;
                (interval.input.end().min(self.end), interval.status)
            }
        };
        let input = ByteRange::new(self.cursor, end)
            .expect("quality intervals advance within checked nonempty ranges");
        self.cursor = end;
        Some(MapInterval { input, status })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Identity {
    len: u64,
    modified: Option<SystemTime>,
    device: u64,
    inode: u64,
    change_sec: i64,
    change_nsec: i64,
}
pub(crate) fn identity(metadata: &Metadata) -> Identity {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Identity {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            device: metadata.dev(),
            inode: metadata.ino(),
            change_sec: metadata.ctime(),
            change_nsec: metadata.ctime_nsec(),
        }
    }
    #[cfg(not(unix))]
    {
        Identity {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            device: 0,
            inode: 0,
            change_sec: 0,
            change_nsec: 0,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsumedRange {
    pub image: ByteRange,
    pub sha256: [u8; 32],
}
/// Immutable snapshot of source identity and consumed dependencies. This ID
/// describes an observed generation, never a full-image content digest.
#[derive(Clone, Debug)]
pub struct SourceManifest {
    generation_id: [u8; 32],
    image_size: u64,
    map_digest: [u8; 32],
    binding: CoordinateBinding,
    consumed: Vec<ConsumedRange>,
    archive_refs: Vec<[u8; 32]>,
}
impl SourceManifest {
    pub fn generation_id(&self) -> [u8; 32] {
        self.generation_id
    }
    pub fn image_size(&self) -> u64 {
        self.image_size
    }
    pub fn map_digest(&self) -> [u8; 32] {
        self.map_digest
    }
    pub fn binding(&self) -> &CoordinateBinding {
        &self.binding
    }
    pub fn consumed(&self) -> &[ConsumedRange] {
        &self.consumed
    }
    pub fn archive_refs(&self) -> &[[u8; 32]] {
        &self.archive_refs
    }
    pub(crate) fn with_archives(mut self, refs: &[[u8; 32]]) -> Self {
        self.archive_refs = refs.to_vec();
        self
    }
    pub fn is_full_image_hash(&self) -> bool {
        false
    }
}
#[derive(Debug)]
pub struct ReadReceipt {
    pub image: ByteRange,
    pub sha256: [u8; 32],
    pub bytes: Vec<u8>,
}
#[derive(Debug)]
pub enum MaskedPiece {
    Available(ReadReceipt),
    Unavailable {
        image: ByteRange,
        input: ByteRange,
        status: AcquisitionStatus,
    },
}
#[derive(Clone, Copy, Debug)]
pub struct ReadLimits {
    pub per_read: u64,
    pub total: u64,
    pub receipts: usize,
}
impl Default for ReadLimits {
    fn default() -> Self {
        Self {
            per_read: 1024 * 1024,
            total: 8 * 1024 * 1024 * 1024,
            receipts: 8192,
        }
    }
}
/// File handles are opened with File::open only. Metadata checks detect common
/// changes; they do not assert immutable snapshots or authenticate unobserved bytes.
pub struct RangeSource {
    image: File,
    map_file: File,
    image_path: PathBuf,
    map_path: PathBuf,
    image_identity: Identity,
    map_identity: Identity,
    map: RescueMap,
    binding: CoordinateBinding,
    limits: ReadLimits,
    bytes_read: u64,
    verification_bytes: u64,
    consumed: Vec<ConsumedRange>,
    budget: Budget,
    run_started: Instant,
    resumed_elapsed: Duration,
}
impl RangeSource {
    pub fn open(
        image: &Path,
        map: &Path,
        binding: CoordinateBinding,
        limits: ReadLimits,
    ) -> Result<Self, AcquisitionError> {
        let budget = Budget::new(
            Duration::from_secs(86400),
            Duration::from_secs(60),
            ResourceCaps {
                read_bytes: limits.total,
                memory_bytes: limits.per_read,
                index_bytes: 0,
                output_bytes: 0,
                visited_postings: 0,
                transitions: 0,
                decoder_seconds: 0,
            },
        )
        .map_err(|_| AcquisitionError::Limit("budget"))?;
        Self::open_with_budget(image, map, binding, limits, budget)
    }
    /// Transfers the case budget into this reader. Resume supplies its prior
    /// cumulative budget; opening a file never replenishes that allowance.
    pub fn open_with_budget(
        image: &Path,
        map: &Path,
        binding: CoordinateBinding,
        limits: ReadLimits,
        budget: Budget,
    ) -> Result<Self, AcquisitionError> {
        Self::open_with_map_limits(image, map, binding, limits, budget, MapLimits::default())
    }
    /// A coordinator may tighten parser limits before any map bytes are read.
    /// In particular, pinning `bytes` to preflight size rejects concurrent growth.
    pub fn open_with_map_limits(
        image: &Path,
        map: &Path,
        binding: CoordinateBinding,
        limits: ReadLimits,
        mut budget: Budget,
        map_limits: MapLimits,
    ) -> Result<Self, AcquisitionError> {
        let hard = MapLimits::default();
        if map_limits.bytes > hard.bytes
            || map_limits.line_bytes > hard.line_bytes
            || map_limits.intervals > hard.intervals
        {
            return Err(AcquisitionError::Limit("map implementation envelope"));
        }
        let run_started = Instant::now();
        let resumed_elapsed = budget.elapsed();
        budget
            .authorize_at(resumed_elapsed, Resource::ReadBytes, 0)
            .map_err(AcquisitionError::Stopped)?;
        let image_file = File::open(image)?;
        let mut map_file = File::open(map)?;
        if !image_file.metadata()?.is_file() || !map_file.metadata()?.is_file() {
            return Err(AcquisitionError::InvalidRange);
        }
        let image_identity = identity(&image_file.metadata()?);
        let map_identity = identity(&map_file.metadata()?);
        if binding.image_domain().end() > image_identity.len {
            return Err(AcquisitionError::InvalidRange);
        }
        let parsed = RescueMap::parse(&mut map_file, map_limits)?;
        let source = Self {
            image: image_file,
            map_file,
            image_path: image.into(),
            map_path: map.into(),
            image_identity,
            map_identity,
            map: parsed,
            binding,
            limits,
            bytes_read: 0,
            verification_bytes: 0,
            consumed: Vec::new(),
            budget,
            run_started,
            resumed_elapsed,
        };
        source.check_generation()?;
        Ok(source)
    }
    pub fn map(&self) -> &RescueMap {
        &self.map
    }
    pub fn binding(&self) -> &CoordinateBinding {
        &self.binding
    }
    pub fn bytes_read(&self) -> u64 {
        self.bytes_read
    }
    /// Metadata intake is reported separately from image payload and resume
    /// revalidation reads. Its wall time is included in the case allowance.
    pub fn map_bytes_read(&self) -> u64 {
        self.map_identity.len
    }
    pub fn verification_bytes(&self) -> u64 {
        self.verification_bytes
    }
    pub fn consumed(&self) -> &[ConsumedRange] {
        &self.consumed
    }
    pub fn manifest(&self) -> Result<SourceManifest, AcquisitionError> {
        self.check_generation()?;
        let mut hash = Sha256::new();
        hash.update(b"frameharbor-observed-source-generation-v1\0");
        for n in [
            self.image_identity.len,
            self.image_identity.device,
            self.image_identity.inode,
            self.binding.input_domain().start(),
            self.binding.input_domain().end(),
            self.binding.image_domain().start(),
            self.binding.image_domain().end(),
        ] {
            hash.update(n.to_le_bytes());
        }
        hash.update(self.image_identity.change_sec.to_le_bytes());
        hash.update(self.image_identity.change_nsec.to_le_bytes());
        match self.image_identity.modified {
            Some(time) => {
                let (before_epoch, duration) = match time.duration_since(SystemTime::UNIX_EPOCH) {
                    Ok(duration) => (false, duration),
                    Err(error) => (true, error.duration()),
                };
                hash.update([1, u8::from(before_epoch)]);
                hash.update(duration.as_secs().to_le_bytes());
                hash.update(duration.subsec_nanos().to_le_bytes());
            }
            None => hash.update([0]),
        }
        hash.update(self.map.digest);
        hash.update([match self.binding.evidence() {
            crate::contracts::BindingEvidence::Unverified => 0,
            crate::contracts::BindingEvidence::OperatorAttested => 1,
            crate::contracts::BindingEvidence::TrustedManifest => 2,
        }]);
        Ok(SourceManifest {
            generation_id: hash.finalize().into(),
            image_size: self.image_identity.len,
            map_digest: self.map.digest,
            binding: self.binding.clone(),
            consumed: self.consumed.clone(),
            archive_refs: Vec::new(),
        })
    }
    pub fn budget(&self) -> &Budget {
        &self.budget
    }
    pub fn into_budget(mut self) -> Budget {
        let elapsed = self
            .resumed_elapsed
            .saturating_add(self.run_started.elapsed());
        let _ = self.budget.authorize_at(elapsed, Resource::ReadBytes, 0);
        self.budget
    }
    fn authorize(&mut self, bytes: u64) -> Result<(), AcquisitionError> {
        self.budget
            .authorize_at(
                self.resumed_elapsed
                    .saturating_add(self.run_started.elapsed()),
                Resource::ReadBytes,
                bytes,
            )
            .map_err(AcquisitionError::Stopped)
    }
    pub(crate) fn can_retain_read(&self, range: ByteRange) -> bool {
        self.consumed.len() < self.limits.receipts
            || self.consumed.iter().any(|item| item.image == range)
    }
    /// Only the session's sealed publication-proof path may retire dependencies.
    /// Source read counters and the cumulative budget are deliberately retained.
    pub(crate) fn retire_consumed_after(
        &mut self,
        baseline: usize,
    ) -> Result<(), AcquisitionError> {
        if baseline > self.consumed.len() {
            return Err(AcquisitionError::Limit("invalid dependency baseline"));
        }
        self.consumed.truncate(baseline);
        self.consumed.shrink_to_fit();
        Ok(())
    }
    pub fn check_generation(&self) -> Result<(), AcquisitionError> {
        if identity(&self.image.metadata()?) != self.image_identity
            || identity(&std::fs::metadata(&self.image_path)?) != self.image_identity
            || identity(&self.map_file.metadata()?) != self.map_identity
            || identity(&std::fs::metadata(&self.map_path)?) != self.map_identity
        {
            return Err(AcquisitionError::SourceChanged);
        }
        Ok(())
    }
    pub fn read(&mut self, image: ByteRange) -> Result<ReadReceipt, AcquisitionError> {
        self.check_generation()?;
        self.binding
            .require_supported()
            .map_err(|_| AcquisitionError::BindingUnverified)?;
        let input = self
            .binding
            .image_to_input(image)
            .map_err(|_| AcquisitionError::InvalidRange)?;
        if let Some(missing) = self.map.quality_iter(input).find(|i| !i.status.available()) {
            return Err(AcquisitionError::Unavailable(
                self.binding
                    .input_to_image(missing.input)
                    .map_err(|_| AcquisitionError::InvalidRange)?,
            ));
        }
        let previous = self
            .consumed
            .iter()
            .find(|c| c.image == image)
            .map(|c| c.sha256);
        if image.len() > self.limits.per_read
            || self
                .bytes_read
                .checked_add(image.len())
                .is_none_or(|n| n > self.limits.total)
            || (previous.is_none() && self.consumed.len() >= self.limits.receipts)
        {
            return Err(AcquisitionError::Limit("source reads"));
        }
        if image.len() > self.budget.caps().memory_bytes {
            return Err(AcquisitionError::Limit("read buffer memory"));
        }
        let size =
            usize::try_from(image.len()).map_err(|_| AcquisitionError::Limit("allocation"))?;
        self.authorize(image.len())?;
        // Charge attempted bytes even when I/O is short or fails.
        self.bytes_read += image.len();
        let mut bytes = vec![0; size];
        self.image.seek(SeekFrom::Start(image.start()))?;
        self.image.read_exact(&mut bytes)?;
        self.check_generation()?;
        let sha256 = Sha256::digest(&bytes).into();
        if previous.is_some_and(|old| old != sha256) {
            return Err(AcquisitionError::SourceChanged);
        }
        if previous.is_none() {
            self.consumed.push(ConsumedRange { image, sha256 });
        }
        Ok(ReadReceipt {
            image,
            sha256,
            bytes,
        })
    }
    /// Returns unavailable spans as metadata only, never placeholder/stale bytes.
    /// Physical zeros from finished acquisition remain physical bytes, not
    /// filesystem-supported synthesized logical zeros.
    pub fn read_masked(&mut self, image: ByteRange) -> Result<Vec<MaskedPiece>, AcquisitionError> {
        self.check_generation()?;
        self.binding
            .require_supported()
            .map_err(|_| AcquisitionError::BindingUnverified)?;
        if image.len() > self.limits.per_read || image.len() > self.budget.caps().memory_bytes {
            return Err(AcquisitionError::Limit("masked read size"));
        }
        let input = self
            .binding
            .image_to_input(image)
            .map_err(|_| AcquisitionError::InvalidRange)?;
        let quality: Vec<_> = self
            .map
            .quality_iter(input)
            .take(self.limits.receipts.saturating_add(1))
            .collect();
        let new_receipts =
            quality
                .iter()
                .filter(|p| p.status.available())
                .try_fold(0usize, |n, p| {
                    let image = self
                        .binding
                        .input_to_image(p.input)
                        .map_err(|_| AcquisitionError::InvalidRange)?;
                    Ok::<_, AcquisitionError>(
                        n + usize::from(!self.consumed.iter().any(|c| c.image == image)),
                    )
                })?;
        if quality.len() > self.limits.receipts
            || self.consumed.len().saturating_add(new_receipts) > self.limits.receipts
        {
            return Err(AcquisitionError::Limit("masked read pieces"));
        }
        let available_bytes: u64 = quality
            .iter()
            .filter(|p| p.status.available())
            .map(|p| p.input.len())
            .sum();
        if self
            .bytes_read
            .checked_add(available_bytes)
            .is_none_or(|n| n > self.limits.total)
        {
            return Err(AcquisitionError::Limit("source reads"));
        }
        let mut result = Vec::new();
        for piece in quality {
            let image = self
                .binding
                .input_to_image(piece.input)
                .map_err(|_| AcquisitionError::InvalidRange)?;
            if piece.status.available() {
                result.push(MaskedPiece::Available(self.read(image)?));
            } else {
                result.push(MaskedPiece::Unavailable {
                    image,
                    input: piece.input,
                    status: piece.status,
                });
            }
        }
        Ok(result)
    }
    /// Revalidates every consumed dependency, under the same total read authority.
    /// A no-op resume cannot claim strong source adoption from mtime alone.
    pub fn revalidate(&mut self) -> Result<(), AcquisitionError> {
        self.check_generation()?;
        let mut buffer = vec![
            0;
            usize::try_from(
                self.limits
                    .per_read
                    .min(self.budget.caps().memory_bytes)
                    .min(1024 * 1024)
            )
            .map_err(|_| AcquisitionError::Limit("allocation"))?
        ];
        if buffer.is_empty() {
            return Err(AcquisitionError::Limit("verification buffer"));
        }
        for index in 0..self.consumed.len() {
            let dependency = self.consumed[index].clone();
            if self
                .bytes_read
                .checked_add(dependency.image.len())
                .is_none_or(|n| n > self.limits.total)
            {
                return Err(AcquisitionError::Limit("verification reads"));
            }
            self.authorize(dependency.image.len())?;
            self.bytes_read += dependency.image.len();
            self.verification_bytes += dependency.image.len();
            self.image.seek(SeekFrom::Start(dependency.image.start()))?;
            let mut remain = dependency.image.len();
            let mut hash = Sha256::new();
            while remain > 0 {
                let n = (remain as usize).min(buffer.len());
                self.image.read_exact(&mut buffer[..n])?;
                hash.update(&buffer[..n]);
                remain -= n as u64;
            }
            let actual: [u8; 32] = hash.finalize().into();
            if actual != dependency.sha256 {
                return Err(AcquisitionError::SourceChanged);
            }
        }
        self.check_generation()
    }
}
#[derive(Debug)]
pub struct PilotReceipt {
    pub windows: usize,
    pub bytes: u64,
    pub eligible_bytes: u64,
    pub image_domain_bytes: u64,
    pub elapsed: Duration,
    pub capped: bool,
    pub deadline_overrun: bool,
}
/// Deterministic distributed windows over declared available intervals. No media
/// interpretation, whole-image identity, ETA, or device benchmark is implied.
pub fn pilot(
    source: &mut RangeSource,
    max_bytes: u64,
    max_time: Duration,
    windows: usize,
) -> Result<PilotReceipt, AcquisitionError> {
    if max_bytes > 8 * 1024 * 1024 * 1024
        || max_time > Duration::from_secs(600)
        || windows == 0
        || windows > 8192
    {
        return Err(AcquisitionError::Limit("pilot"));
    }
    let started = Instant::now();
    let before = source.bytes_read();
    let mut completed = 0;
    let domain = source.binding.image_domain();
    let eligible: Vec<ByteRange> = source
        .map
        .intervals()
        .iter()
        .filter(|i| i.status.available())
        .filter_map(|i| i.input.intersection(source.binding.input_domain()))
        .map(|r| {
            source
                .binding
                .input_to_image(r)
                .map_err(|_| AcquisitionError::InvalidRange)
        })
        .collect::<Result<_, _>>()?;
    let eligible_bytes: u64 = eligible.iter().map(|r| r.len()).sum();
    let count = windows.min(usize::try_from(eligible_bytes).unwrap_or(usize::MAX));
    let mut physical_index = 0;
    let mut cumulative_start = 0;
    for i in 0..count {
        if started.elapsed() >= max_time || source.bytes_read() - before >= max_bytes {
            break;
        }
        let target = ((eligible_bytes as u128 * i as u128) / count as u128) as u64;
        let next_target = ((eligible_bytes as u128 * (i + 1) as u128) / count as u128) as u64;
        while cumulative_start + eligible[physical_index].len() <= target {
            cumulative_start += eligible[physical_index].len();
            physical_index += 1;
        }
        let available = eligible[physical_index];
        let offset = available.start() + target - cumulative_start;
        let len = source
            .limits
            .per_read
            .min(available.end() - offset)
            .min(next_target - target)
            .min(max_bytes - (source.bytes_read() - before));
        if len == 0 {
            break;
        }
        match source
            .read(ByteRange::from_len(offset, len).map_err(|_| AcquisitionError::InvalidRange)?)
        {
            Ok(_) => completed += 1,
            Err(AcquisitionError::Unavailable(_)) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(PilotReceipt {
        windows: completed,
        bytes: source.bytes_read() - before,
        eligible_bytes,
        image_domain_bytes: domain.len(),
        elapsed: started.elapsed(),
        capped: completed < windows,
        deadline_overrun: started.elapsed() > max_time,
    })
}
