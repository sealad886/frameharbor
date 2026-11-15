# Changelog

## 0.1.0-alpha.1 — source preview

Initial source preview of the bounded Rust recovery core:

- Read-only ddrescue image/map acquisition, explicit coordinate binding and
  unavailable-byte masks.
- Allocated FAT32/exFAT extent discovery, supported ordinary AVC/HEVC/AAC MP4
  sample mapping, and conservative segment export from surviving ordinary/fMP4
  tables.
- Independent staged-sample checks and strict full native decoding before
  publication, with separate provenance, ownership, structure, decode, timing
  and coverage claims.
- Durable start/inspect/pause/cancel/resume/export controls, resource admissions,
  checkpoints, source-dependency archives and idempotent completed-work adoption.
- Source-preview build/release instructions, GPL-3.0-or-later project licensing
  and separate third-party license notices.
- Native dependency discovery refreshed for the installed FFmpeg 9.0.2 Homebrew
  revision without changing the required media API profile.
- Fixed checkpoint retention with zero historical frontiers: a publication/link
  interruption preserves the prior linked checkpoint until settlement, rather
  than making a resumable case reference a pruned manifest.

Local alpha verification on 15 November 2026 passed 214 tests across 28 targets,
formatting, all-target Clippy, native/release builds, package checksums, version
and source-identity checks. Two regressions cover the checkpoint fix. Final
exact-commit and hosted review, tagging and publication remain pending in the
[release review ledger](docs/implementation/RELEASE_REVIEW.md).

This prerelease is source only and targets the selected macOS arm64 development
environment. It does not qualify representative large-image performance,
real-camera recovery, the full damage matrix, consumer acceptance or a real-image
rollout. M7/M10/M11 remain open; M8/M9 uncertain joins and raw Annex B stay disabled.
The [release guide](docs/RELEASE.md) and final review evidence distinguish the
historical synthetic results from current candidate checks and publication state.
