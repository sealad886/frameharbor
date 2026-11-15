# Image-first video restoration plan

This package proposes an **MP4-primary** recovery engine from first principles, using ddrescue partition/disk images and their mapfiles. It was designed without reading or adapting the current implementation. The normal budget selected by the user is **24 hours per image**, including discovery, matching, validation, export and shutdown. Ordinary MP4 is the primary release target; ISO fragmented MP4 is handled as a separate layout. Raw Annex B support is optional.

**Recommended order:** use surviving filesystem extents first; reconstruct intact containers and supported sample tables next; recover independently supported segments; then spend a limited residual budget on uncertain placement. Prioritize early useful results and stop with explicit unresolved material instead of expanding a global fragment permutation search.

## Documents

| Document | Purpose |
| --- | --- |
| [Algorithm design](DESIGN.md) | Architecture, claim model, reconstruction algorithms, bounded indexing/matching, resource policies and user operation |
| [Implementation plan](IMPLEMENTATION_PLAN.md) | Twelve ordered milestones, interfaces, corpus, reliability/accuracy/performance gates and rollout |
| [Evidence assessment](EVIDENCE.md) | Verified input facts, primary documentation, separately fetched papers and limits of inference |
| [MP4-specific evidence](EVIDENCE.md#e10-mp4-specific-evidence) | Direct MP4 research, donor/truncation limits, and the correction to the initial broad research scope |
| [Consensus report](CONSENSUS_RESEARCH.md) | Link to the provider's complete unchanged research document and its preservation receipt |
| [Review receipt](REVIEW_RECEIPT.json) | Reviewed document hashes, review scope and completed documentation checks |

## Input-specific basis

Read-only inspection found four approximately 250 GB exFAT partition images and four approximately 63 GB FAT32 partition images under `/Volumes/recovery_external/`. All paired maps declare full acquisition coverage. Representative recording directory trees are readable with The Sleuth Kit 4.14.0. These facts justify testing filesystem-led restoration first; they do not establish that deleted files or all media are intact.

The detailed evidence and probe limits are retained in [image inventory](IMAGE_INVENTORY.json), [filesystem probes](FILESYSTEM_PROBE.json), [directory probes](DIRECTORY_PROBE.json), and [file-entry probes](FILE_ENTRY_PROBE.json). No payload was exported or decoded during planning.

A 24-hour budget is a processing limit and a useful-results target. It cannot guarantee complete recovery from every damaged image. It is **per image**: the product must display a separate cumulative estimate for all eight acquisitions, and reuse work across them only when content and source-generation evidence permit it.

## Implementation priority

The first independently useful delivery follows **M0–M7 → M10 → M11**: evidence contracts, image reads, state/budgets, filesystem mapping, intact containers, supported segment recovery, validation/operation and measured qualification. Optional **M8–M9** residual discovery and uncertain-layout matching follow only when their value and false-join controls pass the same gates.

Outputs distinguish an acquired logical file, preserved encoded samples in a rebuilt container, independently playable segments, provisional ordering, and unresolved material. A successful decoder run never establishes original order or full coverage by itself.

## Status and boundaries

This is a researched design and implementation plan, not an implemented or benchmarked recovery algorithm. The original design received two independent reviews; the MP4-primary correction received a fresh independent accuracy review of the revised documents. These reviews cover methodology and qualification planning, not software, recovery yield, accuracy or throughput. The MP4 container layout and codecs still require discovery from actual file metadata; a `.mp4` suffix alone does not distinguish ordinary MP4 from ISO fragmented MP4.

Planning created only this new document package. It did not modify the existing algorithm, active recovery process, existing cases, source images or mapfiles. No commit, publication, merge or new worktree was performed for this planning task.

