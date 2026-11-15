# Local host development package

This package contains the FrameHarbor executable, native helpers, compiler/source fingerprints, dependency inventory and selected qualification notes. It is a local build artifact, not a release, real-image recovery authorization, portable standalone distribution or completion of M7/M10/M11 acceptance.

## Build and verify

From the project checkout on the selected Darwin arm64 host:

```sh
scripts/package_local.sh
```

The default destination is `target/local-package`. Existing destinations are preserved; choose a fresh explicit path for another build. The script uses installed Rust 1.98.1 and locked offline dependencies, compiles native helpers with warnings denied, runs their ABI probe, smoke-checks executable help, and verifies package-relative SHA-256 entries. Failed staging directories remain as local failure evidence. Builds do not install dependencies or copy third-party libraries.

Inside the resulting package:

```sh
shasum -a 256 -c SHA256SUMS
./bin/frameharbor --help
```

`BUILD-MANIFEST.txt` records exact executable dynamic-library requirements. The selected host needs TSK 4.14.0 at `/usr/local/lib/libtsk.23.dylib` and FFmpeg 9.0.2 libraries at the recorded Homebrew paths. These dependencies are external to the package. A different host, library build or loader layout needs independent verification; hashes detect ordinary artifact drift, not malicious-host modification. No signing, notarization, application installation or external publication is performed.

## Normal operation and recovery

The CLI runs directly without AI control. The packaged helper paths are discovered beside the executable. The following is a command template only; running real recovery requires separate authorization and review of actual input identity, image/map binding, storage and budget.

```sh
./bin/frameharbor start \
  --image /approved/path/image.img --map /approved/path/image.map \
  --case /approved/new-case --output /approved/new-output \
  --filesystem fat32 --fs-start 0 --input-origin 0 --attest-pairing
./bin/frameharbor inspect /approved/new-case
./bin/frameharbor pause /approved/new-case
./bin/frameharbor resume /approved/new-case
./bin/frameharbor export /approved/new-case
./bin/frameharbor cancel /approved/new-case
```

Offsets are decimal bytes. `--input-origin` maps the stated ddrescue input coordinate to image byte zero; filesystem start is independently image-relative. The pairing assertion must reflect actual evidence. New case/output destinations must not exist and need existing parents. Originals and maps remain read-only.

Start, resume and export continue pending work and automatically publish fully validated supported outputs. Pause preserves durable work. Cancel permanently prevents additional work for that case. Completed indexes, outcomes and publications are adopted on resume; source dependencies are reverified under the same authority. A checkpoint failure or source change is returned explicitly. Do not repair a case by editing its databases, frontiers or receipts. Preserve failures and use inspection to distinguish stored historical status from freshly verified payloads.

Defaults are a 24-hour processing allowance including 60-second shutdown reserve, 512 MiB managed memory, and 8 GiB admitted read/output envelopes. `--help` lists immutable start-time overrides. Reported admissions and delivered source bytes do not measure all physical device I/O. Allocation observations are not free-space reservations.

Namespace completion covers `allocated-reachable-native-names-v1`; orphan/deleted/unreadable scope and unsupported media remain explicit. Segment transforms, synthesized headers, constrained timestamps and missing audio remain labelled independently from strict decode success. Read `docs/implementation/SUPPORT.md` and `docs/implementation/STATUS.md` before interpreting output claims. Representative full-size hardware, real-camera media and independent consumer acceptance remain unqualified.


## Current evaluated-candidate boundary

The local package has been built at `target/local-package`. Its executable SHA-256 is `c4b50a08b0b044627330f22e7f3dc144e716399248245e0d5dc29004695312b3`. This candidate remained frozen through the [prospective independent evaluation](PROSPECTIVE_EVALUATION.md): seven bounded procedural controls passed, from two original configuration families. Its bundled documentation is the pre-evaluation snapshot. Current closeout documentation and evaluator receipts will be retained outside the immutable candidate; do not rewrite packaged files and invalidate its checksum manifest.


Package build and smoke verification passed with exit 0; the 212-test selected software gate, formatting and all-target Clippy checks also passed. The build log is `target/verification-final/frameharbor-local-package.log`, SHA-256 `8cdb472c35960d9ac6b950c437756b84ff06949831a7222b539511f4d7f28cc5`. This remains a local host-dependent candidate. Post-evaluation verification passed all 29 package checksum entries and the explicit TSK 4.14.0 header/runtime ABI probe. Independent procedural controls passed, while full hardware/camera/consumer acceptance remains open; no installation or release acceptance is implied.
