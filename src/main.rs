use frameharbor::acquisition::ReadLimits;
use frameharbor::acquisition::{MapLimits, RescueMap};
use frameharbor::contracts::{AcquisitionStatus, NORMAL_TOTAL_SECONDS};
use frameharbor::{
    contracts::{BindingEvidence, ByteRange, CoordinateBinding, ResourceCaps},
    control::CaseConfig,
    filesystem::FilesystemProfile,
    jobs::{HelperPin, JobManifest},
    runner::{RunReport, Runner},
};
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::{collections::BTreeMap, io::Read, sync::atomic::AtomicBool};

fn usage() {
    println!(
        "FrameHarbor — bounded evidence-driven recovery\n\nUsage:\n  frameharbor contracts\n  frameharbor inspect-map MAP\n  frameharbor start --image IMAGE --map MAP --case NEW_CASE --output NEW_OUTPUT\n    --filesystem fat32|exfat --fs-start OFFSET --input-origin OFFSET\n    --attest-pairing [--filesystem-helper PATH] [--media-helper PATH]\n  frameharbor resume CASE\n  frameharbor export CASE\n  frameharbor inspect CASE\n  frameharbor pause CASE\n  frameharbor cancel CASE\n\nStart/resume/export continue pending work and publish fully validated outputs.\nPaths for new case/output must have existing parents. Offsets are decimal bytes.\n--input-origin maps that ddrescue input offset to image offset zero.\n--attest-pairing records the operator's image/map coordinate assertion.\nDefault limits: 24 hours/image including 60 seconds shutdown reserve, 512 MiB\nmemory, 8 GiB source/validation read allowance and generated output.\nCase metadata I/O has separate structural bounds; device I/O is not measured. Unsupported or exhausted work remains unresolved.\nOptional decimal limits: --total-seconds (at most 86400),\n--shutdown-reserve-seconds, --read-bytes, --memory-bytes, --index-bytes,\n--output-bytes, --decoder-seconds, --ledger-bytes, --view-index-bytes,\n--checkpoint-history (0..64 historical manifests; default 2).\nLimits are immutable for a case; low caps can yield only partial results.\nNative helpers default to helpers/ beside this executable; build with\nnative/build_helpers.sh. Real-image runs need explicit operator authorization."
    );
}

