# Image-first video restoration: implementation and qualification plan

Status: design only, 2026-11-09. Every milestone below is planned, not implemented or accepted. This is a greenfield plan; it imposes no retrofit, migration, or compatibility obligation on the current implementation. It authorizes no source changes, recovery run, commit, branch, release, or mutation of existing media/cases.

Read [DESIGN.md](DESIGN.md) for algorithm and claims, [EVIDENCE.md](EVIDENCE.md) for source strength, [CONSENSUS_RESEARCH.md](CONSENSUS_RESEARCH.md) for the research response, and [IMAGE_INVENTORY.json](IMAGE_INVENTORY.json) for the bounded input inventory. Source-derived facts are centralized in the evidence document; engineering targets here require measurement.

## 1. Delivery contract

The first implementation accepts raw partition/disk images and ddrescue maps for the user's **primarily `.mp4` workload**. Ordinary MP4 drives the first release, with FAT32/exFAT, 16 KiB/128 KiB clusters, and supported AVC, HEVC, and audio profiles; ISO fragmented MP4 is a separate supported container profile. Raw Annex B AVC/HEVC is a secondary optional extension and cannot block MP4 acceptance. A donor/reference clip is optional, never another hard input requirement. Work is read-only at the source. The user budget is up to **24 hours per image, including intake, indexing, search, validation, and export**. Successful bounded recovery can finish with qualified partial results and durable unresolved work.

