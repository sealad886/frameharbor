//! Independently authored sparse disk fixtures. Truth never comes from a parser.
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FsKind {
    Fat32,
    ExFat,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    Fragmented,
    Contiguous,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Damage {
    None,
    Deleted,
    PartialChain,
    Crosslink,
    BitmapFree,
    AcquisitionHole,
    MetadataHole,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExpectedOwnership {
    MetadataSupported,
    HistoricalConflict,
    SharedGeometry,
    Unresolved,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpectedSpan {
    pub logical_offset: u64,
    pub image_offset: u64,
    pub len: u64,
}
#[derive(Clone, Debug)]
pub struct GroundTruth {
    pub payload: Vec<u8>,
    pub spans: Vec<ExpectedSpan>,
    pub cluster_size: u64,
    pub logical_image_bytes: u64,
    pub ownership: ExpectedOwnership,
    pub complete: bool,
    pub root_offset: u64,
    pub fat_offset: u64,
    pub second_cluster: u32,
}
pub struct Fixture {
    pub dir: PathBuf,
    pub image: PathBuf,
    pub map: PathBuf,
    pub ground_truth: GroundTruth,
}
fn le16(b: &mut [u8], o: usize, n: u16) {
    b[o..o + 2].copy_from_slice(&n.to_le_bytes())
}
fn le32(b: &mut [u8], o: usize, n: u32) {
    b[o..o + 4].copy_from_slice(&n.to_le_bytes())
}
fn le64(b: &mut [u8], o: usize, n: u64) {
    b[o..o + 8].copy_from_slice(&n.to_le_bytes())
}
fn write_at(f: &mut File, o: u64, b: &[u8]) -> io::Result<()> {
    f.seek(SeekFrom::Start(o))?;
    f.write_all(b)
}
fn payload(size: usize) -> Vec<u8> {
    let mut p: Vec<_> = (0..size)
        .map(|i| ((i as u64 * 37 + i as u64 / 251) % 251) as u8)
        .collect();
    p[..16].copy_from_slice(b"\0\0\0\x18ftypisom\0\0\0\0");
    p
}
impl Fixture {
    pub fn build(kind: FsKind, layout: Layout, damage: Damage) -> io::Result<Self> {
        if (kind == FsKind::Fat32 && damage == Damage::BitmapFree)
            || (kind == FsKind::ExFat
                && layout == Layout::Contiguous
                && damage == Damage::PartialChain)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "damage not applicable",
            ));
        }
        let dir = std::env::temp_dir().canonicalize()?.join(format!(
            "frameharbor-filesystem-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir)?;
        let image = dir.join("image.img");
        let map = dir.join("image.map");
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&image)?;
        let truth = match kind {
            FsKind::Fat32 => fat32(&mut f, layout, damage)?,
            FsKind::ExFat => exfat(&mut f, layout, damage)?,
        };
        f.sync_all()?;
        let map_text = if damage == Damage::MetadataHole {
            let h = truth.root_offset;
            format!(
                "0 + 1\n0 {} +\n{} 512 -\n{} {} +\n",
                h,
                h,
                h + 512,
                truth.logical_image_bytes - h - 512
            )
        } else if damage == Damage::AcquisitionHole {
            let h = &truth.spans[1];
            format!(
                "0 + 1\n0 {} +\n{} {} -\n{} {} +\n",
                h.image_offset,
                h.image_offset,
                h.len,
                h.image_offset + h.len,
                truth.logical_image_bytes - h.image_offset - h.len
            )
        } else {
            format!("0 + 1\n0 {} +\n", truth.logical_image_bytes)
        };
        fs::write(&map, map_text)?;
        // External oracle survives independently of image data and metadata.
        fs::write(dir.join("original.bin"), &truth.payload)?;
        let spans = truth
            .spans
            .iter()
            .map(|s| format!("{} {} {}\n", s.logical_offset, s.image_offset, s.len))
            .collect::<String>();
        fs::write(dir.join("truth-spans.txt"), spans)?;
        Ok(Self {
            dir,
            image,
            map,
            ground_truth: truth,
        })
    }
    /// Replace the owned two-cluster payload and update handcrafted directory
    /// sizes/checksums. Original physical placement remains independent truth.
    pub fn replace_payload(&mut self, bytes: &[u8]) -> io::Result<()> {
        let cluster = self.ground_truth.cluster_size as usize;
        if bytes.len() <= cluster || bytes.len() > cluster * 2 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "two-cluster payload capacity",
            ));
        }
        self.ground_truth.spans[1].len = (bytes.len() - cluster) as u64;
        let mut image = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.image)?;
        for span in &self.ground_truth.spans {
            let start = span.logical_offset as usize;
            write_at(
                &mut image,
                span.image_offset,
                &bytes[start..start + span.len as usize],
            )?;
        }
        image.seek(SeekFrom::Start(self.ground_truth.root_offset))?;
        let mut root = [0u8; 256];
        image.read_exact(&mut root)?;
        if cluster == 16384 {
            for start in [0usize, 32] {
                if root[start] != 0 {
                    le32(&mut root, start + 28, bytes.len() as u32);
                }
            }
        } else {
            for start in [64usize, 160] {
                if root[start] & 0x7f == 5 {
                    le64(&mut root, start + 40, bytes.len() as u64);
                    le64(&mut root, start + 56, bytes.len() as u64);
                    let set = &mut root[start..start + 96];
                    let mut checksum = 0u16;
                    for (i, byte) in set.iter().enumerate() {
                        if i != 2 && i != 3 {
                            checksum = checksum.rotate_right(1).wrapping_add(*byte as u16);
                        }
                    }
                    le16(set, 2, checksum);
                }
            }
        }
        write_at(&mut image, self.ground_truth.root_offset, &root)?;
        image.sync_all()?;
        fs::write(self.dir.join("original.bin"), bytes)?;
        self.ground_truth.payload = bytes.to_vec();
        let spans = self
            .ground_truth
            .spans
            .iter()
            .map(|s| format!("{} {} {}\n", s.logical_offset, s.image_offset, s.len))
            .collect::<String>();
        fs::write(self.dir.join("truth-spans.txt"), spans)?;
        Ok(())
    }
    pub fn allocated_bytes(&self) -> io::Result<u64> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Ok(fs::metadata(&self.image)?.blocks() * 512)
        }
        #[cfg(not(unix))]
        {
            Ok(fs::metadata(&self.image)?.len())
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}
fn ownership(d: Damage) -> ExpectedOwnership {
    match d {
        Damage::Deleted | Damage::BitmapFree => ExpectedOwnership::HistoricalConflict,
        Damage::Crosslink => ExpectedOwnership::SharedGeometry,
        Damage::PartialChain => ExpectedOwnership::Unresolved,
        _ => ExpectedOwnership::MetadataSupported,
    }
}
fn fat32(f: &mut File, layout: Layout, damage: Damage) -> io::Result<GroundTruth> {
    const CLUSTER: u64 = 16384;
    const CLUSTERS: u32 = 65536;
    const FAT_SECTORS: u32 = 513;
    const RESERVED: u32 = 32;
    let heap = (RESERVED + 2 * FAT_SECTORS) as u64 * 512;
    let length = heap + CLUSTERS as u64 * CLUSTER;
    f.set_len(length)?;
    let offset = |c: u32| heap + (c - 2) as u64 * CLUSTER;
    let mut boot = [0u8; 512];
    boot[..3].copy_from_slice(&[0xeb, 0x58, 0x90]);
    boot[3..11].copy_from_slice(b"MSWIN4.1");
    le16(&mut boot, 11, 512);
    boot[13] = 32;
    le16(&mut boot, 14, RESERVED as u16);
    boot[16] = 2;
    boot[21] = 0xf8;
    le16(&mut boot, 24, 63);
    le16(&mut boot, 26, 255);
    le32(&mut boot, 32, (length / 512) as u32);
    le32(&mut boot, 36, FAT_SECTORS);
    le32(&mut boot, 44, 2);
    le16(&mut boot, 48, 1);
    le16(&mut boot, 50, 6);
    boot[64] = 0x80;
    boot[66] = 0x29;
    le32(&mut boot, 67, 0xfade0001);
    boot[71..82].copy_from_slice(b"FRAMEHARBOR");
    boot[82..90].copy_from_slice(b"FAT32   ");
    boot[510..].copy_from_slice(&[0x55, 0xaa]);
    write_at(f, 0, &boot)?;
    write_at(f, 6 * 512, &boot)?;
    let mut info = [0u8; 512];
    le32(&mut info, 0, 0x41615252);
    le32(&mut info, 484, 0x61417272);
    le32(&mut info, 488, 0xffffffff);
    le32(&mut info, 492, 0xffffffff);
    le32(&mut info, 508, 0xaa550000);
    write_at(f, 512, &info)?;
    write_at(f, 7 * 512, &info)?;
    let second = if layout == Layout::Fragmented { 9 } else { 6 };
    let mut fat = vec![0u8; FAT_SECTORS as usize * 512];
    for (c, n) in [
        (0, 0x0ffffff8),
        (1, 0x0fffffff),
        (2, 0x0fffffff),
        (
            5,
            if damage == Damage::PartialChain {
                0
            } else {
                second
            },
        ),
        (second, 0x0fffffff),
    ] {
        le32(&mut fat, c as usize * 4, n)
    }
    write_at(f, RESERVED as u64 * 512, &fat)?;
    write_at(f, (RESERVED + FAT_SECTORS) as u64 * 512, &fat)?;
    let p = payload(CLUSTER as usize + 257);
    let mut root = [0u8; 128];
    root[..11].copy_from_slice(b"RECORD  MP4");
    root[11] = 0x20;
    le16(&mut root, 26, 5);
    le32(&mut root, 28, p.len() as u32);
    if damage == Damage::Deleted {
        root[0] = 0xe5
    }
    if damage == Damage::Crosslink {
        let mut alias = [0u8; 32];
        alias.copy_from_slice(&root[..32]);
        alias[..11].copy_from_slice(b"SHARED  MP4");
        root[32..64].copy_from_slice(&alias)
    }
    write_at(f, offset(2), &root)?;
    write_at(f, offset(5), &p[..CLUSTER as usize])?;
    write_at(f, offset(second), &p[CLUSTER as usize..])?;
    Ok(GroundTruth {
        spans: vec![
            ExpectedSpan {
                logical_offset: 0,
                image_offset: offset(5),
                len: CLUSTER,
            },
            ExpectedSpan {
                logical_offset: CLUSTER,
                image_offset: offset(second),
                len: 257,
            },
        ],
        payload: p,
        cluster_size: CLUSTER,
        logical_image_bytes: length,
        ownership: ownership(damage),
        complete: !matches!(
            damage,
            Damage::PartialChain | Damage::AcquisitionHole | Damage::BitmapFree
        ),
        root_offset: offset(2),
        fat_offset: RESERVED as u64 * 512,
        second_cluster: second,
    })
}
fn exfat(f: &mut File, layout: Layout, damage: Damage) -> io::Result<GroundTruth> {
    const CLUSTER: u64 = 131072;
    const CLUSTERS: u32 = 32;
    const FAT: u32 = 24;
    const HEAP: u32 = 256;
    let heap = HEAP as u64 * 512;
    let length = heap + CLUSTERS as u64 * CLUSTER;
    f.set_len(length)?;
    let offset = |c: u32| heap + (c - 2) as u64 * CLUSTER;
    let mut boot = vec![0u8; 12 * 512];
    boot[..3].copy_from_slice(&[0xeb, 0x76, 0x90]);
    boot[3..11].copy_from_slice(b"EXFAT   ");
    le64(&mut boot, 64, 2048);
    le64(&mut boot, 72, length / 512);
    le32(&mut boot, 80, FAT);
    le32(&mut boot, 84, 1);
    le32(&mut boot, 88, HEAP);
    le32(&mut boot, 92, CLUSTERS);
    le32(&mut boot, 96, 2);
    le32(&mut boot, 100, 0xfade0002);
    le16(&mut boot, 104, 0x0100);
    boot[108] = 9;
    boot[109] = 8;
    boot[110] = 1;
    boot[111] = 0x80;
    boot[112] = 0xff;
    boot[510..512].copy_from_slice(&[0x55, 0xaa]);
    for sector in 1..=8 {
        le32(&mut boot, sector * 512 + 508, 0xaa550000)
    }
    let mut sum = 0u32;
    for (i, b) in boot[..11 * 512].iter().enumerate() {
        if ![106, 107, 112].contains(&i) {
            sum = sum.rotate_right(1).wrapping_add(*b as u32)
        }
    }
    for i in (11 * 512..12 * 512).step_by(4) {
        le32(&mut boot, i, sum)
    }
    write_at(f, 0, &boot)?;
    write_at(f, 12 * 512, &boot)?;
    let second = if layout == Layout::Fragmented { 9 } else { 6 };
    let mut fat = [0u8; 512];
    for (c, n) in [
        (0, 0xfffffff8),
        (1, 0xffffffff),
        (2, 0xffffffff),
        (3, 0xffffffff),
        (4, 0xffffffff),
        (
            5,
            if damage == Damage::PartialChain || layout == Layout::Contiguous {
                // NoFatChain's FAT entries are deliberately unusable: the
                // stream flag and bitmap, not this table, establish layout.
                0
            } else {
                second
            },
        ),
        (second, 0xffffffff),
    ] {
        le32(&mut fat, c as usize * 4, n)
    }
    write_at(f, FAT as u64 * 512, &fat)?;
    let mut bitmap = [0u8; 4];
    for c in [2, 3, 4, 5, second] {
        let bit = (c - 2) as usize;
        bitmap[bit / 8] |= 1 << (bit % 8)
    }
    if damage == Damage::BitmapFree {
        let bit = (second - 2) as usize;
        bitmap[bit / 8] &= !(1 << (bit % 8))
    }
    write_at(f, offset(3), &bitmap)?;
    // Identity runs use the exFAT 0xffff skip encoding. In particular, a raw
    // trailing U+FFFF would be an unterminated skip marker, not a valid table.
    let mut upcase = Vec::new();
    for word in [0xffffu16, 97]
        .into_iter()
        .chain(65u16..=90)
        .chain([0xffff, 65413])
    {
        upcase.extend_from_slice(&word.to_le_bytes());
    }
    let upsum = upcase
        .iter()
        .fold(0u32, |a, b| a.rotate_right(1).wrapping_add(*b as u32));
    write_at(f, offset(4), &upcase)?;
    let p = payload(CLUSTER as usize + 257);
    let mut root = vec![0u8; 256];
    root[0] = 0x81;
    le32(&mut root, 20, 3);
    le64(&mut root, 24, bitmap.len() as u64);
    root[32] = 0x82;
    le32(&mut root, 36, upsum);
    le32(&mut root, 52, 4);
    le64(&mut root, 56, upcase.len() as u64);
    let entry = file_entry(
        "RECORD.MP4",
        p.len() as u64,
        5,
        layout,
        damage == Damage::Deleted,
    );
    root[64..160].copy_from_slice(&entry);
    if damage == Damage::Crosslink {
        let alias = file_entry("SHARED.MP4", p.len() as u64, 5, layout, false);
        root[160..256].copy_from_slice(&alias)
    }
    write_at(f, offset(2), &root)?;
    write_at(f, offset(5), &p[..CLUSTER as usize])?;
    write_at(f, offset(second), &p[CLUSTER as usize..])?;
    Ok(GroundTruth {
        spans: vec![
            ExpectedSpan {
                logical_offset: 0,
                image_offset: offset(5),
                len: CLUSTER,
            },
            ExpectedSpan {
                logical_offset: CLUSTER,
                image_offset: offset(second),
                len: 257,
            },
        ],
        payload: p,
        cluster_size: CLUSTER,
        logical_image_bytes: length,
        ownership: ownership(damage),
        complete: !matches!(
            damage,
            Damage::PartialChain | Damage::AcquisitionHole | Damage::BitmapFree
        ),
        root_offset: offset(2),
        fat_offset: FAT as u64 * 512,
        second_cluster: second,
    })
}
fn file_entry(name: &str, size: u64, cluster: u32, layout: Layout, deleted: bool) -> [u8; 96] {
    let mut b = [0u8; 96];
    b[0] = 0x85;
    b[1] = 2;
    le16(&mut b, 4, 0x20);
    let timestamp = (((2026u32 - 1980) << 9 | 11 << 5 | 9) << 16) | (12 << 11);
    for offset in [8, 12, 16] {
        le32(&mut b, offset, timestamp);
    }
    b[22..25].fill(0x80); // Valid UTC+00:00 offsets.

    b[32] = 0xc0;
    b[33] = if layout == Layout::Contiguous { 3 } else { 1 };
    b[35] = name.len() as u8;
    let mut hash = 0u16;
    for c in name.encode_utf16() {
        for v in c.to_le_bytes() {
            hash = hash.rotate_right(1).wrapping_add(v as u16)
        }
    }
    le16(&mut b, 36, hash);
    le64(&mut b, 40, size);
    le32(&mut b, 52, cluster);
    le64(&mut b, 56, size);
    b[64] = 0xc1;
    for (i, c) in name.encode_utf16().enumerate() {
        le16(&mut b, 66 + i * 2, c)
    }
    if deleted {
        b[0] &= 0x7f;
        b[32] &= 0x7f;
        b[64] &= 0x7f
    }
    let mut sum = 0u16;
    for (i, v) in b.iter().enumerate() {
        if i != 2 && i != 3 {
            sum = sum.rotate_right(1).wrapping_add(*v as u16)
        }
    }
    le16(&mut b, 2, sum);
    b
}

