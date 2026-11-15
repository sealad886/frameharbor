//! Typed evidence boundaries. Decoder success is deliberately independent of identity.
use std::fmt;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContractError {
    InvalidRange,
    Overflow,
    OutOfBounds,
    UnverifiedBinding,
    MissingEvidence,
    InvalidBudget,
    InvalidTransition,
    InvalidDenominator,
}

impl fmt::Display for ContractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for ContractError {}

/// Half-open non-empty byte range. Construction rejects wraparound and reversal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ByteRange {
    start: u64,
    end: u64,
}

impl ByteRange {
    pub fn new(start: u64, end: u64) -> Result<Self, ContractError> {
        if start >= end {
            return Err(ContractError::InvalidRange);
        }
        Ok(Self { start, end })
    }
    pub fn from_len(start: u64, len: u64) -> Result<Self, ContractError> {
        Self::new(
            start,
            start.checked_add(len).ok_or(ContractError::Overflow)?,
        )
    }
    pub fn start(self) -> u64 {
        self.start
    }
    pub fn end(self) -> u64 {
        self.end
    }
    pub fn len(self) -> u64 {
        self.end - self.start
    }
    pub fn is_empty(self) -> bool {
        false
    }
    pub fn contains(self, other: Self) -> bool {
        self.start <= other.start && other.end <= self.end
    }
    pub fn intersection(self, other: Self) -> Option<Self> {
        Self::new(self.start.max(other.start), self.end.min(other.end)).ok()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingEvidence {
    /// The map format itself cannot authenticate an image.
    Unverified,
    /// Explicit acquisition evidence supplied by the operator; not media authentication.
    OperatorAttested,
    /// A trusted acquisition manifest binds the exact image generation and map.
    TrustedManifest,
}

/// Maps one explicit ddrescue input domain into an image. Boot historical offsets
/// are not part of this transform. Piecewise acquisitions use several bindings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoordinateBinding {
    input_domain: ByteRange,
    image_domain: ByteRange,
    evidence: BindingEvidence,
}

impl CoordinateBinding {
    pub fn new(
        input_domain: ByteRange,
        displacement: i128,
        image_size: u64,
        evidence: BindingEvidence,
    ) -> Result<Self, ContractError> {
        let translate = |v: u64| -> Result<u64, ContractError> {
            let n = i128::from(v)
                .checked_add(displacement)
                .ok_or(ContractError::Overflow)?;
            u64::try_from(n).map_err(|_| ContractError::OutOfBounds)
        };
        let image_domain =
            ByteRange::new(translate(input_domain.start)?, translate(input_domain.end)?)?;
        if image_domain.end > image_size {
            return Err(ContractError::OutOfBounds);
        }
        Ok(Self {
            input_domain,
            image_domain,
            evidence,
        })
    }
    pub fn input_domain(&self) -> ByteRange {
        self.input_domain
    }
    pub fn image_domain(&self) -> ByteRange {
        self.image_domain
    }
    pub fn evidence(&self) -> BindingEvidence {
        self.evidence
    }
    pub fn input_to_image(&self, range: ByteRange) -> Result<ByteRange, ContractError> {
        if !self.input_domain.contains(range) {
            return Err(ContractError::OutOfBounds);
        }
        ByteRange::from_len(
            self.image_domain.start + (range.start - self.input_domain.start),
            range.len(),
        )
    }
    pub fn image_to_input(&self, range: ByteRange) -> Result<ByteRange, ContractError> {
        if !self.image_domain.contains(range) {
            return Err(ContractError::OutOfBounds);
        }
        ByteRange::from_len(
            self.input_domain.start + (range.start - self.image_domain.start),
            range.len(),
        )
    }
    pub fn require_supported(&self) -> Result<(), ContractError> {
        if self.evidence == BindingEvidence::Unverified {
            Err(ContractError::UnverifiedBinding)
        } else {
            Ok(())
        }
    }
}

/// Filesystem-local addressing is independent of historical device coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilesystemCoordinates {
    pub start_in_image: u64,
    pub length: u64,
    pub original_partition_start: Option<u64>,
}
impl FilesystemCoordinates {
    pub fn to_image(
        self,
        relative: ByteRange,
        image_size: u64,
    ) -> Result<ByteRange, ContractError> {
        if relative.end > self.length {
            return Err(ContractError::OutOfBounds);
        }
        let start = self
            .start_in_image
            .checked_add(relative.start)
            .ok_or(ContractError::Overflow)?;
        let out = ByteRange::from_len(start, relative.len())?;
        if out.end > image_size {
            return Err(ContractError::OutOfBounds);
        }
        Ok(out)
    }
    pub fn to_original_device(
        self,
        relative: ByteRange,
    ) -> Result<Option<ByteRange>, ContractError> {
        if relative.end > self.length {
            return Err(ContractError::OutOfBounds);
        }
        self.original_partition_start
            .map(|base| {
                ByteRange::from_len(
                    base.checked_add(relative.start)
                        .ok_or(ContractError::Overflow)?,
                    relative.len(),
                )
            })
            .transpose()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcquisitionStatus {
    Finished,
    NonTried,
    NonTrimmed,
    NonScraped,
    BadSector,
    Unknown,
}
impl AcquisitionStatus {
    pub fn from_map_char(c: char) -> Option<Self> {
        match c {
            '+' => Some(Self::Finished),
            '?' => Some(Self::NonTried),
            '*' => Some(Self::NonTrimmed),
            '/' => Some(Self::NonScraped),
            '-' => Some(Self::BadSector),
            _ => None,
        }
    }
    pub fn available(self) -> bool {
        self == Self::Finished
    }
}

/// These identities intentionally cannot be substituted for one another.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourceGeneration(pub String);
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PhysicalGeometry {
    pub source: SourceGeneration,
    pub ordered_spans: Vec<ByteRange>,
}
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ContentClass(pub String);
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LogicalOwner {
    pub source: SourceGeneration,
    pub filesystem: String,
    pub file_identity: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByteProvenance {
    AcquisitionSupported,
    Synthesized,
    Unavailable,
    BindingUnverified,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ownership {
    MetadataSupported,
    SharedGeometry,
    HistoricalConflict,
    Unresolved,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Structure {
    Valid,
    RepairedWithRecordedEdits,
    Invalid,
    Untested,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decode {
    StrictPass,
    Failure,
    Untested,
    TimedOut,
    Unsupported,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Temporal {
    OriginalMappingSupported,
    ConstrainedHypothesis,
    Unresolved,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coverage {
    CompleteWithinKnownLayout { samples: u64 },
    KnownGaps { recovered: u64, listed: u64 },
    UnknownTotal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claims {
    pub bytes: ByteProvenance,
    pub ownership: Ownership,
    pub structure: Structure,
    pub decode: Decode,
    pub temporal: Temporal,
    pub coverage: Coverage,
}

/// Independent comparison of the reopened staged artifact to the asserted sample
/// map, in addition to target evidence. These are separate from decoder status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SampleVerification {
    pub bytes: bool,
    pub identity: bool,
    pub order: bool,
    pub timing: bool,
    pub configuration: bool,
    pub audio: bool,
}

impl Claims {
    pub fn validate(&self) -> Result<(), ContractError> {
        match self.coverage {
            Coverage::CompleteWithinKnownLayout { samples: 0 } => {
                Err(ContractError::InvalidDenominator)
            }
            Coverage::KnownGaps { recovered, listed } if listed == 0 || recovered >= listed => {
                Err(ContractError::InvalidDenominator)
            }
            _ => Ok(()),
        }
    }
    /// Exact describes supported samples, not a complete recording. Shared owners
    /// and historical conflicts cannot become authenticated identities by decoding.
    pub fn require_exact_samples(&self, verified: SampleVerification) -> Result<(), ContractError> {
        self.validate()?;
        if self.bytes != ByteProvenance::AcquisitionSupported
            || self.ownership != Ownership::MetadataSupported
            || !matches!(
                self.structure,
                Structure::Valid | Structure::RepairedWithRecordedEdits
            )
            || self.temporal != Temporal::OriginalMappingSupported
            || !verified.bytes
            || !verified.identity
            || !verified.order
            || !verified.timing
            || !verified.configuration
            || !verified.audio
        {
            return Err(ContractError::MissingEvidence);
        }
        Ok(())
    }
    pub fn require_playable(&self) -> Result<(), ContractError> {
        self.validate()?;
        if self.decode == Decode::StrictPass
            && matches!(
                self.structure,
                Structure::Valid | Structure::RepairedWithRecordedEdits
            )
        {
            Ok(())
        } else {
            Err(ContractError::MissingEvidence)
        }
    }
}

/// An unknown denominator is explicitly unknown, rather than zero or 100%.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Metric {
    pub numerator: u64,
    pub denominator: Option<u64>,
}
impl Metric {
    pub fn ratio(self) -> Result<Option<f64>, ContractError> {
        match self.denominator {
            None => Ok(None),
            Some(0) => Err(ContractError::InvalidDenominator),
            Some(d) if self.numerator > d => Err(ContractError::InvalidDenominator),
            Some(d) => Ok(Some(self.numerator as f64 / d as f64)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalReason {
    CompletedDeclaredScope,
    AcquisitionMissing,
    BindingUnverified,
    Ambiguous,
    Unsupported,
    BudgetExhausted,
    SourceChanged,
    DecoderFailed,
    IoFailed,
    Cancelled,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkState {
    Pending,
    Running,
    Checkpointed,
    Validated,
    Publishing,
    Complete,
    Paused,
    Cancelled,
    Unresolved,
    Failed,
}
impl WorkState {
    pub fn transition(self, next: Self) -> Result<Self, ContractError> {
        use WorkState::*;
        let valid = matches!(
            (self, next),
            (Pending, Running)
                | (
                    Running,
                    Checkpointed | Validated | Paused | Cancelled | Unresolved | Failed
                )
                | (Checkpointed, Running | Paused | Cancelled)
                | (Paused, Running | Cancelled)
                | (Validated, Publishing | Failed)
                | (Publishing, Complete | Failed)
        );
        if valid {
            Ok(next)
        } else {
            Err(ContractError::InvalidTransition)
        }
    }
}

pub const NORMAL_TOTAL_SECONDS: u64 = 24 * 60 * 60;
pub const DEFAULT_HISTORICAL_FRONTIERS: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceCaps {
    /// Conservative admitted source/stage/helper/index byte work. Controller
    /// SQLite pages and frontier metadata have separate structural bounds;
    /// this counter is not a measurement or cap of physical device I/O.
    pub read_bytes: u64,
    pub memory_bytes: u64,
    pub index_bytes: u64,
    pub output_bytes: u64,
    /// Residual candidate-search posting visits. Filesystem metadata geometry
    /// lookups have separate durable parser-domain limits and actual counters.
    pub visited_postings: u64,
    /// Residual hypothesis-graph transitions; not SQLite row mutations.
    pub transitions: u64,
    pub decoder_seconds: u64,
}

/// Cumulative processing time includes validation/export/shutdown. Paused time
/// belongs in a separate counter and never replenishes the processing allowance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Budget {
    total: Duration,
    shutdown_reserve: Duration,
    consumed: Duration,
    caps: ResourceCaps,
    used: ResourceCaps,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resource {
    ReadBytes,
    MemoryBytes,
    IndexBytes,
    OutputBytes,
    VisitedPostings,
    Transitions,
    DecoderSeconds,
}
impl Budget {
    pub fn new(
        total: Duration,
        shutdown_reserve: Duration,
        caps: ResourceCaps,
    ) -> Result<Self, ContractError> {
        if total.is_zero()
            || total > Duration::from_secs(NORMAL_TOTAL_SECONDS)
            || shutdown_reserve >= total
        {
            return Err(ContractError::InvalidBudget);
        }
        Ok(Self {
            total,
            shutdown_reserve,
            consumed: Duration::ZERO,
            caps,
            used: ResourceCaps {
                read_bytes: 0,
                memory_bytes: 0,
                index_bytes: 0,
                output_bytes: 0,
                visited_postings: 0,
                transitions: 0,
                decoder_seconds: 0,
            },
        })
    }
    pub fn elapsed(&self) -> Duration {
        self.consumed
    }
    pub fn remaining(&self) -> Duration {
        self.total.saturating_sub(self.consumed)
    }
    pub fn may_schedule(&self) -> bool {
        self.remaining() > self.shutdown_reserve
    }
    pub fn account_elapsed(&mut self, elapsed: Duration) -> Result<(), ContractError> {
        self.consumed = self
            .consumed
            .checked_add(elapsed)
            .ok_or(ContractError::Overflow)?;
        Ok(())
    }
    /// Authorize work against a fresh cumulative processing-time observation and
    /// its resource limit. The caller includes prior runs, excludes paused time,
    /// and samples its monotonic clock immediately before each bounded operation.
    /// Stale observations cannot replenish time. Elapsed time is retained even
    /// when authorization fails; rejected work never changes resource counters.
    /// This is a scheduling deadline, not an interruption of kernel-blocked I/O.
    pub fn authorize_at(
        &mut self,
        cumulative_elapsed: Duration,
        resource: Resource,
        amount: u64,
    ) -> Result<(), TerminalReason> {
        self.consumed = self.consumed.max(cumulative_elapsed);
        self.charge(resource, amount)
    }

    /// Charge before doing work against the last observed elapsed time. Runtime
    /// operations should use `authorize_at` with a fresh clock observation.
    /// Rejected charges do not change resource counters.
    pub fn charge(&mut self, resource: Resource, amount: u64) -> Result<(), TerminalReason> {
        if !self.may_schedule() {
            return Err(TerminalReason::BudgetExhausted);
        }
        let (used, cap) = match resource {
            Resource::ReadBytes => (&mut self.used.read_bytes, self.caps.read_bytes),
            Resource::MemoryBytes => (&mut self.used.memory_bytes, self.caps.memory_bytes),
            Resource::IndexBytes => (&mut self.used.index_bytes, self.caps.index_bytes),
            Resource::OutputBytes => (&mut self.used.output_bytes, self.caps.output_bytes),
            Resource::VisitedPostings => {
                (&mut self.used.visited_postings, self.caps.visited_postings)
            }
            Resource::Transitions => (&mut self.used.transitions, self.caps.transitions),
            Resource::DecoderSeconds => (&mut self.used.decoder_seconds, self.caps.decoder_seconds),
        };
        let next = used
            .checked_add(amount)
            .filter(|v| *v <= cap)
            .ok_or(TerminalReason::BudgetExhausted)?;
        *used = next;
        Ok(())
    }
    pub fn used(&self) -> ResourceCaps {
        self.used
    }
    pub fn caps(&self) -> ResourceCaps {
        self.caps
    }
}
