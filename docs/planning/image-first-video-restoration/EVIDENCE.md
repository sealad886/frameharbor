# Evidence assessment and design basis

Status: research and bounded read-only inspection, 9 November 2026. This assessment supports a new algorithm; it does not assess the existing recovery implementation. The user's primary target is **MP4 files**, with a **24-hour processing budget per image**. MP4-specific evidence and its limits are assessed in [E10](#e10-mp4-specific-evidence); generic carving and raw-codec work are supporting background.

The proposed design is in [DESIGN.md](DESIGN.md); ordered delivery and acceptance gates are in [IMPLEMENTATION_PLAN.md](IMPLEMENTATION_PLAN.md). The unmodified provider report is linked from [CONSENSUS_RESEARCH.md](CONSENSUS_RESEARCH.md).

## E0. Observed input evidence

[IMAGE_INVENTORY.json](IMAGE_INVENTORY.json), [FILESYSTEM_PROBE.json](FILESYSTEM_PROBE.json), [DIRECTORY_PROBE.json](DIRECTORY_PROBE.json), and [FILE_ENTRY_PROBE.json](FILE_ENTRY_PROBE.json) retain the exact limited observations.

| Acquisition family | Image files | Bytes per image | Observed filesystem | Observed cluster size |
| --- | --- | ---: | --- | ---: |
| BlinkDrive | `BlinkDrive_Nov`, `BlinkDrive_Rescue_2`, `BlinkDrive_Rescue_3`, `BlinkDrive_Rescue_new` | 250,147,241,984 | exFAT | 128 KiB |
| Kitchen / NO NAME | `KITCHEN_Rescue`, `KITCHEN_Rescue_3`, `KITCHEN_Rescue_new`, `NO NAME_Rescue` | 63,277,046,784 | FAT32 | 16 KiB |

All eight files are partition images whose filesystem boot region is at image offset zero. Every paired map declares one finished range covering its entire image. This is declared acquisition coverage, not independent proof that the pair is authentic or that original files remain intact.

The installed Sleuth Kit **4.14.0** successfully opened the representative `BlinkDrive_Rescue_new.img` and `KITCHEN_Rescue_new.img`. Nonrecursive directory probes found readable recording trees: `blink` with dated subdirectories, and `record` with dated subdirectories and video-named entries. The probed `blink_backup` directory was empty. A Kitchen `.h265` entry was a zero-length allocated file; its suffix therefore does not prove the presence of an elementary video stream. A deleted Kitchen MP4 entry retains a nominal 3,407,236-byte length and a first cluster; this does not establish a complete usable chain or intact recording.

An important coordinate case is already present: the exFAT boot record retains a **2048-sector original partition offset**, while the filesystem opens at **image offset zero**. Adding that stored offset to all reads would address the wrong bytes.

All four BlinkDrive images share a volume identifier but have different first-4-KiB hashes. The smaller images include two observed volume identifiers. These facts identify possible acquisition families, not identical contents or safe cross-snapshot assembly.

The eight images total **1,253,697,155,072 bytes**. Destination capacity must be planned separately: the source volume had approximately 454 GiB free at inspection, and exporting every acquisition independently could exceed that. No media payload was exported or decoded, no full-image hash was computed, no filesystem was repaired, and no original image or mapfile was modified. These probes do not establish recording recovery rates, full filesystem consistency, or an end-to-end ETA.

## E1. GNU ddrescue

