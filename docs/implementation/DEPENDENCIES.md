# Dependency decision record

Inspected 9 November 2026. M0 contracts use the Rust standard library; M1/M2 now pin the hashing and SQLite dependencies below. Required Cargo packages were fetched to the existing user Cargo cache for project builds. No global or system Python packages were installed. Filesystem/decoder adapters remain candidates until their behavioral gates pass. This record does not approve shipping installed binaries.

## Rust

Live PATH resolves Homebrew `rustc`/`cargo` 1.99.1, while rustup's pre-existing default is 1.97.1. The independently available `~/.cargo/bin/rustc +1.98.1` and `cargo +1.98.1` reported 1.98.1. Pin and invoke one installed stable toolchain consistently rather than assuming the PATH executable honors `rust-toolchain.toml`; record the actual compiler in test receipts. Rust standard-library licenses are MIT/Apache-2.0 under the [upstream licensing policy](https://www.rust-lang.org/policies/licenses). No third-party Rust crate API is adopted by this M0 record. Consult version-matched local standard-library documentation and current primary documentation when adding APIs.

## Filesystem adapter candidate

`/usr/local/bin/fls -V` reports TSK 4.14.0. Installed `/usr/local/include/tsk/base/tsk_base.h` declares `TSK_VERSION_STR "4.14.0"`. The installed `tsk_fs.h` marks `TSK_FS_ATTR_RECOVERY=0x80`; run `offset`, `addr` and `len` use filesystem blocks, file-walk `a_off` and `a_len` use bytes, and callback `a_addr` is valid only when RAW is set. SPARSE and FILLER flags are distinct. These are inspected header contracts, not a certified Rust ABI or proof of ownership.

Multiple `libtsk.22.dylib` and `.23.dylib` files exist alongside the unversioned library. At the initial M0 assessment no helper had been linked or ABI tested. Subsequent [ABI/cache probes](TSK_PROBE.md) match the 4.14.0 headers with explicit `.23`; the co-installed `.22` actually reports 4.15.0-develop and is rejected. The [isolated callback helper experiment](NATIVE_HELPER.md) now normalizes byte/block units, charges native metadata/prefetch through acquisition masking and preserves recovery/sparse/filler semantics. Its selected synthetic M3 contract is under review; broad filesystem support and OS sandbox/RSS qualification are not implied. Do not use public API documentation labelled 4.3 to certify installed 4.14.0.

Context7 lookup for “The Sleuth Kit” on this date returned unrelated libraries; none was used. The [upstream license overview](https://www.sleuthkit.org/sleuthkit/licenses.php) identifies per-file licenses, primarily IPL/CPL for core code; inspect selected source licenses before redistribution. [TSK source](https://github.com/sleuthkit/sleuthkit) and installed pinned headers take precedence for actual candidate API qualification.

## Decoder candidate

Installed `/opt/homebrew/opt/ffmpeg-full/bin/ffmpeg -version` reports 9.0.2, libavcodec and libavformat 63.1.102. Installed version-major headers report 63. The build enables GPL and version3 plus libx264/libx265 and many other libraries. No decoder policy, subprocess limits, sample comparison, virtual seek bridge or media profile has been certified. Full build configuration and binary digest must enter later qualification receipts; executable version alone does not identify a reproducible build.

Prefer an isolated process over in-process C linkage for hostile media. Initially materialize only selected validation subsets within quota; a pipe cannot implement arbitrary seekable MP4 views. Strict decoding must be full-interval, resource bounded, and independently classified for warnings/errors/timeouts. Consult [FFmpeg documentation](https://ffmpeg.org/ffmpeg-all.html) within the pinned executable's supported options before implementation. The [upstream licensing page](https://ffmpeg.org/legal.html) distinguishes default LGPL from GPL-enabled builds; this installed GPL/version3 configuration cannot be described as LGPL-only. Redistribution remains outside current scope.

## Ledger and hashes

M1 selects `sha2 = "=0.10.9"`, already cached locally, for strong source/sample SHA-256. The orchestrator verified that exact installed crate's `Digest::update`/`finalize` API and `MIT OR Apache-2.0` manifest license; Context7's `/websites/rs_sha2` documents the same incremental API but is not version-pinned, so exact local source resolves that version uncertainty. No ad hoc checksum substitutes for cryptographic identity.

M2 selects `rusqlite = "=0.41.1"`, default features disabled, with `bundled`. Its MIT license and Rust 1.89 minimum fit the selected Rust 1.98.1 toolchain. Current primary [rusqlite documentation](https://docs.rs/rusqlite/0.41.1/rusqlite/) and Context7 examples establish `Connection::open_with_flags`, immediate transactions, bounded busy timeouts, prepared parameter binding and optional rows. The exact local crate source resolves unsigned SQLite conversion details: counters must be checked into SQLite's signed 64-bit representation.

The actual lockfile chooses `libsqlite3-sys 0.39.1`, whose bundled source header declares **SQLite 3.54.1**. A Context7 README snippet described 0.39.0/3.54.0; the actual locked header is authoritative for this build. The selected C source is bundled and SQLite's core is public domain under its [upstream copyright statement](https://sqlite.org/copyright.html). No installed TSK/FFmpeg library is linked through this choice.

The ledger configures foreign keys, DELETE journaling, synchronous EXTRA and fullfsync, checks its application/schema identity before adopting a database, bounds database pages and lock waits, and uses immediate transactions for semantic updates. Database page quotas exclude the rollback journal; the future case space authority must reserve journal and checkpoint overhead. Fault tests establish transaction rollback/reopen behavior, not storage power-loss certification. Dependency choices are recorded in Cargo.toml/Cargo.lock; unselected packages are not a reason to install exploratory dependencies.

## Build environment refresh, 15 November 2026

Homebrew replaced `ffmpeg-full/9.0.2_1` with `9.0.2_2`; the earlier Cellar path no longer exists. Native builds, package library fingerprints and codec-profile controls now use the same `/opt/homebrew/opt/ffmpeg-full` prefix as the existing media fixtures. The prefix currently resolves to `../Cellar/ffmpeg-full/9.0.2_2`. This changes dependency discovery, not the accepted media API: the native helper still requires FFmpeg 9.0.2 and matching header/runtime library versions. Package manifests continue recording actual library hashes and dynamic dependencies. Earlier build fingerprints and qualification reports remain historical evidence; the replacement libraries require fresh checks.
