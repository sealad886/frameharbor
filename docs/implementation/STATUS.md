# FrameHarbor implementation status

Updated: 9 November 2026. Historical planning reviews certify no implementation.

## Ordered checklist

- [x] M0 contract gate: typed evidence, checked coordinates, resource/terminal contracts, seven-row MP4 acceptance matrix, synthetic ground-truth specification, metric denominators; independent review completed. Licensed clips, real-device profiles and external recording holdouts remain pending before broader media qualification.
- [x] M1 selected acquisition contract gate: read-only ranges, explicit masks/binding, observed source generations, bounded synthetic pilot and storage observations independently reviewed. Serial case/source admission is now integrated; host-wide aggregate RSS remains unqualified.
- [x] M2 selected serial foundation gate: semantic ledger, integrated case/read admissions, durable controls/checkpoints, quota rollback and synthetic-only publication protocols independently reviewed. Selected unchanged-original media validation/publication is now exercised under M4/M6; broader profiles remain pending.
- [x] M3 selected synthetic extent gate: FAT32/exFAT valid contiguous/fragmented mappings match retained bytes/spans; deleted/recovery/bitmap/conflict/hole cases remain qualified; five actual malformed filesystem mutations and native hang/failure/cancellation stay bounded and reaped. Broad hostile-input/RSS/OS confinement remains unqualified.
- [x] M4 selected unchanged-original gate: bounded ordinary MP4 mapping, independent staged byte/timing/configuration comparison and strict full decode pass exposed AVC/HEVC/AAC fixtures; independent parser/publication review completed. The selected M5 path now exercises rewritten headers and dependency-safe intervals; contiguous carving and broader table/edit profiles remain unqualified.
- [x] M5/M6 selected synthetic gate: damaged-target availability, conservative closed-IDR segments, selected fMP4 transformations and ordinary durable job controls pass source/output authentication, strict decoding and controller checks. The full declared profile matrix remains unqualified.
- [ ] **Active — M7:** bounded source-dependency archives, durable paged discovery and scoped probe batching; selected software integration now passes; representative performance qualification remains pending.
- [ ] M10: bounded prospective procedural evaluation passed seven controls from two original configuration families; external camera/consumer acceptance and full-matrix qualification remain open.
- [ ] M11: local host-dependent package built and verified; controlled real-image rollout remains separately authorized work after the required gates.

M8/M9 uncertain reconstruction and raw Annex B R1 remain disabled extensions.

## Implemented foundation

`contracts.rs` preserves six separate claims and forbids decode success from establishing original bytes, identity, order, timing, audio, ownership or completeness. It includes checked ranges and coordinate transforms, metric denominators, cumulative budgets, resource accounting and lifecycle transitions. The schema draft aligns with these claims. No private media is needed for contract examples.

`acquisition.rs` parses bounded GNU map status/block records, rejects overlapping/overflowing rows, and preserves gaps as unavailable. It reads regular image files with read-only handles, explicit binding/relocation and acquisition masks. Nonfinished/unknown bytes never enter accepted read receipts. SHA-256 consumed-range dependencies and immutable observed-generation manifests distinguish observed source identity from a full-image digest. Before/after identity checks detect ordinary source/map edits and replacement; they do not authenticate unobserved bytes. Revalidation rechecks consumed bytes under the same read authority and reports its cost separately.

Each image read and revalidation checks fresh cumulative processing time and shutdown reserve before scheduling. Map intake wall time belongs to that allowance; map metadata byte counts are reported separately from payload/revalidation reads. Per-request allocation and source-read/receipt caps are enforced. Returned receipt buffers belong to the caller; a future scheduler must account for aggregate in-flight buffers and metadata allocations rather than claiming that this library alone enforces host RSS.

The pilot selects deterministic nonoverlapping distributed windows from available acquisition intervals, caps work at ten minutes/8 GiB, and reports eligible/domain byte denominators and observed deadline overrun. Blocking OS I/O can exceed the target. No ETA or whole-image throughput claim follows from this fixture pilot.

`storage.rs` obtains a bounded destination free-space observation and filesystem identity comparison. Physical device topology remains explicitly unknown: different filesystems may still share a disk. Free space is not a reservation.

