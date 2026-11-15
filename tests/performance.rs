#![cfg(unix)]
#[allow(dead_code)]
mod common;
use common::filesystems::{Damage, Fixture, FsKind, Layout};
use frameharbor::performance::{MeasurementError, observe_allocation};
use std::{
    fs,
    os::unix::fs::{MetadataExt, symlink},
};
#[test]
fn allocation_observation_deduplicates_hardlinks_and_overlapping_roots_with_bounded_no_follow_walk()
{
    let fixture = Fixture::build(FsKind::Fat32, Layout::Contiguous, Damage::None).unwrap();
    let root = fixture.dir.join("allocation");
    fs::create_dir(&root).unwrap();
    let a = root.join("a");
    fs::write(&a, vec![7; 8192]).unwrap();
    fs::hard_link(&a, root.join("linked")).unwrap();
    fs::write(root.join(".DS_Store"), vec![1; 4096]).unwrap();
    let expected =
        (fs::metadata(&root).unwrap().blocks() + fs::metadata(&a).unwrap().blocks()) * 512;
    assert_eq!(observe_allocation(&[&root, &a], 16).unwrap(), expected);
    assert!(matches!(
        observe_allocation(&[&root], 1),
        Err(MeasurementError::ObservationLimit)
    ));
    assert!(root.join(".DS_Store").exists());
    symlink(&fixture.image, root.join("source-link")).unwrap();
    assert!(matches!(
        observe_allocation(&[&root], 16),
        Err(MeasurementError::ObservationLimit)
    ));
}
