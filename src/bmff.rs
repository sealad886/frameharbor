//! Bounded ordinary-MP4 sample tables. Container evidence establishes the listed
//! snapshot layout; hashes do not authenticate historical pre-damage ownership.
use crate::{
    acquisition::AcquisitionError,
    contracts::ByteRange,
    filesystem::{FilesystemError, LogicalFileView},
    session::{CaseSession, SessionError},
};
use sha2::{Digest, Sha256};
use std::{
    fmt,
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::Path,
    time::Instant,
};

#[derive(Debug)]
pub enum BmffError {
    Io(io::Error),
    Filesystem(FilesystemError),
    Session(SessionError),
    Malformed(&'static str),
    Unsupported(&'static str),
    Unresolved(&'static str),
    ResourceLimit,
}
impl fmt::Display for BmffError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for BmffError {}
impl From<io::Error> for BmffError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<FilesystemError> for BmffError {
    fn from(e: FilesystemError) -> Self {
        Self::Filesystem(e)
    }
}
impl From<SessionError> for BmffError {
    fn from(e: SessionError) -> Self {
        Self::Session(e)
    }
}
pub type Result<T> = std::result::Result<T, BmffError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSpan {
    pub logical: ByteRange,
    pub image: ByteRange,
}
pub trait ReadAt {
    fn len(&self) -> u64;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn read_exact(&mut self, offset: u64, bytes: &mut [u8]) -> Result<()>;
    fn logical_owner(&self) -> Option<&crate::contracts::LogicalOwner> {
        None
    }
    fn source_spans(&self, _range: ByteRange) -> Result<Vec<SourceSpan>> {
        Ok(Vec::new())
    }
    fn reserve(&mut self, _memory: u64) -> Result<()> {
        Ok(())
    }
    /// Settle elapsed work and source identity before returning a result.
    fn finish(&mut self) -> Result<()> {
        Ok(())
    }
}
pub struct SliceReader<'a> {
    bytes: &'a [u8],
}
impl<'a> SliceReader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }
}
impl ReadAt for SliceReader<'_> {
    fn len(&self) -> u64 {
        self.bytes.len() as u64
    }
    fn read_exact(&mut self, offset: u64, bytes: &mut [u8]) -> Result<()> {
        let start = usize::try_from(offset).map_err(|_| BmffError::ResourceLimit)?;
        let end = start
            .checked_add(bytes.len())
            .ok_or(BmffError::ResourceLimit)?;
        bytes.copy_from_slice(
            self.bytes
                .get(start..end)
                .ok_or(BmffError::Malformed("read outside view"))?,
        );
        Ok(())
    }
}
/// Read-only staged-artifact reparse. This reader is not a raw-image adapter.
pub struct FileReader {
    file: File,
    size: u64,
}
impl FileReader {
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(BmffError::Unsupported("non-regular staged artifact"));
        }
        Ok(Self {
            file,
            size: metadata.len(),
        })
    }
}
impl ReadAt for FileReader {
    fn len(&self) -> u64 {
        self.size
    }
    fn read_exact(&mut self, offset: u64, bytes: &mut [u8]) -> Result<()> {
        if offset
            .checked_add(bytes.len() as u64)
            .is_none_or(|n| n > self.size)
        {
            return Err(BmffError::Malformed("read outside staged artifact"));
        }
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.read_exact(bytes)?;
        Ok(())
    }
}
pub struct LogicalReader<'a> {
    session: &'a mut CaseSession,
    view: &'a LogicalFileView,
    reserved: u64,
    cache_start: u64,
    cache: Vec<u8>,
    deadline: Option<Instant>,
    stats: LogicalReadStats,
    cpu_permit: Option<crate::session::CpuPermit>,
}
const LOGICAL_WINDOW: u64 = 64 * 1024;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LogicalReadStats {
    pub logical_requests: u64,
    pub cache_hits: u64,
    pub cache_fills: u64,
    pub precise_reads: u64,
    pub source_bytes: u64,
}
impl<'a> LogicalReader<'a> {
    pub fn new(session: &'a mut CaseSession, view: &'a LogicalFileView) -> Self {
        Self {
            session,
            view,
            reserved: 0,
            cache_start: 0,
            cache: Vec::new(),
            deadline: None,
            stats: LogicalReadStats::default(),
            cpu_permit: None,
        }
    }
    /// Reuse a bounded serial parser-memory grant. This changes neither source
    /// read charging nor acquisition masks, and consumes one batch operation.
    pub fn in_cpu_batch(
        session: &'a mut CaseSession,
        view: &'a LogicalFileView,
        batch: &mut crate::session::CpuBatch,
    ) -> Result<Self> {
        let permit = session.cpu_batch_permit(batch)?;
        let mut reader = Self::new(session, view);
        reader.cpu_permit = Some(permit);
        Ok(reader)
    }
    pub fn stats(&self) -> LogicalReadStats {
        self.stats
    }
    fn check_live(&mut self) -> Result<()> {
        if let Some(permit) = &self.cpu_permit {
            self.session.check_cpu_permit(permit, self.reserved)?;
        } else {
            self.session.check_source()?;
        }
        if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err(BmffError::ResourceLimit);
        }
        Ok(())
    }
    fn window_available(&self, range: ByteRange) -> Result<bool> {
        let spans = match self.source_spans(range) {
            Ok(spans) => spans,
            Err(BmffError::Unresolved(_)) => return Ok(false),
            Err(e) => return Err(e),
        };
        for span in spans {
            if !self.session.range_available(span.image)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}
impl ReadAt for LogicalReader<'_> {
    fn logical_owner(&self) -> Option<&crate::contracts::LogicalOwner> {
        Some(&self.view.owner)
    }
    fn len(&self) -> u64 {
        self.view.size
    }
    fn reserve(&mut self, memory: u64) -> Result<()> {
        self.reserved = memory
            .checked_add(3 * 1024 * 1024 + LOGICAL_WINDOW)
            .ok_or(BmffError::ResourceLimit)?;
        if self.view.owner.source != self.session.source_generation() {
            return Err(BmffError::Filesystem(FilesystemError::SourceMismatch));
        }
        let observed = Instant::now();
        let remaining = if let Some(permit) = &self.cpu_permit {
            self.session.check_cpu_permit(permit, self.reserved)?
        } else {
            self.session.admit_external("bmff", self.reserved)?
        };
        self.deadline = Some(
            observed
                .checked_add(remaining)
                .ok_or(BmffError::ResourceLimit)?,
        );
        Ok(())
    }
    fn read_exact(&mut self, offset: u64, bytes: &mut [u8]) -> Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        if self.deadline.is_none() {
            self.reserve(0)?;
        }
        self.check_live()?;
        let range = ByteRange::from_len(offset, bytes.len() as u64)
            .map_err(|_| BmffError::Malformed("logical read overflow"))?;
        if range.end() > self.view.size || range.len() > 1024 * 1024 {
            return Err(BmffError::ResourceLimit);
        }
        self.stats.logical_requests += 1;
        let mut cursor = range.start();
        while cursor < range.end() {
            self.check_live()?;
            let start = cursor / LOGICAL_WINDOW * LOGICAL_WINDOW;
            let end = start.saturating_add(LOGICAL_WINDOW).min(self.view.size);
            let piece_end = range.end().min(end);
            let dest = (cursor - range.start()) as usize;
            let count = (piece_end - cursor) as usize;
            if !self.cache.is_empty()
                && self.cache_start <= cursor
                && piece_end <= self.cache_start + self.cache.len() as u64
            {
                let pos = (cursor - self.cache_start) as usize;
                bytes[dest..dest + count].copy_from_slice(&self.cache[pos..pos + count]);
                self.stats.cache_hits += 1;
            } else {
                let window =
                    ByteRange::new(start, end).map_err(|_| BmffError::Malformed("cache window"))?;
                if self.window_available(window)? {
                    self.session.admit_external("bmff-cache", self.reserved)?;
                    self.cache =
                        self.view
                            .with_read(self.session, window, LOGICAL_WINDOW, |source| {
                                source.to_vec()
                            })?;
                    self.cache_start = start;
                    self.stats.cache_fills += 1;
                    self.stats.source_bytes += window.len();
                    let pos = (cursor - start) as usize;
                    bytes[dest..dest + count].copy_from_slice(&self.cache[pos..pos + count]);
                } else {
                    // An acquisition hole elsewhere in the window must not make
                    // a smaller, fully available metadata request unavailable.
                    self.cache.clear();
                    self.session.admit_external("bmff-precise", self.reserved)?;
                    let precise = ByteRange::new(cursor, piece_end)
                        .map_err(|_| BmffError::Malformed("precise cache fallback"))?;
                    self.view
                        .with_read(self.session, precise, LOGICAL_WINDOW, |source| {
                            bytes[dest..dest + count].copy_from_slice(source)
                        })?;
                    self.stats.precise_reads += 1;
                    self.stats.source_bytes += precise.len();
                }
            }
            cursor = piece_end;
        }
        Ok(())
    }
    fn finish(&mut self) -> Result<()> {
        self.cache.clear();
        if self.deadline.is_some() {
            self.session.check_source()?;
            if let Some(permit) = &self.cpu_permit {
                self.session.check_cpu_permit(permit, 0)?;
            } else {
                self.session.admit_external("bmff-finish", 0)?;
            }
            self.check_live()?;
        }
        Ok(())
    }
    fn source_spans(&self, range: ByteRange) -> Result<Vec<SourceSpan>> {
        let mut spans = Vec::new();
        let mut cursor = range.start();
        for run in &self.view.runs {
            if let Some(part) = run.logical.intersection(range) {
                if part.start() != cursor {
                    return Err(BmffError::Unresolved("missing logical span"));
                }
                let physical = run
                    .image
                    .ok_or(BmffError::Unresolved("unavailable physical span"))?;
                let image = ByteRange::from_len(
                    physical.start() + part.start() - run.logical.start(),
                    part.len(),
                )
                .map_err(|_| BmffError::Malformed("span overflow"))?;
                spans.push(SourceSpan {
                    logical: part,
                    image,
                });
                cursor = part.end();
            }
        }
        if cursor != range.end() {
            return Err(BmffError::Unresolved("incomplete sample span"));
        }
        Ok(spans)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ParseLimits {
    pub boxes: usize,
    pub depth: usize,
    pub tracks: usize,
    pub samples: usize,
    pub table_entries: usize,
    pub metadata_bytes: u64,
    pub payload_hash_bytes: u64,
    pub memory_bytes: u64,
    pub sample_bytes: u64,
}
impl Default for ParseLimits {
    fn default() -> Self {
        Self {
            boxes: 10000,
            depth: 16,
            tracks: 16,
            samples: 100000,
            table_entries: 1000000,
            metadata_bytes: 16 * 1024 * 1024,
            payload_hash_bytes: 512 * 1024 * 1024,
            memory_bytes: 64 * 1024 * 1024,
            sample_bytes: 32 * 1024 * 1024,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxInfo {
    pub kind: [u8; 4],
    pub start: u64,
    pub size: u64,
    pub header_size: u64,
}
impl BoxInfo {
    fn body(&self) -> u64 {
        self.start + self.header_size
    }
    fn end(&self) -> u64 {
        self.start + self.size
    }
}
#[derive(Debug, Clone)]
pub(crate) struct Node {
    pub(crate) info: BoxInfo,
    pub(crate) children: Vec<Node>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SampleDescription {
    pub index: u32,
    pub codec: [u8; 4],
    pub configuration: Vec<u8>,
    pub configuration_sha256: [u8; 32],
    pub sample_entry_sha256: [u8; 32],
    /// Complete bounded original entry, including its box header.
    pub sample_entry: Vec<u8>,
    pub data_reference: u16,
    pub nal_length_size: Option<u8>,
    pub audio_object_type: Option<u8>,
    pub audio_channels: Option<u8>,
    pub audio_sample_rate: Option<u32>,
    pub sample_entry_channels: Option<u16>,
    pub sample_entry_sample_rate: Option<u32>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub segment_duration: u64,
    pub media_time: i64,
    pub rate_integer: i16,
    pub rate_fraction: i16,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sample {
    pub ordinal: u64,
    pub description_index: u32,
    pub logical: ByteRange,
    pub source_spans: Vec<SourceSpan>,
    pub sha256: [u8; 32],
    pub dts: u64,
    pub pts: i128,
    pub duration: u32,
    pub is_sync: bool,
    pub dependency: u8,
}
/// Target table identity remains present even when acquisition omitted payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutSample {
    pub ordinal: u64,
    pub description_index: u32,
    pub logical: ByteRange,
    pub source_spans: Vec<SourceSpan>,
    pub availability: SampleAvailability,
    pub dts: u64,
    pub pts: i128,
    pub duration: u32,
    pub is_sync: bool,
    pub dependency: u8,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SampleAvailability {
    Available { sha256: [u8; 32] },
    Unavailable { reason: AcquisitionMissing },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcquisitionMissing {
    /// Physical input range rejected by the acquisition mask.
    pub image: ByteRange,
}
impl LayoutSample {
    fn into_exact(self) -> Result<Sample> {
        let sha256 = match self.availability {
            SampleAvailability::Available { sha256 } => sha256,
            SampleAvailability::Unavailable { reason } => {
                return Err(BmffError::Filesystem(FilesystemError::Session(
                    SessionError::Acquisition(AcquisitionError::Unavailable(reason.image)),
                )));
            }
        };
        Ok(Sample {
            ordinal: self.ordinal,
            description_index: self.description_index,
            logical: self.logical,
            source_spans: self.source_spans,
            sha256,
            dts: self.dts,
            pts: self.pts,
            duration: self.duration,
            is_sync: self.is_sync,
            dependency: self.dependency,
        })
    }
}
pub(crate) fn acquisition_missing(error: &BmffError) -> Option<AcquisitionMissing> {
    match error {
        BmffError::Filesystem(FilesystemError::Session(SessionError::Acquisition(
            AcquisitionError::Unavailable(image),
        ))) => Some(AcquisitionMissing { image: *image }),
        _ => None,
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track<S = Sample> {
    pub id: u32,
    pub timescale: u32,
    pub media_duration: u64,
    pub handler: [u8; 4],
    pub descriptions: Vec<SampleDescription>,
    pub edits: Vec<Edit>,
    pub samples: Vec<S>,
    pub sample_groups: Vec<SampleGroup>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SampleGroup {
    pub grouping_type: [u8; 4],
    pub descriptions: Vec<Vec<u8>>,
    pub assignments: Vec<(u32, u32)>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SampleMap<S = Sample> {
    pub file_size: u64,
    pub movie_timescale: u32,
    pub tracks: Vec<Track<S>>,
    pub container_boxes: Vec<BoxInfo>,
}

pub(crate) struct Parser<'a, R: ReadAt> {
    reader: &'a mut R,
    limits: ParseLimits,
    metadata: u64,
    hashed: u64,
    box_count: usize,
    sample_count: usize,
    sample_intervals: std::collections::BTreeMap<u64, u64>,
    span_memory: u64,
}
fn malformed(reason: &'static str) -> BmffError {
    BmffError::Malformed(reason)
}
fn u16be(b: &[u8], p: usize) -> Result<u16> {
    Ok(u16::from_be_bytes(
        b.get(p..p + 2)
            .ok_or_else(|| malformed("truncated integer"))?
            .try_into()
            .unwrap(),
    ))
}
pub(crate) fn u32be(b: &[u8], p: usize) -> Result<u32> {
    Ok(u32::from_be_bytes(
        b.get(p..p + 4)
            .ok_or_else(|| malformed("truncated integer"))?
            .try_into()
            .unwrap(),
    ))
}
pub(crate) fn u64be(b: &[u8], p: usize) -> Result<u64> {
    Ok(u64::from_be_bytes(
        b.get(p..p + 8)
            .ok_or_else(|| malformed("truncated integer"))?
            .try_into()
            .unwrap(),
    ))
}
pub(crate) fn unique<'a>(nodes: &'a [Node], kind: &[u8; 4]) -> Result<Option<&'a Node>> {
    let mut found = nodes.iter().filter(|n| &n.info.kind == kind);
    let first = found.next();
    if found.next().is_some() {
        return Err(malformed("duplicate required box"));
    }
    Ok(first)
}
pub(crate) fn required<'a>(nodes: &'a [Node], kind: &[u8; 4]) -> Result<&'a Node> {
    unique(nodes, kind)?.ok_or(BmffError::Unresolved("missing required target table"))
}
fn is_container(kind: &[u8; 4]) -> bool {
    matches!(
        kind,
        b"moov"
            | b"trak"
            | b"mdia"
            | b"minf"
            | b"stbl"
            | b"dinf"
            | b"edts"
            | b"mvex"
            | b"moof"
            | b"traf"
    )
}
impl<R: ReadAt> Parser<'_, R> {
    fn read(&mut self, offset: u64, len: usize) -> Result<Vec<u8>> {
        if len > 1024 * 1024
            || offset
                .checked_add(len as u64)
                .is_none_or(|v| v > self.reader.len())
        {
            return Err(malformed("metadata read outside view"));
        }
        self.metadata = self
            .metadata
            .checked_add(len as u64)
            .filter(|v| *v <= self.limits.metadata_bytes)
            .ok_or(BmffError::ResourceLimit)?;
        let mut b = vec![0; len];
        self.reader.read_exact(offset, &mut b)?;
        Ok(b)
    }
    pub(crate) fn file_type(&mut self, roots: &[Node]) -> Result<()> {
        let ftyp = required(roots, b"ftyp")?;
        let brands = self.body(ftyp)?;
        if brands.len() < 8 || brands.len() % 4 != 0 {
            return Err(malformed("file type"));
        }
        let known = brands
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != 1)
            .any(|(_, brand)| {
                matches!(
                    brand,
                    b"isom" | b"iso2" | b"iso6" | b"mp41" | b"mp42" | b"avc1" | b"hvc1"
                )
            });
        if !known {
            return Err(BmffError::Unsupported("MP4 brand"));
        }
        Ok(())
    }
    pub(crate) fn body(&mut self, node: &Node) -> Result<Vec<u8>> {
        let n = node.info.size - node.info.header_size;
        let size = usize::try_from(n).map_err(|_| BmffError::ResourceLimit)?;
        if n > self.limits.metadata_bytes || size > 1024 * 1024 {
            return Err(BmffError::ResourceLimit);
        }
        self.read(node.info.body(), size)
    }
    fn children(&mut self, start: u64, end: u64, depth: usize) -> Result<Vec<Node>> {
        if depth > self.limits.depth {
            return Err(BmffError::ResourceLimit);
        }
        let mut cursor = start;
        let mut nodes = Vec::new();
        while cursor < end {
            if end - cursor < 8 {
                return Err(malformed("truncated box header"));
            }
            self.box_count += 1;
            if self.box_count > self.limits.boxes {
                return Err(BmffError::ResourceLimit);
            }
            let header = self.read(cursor, 8)?;
            let small = u32be(&header, 0)?;
            let kind = header[4..8].try_into().unwrap();
            let (header_size, size) = if small == 1 {
                if end - cursor < 16 {
                    return Err(malformed("truncated extended box"));
                }
                (16, u64be(&self.read(cursor + 8, 8)?, 0)?)
            } else if small == 0 {
                (8, end - cursor)
            } else {
                (8, u64::from(small))
            };
            if size < header_size || size > end - cursor {
                return Err(malformed("box exceeds parent"));
            }
            let info = BoxInfo {
                kind,
                start: cursor,
                size,
                header_size,
            };
            let children = if is_container(&kind) {
                self.children(info.body(), info.end(), depth + 1)?
            } else {
                Vec::new()
            };
            cursor = info.end();
            nodes.push(Node { info, children });
        }
        Ok(nodes)
    }
    fn table(&mut self, node: &Node, entry_size: usize, prefix: usize) -> Result<(Vec<u8>, usize)> {
        let bytes = self.body(node)?;
        if bytes.len() < prefix || bytes[..4] != [0; 4] {
            return Err(BmffError::Unsupported("table version"));
        }
        let count = u32be(&bytes, prefix - 4)? as usize;
        if count > self.limits.table_entries
            || prefix
                .checked_add(
                    count
                        .checked_mul(entry_size)
                        .ok_or(BmffError::ResourceLimit)?,
                )
                .is_none_or(|n| n != bytes.len())
        {
            return Err(malformed("table cardinality"));
        }
        Ok((bytes, count))
    }
    pub(crate) fn timescale(&mut self, node: &Node) -> Result<u32> {
        let b = self.body(node)?;
        let minimum = match (&node.info.kind, b.first()) {
            (b"mvhd", Some(0)) => 100,
            (b"mvhd", Some(1)) => 112,
            (b"mdhd", Some(0)) => 24,
            (b"mdhd", Some(1)) => 36,
            _ => return Err(BmffError::Unsupported("time header profile")),
        };
        if b.len() < minimum || b[1..4] != [0, 0, 0] {
            return Err(malformed("truncated or flagged time header"));
        }
        let p = match b.first() {
            Some(0) => 12,
            Some(1) => 20,
            _ => return Err(BmffError::Unsupported("time header version")),
        };
        let timescale = u32be(&b, p)?;
        if timescale == 0 {
            return Err(malformed("zero timescale"));
        }
        Ok(timescale)
    }
    fn edits(&mut self, trak: &Node) -> Result<Vec<Edit>> {
        let Some(edts) = unique(&trak.children, b"edts")? else {
            return Ok(Vec::new());
        };
        let elst = required(&edts.children, b"elst")?;
        let b = self.body(elst)?;
        if b.len() < 8 || b[1..4] != [0; 3] {
            return Err(malformed("truncated or flagged edit header"));
        }
        let version = b[0];
        let width = match version {
            0 => 12,
            1 => 20,
            _ => return Err(BmffError::Unsupported("edit version")),
        };
        let count = u32be(&b, 4)? as usize;
        if count > 64 || 8 + count * width != b.len() {
            return Err(malformed("edit cardinality"));
        }
        let mut edits = Vec::new();
        for i in 0..count {
            let p = 8 + i * width;
            let (duration, time, r) = if version == 0 {
                (
                    u64::from(u32be(&b, p)?),
                    i64::from(u32be(&b, p + 4)? as i32),
                    p + 8,
                )
            } else {
                (u64be(&b, p)?, u64be(&b, p + 8)? as i64, p + 16)
            };
            let integer = u16be(&b, r)? as i16;
            let fraction = u16be(&b, r + 2)? as i16;
            if time < -1 || integer != 1 || fraction != 0 {
                return Err(BmffError::Unsupported("non-unit or dwell edit"));
            }
            edits.push(Edit {
                segment_duration: duration,
                media_time: time,
                rate_integer: integer,
                rate_fraction: fraction,
            });
        }
        Ok(edits)
    }
    fn data_references(&mut self, minf: &Node) -> Result<usize> {
        let dinf = required(&minf.children, b"dinf")?;
        let dref = required(&dinf.children, b"dref")?;
        let header = self.read(dref.info.body(), 8)?;
        if header[..4] != [0, 0, 0, 0] {
            return Err(BmffError::Unsupported("data reference version"));
        }
        let count = u32be(&header, 4)? as usize;
        if count == 0 || count > 64 {
            return Err(BmffError::Unsupported("data reference count"));
        }
        let refs = self.children(dref.info.body() + 8, dref.info.end(), 1)?;
        if refs.len() != count {
            return Err(malformed("data reference count"));
        }
        for reference in refs {
            if reference.info.kind != *b"url " || self.body(&reference)? != [0, 0, 0, 1] {
                return Err(BmffError::Unsupported("external data reference"));
            }
        }
        Ok(count)
    }
    fn descriptions(
        &mut self,
        stsd: &Node,
        reference_count: usize,
        handler: [u8; 4],
    ) -> Result<Vec<SampleDescription>> {
        let header = self.read(stsd.info.body(), 8)?;
        if header[..4] != [0, 0, 0, 0] {
            return Err(BmffError::Unsupported("sample description version"));
        }
        let count = u32be(&header, 4)? as usize;
        if count == 0 || count > 64 {
            return Err(BmffError::Unsupported("sample description count"));
        }
        let entries = self.children(stsd.info.body() + 8, stsd.info.end(), 1)?;
        if entries.len() != count {
            return Err(malformed("description cardinality"));
        }
        let mut descriptions = Vec::new();
        for (index, entry) in entries.into_iter().enumerate() {
            let raw = self.body(&entry)?;
            let reference = u16be(&raw, 6)?;
            if reference == 0 || usize::from(reference) > reference_count {
                return Err(malformed("sample data reference"));
            }
            let codec = entry.info.kind;
            let (prefix, config_kind) = match (&codec, &handler) {
                (b"avc1", b"vide") => (78, *b"avcC"),
                (b"hvc1", b"vide") => (78, *b"hvcC"),
                (b"mp4a", b"soun") => {
                    if u16be(&raw, 8)? != 0 {
                        return Err(BmffError::Unsupported("audio sample-entry version"));
                    }
                    (28, *b"esds")
                }
                _ => return Err(BmffError::Unsupported("sample-entry codec/profile")),
            };
            if raw.len() < prefix {
                return Err(malformed("truncated sample entry"));
            }
            let children = self.children(entry.info.body() + prefix as u64, entry.info.end(), 2)?;
            let config = self.body(required(&children, &config_kind)?)?;
            let (configuration, nal_length_size, audio_object_type) = if codec == *b"avc1" {
                if config.len() < 7 || config[0] != 1 {
                    return Err(malformed("AVC configuration"));
                }
                let width = (config[4] & 3) + 1;
                if width == 3 {
                    return Err(BmffError::Unsupported("reserved AVC NAL length width"));
                }
                validate_avcc(&config)?;
                (config, Some(width), None)
            } else if codec == *b"hvc1" {
                if config.len() < 23 || config[0] != 1 {
                    return Err(malformed("HEVC configuration"));
                }
                let width = (config[21] & 3) + 1;
                validate_hvcc(&config)?;
                (config, Some(width), None)
            } else {
                let asc = aac_configuration(&config)?;
                (asc, None, Some(2))
            };
            let sample_entry = self.read(entry.info.start, entry.info.size as usize)?;
            descriptions.push(SampleDescription {
                index: index as u32 + 1,
                codec,
                configuration_sha256: Sha256::digest(&configuration).into(),
                sample_entry_sha256: Sha256::digest(&sample_entry).into(),
                sample_entry,
                data_reference: reference,
                nal_length_size,
                audio_object_type,
                audio_channels: if codec == *b"mp4a" {
                    Some((configuration[1] >> 3) & 15)
                } else {
                    None
                },
                audio_sample_rate: if codec == *b"mp4a" {
                    Some(aac_rate(&configuration))
                } else {
                    None
                },
                sample_entry_channels: if codec == *b"mp4a" {
                    Some(u16be(&raw, 16)?)
                } else {
                    None
                },
                sample_entry_sample_rate: if codec == *b"mp4a" {
                    Some(u32be(&raw, 24)? >> 16)
                } else {
                    None
                },
                configuration,
            });
        }
        Ok(descriptions)
    }
    pub(crate) fn track_header(&mut self, trak: &Node) -> Result<Track<LayoutSample>> {
        let tkhd = self.body(required(&trak.children, b"tkhd")?)?;
        let minimum = match tkhd.first() {
            Some(0) => 84,
            Some(1) => 96,
            _ => return Err(BmffError::Unsupported("track header version")),
        };
        if tkhd.len() < minimum {
            return Err(malformed("truncated track header"));
        }
        let id = u32be(
            &tkhd,
            match tkhd.first() {
                Some(0) => 12,
                Some(1) => 20,
                _ => return Err(BmffError::Unsupported("track header version")),
            },
        )?;
        if id == 0 {
            return Err(malformed("zero track id"));
        }
        let mdia = required(&trak.children, b"mdia")?;
        let mdhd = required(&mdia.children, b"mdhd")?;
        let timescale = self.timescale(mdhd)?;
        let media_header = self.body(mdhd)?;
        let media_duration = if media_header[0] == 0 {
            u64::from(u32be(&media_header, 16)?)
        } else {
            u64be(&media_header, 24)?
        };
        let handler_bytes = self.body(required(&mdia.children, b"hdlr")?)?;
        let handler: [u8; 4] = handler_bytes
            .get(8..12)
            .ok_or_else(|| malformed("handler"))?
            .try_into()
            .unwrap();
        if handler_bytes.len() < 24 || handler_bytes[..4] != [0; 4] {
            return Err(malformed("handler header"));
        }
        let minf = required(&mdia.children, b"minf")?;
        let refs = self.data_references(minf)?;
        let stbl = required(&minf.children, b"stbl")?;
        let descriptions = self.descriptions(required(&stbl.children, b"stsd")?, refs, handler)?;
        Ok(Track {
            id,
            timescale,
            media_duration,
            handler,
            descriptions,
            edits: self.edits(trak)?,
            samples: Vec::new(),
            sample_groups: Vec::new(),
        })
    }
    fn track(&mut self, trak: &Node, mdats: &[ByteRange]) -> Result<Track<LayoutSample>> {
        let Track {
            id,
            timescale,
            media_duration,
            handler,
            descriptions,
            edits,
            ..
        } = self.track_header(trak)?;
        let mdia = required(&trak.children, b"mdia")?;
        let minf = required(&mdia.children, b"minf")?;
        let stbl = required(&minf.children, b"stbl")?;
        let sizes = self.sizes(stbl)?;
        let count = sizes.len();
        self.sample_count = self
            .sample_count
            .checked_add(count)
            .filter(|n| *n <= self.limits.samples)
            .ok_or(BmffError::ResourceLimit)?;
        let (stts, entries) = self.table(required(&stbl.children, b"stts")?, 8, 8)?;
        let mut durations = Vec::with_capacity(count);
        for i in 0..entries {
            let n = u32be(&stts, 8 + i * 8)? as usize;
            let duration = u32be(&stts, 12 + i * 8)?;
            if n == 0 || duration == 0 || durations.len().checked_add(n).is_none_or(|v| v > count) {
                return Err(malformed("decode timing cardinality"));
            }
            durations.extend(std::iter::repeat_n(duration, n));
        }
        if durations.len() != count {
            return Err(malformed("sample timing count"));
        }
        let mut composition = vec![0i64; count];
        if let Some(ctts) = unique(&stbl.children, b"ctts")? {
            let b = self.body(ctts)?;
            if b.len() < 8 {
                return Err(malformed("truncated composition header"));
            }
            let version = b[0];
            if version > 1 || b[1..4] != [0; 3] {
                return Err(BmffError::Unsupported("composition version"));
            }
            let n = u32be(&b, 4)? as usize;
            if n > self.limits.table_entries || 8 + n * 8 != b.len() {
                return Err(malformed("composition cardinality"));
            }
            let mut cursor: usize = 0;
            for i in 0..n {
                let run = u32be(&b, 8 + i * 8)? as usize;
                let raw = u32be(&b, 12 + i * 8)?;
                let value = if version == 0 {
                    i64::from(raw)
                } else {
                    i64::from(raw as i32)
                };
                let end = cursor
                    .checked_add(run)
                    .filter(|end| *end <= count)
                    .ok_or_else(|| malformed("composition count"))?;
                if run == 0 {
                    return Err(malformed("zero composition run"));
                }
                composition[cursor..end].fill(value);
                cursor = end;
            }
            if cursor != count {
                return Err(malformed("composition sample count"));
            }
        }
        let mut sync = vec![true; count];
        if let Some(stss) = unique(&stbl.children, b"stss")? {
            sync.fill(false);
            let (b, n) = self.table(stss, 4, 8)?;
            let mut previous = 0;
            for i in 0..n {
                let ordinal = u32be(&b, 8 + i * 4)? as usize;
                if ordinal == 0 || ordinal > count || ordinal <= previous {
                    return Err(malformed("sync sample ordinal"));
                }
                sync[ordinal - 1] = true;
                previous = ordinal;
            }
        }
        let dependency = if let Some(sdtp) = unique(&stbl.children, b"sdtp")? {
            let b = self.body(sdtp)?;
            if b.len() != count + 4 || b[..4] != [0, 0, 0, 0] {
                return Err(malformed("dependency cardinality"));
            }
            b[4..].to_vec()
        } else {
            vec![0; count]
        };
        for unsupported in [b"stsh", b"subs", b"saiz", b"saio", b"senc"] {
            if unique(&stbl.children, unsupported)?.is_some() {
                return Err(BmffError::Unsupported("dependency or encryption table"));
            }
        }
        let stco = unique(&stbl.children, b"stco")?;
        let co64 = unique(&stbl.children, b"co64")?;
        let (offset_box, width) = match (stco, co64) {
            (Some(n), None) => (n, 4),
            (None, Some(n)) => (n, 8),
            _ => return Err(malformed("chunk offset table")),
        };
        let (offsets, chunk_count) = self.table(offset_box, width, 8)?;
        if chunk_count == 0 {
            return Err(BmffError::Unresolved("no chunk layout"));
        }
        let (stsc, sc_count) = self.table(required(&stbl.children, b"stsc")?, 12, 8)?;
        if sc_count == 0 {
            return Err(malformed("empty sample-to-chunk"));
        }
        let mut mapping = Vec::new();
        for i in 0..sc_count {
            let p = 8 + i * 12;
            let first = u32be(&stsc, p)? as usize;
            let per = u32be(&stsc, p + 4)? as usize;
            let desc = u32be(&stsc, p + 8)?;
            if first == 0
                || first > chunk_count
                || per == 0
                || desc == 0
                || desc as usize > descriptions.len()
                || mapping.last().is_some_and(|(old, _, _)| *old >= first)
            {
                return Err(malformed("sample-to-chunk entry"));
            }
            mapping.push((first, per, desc));
        }
        if mapping[0].0 != 1 {
            return Err(malformed("first chunk is not one"));
        }
        let mut samples = Vec::with_capacity(count);
        let mut ordinal = 0usize;
        let mut dts = 0u64;
        let mut sc = 0usize;
        for chunk in 1..=chunk_count {
            while sc + 1 < mapping.len() && mapping[sc + 1].0 <= chunk {
                sc += 1;
            }
            let (_, per, description) = mapping[sc];
            let p = 8 + (chunk - 1) * width;
            let mut offset = if width == 4 {
                u64::from(u32be(&offsets, p)?)
            } else {
                u64be(&offsets, p)?
            };
            for _ in 0..per {
                if ordinal >= count {
                    return Err(malformed("chunk sample count"));
                }
                let size = u64::from(sizes[ordinal]);
                if size == 0 || size > self.limits.sample_bytes {
                    return Err(BmffError::ResourceLimit);
                }
                let logical = ByteRange::from_len(offset, size)
                    .map_err(|_| malformed("sample offset overflow"))?;
                if !mdats.iter().any(|data| data.contains(logical)) {
                    return Err(malformed("sample outside media data"));
                }
                let (spans, availability) = self.payload(logical)?;
                samples.push(LayoutSample {
                    ordinal: ordinal as u64,
                    description_index: description,
                    logical,
                    source_spans: spans,
                    availability,
                    dts,
                    pts: i128::from(dts) + i128::from(composition[ordinal]),
                    duration: durations[ordinal],
                    is_sync: sync[ordinal],
                    dependency: dependency[ordinal],
                });
                dts = dts
                    .checked_add(u64::from(durations[ordinal]))
                    .ok_or_else(|| malformed("decode time overflow"))?;
                offset = logical.end();
                ordinal += 1;
            }
        }
        if ordinal != count {
            return Err(malformed("unmapped samples"));
        }
        if media_duration != dts {
            return Err(malformed("media duration and decode table disagree"));
        }
        Ok(Track {
            id,
            timescale,
            media_duration,
            handler,
            descriptions,
            edits,
            samples,
            sample_groups: self.sample_groups(stbl, count)?,
        })
    }
    pub(crate) fn payload(
        &mut self,
        logical: ByteRange,
    ) -> Result<(Vec<SourceSpan>, SampleAvailability)> {
        if logical.is_empty() || logical.len() > self.limits.sample_bytes {
            return Err(BmffError::ResourceLimit);
        }
        // Reserve unique logical geometry before hashing or expanding physical
        // spans. Repeated/conflicting chunk layouts cannot amplify expensive work.
        if self
            .sample_intervals
            .range(..=logical.start())
            .next_back()
            .is_some_and(|(_, end)| *end > logical.start())
            || self
                .sample_intervals
                .range(logical.start()..)
                .next()
                .is_some_and(|(start, _)| *start < logical.end())
        {
            return Err(malformed("overlapping sample payloads"));
        }
        self.sample_intervals.insert(logical.start(), logical.end());
        let availability = match self.hash(logical) {
            Ok(sha256) => SampleAvailability::Available { sha256 },
            Err(error) => match acquisition_missing(&error) {
                Some(reason) => SampleAvailability::Unavailable { reason },
                None => return Err(error),
            },
        };
        let spans = self.reader.source_spans(logical)?;
        self.span_memory = self
            .span_memory
            .checked_add(
                (spans.capacity() as u64)
                    .checked_mul(std::mem::size_of::<SourceSpan>() as u64)
                    .ok_or(BmffError::ResourceLimit)?,
            )
            .ok_or(BmffError::ResourceLimit)?;
        let baseline = (self.limits.samples as u64)
            .checked_mul(384)
            .and_then(|n| n.checked_add(self.limits.metadata_bytes))
            .ok_or(BmffError::ResourceLimit)?;
        if baseline
            .checked_add(self.span_memory)
            .is_none_or(|n| n > self.limits.memory_bytes)
        {
            return Err(BmffError::ResourceLimit);
        }
        Ok((spans, availability))
    }
    fn sample_groups(&mut self, stbl: &Node, count: usize) -> Result<Vec<SampleGroup>> {
        let sgpd = unique(&stbl.children, b"sgpd")?;
        let sbgp = unique(&stbl.children, b"sbgp")?;
        let (sgpd, sbgp) = match (sgpd, sbgp) {
            (None, None) => return Ok(Vec::new()),
            (Some(a), Some(b)) => (a, b),
            _ => return Err(BmffError::Unsupported("incomplete sample grouping")),
        };
        let description = self.body(sgpd)?;
        if description.len() < 12 || description[1..4] != [0; 3] {
            return Err(malformed("truncated or flagged group description"));
        }
        let version = *description
            .first()
            .ok_or_else(|| malformed("sample group description"))?;
        let grouping: [u8; 4] = description
            .get(4..8)
            .ok_or_else(|| malformed("group type"))?
            .try_into()
            .unwrap();
        if !matches!(&grouping, b"roll" | b"prol") {
            return Err(BmffError::Unsupported("sample group type"));
        }
        let (default_length, n, mut cursor) = match version {
            0 => (2, u32be(&description, 8)? as usize, 12),
            1 => (
                u32be(&description, 8)? as usize,
                u32be(&description, 12)? as usize,
                16,
            ),
            _ => return Err(BmffError::Unsupported("sample group version")),
        };
        if n > self.limits.table_entries {
            return Err(BmffError::ResourceLimit);
        }
        let mut descriptions = Vec::new();
        for _ in 0..n {
            let length = if default_length == 0 {
                let v = u32be(&description, cursor)? as usize;
                cursor += 4;
                v
            } else {
                default_length
            };
            if length != 2 {
                return Err(BmffError::Unsupported("roll distance representation"));
            }
            let end = cursor.checked_add(length).ok_or(BmffError::ResourceLimit)?;
            descriptions.push(
                description
                    .get(cursor..end)
                    .ok_or_else(|| malformed("roll description"))?
                    .to_vec(),
            );
            cursor = end;
        }
        if cursor != description.len() {
            return Err(malformed("group description trailing bytes"));
        }
        let assignment = self.body(sbgp)?;
        if assignment.len() < 12 || assignment[1..4] != [0; 3] {
            return Err(malformed("truncated or flagged group assignment"));
        }
        let version = *assignment
            .first()
            .ok_or_else(|| malformed("sample group assignments"))?;
        if assignment.get(4..8) != Some(grouping.as_slice()) {
            return Err(malformed("sample group type mismatch"));
        }
        let count_at = match version {
            0 => 8,
            1 => {
                if u32be(&assignment, 8)? != 0 {
                    return Err(BmffError::Unsupported("grouping type parameter"));
                }
                12
            }
            _ => return Err(BmffError::Unsupported("assignment version")),
        };
        let n = u32be(&assignment, count_at)? as usize;
        let prefix = count_at + 4;
        if n > self.limits.table_entries || prefix + n * 8 != assignment.len() {
            return Err(malformed("sample group assignment cardinality"));
        }
        let mut assigned = 0usize;
        let mut assignments = Vec::new();
        for i in 0..n {
            let samples = u32be(&assignment, prefix + i * 8)?;
            let index = u32be(&assignment, prefix + i * 8 + 4)?;
            if samples == 0 || index as usize > descriptions.len() {
                return Err(malformed("sample group assignment"));
            }
            assigned = assigned
                .checked_add(samples as usize)
                .filter(|v| *v <= count)
                .ok_or_else(|| malformed("sample group count"))?;
            assignments.push((samples, index));
        }
        if assigned != count {
            return Err(malformed("sample group sample count"));
        }
        Ok(vec![SampleGroup {
            grouping_type: grouping,
            descriptions,
            assignments,
        }])
    }
    fn sizes(&mut self, stbl: &Node) -> Result<Vec<u32>> {
        match (
            unique(&stbl.children, b"stsz")?,
            unique(&stbl.children, b"stz2")?,
        ) {
            (Some(node), None) => {
                let b = self.body(node)?;
                if b.len() < 12 || b[..4] != [0, 0, 0, 0] {
                    return Err(malformed("sample-size header"));
                }
                let constant = u32be(&b, 4)?;
                let count = u32be(&b, 8)? as usize;
                if count == 0 {
                    return Err(BmffError::Unresolved("empty sample table"));
                }
                if count > self.limits.samples {
                    return Err(BmffError::ResourceLimit);
                }
                if constant > 0 {
                    if b.len() != 12 {
                        return Err(malformed("constant sample sizes"));
                    }
                    Ok(vec![constant; count])
                } else {
                    if 12 + count * 4 != b.len() {
                        return Err(malformed("sample-size count"));
                    }
                    (0..count).map(|i| u32be(&b, 12 + i * 4)).collect()
                }
            }
            (None, Some(node)) => {
                let b = self.body(node)?;
                if b.len() < 12 || b[..7] != [0; 7] {
                    return Err(malformed("compact sample-size header"));
                }
                let width = b[7];
                if ![4, 8, 16].contains(&width) {
                    return Err(BmffError::Unsupported("compact sample-size width"));
                }
                let count = u32be(&b, 8)? as usize;
                if count == 0 || count > self.limits.samples {
                    return Err(BmffError::ResourceLimit);
                }
                let needed = (count * usize::from(width)).div_ceil(8);
                if b.len() != 12 + needed {
                    return Err(malformed("compact sample-size count"));
                }
                Ok((0..count)
                    .map(|i| match width {
                        4 => u32::from(if i % 2 == 0 {
                            b[12 + i / 2] >> 4
                        } else {
                            b[12 + i / 2] & 15
                        }),
                        8 => u32::from(b[12 + i]),
                        _ => u32::from(u16::from_be_bytes([b[12 + i * 2], b[13 + i * 2]])),
                    })
                    .collect())
            }
            _ => Err(BmffError::Unresolved("missing or conflicting sample sizes")),
        }
    }
    fn hash(&mut self, range: ByteRange) -> Result<[u8; 32]> {
        self.hashed = self
            .hashed
            .checked_add(range.len())
            .filter(|v| *v <= self.limits.payload_hash_bytes)
            .ok_or(BmffError::ResourceLimit)?;
        let mut hash = Sha256::new();
        let mut cursor = range.start();
        let mut buffer = vec![0; 65536];
        while cursor < range.end() {
            let n = (range.end() - cursor).min(buffer.len() as u64) as usize;
            self.reader.read_exact(cursor, &mut buffer[..n])?;
            hash.update(&buffer[..n]);
            cursor += n as u64;
        }
        Ok(hash.finalize().into())
    }
}

fn validate_avcc(b: &[u8]) -> Result<()> {
    let mut p = 6;
    let sps = (b[5] & 31) as usize;
    if sps == 0 {
        return Err(malformed("missing AVC SPS"));
    }
    for _ in 0..sps {
        let n = usize::from(u16be(b, p)?);
        p = p.checked_add(2 + n).ok_or(BmffError::ResourceLimit)?;
        if n == 0 || p > b.len() {
            return Err(malformed("AVC SPS length"));
        }
    }
    let count = *b.get(p).ok_or_else(|| malformed("missing AVC PPS count"))? as usize;
    p += 1;
    if count == 0 {
        return Err(malformed("missing AVC PPS"));
    }
    for _ in 0..count {
        let n = usize::from(u16be(b, p)?);
        p += 2 + n;
        if n == 0 || p > b.len() {
            return Err(malformed("AVC PPS length"));
        }
    } // High-profile extension bytes are preserved; codec syntax qualification is separate.
    Ok(())
}
fn validate_hvcc(b: &[u8]) -> Result<()> {
    let arrays = b[22] as usize;
    let mut p = 23;
    let mut parameters = [false; 3];
    for _ in 0..arrays {
        let kind = *b.get(p).ok_or_else(|| malformed("HEVC array"))? & 63;
        p += 1;
        let count = usize::from(u16be(b, p)?);
        p += 2;
        for _ in 0..count {
            let n = usize::from(u16be(b, p)?);
            p += 2 + n;
            if n == 0 || p > b.len() {
                return Err(malformed("HEVC NAL length"));
            }
        }
        if (32..=34).contains(&kind) && count > 0 {
            parameters[(kind - 32) as usize] = true;
        }
    }
    if p != b.len() || !parameters.iter().all(|v| *v) {
        return Err(malformed("HEVC parameter arrays"));
    }
    Ok(())
}
fn descriptor<'a>(b: &'a [u8], cursor: &mut usize) -> Result<(u8, &'a [u8])> {
    let kind = *b.get(*cursor).ok_or_else(|| malformed("descriptor tag"))?;
    *cursor += 1;
    let mut len = 0usize;
    let mut terminated = false;
    for _ in 0..4 {
        let byte = *b
            .get(*cursor)
            .ok_or_else(|| malformed("descriptor length"))?;
        *cursor += 1;
        len = len
            .checked_mul(128)
            .and_then(|v| v.checked_add((byte & 127) as usize))
            .ok_or(BmffError::ResourceLimit)?;
        if byte & 128 == 0 {
            terminated = true;
            break;
        }
    }
    if !terminated {
        return Err(malformed("descriptor length overflow"));
    }
    let end = cursor.checked_add(len).ok_or(BmffError::ResourceLimit)?;
    let payload = b
        .get(*cursor..end)
        .ok_or_else(|| malformed("descriptor exceeds parent"))?;
    *cursor = end;
    Ok((kind, payload))
}
fn aac_configuration(esds: &[u8]) -> Result<Vec<u8>> {
    if esds.get(..4) != Some(&[0, 0, 0, 0]) {
        return Err(BmffError::Unsupported("ES descriptor version"));
    }
    let mut p = 4;
    let (tag, es) = descriptor(esds, &mut p)?;
    if tag != 3 || es.len() < 3 {
        return Err(malformed("ES descriptor"));
    }
    let flags = es[2];
    let mut child = 3;
    if flags & 128 != 0 {
        child += 2;
    }
    if flags & 64 != 0 {
        let n = *es.get(child).ok_or_else(|| malformed("ES URL"))? as usize;
        child += 1 + n;
    }
    if flags & 32 != 0 {
        child += 2;
    }
    let (tag, decoder) = descriptor(es, &mut child)?;
    if tag != 4 || decoder.len() < 13 || decoder[0] != 0x40 {
        return Err(BmffError::Unsupported("audio decoder object"));
    }
    let mut p = 13;
    let (tag, asc) = descriptor(decoder, &mut p)?;
    if tag != 5 || asc.len() < 2 {
        return Err(malformed("audio specific config"));
    }
    let object = asc[0] >> 3;
    let rate = ((asc[0] & 7) << 1) | (asc[1] >> 7);
    let channels = (asc[1] >> 3) & 15;
    if object != 2 || rate > 12 || channels == 0 || channels > 2 {
        return Err(BmffError::Unsupported(
            "AAC profile/rate/channel configuration",
        ));
    }
    Ok(asc.to_vec())
}
fn aac_rate(asc: &[u8]) -> u32 {
    const RATES: [u32; 13] = [
        96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
    ];
    RATES[(((asc[0] & 7) << 1) | (asc[1] >> 7)) as usize]
}

pub fn inspect(reader: &mut impl ReadAt, limits: ParseLimits) -> Result<SampleMap> {
    inspect_layout(reader, limits)?.into_exact()
}
impl SampleMap<LayoutSample> {
    /// Move an already parsed all-available target map into exact representation;
    /// unavailable payload remains an error, never a placeholder hash.
    pub fn into_exact(self) -> Result<SampleMap> {
        let layout = self;
        let tracks = layout
            .tracks
            .into_iter()
            .map(|track| {
                Ok(Track {
                    id: track.id,
                    timescale: track.timescale,
                    media_duration: track.media_duration,
                    handler: track.handler,
                    descriptions: track.descriptions,
                    edits: track.edits,
                    samples: track
                        .samples
                        .into_iter()
                        .map(LayoutSample::into_exact)
                        .collect::<Result<_>>()?,
                    sample_groups: track.sample_groups,
                })
            })
            .collect::<Result<_>>()?;
        Ok(SampleMap {
            file_size: layout.file_size,
            movie_timescale: layout.movie_timescale,
            tracks,
            container_boxes: layout.container_boxes,
        })
    }
}
/// Parses target tables once; only acquisition-missing payload becomes unavailable.
/// Filesystem ownership qualification and source-generation guards still apply.
pub fn inspect_layout(
    reader: &mut impl ReadAt,
    limits: ParseLimits,
) -> Result<SampleMap<LayoutSample>> {
    let result = inspect_inner(reader, limits);
    // Terminal source/control/budget authority takes precedence over parse
    // classification, including when the target metadata is damaged.
    reader.finish()?;
    result
}
pub(crate) fn with_container<R: ReadAt, T>(
    reader: &mut R,
    limits: ParseLimits,
    inspect: impl FnOnce(&mut Parser<'_, R>, &[Node]) -> Result<T>,
) -> Result<T> {
    let result = with_container_unsettled(reader, limits, inspect);
    reader.finish()?;
    result
}
fn with_container_unsettled<R: ReadAt, T>(
    reader: &mut R,
    limits: ParseLimits,
    inspect: impl FnOnce(&mut Parser<'_, R>, &[Node]) -> Result<T>,
) -> Result<T> {
    if limits.boxes == 0
        || limits.boxes > 100000
        || limits.depth == 0
        || limits.depth > 64
        || limits.tracks == 0
        || limits.tracks > 64
        || limits.samples == 0
        || limits.samples > 1000000
        || limits.table_entries > 1000000
        || limits.metadata_bytes > 64 * 1024 * 1024
        || limits.memory_bytes < limits.samples as u64 * 384 + limits.metadata_bytes
        || limits.memory_bytes > 1024 * 1024 * 1024
    {
        return Err(BmffError::ResourceLimit);
    }
    reader.reserve(limits.memory_bytes)?;
    let file_size = reader.len();
    let mut parser = Parser {
        reader,
        limits,
        metadata: 0,
        hashed: 0,
        box_count: 0,
        sample_count: 0,
        sample_intervals: std::collections::BTreeMap::new(),
        span_memory: 0,
    };
    let roots = parser.children(0, file_size, 0)?;
    inspect(&mut parser, &roots)
}
pub(crate) fn flatten_boxes(nodes: &[Node], out: &mut Vec<BoxInfo>) {
    for node in nodes {
        out.push(node.info.clone());
        flatten_boxes(&node.children, out);
    }
}
fn inspect_inner(reader: &mut impl ReadAt, limits: ParseLimits) -> Result<SampleMap<LayoutSample>> {
    with_container_unsettled(reader, limits, |parser, roots| {
        let file_size = parser.reader.len();

        parser.file_type(roots)?;
        if roots.iter().any(|n| n.info.kind == *b"moof") {
            return Err(BmffError::Unsupported(
                "ISO fragmented MP4 is a separate profile",
            ));
        }
        let moov = required(roots, b"moov")?;
        if unique(&moov.children, b"mvex")?.is_some() {
            return Err(BmffError::Unsupported("fragment defaults"));
        }
        let movie_timescale = parser.timescale(required(&moov.children, b"mvhd")?)?;
        let mdats = roots
            .iter()
            .filter(|n| n.info.kind == *b"mdat")
            .filter_map(|n| ByteRange::new(n.info.body(), n.info.end()).ok())
            .collect::<Vec<_>>();
        if mdats.is_empty() {
            return Err(BmffError::Unresolved("missing media data"));
        }
        let mut tracks = Vec::new();
        for trak in moov.children.iter().filter(|n| n.info.kind == *b"trak") {
            if tracks.len() >= parser.limits.tracks {
                return Err(BmffError::ResourceLimit);
            }
            let track = parser.track(trak, &mdats)?;
            if tracks
                .iter()
                .any(|old: &Track<LayoutSample>| old.id == track.id)
            {
                return Err(malformed("duplicate track identity"));
            }
            tracks.push(track);
        }
        if tracks.is_empty() {
            return Err(BmffError::Unresolved("missing target tracks"));
        }
        let mut container_boxes = Vec::new();
        flatten_boxes(roots, &mut container_boxes);
        Ok(SampleMap {
            file_size,
            movie_timescale,
            tracks,
            container_boxes,
        })
    })
}

/// Optional donor evidence can qualify a configuration prior only. Target
/// ordinals, placement, timing and sample counts always come from target tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DonorAssessment {
    NotSupplied,
    ConfigurationCompatible,
    ConfigurationRejected,
    UnqualifiedWithoutTarget,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DamageReason {
    MissingTargetMetadata,
    InvalidTargetMetadata,
    AcquisitionMissingMetadata(AcquisitionMissing),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DamageRoute {
    TargetLayout {
        map: SampleMap<LayoutSample>,
        donor: DonorAssessment,
    },
    Unresolved {
        reason: DamageReason,
        donor: DonorAssessment,
    },
}
/// Bounded damage classification; creates no playable artifact or ownership claim.
/// A donor never repairs absent target tables or supplies replacement timing.
pub fn inspect_damage(
    reader: &mut impl ReadAt,
    limits: ParseLimits,
    donor: Option<&SampleMap>,
) -> Result<DamageRoute> {
    match inspect_layout(reader, limits) {
        Ok(map) => {
            let assessment = match donor {
                None => DonorAssessment::NotSupplied,
                Some(prior) => {
                    let compatible = map.tracks.len() == prior.tracks.len()
                        && map.tracks.iter().zip(&prior.tracks).all(|(target, prior)| {
                            target.handler == prior.handler
                                && target.descriptions == prior.descriptions
                        });
                    if compatible {
                        DonorAssessment::ConfigurationCompatible
                    } else {
                        DonorAssessment::ConfigurationRejected
                    }
                }
            };
            Ok(DamageRoute::TargetLayout {
                map,
                donor: assessment,
            })
        }
        Err(error) => {
            let reason = match &error {
                BmffError::Unresolved(_) => DamageReason::MissingTargetMetadata,
                BmffError::Malformed(_) => DamageReason::InvalidTargetMetadata,
                _ => match acquisition_missing(&error) {
                    Some(reason) => DamageReason::AcquisitionMissingMetadata(reason),
                    None => return Err(error),
                },
            };
            Ok(DamageRoute::Unresolved {
                reason,
                donor: if donor.is_some() {
                    DonorAssessment::UnqualifiedWithoutTarget
                } else {
                    DonorAssessment::NotSupplied
                },
            })
        }
    }
}

/// Canonical ordinary/declared-fragmented target route. Ordinary inspection
/// rejects moof before hashing sample payloads; only that explicit profile signal
/// enables fragment parsing, never a generic parse/source failure.
pub fn inspect_target_layout(
    reader: &mut impl ReadAt,
    limits: ParseLimits,
) -> Result<SampleMap<LayoutSample>> {
    match inspect_layout(reader, limits) {
        Err(BmffError::Unsupported("ISO fragmented MP4 is a separate profile")) => {
            crate::fragmented::inspect(reader, limits)?.into_layout()
        }
        result => result,
    }
}
