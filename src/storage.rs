//! Bounded read-only storage observations; physical topology stays uncertain.
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct StorageObservation {
    pub destination_free_bytes: Option<u64>,
    pub same_filesystem: Option<bool>,
    pub topology: &'static str,
    pub limitations: Vec<&'static str>,
}
pub fn observe_storage(source: &Path, destination: &Path) -> StorageObservation {
    let mut result = StorageObservation {
        destination_free_bytes: None,
        same_filesystem: None,
        topology: "physical-device-unknown",
        limitations: Vec::new(),
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let (Ok(a), Ok(b)) = (source.metadata(), destination.metadata()) {
            result.same_filesystem = Some(a.dev() == b.dev());
        }
    }
    // Equal filesystem IDs imply shared filesystem accounting. Different IDs do
    // not prove distinct physical disks (containers, partitions and RAID exist).
    result
        .limitations
        .push("filesystem identity does not establish independent physical devices");
    let Some(path) = destination.canonicalize().ok() else {
        result.limitations.push("destination metadata unavailable");
        return result;
    };
    let Ok(mut child) = Command::new("/bin/df")
        .args(["-Pk"])
        .arg(path)
        .env("LC_ALL", "C")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        result.limitations.push("free-space helper unavailable");
        return result;
    };
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if status.success() {
                    let mut bytes = Vec::new();
                    if let Some(stdout) = child.stdout.take()
                        && stdout.take(65537).read_to_end(&mut bytes).is_ok()
                        && bytes.len() <= 65536
                        && let Ok(s) = std::str::from_utf8(&bytes)
                        && let Some(row) = s.lines().last()
                    {
                        let fields: Vec<_> = row.split_whitespace().collect();
                        if fields.len() >= 6 {
                            result.destination_free_bytes = fields[3]
                                .parse::<u64>()
                                .ok()
                                .and_then(|n| n.checked_mul(1024));
                        }
                    }
                }
                break;
            }
            Ok(None) if started.elapsed() < Duration::from_secs(2) => {
                std::thread::sleep(Duration::from_millis(10))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                result
                    .limitations
                    .push("free-space helper failed or exceeded two seconds");
                break;
            }
        }
    }
    if result.destination_free_bytes.is_none() {
        result
            .limitations
            .push("free space unknown; do not reserve output from this observation");
    }
    result
        .limitations
        .push("free space is an observation, not a reservation");
    result
}
