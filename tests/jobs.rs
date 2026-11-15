#[allow(dead_code)]
mod common;
use common::filesystems::{Damage, Fixture, FsKind, Layout};
use frameharbor::{
    acquisition::ReadLimits, contracts::*, control::CaseConfig, filesystem::FilesystemProfile,
    jobs::*, ledger::Ledger,
};
use std::{fs, path::PathBuf};
fn config() -> CaseConfig {
    CaseConfig {
        total_millis: 60000,
        shutdown_reserve_millis: 1000,
        caps: ResourceCaps {
            read_bytes: 128 * 1024 * 1024,
            memory_bytes: 512 * 1024 * 1024,
            index_bytes: 16 * 1024 * 1024,
            output_bytes: 16 * 1024 * 1024,
            visited_postings: 0,
            transitions: 0,
            decoder_seconds: 0,
        },
    }
}
fn manifest(f: &Fixture) -> JobManifest {
    let root = f.dir.canonicalize().unwrap();
    let size = f.ground_truth.logical_image_bytes;
    JobManifest {
        image: root.join("image.raw"),
        map: root.join("image.map"),
        case_dir: root.clone(),
        output_dir: root.join("exports"),
        binding: CoordinateBinding::new(
            ByteRange::new(0, size).unwrap(),
            0,
            size,
            BindingEvidence::OperatorAttested,
        )
        .unwrap(),
        profile: FilesystemProfile::Fat32,
        fs_start: 0,
        filesystem_helper: HelperPin {
            path: root.join("filesystem-helper"),
            sha256: [1; 32],
        },
        media_helper: HelperPin {
            path: root.join("media-helper"),
            sha256: [2; 32],
        },
        read_limits: ReadLimits::default(),
        case_config: config(),
        ledger_bytes: 16 * 1024 * 1024,
        index_bytes: 4 * 1024 * 1024,
        checkpoint_history: 2,
    }
}
#[test]
fn manifest_roundtrip_is_immutable_bounded_and_semantic() {
    let f = Fixture::build(FsKind::Fat32, Layout::Contiguous, Damage::None).unwrap();
    let m = manifest(&f);
    let mut ledger = Ledger::open(&f.dir.join("case.sqlite"), m.ledger_bytes).unwrap();
    m.persist(&mut ledger).unwrap();
    m.persist(&mut ledger).unwrap();
    let loaded = JobManifest::load(&ledger).unwrap();
    let before = fs::read(f.dir.join("case.sqlite")).unwrap();
    assert_eq!(
        JobManifest::load_from_case(&f.dir.canonicalize().unwrap())
            .unwrap()
            .encode()
            .unwrap(),
        m.encode().unwrap()
    );
    assert_eq!(fs::read(f.dir.join("case.sqlite")).unwrap(), before);
    assert_eq!(m.encode().unwrap(), loaded.encode().unwrap());
    let source = SourceGeneration("source1".into());
    assert_eq!(
        m.discovery_spec(source.clone()).unwrap().id().unwrap(),
        loaded.discovery_spec(source.clone()).unwrap().id().unwrap()
    );
    let mut changed = m.clone();
    changed.fs_start = 512;
    assert!(changed.persist(&mut ledger).is_err());
    assert_ne!(
        m.discovery_spec(source.clone()).unwrap().id().unwrap(),
        changed.discovery_spec(source).unwrap().id().unwrap()
    );
    let mut bytes = m.encode().unwrap();
    bytes[12] ^= 1;
    assert!(JobManifest::decode(&bytes).is_err());
    assert!(JobManifest::decode(&vec![0; 65537]).is_err());
    let mut bad = m.clone();
    bad.image = PathBuf::from("relative");
    assert!(bad.encode().is_err());
    bad = m;
    bad.binding = CoordinateBinding::new(
        ByteRange::new(0, 512).unwrap(),
        0,
        512,
        BindingEvidence::Unverified,
    )
    .unwrap();
    assert!(bad.encode().is_err());
}
#[test]
fn helper_pins_enforce_content_and_bounded_file_size() {
    use sha2::{Digest, Sha256};
    let f = Fixture::build(FsKind::Fat32, Layout::Contiguous, Damage::None).unwrap();
    let mut m = manifest(&f);
    for pin in [&mut m.filesystem_helper, &mut m.media_helper] {
        fs::write(&pin.path, b"synthetic helper bytes").unwrap();
        pin.sha256 = Sha256::digest(b"synthetic helper bytes").into();
    }
    assert_eq!(
        m.verify_helpers().unwrap(),
        2 * b"synthetic helper bytes".len() as u64
    );
    fs::write(&m.filesystem_helper.path, b"changed helper bytes").unwrap();
    assert!(m.verify_helpers().is_err());
    fs::File::create(&m.filesystem_helper.path)
        .unwrap()
        .set_len(16 * 1024 * 1024 + 1)
        .unwrap();
    assert!(m.verify_helpers().is_err());
}

#[cfg(unix)]
#[test]
fn case_manifest_loader_rejects_symlink_case_ancestors() {
    use std::os::unix::fs::symlink;
    let f = Fixture::build(FsKind::Fat32, Layout::Contiguous, Damage::None).unwrap();
    let root = f.dir.canonicalize().unwrap();
    let m = manifest(&f);
    let mut ledger = Ledger::open(&root.join("case.sqlite"), m.ledger_bytes).unwrap();
    m.persist(&mut ledger).unwrap();
    assert!(JobManifest::load_from_case(&root).is_ok());
    let link = root.join("case-link");
    symlink(&root, &link).unwrap();
    assert!(JobManifest::load_from_case(&link).is_err());
}
