# Start FrameHarbor implementation

Use the following prompt to begin the next implementation session in `/Users/andrew/zRepos/frameharbor`.

---

Implement the approved greenfield **FrameHarbor** plan as a standalone Rust tool. Start with M0, then progress through the ordered milestones as their behavioral checks pass. Complete safe, bounded implementation work autonomously; do not turn this into another general research/planning pass or request permission for every ordinary local step.

## Project and authority

- Work in `/Users/andrew/zRepos/frameharbor`. Read applicable repository instructions and inspect live status before editing. Preserve other people's changes.
- The handoff contains folders and planning documents only. The user is handling repository/project setup. Inspect and preserve their current setup; do not assume recovery code, a Cargo manifest, tests, or measured performance already exist.
- This invocation authorizes bounded source implementation, project-local synthetic fixtures, tests, builds, and ordinary documentation updates. The prompt file itself did not execute anything when created.
- Real-image broad recovery launches, starting/stopping other jobs, source/map/case mutations, publication, commits/pushes/releases, and new worktrees require separate explicit authorization. Do not interfere with other projects or their jobs.
- Git was deliberately left uninitialized at handoff. Inspect whatever repository state the user has since created; preserve it. Do not commit or publish merely because implementation begins.
- Do not read, adapt, modify, or depend on `/Users/andrew/zRepos/videorebuild` or its implementation/cases. There is no in-repository Python CLI adapter or compatibility requirement. All new implementation belongs in FrameHarbor.

## Authoritative local material

Read these files before selecting implementation contracts:

1. `README.md` and `docs/planning/RELOCATION.md` for current project identity, workspace status, relocation, and interpretation of historical paths/status.
2. `docs/planning/image-first-video-restoration/DESIGN.md`, especially the MP4 damage matrix and separate evidence claims.
3. `docs/planning/image-first-video-restoration/IMPLEMENTATION_PLAN.md` for milestones, interfaces, corpus, and gates.
4. `docs/planning/image-first-video-restoration/EVIDENCE.md`, especially E0 and E10, for bounded input facts and MP4-specific source applicability.
5. `docs/planning/image-first-video-restoration/CONSENSUS_RESEARCH.md` and its linked original/addendum reports. Keep provider documents **verbatim**; never rewrite them as a custom summary or reuse one report's citation numbers for another.
6. `docs/planning/image-first-video-restoration/REVIEW_RECEIPT.json` and `docs/planning/MOVE_RECEIPT.json`. Earlier review hashes and paths describe historical revisions, not a certification of newly implemented code.

The planning bundle moved without altering its internal folder structure. Earlier statements that the document did not authorize implementation describe the planning task; this invocation authorizes the bounded implementation scope above. Design requirements and source/evidence limits remain in force.

## Required product behavior

- **Ordinary `.mp4` is primary.** Support declared AVC/H.264, HEVC/H.265, and audio profiles through evidence-driven MP4 reconstruction. ISO fragmented MP4 is a separate profile; raw Annex B is optional R1.
- Distinguish a filesystem-fragmented ordinary MP4 from `moof`/`traf`/`trun` ISO fMP4. Noncontiguous disk allocation does not imply fMP4.
- The only hard recovery inputs are raw ddrescue disk/partition images and their associated mapfiles. A donor/reference clip is optional.
- The normal budget is **24 hours per image**, covering intake, indexes, search, full validation, export, and checkpoint/shutdown reserve. Report whole-family cost separately; eight per-image runs do not imply completion of the whole family in 24 hours.
- Produce useful exact/evidenced files and conservative segments early. Preserve an explicit unresolved remainder when evidence or budgets end. No universal weeks-to-hours, completeness, or original-order guarantee is established.
- Provide ordinary inspect/pause/cancel/resume/export controls and durable idempotent state. Default checkpoint history is current plus at most two historical frontier manifests sharing immutable payloads, configurable for more.

## Input context: historical, read-only observations

The source location is `/Volumes/recovery_external/`. Do not start a recovery run simply because this path is known. Use synthetic fixtures for initial implementation.

The preserved JSON probes record:

