# Standalone project decision and relocation

Date: 9 November 2026.

The user decided that this recovery system should be its own tool, named FrameHarbor, in /Users/andrew/zRepos/frameharbor. It is independent of VideoRebuild; its eventual executable, source tree, build manifest and case format belong in this project. An in-repository engine package, Python wrapper or VideoRebuild command integration is not part of this project boundary.

The entire planning package was moved, not copied:

- Original: /Users/andrew/zRepos/videorebuild/docs/designs/image-first-video-restoration/
- Current: /Users/andrew/zRepos/frameharbor/docs/planning/image-first-video-restoration/

All 16 original planning/evidence files are retained unchanged, including both complete Consensus reports and review receipts. Relative links within the package remain valid. MOVE_RECEIPT.json records original and new paths, byte counts, SHA-256 values, and filesystem identity checks.

Some historical JSON receipts contain paths beginning with docs/designs/image-first-video-restoration/ or the former absolute project path. Resolve those through the mapping above; the source path is no longer the current location. Do not rewrite the original receipts to make historical review hashes appear current. Provider reports retain their own separate citation numbering.

The algorithm remains a greenfield MP4-primary design using ddrescue images and mapfiles, with a cumulative 24-hour processing budget per image. Existing design-review acceptance is not a measurement of implemented accuracy, recovery yield or performance. START_PROMPT.md defines the next agent's implementation kickoff.

This relocation leaves folders and planning/handover documents only. The user is handling repository and project setup. No recovery code, image processing job, old-case migration, publication or release was performed.
