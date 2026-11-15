//! Canonical bounded controller for the selected unchanged-original media path.
//! A complete discovery index is adopted on restart; provisional records never
//! authorize reads or exports. Unsupported media stays an explicit remainder.
use crate::{
    acquisition::ReadLimits,
    bmff::{self, BmffError, LogicalReader, ParseLimits},
    checkpoint::{CheckpointLimits, CheckpointStore},
    contracts::{ResourceCaps, SourceGeneration},
    control::{CaseControl, CaseSnapshot, ControlError},
    discovery::{DiscoveryError, DiscoveryLimits, DiscoveryQueue},
    filesystem::{self, AdapterLimits, ExtentEvidence},
    jobs::{JobError, JobManifest},
    ledger::{Ledger, LedgerError, MEDIA_OUTCOME, PORTABLE_ARTIFACT},
    publication::{
        Artifact, ArtifactKind, FaultPoint, PublicationError, PublicationLimits, Publisher,
    },
    session::{CaseSession, CpuBatch, SessionError, SessionInput},
    validation::{self, ArtifactTransform, StagingArea, ValidationError, ValidationLimits},
};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::{Instant, SystemTime, UNIX_EPOCH},
};
const OWNER: &[u8] = b"FrameHarbor runner case v1\n";
#[derive(Debug)]
pub enum RunnerError {
    Io(io::Error),
    Job(JobError),
    Ledger(LedgerError),
    Control(ControlError),
    Session(SessionError),
    Publication(PublicationError),
    Validation(ValidationError),
    Media(BmffError),
    Filesystem(filesystem::FilesystemError),
    Discovery(DiscoveryError),
    Invalid,
    Unowned,
}
macro_rules! from_error {
    ($t:ty,$v:ident) => {
        impl From<$t> for RunnerError {
            fn from(e: $t) -> Self {
                Self::$v(e)
            }
        }
    };
}
from_error!(io::Error, Io);
from_error!(JobError, Job);
from_error!(LedgerError, Ledger);
from_error!(ControlError, Control);
from_error!(SessionError, Session);
from_error!(PublicationError, Publication);
from_error!(ValidationError, Validation);
from_error!(BmffError, Media);
from_error!(filesystem::FilesystemError, Filesystem);
from_error!(DiscoveryError, Discovery);
impl std::fmt::Display for RunnerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for RunnerError {}
pub type Result<T> = std::result::Result<T, RunnerError>;
#[derive(Debug, Clone)]
pub struct RunReport {
    pub source_valid: bool,
    pub payloads_reverified: bool,
    /// Actual image payload reads in this live session, including resume verification.
    /// Read-only stored inspection has no live counter.
    pub image_read_bytes_this_run: Option<u64>,
    pub source: SourceGeneration,
    pub snapshot: CaseSnapshot,
    pub remaining_millis: u64,
    pub discovery_complete: bool,
    pub discovery_scope: &'static str,
    pub excluded_virtual_files: u64,
    pub excluded_virtual_directories: u64,
    pub deferred_discovery_entries: u64,
    pub pending_media: u64,
    pub unresolved_media: u64,
    pub unresolved_examples_truncated: bool,
    pub provisional_views: usize,
    pub qualified_views: usize,
    pub artifacts: Vec<Artifact>,
    pub unresolved: Vec<String>,
}
pub struct Runner {
    manifest: JobManifest,
    source: SourceGeneration,
    session: Option<CaseSession>,
}
fn source(ledger: &Ledger) -> Result<SourceGeneration> {
    let s: String = ledger
        .connection
        .query_row("SELECT source FROM case_control LIMIT 1", [], |r| r.get(0))
        .map_err(LedgerError::from)?;
    Ok(SourceGeneration(s))
}
fn now() -> Result<u64> {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| RunnerError::Invalid)?
            .as_millis(),
    )
    .map_err(|_| RunnerError::Invalid)
}
fn owned(case: &Path) -> Result<()> {
    if fs::canonicalize(case)? != case || !fs::symlink_metadata(case)?.is_dir() {
        return Err(RunnerError::Unowned);
    }
    let marker = case.join("OWNER");
    if !fs::symlink_metadata(&marker)?.is_file() {
        return Err(RunnerError::Unowned);
    }
    let mut b = Vec::new();
    File::open(marker)?
        .take(OWNER.len() as u64 + 1)
        .read_to_end(&mut b)?;
    if b != OWNER {
        return Err(RunnerError::Unowned);
    }
    Ok(())
}
fn input(m: &JobManifest) -> SessionInput {
    SessionInput {
        image: m.image.clone(),
        map: m.map.clone(),
        binding: m.binding.clone(),
        limits: ReadLimits {
            per_read: m.read_limits.per_read,
            total: m.read_limits.total,
            receipts: m.read_limits.receipts,
        },
    }
}
fn publication_limits(m: &JobManifest) -> PublicationLimits {
    PublicationLimits {
        artifact_bytes: 64 * 1024 * 1024,
        output_bytes: m.case_config.caps.output_bytes,
        verification_bytes: m.case_config.caps.read_bytes,
        database_bytes: m.ledger_bytes,
        artifacts: 1024,
    }
}
fn ledger(m: &JobManifest) -> Result<Ledger> {
    Ok(Ledger::open(
        &m.case_dir.join("case.sqlite"),
        m.ledger_bytes,
    )?)
}
impl Runner {
    pub fn create(manifest: JobManifest) -> Result<Self> {
        Self::create_started(manifest, Instant::now())
    }
    /// Include caller intake/preflight in the same processing allowance.
    /// A future start instant is rejected by the session clock.
    pub fn create_started(manifest: JobManifest, started: Instant) -> Result<Self> {
        Instant::now()
            .checked_duration_since(started)
            .ok_or(RunnerError::Invalid)?;
        manifest.encode()?;
        // Fresh case ownership is exclusive; never adopt an existing directory.
        let parent = manifest.case_dir.parent().ok_or(RunnerError::Invalid)?;
        if fs::canonicalize(parent)? != parent || manifest.case_dir == manifest.output_dir {
            return Err(RunnerError::Unowned);
        }
        fs::create_dir(&manifest.case_dir)?;
        let mut marker = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(manifest.case_dir.join("OWNER"))?;
        marker.write_all(OWNER)?;
        marker.sync_all()?;
        File::open(&manifest.case_dir)?.sync_all()?;
        File::open(parent)?.sync_all()?;
        let mut l = ledger(&manifest)?;
        manifest.persist(&mut l)?;
        let mut session =
            CaseSession::create_started(l, input(&manifest), manifest.case_config, started)?;
        session.admit_work(
            "helper-pins",
            ResourceCaps {
                read_bytes: 32 * 1024 * 1024,
                memory_bytes: 65536,
                index_bytes: 0,
                output_bytes: 0,
                visited_postings: 0,
                transitions: 0,
                decoder_seconds: 0,
            },
        )?;
        manifest.verify_helpers()?;
        session.check_source()?;
        let mut checkpoints = CheckpointStore::create(
            &manifest.case_dir.join("frontiers"),
            checkpoint_limits(&manifest),
        )?;
        Publisher::create(&manifest.output_dir, publication_limits(&manifest))?;
        session.checkpoint(&mut checkpoints)?;
        let source = session.source_generation();
        Ok(Self {
            manifest,
            source,
            session: Some(session),
        })
    }
    pub fn open(case_dir: &Path) -> Result<Self> {
        owned(case_dir)?;
        let manifest = JobManifest::load_from_case(case_dir)?;
        if manifest.case_dir != case_dir {
            return Err(RunnerError::Unowned);
        }
        let source = source(&Ledger::open_readonly(
            &manifest.case_dir.join("case.sqlite"),
            manifest.ledger_bytes,
        )?)?;
        Ok(Self {
            manifest,
            source,
            session: None,
        })
    }
    pub fn manifest(&self) -> &JobManifest {
        &self.manifest
    }
    pub fn inspect(case_dir: &Path) -> Result<RunReport> {
        let runner = Self::open(case_dir)?;
        runner.report()
    }
    fn report(&self) -> Result<RunReport> {
        let l = Ledger::open_readonly(
            &self.manifest.case_dir.join("case.sqlite"),
            self.manifest.ledger_bytes,
        )?;
        let control = CaseControl::adopt(l, self.source.clone())?;
        let snapshot = control.snapshot()?;
        let l = control.into_ledger();
        let spec = self.manifest.discovery_spec(self.source.clone())?;
        let source_valid: bool = l
            .connection
            .query_row(
                "SELECT valid FROM sources WHERE id=?1",
                [&self.source.0],
                |r| r.get(0),
            )
            .map_err(LedgerError::from)?;
        let queue = DiscoveryQueue::open_readonly(
            &self.manifest.case_dir,
            &l,
            &spec,
            self.manifest.profile,
            self.manifest.fs_start,
            discovery_limits(&self.manifest),
        )?;
        let mut complete = false;
        let mut total = 0;
        let mut excluded_virtual_files = 0;
        let mut excluded_virtual_directories = 0;
        let mut deferred_discovery_entries = 0;
        let mut pending_media = 0;
        let mut unresolved_media = 0;
        let mut unresolved=vec!["Virtual filesystem and orphan namespaces are excluded from allocated-reachable discovery".to_owned()];
        if let Some(queue) = queue {
            let status = queue.status(&l)?;
            complete = status.complete;
            total = status.views;
            excluded_virtual_files = status.excluded_virtual_files;
            excluded_virtual_directories = status.excluded_virtual_directories;
            deferred_discovery_entries = status.deferred_entries;
            let progress = queue.media_progress(&l, 128)?;
            pending_media = progress.pending;
            unresolved_media = progress.unresolved;
            unresolved.extend(progress.examples);
        }
        if !source_valid {
            unresolved.push("Source generation invalidated; indexed views cannot authorize new reads or exports".into());
        }
        if !complete {
            unresolved.push(format!("Selected filesystem namespace incomplete; {deferred_discovery_entries} deferred entries; provisional geometry cannot authorize media output"));
        }
        if pending_media > 0 {
            unresolved.push(format!(
                "{pending_media} file views await media evaluation/export"
            ));
        }
        let unresolved_examples_truncated = unresolved_media > 128;
        let total = usize::try_from(total).map_err(|_| RunnerError::Invalid)?;

        let artifacts = Publisher::open_readonly(
            &self.manifest.output_dir,
            publication_limits(&self.manifest),
        )?
        .media_receipts()?;
        Ok(RunReport {
            source_valid,
            payloads_reverified: false,
            image_read_bytes_this_run: None,
            source: self.source.clone(),
            remaining_millis: self
                .manifest
                .case_config
                .total_millis
                .saturating_sub(snapshot.elapsed_millis),
            snapshot,
            discovery_complete: complete,
            discovery_scope: filesystem::NAMESPACE_POLICY,
            excluded_virtual_files,
            excluded_virtual_directories,
            deferred_discovery_entries,
            pending_media,
            unresolved_media,
            unresolved_examples_truncated,
            provisional_views: if complete { 0 } else { total },
            qualified_views: if complete { total } else { 0 },
            artifacts,
            unresolved,
        })
    }
    /// Ordinary external stop requests update the same durable authority. A live
    /// runner observes this at its next guard; it owns checkpoint/child cleanup.
    pub fn pause(case_dir: &Path) -> Result<RunReport> {
        Self::stop(case_dir, false)
    }
    pub fn cancel(case_dir: &Path) -> Result<RunReport> {
        Self::stop(case_dir, true)
    }
    fn stop(case_dir: &Path, cancel: bool) -> Result<RunReport> {
        let r = Self::open(case_dir)?;
        let mut c = CaseControl::adopt(ledger(&r.manifest)?, r.source.clone())?;
        if cancel {
            c.cancel_at(now()?)?;
        } else {
            c.pause_at(now()?)?;
        }
        drop(c);
        r.report()
    }
    pub fn run(&mut self, cancel: &AtomicBool) -> Result<RunReport> {
        self.run_started(cancel, Instant::now())
    }
    /// Include resumed caller intake before opening durable checkpoint history.
    /// An already-active session retains its original processing clock.
    pub fn run_started(&mut self, cancel: &AtomicBool, started: Instant) -> Result<RunReport> {
        Instant::now()
            .checked_duration_since(started)
            .ok_or(RunnerError::Invalid)?;
        let mut checkpoints = CheckpointStore::open(
            &self.manifest.case_dir.join("frontiers"),
            checkpoint_limits(&self.manifest),
        )?;
        if self.session.is_none() {
            self.session = Some(CaseSession::resume_started(
                ledger(&self.manifest)?,
                input(&self.manifest),
                self.source.clone(),
                &checkpoints,
                started,
            )?);
        }
        let result = (|| {
            let session = self.session.as_mut().ok_or(RunnerError::Invalid)?;
            session.admit_work(
                "helper-pins",
                ResourceCaps {
                    read_bytes: 32 * 1024 * 1024,
                    memory_bytes: 65536,
                    index_bytes: 0,
                    output_bytes: 0,
                    visited_postings: 0,
                    transitions: 0,
                    decoder_seconds: 0,
                },
            )?;
            self.manifest.verify_helpers()?;
            session.check_source()?;
            self.run_inner(cancel, &mut checkpoints)?;
            let bound = fs::metadata(self.manifest.case_dir.join("case.sqlite"))?
                .len()
                .saturating_mul(2)
                .saturating_add(
                    fs::metadata(self.manifest.output_dir.join("publication.sqlite"))?
                        .len()
                        .saturating_mul(2),
                );
            self.session
                .as_mut()
                .ok_or(RunnerError::Invalid)?
                .admit_work(
                    "report",
                    ResourceCaps {
                        read_bytes: bound,
                        memory_bytes: bound,
                        index_bytes: 0,
                        output_bytes: 0,
                        visited_postings: 0,
                        transitions: 0,
                        decoder_seconds: 0,
                    },
                )?;
            let mut report = self.report()?;
            report.artifacts = Publisher::open(
                &self.manifest.output_dir,
                publication_limits(&self.manifest),
            )?
            .published_media_with_session(self.session.as_mut().ok_or(RunnerError::Invalid)?)?;
            report.payloads_reverified = true;
            Ok::<_, RunnerError>(report)
        })();
        let session = self.session.as_mut().ok_or(RunnerError::Invalid)?;
        let stopped = if cancel.load(Ordering::Relaxed) {
            session.cancel(&mut checkpoints)
        } else {
            session.pause(&mut checkpoints)
        };
        let actual_reads = session.image_bytes_read_this_run();
        let snapshot = session.snapshot();
        self.session.take();
        let mut report = result?;
        stopped?;
        report.snapshot = snapshot?;
        report.image_read_bytes_this_run = Some(actual_reads);
        report.remaining_millis = self
            .manifest
            .case_config
            .total_millis
            .saturating_sub(report.snapshot.elapsed_millis);
        Ok(report)
    }
    fn run_inner(&mut self, cancel: &AtomicBool, checkpoints: &mut CheckpointStore) -> Result<()> {
        let session = self.session.as_mut().ok_or(RunnerError::Invalid)?;
        let limits = discovery_limits(&self.manifest);
        let spec = self.manifest.discovery_spec(self.source.clone())?;
        let existing = session.with_ledger_resources(state_read(65536), |l| {
            Ok(DiscoveryQueue::open_readonly(
                &self.manifest.case_dir,
                l,
                &spec,
                self.manifest.profile,
                self.manifest.fs_start,
                limits,
            ))
        })??;
        let queue = match existing {
            Some(queue) => queue,
            None => session.with_ledger(32768, |l| {
                Ok(DiscoveryQueue::open(
                    &self.manifest.case_dir,
                    l,
                    &spec,
                    self.manifest.profile,
                    self.manifest.fs_start,
                    limits,
                ))
            })??,
        };
        loop {
            let cursor =
                session.with_ledger_resources(state_read(65536), |l| Ok(queue.next(l)))??;
            let Some(cursor) = cursor else { break };
            let batch = filesystem::inspect_directory_batch(
                session,
                &self.manifest.filesystem_helper.path,
                session.image_size(),
                self.manifest.fs_start,
                self.manifest.profile,
                cursor,
                filesystem::DirectoryBatchLimits {
                    adapter: AdapterLimits {
                        files: limits.entries_per_batch,
                        runs: limits.runs_per_batch,
                        ..AdapterLimits::default()
                    },
                    entries: limits.entries_per_batch,
                },
                cancel,
                |s, event| {
                    if let filesystem::DirectoryEvent::BeforeRead { delivered, .. } = event {
                        s.prepare_discovery_read(delivered, checkpoints)?;
                    }
                    Ok(())
                },
            )?;
            let bound = queue.write_bound(&batch)?;
            let proof = session.seal_discovery_batch(checkpoints, batch.digest())?;
            session
                .with_ledger_resources(bound, |l| Ok(queue.commit_batch(l, &batch, proof)))??;
        }
        let catalog = session
            .with_ledger_resources(state_read(65536), |l| Ok(queue.catalog_read_bound(l)))??;
        session.with_ledger_resources(catalog, |l| Ok(queue.verify_catalog(l)))??;
        let complete = session.with_ledger_resources(state_read(4096), |l| {
            Ok(queue.status(l).map(|s| s.complete))
        })??;
        let complete = if complete {
            true
        } else {
            session.with_ledger(8192, |l| Ok(queue.finish(l)))??
        };
        if !complete {
            return Ok(());
        }
        let mut publisher = Publisher::open(
            &self.manifest.output_dir,
            publication_limits(&self.manifest),
        )?;
        publisher.reconcile_media(session)?;
        let mut outputs = publisher.published_media_with_session(session)?;
        let mut after: Option<String> = None;
        loop {
            let bound = session.with_ledger_resources(state_read(65536), |l| {
                Ok(queue.pending_page_read_bound(l, after.as_deref(), limits.page_size))
            })??;
            let views = session.with_ledger_resources(bound, |l| {
                Ok(queue.pending_qualified_page(l, after.as_deref(), limits.page_size))
            })??;
            if views.is_empty() {
                break;
            }
            let jobs = views
                .iter()
                .map(|v| Ok((v.id.clone(), v.media_spec()?)))
                .collect::<Result<Vec<_>>>()?;
            let references = jobs
                .iter()
                .map(|(id, spec)| (id.as_str(), spec))
                .collect::<Vec<_>>();
            let ids = session.with_ledger_resources(
                ResourceCaps {
                    read_bytes: 65536,
                    memory_bytes: bound.memory_bytes.saturating_add(jobs.len() as u64 * 2048),
                    index_bytes: 16384 + jobs.len() as u64 * 2048,
                    output_bytes: 0,
                    visited_postings: 0,
                    transitions: 0,
                    decoder_seconds: 0,
                },
                |l| Ok(queue.bind_media_work_batch(l, &references)),
            )??;
            if ids.len() != views.len() {
                return Err(RunnerError::Invalid);
            }
            drop(references);
            drop(jobs);
            let probe_memory = ParseLimits::default()
                .memory_bytes
                .saturating_add(bound.memory_bytes)
                .saturating_add(3 * 1024 * 1024)
                .saturating_add(128 * 4096);
            let threshold = self.manifest.read_limits.receipts.min(768);
            let mut cpu = None;
            let mut pending = Vec::new();
            for (indexed, work) in views.into_iter().zip(ids) {
                after = Some(indexed.id.clone());
                if cancel.load(Ordering::Relaxed) {
                    return Err(filesystem::FilesystemError::Cancelled.into());
                }
                session.check_source()?;
                let view = &indexed.view;
                if let Some(artifact) = outputs.iter().find(|artifact| {
                    matches!(&artifact.kind, ArtifactKind::ValidatedMedia(e)
                        if e.owner == view.owner
                            && matches!(e.receipt.transform, ArtifactTransform::OriginalFile))
                }) {
                    finish_probe_cpu(session, &mut cpu)?;
                    flush_probe_outcomes(session, checkpoints, &mut pending, threshold, true)?;
                    portable_reference(session, &work, &artifact.id)?;
                    outcome(
                        session,
                        &work,
                        &format!("exported {}", artifact.id),
                        checkpoints,
                    )?;
                    continue;
                }
                if view.evidence != ExtentEvidence::MetadataAllocated
                    || view.size == 0
                    || view.size > 64 * 1024 * 1024
                {
                    let qualification = session
                        .with_ledger_resources(state_read(16384), |l| {
                            Ok(queue.qualification(l, &indexed.id))
                        })??;
                    pending.push((work,format!("{}: unsupported ownership/size {:?} ({} bytes); {} allocation runs not checked, negative allocation={}, shared geometry={}",view.owner.file_identity,view.evidence,view.size,qualification.unchecked_runs.len(),qualification.observed_allocation_negative,qualification.shared_geometry)));
                    continue;
                }
                if session.live_dependency_count() >= threshold {
                    finish_probe_cpu(session, &mut cpu)?;
                    flush_probe_outcomes(session, checkpoints, &mut pending, threshold, true)?;
                    session.archive_dependencies(checkpoints)?;
                }
                if cpu.is_none() {
                    cpu = Some(session.begin_cpu_batch("media-probe-batch", probe_memory, 128)?);
                }
                let epoch = session.begin_read_epoch()?;
                let layout = match bmff::inspect_target_layout(
                    &mut LogicalReader::in_cpu_batch(
                        session,
                        view,
                        cpu.as_mut().ok_or(RunnerError::Invalid)?,
                    )?,
                    ParseLimits::default(),
                ) {
                    Ok(map) => map,
                    Err(
                        e @ (BmffError::Malformed(_)
                        | BmffError::Unsupported(_)
                        | BmffError::Unresolved(_)
                        | BmffError::ResourceLimit),
                    ) => {
                        session.check_source()?;
                        session.retain_read_epoch(epoch)?;
                        pending.push((work, format!("{}: {e}", view.owner.file_identity)));
                        continue;
                    }
                    Err(error) if bmff::acquisition_missing(&error).is_some() => {
                        let missing = bmff::acquisition_missing(&error).unwrap();
                        session.retain_read_epoch(epoch)?;
                        pending.push((work,format!("{}: target metadata acquisition missing at image {:?}; sample mapping unresolved",view.owner.file_identity,missing.image)));
                        continue;
                    }
                    Err(e) => return Err(e.into()),
                };
                finish_probe_cpu(session, &mut cpu)?;
                flush_probe_outcomes(session, checkpoints, &mut pending, threshold, false)?;
                let needs_segment = layout.container_boxes.iter().any(|b| b.kind == *b"moof")
                    || layout.tracks.iter().flat_map(|t| &t.samples).any(|s| {
                        matches!(s.availability, bmff::SampleAvailability::Unavailable { .. })
                    });
                if needs_segment {
                    let (text, latest) = recover_segments(
                        &self.manifest,
                        session,
                        view,
                        &layout,
                        &work,
                        &mut publisher,
                        &mut outputs,
                        cancel,
                    )?;
                    outcome(session, &work, &text, checkpoints)?;
                    if let Some(artifact) = latest {
                        publisher.retire_source_epoch(session, epoch, &artifact, &work)?;
                        session.checkpoint(checkpoints)?;
                    } else {
                        session.retain_read_epoch(epoch)?;
                    }
                    continue;
                }
                let map = layout.into_exact()?;
                let stage_root = self.manifest.case_dir.join(format!("stage-{work}"));
                // Preserve failed stages. Repeated attempts consume distinct bounded
                // directories and shared cumulative output admissions.
                let mut attempt = 0;
                let root = loop {
                    let p = stage_root.with_file_name(format!("stage-{work}-{attempt}"));
                    if !p.exists() {
                        break p;
                    }
                    attempt += 1;
                    if attempt >= 1024 {
                        return Err(RunnerError::Invalid);
                    }
                };
                let mut area =
                    StagingArea::create(&root, self.manifest.case_config.caps.output_bytes, 1024)?;
                let staged = area.stage_exact(session, view, map, ValidationLimits::default())?;
                let validated = match validation::validate(
                    session,
                    staged,
                    &self.manifest.media_helper.path,
                    ValidationLimits::default(),
                    cancel,
                ) {
                    Ok(v) => v,
                    Err(
                        e @ (ValidationError::Unsupported
                        | ValidationError::SampleMismatch(_)
                        | ValidationError::NativeError { .. }),
                    ) => {
                        session.check_source()?;
                        outcome(
                            session,
                            &work,
                            &format!(
                                "{}: strict validation rejected {e}",
                                view.owner.file_identity
                            ),
                            checkpoints,
                        )?;
                        session.retain_read_epoch(epoch)?;
                        continue;
                    }
                    Err(e) => return Err(e.into()),
                };
                let artifact = publisher.publish_media(session, &validated, FaultPoint::None)?;
                portable_reference(session, &work, &artifact.id)?;
                outcome(
                    session,
                    &work,
                    &format!("exported {}", artifact.id),
                    checkpoints,
                )?;
                publisher.retire_source_epoch(session, epoch, &artifact, &work)?;
                session.checkpoint(checkpoints)?;
                outputs.push(artifact);
            }
            finish_probe_cpu(session, &mut cpu)?;
            flush_probe_outcomes(session, checkpoints, &mut pending, threshold, true)?;
        }
        Ok(())
    }
}
fn outcome(
    session: &mut CaseSession,
    work: &str,
    text: &str,
    checkpoints: &mut CheckpointStore,
) -> Result<()> {
    session.checkpoint(checkpoints)?;
    let write = (text.len() as u64).saturating_mul(2).saturating_add(4096);
    session.with_ledger(write, |l| {
        l.append_receipt(work, MEDIA_OUTCOME, text.as_bytes())
    })?;
    Ok(())
}

