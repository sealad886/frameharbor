#![cfg(target_os = "macos")]
#[allow(dead_code)]
mod common;
use common::filesystems::{Damage, Fixture, FsKind, Layout};
use frameharbor::{
    acquisition::ReadLimits,
    contracts::*,
    control::CaseConfig,
    filesystem::*,
    ledger::Ledger,
    session::{CaseSession, SessionInput},
};
use std::{
    collections::{BTreeSet, VecDeque},
    fs::{self, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::PathBuf,
    process::Command,
    sync::{OnceLock, atomic::AtomicBool},
};
fn helper() -> &'static PathBuf {
    static H: OnceLock<PathBuf> = OnceLock::new();
    H.get_or_init(|| {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let out = root.join(format!("target/tsk-directory-batch-{}", std::process::id()));
        let status = Command::new("/usr/bin/cc")
            .args([
                "-std=c11",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-I/usr/local/include",
            ])
            .arg(root.join("native/tsk_helper.c"))
            .arg("/usr/local/lib/libtsk.23.dylib")
            .arg("-o")
            .arg(&out)
            .status()
            .unwrap();
        assert!(status.success());
        out
    })
}
fn session(f: &Fixture) -> CaseSession {
    let size = f.ground_truth.logical_image_bytes;
    CaseSession::create(
        Ledger::open(&f.dir.join("case.sqlite"), 32 * 1024 * 1024).unwrap(),
        SessionInput {
            image: f.image.clone(),
            map: f.map.clone(),
            binding: CoordinateBinding::new(
                ByteRange::new(0, size).unwrap(),
                0,
                size,
                BindingEvidence::OperatorAttested,
            )
            .unwrap(),
            limits: ReadLimits {
                per_read: 1024 * 1024,
                total: 512 * 1024 * 1024,
                receipts: 8192,
            },
        },
        CaseConfig {
            total_millis: 600000,
            shutdown_reserve_millis: 1000,
            caps: ResourceCaps {
                read_bytes: 512 * 1024 * 1024,
                memory_bytes: 512 * 1024 * 1024,
                index_bytes: 32 * 1024 * 1024,
                output_bytes: 16 * 1024 * 1024,
                visited_postings: 0,
                transitions: 0,
                decoder_seconds: 0,
            },
        },
    )
    .unwrap()
}
fn entry(name: &[u8; 11], cluster: u32, directory: bool) -> [u8; 32] {
    let mut b = [0; 32];
    b[..11].copy_from_slice(name);
    b[11] = if directory { 0x10 } else { 0x20 };
    b[20..22].copy_from_slice(&((cluster >> 16) as u16).to_le_bytes());
    b[26..28].copy_from_slice(&(cluster as u16).to_le_bytes());
    if !directory {
        b[28..32].copy_from_slice(&512u32.to_le_bytes());
    }
    b
}
fn write_at(file: &mut fs::File, offset: u64, bytes: &[u8]) {
    file.seek(SeekFrom::Start(offset)).unwrap();
    file.write_all(bytes).unwrap();
}
fn namespace() -> Fixture {
    let f = Fixture::build(FsKind::Fat32, Layout::Fragmented, Damage::None).unwrap();
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&f.image)
        .unwrap();
    let mut boot = [0; 512];
    file.read_exact(&mut boot).unwrap();
    let fat_bytes = u32::from_le_bytes(boot[36..40].try_into().unwrap()) as u64 * 512;
    let cluster = f.ground_truth.cluster_size;
    let offset = |n: u32| f.ground_truth.root_offset + (n - 2) as u64 * cluster;
    for (n, next) in [(100, 101), (101, 0x0fffffff), (104, 105), (105, 0x0fffffff)] {
        for copy in 0..2 {
            write_at(
                &mut file,
                f.ground_truth.fat_offset + copy * fat_bytes + n as u64 * 4,
                &(next as u32).to_le_bytes(),
            );
        }
    }
    let mut root = vec![0; cluster as usize];
    root[..32].copy_from_slice(&entry(b"DIRA       ", 100, true));
    root[32..64].copy_from_slice(&entry(b"DIRB       ", 104, true));
    write_at(&mut file, offset(2), &root);
    for (n, prefix) in [(100, b'A'), (104, b'B')] {
        let mut data = vec![0; 2 * cluster as usize];
        data[..32].copy_from_slice(&entry(b".          ", n, true));
        data[32..64].copy_from_slice(&entry(b"..         ", 0, true));
        for i in 0..550 {
            let name = format!("{}{i:07}BIN", prefix as char);
            data[(i + 2) * 32..(i + 3) * 32].copy_from_slice(&entry(
                name.as_bytes().try_into().unwrap(),
                5,
                false,
            ));
        }
        write_at(&mut file, offset(n), &data);
    }
    file.sync_all().unwrap();
    f
}
#[test]
fn native_directory_cursor_processes_more_than_1024_names_in_bounded_pages() {
    let f = namespace();
    let mut case = session(&f);
    let mut queue = VecDeque::from([DirectoryCursor {
        directory_inode: None,
        next_entry: 0,
    }]);
    let mut directories = BTreeSet::new();
    let mut files = BTreeSet::new();
    let mut pages = 0;
    let mut resumed = false;
    while let Some(cursor) = queue.pop_front() {
        let batch = inspect_directory_batch(
            &mut case,
            helper(),
            f.ground_truth.logical_image_bytes,
            0,
            FilesystemProfile::Fat32,
            cursor,
            DirectoryBatchLimits {
                entries: 128,
                ..DirectoryBatchLimits::default()
            },
            &AtomicBool::new(false),
            |_, _| Ok(()),
        )
        .unwrap();
        pages += 1;
        assert!(batch.counters().entries_completed <= 128);
        assert!(batch.files().len() <= 128);
        assert_eq!(batch.start_entry(), cursor.next_entry);
        directories.insert(batch.directory_inode());
        for child in batch.children() {
            if !directories.contains(&child.inode) {
                queue.push_back(DirectoryCursor {
                    directory_inode: Some(child.inode),
                    next_entry: 0,
                });
            }
        }
        for file in batch.files() {
            assert_ne!(file.view.evidence, ExtentEvidence::MetadataAllocated);
            assert!(
                files.insert(file.view.owner.file_identity.clone()),
                "completed file entry processed twice"
            );
        }
        if !batch.directory_complete() {
            let next = DirectoryCursor {
                directory_inode: Some(batch.directory_inode()),
                next_entry: batch.next_entry(),
            };
            if !resumed {
                let mut checkpoints = frameharbor::checkpoint::CheckpointStore::create(
                    &f.dir.join("frontiers"),
                    frameharbor::checkpoint::CheckpointLimits::default(),
                )
                .unwrap();
                let source = case.source_generation();
                case.pause(&mut checkpoints).unwrap();
                drop(case);
                case = CaseSession::resume(
                    Ledger::open(&f.dir.join("case.sqlite"), 32 * 1024 * 1024).unwrap(),
                    SessionInput {
                        image: f.image.clone(),
                        map: f.map.clone(),
                        binding: CoordinateBinding::new(
                            ByteRange::new(0, f.ground_truth.logical_image_bytes).unwrap(),
                            0,
                            f.ground_truth.logical_image_bytes,
                            BindingEvidence::OperatorAttested,
                        )
                        .unwrap(),
                        limits: ReadLimits {
                            per_read: 1024 * 1024,
                            total: 512 * 1024 * 1024,
                            receipts: 8192,
                        },
                    },
                    source,
                    &checkpoints,
                )
                .unwrap();
                resumed = true;
            }
            queue.push_front(next);
        }
    }
    assert_eq!(files.len(), 1100);
    assert_eq!(directories.len(), 3);
    assert!(pages >= 11);
    assert!(resumed);
}

