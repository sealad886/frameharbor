# Independent native media validation

Observed 9 November 2026. This records the selected synthetic validation foundation, not blind accuracy, real-image recovery or a general codec/hardware qualification.

## Pinned implementation

The helper is `native/ffmpeg_helper.c`, using the common `native/limits.h` policy and Rust's canonical subprocess supervisor. Installed headers and actual pkg-config versions are:

| Component | Selected build |
| --- | --- |
| FFmpeg | 9.0.2, Homebrew `ffmpeg-full/9.0.2_1` |
| libavformat | 63.1.102 |
| libavcodec | 63.1.102 |
| libavutil | 61.1.102 |

Compilation used:

```sh
cc -std=c11 -Wall -Wextra -Werror \
  -I/opt/homebrew/Cellar/ffmpeg-full/9.0.2_1/include \
  native/ffmpeg_helper.c \
  -L/opt/homebrew/Cellar/ffmpeg-full/9.0.2_1/lib \
  -lavformat -lavcodec -lavutil -o target/ffmpeg-helper
```

The helper rejects a global version other than 9.0.2 or any compile/runtime library-version mismatch before requesting input bytes. Rust checks the exact selected protocol/ABI. Numeric codec IDs are interpreted within that pin; the actual HEVC ID in these headers/runtime is 172. Assuming a remembered ID of 173 was caught by the independent packet check and corrected in the validator.

Context7's current FFmpeg doxygen examples confirmed the custom AVIO lifecycle and codec send/receive/drain model. Its version catalogue did not contain 9.0.2, so the actual installed, version-matched headers resolved API ownership and signature uncertainty. Primary [custom AVIO documentation](https://ffmpeg.org/doxygen/trunk/avio_read_callback_8c-example.html) describes caller-owned AVIO buffer cleanup; the actual pinned headers and compilation govern this implementation.

## Independent observations and strict policy

The helper accepts only artifact length. It receives no source image, staged filename or URL. Custom AVIO implements bounded byte-read IPC, local seek and size queries; the parent serves every actual read through the case authority. Reads are clamped to 64 KiB and the remaining artifact length. EOF returns an explicit error code rather than fabricated source zeros. Requests to open any external substream URL are denied.

The MOV demuxer runs with `ignore_editlist=1`, and unused-option detection prevents silently claiming that policy if it was not applied. Raw packet media timestamps are compared with original target tables; the original edit list is preserved and checked separately. No CFR clock is inferred. Global demux interleaving order is not a claim of original per-track identity or chronology.

For every supported stream the helper reports track identity, codec/tag, timebase and raw extradata hash. AAC channel/rate observations come from the initialized decoder's ASC interpretation; legacy sample-entry defaults remain separate in the BMFF map. Each packet reports stream ordinal, actual encoded payload position/size/SHA-256, DTS/PTS/duration and flags. Unknown timestamps remain `NOPTS`, never zero-filled.

Each decoder has one thread, error concealment disabled, and error recognition mask 65551: CRC, bitstream, buffer, explode and careful checks. Corrupt packets, corrupt frames, decoder error flags, warnings and errors prevent success. Every stream is fully drained at EOF, and a nonempty stream producing no frames fails. Cleanup finishes before the final summary and successful completion marker. Decoder frame counts and audio `nb_samples` remain observations: an AAC frame of 1024 decoded samples does not override a target table's shorter final packet duration.

Actual frame dimensions are reported and bounded by 4096×2160; decoder pixel allocation also has a coded-padding allowance. Audio frames are bounded to 8192 samples/eight channels. These are guards, not proof that every profile within those maxima is qualified. The tested mathematical media are 64×64 AVC/HEVC with AAC-LC.

## Evidence and limits

A standalone synthetic byte-IPC smoke produced:

| Fixture | Bytes / served reads | Packets / frames | Warnings / errors | Observed maximum RSS |
| --- | --- | --- | --- | ---: |
| AVC + AAC, one second | 16,565 / 1 | 60 / 60 | 0 / 0 | 16,973,824 bytes |
| HEVC `hvc1` + AAC, one second | 16,841 / 1 | 60 / 60 | 0 / 0 | 16,220,160 bytes |

The stronger Rust staged-validation tests passed eight exposed AVC/HEVC × head/tail `moov` × CFR/VFR profiles with B frames and audio. They copy a qualified fragmented logical view into fresh owned staging, independently reparse the actual artifact, compare every packet/configuration/timing observation, strictly decode the whole interval, and recheck staged bytes/identity. Wrong asserted bytes, order, timing, configuration or audio cannot create the sealed validated token. Native hang, controlled exit and malformed protocol are exercised through the actual helper with owned-child reap receipts.

These tests do not establish untouched-holdout precision, general camera compatibility, all malformed-input behavior, real-image yield or a 24-hour device envelope. Native CPU/VM/read/output/wall controls have the same explicit [process/resource limitations](NATIVE_HELPER.md) as the filesystem helper. Darwin's additional-VM mapping limit is not an absolute RSS guarantee. The experimental OS source-read allowlist remains unqualified and disabled; no exploitation-resistant sandbox is claimed. Every playable publication still requires the sealed validation receipt and the durable canonical publication protocol.
