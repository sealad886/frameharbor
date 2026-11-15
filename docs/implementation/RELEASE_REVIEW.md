# Alpha release review ledger

Updated 15 November 2026. This is a resumable record for the requested first
release, using the Iterating PR Bot Fixes workflow. It does not certify the open
full-release gates in `RELEASE_CHECKLIST.md`.

## Authority and identity

- Requested: finish the project, commit milestones, clean up using Iterating PR
  Bot Fixes, and tag a release for GitHub publication.
- Owner subsequently selected `0.1.0-alpha.1` and GPL-3.0-or-later, with separate
  third-party license details and texts.
- Initial checkout: unborn `main`, no remote, no existing tags, no staged files,
  one worktree. Existing implementation and planning files are preserved.
- The owner authorized a new public repository. Created and read back
  `https://github.com/sealad886/frameharbor` under the authenticated `sealad886`
  account: public, empty, default branch `main`, push permission confirmed.
  `origin` points to its HTTPS Git URL. No hosted PR or CI evidence exists yet.
  Hosted review is pending, not clean or unavailable by waiver.
- Local commits and a version tag are authorized. No new worktree or real-image
  recovery is authorized. Source-only preview; no portable native binary claim.

## Ordered checklist

- [x] Repair and verify the alpha candidate; preserve generated evidence under `target/`.
- [x] Commit the preserved planning milestone: signed `31058c3c3ba0f61582f22c0290c7c7ee19f59a3b`.
- [ ] **Active: commit and review the alpha implementation.** Bind checks and a
  fresh independent review to the exact candidate commit.
- [ ] Commit coherent release preparation and final review repairs; bind checks
  and a fresh independent review to the exact candidate commit.
- [ ] Complete applicable hosted review against the verified destination, then
  create and read back the authorized tag. Record local and remote state separately.

## Review sources and rounds

Ten rounds maximum. Two local rounds completed; no hosted round requested.

| Source | Revision/scope | Explicit outcome | Disposition |
| --- | --- | --- | --- |
| Independent Code Reviewer, round 1 | Initial uncommitted Rust/native/controller source; file digests retained in `target/release-prep/review-round1.md` | Actionable: P1 checkpoint retention defect | Addressed in round 2; fresh final-commit review required |
| Independent Code Reviewer, round 2 | Four repaired checkpoint/control/session/test file digests in `target/release-prep/review-round2.md` | Explicit clean for repaired R1-P1 boundary | Retired clean; final-commit review remains required for the complete release |
| Configured GitHub providers | New empty repository; no PR yet | Not preflighted or requested | Supplies no review coverage |

## Findings and pattern analysis

| ID | Requirement and root cause | Repair boundary | Evidence/status |
| --- | --- | --- | --- |
| R1-P1 | History zero must preserve resumability across publication/link interruption. Publishing CURRENT pruned the prior manifest before SQLite committed its replacement link. | Shared checkpoint/session retention boundary retains one linked frontier until settlement, then enforces history; corruption still fails closed. | Repaired; 47 focused tests including two new regressions and the full 214-test suite pass. Round 2 explicitly clean. |
| BUILD-1 | Native builds must use the selected available FFmpeg build. Hardcoded Homebrew `9.0.2_1` directory no longer exists. | Existing canonical `opt/ffmpeg-full` prefix in native build, package fingerprints, codec-profile test; retain exact runtime/header checks. | Installed `9.0.2_2` reports 9.0.2 and expected ABI. Fresh native build/TSK probe, codec controls and full suite passed. |

The reviewer inspected CLI start/resume, source and coordinate authority,
discovery/outcome transactions, dependency archives, staged sample/native decode
validation, publication identity and native supervision. No additional defect
was demonstrated in that bounded pass. Exhaustive parser, hostile-native and
hardware qualification were not claimed. Concurrent package edits are outside
that report's final-head coverage.

## Verification and integration

The fresh selected gate passed **214 tests across 28 targets, zero failed or
ignored**, plus formatting, all-target Clippy with warnings denied, release/native
build, package checksums, version `0.1.0-alpha.1`, and frozen source identity.
Evidence is under `target/release-prep/alpha1-verification-managed`.

- Test log SHA-256: `53c08b2dc348ebf61799bcfc96f4fa4f65a309ce956627d8cfc1055e2745ac22`.
- Package log SHA-256: `55bd6115dcb58063e51531259767fa8c40bebbc98d8b7dd5d2a9974e30365f96`.
- Local binary SHA-256: `3af52c066ade9f19125da0a1dafb871147572f62c968a20e759a7869ce5426e2`.
- Build manifest SHA-256: `eee74990f81f4382330c03177117f7094f9f770b5e8bb876f4349345c7b3ab4a`.

Only documentation closeout and removal of trailing blank lines from the two
TOML files followed that gate; locked Cargo metadata validation passed afterward.
The source/native/test/build-script bytes remain unchanged. Upstream license
whitespace and historical report bytes are intentionally preserved. Final-commit
review, hosted outcomes, tag identity and release readback remain pending.
Historical November 9 checks remain historical; its package is preserved.

The first background verification process ended before recording any check
completion; no result is inferred from its empty log. That attempt remains under
`target/release-prep/alpha1-verification`. The managed retry completed all six
checks successfully and queued its completion callback.

The public empty repository has been created. No pushes, PR creation, merge,
release publication, tag, or cleanup has occurred yet. No source/operational
data or existing evidence was deleted.