`ledger.rs` provides a bounded SQLite semantic work store with canonical stable IDs, guarded transactional transitions, append-only receipts, idempotent accounting, immutable index registration and source-generation invalidation. Ordinary job/controller integration uses this same authority.

`checkpoint.rs` durably stages and renames checksummed immutable frontier manifests, with current plus two histories by default. They share payload hashes rather than copy payload files. Interrupted staging is reconciled or preserved as bounded failure evidence; corrupt committed history blocks reuse. Synthetic fault injection verifies post-flush/rename boundaries and partial staging recovery. These are single-writer components; storage power-loss behavior is not certified.

`control.rs` centralizes durable per-image processing time, serial resource admissions and per-phase totals in the semantic ledger. Pause/resume preserves consumed work; cancelled cases reject new work. Repeated admissions are explicitly distinguished from permission to execute again. Checkpoint links reopen and verify committed frontier hashes. Shutdown uses a separate bounded reserve admission. Active crash downtime is conservatively charged; backward clocks fail closed.

`session.rs` now owns the source, durable controller and monotonic runtime clock. An advisory lease prevents concurrent live sessions from violating the serial memory envelope. Reads receive fresh durable admissions before I/O, source changes immediately invalidate dependent work, resume rehashes checkpointed dependencies, and admitted source metadata limits prevent map growth/fanout from escaping preflight bounds. Conservative managed-memory estimates include source metadata, dependencies, payload and checkpoint working buffers. Stop controls reject new work even if checkpoint storage fails; that failure is returned separately. An initial crash before the first frontier adopts no uncheckpointed results and preserves prior charges. Equal completed logical reads produce identical frontier bytes/hash across uninterrupted and resumed runs. Initial source identity still requires bounded bootstrap intake; its original start wall time and map cost are persisted before an operational session is returned.

`publication.rs` provides one canonical `Publisher` for explicitly typed synthetic fixtures and sealed `ValidatedArtifact` media. It verifies the reopened flushed stage, persists an intent, creates the final name without overwriting through a same-filesystem hard link, flushes the directory, then commits published state. Uncommitted artifacts never appear through normal enumeration. Media identity binds source generation, filesystem owner and payload hash. Pending adoption requires the current source; committed portable media remains inspectable after source loss. Bounded receipts retain complete file extents including headers, source/map/binding facts, consumed hashes, six separate claims, helper fingerprint/options, counters and elapsed validation time. Shared case admission charges media copying, verification and storage alongside publisher quotas. The sealed token distinguishes unchanged originals from sample-preserving transformations. Transformed receipts retain original sample ordinals, physical spans, configuration hashes and source/output timestamps; synthesized headers, repaired structure, constrained presentation and audio/coverage gaps remain explicit. Full transformation identity participates in publication identity. Reconstructed headers or timing cannot obtain unchanged-original claims. Physical power-loss behavior remains unqualified.

The memory-only native [TSK ABI/cache probe](TSK_PROBE.md) passed against the explicitly selected installed 4.14.0 headers/library. The [native helper](NATIVE_HELPER.md) and Rust filesystem adapter now exercise FAT32/exFAT fixture extents through acquisition-controlled byte IPC; they accept no native image path. Darwin uses current virtual-map size plus 64 MiB additional mapping allowance, not a fictitious absolute RSS cap. Actual child kill/wait receipts verify reaping under timeout, controlled native exit, malformed protocol and cancellation. The optional OS read-allowlist sandbox experiment remains failed/unqualified and disabled; no exploitation-resistant OS sandbox is claimed.

`bmff.rs` now maps supported ordinary MP4 sample tables through bounded source/staged readers. Eight generated AVC/HEVC, head/tail movie-box and CFR/VFR profiles with AAC match independent per-packet positions, hashes, DTS/PTS/duration and codec configuration. A separate integration compares actual fragmented FAT32/exFAT images against independent physical/sample truth, including four-byte encoded NAL prefixes split at 16 KiB and 128 KiB cluster boundaries. These are exposed development fixtures, not blind accuracy or real recordings.

