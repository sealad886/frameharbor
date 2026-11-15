# Examine the scientific evidence for a practical, pro-consumer grade VIDEO FILE R

**Question:** The user has now clarified after reading the first report: 'my use case has primarily .mp4 files, which I don't think is accounted for in your initial research.' Reassess the evidence specifically for MP4 container restoration, not generic multimedia or raw H.264/H.265 recovery. This is a genuinely new follow-up narrowing applicability. Produce a standalone MP4-focused addendum with primary MP4-specific research and explicit evidence gaps; keep the existing report unchanged. Inputs are raw ddrescue PARTITION images and maps; local read-only metadata identifies FAT32 (16 KiB clusters) and exFAT (128 KiB clusters) recording directories. All paired maps declare finished coverage, but file metadata can be deleted/stale. Normal processing budget24hours perimage, efficiency and low false acceptance central; no existing implementation constraints.

Cover: (1) ordinary MP4 (ftyp/moov/mdat, sample sizes/chunk maps/DTS/PTS/audio interleaving) versus ISO fragmented MP4(moof/trun/tfhd/tfdt); filesystem-fragmented ordinaryMP4 is NOT synonymous with fMP4. (2) Evidence for disk-image extraction with intact directory/FAT/NoFatChain mappings, physical fragmentation with intact moov, carved isolated moov/mdat association, missing/truncated moov, partial/overwritten mdat, and missing fMP4 initialization. (3) MP4-specific published methods, datasets, quantitative evidence and actual limitations; consider Forensic analysis of video file formats(Gloe/Fischer/Kirchner2014), MP4-Karver/corruptedMP4 work, and more recent relevant studies, while marking standards observations, algorithm experiments and narrow tool demonstrations separately. (4) Surviving sample tables give lengths/offsets/timing, NOT hashes of lost payload. Compare length-prefixed AVC/HEVC and AAC sample recognition with raw AnnexB assumptions; guard Bframes, VFR, audio priming, POC/counter wraps, codecconfigchanges and identical recordertemplates. (5) Optional donor/reference files: what can supply codec configuration or recorder priors, what cannot restore missing original per-recording sample tables, frame timing or content identity; donor selection and falsification requirements. (6) Efficient, accuracy-preserving MP4-first algorithm priorities within24h: exact extents and lazy logical views, qualified container topology, shared/coalesced scan and discriminative indexed constraints, bounded missing-layout hypotheses and conservative original-sample segments. (7) Explicit applicability audit of the earlier report: AVI/NTFS-compressed-media, proprietary DVR rawstreams-to-MP4 export, Btrfs and generic adjacency papers cannot by themselves substantiate fast or correct MP4 CONTAINER reconstruction. State whether each relied-on paper directly evaluates MP4 restoration and its scope. Do not equate remuxing raw video INTO MP4 with recovering an original MP4 file. (8) MP4-specific release/accuracy/performance benchmarks and unresolved situations; decoder success does not prove original order, original timing or completeness. Avoid invented speedups or universal recovery claims. Save the full cited addendum as the document; it will be preserved verbatim alongside our independently corrected design.

# **MP4 Container Restoration** From ddrescue Images: Evidence Addendum

The MP4-specific evidence base is thin and uneven: no supplied paper evaluates recovery of an original MP4 container from a raw partition image with deleted or stale FAT32/exFAT directory entries, and none reports end-to-end wall-clock or false-acceptance figures for a 24-hour-per-image MP4 pipeline. What exists splits into three tiers—container-structure forensics that parses MP4 boxes but does not recover them, carving/repair demonstrations that target corrupted MP4 but report coarse playability percentages, and codec-level fragment work that recovers frames rather than the original container. The earlier report's AVI, Btrfs, and proprietary-DVR evidence cannot by itself substantiate fast or correct MP4 container reconstruction.

## Ordinary MP4 vs. Fragmented MP4: A Distinction the Literature Rarely Draws

