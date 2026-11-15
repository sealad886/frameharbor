# Release completion checklist

Reviewed 15 November 2026. This checklist records the current release boundary;
it does not replace the preserved implementation plan or certify its open gates.

## Current decision

The approved first tag is a **0.1.0-alpha.1 source preview prerelease**, after the
checks below pass. The user selected GPL-3.0-or-later for project-owned code.
The version must identify the preview channel consistently in Cargo,
`v0.1.0-alpha.1`, and release notes. A supported production release still requires
the original M7/M10/M11 evidence; publishing a tag cannot supply that evidence.

The host-dependent package is useful for local verification. It is not a
portable binary distribution: native dependencies remain outside the bundle,
and supported installation on another host has not been qualified.

## Ordered work

- [x] Preserve the historical handoff and planning receipts. Signed milestone
  `31058c3c3ba0f61582f22c0290c7c7ee19f59a3b` records those documents.
- [ ] Commit the verified implementation milestone, then commit coherent review
  and release-preparation milestones. Inspect exact staged contents and preserve
  local generated evidence. No worktree is required.
- [x] Record the approved GPL-3.0-or-later project license and separate
  third-party license texts/notices. The preview publishes source only;
  linked helper binary distribution remains outside its release assets.
- [x] Refresh native dependency pins and affected build/test/package callers.
  The verified fresh bundle is `target/local-package-alpha1`; the earlier
  evaluated package remains unchanged. Actual source, binary, helper and
  external-library hashes are recorded in its build manifest.
- [x] Pass local software checks: 214 tests across 28 targets, formatting,
  all-target Clippy, native/release builds, package smoke/checksums, version
  and source identity. The checkpoint repair also passed 47 focused tests.
- [x] Verify the authorized GitHub destination and initially empty history:
  [sealad886/frameharbor](https://github.com/sealad886/frameharbor).
- [ ] **Active: complete final review.** Bind independent review to the exact
  release commit, inspect hosted reviewer/check requirements, and obtain the
  applicable hosted outcomes. No hosted CI workflow or PR existed at preflight;
  local review/checks are separate evidence, not hosted completion.
- [ ] Finish the invoked PR-review lifecycle where applicable, then create and
  read back the authorized version tag. Publication must identify the preview
  limitations and distinguish local tags from pushed tags or GitHub releases.

## Demonstrated environment drift

On 15 November, `/opt/homebrew/Cellar/ffmpeg-full/9.0.2_1` was absent. The installed
`/opt/homebrew/opt/ffmpeg-full` points to `9.0.2_2`. The executable still reports
FFmpeg 9.0.2, libavformat/libavcodec 63.1.102 and libavutil 61.1.102, but this is a
different installed build. Existing helper load commands use the Homebrew `opt`
paths; the old helper SHA alone does not freeze its external dynamic libraries.
Build scripts and package fingerprints now use the installed `opt` prefix and
passed fresh local verification. Do not relabel the 9 November evaluation as evaluation of
the new dependency build.

The installed FFmpeg build enables GPL and version 3; `ffmpeg -L` identifies
GPL-3.0-or-later. FFmpeg's [license guidance](https://ffmpeg.org/legal.html)
distinguishes that build from LGPL-only builds. TSK's
[license overview](https://www.sleuthkit.org/sleuthkit/licenses.php) requires
checking per-file licenses; its core includes IPL/CPL code. These observations
do not establish complete binary redistribution compliance. Keeping third-party
libraries out of a bundle does not by itself settle the linked helpers' terms.

## Full supported-release gates still open

| Gate | Evidence still needed | Why local cleanup cannot close it |
| --- | --- | --- |
| M7 | Representative 63.28 GB/250.15 GB workloads, allocated bytes and real reads, storage topology, phase/validation/checkpoint costs, resource/deadline and long resume behavior | Small sparse fixtures and microbenchmarks do not measure the declared hardware envelope |
| M10 | Fresh independent media/configuration qualification across the required damage matrix, plus ordinary-user acceptance | The exposed seven-control evaluation covers two procedural families and correlated variants |
| M11 | Supported-host installation and a separately authorized new-case read-only real-image pilot after prior gates | A local package and release request do not identify or authorize a real recovery input/case |

M8/M9 uncertain reconstruction and raw Annex B remain disabled optional
extensions; they do not need implementation for the selected core preview.
Do not weaken exactness, source safety, strict validation, or unresolved labels
to promote an open gate.
