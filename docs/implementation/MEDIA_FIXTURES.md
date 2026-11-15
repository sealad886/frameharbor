# M4 mathematical media development corpus

`tests/common/media.rs` generates local two-second, 64×64 mathematical `testsrc2` video and a 440 Hz sine wave. It uses the installed FFmpeg/ffprobe 9.0.2 executables, libx264 or libx265, and native AAC at 24 kbit/s. No private clip, externally downloaded recording or real device is required. These exposed fixtures are development/regression data, not blind holdouts, real-camera examples or measured recovery accuracy.

## Generation and bounds

`MediaFixture::generate(Codec::{Avc,Hevc}, Moov::{Head,Tail}, Timing::{Cfr,Vfr})` returns retained original bytes plus an independent ffprobe oracle. HEVC is explicitly tagged `hvc1`; AVC uses `avc1`; audio uses `mp4a`. Both video encoders use two B frames and disabled adaptive B-frame placement. One encoder thread, one filter thread and disabled x265 pools bound this tiny workload. VFR selects a nonuniform subset of a 12 Hz source while retaining original timestamps; it does not rewrite them to a constant rate.

Every subprocess uses argv directly, null stdin, owned stdout/stderr files, a ten-second runtime bound and a four-MiB per-file output/log bound. No network input or shell execution exists in the helper. Complete version/configuration text, generator arguments, raw oracle arguments and raw/default ffprobe outputs are retained beside `original.mp4` for the fixture lifetime. Success cleanup removes only its owned temporary directory. A failed generation leaves its owned files as failure evidence; it is never silently treated as a missing optional test.

A head-`moov` case uses FFmpeg's faststart pass. Tail-`moov` uses ordinary MP4's default layout. Neither profile uses `moof`. Encoded bytes are retained; tests do not require byte-identical output across different encoder builds. Executable version checks reject versions other than 9.0.2. Build configuration is recorded but this development fixture does not establish a portable reproducible-binary qualification.

## Independent oracle

FFprobe is run with `-ignore_editlist 1 -show_packets -show_streams -show_data_hash sha256`, selected packet/stream fields and the flat writer. Raw per-track DTS, PTS, duration, stream ID, timebase, codec/configuration hash, sample byte position/length/hash, flags and side data are parsed without using FrameHarbor's BMFF parser. A second default-editlist probe retains timing transformations and audio skip/discard-padding observations. Raw media-table timing and edited playback timing must be compared separately; a default ffprobe timestamp shift is not evidence of a parser error.

`PacketTruth.ordinal` is per stream. Position and size identify bytes in the immutable original file, not physical disk addresses. `StreamTruth.extradata_sha256` is ffprobe's codec extradata SHA-256, corresponding to the selected profile's `avcC`, `hvcC` or AAC AudioSpecificConfig representation. The tests independently hash packet byte ranges in the original file and require exact equality with ffprobe's packet hashes. They also require actual decode/presentation reordering, genuine nonuniform presentation intervals for VFR, positive sample durations, AAC presence, interleaved physical packet placement and the requested ordinary-MP4 movie-box position.

Neither packet inspection nor successful sample hashing is a strict decode verdict or proof of original recording ownership. These mathematical originals provide known ground truth; production media must still pass source/ownership, sample-map, staged-artifact and full bounded decoder checks separately.

## Filesystem placement

`Fixture::replace_payload` accepts payloads larger than one cluster and no larger than the existing two-cluster allocation. It updates owned directory sizes/checksums, physical spans, external original bytes and in-memory truth without deriving placement from the product parser. `MediaFixture::padded_bytes` appends a legal top-level `free` box to fill that capacity; encoded samples and their logical offsets remain unchanged. One test places the same original AVC/AAC VFR media into both independent fragmented filesystem profiles, retains the original outside the image and verifies all sample bytes unchanged.

`MediaFixture::generate_prefix_split(codec, timing, boundary)` makes a measured FFmpeg movie-box reservation, then regenerates with the adjusted reservation and a fresh oracle. The first real video sample's four-byte NAL length starts at `boundary - 1`. Tests independently read its first byte from the end of one physical cluster and its remaining three bytes from the nonadjacent next cluster, for AVC and HEVC at both 16 KiB and 128 KiB boundaries. Encoded media and raw timestamps remain compared against the regenerated original; no artificial ASCII prefix substitutes for encoded NAL data. Appending padding alone would not establish this coverage.

## Validation record

On 9 November 2026, `cargo test --test media_fixtures --offline --locked` passed three tests with actual Rust/Cargo 1.98.1. The four generated profile combinations are AVC/head/CFR, AVC/tail/VFR, HEVC/head/VFR and HEVC/tail/CFR; each includes B-frame reordering and AAC. The ordinary filesystem placement test generates an additional AVC/tail/VFR fixture. Four aligned cases additionally cover AVC/HEVC at FAT32/exFAT cluster sizes, each using a measurement encode and a fresh aligned encode. This accepts fixture behavior only; it does not accept M4–M6 recovery, donor repair, damaged-media salvage, general codec profiles, full decoding or blind performance.

Current primary references consulted: [ffprobe options and writers](https://ffmpeg.org/ffprobe.html), [MOV/MP4 muxer and demuxer options](https://ffmpeg.org/ffmpeg-formats.html), and [mathematical source/filter documentation](https://ffmpeg.org/ffmpeg-filters.html). Version-specific behavior is checked against the installed 9.0.2 executable help and generated packet receipts rather than assuming current web documentation alone pins the installed build.
