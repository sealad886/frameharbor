# Frozen-package prospective procedural evaluation

On 9 November 2026, an independent evaluator generated two previously unseen procedural capture/encoder configuration families after the coordinator froze the local package. Four intact filesystem cases and three negative controls met their declared expectations. This is bounded prospective procedural correctness evidence. It does not complete M7, M10, hardware, consumer, real-camera, broad damaged-media or statistical false-join qualification.

## Frozen artifact and independence

The evaluator received the package freeze before selecting the random seed or generating either original. The run began at `2026-11-09T22:02:55Z`, with seed `8645522906898802557`. Exact commands, executable versions, original bytes, raw independent ffprobe output, sample truth, filesystem placement and expectations remain under `target/prospective-evaluation/run-20261109T220255Z`. This ignored local evidence directory is required to reproduce the recorded verdict; this document alone is not the receipt.

| Frozen component | SHA-256 |
| --- | --- |
| `target/local-package/bin/frameharbor` | `c4b50a08b0b044627330f22e7f3dc144e716399248245e0d5dc29004695312b3` |
| Bundled `tsk-helper` | `63459e9c60432e18dd120d53ea5ed0a367416599bd9f3c4fa846369693547f40` |
| Bundled `ffmpeg-helper` | `44c46cc33a7ef016d9e41735d7db7974ec44668264962f52d89a2c613d4cc07b` |
| `BUILD-MANIFEST.txt` | `bda20813412fefcb32e050ade4896200f30ae4a8f74da40a8b8d91e1c7fa31f0` |

Hashes matched before generation and after evaluation. The exact packaged CLI used its bundled native helpers. No engine code, thresholds, profiles or package files changed during evaluation. The evaluator inspected exposed filesystem scaffold code to understand geometry, but generated fresh media/configurations and independent Python placement/oracles. This is disclosed prospective evaluation, not a previously sealed third-party holdout or independent filesystem-format certification. Filesystem/damage variants of one original are correlated; seven controls do not constitute seven independent recording families. Results and truth are now exposed regression material.

Only synthetic image and map paths were supplied as recovery inputs. Original encoded files, truth, expected outcomes and placement documents stayed outside each `inputs/` directory. Ordinary host filesystem permissions were used; this was not an adversarial proof that recovery processes could never access evaluator truth.

## Fresh original families

Both families use a new deterministic moving mathematical luma/chroma field, one 2.4-second encode, three B frames with adaptive placement disabled, two references, CRF 24, one encoder thread, a one-channel 32 kHz AAC-LC track at 40 kbit/s and ordinary tail-`moov` MP4. Full argv and FFmpeg/ffprobe 9.0.2 build information are retained. Each original and each actual published output independently passed strict complete video/audio decoding with `ffmpeg -v error -xerror`. No private recordings, external clips or real images were used.

| Family | Fresh configuration | Original bytes | Original packets | Original SHA-256 |
| --- | --- | ---: | ---: | --- |
| AVC | 144×128, 23 Hz, 679 Hz audio, moving-field phase 33 | 37,273 | 131 | `28154f0e07e3426f523fbcefba96a7e1b21e94d7101ea3faa7d2f1ec09067dc1` |
| HEVC `hvc1` | 144×112, 19 Hz, 682 Hz audio, moving-field phase 41 | 31,432 | 122 | `180860da73d3378844db03adb1d9b1f181d81a74b97a63481aac8c25635f9cd4` |

Both originals contain actual DTS/PTS reordering. ffprobe `-show_packets -show_streams -show_data_hash sha256` supplies packet positions, sizes, hashes, integer DTS/PTS/duration, rational track timebases, sample configuration/extradata, rate/channels, flags and audio side data. Every packet hash was independently recomputed from its original or published file byte range. Raw-table timing uses `-ignore_editlist 1`; a separate default-editlist oracle checks edited packet timing and audio behavior. Per-track original ordinals are retained. Comparison checks packet sequence, hashes, sizes, integer DTS/PTS/duration/flags, stream identity/configuration/timebase and edited packet/stream facts. Exact whole-file byte comparison independently covers all remaining metadata and side-data representation.

## Filesystem and negative outcomes

