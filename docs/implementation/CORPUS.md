# Fixture and ground-truth contract

Status: fixture contract, 9 November 2026. Selected procedural filesystem/media fixtures and strict native decoding are implemented; see `MEDIA_FIXTURES.md`, `FILESYSTEM_FIXTURES.md` and `STATUS.md` for current evidence. This contract does not establish blind accuracy or real-image recovery. Exposed fixtures and failures are development regression data.

## Fixture construction

Ordinary MP4 is the principal stratum. Generate original procedural video and audio locally, retain the original encoded MP4, then construct raw FAT32/exFAT image layouts from independently recorded placement instructions. No private recording is required. Record the generator source/version, exact command, seed, encoder/configuration, original digest, provenance and redistribution grant before admitting a clip. Public availability alone is not a license. No licensed third-party clips or consenting real-device recordings have been acquired in this implementation session; these remain corpus acquisition work before later qualification.

Keep `truth/` outside the image, case and recovery process's input tree. A fixture package contains:

- `original.mp4`: immutable original logical file, plus its digest and byte length.
- `truth.json`: recording/configuration family, track identities, each original encoded sample's ordinal, logical byte ranges, SHA-256, size, DTS, PTS, duration, timescale, sample-description/configuration digest, sync/reference facts, edits, audio delay/priming and channel/rate facts. Timing uses integers and rational timebases, never rounded floating point.
- `placement.json`: explicit image generation and ordered physical spans for every original logical range; half-open byte coordinates, including spans split inside samples and NAL length prefixes. Keep equal-content physical copies and logical owners separate.
- `operations.json`: ordered deletion, allocation reuse, overwrite, table corruption, truncation, relocation and map-status changes, with exact original and resulting masks. Preserve original truth even after damage.
- `image.img` and `image.map`: the only mandatory recovery inputs. Binding assertions record input/output displacement, rescue domain, image-relative filesystem start and optional historical original partition start separately.
- `expectation.json`: permitted output classes, required unresolved material and reason, known original denominator, surviving acquisition-supported denominator and impossible-recovery controls.

The oracle independently reads original samples and placement truth. Compare the actual staged artifact's samples, track identity, ordinal, payload digest, configuration, DTS/PTS/duration and audio synchronization against truth, accounting only for declared reversible framing/timestamp transforms. Full strict decoding is a separate requirement; it does not replace this oracle. A remux can preserve samples while changing original file bytes. Synthesized headers, missing audio, inferred timing, unavailable spans and unsupported edits must remain explicit.

The claim transport schema maps `provenance` to Rust `Claims.bytes`, `decoder` to `Claims.decode`, and kebab-case enum labels to their Rust variants (`metadata-supported-owner` maps to `Ownership::MetadataSupported`). Its sample evidence fields `bytes`, `identity`, `order`, `timing`, `configuration` and `audio` map directly to `SampleVerification`; exact samples require every field. Audio passes only when original audio behavior or its evidenced original absence is verified. Dropping unverified audio does not satisfy that field. Decoder timeout and unsupported-profile outcomes remain separate from failure and untested. These labels never upgrade temporal correctness or coverage.

## Required strata and variants

[`acceptance-matrix.json`](acceptance-matrix.json) maps all seven DESIGN §6.0 rows. Ordinary MP4 and fMP4 have separate denominators and receipts. Raw Annex B is disabled and outside the mandatory path.

Cross ordinary-MP4 cases with head/tail `moov`, AVC and HEVC, supported interleaved audio, B-frame decode/presentation reordering, VFR, configuration changes, multiple sample descriptions and supported edits/priming. Include 16 KiB FAT32 and 128 KiB exFAT cluster geometry, contiguous and fragmented allocation, partial/deleted chains, `NoFatChain` with bitmap agreement/disagreement, reused/crosslinked clusters, misleading extensions and zero-length entries. Place boundaries inside a box header, chunk, encoded sample, NAL prefix and audio sample.

Acquisition variants include `+ ? * / -`, uncovered gaps, stale nonzero destination data, sparse host-file holes, nonzero input/output relocation, wrong same-size map, whole-device/partition mismatch, short reads and generation changes. A partition filesystem at image offset zero with a historical 2,048-sector boot offset must never acquire that offset a second time.

Malformed/scale cases include arithmetic overflow, nested/oversized boxes, contradictory table counts, external references, loops, signature floods, duplicate geometry, equal-content distinct physical copies, repeated black frames/silence and resource-cap exhaustion. Ambiguous surviving observations must admit at least two different ground-truth originals; the expected result is unresolved or qualified alternatives, even when both decode.

Reliability fixtures inject pause/cancel, child crash/hang, disk full, source/map changes, corrupt checkpoint and failure at every publication durability boundary. Compare resumed versus uninterrupted semantic results at equal logical work/byte budgets. Wall-deadline truncation uses a separate truthful-partial-result oracle. Process termination is not proof of power-loss durability.

## Independence and holdout protocol

Split development, validation and holdout by original recording **and capture/encoder configuration family** before generating damage variants. Cloned clips, repeated scenes and differently damaged copies of one original are correlated trials. Keep holdout truth inaccessible to implementers until algorithms, thresholds, profiles and build digest are frozen. An independent evaluator may prepare procedural unseen configurations without exposing them to recovery code. All current suite fixtures remain exposed development data. Following the frozen selected-software gate, an independent evaluator generated two new procedural capture/encoder configuration families against the frozen local package. The [prospective report](PROSPECTIVE_EVALUATION.md) records seven passing controls, including four intact outputs and three negative controls. Filesystem/damage variants of one original are correlated. This does not make the existing 212-test development suite blind, establish real-camera accuracy or complete M10. The prospective truth and results are now exposed regression material; future tuning against them needs a fresh evaluation for another prospective claim.

When a failing holdout becomes visible during a fix, retain its failing receipt and reclassify it as regression data. Use a fresh holdout or disclose a prospective evaluation protocol for the next blind claim. Real-device examples require explicit provenance/rights and remain separate from synthetic tests. Do not use the user's recovery images to supply missing ground truth.

## Metrics and receipts

The machine matrix lists each numerator, denominator and unknown/not-applicable rule. Store raw counters and per-profile outcomes, not only aggregate rates. Every receipt identifies fixture/build/source generations, configured limits, evaluated/skipped scope, transformations, six independent claims, terminal reason and retained unresolved remainder.

Zero false accepted uncertain joins supports a one-sided 95% bound of `1 - 0.05^(1/n)` only for genuinely independent boundary trials from the disclosed distribution. About 3,000 independent trials are needed for a bound near 0.1%; correlated damage variants cannot inflate `n`. No such measurement exists yet.