#[test]
fn batch_metadata_holes_and_interrupted_callbacks_leave_no_successful_cursor() {
    let f = Fixture::build(FsKind::Fat32, Layout::Fragmented, Damage::MetadataHole).unwrap();
    let mut case = session(&f);
    let mut cleanup = None;
    let result = inspect_directory_batch_observed(
        &mut case,
        helper(),
        f.ground_truth.logical_image_bytes,
        0,
        FilesystemProfile::Fat32,
        DirectoryCursor {
            directory_inode: None,
            next_entry: 0,
        },
        DirectoryBatchLimits::default(),
        &AtomicBool::new(false),
        |_, _| Ok(()),
        |c| cleanup = Some(c),
    );
    assert!(result.is_err());
    let cleanup = cleanup.unwrap();
    assert!(cleanup.reaped);
    let f = Fixture::build(FsKind::Fat32, Layout::Fragmented, Damage::None).unwrap();
    let mut case = session(&f);
    let mut completed = 0;
    let mut cleanup = None;
    let result = inspect_directory_batch_observed(
        &mut case,
        helper(),
        f.ground_truth.logical_image_bytes,
        0,
        FilesystemProfile::Fat32,
        DirectoryCursor {
            directory_inode: None,
            next_entry: 0,
        },
        DirectoryBatchLimits::default(),
        &AtomicBool::new(false),
        |_, event| {
            if let DirectoryEvent::EntryComplete { .. } = event {
                completed += 1;
                return Err(FilesystemError::Unresolved);
            }
            Ok(())
        },
        |c| cleanup = Some(c),
    );
    assert!(result.is_err());
    assert_eq!(completed, 1);
    assert!(cleanup.unwrap().reaped);
    let batch = inspect_directory_batch(
        &mut case,
        helper(),
        f.ground_truth.logical_image_bytes,
        0,
        FilesystemProfile::Fat32,
        DirectoryCursor {
            directory_inode: None,
            next_entry: 0,
        },
        DirectoryBatchLimits::default(),
        &AtomicBool::new(false),
        |_, _| Ok(()),
    )
    .unwrap();
    assert!(batch.directory_complete());
    assert_eq!(batch.files().len(), 1);
}

