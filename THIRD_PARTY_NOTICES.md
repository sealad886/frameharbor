# Third-party notices

This source preview does not vendor Rust crates, TSK, FFmpeg, their libraries,
compiled helpers or private media. Builds obtain the exact Rust dependencies in
`Cargo.lock` and use separately installed native libraries. This file inventories
those dependencies and links their **separate, unmodified license texts**; it
does not combine those texts with the project license.

Project-owned code is **GPL-3.0-or-later**, under [LICENSE](LICENSE) and the
[GNU GPLv3 terms](https://www.gnu.org/licenses/gpl-3.0.html). This grant does not
replace third-party terms or relicense preserved provider research, quoted
sources, third-party documentation or dependency code. Keep their attribution
and original rights notices. Historical planning/provider documents remain
verbatim; their presence is not a grant of additional third-party rights.

## Locked Rust dependencies

The following license expressions were read from the exact cached crate
manifests corresponding to `Cargo.lock` on 15 November 2026. Files below are
verbatim copies from those crate packages, including their copyright notices.
`MIT/Apache-2.0` is the older dual-license expression used by those packages;
both alternatives are retained. These are dependency and build-dependency
notices, not a claim that their code was copied into this source tree.

| Package | License expression | Separate license texts |
| --- | --- | --- |
| bitflags 2.13.2 | MIT OR Apache-2.0 | [MIT](licenses/third-party/bitflags-2.13.2/LICENSE-MIT), [Apache-2.0](licenses/third-party/bitflags-2.13.2/LICENSE-APACHE) |
| block-buffer 0.10.4 | MIT OR Apache-2.0 | [MIT](licenses/third-party/block-buffer-0.10.4/LICENSE-MIT), [Apache-2.0](licenses/third-party/block-buffer-0.10.4/LICENSE-APACHE) |
| cc 1.11.1 | MIT OR Apache-2.0 | [MIT](licenses/third-party/cc-1.11.1/LICENSE-MIT), [Apache-2.0](licenses/third-party/cc-1.11.1/LICENSE-APACHE) |
| cfg-if 1.0.5 | MIT OR Apache-2.0 | [MIT](licenses/third-party/cfg-if-1.0.5/LICENSE-MIT), [Apache-2.0](licenses/third-party/cfg-if-1.0.5/LICENSE-APACHE) |
| cpufeatures 0.2.17 | MIT OR Apache-2.0 | [MIT](licenses/third-party/cpufeatures-0.2.17/LICENSE-MIT), [Apache-2.0](licenses/third-party/cpufeatures-0.2.17/LICENSE-APACHE) |
| crypto-common 0.1.7 | MIT OR Apache-2.0 | [MIT](licenses/third-party/crypto-common-0.1.7/LICENSE-MIT), [Apache-2.0](licenses/third-party/crypto-common-0.1.7/LICENSE-APACHE) |
| digest 0.10.7 | MIT OR Apache-2.0 | [MIT](licenses/third-party/digest-0.10.7/LICENSE-MIT), [Apache-2.0](licenses/third-party/digest-0.10.7/LICENSE-APACHE) |
| fallible-iterator 0.3.0 | MIT/Apache-2.0 | [MIT](licenses/third-party/fallible-iterator-0.3.0/LICENSE-MIT), [Apache-2.0](licenses/third-party/fallible-iterator-0.3.0/LICENSE-APACHE) |
| fallible-streaming-iterator 0.1.9 | MIT/Apache-2.0 | [MIT](licenses/third-party/fallible-streaming-iterator-0.1.9/LICENSE-MIT), [Apache-2.0](licenses/third-party/fallible-streaming-iterator-0.1.9/LICENSE-APACHE) |
| find-msvc-tools 0.1.15 | MIT OR Apache-2.0 | [MIT](licenses/third-party/find-msvc-tools-0.1.15/LICENSE-MIT), [Apache-2.0](licenses/third-party/find-msvc-tools-0.1.15/LICENSE-APACHE) |
| generic-array 0.14.7 | MIT | [MIT](licenses/third-party/generic-array-0.14.7/LICENSE) |
| libc 0.2.190 | MIT OR Apache-2.0 | [MIT](licenses/third-party/libc-0.2.190/LICENSE-MIT), [Apache-2.0](licenses/third-party/libc-0.2.190/LICENSE-APACHE) |
| libsqlite3-sys 0.39.1 | MIT | [MIT](licenses/third-party/libsqlite3-sys-0.39.1/LICENSE) |
| pkg-config 0.3.34 | MIT OR Apache-2.0 | [MIT](licenses/third-party/pkg-config-0.3.34/LICENSE-MIT), [Apache-2.0](licenses/third-party/pkg-config-0.3.34/LICENSE-APACHE) |
| rusqlite 0.41.1 | MIT | [MIT](licenses/third-party/rusqlite-0.41.1/LICENSE) |
| sha2 0.10.9 | MIT OR Apache-2.0 | [MIT](licenses/third-party/sha2-0.10.9/LICENSE-MIT), [Apache-2.0](licenses/third-party/sha2-0.10.9/LICENSE-APACHE) |
| shlex 2.0.1 | MIT OR Apache-2.0 | [MIT](licenses/third-party/shlex-2.0.1/LICENSE-MIT), [Apache-2.0](licenses/third-party/shlex-2.0.1/LICENSE-APACHE) |
| smallvec 1.16.5 | MIT OR Apache-2.0 | [MIT](licenses/third-party/smallvec-1.16.5/LICENSE-MIT), [Apache-2.0](licenses/third-party/smallvec-1.16.5/LICENSE-APACHE) |
| typenum 1.20.1 | MIT OR Apache-2.0 | [MIT](licenses/third-party/typenum-1.20.1/LICENSE-MIT), [Apache-2.0](licenses/third-party/typenum-1.20.1/LICENSE-APACHE), [upstream expression](licenses/third-party/typenum-1.20.1/LICENSE) |
| vcpkg 0.2.15 | MIT/Apache-2.0 | [MIT](licenses/third-party/vcpkg-0.2.15/LICENSE-MIT), [Apache-2.0](licenses/third-party/vcpkg-0.2.15/LICENSE-APACHE) |
| version_check 0.9.5 | MIT/Apache-2.0 | [MIT](licenses/third-party/version_check-0.9.5/LICENSE-MIT), [Apache-2.0](licenses/third-party/version_check-0.9.5/LICENSE-APACHE) |

`libsqlite3-sys` bundles SQLite 3.54.1 at build time. SQLite's deliverable core
is [public domain](https://sqlite.org/copyright.html), separately from its Rust
bindings' MIT license. The [verbatim public-domain notice](licenses/third-party/sqlite-3.54.1/PUBLIC-DOMAIN-NOTICE.txt)
is the opening notice from the exact bundled `sqlite3.h`; no license text has
been invented for SQLite. No SQLite source files are vendored by this preview.

The Rust compiler/standard library are external build tools. Their terms are
described by the [Rust licensing policy](https://www.rust-lang.org/policies/licenses).

## External native dependencies

### FFmpeg 9.0.2

The selected installed build is Homebrew `ffmpeg-full/9.0.2_2`, reached through
`/opt/homebrew/opt/ffmpeg-full`. It reports libavformat/libavcodec 63.1.102 and
libavutil 61.1.102. `ffmpeg -L` identifies GPL-3.0-or-later; configuration enables
`--enable-gpl` and `--enable-version3`. FrameHarbor's media helper dynamically
links these libraries for local validation. This is not an LGPL-only build.

The installed build's separate, verbatim texts are retained as
[GPLv3](licenses/third-party/ffmpeg-9.0.2/COPYING.GPLv3),
[GPLv2](licenses/third-party/ffmpeg-9.0.2/COPYING.GPLv2),
[LGPLv3](licenses/third-party/ffmpeg-9.0.2/COPYING.LGPLv3), and
[LGPLv2.1](licenses/third-party/ffmpeg-9.0.2/COPYING.LGPLv2.1).
The separate upstream [license overview](licenses/third-party/ffmpeg-9.0.2/LICENSE.md)
explains per-file and optional-library terms; see also
[FFmpeg's official licensing guidance](https://ffmpeg.org/legal.html).
Retaining alternative texts does not change the selected build's GPLv3-or-later
configuration.

This source preview ships neither FFmpeg nor its transitive native libraries.
No complete redistribution audit of that installed native dependency graph is
claimed. A future binary distribution must review its exact linked components,
corresponding source, attribution and other applicable obligations; an external
library path or a hash alone is not that audit.

### The Sleuth Kit 4.14.0

The filesystem helper uses installed TSK 4.14.0 headers and
`/usr/local/lib/libtsk.23.dylib`. The
[official overview](https://www.sleuthkit.org/sleuthkit/licenses.php) and
[pinned 4.14.0 notices](https://github.com/sleuthkit/sleuthkit/blob/sleuthkit-4.14.0/licenses/README.md)
identify per-file terms, including IPL/CPL core code and additional components.
Upstream 4.14.0 texts are retained separately as
[IBM Public License](licenses/third-party/sleuthkit-4.14.0/IBM-LICENSE),
[Common Public License 1.0](licenses/third-party/sleuthkit-4.14.0/cpl1.0.txt),
[Apache-2.0](licenses/third-party/sleuthkit-4.14.0/Apache-LICENSE-2.0.txt),
[BSD](licenses/third-party/sleuthkit-4.14.0/bsd.txt), and
[MIT](licenses/third-party/sleuthkit-4.14.0/mit.txt).
Each file was downloaded unchanged from that upstream tag's `licenses/`
directory; these are not project-authored substitute licenses.

TSK is not vendored or distributed as a binary here. These selected upstream
texts and links are an inventory, not a claim that every installed TSK build
component has been audited or that the project's GPL grant overrides TSK terms.

## Preserved research and documentation

`docs/planning/image-first-video-restoration/` contains relocated historical
planning, source references and provider research reports. Their original
attributions and contents are preserved. The GPL grant for project-owned code
does not relicense these third-party reports or any quoted source. Research
citations are evidence references, not dependency redistribution licenses.
