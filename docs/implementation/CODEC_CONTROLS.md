# Exposed 720p and 1080p codec controls

On 9 November 2026, the selected Darwin arm64 host passed four exposed mathematical codec controls through actual staging, independent staged-file parsing and full bounded strict native validation. The focused test passed in 8.95 seconds under actual Rust 1.98.1 with offline locked dependencies. Independent read-only review of the fixture extension and test found no blocker for this narrow claim.

These are codec-stage controls with an explicit synthetic direct byte-layout oracle. They do not exercise native filesystem discovery, establish historical ownership, represent a camera recording, measure physical device I/O or qualify full-image recovery. No 4K validation was run: the helper's 4096×2160 admission ceiling is a limit, not a proven codec profile.

## Inputs and checks

`MediaFixture::generate_codec_control` generates one second of 12 Hz `testsrc2` video and 440 Hz mathematical audio with installed FFmpeg/ffprobe 9.0.2. AVC uses libx264; HEVC uses libx265 and the `hvc1` tag. These controls use the ultrafast preset, CRF 35, two configured B frames, a 12-frame GOP, one encoder/filter thread, and AAC at 24 kbit/s, stereo 48 kHz. The ordinary 64×64 development fixtures retain their original duration/preset/channel settings.

Independent ffprobe stream facts assert the requested resolution and stereo 48 kHz audio. Packet positions, sizes and SHA-256 values are checked against retained original bytes. The canonical BMFF parser builds the expected source map; the staging/validation boundary independently reparses the actual staged file and compares encoded samples, configuration and timing, then the native helper verifies packets and fully drains strict decoders. Assertions require 12 video samples, audio presence, strict decode pass, complete packet counts, corresponding frame/sample counts, unchanged whole-file bytes and original-file preservation.

Each test case uses a 120-second allowance including a one-second shutdown reserve, 512 MiB managed memory, 1 GiB admitted reads, 16 MiB index allowance and 64 MiB output allowance. Existing native resource limits remain unchanged. Native helper RSS below is decoder-child observation, excluding the parent and filesystem helper; it is not an aggregate memory guarantee.

## Observations

Raw receipt: `target/codec-profile-observations-28619.tsv`, SHA-256 `13acdb75f10f563e52553931bac2aa905ca18cbe011c7431ea5b3b7d9ccbb4e8`. Times are observations from this run, not calibrated throughput or ETA.

| Codec | Resolution | Encoded bytes | Video / audio samples | Validation ms | Stage + validation ms | Decoder peak RSS bytes | Image API delivered bytes | Staged API read bytes |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| AVC | 1280×720 | 198,721 | 12 / 48 | 951 | 1,087 | 27,344,896 | 731,461 | 997,488 |
| HEVC | 1280×720 | 76,208 | 12 / 48 | 686 | 785 | 29,376,512 | 381,040 | 533,412 |
| AVC | 1920×1080 | 425,050 | 12 / 48 | 895 | 1,153 | 39,813,120 | 1,404,354 | 1,896,711 |
| HEVC | 1920×1080 | 138,834 | 12 / 48 | 735 | 870 | 45,072,384 | 563,098 | 771,717 |

Image API counts include repeated source reads across initial parsing, staging and validation. Staged API counts report validation reads. Neither counter measures raw storage traffic or cache behavior. Source allocation was not recorded in this control receipt, so no allocated-byte or physical-I/O conclusion follows. The one-second low-complexity mathematical controls do not qualify long GOPs, camera parameter sets, higher bit depth/chroma, high bitrate, configuration changes, high frame rates or long-duration decoder behavior.

Reviewed fixture source SHA-256: `ca4575336754fe3a0e21c3754e8308ea0226a11f07511c14324bf963de6b1aab` (`tests/common/media.rs`). Reviewed test SHA-256: `a64ce0a53433de4420a9ed579825e911906441a6d2300fb29fedab11d41e4933` (`tests/codec_profiles.rs`). This evidence supplements selected synthetic coverage; representative M7 hardware, external M10 acceptance and authorized M11 real-image rollout remain pending.