#[cfg(unix)]
#[test]
fn malformed_batch_cursor_records_are_rejected_and_children_reaped() {
    use std::os::unix::fs::PermissionsExt;
    for records in [
        "DIRECTORY 2 1 0\nENTRY 1\n",
        "DIRECTORY 2 1 0\nENTRY 0\nIGNORED\nENTRY_END 2\n",
        "DIRECTORY 2 2 0\nBATCH_END 0 1 0\n",
        "DIRECTORY 2 1 0\nENTRY 0\nEXCLUDED 9\n",
    ] {
        let f = Fixture::build(FsKind::Fat32, Layout::Fragmented, Damage::None).unwrap();
        let path = f.dir.join("bad-batch-helper");
        let size = f.ground_truth.logical_image_bytes;
        fs::write(&path,format!("#!/bin/sh\nprintf '%s\\n' 'HELLO 4.14.0 4.14.0' 'GEOMETRY 8 512 {} 2'\nprintf '%s' '{}'\n",size/512,records)).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        let mut case = session(&f);
        let mut cleanup = None;
        let result = inspect_directory_batch_observed(
            &mut case,
            &path,
            size,
            0,
            FilesystemProfile::Fat32,
            DirectoryCursor {
                directory_inode: None,
                next_entry: 0,
            },
            DirectoryBatchLimits::default(),
            &AtomicBool::new(false),
            |_, _| Ok(()),
            |c| cleanup = Some(c),
        );
        assert!(matches!(result, Err(FilesystemError::Protocol)));
        assert!(cleanup.unwrap().reaped);
    }
}