- Eight raw **partition** images totaling 1,253,697,155,072 bytes.
- Four BlinkDrive-family exFAT images, each 250,147,241,984 bytes, with observed 128 KiB clusters.
- Four Kitchen/NO NAME-family FAT32 images, each 63,277,046,784 bytes, with observed 16 KiB clusters.
- Every paired map declares one finished interval covering its image. This means declared acquisition coverage, **not** intact videos, valid historical ownership, authentic pairing, or an undamaged filesystem.
- The filesystem starts at image offset zero. The exFAT boot record retains a historical 2,048-sector partition offset; do not add it again to image-relative reads.
- Representative root/recording directories were readable using Sleuth Kit 4.14.0. That does not validate every file chain. The probed `.h265` entry was zero length; it does not establish a raw-video payload.
- Images sharing names or volume identifiers are not proven identical or the same acquisition epoch. Do not silently combine recordings across images.
- Approximately 454 GiB was free at inspection, less than aggregate image bytes. Recheck storage before a real action; use virtual extent manifests, selective exports, and quotas rather than copying every candidate.

Exact filenames and scope are in `IMAGE_INVENTORY.json`, `FILESYSTEM_PROBE.json`, `DIRECTORY_PROBE.json`, and `FILE_ENTRY_PROBE.json` under the planning directory. These observations are not current measurements, recovered footage, or permission to mutate inputs.

## Non-negotiable correctness boundaries

- Keep images/maps read-only; put synthetic fixtures, cases, indexes, and outputs under explicitly owned project/work destinations.
- Bind ddrescue input coordinates, image offsets, and filesystem offsets explicitly. Non-finished/unknown acquisition bytes remain unavailable even if stale destination bytes look like video.
- Read logical MP4 offsets through evidenced filesystem extents. A cluster boundary can split a chunk, sample, or NAL length prefix. Separate metadata-backed extents from library recovery/contiguity guesses and normalize address units.
- Track source-byte provenance, physical ownership, structure, decode result, temporal correctness, and coverage separately. A successful decoder run is not proof of original order, identity, timing, or completeness.
- Preserve encoded samples and independently compare the actual staged artifact/view against its asserted sample map. Label remuxed headers, timestamp changes, framing transforms, missing audio, and inferred timing.
- Missing target `moov`/tables cannot be recreated accurately just by copying a donor's `moov` or parsing NAL headers. Donors supply only qualified codec/configuration/layout priors. Interleaving, B frames, VFR, sample lengths, DTS/PTS, and audio synchronization need target evidence.
- Do not invent a unique continuation when multiple owners/orders remain possible. Split at ambiguity and retain qualified alternatives.
- Deduplicate exact geometry before candidate expansion. Bound retrieval work, posting visits, reads, edges, memory, disk, decode probes, and elapsed work. No blind all-pairs or global fragment permutations.
- Isolate hostile-input native parsers/decoders; Rust does not make their C code safe. Qualify pinned headers/ABI and behavior before adopting filesystem/media dependencies.
- Make checkpoint/publication durable and recoverable. Flush staged bytes before durable publication intent, rename/directory flush, and published-state commit. Preserve failure evidence and source dependencies of virtual artifacts.

## Begin with M0 and continue implementation

1. Inspect the user's project setup and authoritative documents. Maintain a concise checklist with one active step and record current milestone/evidence.
2. Implement M0's bounded requirements/claim schemas, checked range/coordinate contracts, budget/terminal states, and fixture-ground-truth specification. Make ordinary MP4 the principal corpus stratum; map every damage-matrix row to positive, negative, or ambiguous cases.
3. Confirm the local Rust toolchain and candidate dependency versions. Use current primary documentation within pinned versions; distinguish the historically observed TSK 4.14.0 from whatever is actually selected now. Do not install dependencies merely to explore options.
4. Build minimal synthetic tests for contracts/invariants and deliberate failures. Test source-byte/sample order/timing/audio correctness against retained ground truth; decoder success alone is not the oracle. Preserve untouched holdouts; exposed failures become regression data.
5. Progress through **M0–M7 → M10 → M11** as each gate passes. M6 validation/control foundations start after M2 and protect each media milestone. M11's real-image rollout still waits for separate explicit run authorization.
6. Treat M8–M9 residual discovery/uncertain matching and optional raw R1 as extensions. They must not block a qualified useful MP4 core or consume its default budget; repeat affected gates before enabling them.
7. Use bounded specialist delegation and independent verification where applicable instructions require them. Keep file ownership explicit and integrate only verified, in-scope work.
8. Report what was implemented, tests/builds actually run, evidence produced, current milestone, and remaining bounded risks. Do not claim planning reviews, a built binary, or a synthetic pilot prove real-image recovery accuracy/yield/performance.

Start M0 now and continue safe implementation rather than ending with a proposal. Pause only at a concrete authorization boundary, genuine blocking dependency, or failed gate that needs a user decision.

---