`native/ffmpeg_helper.c` passes the selected synthetic envelope against installed FFmpeg 9.0.2/libavformat+codec 63.1.102/libavutil 61.1.102. Custom seekable byte IPC accepts artifact length, with no source path or descriptor. Independent packet/configuration hashes and timing accompany strict full decoding: one decoder thread, concealment disabled, corrupt/error/warning rejection and complete draining. Shared native limits and the canonical Rust supervisor retain the declared process envelope. `validation.rs` independently reparses the actual staged file and compares samples, positions, DTS/PTS, durations and configuration before checking native packets and fully drained frames. Fresh source/file checks and a whole-stage hash protect its private validated token. Bounded metered helper fingerprints detect ordinary binary drift, not binary attestation.

`jobs.rs` and `runner.rs` now connect normal commands to the same Ledger, case controls, checkpoints, a durable directory queue, global geometry index and publisher. Completed END_FILE observations are durably retained as provisional candidates; only complete conflict-qualified discovery within the explicitly selected allocated reachable namespace authorizes media reads. A completed index and committed output receipts are adopted on resume. Unsupported ownership, size and container features remain explicit unresolved work. Index storage quota is separate from actual serialized read/write admissions; read-only adoption does not consume index-write quota. Native end-to-end tests exercise exact output, restart adoption, exclusive case creation, pause/cancel, unsupported input and source-invalidated portable receipt inspection. Inspection uses read-only database handles and stored receipts without asserting current payload existence or a new hash verdict; active execution performs budgeted fresh verification.

`fragmented.rs` shares the canonical bounded box/header/payload parser with ordinary MP4. The selected profile resolves explicit or moof-relative data bases, required decode times, trex/tfhd/trun defaults, signed offsets/composition times and multiple tracks/runs inside one established logical file. Five focused tests and independent review pass. Missing initialization, unsupported implicit bases, sequence discontinuities and timestamp gaps/resets remain unresolved; repeated IDs/configuration cannot join files. This parser cannot produce a playable token. Selected closed-IDR segment planning, Rust header synthesis and corresponding independent validation/publication now pass focused checks; the full declared profile matrix and larger envelopes remain pending.

The executable now exposes start, inspect, pause, cancel, resume and export alongside contract/map inspection. The initial controller automatically exports selected validated unchanged files; export continues pending work through that same authority. Native TSK/FFmpeg checks qualify only the stated synthetic envelope, not broad hostile-input or real-media operation.

## Verification and limitations

Historical integrated gates passed 96 tests after M3, 165 after selected media/controller integration and 170 after portable epoch retirement. Those counts precede the current archive/paged-discovery/scoped-probe work and do not certify it. Current focused checks and remaining gate are recorded below. Actual bounded `SQLITE_FULL` tests prove atomic state/resource/phase rollback while preserving the committed frontier; they do not fill the host disk or certify physical power loss. Native helpers compile with C warnings denied. Filesystem fixture tests cover 25 applicable profile/layout/damage combinations; actual sparse allocation is disclosed in [the fixture record](FILESYSTEM_FIXTURES.md).

Initial tests selected Cargo 1.98.1 but inherited Homebrew rustc 1.99.1. Final checks explicitly prepend the installed 1.98.1 toolchain directory to PATH for both compiler and Cargo; this difference is material and retained here. No packages were installed globally. Dependency fetches required by the SQLite skeleton are recorded in its decision evidence.

The checkout remains unborn `main`, with user planning material preserved. No commits, external publication, releases, worktrees, real-image reads, real source/map mutations, private-media access or other-project recovery work occurred. Owned synthetic publication fixtures are exercised locally. No synthetic foundation proves MP4 recovery accuracy, real-image yield, blind acceptance or full-scale 24-hour performance.