#[test]
fn virtual_scope_exclusions_do_not_skip_real_directory_with_orphan_name() {
    let f = Fixture::build(FsKind::Fat32, Layout::Fragmented, Damage::None).unwrap();
    let mut image = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&f.image)
        .unwrap();
    let mut boot = [0; 512];
    image.read_exact(&mut boot).unwrap();
    let fat_bytes = u32::from_le_bytes(boot[36..40].try_into().unwrap()) as u64 * 512;
    for copy in 0..2 {
        write_at(
            &mut image,
            f.ground_truth.fat_offset + copy * fat_bytes + 100 * 4,
            &0x0fffffffu32.to_le_bytes(),
        );
    }
    let short = *b"ORPHAN~1   ";
    let mut checksum = 0u8;
    for b in short {
        checksum = checksum.rotate_right(1).wrapping_add(b);
    }
    let name = "$OrphanFiles"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut long = [0xffu8; 32];
    long[0] = 0x41;
    long[11] = 0x0f;
    long[12] = 0;
    long[13] = checksum;
    long[26..28].fill(0);
    for (offset, ch) in [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30]
        .into_iter()
        .zip(name)
    {
        long[offset..offset + 2].copy_from_slice(&ch.to_le_bytes());
    }
    let mut root = vec![0; f.ground_truth.cluster_size as usize];
    root[..32].copy_from_slice(&long);
    root[32..64].copy_from_slice(&entry(&short, 100, true));
    write_at(&mut image, f.ground_truth.root_offset, &root);
    let mut dir = vec![0; f.ground_truth.cluster_size as usize];
    dir[..32].copy_from_slice(&entry(b".          ", 100, true));
    dir[32..64].copy_from_slice(&entry(b"..         ", 0, true));
    write_at(
        &mut image,
        f.ground_truth.root_offset + 98 * f.ground_truth.cluster_size,
        &dir,
    );
    image.sync_all().unwrap();
    let mut case = session(&f);
    let batch = inspect_directory_batch(
        &mut case,
        helper(),
        f.ground_truth.logical_image_bytes,
        0,
        FilesystemProfile::Fat32,
        DirectoryCursor {
            directory_inode: None,
            next_entry: 0,
        },
        DirectoryBatchLimits::default(),
        &AtomicBool::new(false),
        |_, _| Ok(()),
    )
    .unwrap();
    assert_eq!(batch.children().len(), 1);
    assert_eq!(batch.exclusions().virtual_directories, 1);
    assert_eq!(batch.exclusions().virtual_files, 3);
    assert_eq!(batch.ignored_virtual_entries(), 4);
    assert_eq!(batch.namespace_policy(), NAMESPACE_POLICY);
    assert!(batch.deferred().is_empty());
}

#[test]
fn allocation_work_cap_is_reported_unperformed_without_claiming_a_conflict() {
    let f = Fixture::build(FsKind::Fat32, Layout::Fragmented, Damage::None).unwrap();
    let mut image = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&f.image)
        .unwrap();
    let mut boot = [0; 512];
    image.read_exact(&mut boot).unwrap();
    let fat_bytes = u32::from_le_bytes(boot[36..40].try_into().unwrap()) as u64 * 512;
    let clusters = 2560u32;
    for copy in 0..2 {
        for n in 5..5 + clusters {
            let next = if n + 1 == 5 + clusters {
                0x0fffffff
            } else {
                n + 1
            };
            write_at(
                &mut image,
                f.ground_truth.fat_offset + copy * fat_bytes + n as u64 * 4,
                &next.to_le_bytes(),
            );
        }
    }
    let mut root = vec![0; f.ground_truth.cluster_size as usize];
    let mut item = entry(b"LARGE   MP4", 5, false);
    item[28..32].copy_from_slice(&(clusters * f.ground_truth.cluster_size as u32).to_le_bytes());
    root[..32].copy_from_slice(&item);
    write_at(&mut image, f.ground_truth.root_offset, &root);
    image.sync_all().unwrap();
    let mut case = session(&f);
    let batch = inspect_directory_batch(
        &mut case,
        helper(),
        f.ground_truth.logical_image_bytes,
        0,
        FilesystemProfile::Fat32,
        DirectoryCursor {
            directory_inode: None,
            next_entry: 0,
        },
        DirectoryBatchLimits::default(),
        &AtomicBool::new(false),
        |_, _| Ok(()),
    )
    .unwrap();
    assert_eq!(batch.files().len(), 1);
    let file = &batch.files()[0];
    assert_eq!(file.unchecked_runs, [0]);
    assert_eq!(file.view.evidence, ExtentEvidence::MetadataCandidate);
    assert!(!file.view.runs[0].allocation_consistent);
    assert_eq!(batch.counters().allocation_checks, 0);
}