In every task and benchmark, distinguish filesystem-fragmented ordinary MP4 (noncontiguous disk allocation) from ISO fragmented MP4 (`moof`/`traf`/`trun`). Never substitute fMP4 or raw-stream success for ordinary MP4 qualification. The [MP4 damage decision matrix](DESIGN.md#60-mp4-damage-decision-matrix) defines the required paths for intact/damaged extents, surviving/lost `moov` tables, missing media, and optional-reference limits.

Scope is algorithm, evidence, resource control, recovery controls, and usable local results. A thin conventional CLI/local interface must operate without an AI agent. New formats/filesystems and richer user interfaces follow only after measured need. No generative repair, cloud upload, unrestricted fragment permutations, silent source merging, or claims that decoding proves original order are part of the initial product.

Milestones are ordered. A milestone advances only after its listed behavioral gate passes; failed in-scope checks return to implementation with the failing fixture and expected behavior. Allow at most three dev/QA attempts for one blocking defect before documenting the constraint and revising that milestone's proposed scope. Do not weaken exactness, source safety, or deadline enforcement to gain a pass. Independent review is required after M4, M9, and M10.

The first supported release follows **M0–M7 → M10 → M11**, covering filesystem/intact MP4 recovery, supported damaged-MP4/fMP4 segments, validation, publication, normal controls, and measured deadlines. M6's generic validation/control foundation starts after M2 and is integrated into each media milestone; useful exact MP4 recovery does not wait for uncertain matching. **M8–M9** add residual discovery and missing-layout hypotheses, then repeat the affected M7/M10 gates before enabling those features. Their incompleteness cannot block an otherwise qualified exact/conservative release or silently enable unqualified search. Raw-stream recovery is outside the mandatory MP4 release path.

## 2. Proposed implementation boundaries

Use one native Rust program with modules and narrow interfaces. Pin a stable Rust toolchain and dependency versions during M0; no current repository language/dependency constraint is assumed. Use SQLite for semantic jobs/evidence, compact immutable files for large indexes, and isolated FFmpeg processes for supported independent decoding. Evaluate a maintained read-only filesystem library/tool adapter against FAT32/exFAT fixtures before adoption. Exact package names/versions are implementation decisions gated by current API, license, portability, and malformed-input checks.

| Module | Responsibility | Public contract |
| --- | --- | --- |
| `acquisition` | Image/map binding, coordinate transforms, interval availability, source generations | `read(range) -> bytes + quality + source refs` |
| `filesystem` | Certified geometry and logical extent views | `discover`, `enumerate_media_views`, capability/limitation report |
| `bmff` | Bounded parse/write and logical sample maps | observations, placement constraints, exact table resolution |
| `codec` | AVC/HEVC/audio framing and supported dependency facts | access-unit candidates, config, random-access/dependency constraints |
| `discovery` | Sequential selective scanning and compact indexes | canonical candidate classes and saturation receipts |
| `assembly` | Exact reconstruction and bounded uncertain hypotheses | artifact plans, frontiers, cutoff reasons |
| `validation` | Provenance, structure, decode, timing, coverage checks | independent claim fields and reproducible receipts |
| `ledger` | Stable work identity, transactions, resume, budgets | idempotent work adoption and dependency invalidation |
| `publication` | Space reservation, staged outputs, verification, atomic publish/reconcile | final artifact only with committed validation evidence |
| `control` | Normal local CLI/interface and machine-readable reports | start, inspect, pause, cancel, resume, export |

Use buffered positional source reads and a seekable `VirtualFileView` over ordered spans/synthesized bytes. For decoder validation, qualify a narrow isolated FFmpeg/libavformat AVIO read/seek bridge against materialized controls, or explicitly limit validation to a selected materialized subset within quota. A simple pipe cannot supply arbitrary seekable discontiguous views. Small image-backed manifests avoid copying every useful candidate; portable exports are separate, budgeted operations. No FUSE/kernel component is required.

## 3. Ordered milestones and QA gates

### M0 — Freeze evidence contracts and ground truth

Tasks:

- M0.1 Turn the six claim dimensions in DESIGN into typed states and forbidden upgrades; define the exact class as original sample bytes, original logical sample identity/order/timing, and supported configuration.
- M0.2 Define source coordinate transformations, half-open checked ranges, acquisition status rules, physical geometry identity, content classes, and separate logical owners.
- M0.3 Define the 24-hour total budget, search/read/memory/index/output sub-budgets, terminal reasons, and consumer stop/resume behavior.
- M0.4 Build a synthetic image/capture corpus specification retaining original logical files and physical maps; acquire licensed redistributable clips and real-device examples for later holdout.
- M0.4a Make ordinary MP4 the principal corpus stratum and map every row of the MP4 damage matrix to positive, negative, or irreducibly ambiguous ground-truth fixtures; report fMP4 separately and raw streams only if enabled.
- M0.5 Select/pin toolchain and candidate libraries after API/license/security checks; document unsupported filesystem/codec features. Verify actual TSK 4.14.0 installed headers/ABI against the pinned build rather than assuming older online documentation matches; isolate native parser/decoder helpers.

Deliverables: machine-readable schema draft, acceptance matrix, fixture generator specification, profile support matrix, dependency decision record.

Gate: an independent reviewer can classify intentionally ambiguous examples without consulting a decoder score; every planned metric has a denominator; no fixture depends on private recordings. No implementation is counted as accepted yet.

### M1 — Acquisition-aware read-only range engine

Depends on M0.

- M1.1 Implement map parsing, whole/partition-image mapping, nonzero input/output translation, rescue domains, gaps, and invalid/conflicting interval rejection.
- M1.2 Implement bounded reads returning quality intervals; represent physical bytes, supported logical zeros, and unavailable logical spans distinctly.
- M1.3 Create immutable source-generation manifests, consumed-range hashes, change detection, and binding assumptions/limitations.
- M1.4 Discover physical storage topology and destination free space without changing source media; record uncertainty if topology cannot be determined.
- M1.5 Implement read counters and a deterministic stratified pilot capped at ten minutes or 8 GiB; first results must not wait for a whole-image hash.
- M1.6 Property-test coordinate/range algebra and fuzz malformed map input; assert bounded allocations and no reads outside authorized source ranges.

Tests prove: all map states, nonzero relocation, partition image/whole-device map mismatch, wrong same-size map, map/image length mismatch, map gaps, stale valid-looking media in unavailable regions, sparse-file semantics, range overflow, short reads, and image/map change during work. Include a partition image starting at byte zero whose exFAT boot field still stores its old 2,048-sector disk offset; adding that value again must fail the coordinate contract.

Gate: no unavailable byte enters an acquisition-supported output; unverified binding blocks that claim; no source write is attempted; observed read volume stays within declared work bounds. The supplied eight images are not needed for regression tests; synthetic image/map pairs cover these requirements.

### M2 — Ledger, budgets, checkpoints, and publication skeleton

Depends on M1.

- M2.1 Implement stable work IDs from source generation, semantic inputs, algorithm/schema version, and meaningful parameters. Exclude thread count, progress timestamps, and ETA from identity.
- M2.2 Add transactional state transitions, bounded single-writer batches, append-only receipts, immutable index-run registration, and dependency invalidation.
- M2.3 Enforce total wall budget and per-stage byte/CPU/space limits through one budget authority, including subprocess costs and shutdown reserve.
- M2.4 Implement pause/cancel/resume and compact frontier persistence without copying media or multi-GiB checkpoint trees.
- M2.5 Implement staged file publication, durable flushes, rename/ledger reconciliation, and disk-full handling.
- M2.6 Default frontier retention to current plus at most two historical manifests, configurable for more; share immutable payloads, compact replay events, retain final/failure/evidence receipts, and retire only unreferenced case-owned temporary generations within quotas.
- M2.7 Implement image-backed artifact manifests and their source-dependency checks; distinguish these from portable exports in ordinary controls.

Tests prove: crash recovery at every publication boundary (staged-content fsync, durable intent, file rename, directory flush, published commit), interrupted index writes, duplicate job submissions, source-generation mismatch, cancellation while a child process is active, disk full, corrupt checkpoint, resume adoption without duplicate artifacts, and repeated checkpoints/compaction obeying current-plus-two history without cloning payloads. Add fault-injection checks for lost/reordered unflushed writes and controlled power-loss testing where practical; process termination alone does not prove storage durability. Source loss/change must visibly break dependent virtual results rather than masquerade as portable files.

Gate: at equal completed logical work and byte/edge budgets, with no wall-deadline truncation, resumed and uninterrupted runs have equal persistent semantic results and artifact hashes. Deadline tests separately verify truthful stable partial results and cumulative budget accounting; they need not finish equal work. Final names never denote unvalidated artifacts. A no-op resume performs bounded manifest/index verification rather than a full rescan when a stable source generation supports adoption. Hash revalidation reads required by source uncertainty are reported separately and cannot be silently skipped.

### M3 — FAT32/exFAT logical extent recovery

Depends on M2.

- M3.1 Validate partition/boot geometry, FAT tables, root traversal, allocation structures, and metadata bounds. Treat observed boot fields as untrusted until consistency checks pass.
- M3.2 Enumerate allocated media candidates and supported deleted records; emit file sizes, stream/owner identities, ordered logical extents, and evidence strength.
- M3.3 Support FAT32 chains and exFAT contiguous/chain cases only when their metadata agrees; handle partial chains and conflicts explicitly.
- M3.4 Represent crosslinks, identical geometry with several owners, reused deleted extents, and distinct physical copies without global ownership assumptions.
- M3.5 Provide a logical reader that crosses cluster boundaries inside boxes, MP4 chunks, samples, and NAL length prefixes.
- M3.6 Separate backend metadata-backed extents from recovery/contiguity guesses; normalize callback units explicitly and account for native parser metadata reads through the acquisition mask.

Tests prove: 16 KiB/128 KiB cluster examples; fragmented files; intact/partial/deleted chains; exFAT `NoFatChain` behavior and authoritative bitmap allocation; disagreement between allocation evidence and stale directory entries; reused clusters; hardlink/alias-like owner representations where applicable; nonmedia/misleading extensions/zero-length entries; damaged metadata; source holes inside filesystem structures; byte/sector/cluster callback normalization; library recovery guesses not promoted to exact. Mutate filesystem records and chains under bounded subprocess/resource limits to expose parser fanout, loops, out-of-range reads, and recoverable native-helper crashes.

Gate: all supported valid extent views match fixture ground truth byte-for-byte; wrong/stale ownership is qualified, not promoted to exactness; unknown chain portions remain unknown. Before supporting APFS, NTFS, ext4, or other profiles, add separate adapter certification milestones rather than claiming generic coverage.

### M4 — Intact MP4 and exact table-driven recovery

Depends on M3.

- M4.1 Implement bounded BMFF parsing and supported table schemas with count/offset/overflow/data-reference validation.
- M4.2 Resolve chunk/sample maps lazily through logical extents, retaining original sample identity, sizes, DTS, PTS, durations, descriptions, sync/dependency facts, and supported edits.
- M4.3 Recover intact logical files and encoded-sample-preserving reconstructed files. Enumerate rewritten headers/tables and timestamp normalization separately.
- M4.4 Carve a proven contiguous container only after establishing its placement transform; do not reinterpret logical chunk offsets as physical addresses.
- M4.5 Export dependency-safe intervals when exact tables identify missing samples; preserve gaps and known coverage denominators.
- M4.6 Test surviving target table reconstruction under audio/video interleaving, B-frame decode/presentation reordering, VFR, audio priming/edits, and sample boundaries crossing filesystem clusters. Keep original-file-byte preservation distinct from encoded-sample-preserving remux.

Tests prove: head/tail `moov`; supported 64-bit offsets; multiple sample descriptions; interleaved audio/video; variable frame rate; composition reordering; edit/priming behavior within declared scope; samples spanning clusters; malformed/partial tables; external data-reference rejection; unchanged encoded payload. Add property/fuzz tests for box lengths, counts, nesting, table arithmetic, and writer/parser round trips within the supported representation contract; every fuzz run has memory/time/read caps.

Gate: every exact-class sample matches bytes, logical ordinal, timing, and configuration ground truth; the actual staged artifact is independently reparsed and compared against the asserted source sample map, not merely checked for decodability. Unsupported features remain explicit; all published playable outputs undergo complete validation. Independent QA challenges the exactness/ownership/timing labels before M5.

### M5 — Damaged MP4, fMP4, and independent segment salvage

Depends on M4.

- M5.1 Route ordinary MP4 with missing/damaged `moov` through the damage matrix, preserving surviving target tables and known logical payload order. Attempt only supported sample/access-unit salvage; classify unestablished timing, sample boundaries, audio, and placement as inferred or unresolved.
- M5.2 Make a reference clip optional and strictly limit its role to compatible codec configuration/layout priors. Reject donor-`moov` copying as proof of original sample counts/sizes/offsets/order/timing or synchronization; the target's evidence must establish those claims.
- M5.3 Enforce AVC/HEVC random-access and reference rules; preserve leading-picture/open-GOP uncertainty and audio synchronization gaps.
- M5.4 Implement the declared ISO BMFF fragment profiles, initialization/default resolution, data-base rules, offsets, durations, and composition timing.
- M5.5 Recover fragment runs tied to an established logical file. Keep track IDs, sequence numbers, timestamps, and codec hashes as constraints rather than universal recording IDs.
- M5.6 Recover isolated independently supported segments with missing initialization/configuration recorded.

Tests prove: missing/partial ordinary-MP4 `moov` and sample tables with/without an optional reference; mismatched donors that produce plausible but wrong sample/timing metadata; interleaved audio whose boundaries cannot be inferred from NAL parsing; separate fMP4 initialization, multiple `traf`/`trun`, default overrides, supported absolute/relative addressing, nonadjacent `mdat`, missing fragments/init, sequence wrap/reset, reused IDs/timestamps across recordings, HEVC CRA/leading pictures, and valid wrong-order fragments that still decode. Add supported configuration-declared NAL prefix widths and in-band parameter sets (`avc3`/`hev1`). Missing original metadata must remain missing or inferred even when a donor-assisted result decodes.

Gate: the system never autojoins unrelated recordings on repeated counters/configuration; independent segments remain useful; ordinary-MP4 table loss is not silently replaced by donor or codec assumptions; MSE profile checks are not mistakenly applied to every supported ISO BMFF file. Unknown original timing/audio cannot pass as exact restoration.

### M6 — Full validation and normal user operation

Foundation depends on M2; complete profile integration depends on M4/M5. It is implemented alongside those milestones and does not depend on M8/M9. The validation skeleton protects every earlier published output.

- M6.1 Record independent provenance, ownership, structure, decoder, temporal, and coverage verdicts with reproducible tool versions/options.
- M6.2 Validate every exported playable interval in full with strict settings and bounded subprocesses. Keep permissive preview separate.
- M6.2a Independently compare final staged encoded samples/ordinals/configuration/timing to the source sample map; verify declared timestamp/framing transforms and reject undeclared byte changes, repetition, omission, or reordering.
- M6.2a-raw Optional R1 only: for raw streams without container timing, compare ordered span/NAL/access-unit hashes and configuration against the declared plan; preserve unknown original timing and distinguish inferred playback timing.
- M6.2b Qualify the chosen virtual-result validation path: either a seekable bridge compared against materialized controls, or quota-selected materialization with byte equivalence to the reopened view. In both paths, compare payload/timing/configuration and completely decode that exact byte stream, including cross-extent seeks and synthesized metadata. Report dependency loss and prevent reads outside the declared view.
- M6.3 Provide ordinary start/inspect/pause/cancel/resume/export controls, clear result groups, and a local inspectable report.
- M6.4 Display scan scope, evaluated/skipped candidates, unresolved reasons, elapsed/remaining budget, estimated next cost, and output-space requirements.
- M6.5 Ensure report/preview/export operations preserve qualification and do not upload private data.

Tests prove: decoder errors/warnings/timeouts, unsupported profile, packet/frame count mismatch, missing audio, synthetic timestamps, output-space shortfall, source changes before publish, and restart from ordinary user controls. Inject validly decodable mutations, repeated samples, reordered samples, altered configuration, and wrong timing into staged files after assembly; artifact-to-source validation must reject each unsupported change.

Gate: a user can identify recovered results, uncertain order/ownership/timing, unsearched work, and cost of another bounded pass without reading raw logs or asking an agent. “Passed decode” and “complete original recording” remain visibly different fields.

### M7 — Performance pilot and deadline qualification

Depends on M6.

Qualify the currently enabled exact/segment features first; repeat affected measurements when M8/M9 are added. Candidate-index/search metrics are not applicable before those features exist, rather than fictitious zero-cost results.

- M7.1 Benchmark representative 63.28 GB and 250.15 GB image sizes with the observed cluster geometries, plus smaller/full-scale density stress cases. Synthetic images may be sparse for correctness tests, but performance tests must disclose allocated bytes and real reads.
- M7.2 Measure sequential baseline, read amplification, metadata yield, index bytes per image GiB, candidate-density percentiles, maximum posting cardinality, exact export rate, decoder rate by codec/resolution, CPU, RSS, and destination allocation. Optional R1 only: for raw streams with unknown timebase, use frames/access units or encoded bytes per second rather than guessed video duration.
- M7.3 Calibrate phase ETA intervals and remaining-space projections from actual measured work; include shutdown/checkpoint and validation costs within 24 hours.
- M7.4 Verify overload controls and useful early results on rotational storage, SSD, shared source/destination device, low free space, and CPU-limited HEVC decoding.
- M7.5 Run long pause/resume and deadline tests proving completed scans/indexes are adopted rather than rebuilt.
- M7.6 Report per-image and whole-family costs separately. Exercise a free-space envelope below the aggregate image volume, using duplicate-aware views and selective output quotas rather than full re-export.

Gate: pilot finishes within its cap; declared resource bounds hold; 24-hour deadline stops work with a durable truthful report. A minimum viable release envelope states which image/profile/hardware combinations meet the complete-run target and which yield partial results. No weeks-to-hours marketing claim is accepted from a microbenchmark.

### M8 — Residual discovery and selective index

Depends on M5 and M6. This is an extension to the already qualified core release path.

- M8.1 Scan declared eligible/unaccounted physical ranges sequentially with tile overlap, bounded header filters, deeper structural confirmation, and supported codec anchors.
- M8.2 Canonicalize exact geometry before content grouping or owner/hypothesis expansion. Keep compressed postings for repeated content.
- M8.3 Build compact sorted index runs under explicit memory/scratch caps; estimate density and skew from actual pilot observations.
- M8.3a Add selective configuration/length/timing/offset access paths where evidence permits; store posting cardinalities, enforce visited-posting/index-byte/CPU query budgets, and memoize compact hot-bucket outcomes.
- M8.4 Handle signature floods, huge duplicate buckets, and nested false candidates as saturation/unresolved outcomes with declared coverage.
- M8.5 Add bounded cluster-boundary continuation candidates for missing chains, including split length fields; do not assume sample/MP4-chunk contiguity.

Tests prove: cross-tile headers, huge boxes, embedded random signatures, repeated zeros/black/silent payload, million-entry collision classes, duplicate geometry reached through many metadata paths, and adversarial tiny extents. Instrument visited postings and index bytes, not only returned candidates; repeated beam states must not repeatedly scan the entire collision bucket. No-selectivity queries must stop with explicit unresolved/cutoff receipts.

Gate: scan-stage source-read amplification is at most 1.15× declared scanned bytes on the benchmark envelope, excluding separately metered metadata/extraction/hash-revalidation reads; throughput target is at least 70% of the same-device sequential-read baseline when CPU is not demonstrably limiting. These are release targets to validate, not current results. No total work grows quadratically with duplicate postings; cap hits visibly reduce evaluation coverage.

### M9 — Bounded missing-layout hypotheses

Depends on M8.

- M9.1 Implement hard constraints and an interpretable ranking function, with separate supporting/contradicting evidence on each edge.
- M9.2 Limit queries to constrained groups and neighborhoods; cap materialized successors, retained frontier, transitions, speculative reads, decoder probes, per-anchor time, and total speculative time as specified in DESIGN.
- M9.2a Record deterministic locality windows and candidate ordering; account for truncated query universes and all query work before materialized edge evaluation.
- M9.3 Implement deterministic state keys, semantic transposition checks, resumable frontiers, and cutoff receipts.
- M9.4 Keep uncertain paths provisional. Default export splits at ambiguity; inferred sample sizes/timing never become exact because a path decodes.
- M9.5 Support video-only or elementary-stream salvage when MP4 audio framing/configuration cannot be established; no fabricated AAC packet boundaries.

Tests prove: repeated complete GOPs; reset counters; static security footage; silent audio; multiple physically distinct equal fragments; one geometry with multiple owners; plausible visually smooth wrong sequences; abrupt correct edits; POC wrap; missing sample sizes; mismatched reference clips; one physical span reused to fill multiple positions.

Gate: all configured bounds hold on adversarial skew; no automatic inferred join is presented as original-mapping-supported; ambiguous fixtures remain unresolved. Independent review must attempt to produce a wrong but decodable assembly and verify that the claim model rejects its promotion.

### M10 — Blind accuracy, reliability, and consumer acceptance

Core MP4 qualification depends on M7. Enabling M8/M9 additionally requires their own gates and a repeated affected M7/M10 evaluation; otherwise those features remain disabled. Optional raw-stream work cannot substitute for an MP4 gate or delay acceptance of the qualified MP4 paths.

- M10.1 Freeze algorithms, thresholds, and supported profiles before opening the held-out corpus.
- M10.2 Run independent end-to-end evaluation against original logical samples, physical extents, timing, and known corruption operations.
- M10.3 Audit accepted joins, provenance, completeness labels, unsupported cases, and cap exhaustion; preserve raw test receipts and failures.
- M10.4 Repeat crash/disk-full/source-change tests at all publication/checkpoint boundaries and compare semantic results to uninterrupted controls.
- M10.5 Conduct ordinary-user acceptance: select image/map, see feasibility, obtain qualified outputs, stop, resume, and inspect unresolved material.

Gate: all mandatory gates in sections 4–6 pass. Exact recovery and conservative segments can be accepted while inferred cross-fragment ordering remains disabled by default. Independent review has veto over unsupported exactness/completeness claims; the decision report names the tested envelope and remaining uncertainty.

### M11 — Controlled rollout as a new case

Depends on M10 and separate user authorization to run real recovery.

- M11.1 Package a reproducible binary/tool manifest and documented local operation/recovery controls; verify supported host installation.
- M11.2 Create a new case/output destination; do not migrate, rewrite, or mutate any existing recovery case.
- M11.3 Run read-only pilot on one selected image/map; compare projected costs to the 24-hour budget and actual free space.
- M11.4 Continue only within the selected scope and budget; publish qualified results continuously; retain ambiguous and unsearched receipts.
- M11.5 Compare new-case outputs by bytes, provenance, original-order evidence, playable intervals, and unique content yield. Any comparison against another tool uses independent outputs and equal input/budget, not inherited claims.

Gate: source images/maps remain unchanged; case state is resumable; actual outcomes and elapsed/resource costs are reported. Running a pilot is not acceptance of the product or completion of all image recovery.

For the observed eight-image family, choose image priority only after comparing bounded metadata/content evidence. Shared names, serials, or directory trees do not establish equal epochs. Cross-image reuse may avoid repeating byte-verified work but must preserve each source's physical provenance and never silently merge recordings. Show cumulative case/family cost so “24 hours per image” cannot be mistaken for “24 hours for all images.”

Optional extension R1, outside the mandatory MP4 path: recover confirmed raw Annex B AVC/HEVC logical files and dependency-safe segments under the same source/budget/claim controls. Add raw framing, split-start-code, missing timing/audio, and extension-mislabel tests; repeat affected validation/performance gates. It requires its own selected scope rather than taking MP4 recovery budget by default.

## 4. Corpus and ground-truth design

Each fixture retains original file bytes; original logical sample ordinal; physical source ranges, including split samples; sample-byte hashes; DTS/PTS/durations; sample-description identity; deletion/reuse operations; map-coordinate transform; and exact damage mask. Ground truth is stored outside the image and never exposed to recovery ranking.

| Corpus axis | Required coverage |
| --- | --- |
| Containers | Primary: ordinary MP4 with head/tail/missing `moov`, partial tables, intact/damaged extents, missing/interleaved `mdat`; separately: fMP4 with missing init/fragments and supported offsets/defaults; optional R1 only: raw Annex B AVC/HEVC |
| Codecs/timing | AVC, HEVC, supported audio, B frames, open GOP, IDR/CRA, VFR, parameter changes, edits, audio delay/priming |
| Filesystems | FAT32/exFAT valid allocation, fragmented/deleted/partial chains, contiguous exFAT metadata, stale entries, reuse/conflicts |
| Acquisition | All map states, unknown ranges, stale nonzero data, sparse holes, nonzero displacement, partition/whole-device mismatch |
| Ambiguity | Repeated scenes/silence, shared parameters/IDs/counters, duplicate recordings, identical content at distinct coordinates, shared geometry |
| Malformed/scale | Overflow, deep/huge boxes, corrupt structures, signature floods, high-cardinality postings, tiny extents, index cap |
| Reliability | Kill/cancel/pause/resume, image/map change, checkpoint corruption/history compaction, disk full, decoder crash/hang, publication crash, virtual dependency loss |

Construct physically fragmented layouts at cluster boundaries, including boundaries inside NAL length prefixes and audio samples. Include impossible-recovery controls where surviving bytes cannot determine original order or ownership. Correct output for these controls is qualified ambiguity, not high recall.

Ordinary MP4 is the principal blind stratum and must cover every damage-matrix row, including intact logical extents plus lost `moov`, intact tables plus broken physical chains, partial `mdat`, and simultaneous extent/table loss. Include B frames, VFR, interleaved supported audio, and optional matching/mismatched/no donors. Report per-path byte/sample-order/timing/audio precision and recoverable coverage; do not average raw, DVR, AVI, or fMP4 successes into an ordinary-MP4 result.

Split development, validation, and blind holdout by original recording and capture/encoder configuration. Variants of one source clip, repeated frames, and cloned fixtures are correlated and cannot inflate independent trial counts. Include unseen device/configuration families. Report public-dataset limitations: intact public recordings help generate controlled damage but do not reproduce every real deletion, overwrite, or capture failure.

Any holdout fixture revealed to developers during a fix becomes regression data. A subsequent claim of blind performance requires a fresh untouched holdout or a clearly disclosed prospective evaluation protocol; repeatedly tuning against the same failures does not preserve blindness.

## 5. Metrics and denominators

| Metric | Definition and limit |
| --- | --- |
| Acquisition coverage | Finished bytes / bound rescue-domain bytes; report other statuses separately |
| Scan coverage | Eligible bytes actually scanned / eligible bytes in declared search scope |
| Metadata evaluation | Evaluated recognized objects / discovered recognized objects, with unsupported/capped counts |
| Search accounting | Generated/evaluated/rejected/unresolved/skipped hypotheses within declared candidate-generation rules |
| Exact sample precision | Correct byte identity, original ordinal/timing/configuration / samples claimed exact |
| Known-recording recovery | Recovered correct sample bytes and video time / known original bytes/time; unknown when denominator is unavailable |
| False accepted joins | Wrong accepted original-order boundaries / independently evaluated accepted boundaries |
| Unique physical yield | Union of recovered physical intervals, preventing duplicate-export inflation |
| Logical yield | Supported file identities and independently playable intervals, with aliases/clones explained |
| Resource cost | All source reads/writes, CPU/wall time, peak RSS, allocated scratch/index/output, decode/probe cost |
| Resume overhead | Incremental checkpoint writes, resume verification bytes/time, adopted completed units, semantic equality |

No single recovery percentage combines bytes, durations, file counts, or hypotheses. On damaged real images without surviving totals, report original-video coverage as unknown. Finishing all generated candidates proves only configured processing coverage.

## 6. Mandatory acceptance gates

| Gate | Required result |
| --- | --- |
| Source safety and binding | Zero source/map mutations; correct offset/status behavior; no acquisition-backed claim from wrong/unverified binding |
| Provenance | Every copied output byte resolves to supported source spans; all synthesized metadata/timing/zeros are explicit |
| Exactness | Zero misclassified exact outputs on blind fixtures; all claimed exact sample bytes, original identity/order/timing/configuration match ground truth |
| MP4-primary release | Required ordinary-MP4 damage paths and interleaved audio/B-frame/VFR cases pass their declared gates; missing target tables are not replaced by donor/NAL assumptions, and raw/fMP4 outcomes cannot stand in for ordinary-MP4 evidence |
| Ownership ambiguity | Distinct physical copies and shared geometry retain correct alternatives; content equality does not create ownership proof |
| Conservative salvage | Ambiguous/unsupported boundaries split or stop by default; missing audio or timing remains visible |
| Validation | Actual staged sample bytes/order/configuration/timing match the asserted map/declared transforms, plus complete strict decode for every published playable interval; untested/timeout/unsupported never reported as pass |
| Completeness | Every completeness claim has its stated defensible denominator; budget exhaustion cannot imply complete recovery |
| Resource/deadline | Memory/disk/read/candidate limits and 24-hour total cap enforced; useful partial results and unresolved frontier survive |
| Resume/publication | Equal semantic outcomes/hashes at equal completed logical work; truthful partial coverage under wall deadlines; no final artifact without a reconciled completed receipt |
| Efficiency | No unrestricted all-pairs joins; scan complexity linear in declared bytes plus metered overhead; repetitive-input stress stays bounded |
| Consumer operation | Image/map selection, feasibility, results, pause/cancel/resume, uncertainty, and continuation cost understandable without agent intervention |

For uncertain automatic joins, set an additional statistical release gate: zero observed false accepted joins in a blind evaluation with enough genuinely independent boundary trials to support a declared upper error bound. With zero errors in `n` independent trials, a one-sided 95% binomial bound is `1 - 0.05^(1/n)` (approximately `3/n`). About 3,000 independent trials support an upper bound near 0.1% under that evaluation distribution; correlated variants do not count as independent. Publish the sampling/clustering assumptions. This does not prove a universal false-join rate. If sufficient evidence is unavailable, keep uncertain joins provisional and ship the exact/conservative paths.

## 7. Benchmark protocol and stop rules

Run a sequential source-read baseline on equivalent storage/read conditions, then full pipeline tests with cold/warm cache explicitly labeled. Count logical source bytes and actual device I/O where available. Disclose filesystem sparse allocation, compression, cache effects, source/destination topology, CPU model, RAM, codec build, thermal state, and whether datasets fit in RAM.

Report phase-by-phase elapsed time and overlap, time-to-first-useful-output, complete validation time, recovery yield and ambiguity, maximum posting list, index/scratch allocation, checkpoint overhead, and total wall time. Compare methods under equal image/input evidence, complete end-to-end budgets, and qualification rules. An inner-loop match-rate number is not whole-case throughput.

Normalized initial release targets:

- Scan-only amplification at most 1.15× and at least 70% of sequential baseline on the declared non-CPU-limited envelope.
- Checkpoint overhead at most 5% of end-to-end time on benchmark cases, with no periodic copy proportional to total media/index size.
- Bounded candidate edges/frontiers under every repetitive/malformed fixture; report exact configured counts rather than asymptotic claims alone.
- Pilot at most ten minutes/8 GiB, followed by measured ETA interval, space estimate, and explicit feasibility against 24 hours.
- Deadline control stops new work early enough to validate/publish already reserved results or reports them as staged/unvalidated; shutdown/checkpoint time is reserved inside the cap. Inject slow/hung worker I/O; report an OS-level uninterruptible-I/O overrun explicitly instead of claiming an impossible hard kernel deadline.

Missing a speed target triggers profiling of the measured bottleneck before changing algorithms. If full strict decoding dominates 24 hours, prioritize smaller independently useful outputs with reserved validation budget; do not label unvalidated exports as validated. If the source I/O floor exceeds the budget, report the impossibility before promising complete scanning. If posting skew dominates, strengthen constraints or return ambiguity; do not increase limits without a costed user choice.

## 8. Release evidence package and remaining decisions

The qualification package contains pinned build/dependency manifests, support matrix, fixture/damage specifications, blinded evaluation receipts, benchmark environment/raw counters, source-safety and crash tests, known limitations, and an operational recovery guide. Preserve historical failing evidence; fixes add new evidence rather than rewriting failures as passes.

Decisions still requiring implementation evidence are filesystem adapter choice, supported AVC/HEVC/audio/profile envelope, strict decoder configuration and error classification, output container policy, index record layouts, host-specific concurrency, and achievable 24-hour completeness/partial-yield envelope. These are bounded prototype questions, not reasons to begin an unbounded recovery attempt.

The first accepted result should be a runnable, source-safe system that quickly recovers supported exact layouts, preserves useful verified segments, stops honestly on ambiguity, and can resume ordinary user-controlled work. Harder reconstruction is added only when it meets the same evidence and resource contracts.
