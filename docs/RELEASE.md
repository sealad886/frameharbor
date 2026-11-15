# FrameHarbor 0.1.0-alpha.1 source preview

This alpha provides source for the bounded evidence-driven MP4 recovery core.
It is not a declaration that the complete implementation plan passed acceptance.
The release intent is `0.1.0-alpha.1`, with tag `v0.1.0-alpha.1`. The review ledger
records actual commits, verification and tag/publication state; the version name
alone does not prove publication.

## Supported development environment

The exercised build environment is macOS arm64 with Apple command-line C tools,
Rust 1.98.1 and the selected native dependencies below. Other hosts and loader
layouts have not been qualified. Dependencies are installed separately; this
source preview contains no third-party library or compiled-helper assets.

| Component | Selected environment |
| --- | --- |
| Rust | `rust-toolchain.toml` pins 1.98.1; ensure both Cargo and rustc use it |
| Rust crates | Exact versions and source checksums in `Cargo.lock`; `sha2` 0.10.9 and `rusqlite` 0.41.1 with bundled SQLite 3.54.1 |
| TSK | 4.14.0 headers under `/usr/local/include`, library `/usr/local/lib/libtsk.23.dylib`; memory-only ABI probe must pass |
| FFmpeg | 9.0.2 at `/opt/homebrew/opt/ffmpeg-full`, currently Homebrew `9.0.2_2`; libavformat/libavcodec 63.1.102 and libavutil 61.1.102 |

The FFmpeg build enables GPL and version 3. Its upstream
[license guidance](https://ffmpeg.org/legal.html) and the
[third-party inventory](../THIRD_PARTY_NOTICES.md) describe the relevant license
boundaries. This inventory is not a certification of a future binary bundle.
TSK's [pinned license overview](https://github.com/sleuthkit/sleuthkit/blob/sleuthkit-4.14.0/licenses/README.md)
identifies per-file terms.

## Build and check

From the repository root, with the selected dependencies already installed:

```sh
export PATH="$HOME/.rustup/toolchains/1.98.1-aarch64-apple-darwin/bin:$PATH"
rustc --version
cargo --version
cargo fetch --locked
native/build_helpers.sh
cargo fmt --all --check
cargo clippy --offline --locked --all-targets -- -D warnings
cargo test --offline --locked
cargo build --release --offline --locked
target/release/frameharbor --version
target/release/frameharbor --help
```

`cargo fetch --locked` needs network access on a fresh checkout; omit it when
the exact lockfile dependencies are already cached. No dependencies are vendored.
Native helper builds run a memory-only TSK probe and do not open recovery images.
The C builds treat warnings as errors. Dependency fingerprints must be refreshed
when external libraries change, even if their displayed API versions stay equal.

For a host-specific local bundle, select a new destination:

```sh
scripts/package_local.sh target/local-package-alpha1
```

The script refuses existing destinations, records actual dependency/source
fingerprints and verifies checksums/help. This is an optional local development
artifact, not a portable release asset or an installation step. Keep the earlier
`target/local-package` evaluation artifact unchanged if it exists.

## Operate without an AI agent

Start requires an image/map coordinate assertion, explicit filesystem start and
fresh case/output destinations. Offsets are decimal bytes. In this template,
ddrescue input offset zero maps to image byte zero, and the filesystem also
starts at image byte zero. Use values established for the actual acquisition;
do not add a historical partition offset twice.

```sh
target/release/frameharbor start \
  --image /approved/image.img --map /approved/image.map \
  --case /approved/new-case --output /approved/new-output \
  --filesystem fat32 --fs-start 0 --input-origin 0 --attest-pairing \
  --filesystem-helper target/debug/helpers/tsk-helper \
  --media-helper target/debug/helpers/ffmpeg-helper
target/release/frameharbor inspect /approved/new-case
target/release/frameharbor pause /approved/new-case
target/release/frameharbor resume /approved/new-case
target/release/frameharbor export /approved/new-case
target/release/frameharbor cancel /approved/new-case
```

Use `--filesystem exfat` for the supported exFAT profile. Destination parents must
exist. Images/maps stay read-only. Start, resume and export continue pending work
and publish supported outputs only after full validation. Pause preserves work;
cancel permanently prevents further work in that case. Inspect reports stored
receipts and marks whether payloads have been freshly reverified. Preserve failed
cases and use a new destination rather than editing databases or receipts.

Defaults are 24 hours per image including 60 seconds of shutdown reserve,
512 MiB managed memory, and 8 GiB admitted read/output allowances. `--help` lists
start-time overrides. These development limits are not a measured complete-image
throughput envelope. Read counters do not measure all physical device I/O.
This template does not authorize the agent to launch a real-image recovery run.

## Evidence and limits

The selected core handles acquisition masks, allocated FAT32/exFAT extents,
supported ordinary `avc1`/`hvc1` plus AAC-LC mappings, strict full validation,
durable controls and conservative closed-IDR segments from surviving ordinary
or supported fMP4 tables. Six separate claims preserve provenance, ownership,
structure, decode, timing and coverage limits. Unsupported media and missing
evidence remain unresolved; decode success alone never proves an original
recording's identity, order or completeness.

The 9 November development suite passed 212 tests. A frozen older local package
also passed seven prospective procedural controls from two new configurations;
those variants are correlated and now exposed. See
[historical status](implementation/STATUS.md),
[support details](implementation/SUPPORT.md), and
[prospective evidence](implementation/PROSPECTIVE_EVALUATION.md).
The alpha needs fresh verification on its final commit and current dependencies;
the [review ledger](implementation/RELEASE_REVIEW.md) records it.

The 15 November local alpha gate passed 214 tests across 28 targets, with zero
failures or ignored tests. Formatting, all-target Clippy with warnings denied,
native/release builds, local package checksums, version and source-identity
checks passed against the current FFmpeg `9.0.2_2` build. A checkpoint retention
defect under `--checkpoint-history 0` was repaired: the prior linked frontier
survives interruption until the replacement link settles. Two regressions and
the 47-test focused checkpoint/control/session gate passed before the full run.

The fresh local bundle is `target/local-package-alpha1`, with executable SHA-256
`3af52c066ade9f19125da0a1dafb871147572f62c968a20e759a7869ce5426e2`.
Its manifest records actual source/helper/external-library identities; it now
includes the project license and all 51 separate third-party notice files.
The bundle's docs are the pre-closeout snapshot; this guide and the review ledger
add the current verification results without rewriting that tested artifact.
Final exact-commit review, hosted review and tag/publication readback remain
pending. These local checks do not complete the broader qualification gates.

Full M7 representative 63.28 GB/250.15 GB hardware/deadline measurements, M10
camera/full-matrix/user acceptance, and M11 installation/real-image rollout remain
open. No real-image yield, complete-image speed or universal recovery guarantee
is established. M8/M9 uncertain joins and raw Annex B remain disabled extensions.

## Source and licensing

The authorized GitHub destination is
[sealad886/frameharbor](https://github.com/sealad886/frameharbor). Its initially
empty remote has been verified; hosted review and publication remain pending.
The intended GitHub publication is a prerelease of source. It does not include
the ignored `target/` evidence, compiled helpers, native libraries or private
media. A public tag is not an installation test or a completed qualification
gate. Verify the authorized repository, exact tag commit, configured review
outcomes and hosted checks before publication.

Project-owned code is GPL-3.0-or-later under [LICENSE](../LICENSE).
[THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md) links each dependency's separate
license files. The project grant does not relicense third-party research reports,
quoted sources or dependencies. Preserved planning/provider documents remain
verbatim and historical; their review receipts do not certify this alpha.
