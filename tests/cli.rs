use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_frameharbor"))
}

#[test]
fn map_inspection_reports_input_coverage_and_preserves_source() {
    let path = std::env::temp_dir().join(format!(
        "frameharbor-cli-{}-{}.map",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let input = "4096 + 1\n4096 4 +\n4102 2 -\n4104 2 ?\n4106 2 *\n4108 2 /\n";
    fs::write(&path, input).unwrap();
    let output = binary().arg("inspect-map").arg(&path).output().unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    let fields: std::collections::HashMap<_, _> = text
        .lines()
        .map(|line| line.split_once('\t').unwrap())
        .collect();
    assert_eq!(fields["finished_bytes"], "4");
    assert_eq!(fields["internal_unknown_gap_bytes"], "2");
    for field in [
        "bad_sector_bytes",
        "non_tried_bytes",
        "non_trimmed_bytes",
        "non_scraped_bytes",
    ] {
        assert_eq!(fields[field], "2");
    }
    assert_eq!(fields["binding"], "unverified");
    assert_eq!(fields["recovered_video"], "not-evaluated");
    assert_eq!(fs::read_to_string(&path).unwrap(), input);
    fs::remove_file(path).unwrap();
}

#[test]
fn malformed_map_and_incomplete_start_arguments_fail() {
    let path = std::env::temp_dir().join(format!(
        "frameharbor-cli-bad-{}-{}.map",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(&path, "0 + 1\n0 20 +\n10 20 -\n").unwrap();
    let output = binary().arg("inspect-map").arg(&path).output().unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!binary().arg("start").status().unwrap().success());
    fs::remove_file(path).unwrap();
}

#[cfg(target_os = "macos")]
#[allow(dead_code)]
mod common;

#[cfg(target_os = "macos")]
#[test]
fn synthetic_cli_start_resume_inspect_and_cancel_preserve_exact_export() {
    use common::{
        filesystems::{Damage, Fixture, FsKind, Layout},
        media::{Codec, MediaFixture, Moov, Timing},
    };
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    assert!(
        Command::new("/bin/sh")
            .arg(root.join("native/build_helpers.sh"))
            .status()
            .unwrap()
            .success()
    );
    let media = MediaFixture::generate(Codec::Hevc, Moov::Head, Timing::Vfr).unwrap();
    let mut fixture = Fixture::build(FsKind::Fat32, Layout::Fragmented, Damage::None).unwrap();
    fixture
        .replace_payload(&media.padded_bytes(32768).unwrap())
        .unwrap();
    let case = fixture.dir.join("cli-case");
    let output = fixture.dir.join("cli-export");
    let mut start = binary();
    start
        .arg("start")
        .arg("--image")
        .arg(&fixture.image)
        .arg("--map")
        .arg(&fixture.map)
        .arg("--case")
        .arg(&case)
        .arg("--output")
        .arg(&output)
        .args([
            "--filesystem",
            "fat32",
            "--input-origin",
            "0",
            "--fs-start",
            "0",
            "--total-seconds",
            "120",
            "--shutdown-reserve-seconds",
            "5",
            "--read-bytes",
            "2147483648",
            "--index-bytes",
            "67108864",
            "--checkpoint-history",
            "4",
        ]);
    // A map cannot authenticate its pairing; missing assertion must create no case.
    assert!(!start.output().unwrap().status.success());
    assert!(!case.exists());
    let first = start.arg("--attest-pairing").output().unwrap();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let text = String::from_utf8(first.stdout).unwrap();
    let manifest =
        frameharbor::jobs::JobManifest::load_from_case(&case.canonicalize().unwrap()).unwrap();
    assert_eq!(manifest.checkpoint_history, 4);
    assert_eq!(manifest.case_config.total_millis, 120000);
    assert_eq!(manifest.case_config.shutdown_reserve_millis, 5000);
    assert_eq!(manifest.case_config.caps.read_bytes, 2147483648);
    assert_eq!(manifest.case_config.caps.index_bytes, 67108864);
    let artifact = text.lines().find(|s| s.starts_with("artifact\t")).unwrap();
    let path = artifact.split('\t').nth(4).unwrap();
    assert_eq!(fs::read(path).unwrap(), fixture.ground_truth.payload);
    for command in ["inspect", "pause", "resume", "export", "inspect"] {
        let result = binary().arg(command).arg(&case).output().unwrap();
        assert!(
            result.status.success(),
            "{command}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            String::from_utf8(result.stdout)
                .unwrap()
                .lines()
                .any(|s| s == artifact)
        );
    }
    assert!(
        binary()
            .arg("cancel")
            .arg(&case)
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(
        !binary()
            .arg("resume")
            .arg(&case)
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(fs::read(path).unwrap(), fixture.ground_truth.payload);
}

#[cfg(target_os = "macos")]
#[test]
fn invalid_cli_budgets_create_no_case_or_output() {
    use common::filesystems::{Damage, Fixture, FsKind, Layout};
    let fixture = Fixture::build(FsKind::Fat32, Layout::Contiguous, Damage::None).unwrap();
    for (i, (key, value)) in [
        ("--total-seconds", "86401"),
        ("--read-bytes", "0"),
        ("--memory-bytes", "18446744073709551615"),
    ]
    .into_iter()
    .enumerate()
    {
        let case = fixture.dir.join(format!("bad-case-{i}"));
        let output = fixture.dir.join(format!("bad-output-{i}"));
        let result = binary()
            .arg("start")
            .arg("--image")
            .arg(&fixture.image)
            .arg("--map")
            .arg(&fixture.map)
            .arg("--case")
            .arg(&case)
            .arg("--output")
            .arg(&output)
            .args([
                "--filesystem",
                "fat32",
                "--input-origin",
                "0",
                "--fs-start",
                "0",
                "--attest-pairing",
                key,
                value,
            ])
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(!case.exists());
        assert!(!output.exists());
    }
}