Live source dependencies remain capped at 1,024 unique geometries. Immutable content-addressed dependency blocks are linked into durable checkpoints before live records are cleared; resume streams their verification under the same source/read authority. Archive references also survive in portable media provenance. The archive envelope is bounded at 1,024 blocks/64 MiB, rather than unlimited case growth. Directory inode/entry cursors, immutable batch receipts and a same-Ledger geometry index replace the former whole-discovery file cap. Per-batch run and allocation budgets stop before an unreported file and resume it with a fresh bounded budget. A file too large for one allocation-check envelope advances with explicit unchecked evidence. TSK still materializes one directory listing, so extreme flat-directory memory/reopen cost remains qualified. The discovery domain is allocated reachable native names; orphan/deleted/unreadable namespaces are not silently treated as searched. Geometry visits have a separate durable total cap; optional residual-search postings/transitions remain disabled.

Unsupported media probes now use bounded CPU permits and atomic batches of work links/outcomes. Every actual source read still receives durable byte admission. A completed pending page is excluded by SQL on resume; pending outcomes remain bounded to 128 and replay after interruption if not committed. CPU batches close before checkpoint/archive/output work; read epochs retain or retire their evidence through the explicit source authority. Independent CPU/reader/Ledger review found no blockers. The 1,100-entry controller regression includes 1,099 nonzero unsupported files followed by valid media; its pre-batching baseline passed in 509.95 seconds. The matched post-batching run passed in 119.56 seconds (about 4.27× faster), including exact output and completed-work adoption on resume. This is a correctness/overhead pilot, not representative throughput.

`performance.rs` adds a bounded synthetic pilot measurement harness around the canonical runner. It records observed wall time, actual source-read counters, source logical/allocated bytes, case/output allocation and decoder-helper RSS receipts (excluding filesystem/parent RSS). Allocation walks reject symlinks, cap entries, deduplicate hard links and overlapping roots, and ignore `.DS_Store` without deleting it. Its allocation-contract test passes. No representative hardware run, large allocated image, throughput ETA or complete-run deadline qualification follows from this harness. Normal CLI start/resume/export intake, path/helper-pin preparation, Runner case/Ledger setup, and resumed checkpoint-history opening now propagate their original start Instant into the same durable processing clock. The post-run allocation observer remains outside durable worker processing accounting and cannot establish complete-pilot deadline qualification.

Selected M5 transformation tests now materialize closed-IDR sample-preserving ordinary MP4 segments, independently reparse source/staged mappings, strictly decode actual bytes and persist original sample identities with synthesized-header, constrained-presentation and audio-gap labels. Ordinary acquisition-hole and fMP4 source tests pass at this boundary. These are not unchanged-file or whole-recording claims. Independent validation review exposed forged audio-omission and dependency-group metadata gaps; fixes and regressions pass. The full controller acquisition-hole test initially failed during TSK discovery because its speculative 64 KiB image-cache read crossed unavailable payload. The regression now passes with bounded acquired-prefix replies: native code receives only actually acquired bytes and records the shorter read; missing requested metadata remains fatal. The original failure identified a real integration gap that lower-level supplied-geometry tests had hidden. The selected prefix/malformed regressions pass in the final integrated gate; broader hostile-input qualification remains open.

Source read accounting now deduplicates repeated exact geometry/hash dependencies while charging every actual read. Nonzero acquisition relocations report unavailable gaps in image coordinates. Sealed portable-work retirement preserves discovery dependencies and byte/time charges, requires a fresh committed-artifact check plus same-Ledger typed links/outcome, and removes only new ranges whose hashes are durably retained in the artifact receipt. A 900-range epoch followed by 900 distinct new ranges passes under the live 1,024-range cap. Discovery source dependencies are checkpointed before final index adoption; work reads are checkpointed before completed outcomes. Failed or uncovered epochs retain their dependencies. Post-epoch integration passed 170 tests before archive/paged-discovery changes. The final complete selected software gate is recorded below; prospective independent results are recorded below.


Latest focused checks after scoped CPU/outcome batching: six selected Runner tests passed (43.99 s), corrupt sample-count continuation passed separately (5.96 s), committed-original crash-prefix adoption restored its typed portable link before outcome (6.66 s), and all-target Clippy passed. Native allocation-budget batching passed filesystem 13/13 and batch 8/8, including three actual contiguous 16 MiB files split into two plus one, all allocation-checked, and an oversized 40 MiB file that advances explicitly unchecked. These historical focused results supplement the final integrated gate recorded below.


