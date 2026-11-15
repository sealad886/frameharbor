# FrameHarbor

Standalone project for MP4 restoration from raw ddrescue images and mapfiles.

**0.1.0-alpha.1 is a source-only preview.** It exposes the selected synthetic
recovery core for evaluation; representative hardware, real-camera and consumer
acceptance gates remain open. [Build and operating instructions](docs/RELEASE.md)
describe the selected macOS arm64 environment. No portable native binary package
is provided. See the [changelog](CHANGELOG.md) for scope and limitations.

FrameHarbor now includes bounded image/map acquisition, FAT32/exFAT extent
discovery, ordinary MP4 sample mapping, strict native validation, durable
publication, and restartable job controls. The selected unchanged-original
AVC/HEVC/AAC path is tested with synthetic fixtures. Conservative closed-IDR
AVC/HEVC segments from surviving ordinary/fMP4 target tables also have source-map,
strict-decode and publication tests. Missing tables, broader recovery profiles,
full-scale performance and blind media qualification remain in development. The planning package was
moved here from VideoRebuild; this implementation is standalone.

- [Agent kickoff / handover](START_PROMPT.md)
- [Algorithm design](docs/planning/image-first-video-restoration/DESIGN.md)
- [Implementation plan](docs/planning/image-first-video-restoration/IMPLEMENTATION_PLAN.md)
- [Evidence and MP4 research](docs/planning/image-first-video-restoration/EVIDENCE.md)
- [Consensus research](docs/planning/image-first-video-restoration/CONSENSUS_RESEARCH.md)
- [Relocation details](docs/planning/RELOCATION.md)

MP4 is the primary target. The planned normal budget is 24 hours per image, with explicit unresolved results when evidence or resources are exhausted. Original images and mapfiles remain read-only.

At relocation handoff, the original 16 planning/evidence files were moved
unchanged into `docs/planning/image-first-video-restoration/`, and no recovery
code had been implemented or run. Subsequent implementation progress is
recorded below; the historical reports and receipts remain unchanged.

## Implementation status

The 15 November alpha candidate passed **214 tests**, formatting, all-target
Clippy with warnings denied, native/release builds, package checksums, version
and source-identity checks with the current native dependencies. Review also
found and repaired a zero-history checkpoint interruption defect, with two new
regressions. Final exact-commit and hosted review, tagging and publication remain
pending; see the [release review ledger](docs/implementation/RELEASE_REVIEW.md).

The 9 November selected synthetic software gate passed 212 tests, including the native
filesystem/media pipeline and restartable controller. An independent prospective
evaluation of that frozen local package passed seven bounded controls across two
fresh procedural encoder families. Broader media recovery milestones remain
unqualified. See
[implementation status](docs/implementation/STATUS.md),
[supported capabilities](docs/implementation/SUPPORT.md), and
[prospective evaluation](docs/implementation/PROSPECTIVE_EVALUATION.md).

The historical local development bundle and raw receipts are under ignored
`target/` paths on the development host; they are not included in a source
checkout. Native libraries changed after that evaluation. Follow the current
[source-preview instructions](docs/RELEASE.md) for fresh builds and checks;
historical receipts do not certify a replacement library build.

The selected compiler is Rust 1.98.1. A Homebrew `rustc`/`cargo` on PATH can
bypass the rustup project pin. Put the
selected toolchain first for builds and tests:

```sh
export PATH="$HOME/.rustup/toolchains/1.98.1-aarch64-apple-darwin/bin:$PATH"
rustc --version
cargo test --offline --locked
cargo run --offline --locked -- contracts
cargo run --offline --locked -- inspect-map /path/to/acquisition.map
```

`inspect-map` reads only the specified map. Its byte counts use ddrescue input
coordinates. They do not authenticate a pairing, inspect an image, or establish
recovered video. Build the pinned native helpers locally before synthetic recovery tests:

```sh
native/build_helpers.sh
cargo run --offline --locked -- --help
```

`start` requires explicit filesystem/input offsets and an operator image/map
pairing assertion. It accepts fresh case/output destinations and automatically
exports independently validated unchanged MP4 files and supported conservative
segments. `resume` adopts completed
work and continues pending work; `export` also continues pending work and
publishes validated outputs; `inspect`, `pause`, and `cancel` operate on the
same durable case. Cancelled cases reject resume. Inspect lists stored receipts;
`payloads_reverified=false` means it has not freshly read exported bytes.
Each artifact reports separate evidence claims, original-file preservation, audio
presence, and recorded transforms. Segment headers are synthesized; presentation
timing remains conservatively qualified, and missing audio/gaps stay explicit.

Default limits are 24 hours per image including a 60-second shutdown reserve,
512 MiB managed memory, an 8 GiB admitted source/validation read allowance,
and 8 GiB generated output. These are bounded development
limits, not a qualified full-image throughput configuration. Acquisition gaps,
unsupported media, ambiguous ownership and exhausted work remain explicit
unresolved results. No real-image run is authorized by these examples.

Discovery reports its explicit `allocated-reachable-native-names-v1` namespace.
TSK synthetic virtual files/directories are counted separately; real files with
matching names remain in scope. Deferred or unreadable real directories remain
unresolved. Namespace completion does not establish whole-image video coverage.
Directory cursors, source-dependency archives and geometry indexes share durable
case authority; resume adopts sealed batches instead of restarting a root walk.

Start-time limits can be set with `--total-seconds` (2..86400),
`--shutdown-reserve-seconds`, `--read-bytes`, `--memory-bytes`, `--index-bytes`,
`--output-bytes`, `--decoder-seconds`, `--ledger-bytes`, and
`--view-index-bytes`. Values are decimal bytes/seconds and remain fixed for the
case. `--checkpoint-history` keeps 0..64 historical frontiers in addition to
current; the default is two. More history increases bounded metadata work.

Reported resource totals are conservative admissions. Image-read counters record
bytes delivered by the range engine; neither counter measures physical device
I/O or its cache behavior. SQLite pages and checkpoint metadata are bounded by
case quotas, frontier sizes/history and elapsed time, with physical I/O
qualification still pending.

## License

Project-owned code is licensed under **GPL-3.0-or-later**; see [LICENSE](LICENSE)
and [GNU's GPLv3 text](https://www.gnu.org/licenses/gpl-3.0.html).
[Third-party notices](THIRD_PARTY_NOTICES.md) identify dependencies and separate
license files. This grant does not relicense preserved third-party research
reports, quoted sources, dependencies or other third-party material.