Native FAT32 uses valid 65,536-cluster geometry with 16 KiB clusters. AVC occupies nonadjacent clusters `[7,10,13]`; HEVC occupies `[7,10]`. Native exFAT uses 128 KiB clusters, explicit FAT allocation, valid directory/boot/upcase/bitmap checksums and historical partition start 2,048 sectors while filesystem offset remains zero. Each small exFAT media file occupies one cluster. Physical spans reconstruct the retained original before recovery. FAT32 image logical length is 1,074,283,520 bytes with under 1 MiB actual allocation; the small exFAT images are 16,908,288 bytes and fully allocated on this host. No exFAT fragmentation claim is made.

| Case | Observed result | Verdict |
| --- | --- | --- |
| AVC/FAT32 intact | One sealed byte-identical original; all 131 packets/config/raw and edited timing/audio match; independent full decode passes | PASS |
| AVC/exFAT intact | Same original and all oracle checks pass | PASS |
| HEVC/FAT32 intact | One sealed byte-identical original; all 122 packets/config/raw and edited timing/audio match; independent full decode passes | PASS |
| HEVC/exFAT intact | Same original and all oracle checks pass | PASS |
| AVC/FAT32 acquisition unavailable | First 512 logical file bytes marked `-`, stale original bytes retained; zero published MP4s; explicit `target metadata acquisition missing` and `sample mapping unresolved`, one unresolved media item | PASS |
| HEVC/FAT32 acquisition unavailable | Same negative expectation and explicit unresolved outcome | PASS |
| Overlapping map | Nonzero exit and explicit `overlapping or unsorted map intervals`; zero published MP4s | PASS |

The unavailable controls remove container metadata from acquisition; they do not test unavailable encoded sample payload. A comment in the retained initial evaluator script describes the hole as a video-sample hole; actual placement, operations, supplemental verification and this report establish the tested metadata hole. No evidence was regenerated to conceal this description error. Intact reports assert acquisition-supported bytes, metadata-supported ownership, valid structure, strict decode pass, original mapping and complete known-layout sample coverage. The independent oracle supports those assertions for these originals only. Stored case state is `Paused` with discovery complete and no pending media; this does not establish exhaustive whole-image recovery.

## Evidence and reproduction

- `target/prospective-evaluation/evaluate.py`: retained evaluator and exact generation/run commands. SHA-256 at execution: `ca8af1d6386c1da09c825544700d70ecf2164ef24c0709e5493277d31b27067b`.
- `run-20261109T220255Z/freeze.json`: seed, timestamp, frozen component identities. SHA-256: `fb5fae73387457c3c6dd23e528723769530f9650423570bd874f4ad7d569a03d`.
- `run-20261109T220255Z/receipt.json`: initial complete seven-case receipt, preserved unchanged. SHA-256: `1933d4848920e1dbf4adc4fa8afd7224c8491a941235795e22a96ad129f8bc71`.
- `target/prospective-evaluation/verify_receipt.py` and run `verification.json`: explicit negative-reason assertions plus independent strict decoding of actual staged artifacts.
- Family `truth/`: immutable original, configuration, generator/probe argv and raw outputs, independent packet truth, original decode logs, placement and expectations.
- Each case: only image/map under `inputs/`; exact CLI argv, start/inspect output, durable case, actual output and independently probed/decode evidence retained separately.

The retained receipt can be independently rechecked with `/usr/bin/python3 target/prospective-evaluation/verify_receipt.py target/prospective-evaluation/run-20261109T220255Z`. Running `evaluate.py` again creates a new timestamped evaluation with a newly chosen seed; it does not reproduce the same configuration selection. Reproducing these originals requires the retained generation argv and recorded FFmpeg build. No dependency installation is required by these scripts.

No product defects were reproduced in this bounded scope. Remaining untested scope includes real camera/device provenance, higher resolutions and sustained hardware behavior, independent consumer acceptance, fMP4, VFR, head-`moov`, edits beyond generated defaults, in-band configuration changes, damaged-payload salvage, ambiguous joins, exFAT multi-cluster fragmentation, broad malformed images and physical power-loss durability. No new false-join confidence bound or full-matrix gate is justified. These CLI trials have no UI; screenshots were not applicable. All seven controls and four staged decode checks passed; no engine fix or retuning followed evaluation.