#[test]
fn aggregate_run_limit_finishes_a_prefix_and_resumes_before_the_unreported_file() {
    let f = Fixture::build(FsKind::Fat32, Layout::Fragmented, Damage::None).unwrap();
    let mut image = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&f.image)
        .unwrap();
    let mut boot = [0; 512];
    image.read_exact(&mut boot).unwrap();
    let fat_bytes = u32::from_le_bytes(boot[36..40].try_into().unwrap()) as u64 * 512;
    let mut root = vec![0; f.ground_truth.cluster_size as usize];
    for (i, start) in [100u32, 2000, 4000].into_iter().enumerate() {
        for copy in 0..2 {
            for ordinal in 0..512 {
                let cluster = start + ordinal * 2;
                let next = if ordinal == 511 {
                    0x0fffffff
                } else {
                    cluster + 2
                };
                write_at(
                    &mut image,
                    f.ground_truth.fat_offset + copy * fat_bytes + u64::from(cluster) * 4,
                    &next.to_le_bytes(),
                );
            }
        }
        let name = format!("FRAG{i:04}MP4");
        let mut item = entry(name.as_bytes().try_into().unwrap(), start, false);
        item[28..32].copy_from_slice(&(512 * f.ground_truth.cluster_size as u32).to_le_bytes());
        root[i * 32..(i + 1) * 32].copy_from_slice(&item);
    }
    write_at(&mut image, f.ground_truth.root_offset, &root);
    image.sync_all().unwrap();
    let mut case = session(&f);
    let first = inspect_directory_batch(
        &mut case,
        helper(),
        f.ground_truth.logical_image_bytes,
        0,
        FilesystemProfile::Fat32,
        DirectoryCursor {
            directory_inode: None,
            next_entry: 0,
        },
        DirectoryBatchLimits::default(),
        &AtomicBool::new(false),
        |_, _| Ok(()),
    )
    .unwrap();
    assert_eq!(first.files().len(), 2);
    assert_eq!(
        first
            .files()
            .iter()
            .map(|f| f.view.runs.len())
            .sum::<usize>(),
        1024
    );
    assert_eq!(first.next_entry(), 2);
    assert_eq!(first.stop_reason(), BatchStopReason::RunLimit);
    assert!(!first.directory_complete());
    let second = inspect_directory_batch(
        &mut case,
        helper(),
        f.ground_truth.logical_image_bytes,
        0,
        FilesystemProfile::Fat32,
        DirectoryCursor {
            directory_inode: Some(first.directory_inode()),
            next_entry: first.next_entry(),
        },
        DirectoryBatchLimits::default(),
        &AtomicBool::new(false),
        |_, _| Ok(()),
    )
    .unwrap();
    assert_eq!(second.files().len(), 1);
    assert_eq!(second.files()[0].entry_index, 2);
    assert_eq!(second.files()[0].view.runs.len(), 512);
    assert!(second.directory_complete());
    assert_eq!(second.stop_reason(), BatchStopReason::DirectoryComplete);
    let owners = first
        .files()
        .iter()
        .chain(second.files())
        .map(|f| &f.view.owner.file_identity)
        .collect::<BTreeSet<_>>();
    assert_eq!(owners.len(), 3);
}

