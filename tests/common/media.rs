//! Development-only mathematical media and independent packet ground truth.
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};
const FFMPEG: &str = "/opt/homebrew/opt/ffmpeg-full/bin/ffmpeg";
const FFPROBE: &str = "/opt/homebrew/opt/ffmpeg-full/bin/ffprobe";
const CAP: u64 = 4 * 1024 * 1024;
static NEXT: AtomicU64 = AtomicU64::new(0);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    Avc,
    Hevc,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Moov {
    Head,
    Tail,
    Fragmented,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Timing {
    Cfr,
    Vfr,
}
#[derive(Debug, Clone)]
pub struct PacketTruth {
    pub stream_index: usize,
    pub ordinal: usize,
    pub pos: u64,
    pub size: u64,
    pub dts: i64,
    pub pts: i64,
    pub duration: i64,
    pub sha256: String,
    pub flags: String,
    pub side_data: BTreeMap<String, String>,
}
#[derive(Debug, Clone)]
pub struct StreamTruth {
    pub index: usize,
    pub id: u32,
    pub codec_name: String,
    pub codec_type: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u8>,
    pub time_base: (i64, i64),
    pub extradata_sha256: String,
}
pub struct MediaFixture {
    pub dir: PathBuf,
    pub file: PathBuf,
    pub bytes: Vec<u8>,
    pub packets: Vec<PacketTruth>,
    pub streams: Vec<StreamTruth>,
    pub probe_text: String,
    pub version: String,
    pub default_probe_text: String,
}
fn error(s: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, s)
}
fn run(exe: &str, args: &[String], dir: &Path, label: &str) -> io::Result<Vec<u8>> {
    let output = dir.join(format!("{label}.stdout"));
    let errors = dir.join(format!("{label}.stderr"));
    let stdout = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output)?;
    let stderr = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&errors)?;
    let mut child = Command::new(exe)
        .args(args)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()?;
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                return Err(error(&format!(
                    "{label} failed: {}",
                    String::from_utf8_lossy(&fs::read(&errors)?)
                )));
            }
            break;
        }
        let too_large = fs::read_dir(dir)?
            .filter_map(|e| e.ok())
            .filter_map(|e| e.metadata().ok())
            .any(|m| m.len() > CAP);
        if start.elapsed() > Duration::from_secs(10) || too_large {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error("media fixture subprocess time/output cap"));
        }
        std::thread::yield_now();
    }
    if fs::metadata(&output)?.len() > CAP || fs::metadata(&errors)?.len() > CAP {
        return Err(error("fixture log cap"));
    }
    fs::read(output)
}
fn text(bytes: Vec<u8>) -> io::Result<String> {
    String::from_utf8(bytes).map_err(|_| error("non UTF8 tool output"))
}
fn fields(s: &str, prefix: &str) -> io::Result<BTreeMap<usize, BTreeMap<String, String>>> {
    let mut result: BTreeMap<usize, BTreeMap<String, String>> = BTreeMap::new();
    for line in s.lines() {
        let Some(rest) = line.strip_prefix(prefix) else {
            continue;
        };
        let Some((index, tail)) = rest.split_once('.') else {
            return Err(error("flat oracle index"));
        };
        let index = index
            .parse::<usize>()
            .map_err(|_| error("flat numeric index"))?;
        let Some((name, value)) = tail.split_once('=') else {
            return Err(error("flat oracle field"));
        };
        result
            .entry(index)
            .or_default()
            .insert(name.into(), value.trim_matches('"').into());
    }
    Ok(result)
}
fn required<'a>(m: &'a BTreeMap<String, String>, key: &str) -> io::Result<&'a str> {
    m.get(key)
        .map(String::as_str)
        .ok_or_else(|| error(&format!("missing oracle {key}")))
}
fn number<T: std::str::FromStr>(m: &BTreeMap<String, String>, key: &str) -> io::Result<T> {
    required(m, key)?
        .parse()
        .map_err(|_| error(&format!("invalid oracle {key}")))
}
fn digest(s: &str) -> io::Result<String> {
    let h = s
        .strip_prefix("SHA256:")
        .ok_or_else(|| error("non SHA256 oracle"))?;
    if h.len() != 64 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(error("invalid oracle SHA256"));
    }
    Ok(h.to_ascii_lowercase())
}
impl MediaFixture {
    pub fn generate(codec: Codec, moov: Moov, timing: Timing) -> io::Result<Self> {
        Self::generate_inner(codec, moov, timing, None, false, None)
    }
    /// Closed HEVC IDR_N_LP development profile; zero B/leading pictures,
    /// fixed 12-frame closed GOPs. Ordinary fixture behavior stays unchanged.
    pub fn generate_closed_idr(codec: Codec, moov: Moov, timing: Timing) -> io::Result<Self> {
        Self::generate_inner(codec, moov, timing, None, true, None)
    }
    /// Exposed codec-rate control, not camera or filesystem qualification.
    /// One second/12 frames, ultrafast software encoder, stereo AAC at48kHz.
    pub fn generate_codec_control(codec: Codec, width: u32, height: u32) -> io::Result<Self> {
        if ![(1280, 720), (1920, 1080), (3840, 2160)].contains(&(width, height)) {
            return Err(error("unsupported codec control resolution"));
        }
        Self::generate_inner(
            codec,
            Moov::Tail,
            Timing::Cfr,
            None,
            false,
            Some((width, height)),
        )
    }
    /// Fresh native oracle after moov reservation aligns a real four-byte NAL
    /// length prefix so that its final three bytes cross a filesystem boundary.
    pub fn generate_prefix_split(codec: Codec, timing: Timing, boundary: u64) -> io::Result<Self> {
        if ![16384, 131072].contains(&boundary) {
            return Err(error("unsupported fixture boundary"));
        }
        let measurement = Self::generate_inner(codec, Moov::Head, timing, Some(8192), false, None)?;
        let video = measurement
            .streams
            .iter()
            .find(|s| s.codec_type == "video")
            .ok_or_else(|| error("missing video"))?;
        let first = measurement
            .packets
            .iter()
            .find(|p| p.stream_index == video.index)
            .ok_or_else(|| error("missing video sample"))?;
        let reserve = 8192u64
            .checked_add(boundary - 1)
            .and_then(|n| n.checked_sub(first.pos))
            .ok_or_else(|| error("alignment reserve"))?;
        let aligned = Self::generate_inner(codec, Moov::Head, timing, Some(reserve), false, None)?;
        let video = aligned
            .streams
            .iter()
            .find(|s| s.codec_type == "video")
            .ok_or_else(|| error("missing video"))?;
        let first = aligned
            .packets
            .iter()
            .find(|p| p.stream_index == video.index)
            .ok_or_else(|| error("missing video sample"))?;
        if first.pos != boundary - 1 || first.size < 5 {
            return Err(error("NAL prefix alignment failed"));
        }
        let pos = first.pos as usize;
        let nal_length = u32::from_be_bytes(aligned.bytes[pos..pos + 4].try_into().unwrap()) as u64;
        if nal_length == 0 || nal_length > first.size - 4 {
            return Err(error("invalid generated NAL length prefix"));
        }
        Ok(aligned)
    }
    fn generate_inner(
        codec: Codec,
        moov: Moov,
        timing: Timing,
        reserve: Option<u64>,
        closed_idr: bool,
        resolution: Option<(u32, u32)>,
    ) -> io::Result<Self> {
        let dir = std::env::temp_dir().canonicalize()?.join(format!(
            "frameharbor-media-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir)?;
        let file = dir.join("original.mp4");
        let version = text(run(FFMPEG, &["-version".into()], &dir, "ffmpeg-version")?)?;
        if !version.starts_with("ffmpeg version 9.0.2 ") {
            return Err(error("unqualified FFmpeg build version"));
        }
        let probe_version = text(run(FFPROBE, &["-version".into()], &dir, "ffprobe-version")?)?;
        if !probe_version.starts_with("ffprobe version 9.0.2 ") {
            return Err(error("unqualified ffprobe build version"));
        }
        let (width, height) = resolution.unwrap_or((64, 64));
        let duration = if resolution.is_some() { "1" } else { "2" };
        let channels = if resolution.is_some() { "2" } else { "1" };
        let video_input = format!("testsrc2=size={width}x{height}:rate=12:duration={duration}");
        let audio_input = format!("sine=frequency=440:sample_rate=48000:duration={duration}");
        let mut args: Vec<String> = [
            "-hide_banner",
            "-nostdin",
            "-v",
            "error",
            "-n",
            "-f",
            "lavfi",
            "-i",
            video_input.as_str(),
            "-f",
            "lavfi",
            "-i",
            audio_input.as_str(),
            "-map",
            "0:v:0",
            "-map",
            "1:a:0",
            "-filter_threads",
            "1",
            "-threads",
            "1",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        if timing == Timing::Vfr {
            args.extend(["-vf", "select=not(eq(mod(n\\,3)\\,1))"].map(str::to_owned));
        }
        args.extend(
            [
                "-fps_mode",
                "vfr",
                "-c:v",
                if codec == Codec::Avc {
                    "libx264"
                } else {
                    "libx265"
                },
                "-preset",
                if resolution.is_some() {
                    "ultrafast"
                } else {
                    "medium"
                },
                "-crf",
                "35",
                "-bf",
                if closed_idr { "0" } else { "2" },
                "-g",
                "12",
                "-threads:v",
                "1",
            ]
            .map(str::to_owned),
        );
        if codec == Codec::Avc {
            args.extend(["-x264-params", "b-adapt=0:scenecut=0:threads=1"].map(str::to_owned));
        } else {
            args.extend(
                [
                    "-x265-params",
                    if closed_idr { "pools=none:frame-threads=1:b-adapt=0:bframes=0:open-gop=0:radl=0:keyint=12:min-keyint=12:scenecut=0:log-level=error" } else { "pools=none:frame-threads=1:b-adapt=0:bframes=2:log-level=error" },
                    "-tag:v",
                    "hvc1",
                ]
                .map(str::to_owned),
            );
        }
        args.extend(
            [
                "-c:a",
                "aac",
                "-b:a",
                "24k",
                "-ac",
                channels,
                "-threads:a",
                "1",
                "-t",
                duration,
                "-map_metadata",
                "-1",
                "-video_track_timescale",
                "12288",
            ]
            .map(str::to_owned),
        );
        if let Some(reserve) = reserve {
            args.extend(["-moov_size".into(), reserve.to_string()]);
        } else if moov == Moov::Fragmented {
            args.extend(
                ["-movflags", "+empty_moov+frag_keyframe+default_base_moof"].map(str::to_owned),
            );
        } else if moov == Moov::Head {
            args.extend(["-movflags", "+faststart"].map(str::to_owned));
        }
        args.push(file.to_string_lossy().into());
        fs::write(dir.join("generation-arguments.txt"), args.join("\n"))?;
        run(FFMPEG, &args, &dir, "encode")?;
        if fs::metadata(&file)?.len() > CAP {
            return Err(error("media bytes cap"));
        }
        let bytes = fs::read(&file)?;
        let entries = "stream=index,id,codec_name,codec_type,width,height,sample_rate,channels,time_base,extradata_hash:packet=stream_index,pts,dts,duration,size,pos,flags,data_hash:packet_side_data=side_data_type,skip_samples,discard_padding";
        let mut probe: Vec<String> = [
            "-v",
            "error",
            "-ignore_editlist",
            "1",
            "-show_packets",
            "-show_streams",
            "-show_data_hash",
            "sha256",
            "-show_entries",
            entries,
            "-of",
            "flat",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        probe.push(file.to_string_lossy().into());
        fs::write(dir.join("raw-probe-arguments.txt"), probe.join("\n"))?;
        let probe_text = text(run(FFPROBE, &probe, &dir, "raw-oracle")?)?;
        probe.drain(2..4);
        let default_probe_text = text(run(FFPROBE, &probe, &dir, "default-oracle")?)?;
        let mut ordinals = BTreeMap::<usize, usize>::new();
        let mut packets = Vec::new();
        for (_, p) in fields(&probe_text, "packets.packet.")? {
            let stream_index = number(&p, "stream_index")?;
            let ordinal = ordinals.entry(stream_index).or_default();
            packets.push(PacketTruth {
                stream_index,
                ordinal: *ordinal,
                pos: number(&p, "pos")?,
                size: number(&p, "size")?,
                dts: number(&p, "dts")?,
                pts: number(&p, "pts")?,
                duration: number(&p, "duration")?,
                sha256: digest(required(&p, "data_hash")?)?,
                flags: required(&p, "flags")?.into(),
                side_data: p
                    .iter()
                    .filter(|(k, _)| k.starts_with("side_data_list."))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            });
            *ordinal += 1;
        }
        let mut streams = Vec::new();
        for (_, s) in fields(&probe_text, "streams.stream.")? {
            let id = required(&s, "id")?;
            let id = u32::from_str_radix(
                id.strip_prefix("0x")
                    .ok_or_else(|| error("stream ID representation"))?,
                16,
            )
            .map_err(|_| error("stream ID value"))?;
            let (n, d) = required(&s, "time_base")?
                .split_once('/')
                .ok_or_else(|| error("timebase"))?;
            streams.push(StreamTruth {
                index: number(&s, "index")?,
                id,
                codec_name: required(&s, "codec_name")?.into(),
                codec_type: required(&s, "codec_type")?.into(),
                width: if s.contains_key("width") {
                    Some(number(&s, "width")?)
                } else {
                    None
                },
                height: if s.contains_key("height") {
                    Some(number(&s, "height")?)
                } else {
                    None
                },
                sample_rate: if s.contains_key("sample_rate") {
                    Some(number(&s, "sample_rate")?)
                } else {
                    None
                },
                channels: if s.contains_key("channels") {
                    Some(number(&s, "channels")?)
                } else {
                    None
                },
                time_base: (
                    n.parse().map_err(|_| error("timebase numerator"))?,
                    d.parse().map_err(|_| error("timebase denominator"))?,
                ),
                extradata_sha256: digest(required(&s, "extradata_hash")?)?,
            });
        }
        Ok(Self {
            dir,
            file,
            bytes,
            packets,
            streams,
            probe_text,
            version,
            default_probe_text,
        })
    }
    /// Append a legal top-level free box; sample offsets and payload stay unchanged.
    pub fn padded_bytes(&self, total: usize) -> io::Result<Vec<u8>> {
        if total < self.bytes.len() + 8 || total > CAP as usize {
            return Err(error("padding size"));
        }
        let padding = total - self.bytes.len();
        let mut b = self.bytes.clone();
        b.extend_from_slice(&(padding as u32).to_be_bytes());
        b.extend_from_slice(b"free");
        b.resize(total, 0);
        Ok(b)
    }
    pub fn verify_packet_bytes(&self) -> io::Result<()> {
        for p in &self.packets {
            let start = usize::try_from(p.pos).map_err(|_| error("packet offset"))?;
            let end = start
                .checked_add(usize::try_from(p.size).map_err(|_| error("packet size"))?)
                .ok_or_else(|| error("packet range overflow"))?;
            let bytes = self
                .bytes
                .get(start..end)
                .ok_or_else(|| error("packet out of range"))?;
            if format!("{:x}", Sha256::digest(bytes)) != p.sha256 {
                return Err(error("packet payload mismatch"));
            }
        }
        Ok(())
    }
}
impl Drop for MediaFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}