fn state_read(bytes: u64) -> ResourceCaps {
    ResourceCaps {
        read_bytes: bytes,
        memory_bytes: bytes.saturating_mul(3),
        index_bytes: 0,
        output_bytes: 0,
        visited_postings: 0,
        transitions: 0,
        decoder_seconds: 0,
    }
}

#[allow(clippy::too_many_arguments)]
fn recover_segments(
    manifest: &JobManifest,
    session: &mut CaseSession,
    view: &filesystem::LogicalFileView,
    layout: &bmff::SampleMap<bmff::LayoutSample>,
    work: &str,
    publisher: &mut Publisher,
    outputs: &mut Vec<Artifact>,
    cancel: &AtomicBool,
) -> Result<(String, Option<Artifact>)> {
    use crate::segments::{self, SegmentLimits};
    let plans = match segments::plan(
        &mut LogicalReader::new(session, view),
        layout,
        SegmentLimits::default(),
    ) {
        Ok(plans) => plans,
        Err(
            e @ (BmffError::Malformed(_) | BmffError::Unsupported(_) | BmffError::Unresolved(_)),
        ) => {
            return Ok((
                format!(
                    "{}: segment planning unresolved {e}",
                    view.owner.file_identity
                ),
                None,
            ));
        }
        Err(e) => return Err(e.into()),
    };
    let mut accepted = std::collections::BTreeSet::new();
    let mut rejected = 0;
    let mut latest = None;
    for plan in plans.plans {
        session.check_source()?;
        if cancel.load(Ordering::Relaxed) {
            return Err(filesystem::FilesystemError::Cancelled.into());
        }
        if let Some(artifact) = outputs.iter().find(|a| segment_matches(a, &plan, view)) {
            portable_reference(session, work, &artifact.id)?;
            latest = Some(artifact.clone());
            if let ArtifactKind::ValidatedMedia(e) = &artifact.kind
                && let ArtifactTransform::SamplePreserving(t) = &e.receipt.transform
            {
                for origin in &t.mappings {
                    accepted.insert((origin.track_id, origin.original.ordinal));
                }
            }
            continue;
        }
        let mut attempt = 0;
        let root = loop {
            let p = manifest
                .case_dir
                .join(format!("stage-{work}-segment-{attempt}"));
            if !p.exists() {
                break p;
            }
            attempt += 1;
            if attempt >= 1024 {
                return Err(RunnerError::Invalid);
            }
        };
        let mut area = StagingArea::create(&root, manifest.case_config.caps.output_bytes, 1024)?;
        let staged = area.stage_segment(session, view, &plan, ValidationLimits::default())?;
        let validated = match validation::validate(
            session,
            staged,
            &manifest.media_helper.path,
            ValidationLimits::default(),
            cancel,
        ) {
            Ok(v) => v,
            Err(
                ValidationError::Unsupported
                | ValidationError::SampleMismatch(_)
                | ValidationError::NativeError { .. },
            ) => {
                session.check_source()?;
                rejected += 1;
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        let artifact = publisher.publish_media(session, &validated, FaultPoint::None)?;
        if let ArtifactKind::ValidatedMedia(e) = &artifact.kind
            && let ArtifactTransform::SamplePreserving(t) = &e.receipt.transform
        {
            for origin in &t.mappings {
                accepted.insert((origin.track_id, origin.original.ordinal));
            }
        }
        portable_reference(session, work, &artifact.id)?;
        latest = Some(artifact.clone());
        outputs.push(artifact);
    }
    let coverage = layout
        .tracks
        .iter()
        .map(|track| {
            format!(
                "track {}: {}/{} source samples exported",
                track.id,
                accepted.iter().filter(|(id, _)| *id == track.id).count(),
                track.samples.len()
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    Ok((
        format!(
            "{}: sample-preserving segments; synthesized headers and constrained presentation; {coverage}; {rejected} rejected segments; original-file completeness unestablished",
            view.owner.file_identity
        ),
        latest,
    ))
}
fn segment_matches(
    artifact: &Artifact,
    plan: &crate::segments::TransformationPlan,
    view: &filesystem::LogicalFileView,
) -> bool {
    let ArtifactKind::ValidatedMedia(e) = &artifact.kind else {
        return false;
    };
    let ArtifactTransform::SamplePreserving(t) = &e.receipt.transform else {
        return false;
    };
    if e.owner != view.owner
        || t.labels != plan.labels()
        || t.source_counts != plan.source_counts()
        || t.mappings.len()
            != plan
                .selected_source()
                .tracks
                .iter()
                .map(|track| track.samples.len())
                .sum::<usize>()
    {
        return false;
    }
    plan.selected_source()
        .tracks
        .iter()
        .zip(&plan.expected_output().tracks)
        .flat_map(|(track, out)| {
            track
                .samples
                .iter()
                .zip(&out.samples)
                .map(move |(original, output)| (track.id, track.timescale, original, output))
        })
        .zip(&t.mappings)
        .all(|((id, timescale, original, output), m)| {
            m.track_id == id
                && m.timescale == timescale
                && m.original == *original
                && m.output_ordinal == output.ordinal
                && m.output_dts == output.dts
                && m.output_pts == output.pts
        })
}

fn portable_reference(session: &mut CaseSession, work: &str, artifact: &str) -> Result<()> {
    session.with_ledger(8192, |l| {
        let refs = l.receipts(work, PORTABLE_ARTIFACT, 64)?;
        if !refs.iter().any(|r| r == artifact.as_bytes()) {
            l.append_receipt(work, PORTABLE_ARTIFACT, artifact.as_bytes())?;
        }
        Ok(())
    })?;
    Ok(())
}

fn discovery_limits(manifest: &JobManifest) -> DiscoveryLimits {
    DiscoveryLimits {
        index_bytes: manifest.index_bytes,
        ..DiscoveryLimits::default()
    }
}

fn checkpoint_limits(manifest: &JobManifest) -> CheckpointLimits {
    CheckpointLimits {
        historical_manifests: manifest.checkpoint_history,
        ..CheckpointLimits::default()
    }
}

fn finish_probe_cpu(session: &mut CaseSession, batch: &mut Option<CpuBatch>) -> Result<()> {
    if let Some(batch) = batch.take() {
        session.finish_cpu_batch(batch)?;
    }
    Ok(())
}
fn flush_probe_outcomes(
    session: &mut CaseSession,
    store: &mut CheckpointStore,
    pending: &mut Vec<(String, String)>,
    threshold: usize,
    archive_allowed: bool,
) -> Result<()> {
    if pending.is_empty() {
        return Ok(());
    }
    if pending.len() > 128 {
        return Err(RunnerError::Invalid);
    }
    session.check_source()?;
    if archive_allowed && session.live_dependency_count() >= threshold {
        session.archive_dependencies(store)?;
    } else {
        session.checkpoint(store)?;
    }
    let bytes = pending.iter().try_fold(0u64, |n, (id, text)| {
        n.checked_add((id.len() + text.len() + MEDIA_OUTCOME.len()) as u64)
            .ok_or(RunnerError::Invalid)
    })?;
    let refs = pending
        .iter()
        .map(|(id, text)| (id.as_str(), MEDIA_OUTCOME, text.as_bytes()))
        .collect::<Vec<_>>();
    let write = bytes
        .saturating_mul(2)
        .saturating_add(pending.len() as u64 * 1024)
        .saturating_add(16384);
    session.with_ledger_resources(
        ResourceCaps {
            read_bytes: 4096,
            memory_bytes: write.saturating_mul(2),
            index_bytes: write,
            output_bytes: 0,
            visited_postings: 0,
            transitions: 0,
            decoder_seconds: 0,
        },
        |l| l.append_receipt_batch(&refs),
    )?;
    pending.clear();
    Ok(())
}