Primary reference: [GNU ddrescue manual](https://www.gnu.org/software/ddrescue/manual/ddrescue_manual.html), version 1.30, especially mapfile structure and input/output positioning.

A map describes acquisition status and input-coordinate ranges, not original file extents. Non-finished destination regions may contain zeros or older bytes. The proposed reader therefore binds the pair and coordinate transform explicitly, and treats unsupported acquisition ranges as unavailable. This is a design consequence of acquisition semantics, not a claim that these fully marked maps are damaged.

## E2. Filesystem evidence

Primary references: [The Sleuth Kit capabilities](https://www.sleuthkit.org/sleuthkit/desc.php), [exFAT implementation notes](https://wiki.sleuthkit.org/ExFAT-Implementation-Notes/), and [C filesystem API](https://www.sleuthkit.org/sleuthkit/docs/api-docs/4.3/group__fslib.html).

TSK supports raw-image filesystem analysis, allocated/deleted metadata and data access. Readable recording directories were independently observed here. This justifies implementing filesystem-led extent recovery first; it does not establish that stale deleted chains are authoritative. Public C API documentation consulted is labelled 4.3, whereas the installed tools are 4.14.0. Context7 returned no relevant TSK documentation entry. The installed headers expose `TSK_FS_ATTR_RECOVERY` for recovery-mode-derived attributes, byte offsets in file-walk callbacks, and filesystem-block units in runs. The implementation gate must certify the pinned ABI, physical-address validity flags, recovered-versus-inferred extents, sparse runs and cancellation behavior; header inspection alone does not certify an adapter.

## E3. ISO BMFF sample layout

Primary references: Apple's [sample-to-chunk](https://developer.apple.com/documentation/quicktime-file-format/sample-to-chunk_atom), [sample size](https://developer.apple.com/documentation/quicktime-file-format/sample_size_atom), [chunk offset](https://developer.apple.com/documentation/quicktime-file-format/chunk_offset_atom), and [composition offset](https://developer.apple.com/documentation/quicktime-file-format/composition_offset_atom) documentation.

Surviving sample tables provide logical placement, sample lengths, and timing relationships. They do not contain fingerprints of unknown payload bytes. The design maps logical file positions through filesystem extents and verifies referenced bytes. Physical filesystem fragmentation can cross container chunks and codec samples; support for those boundaries is an implementation requirement.

## E4. Fragmented MP4 profile

Primary reference: [W3C ISO BMFF Byte Stream Format](https://www.w3.org/TR/mse-byte-stream-format-isobmff/).

The specification describes initialization and media segments for an MSE profile. Its narrower addressing and timing requirements must not be imposed on every legal ISO BMFF file. The proposed parser declares supported brands/addressing variants, reconstructs each fragment's data references, and keeps unknown initialization or chronology explicit.

## E5. Independent decoding

Primary references: [FFmpeg complete documentation](https://www.ffmpeg.org/ffmpeg-all.html) and [format options](https://www.ffmpeg.org/ffmpeg-formats.html).

FFmpeg exposes error-detection controls and permissive error-handling options. The proposed validator records the exact decoder build and strict policy, frame/packet accounting and failures. Its pass is a decoder result; it cannot prove original recording identity, original temporal order, or complete recovery. Permissive previews have separate labels.

## E6. exFAT specification

Primary reference: [Microsoft exFAT specification](https://learn.microsoft.com/en-us/windows/win32/fileio/exfat-specification), particularly sections 6.3.4.2, 7.1 and 7.6.

The allocation bitmap and stream allocation flags have distinct roles. `NoFatChain=1` specifies contiguous allocation and makes the corresponding FAT entries invalid for chain traversal. The proposed adapter checks those semantics, entry consistency, file-length bounds and evidence generation before relying on a layout. Deleted or cross-linked metadata remains qualified.

## E7. Evaluation data

Primary references: [NIST CFReDS](https://cfreds.nist.gov/all/) and [Digital Corpora disk images](https://digitalcorpora.org/corpora/disk-images/).

These provide public forensic test material; Digital Corpora identifies Canon images useful for file-recovery/carving exercises. Their existence does not establish adequate ground truth for modern fragmented MP4/HEVC. The release plan therefore also requires an independently generated, byte-truthed FAT32/exFAT corpus and held-out recorder profiles, including acquisition holes and valid-decode false joins.

## E8. Supporting research records and applicability limits

Consensus completed its research brief and supplied an unmodified report with references. The following selected records were additionally fetched individually before use. Numbered links preserve the corresponding reference numbers in that report. Paper conclusions below are limited to retrieved abstracts; no numerical performance result is treated as verified without inspecting its experimental method and workload.

| Reference | Retrieved evidence | Appropriate role and limit |
| --- | --- | --- |
| Na et al., *Frame-Based Recovery of Corrupted Video Files Using Video Codec Specifications*, IEEE TIP 23(2), 517–526 (2014), DOI [10.1109/TIP.2013.2285625](https://doi.org/10.1109/TIP.2013.2285625). [6](https://consensus.app/papers/framebased-recovery-of-corrupted-video-files-using-video-na-shim/367e96807d825a98a4d03ed6f4b120ad/) | Describes codec-aware extraction and connection of recoverable frames, including non-overwritten portions. | Supports a codec-aware salvage route. It does not establish universal ordering certainty or the proposed engine's throughput. |
| Yoo et al., *A study on multimedia file carving method*, online 2011 / Multimedia Tools and Applications 61, 243–261, DOI [10.1007/s11042-010-0704-y](https://doi.org/10.1007/s11042-010-0704-y). [2](https://consensus.app/papers/a-study-on-multimedia-file-carving-method-yoo-park/dcf168e4e3695a709e1a318f80980562/) | Discusses format-aware contiguous multimedia carving and NTFS compression. | Supports exploiting structure. Its formats and compressed-filesystem setting do not demonstrate fast arbitrary-fragment MP4 recovery on these images. |
| Alghafli, Yeun and Damiani, *The VidCarve Approach*, online 2019 / IEEE TSC 6(1), 131–143 (2021), DOI [10.1109/TSUSC.2019.2914192](https://doi.org/10.1109/TSUSC.2019.2914192). [12](https://consensus.app/papers/techniques-for-measuring-the-probability-of-adjacency-alghafli-yeun/db87ad745094586b93ad99e8f317f68c/) | Describes weighted adjacency and reassembly; bibliographic dates checked against the [authors' institution](https://khazna.ku.ac.ae/en/publications/techniques-for-measuring-the-probability-of-adjacency-between-car/). | Supports ranking uncertain continuations. Probabilistic adjacency cannot certify a unique original sequence. |
| Pal and Memon, *The evolution of file carving*, IEEE SPM 26, 59–71 (2009), DOI [10.1109/MSP.2008.931081](https://doi.org/10.1109/MSP.2008.931081). [18](https://consensus.app/papers/the-evolution-of-file-carving-pal-memon/b49696a3d734553289da439f8604af0a/) | Reviews carving techniques and limitations. | Background for the search problem. Physical NAND/chip-off considerations must not be transferred to controller-addressed partition images as another permutation problem. |

Research about Btrfs, proprietary DVR stores, AVI or raw NAND is not direct validation of FAT32/exFAT MP4 recovery. Preprints and vendor-specific results in the provider report require separate applicability and reproduction checks. This evidence supports a staged experiment; it does not prove a complete 24-hour recovery guarantee.

## E9. Engineering decisions and falsification

The time budgets, candidate caps, Rust core, adapter choices, index representation and resource limits in the design are proposals. They follow the user constraint and explicit failure analysis rather than measured performance on this image collection.

The first implementation decision gate is whether mapped, metadata-supported recovery yields useful validated recordings quickly. If it does, deliver that independently useful release before funding speculative reconstruction. If it does not, inspect the failure categories and remaining evidence before increasing search complexity.

A credible 24-hour result can contain complete supported recordings, conservative segments and a precise unresolved remainder. It cannot trade away false-join control merely to label more files recovered. Speed and recovery yield must be tested together on identical ground-truth work, including the cost of scanning, hashing, decoding, publication, resume and index maintenance.

## E10. MP4-specific evidence

The original design contained MP4 algorithms, but the initial research mixed direct MP4 evidence with generic multimedia, AVI, filesystem-specific and proprietary-recorder studies. That mixture is insufficient as a single evidence basis for this MP4-primary use case. The corrected design and release gates therefore distinguish the following problems:

- An ordinary MP4 physically scattered across FAT32/exFAT clusters, with a surviving logical file map.
- An ordinary MP4 with surviving `moov` sample tables but uncertain or incomplete media placement.
- An MP4 with missing/truncated `moov`, possibly with only `mdat` payload surviving.
- ISO fragmented MP4 using `moof`/`trun`, which is a separate container layout rather than a synonym for filesystem fragmentation.

Raw AVC/HEVC extraction and conversion into a new MP4 cannot, by themselves, demonstrate restoration of an original MP4's sample identity, timing, audio interleaving or completeness.

| Source | Direct relevance | Supported use and limits |
| --- | --- | --- |
| Gloe, Fischer and Kirchner, *Forensic analysis of video file formats* (2014), DOI [10.1016/j.diin.2014.03.009](https://doi.org/10.1016/j.diin.2014.03.009); [primary paper](https://dfrws.org/wp-content/uploads/2019/06/2014_EU_paper-forensic_analysis_of_video_file_formats.pdf) | Direct MP4-like container structure study | Documents device-dependent box/parameter variation and the difficulty of losing a trailing movie box. Supports profile-aware parsing; it is not an MP4-recovery throughput or correctness benchmark. |
| Abdi et al., *Corrupted MP4 Carving Using MP4-Karver* (2016), DOI [10.14569/IJACSA.2016.070313](https://doi.org/10.14569/IJACSA.2016.070313); [primary paper](https://thesai.org/Downloads/Volume7No3/Paper_13-Corrupted_MP4_Carving_Using_MP4_Karver.pdf) | Direct MP4 carving/repair experiment | Evaluates three small images and reports carved/playable counts. The described repair and those counts do not establish preserved original sample/timeline correctness or feasibility on 250 GB images. Its broad conclusion cannot substitute for the proposed ground-truth and scaling gates. |
| [anthwlock/untrunc upstream documentation](https://github.com/anthwlock/untrunc) | Practical truncated MP4/MOV repair implementation | Uses a working reference, ideally from the same camera. It is a comparison candidate for logical-file truncation, not proof of disk-fragment assignment or original timing. The current README also notes FFmpeg internal-ABI compatibility concerns, so any evaluation needs a qualified pinned build. No installation or execution was performed here. |
| Sadeghi, Fahiminia and Teimouri, *Dataset for file fragment classification of video file formats* (2020), DOI [10.1186/s13104-020-05037-x](https://doi.org/10.1186/s13104-020-05037-x); [primary data paper](https://pmc.ncbi.nlm.nih.gov/articles/PMC7160908/) | Contains MP4/codec-labelled fragments | Useful for optional fragment classification tests. Random fixed-size fragments from converted videos do not provide the disk-extent, original-order and full-file recovery ground truth required for this release. |

The [MP4-focused Consensus addendum](<research/mp4/Examine the scientific evidence for a practical, pro-consumer grade VIDEO FILE R.md>) is preserved unchanged, separately from the first report. Its citations keep their own numbering. The primary-source applicability judgments above are independently authored; neither research report is a measured result for the proposed algorithm.

### MP4-first implementation consequences

1. Ship metadata-supported ordinary MP4 recovery first, including audio and complete sample/timing accounting. A file's filesystem extent map is composed before interpreting MP4 logical offsets.
2. With surviving tables, verify all referenced sample spans; retain known gaps instead of inventing samples or guessing away contradictory evidence.
3. With a missing movie box, use an optional donor only for supported codec/configuration or recorder priors. A donor is not the lost recording's sample table. Inferred timing and order retain hypothesis labels unless independent evidence establishes them.
4. Support `moof`-based MP4 through its own addressing/initialization rules and tests.
5. Make MP4 fixture coverage, original encoded samples, logical order, DTS/PTS, audio synchronization, false joins and end-to-end resource costs the primary release gates. Raw Annex B capability is optional and cannot consume the default MP4 recovery budget or block that release.

The 24-hour policy is unchanged. Faster format recognition is useful only when the complete MP4 pipeline also meets its evidence and resource gates.