impl Fixture {
    /// Independent FAT32 namespace mutation: 1,099 unique allocated nonmedia
    /// files precede the retained original payload in two directory chains.
    /// Only owned synthetic metadata/data are changed; original bytes/spans stay
    /// the external oracle for the final MP4 entry.
    pub fn add_many_files_before_payload(
        &mut self,
        count: usize,
        nonmedia_size: usize,
    ) -> io::Result<()> {
        if !(2..=1800).contains(&count)
            || !(8..=16384).contains(&nonmedia_size)
            || self.ground_truth.cluster_size != 16384
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "FAT32 namespace fixture bounds",
            ));
        }
        let mut image = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.image)?;
        let mut boot = [0; 512];
        image.read_exact(&mut boot)?;
        let fat_bytes = u64::from(u32::from_le_bytes(boot[36..40].try_into().unwrap())) * 512;
        let cluster = self.ground_truth.cluster_size;
        let offset = |n: u32| self.ground_truth.root_offset + u64::from(n - 2) * cluster;
        let mut set_fat = |n: u32, next: u32| -> io::Result<()> {
            for copy in 0..2 {
                write_at(
                    &mut image,
                    self.ground_truth.fat_offset + copy * fat_bytes + u64::from(n) * 4,
                    &next.to_le_bytes(),
                )?;
            }
            Ok(())
        };
        for (n, next) in [
            (100, 101),
            (101, 0x0fff_ffff),
            (104, 105),
            (105, 0x0fff_ffff),
        ] {
            set_fat(n, next)?;
        }
        for i in 0..count - 1 {
            set_fat(200 + i as u32, 0x0fff_ffff)?;
        }
        let entry = |name: &[u8; 11], cluster: u32, size: u32, directory: bool| {
            let mut b = [0; 32];
            b[..11].copy_from_slice(name);
            b[11] = if directory { 0x10 } else { 0x20 };
            b[20..22].copy_from_slice(&((cluster >> 16) as u16).to_le_bytes());
            b[26..28].copy_from_slice(&(cluster as u16).to_le_bytes());
            b[28..32].copy_from_slice(&size.to_le_bytes());
            b
        };
        let mut root = vec![0; cluster as usize];
        root[..32].copy_from_slice(&entry(b"DIRA       ", 100, 0, true));
        root[32..64].copy_from_slice(&entry(b"DIRB       ", 104, 0, true));
        write_at(&mut image, offset(2), &root)?;
        let first = count / 2;
        for (dir, begin, end, prefix) in [(100, 0, first, b'A'), (104, first, count, b'B')] {
            let mut bytes = vec![0; 2 * cluster as usize];
            bytes[..32].copy_from_slice(&entry(b".          ", dir, 0, true));
            bytes[32..64].copy_from_slice(&entry(b"..         ", 0, 0, true));
            for i in begin..end {
                let last = i + 1 == count;
                let name = if last {
                    format!("{prefix:02X}{i:06}MP4")
                } else {
                    format!("{prefix:02X}{i:06}BIN")
                };
                let physical = if last { 5 } else { 200 + i as u32 };
                let size = if last {
                    self.ground_truth.payload.len() as u32
                } else {
                    nonmedia_size as u32
                };
                let slot = (i - begin + 2) * 32;
                bytes[slot..slot + 32].copy_from_slice(&entry(
                    name.as_bytes().try_into().unwrap(),
                    physical,
                    size,
                    false,
                ));
                if !last {
                    let mut noise = vec![0; nonmedia_size];
                    noise[..8].copy_from_slice(b"not-mp4!");
                    write_at(&mut image, offset(physical), &noise)?;
                }
            }
            write_at(&mut image, offset(dir), &bytes)?;
        }
        image.sync_all()?;
        fs::write(
            self.dir.join("namespace-truth.txt"),
            format!(
                "FAT32 allocated reachable files={count}; unique nonmedia bytes=not-mp4!; last entry points to retained original physical spans\n"
            ),
        )?;
        Ok(())
    }
}