Ordinary MP4 organizes media as ftyp, moov, and mdat boxes, with the moov carrying the sample tables that map samples to chunks [[1.1]](https://consensus.app/papers/realising-a-push-button-modality-for-videobased-forensics-zawali-ikuesan/fe9f14f9101254dda0b6864f2a7a4d40/) [[2.1]](https://consensus.app/papers/video-forensic-of-fragmented-video-based-on-h264avc-video-sheng-liao/d67139cf71bc5a5998445f13455fd416/) [[3.1]](https://consensus.app/papers/forensic-analysis-of-video-file-formats-gloe-fischer/ff16f68182555041859a1bc87983ad78/). The mdat holds the coded bitstream, and in most fragmented-file cases the recoverable data is mdat content [[2.2]](https://consensus.app/papers/video-forensic-of-fragmented-video-based-on-h264avc-video-sheng-liao/d67139cf71bc5a5998445f13455fd416/) [[2.3]](https://consensus.app/papers/video-forensic-of-fragmented-video-based-on-h264avc-video-sheng-liao/d67139cf71bc5a5998445f13455fd416/). ISO fragmented MP4 instead uses moof atoms containing shorter data chunks of elementary streams [[3.2]](https://consensus.app/papers/forensic-analysis-of-video-file-formats-gloe-fischer/ff16f68182555041859a1bc87983ad78/), a structure Gloe et al. observed in their corpus but did not treat as a recovery target [[3.3]](https://consensus.app/papers/forensic-analysis-of-video-file-formats-gloe-fischer/ff16f68182555041859a1bc87983ad78/).

Critically, **filesystem-level fragmentation of an ordinary MP4 is not the same as fMP4**: the former is a storage-layout problem where moov sample tables still describe the original chunk order, while the latter is a container design where moof/trun boxes carry per-fragment metadata. No supplied paper tests recovery of fMP4 with missing initialization segments, and no paper distinguishes these two senses of "fragmented" in its evaluation. The MP4-Karver work treats a corrupted file as one missing moov, repaired by skipping ftyp/moov and copying mdat [[4.1]](https://consensus.app/papers/corrupted-mp4-carving-using-mp4karver-nur-malik/2156c7868c555f0d90cd3a9451fc16b5/) [[4.2]](https://consensus.app/papers/corrupted-mp4-carving-using-mp4karver-nur-malik/2156c7868c555f0d90cd3a9451fc16b5/)—a repair of an ordinary MP4, not fMP4 reconstruction.

## What Each Relied-On Paper Actually Evaluates

| Paper | Directly evaluates MP4 restoration? | Scope and evidence strength |
|---|---|---|
| MP4-Karver (Abdi et al.) | Yes, corrupted MP4 repair | Reports ~97% average improvement over PhotoRec/Scalpel in playable restoration [[5.1]](https://consensus.app/papers/mp4karver-carving-of-corrupted-mp4-videos-using-asmd-abdi/acbf0b353c8c5f39b3df9b9ab81f4a23/); no byte-level or ordering validation, no dataset description [[5.2]](https://consensus.app/papers/mp4karver-carving-of-corrupted-mp4-videos-using-asmd-abdi/acbf0b353c8c5f39b3df9b9ab81f4a23/) |
| VidCarve (Alghafli et al. 2019) | Partly—video fragments, codec-spec overwritten | Weight-assignment and reassembly for fragments; claims accuracy but reports no MP4-specific byte-correctness figures [[6.1]](https://consensus.app/papers/techniques-for-measuring-the-probability-of-adjacency-alghafli-yeun/db87ad745094586b93ad99e8f317f68c/) [[6.2]](https://consensus.app/papers/techniques-for-measuring-the-probability-of-adjacency-alghafli-yeun/db87ad745094586b93ad99e8f317f68c/) |
| Alghafli & Martin 2016 | Partly—H.264 fragments in MP4 | Precision high, recall low; NIST datasets had only one MP4, bifragment/trifragment, codec spec intact [[7.1]](https://consensus.app/papers/identification-and-recovery-of-video-fragments-for-alghafli-martin/0260dea68a9555f0a3fa6300aa36d84b/) [[7.2]](https://consensus.app/papers/identification-and-recovery-of-video-fragments-for-alghafli-martin/0260dea68a9555f0a3fa6300aa36d84b/) [[7.3]](https://consensus.app/papers/identification-and-recovery-of-video-fragments-for-alghafli-martin/0260dea68a9555f0a3fa6300aa36d84b/) |
| Na et al. 2014 (frame-based) | Codec-level, container-agnostic | Restores frames regardless of fragmentation; uses STSZ box when intact, cluster-size heuristic when corrupted, connects only two fragments [[7.4]](https://consensus.app/papers/identification-and-recovery-of-video-fragments-for-alghafli-martin/0260dea68a9555f0a3fa6300aa36d84b/) [[7.5]](https://consensus.app/papers/identification-and-recovery-of-video-fragments-for-alghafli-martin/0260dea68a9555f0a3fa6300aa36d84b/) [[7.6]](https://consensus.app/papers/identification-and-recovery-of-video-fragments-for-alghafli-martin/0260dea68a9555f0a3fa6300aa36d84b/) |
| Gloe/Fischer/Kirchner 2014 | No—authentication, not recovery | Parses MP4-like containers across 19 cameras, 14 phones, 6 editors for provenance [[3.4]](https://consensus.app/papers/forensic-analysis-of-video-file-formats-gloe-fischer/ff16f68182555041859a1bc87983ad78/) [[3.5]](https://consensus.app/papers/forensic-analysis-of-video-file-formats-gloe-fischer/ff16f68182555041859a1bc87983ad78/) |
| Iuliani et al. 2019 | No—integrity/brand attribution | Unsupervised container analysis on 578 smartphone videos; low computational cost but not recovery [[8.1]](https://consensus.app/papers/a-video-forensic-framework-for-the-unsupervised-analysis-iuliani-shullani/81f5a15ec3be5410b272e3111e6cc818/) [[8.2]](https://consensus.app/papers/a-video-forensic-framework-for-the-unsupervised-analysis-iuliani-shullani/81f5a15ec3be5410b272e3111e6cc818/) |
| Xiang et al. 2021 | No—metadata forensics | MP4/MOV metadata tree features for attribution and manipulation detection [[9.1]](https://consensus.app/papers/forensic-analysis-of-video-files-using-metadata-xiang-horváth/a274bbbeb63659549e19867b98352f1c/) [[9.2]](https://consensus.app/papers/forensic-analysis-of-video-files-using-metadata-xiang-horváth/a274bbbeb63659549e19867b98352f1c/) |
| Altinisik & Sencar 2021 | Codec-level header generation | Generates H.264 headers for fragments lacking parameters; 11.3 decode trials average on 55K videos [[10.1]](https://consensus.app/papers/automatic-generation-of-h264-parameter-sets-to-recover-altinisik-sencar/591b7702543d5d8b82dad82ad46f0b2a/) |
| Sheng et al. 2014 | Codec-level IDR recovery | Restores IDR images from fragmented MP4 by reconstructing SPS/PPS; cannot restore video sequence [[2.4]](https://consensus.app/papers/video-forensic-of-fragmented-video-based-on-h264avc-video-sheng-liao/d67139cf71bc5a5998445f13455fd416/) [[2.5]](https://consensus.app/papers/video-forensic-of-fragmented-video-based-on-h264avc-video-sheng-liao/d67139cf71bc5a5998445f13455fd416/) |
| Rzayeva et al. 2025 | Proprietary DVR, not standard MP4 | 91.8% recovery, 2.4% false positive on Hikvision/Dahua; MP4 conversion is an export step, not container reconstruction [[11.1]](https://consensus.app/papers/automated-forensic-recovery-methodology-for-video-rzayeva-shayakhmetov/46acbb6795b35ef992548f108ffe8018/) [[11.2]](https://consensus.app/papers/automated-forensic-recovery-methodology-for-video-rzayeva-shayakhmetov/46acbb6795b35ef992548f108ffe8018/) |
| Yoon & Hwang 2026 | Proprietary Honeywell, not MP4 | Feasibility only; quantitative evaluation explicitly left for future work [[12.1]](https://consensus.app/papers/forensic-analysis-of-video-data-deletion-and-recovery-in-yoon-hwang/ae904860d16e59f7bdeb06eae3000448/) |
| Zawali et al. 2021 | No—tamper detection | MP4 signature identification and alteration detection, not recovery [[1.2]](https://consensus.app/papers/realising-a-push-button-modality-for-videobased-forensics-zawali-ikuesan/fe9f14f9101254dda0b6864f2a7a4d40/) [[1.3]](https://consensus.app/papers/realising-a-push-button-modality-for-videobased-forensics-zawali-ikuesan/fe9f14f9101254dda0b6864f2a7a4d40/) |

**Figure 1:** Applicability audit of supplied papers to MP4 container restoration

## Surviving Sample Tables, Codec Constraints, and Donor Files

When moov survives, its sample tables give lengths, offsets, and timing—but **not hashes of lost payload**, so they constrain candidate reassembly without proving content identity. Na et al. exploit the STSZ box for frame lengths and fall back to cluster-size heuristics when it is corrupted, connecting only two fragments at a time [[7.4]](https://consensus.app/papers/identification-and-recovery-of-video-fragments-for-alghafli-martin/0260dea68a9555f0a3fa6300aa36d84b/) [[7.5]](https://consensus.app/papers/identification-and-recovery-of-video-fragments-for-alghafli-martin/0260dea68a9555f0a3fa6300aa36d84b/) [[7.7]](https://consensus.app/papers/identification-and-recovery-of-video-fragments-for-alghafli-martin/0260dea68a9555f0a3fa6300aa36d84b/). VidCarve estimates adjacency probabilities from codec parameters when those parameters are overwritten [[6.3]](https://consensus.app/papers/techniques-for-measuring-the-probability-of-adjacency-alghafli-yeun/db87ad745094586b93ad99e8f317f68c/). Altinisik and Sencar show that missing H.264 parameter sets can be regenerated from a learned parameter dictionary, cutting decode trials by more than 10× versus header stitching [[10.2]](https://consensus.app/papers/automatic-generation-of-h264-parameter-sets-to-recover-altinisik-sencar/591b7702543d5d8b82dad82ad46f0b2a/) [[10.1]](https://consensus.app/papers/automatic-generation-of-h264-parameter-sets-to-recover-altinisik-sencar/591b7702543d5d8b82dad82ad46f0b2a/)—but this recovers decodable frames, not the original container's sample ordering or timing.

Donor/reference files can supply codec configuration and recorder priors: a lost file header can be reconstructed from a reference video from the same device, with missing data zero-filled [[4.3]](https://consensus.app/papers/corrupted-mp4-carving-using-mp4karver-nur-malik/2156c7868c555f0d90cd3a9451fc16b5/). What donors cannot supply is the original per-recording sample table, frame timing, or content identity—no supplied paper demonstrates donor-based recovery of a lost moov's chunk offsets or DTS/PTS. Gloe et al. show that container structure varies by manufacturer and model [[3.6]](https://consensus.app/papers/forensic-analysis-of-video-file-formats-gloe-fischer/ff16f68182555041859a1bc87983ad78/), which supports donor priors for codec config but also warns that identical recorder templates will produce ambiguous candidates. Xiang et al. note that MP4's vendor-specific nuances require separate parsing strategies [[9.3]](https://consensus.app/papers/forensic-analysis-of-video-files-using-metadata-xiang-horváth/a274bbbeb63659549e19867b98352f1c/), and that metadata-based methods fail on some social-network-processed videos [[9.4]](https://consensus.app/papers/forensic-analysis-of-video-files-using-metadata-xiang-horváth/a274bbbeb63659549e19867b98352f1c/).

## Evidence Gaps and Unresolved Situations

| | Byte correctness | Ordering proof | False acceptance | Wallclock | Dataset scale |
|---|---|---|---|---|---|
| Ordinary MP4 repair |  |  |  |  |  |
| Fragmented MP4 |  |  |  |  |  |
| Codec fragment recovery |  |  |  |  |  |
| Container forensics |  |  |  |  |  |
| Proprietary DVR |  |  |  |  |  |

The most striking gap is **ordering and timing proof**: no supplied paper validates that a recovered MP4's sample order, DTS/PTS, or audio interleaving matches the original. Na et al. restore frames "regardless of fragmentation" but do not verify original sequence [[13.1]](https://consensus.app/papers/framebased-recovery-of-corrupted-video-files-using-video-na-shim/367e96807d825a98a4d03ed6f4b120ad/); Sheng et al. explicitly cannot restore video sequence, only IDR images [[2.5]](https://consensus.app/papers/video-forensic-of-fragmented-video-based-on-h264avc-video-sheng-liao/d67139cf71bc5a5998445f13455fd416/); MP4-Karver reports playability percentages without byte-level comparison [[5.1]](https://consensus.app/papers/mp4karver-carving-of-corrupted-mp4-videos-using-asmd-abdi/acbf0b353c8c5f39b3df9b9ab81f4a23/). Decoder success therefore cannot stand as evidence of correct original order or completeness.

A second gap is **dataset realism**. The NIST CFReDS sets used by Alghafli and Martin contained only one MP4, fragmented into two or three pieces with codec specification intact [[7.1]](https://consensus.app/papers/identification-and-recovery-of-video-fragments-for-alghafli-martin/0260dea68a9555f0a3fa6300aa36d84b/), and none had H.264 codec-spec overwritten [[7.8]](https://consensus.app/papers/identification-and-recovery-of-video-fragments-for-alghafli-martin/0260dea68a9555f0a3fa6300aa36d84b/). Altinisik and Sencar's 104,524-video corpus is large but built for header generation, not container recovery [[10.3]](https://consensus.app/papers/automatic-generation-of-h264-parameter-sets-to-recover-altinisik-sencar/591b7702543d5d8b82dad82ad46f0b2a/). No supplied study reports false-acceptance rates for MP4 container reconstruction, and none reports wall-clock or I/O amplification for a 24-hour budget. The proprietary-DVR papers report false-positive rates (2.4%) but for vendor formats, not standard MP4 [[11.1]](https://consensus.app/papers/automated-forensic-recovery-methodology-for-video-rzayeva-shayakhmetov/46acbb6795b35ef992548f108ffe8018/). The field's own surveys conclude that advanced multimedia carving remains open [[14.1]](https://consensus.app/papers/file-carving-survey-on-techniques-tools-and-areas-of-use-alrobieh-raqpan/35687f8422c857e49e73268203f52843/), and that video carvers are often not robust enough to recover even single frames when data is corrupted [[15.1]](https://consensus.app/papers/automating-video-file-carving-and-content-identification-yannikos-ashraf/97d82dc3c2f85eea9b456bf6619a9d2d/).

For a 24-hour MP4-first pipeline, the evidence supports prioritizing exact extents from surviving FAT/NoFatChain mappings, then moov-guided sample-table reconstruction when moov survives, then codec-constrained frame recovery as a bounded fallback—while treating every output as a hypothesis whose original ordering and timing remain unproven unless independently validated.
 
_These search results were found and analyzed using Consensus, an AI-powered search engine for research. Try it at https://consensus.app. © 2026 Consensus NLP, Inc. Personal, non-commercial use only; redistribution requires copyright holders’ consent._
 
## References
 
`1.` Zawali B, Ikuesan R, Kebande V, Furnell S, A-Dhaqm A. Realising a Push Button Modality for Video-Based Forensics. *Infrastructures.* 2021;6:54. doi:10.3390/infrastructures6040054
 
> [1.1] An MP4 file has three sections: header (ftyp), video data (mdat), index information (moov), as shown in Figure (Body)
 
> [1.2] The MP4 video format (MPEG-4 Section 14, also known as MPEG-4 AVC, where AVC denotes Advanced Video Coding and MPEG refers to Motion Picture Expert Group) is one of the most common digital multimedia formats for storing video and audio. (Body)
 
> [1.3] A comprehensive evaluation of the efficiency of the tool against file alteration showed that the tool was capable of identifying falsified files, which satisfied the underlying assertion of the PBFM model. (Abstract)
 
`2.` Sheng K, Liao X, Zhang Q, Qu J, Tan Y. Video Forensic of Fragmented Video Based on H.264/AVC Video Compression Standard. 2014. doi:10.2991/meic-14.2014.111
 
> [2.1] (a) that MP4 file consists of three large boxes, respectively are ftyp box, moov box and mdat box, which are used to indicate the file type, store the media information and store the media data separately. (Body)
 
> [2.2] The mdat box is the key box that we need to pay further attention, and bitstream data is stored in this box. (Body)
 
> [2.3] Meanwhile, in most cases, video file fragmentation obtained is data from the mdat box. (Body)
 
> [2.4] This paper just focused on video forensic of incomplete files in the disk and satisfactory results were achieved by restoring images of IDR frame from fragmented video through the reconstruction of SPS (sequence parameter set) and PPS (picture parameter set). (Body)
 
> [2.5] 4. At present, only IDR image can be restored, while nothing can be do with video sequence. (Body)
 
`3.` Gloe T, Fischer A, Kirchner M. Forensic analysis of video file formats. *Digit. Investig..* 2014;11:S68-S76. doi:10.1016/j.diin.2014.03.009
 
> [3.1] 4 The actual data stream is stored in the mdat atom, which is accompanied by corresponding metadata in the moov atom. (Body)
 
> [3.2] We also encountered files with moof atoms, which contain shorter data chunks of elementary streams. (Body)
 
> [3.3] Based on a description of our (Test setup) and (General observations) on video file format forensics, the following sections demonstrate that peculiarities of the (AVI Container format), of (Quicktime and related container formats (MP4, 3GP)), and of (MJPEG Compression parameters) can yield important insights about provenance and processing history of digital videos. (Body)
 
> [3.4] We use customized parsers to extract all file format structures of videos from overall 19 digital camera models, 14 mobile phone models, and 6 video editing toolboxes. (Abstract)
 
> [3.5] Note that this differs from video file carving (Pal and Memon, 2009;Lewis, 2012), where it is usually sufficient to find valid video streams in (fragmented) mass storage dumps. (Body)
 
> [3.6] Similar to differences in the JPEG file structure, we identify manufacturer-and model-specific video file format characteristics and point to traces left by processing software. (Body)
 
`4.` Nur A, Malik K, Mohammed Y, Naseem R, Aamir M. Corrupted MP4 Carving Using MP4-Karver. *International Journal of Advanced Computer Science and Applications.* 2016;7. doi:10.14569/ijacsa.2016.070313
 
> [4.1] Also, this figure shows the corrupt video file which looks a like the normal structure however, it contains the 'ftyp' and 'mdat' atoms only. (Body)
 
> [4.2] Therefore, the file can be repaired simply by skipping over the initial 'ftyp' and 'moov' atoms, and the header for the 'mdat' atom, and copying the remaining data into a new file. (Body)
 
> [4.3] 1) Retrieving video. (Abstract)
 
`5.` Abdi A. MP4-Karver: carving of corrupted MP4 videos using ASMD repairing technique. 2017.
 
> [5.1] The experimental result shows that the proposed MP4-Karver tool increases the restoration carving and repairing of MP4 corrupted videos with average of 97% improvement as compared to PhotoRec and Scalpel. (Abstract)
 
> [5.2] The proposed MP4-Karver tool focuses on carving, repair corrupted MP4 videos and getting a higher successfulrate of playable MP4 video file format. (Abstract)
 
`6.` Alghafli K, Yeun C, Damiani E. Techniques for Measuring the Probability of Adjacency between Carved Video Fragments: The VidCarve Approach. *IEEE Transactions on Sustainable Computing.* 2019;6:131-143. doi:10.1109/tsusc.2019.2914192
 
> [6.1] This paper focuses on the weight assignment and reassembly processes where the codec specification parameters of carved fragments were overwritten. (Abstract)
 
> [6.2] We provide experimental results for the proposed techniques of weight assignment and reassembly. (Abstract)
 
> [6.3] We propose several weight assignment techniques to estimate the probability of adjacency between video fragments. (Abstract)
 
`7.` Alghafli K, Martin T. Identification and recovery of video fragments for forensics file carving. *2016 11th International Conference for Internet Technology and Secured Transactions (ICITST).* 2016. doi:10.1109/icitst.2016.7856710
 
> [7.1] These datasets have only one MP4 file based on the H.264 bitstream, fragmented into two or three fragments with the codec specification intact. (Methods)
 
> [7.2] Our experiments have shown that the precision of our prototype and video carving tools were high. (Results)
 
> [7.3] However, the recall values were not. (Results)
 
> [7.4] The process of connecting valid frames relies on the information on frame length that is stored in a STSZ box of the video container. (Methods)
 
> [7.5] In the cases where the STSZ box was corrupted or overwritten, they proposed a method to connect two set of frames according to the cluster size. (Methods)
 
> [7.6] Their proposed method only connects two fragments. (Methods)
 
> [7.7] The main limitation of their approach was that in the case of STSZ box was corrupted or overwritten, they cannot connect all the video fragments. (Methods)
 
> [7.8] However, none of these 5 datasets have a video file based on the H.264 bitstream with the codec specification part overwritten. (Methods)
 
`8.` Iuliani M, Shullani D, Fontani M, Meucci S, Piva A. A Video Forensic Framework for the Unsupervised Analysis of MP4-Like File Container. *IEEE Transactions on Information Forensics and Security.* 2019;14:635-645. doi:10.1109/tifs.2018.2859760
 
> [8.1] In this paper, we introduce a method for unsupervised analysis of video file containers, and present two main forensic applications of such method: the first one deals with video integrity verification, based on the dissimilarity between a reference and a query file container; the second one focuses on the identification and classification of the source device brand, based on the analysis of containers structure and content. (Abstract)
 
> [8.2] We tested and proved the effectiveness of both applications on a dataset composed by 578 videos taken with modern smartphones from major brands and models. (Abstract)
 
`9.` Xiang Z, Horváth J, Baireddy S, Bestagini P, Tubaro S, Delp E. Forensic Analysis of Video Files Using Metadata. *2021 IEEE/CVF Conference on Computer Vision and Pattern Recognition Workshops (CVPRW).* 2021. doi:10.1109/cvprw53098.2021.00115
 
> [9.1] In this paper, we leverage the seminal work presented in [20, 39] to investigate the use of metadata for video forensic analysis of the MP4 and MOV video formats, which are among the most popular video wrappers/containers. (Introduction)
 
> [9.2] In this paper we propose a framework that exploits an MP4 video file's metadata to solve multiple video forensics problems, including brand attribution, video editing tool identification, social network attribution, and manipulation detection. (Introduction)
 
> [9.3] The MP4 standard is around twenty years old, it contains numerous vendor-specific nuances that require separate parsing strategies. (Introduction)
 
> [9.4] Our research also exposed the limitation of metadata-based forensics methods, namely its failure to analyze videos from specific social networks such as TikTok and WeiBo. (Conclusion)
 
`10.` Altinisik E, Sencar H. Automatic Generation of H.264 Parameter Sets to Recover Video File Fragments. *IEEE Transactions on Information Forensics and Security.* 2021;16:4857-4868. doi:10.1109/tifs.2021.3118876
 
> [10.1] Evaluation of the effectiveness of the proposed method on more than 55K videos with diverse provenance shows that it can generate valid headers on average in 11.3 decoding trials per video. (Abstract)
 
> [10.2] To accomplish this, we examined a very large corpus of videos to learn patterns of encoding settings commonly used by encoders and created a parameter dictionary. (Abstract)
 
> [10.3] Overall, this resulted in a dataset of 104,524 videos in MP4 container file format. (Introduction)
 
`11.` Rzayeva L, Shayakhmetov M, Atanbayev Y, Budenov R. Automated Forensic Recovery Methodology for Video Evidence from Hikvision and Dahua DVR/NVR Systems. *Inf..* 2025;16:983. doi:10.3390/info16110983
 
> [11.1] Testing on 27 surveillance hard drives showed a recovery rate of 91.8, a temporal accuracy of 96.7% and a false positive rate of 2.4%—the lowest of the tools tested with statistically significant improvements over commercial tools (p < 0.01). (Abstract)
 
> [11.2] The open methodology offers the necessary algorithmic transparency to be court-admissible, and the automated MP4 conversion with metadata left intact makes the integration of forensic workflow possible. (Abstract)
 
`12.` Yoon J, Hwang S. Forensic analysis of video data deletion and recovery in Honeywell surveillance file system. *ArXiv.* 2026;abs/2605.07430. doi:10.48550/arxiv.2605.07430
 
> [12.1] In this study, we demonstrate that video recovery is feasible, and leave a comprehensive quantitative evaluation for future work. (Discussion)
 
`13.` Na G, Shim K, Moon K, Kong SG, Kim E, Lee J. Frame-Based Recovery of Corrupted Video Files Using Video Codec Specifications. *IEEE Transactions on Image Processing.* 2014;23:517-526. doi:10.1109/tip.2013.2285625
 
> [13.1] Experiment results show that the proposed technique successfully restores fragmented video files regardless of the amount of fragmentations. (Abstract)
 
`14.` Alrobieh ZS, Raqpan AMAA. File Carving Survey on Techniques, Tools and Areas of Use. *Transactions on Networks and Communications.* 2020. doi:10.14738/tnc.81.7636
 
> [14.1] We conclude that proposing new advanced method for carving multimedia files still open and new direction for future research. (Abstract)
 
`15.` Yannikos Y, Ashraf N, Steinebach M, Winter C. Automating Video File Carving and Content Identification. 2013. doi:10.1007/978-3-642-41148-9_14
 
> [15.1] Additionally, multimedia file carvers are often not robust enough – recovering single frames of video files is often not possible if some of the data is corrupted or missing. (Abstract)