Local packaging preparation adds `scripts/package_local.sh` and operator instructions in `LOCAL_PACKAGE.md`. The script builds only a local host-dependent development bundle, verifies hashes/help/ABI, records external native-library requirements and fingerprints, and refuses existing destinations. Independent review corrected directory-adoption races and ensured the shipped native rebuild inputs include `limits.h`. Shell syntax and existing-destination preservation checks pass. The local package is now built at `target/local-package`; its frozen executable SHA-256 is `c4b50a08b0b044627330f22e7f3dc144e716399248245e0d5dc29004695312b3`. Independent prospective evaluation and the final selected synthetic integrated suite passed within their stated scope. Bundled docs are a pre-evaluation snapshot; current closeout/evaluator receipts remain outside the immutable candidate. No installation, signing, notarization, publication or real-image execution is authorized by this preparation.


Four exposed one-second mathematical codec controls (AVC/HEVC at 1280×720 and 1920×1080, stereo AAC 48 kHz) pass actual staging, independent output parsing and full strict native decoding under existing limits. The focused run passed in 8.95 seconds; independent fixture/test review found no narrow-contract blocker. [Codec control evidence](CODEC_CONTROLS.md) retains exact byte/read/time/decoder-RSS observations and source fingerprints. These synthetic direct-layout cases do not qualify filesystem mapping, cameras, physical I/O, long media or 4K; the 4096×2160 admission ceiling remains unproven. Full M7/M10/M11 qualification has not advanced.


Final startup-clock integration includes caller intake and resumed checkpoint opening in the durable processing allowance. The source budget is seeded from startup elapsed time without adding it twice. Independent review verified Clock/control/source-budget alignment. Public Runner future starts are rejected before creating destinations (focused negative test: 0.19 s); an independently rerun 250 ms caller-intake test passed (0.35 s). A resumed-case test with 250 ms of real caller intake after its prior durable pause passed (2.75 s), preserving prior elapsed time; startup backdating across that prior pause correctly fails closed as ClockReversed. These software checks close the observed startup accounting window without qualifying hardware deadlines. The algorithm source remains frozen after the selected integrated gate for prospective independent evaluation.


Closeout preservation audit: all 16 historical planning files still match the byte lengths and SHA-256 values in `docs/planning/MOVE_RECEIPT.json` (978,846 bytes total). The audit read only their current FrameHarbor paths. The acceptance matrix now includes a selected-development test crosswalk for each damage row; original full-corpus specifications remain distinct from partial exposed-test coverage. The final integrated-suite verdict is now recorded below; the narrow prospective evaluator verdict is recorded below. Full M7/M10/M11 remain open.


Final frozen-source software gate: actual Rust 1.98.1, offline locked, **212 tests passed, 0 failed, 0 ignored**, across 28 test targets including empty unit/doc targets. Runner passed 12/12 in 142.92 seconds within the integrated run. The standalone matched 119.56-second namespace pilot remains the comparative timing; concurrent suite timing is not substituted. `cargo fmt --check` and all-target Clippy with `-D warnings` passed. Release build, native ABI/helper build, executable-help smoke and package checksum verification passed through `scripts/package_local.sh` (exit 0). This qualifies the selected synthetic software gate only.

Retained integrated log: `target/verification-final/frameharbor-final-integrated.log`, SHA-256 `50c335bfaa90f75b6814f3ba5028d8d2bed70943881a391cd7d85e176f133fe1`. Package build log: `target/verification-final/frameharbor-local-package.log`, SHA-256 `8cdb472c35960d9ac6b950c437756b84ff06949831a7222b539511f4d7f28cc5`. Both hashes were reread during documentation closeout. Prospective independent evaluation passed within the bounded scope recorded below; broad hardware/camera/consumer acceptance and real rollout remain unqualified.