#[test]
fn bounded_inspect_convenience_reports_each_real_child_cleanup_on_global_cap() {
    let f = namespace();
    let mut case = session(&f);
    let mut cleanup = Vec::new();
    let result = inspect_observed(
        &mut case,
        helper(),
        f.ground_truth.logical_image_bytes,
        0,
        FilesystemProfile::Fat32,
        AdapterLimits {
            files: 1,
            ..AdapterLimits::default()
        },
        &AtomicBool::new(false),
        |receipt| cleanup.push(receipt),
    );
    assert!(matches!(result, Err(FilesystemError::ResourceLimit)));
    assert_eq!(cleanup.len(), 2);
    assert!(cleanup.iter().all(|r| r.reaped));
}

#[test]
fn allocation_budget_stops_before_a_file_that_fits_a_fresh_batch() {
    let f = Fixture::build(FsKind::Fat32, Layout::Fragmented, Damage::None).unwrap();
    let mut image = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&f.image)
        .unwrap();
    let mut boot = [0; 512];
    image.read_exact(&mut boot).unwrap();
    let fat_bytes = u32::from_le_bytes(boot[36..40].try_into().unwrap()) as u64 * 512;
    let mut root = vec![0; f.ground_truth.cluster_size as usize];
    for (i, start) in [100u32, 2000, 4000].into_iter().enumerate() {
        for copy in 0..2 {
            for ordinal in 0..1024 {
                let cluster = start + ordinal;
                let next = if ordinal == 1023 {
                    0x0fffffff
                } else {
                    cluster + 1
                };
                write_at(
                    &mut image,
                    f.ground_truth.fat_offset + copy * fat_bytes + u64::from(cluster) * 4,
                    &next.to_le_bytes(),
                );
            }
        }
        let name = format!("ALLOC{i:03}MP4");
        let mut item = entry(name.as_bytes().try_into().unwrap(), start, false);
        item[28..32].copy_from_slice(&(16 * 1024 * 1024u32).to_le_bytes());
        root[i * 32..(i + 1) * 32].copy_from_slice(&item);
    }
    write_at(&mut image, f.ground_truth.root_offset, &root);
    image.sync_all().unwrap();
    let mut case = session(&f);
    let first = inspect_directory_batch(
        &mut case,
        helper(),
        f.ground_truth.logical_image_bytes,
        0,
        FilesystemProfile::Fat32,
        DirectoryCursor {
            directory_inode: None,
            next_entry: 0,
        },
        DirectoryBatchLimits::default(),
        &AtomicBool::new(false),
        |_, _| Ok(()),
    )
    .unwrap();
    assert_eq!(first.files().len(), 2);
    assert_eq!(first.next_entry(), 2);
    assert_eq!(first.stop_reason(), BatchStopReason::AllocationBudget);
    assert_eq!(first.counters().allocation_checks, 65536);
    assert!(first.files().iter().all(
        |f| f.unchecked_runs.is_empty() && f.view.runs.iter().all(|r| r.allocation_consistent)
    ));
    let second = inspect_directory_batch(
        &mut case,
        helper(),
        f.ground_truth.logical_image_bytes,
        0,
        FilesystemProfile::Fat32,
        DirectoryCursor {
            directory_inode: Some(first.directory_inode()),
            next_entry: first.next_entry(),
        },
        DirectoryBatchLimits::default(),
        &AtomicBool::new(false),
        |_, _| Ok(()),
    )
    .unwrap();
    assert_eq!(second.files().len(), 1);
    assert_eq!(second.files()[0].entry_index, 2);
    assert!(second.directory_complete());
    assert_eq!(second.counters().allocation_checks, 32768);
    assert!(second.files()[0].unchecked_runs.is_empty());
    assert!(
        second.files()[0]
            .view
            .runs
            .iter()
            .all(|r| r.allocation_consistent)
    );
    assert_eq!(
        first
            .files()
            .iter()
            .chain(second.files())
            .map(|f| &f.view.owner.file_identity)
            .collect::<BTreeSet<_>>()
            .len(),
        3
    );
}