fn report(r: RunReport) {
    println!("source\t{}", r.source.0);
    println!("state\t{:?}", r.snapshot.state);
    println!("elapsed_millis\t{}", r.snapshot.elapsed_millis);
    println!("remaining_millis\t{}", r.remaining_millis);
    println!("discovery_complete\t{}", r.discovery_complete);
    println!("discovery_scope\t{}", r.discovery_scope);
    println!("excluded_virtual_files\t{}", r.excluded_virtual_files);
    println!(
        "excluded_virtual_directories\t{}",
        r.excluded_virtual_directories
    );
    println!(
        "deferred_discovery_entries\t{}",
        r.deferred_discovery_entries
    );
    println!("pending_media\t{}", r.pending_media);
    println!("unresolved_media\t{}", r.unresolved_media);
    println!(
        "unresolved_examples_truncated\t{}",
        r.unresolved_examples_truncated
    );
    println!("stored_source_valid\t{}", r.source_valid);
    println!("payloads_reverified\t{}", r.payloads_reverified);
    println!(
        "image_read_bytes_this_run\t{:?}",
        r.image_read_bytes_this_run
    );
    println!("provisional_views\t{}", r.provisional_views);
    println!("qualified_views\t{}", r.qualified_views);
    println!("resources\t{:?}", r.snapshot.used);
    for a in r.artifacts {
        println!(
            "artifact\t{}\t{}\t{}\t{}",
            a.id,
            a.bytes,
            a.sha256,
            a.path.display()
        );
        if let frameharbor::publication::ArtifactKind::ValidatedMedia(evidence) = &a.kind {
            let receipt = &evidence.receipt;
            println!("artifact_claims\t{}\t{:?}", a.id, receipt.claims);
            println!(
                "original_file_bytes_preserved\t{}\t{}",
                a.id, receipt.original_file_bytes_preserved
            );
            println!("audio_present\t{}\t{}", a.id, receipt.audio_present);
            match &receipt.transform {
                frameharbor::validation::ArtifactTransform::OriginalFile => {
                    println!("artifact_transform\t{}\toriginal-file", a.id)
                }
                frameharbor::validation::ArtifactTransform::SamplePreserving(segment) => {
                    println!(
                        "artifact_transform\t{}\tsample-preserving-segment\t{} samples",
                        a.id,
                        segment.mappings.len()
                    );
                    for label in &segment.labels {
                        println!("transform_label\t{}\t{label:?}", a.id);
                    }
                }
            }
        }
    }
    for u in r.unresolved {
        println!("unresolved\t{u}");
    }
}
fn new_path(value: &OsString) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let p = PathBuf::from(value);
    let name = p
        .file_name()
        .ok_or("new destination needs a directory name")?;
    if name == "." || name == ".." || p.exists() {
        return Err("destination must be new".into());
    }
    let parent = p
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    Ok(parent.canonicalize()?.join(name))
}
fn pin(path: PathBuf) -> Result<HelperPin, Box<dyn std::error::Error>> {
    let path = path.canonicalize()?;
    let mut f = File::open(&path)?;
    if !f.metadata()?.is_file() || f.metadata()?.len() > 16 * 1024 * 1024 {
        return Err("invalid helper size/type".into());
    }
    let mut h = Sha256::new();
    let mut bytes = [0; 65536];
    let mut remaining = 16 * 1024 * 1024u64;
    loop {
        let n = f.read(&mut bytes[..remaining.min(65536) as usize])?;
        if n == 0 {
            break;
        }
        h.update(&bytes[..n]);
        remaining -= n as u64;
    }
    let mut extra = [0];
    if f.read(&mut extra)? != 0 {
        return Err("helper grew beyond limit".into());
    }
    Ok(HelperPin {
        path,
        sha256: h.finalize().into(),
    })
}
fn start(args: &[OsString]) -> Result<(), Box<dyn std::error::Error>> {
    let started = std::time::Instant::now();
    let mut options = BTreeMap::new();
    let mut attested = false;
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--attest-pairing" {
            if attested {
                return Err("duplicate pairing assertion".into());
            }
            attested = true;
            i += 1;
            continue;
        }
        let key = args[i].to_str().ok_or("invalid option")?;
        if ![
            "--image",
            "--map",
            "--case",
            "--output",
            "--filesystem",
            "--fs-start",
            "--input-origin",
            "--filesystem-helper",
            "--media-helper",
            "--total-seconds",
            "--shutdown-reserve-seconds",
            "--read-bytes",
            "--memory-bytes",
            "--index-bytes",
            "--output-bytes",
            "--decoder-seconds",
            "--ledger-bytes",
            "--view-index-bytes",
            "--checkpoint-history",
        ]
        .contains(&key)
        {
            return Err(format!("unknown option: {key}").into());
        }
        let value = args.get(i + 1).ok_or("option needs value")?.clone();
        if options.insert(key, value).is_some() {
            return Err("duplicate option".into());
        }
        i += 2;
    }
    if !attested {
        return Err("start requires --attest-pairing coordinate assertion".into());
    }
    let required = |k| options.get(k).ok_or_else(|| format!("missing {k}"));
    let image = PathBuf::from(required("--image")?).canonicalize()?;
    let map = PathBuf::from(required("--map")?).canonicalize()?;
    let size = image.metadata()?.len();
    let origin: u64 = required("--input-origin")?
        .to_str()
        .ok_or("invalid input origin")?
        .parse()?;
    let fs_start: u64 = required("--fs-start")?
        .to_str()
        .ok_or("invalid filesystem offset")?
        .parse()?;
    let profile = match required("--filesystem")?.to_str() {
        Some("fat32") => FilesystemProfile::Fat32,
        Some("exfat") => FilesystemProfile::ExFat,
        _ => return Err("filesystem must be fat32 or exfat".into()),
    };
    let number = |key: &str, default: u64| -> Result<u64, Box<dyn std::error::Error>> {
        match options.get(key) {
            Some(value) => Ok(value.to_str().ok_or("invalid numeric value")?.parse()?),
            None => Ok(default),
        }
    };
    let total = number("--total-seconds", NORMAL_TOTAL_SECONDS)?;
    let reserve = number("--shutdown-reserve-seconds", 60.min((total / 4).max(1)))?;
    if !(2..=NORMAL_TOTAL_SECONDS).contains(&total) || reserve == 0 || reserve >= total {
        return Err(
            "total must be 2..86400 seconds, with a positive smaller shutdown reserve".into(),
        );
    }
    let read = number("--read-bytes", 8 * 1024 * 1024 * 1024)?;
    let memory = number("--memory-bytes", 512 * 1024 * 1024)?;
    let index = number("--index-bytes", 128 * 1024 * 1024)?;
    let output = number("--output-bytes", 8 * 1024 * 1024 * 1024)?;
    let decoder = number("--decoder-seconds", total - reserve)?;
    let ledger_bytes = number("--ledger-bytes", 64 * 1024 * 1024)?;
    let view_index_bytes = number("--view-index-bytes", 64 * 1024 * 1024)?;
    let checkpoint_history = number("--checkpoint-history", 2)?;
    if [read, memory, index, output]
        .iter()
        .any(|n| *n == 0 || *n > i64::MAX as u64)
        || decoder > i64::MAX as u64
        || checkpoint_history > 64
        || !(1024 * 1024..=1024 * 1024 * 1024).contains(&ledger_bytes)
        || !(1..=1024 * 1024 * 1024).contains(&view_index_bytes)
    {
        return Err("invalid resource limit; ledger quota must be 1 MiB..1 GiB and view-index quota at most 1 GiB".into());
    }
    let helpers = std::env::current_exe()?
        .parent()
        .ok_or("executable parent unavailable")?
        .join("helpers");
    let manifest = JobManifest {
        image,
        map,
        case_dir: new_path(required("--case")?)?,
        output_dir: new_path(required("--output")?)?,
        binding: CoordinateBinding::new(
            ByteRange::new(
                origin,
                origin.checked_add(size).ok_or("coordinate overflow")?,
            )?,
            -i128::from(origin),
            size,
            BindingEvidence::OperatorAttested,
        )?,
        profile,
        fs_start,
        filesystem_helper: pin(options
            .get("--filesystem-helper")
            .map(PathBuf::from)
            .unwrap_or_else(|| helpers.join("tsk-helper")))?,
        media_helper: pin(options
            .get("--media-helper")
            .map(PathBuf::from)
            .unwrap_or_else(|| helpers.join("ffmpeg-helper")))?,
        read_limits: ReadLimits {
            total: read,
            ..ReadLimits::default()
        },
        case_config: CaseConfig {
            total_millis: total * 1000,
            shutdown_reserve_millis: reserve * 1000,
            caps: ResourceCaps {
                read_bytes: read,
                memory_bytes: memory,
                index_bytes: index,
                output_bytes: output,
                visited_postings: 0,
                transitions: 0,
                decoder_seconds: decoder,
            },
        },
        ledger_bytes,
        index_bytes: view_index_bytes,
        checkpoint_history: checkpoint_history as usize,
    };
    report(Runner::create_started(manifest, started)?.run(&AtomicBool::new(false))?);
    Ok(())
}

