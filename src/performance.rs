//! Bounded observations for exposed synthetic pilots. No ETA or representative
//! hardware qualification follows from a tiny fixture or sparse image.
use crate::{
    jobs::JobManifest,
    publication::ArtifactKind,
    runner::{RunReport, Runner, RunnerError},
};
use std::{
    collections::HashSet,
    fs, io,
    path::Path,
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};
#[derive(Debug)]
pub enum MeasurementError {
    Runner(RunnerError),
    Io(io::Error),
    UnsupportedHost,
    ObservationLimit,
}
impl From<RunnerError> for MeasurementError {
    fn from(e: RunnerError) -> Self {
        Self::Runner(e)
    }
}
impl From<io::Error> for MeasurementError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}
impl std::fmt::Display for MeasurementError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for MeasurementError {}
#[derive(Debug)]
pub struct PilotMeasurement {
    pub report: RunReport,
    /// Includes Runner intake, processing and bounded allocation observation.
    /// Caller configuration/helper-pin preparation is outside this interval.
    pub wall: Duration,
    pub source_logical_bytes: u64,
    pub source_allocated_bytes: u64,
    pub case_and_output_allocated_bytes: u64,
    pub published_logical_bytes: u64,
    /// Decoder helper peak from committed validation receipts; excludes the
    /// filesystem helper and parent RSS, and is not an aggregate memory bound.
    pub observed_decoder_max_rss_bytes: u64,
}
/// Start only on caller-authorized inputs. This API does not authorize a real
/// recovery run; development tests invoke it solely on owned synthetic images.
pub fn measure_new(
    manifest: JobManifest,
    cancel: &AtomicBool,
) -> Result<PilotMeasurement, MeasurementError> {
    let started = Instant::now();
    let source = fs::metadata(&manifest.image)?;
    let source_logical_bytes = source.len();
    let source_allocated_bytes = allocated(&source)?;
    let case = manifest.case_dir.clone();
    let output = manifest.output_dir.clone();
    let mut runner = Runner::create_started(manifest, started)?;
    let report = runner.run(cancel)?;
    let case_and_output_allocated_bytes =
        observe_allocation(&[case.as_path(), output.as_path()], 10000)?;
    let mut published_logical_bytes = 0u64;
    let mut observed_decoder_max_rss_bytes = 0;
    for artifact in &report.artifacts {
        published_logical_bytes = published_logical_bytes
            .checked_add(artifact.bytes)
            .ok_or(MeasurementError::ObservationLimit)?;
        if let ArtifactKind::ValidatedMedia(e) = &artifact.kind {
            observed_decoder_max_rss_bytes =
                observed_decoder_max_rss_bytes.max(e.receipt.max_rss_bytes);
        }
    }
    Ok(PilotMeasurement {
        report,
        wall: started.elapsed(),
        source_logical_bytes,
        source_allocated_bytes,
        case_and_output_allocated_bytes,
        published_logical_bytes,
        observed_decoder_max_rss_bytes,
    })
}
fn allocated(metadata: &fs::Metadata) -> Result<u64, MeasurementError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        metadata
            .blocks()
            .checked_mul(512)
            .ok_or(MeasurementError::ObservationLimit)
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        Err(MeasurementError::UnsupportedHost)
    }
}
/// Read-only metadata walk; hard links and overlapping roots count once. Bounds
/// include directories. Symlinks are rejected rather than followed; .DS_Store is
/// ignored and preserved. Allocation observations are not free-space reservations.
pub fn observe_allocation(
    roots: &[&Path],
    maximum_entries: usize,
) -> Result<u64, MeasurementError> {
    if maximum_entries == 0 || maximum_entries > 100000 || roots.len() > 16 {
        return Err(MeasurementError::ObservationLimit);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let mut pending = roots.iter().map(|p| p.to_path_buf()).collect::<Vec<_>>();
        let mut seen = HashSet::new();
        let mut entries = 0usize;
        let mut bytes = 0u64;
        while let Some(path) = pending.pop() {
            if path.file_name().is_some_and(|s| s == ".DS_Store") {
                continue;
            }
            let m = fs::symlink_metadata(&path)?;
            if m.file_type().is_symlink() {
                return Err(MeasurementError::ObservationLimit);
            }
            if !seen.insert((m.dev(), m.ino())) {
                continue;
            }
            entries += 1;
            if entries > maximum_entries {
                return Err(MeasurementError::ObservationLimit);
            }
            bytes = bytes
                .checked_add(allocated(&m)?)
                .ok_or(MeasurementError::ObservationLimit)?;
            if m.is_dir() {
                for child in fs::read_dir(&path)? {
                    if pending.len() + entries >= maximum_entries {
                        return Err(MeasurementError::ObservationLimit);
                    }
                    pending.push(child?.path());
                }
            } else if !m.is_file() {
                return Err(MeasurementError::ObservationLimit);
            }
        }
        Ok(bytes)
    }
    #[cfg(not(unix))]
    {
        let _ = roots;
        Err(MeasurementError::UnsupportedHost)
    }
}