[Independent prospective evaluation](PROSPECTIVE_EVALUATION.md) passed **7/7 controls from two new original configuration families** against the frozen package: four intact AVC/HEVC × FAT32/exFAT cases yielded byte-identical files with independent packet/configuration, raw/edited rational timing and audio checks; two acquisition-missing **container metadata** controls yielded no MP4 and explicit unresolved reasons; an overlapping map was rejected. Four actual published outputs also passed independent full strict decoding. Filesystem/damage variants are correlated, so seven controls are not seven independent recordings or a statistical false-join estimate. No engine defect, fix or retuning followed this evaluation. Raw receipt SHA-256: `1933d4848920e1dbf4adc4fa8afd7224c8491a941235795e22a96ad129f8bc71` under `target/prospective-evaluation/run-20261109T220255Z/receipt.json`; truth/results are now exposed regression material.

The evaluated executable/helper/build identities matched their frozen values before and after evaluation. Post-evaluation verification passed all 29 entries in the package SHA-256 manifest and the explicit TSK 4.14.0 header/runtime ABI probe. The immutable package retains its pre-evaluation documentation; this current documentation and evaluator report are the closeout addendum outside it. No package rewrite, installation or external release occurred. Selected bounded implementation, local builds and procedural evaluation are complete; representative M7 hardware, full external/consumer M10 acceptance and separately authorized M11 real-image rollout remain open.

## 15 November 2026 — source-preview alpha preparation

The user approved `0.1.0-alpha.1`, GPL-3.0-or-later for project-owned code,
milestone commits, and preparation for GitHub publication. These permissions
supersede the earlier session's no-commit/no-release boundary for that scope;
they do not authorize a real-image recovery run. Historical planning and provider
documents remain unchanged. Signed initial milestone
`31058c3c3ba0f61582f22c0290c7c7ee19f59a3b` preserves the handoff. Current work uses
`codex/alpha-preview`; the verified publication destination is
[sealad886/frameharbor](https://github.com/sealad886/frameharbor).

Independent review found a resumability defect when checkpoint history was zero:
publishing a new CURRENT could prune the prior manifest before SQLite committed
its replacement link. The shared checkpoint/session boundary now retains the
prior linked frontier until settlement, then enforces configured history.
Corruption continues to fail closed. Two regression tests and 47 focused
checkpoint/control/session tests passed; a second review explicitly cleared that
repair boundary. Complete final-commit review remains separate.

The selected local alpha software gate passed **214 tests across 28 targets,
0 failed and 0 ignored**, under the selected Rust 1.98.1 offline locked toolchain.
Formatting, all-target Clippy with `-D warnings`, native/release builds, local
package smoke/checksum verification, executable version and source-identity
checks all returned zero. FFmpeg discovery now uses the installed `opt` prefix,
currently `9.0.2_2`; displayed media API versions remain pinned, and actual
external-library fingerprints were refreshed rather than inheriting the earlier
build's evaluation. These results are selected software evidence, not full M7,
M10 or M11 acceptance.

Raw logs are retained under
`target/release-prep/alpha1-verification-managed/`:

- `tests.log`: SHA-256 `53c08b2dc348ebf61799bcfc96f4fa4f65a309ce956627d8cfc1055e2745ac22`.
- `package.log`: SHA-256 `55bd6115dcb58063e51531259767fa8c40bebbc98d8b7dd5d2a9974e30365f96`.
- `results.tsv` and `verdict` record each completed gate's result.

The fresh local bundle `target/local-package-alpha1` has executable SHA-256
`3af52c066ade9f19125da0a1dafb871147572f62c968a20e759a7869ce5426e2` and build-manifest
SHA-256 `eee74990f81f4382330c03177117f7094f9f770b5e8bb876f4349345c7b3ab4a`. It contains
the separate project license, third-party notices and 51 third-party text files.
It remains a host-dependent local development artifact, with a pre-closeout
documentation snapshot, outside the source-only release assets. The earlier
evaluated package is preserved. All four listed digests were reread during this
documentation update.

See the [source-preview guide](../RELEASE.md) and
[release review ledger](RELEASE_REVIEW.md) for current exact-commit review,
hosted review, tag and publication state. Those outcomes remain pending at this
update; no local passing check is substituted for them. Representative hardware,
camera/full-matrix/user acceptance, supported installation and real-image rollout
remain open gates.