fn inspect_map(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let map = RescueMap::parse(File::open(path)?, MapLimits::default())?;
    let mut counts = [0u64; 5];
    for interval in map.intervals() {
        let index = match interval.status {
            AcquisitionStatus::Finished => 0,
            AcquisitionStatus::NonTried => 1,
            AcquisitionStatus::NonTrimmed => 2,
            AcquisitionStatus::NonScraped => 3,
            AcquisitionStatus::BadSector => 4,
            AcquisitionStatus::Unknown => unreachable!("unknown status represents map gaps"),
        };
        counts[index] = counts[index]
            .checked_add(interval.input.len())
            .ok_or("map status byte count overflow")?;
    }
    // This is input-coordinate map coverage, never inferred image identity.
    println!("coordinates\tddrescue-input");
    println!("binding\tunverified");
    println!("intervals\t{}", map.intervals().len());
    for (key, value) in [
        "finished_bytes",
        "non_tried_bytes",
        "non_trimmed_bytes",
        "non_scraped_bytes",
        "bad_sector_bytes",
    ]
    .into_iter()
    .zip(counts)
    {
        println!("{key}\t{value}");
    }
    let first = map.intervals().first().ok_or("empty map")?;
    let last = map.intervals().last().ok_or("empty map")?;
    let span = last.input.end() - first.input.start();
    let declared = counts.into_iter().try_fold(0u64, |total, value| {
        total
            .checked_add(value)
            .ok_or("declared byte count overflow")
    })?;
    println!("internal_unknown_gap_bytes\t{}", span - declared);
    println!("outside_declared_span\tunknown");
    println!("recovered_video\tnot-evaluated");
    Ok(())
}

fn run(args: Vec<OsString>) -> Result<(), Box<dyn std::error::Error>> {
    match args.as_slice() {
        [] => usage(),
        [command] if command == "--help" || command == "-h" => usage(),
        [command] if command == "--version" => {
            println!("frameharbor {}", env!("CARGO_PKG_VERSION"));
        }
        [command] if command == "contracts" => {
            println!(
                "claim_dimensions\tbyte-provenance,ownership,structure,decoder,temporal,coverage"
            );
            println!("normal_budget_seconds_per_image\t{NORMAL_TOTAL_SECONDS}");
            println!("decoder_pass_establishes_original_order\tfalse");
            println!("whole_family_cost\treported-separately");
            println!("media_profiles\tnot-qualified");
        }
        [command, path] if command == "inspect-map" => inspect_map(Path::new(path))?,
        [command, tail @ ..] if command == "start" => start(tail)?,
        [command, path]
            if ["resume", "export", "inspect", "pause", "cancel"]
                .iter()
                .any(|c| command == c) =>
        {
            let started = std::time::Instant::now();
            let path = Path::new(path).canonicalize()?;
            let r = match command.to_str() {
                Some("resume" | "export") => {
                    Runner::open(&path)?.run_started(&AtomicBool::new(false), started)?
                }
                Some("pause") => Runner::pause(&path)?,
                Some("cancel") => Runner::cancel(&path)?,
                _ => Runner::inspect(&path)?,
            };
            report(r);
        }
        _ => return Err("invalid arguments; use --help".into()),
    }
    Ok(())
}

fn main() -> std::process::ExitCode {
    match run(std::env::args_os().skip(1).collect()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("frameharbor: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
