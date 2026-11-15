use frameharbor::storage::observe_storage;
#[test]
fn storage_observation_preserves_physical_uncertainty() {
    let path = std::env::temp_dir();
    let observed = observe_storage(&path, &path);
    assert_eq!(observed.same_filesystem, Some(true));
    assert_eq!(observed.topology, "physical-device-unknown");
    #[cfg(unix)]
    assert!(observed.destination_free_bytes.is_some());
    let unavailable = observe_storage(
        &path,
        &path.join("frameharbor-missing-storage-observation-path"),
    );
    assert_eq!(unavailable.destination_free_bytes, None);
    assert!(!unavailable.limitations.is_empty());
}
