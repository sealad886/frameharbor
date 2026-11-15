# TSK candidate qualification probe

Observed 9 November 2026 on this Apple Silicon Mac. This is preparatory dependency evidence, not an accepted filesystem adapter or M3 gate. No image, map, private recording or real recovery case was opened. The program accepts no source path and produces an arithmetic pattern in memory.

The optional [native probe](../../native/tsk_abi_probe.c) compiles against the selected installed headers and an explicit library, then runs in a separate process:

```sh
cc -std=c11 -Wall -Wextra -Werror -I/usr/local/include \
  native/tsk_abi_probe.c /usr/local/lib/libtsk.23.dylib \
  -o target/tsk-abi-probe
./target/tsk-abi-probe
```

Observed results:

| Observation | Result |
| --- | --- |
| Header / runtime version | 4.14.0 / 4.14.0 |
| `sizeof(TSK_IMG_INFO)` | 2,097,920 bytes |
| `sizeof(TSK_FS_ATTR_RUN)` | 48 bytes |
| Requested logical read | 31 bytes at byte offset 513 |
| External read callbacks | 1 |
| Actual callback bytes / maximum callback size | 65,536 / 65,536 bytes |
| Arithmetic byte-pattern comparison | Pass |
| Callback outside synthetic image bounds | None |

A deliberate mismatch build linked the same probe to the co-installed
`/usr/local/lib/libtsk.22.dylib`. That library reported `4.15.0-develop` against
the `4.14.0` headers. The probe exited **2**, before constructing any image
object, with `header/runtime mismatch: adapter must remain disabled`. Library
filename numbering alone therefore cannot select the matching implementation.

The read cache amplifies a 31-byte request to a 64 KiB callback. A future helper must route and charge **every callback byte**, including metadata/prefetch, through acquisition masks and the case budget. It cannot meter only logical file payload. Denying a prefetch that crosses unavailable acquisition bytes is conservative; substituting stale destination bytes would violate the source contract. The approximately 2 MiB image object and native decoder/parser RSS belong to the host budget.

The exact [4.14.0 upstream implementation](https://github.com/sleuthkit/sleuthkit/blob/sleuthkit-4.14.0/tsk/img/img_open.cpp) defines the external image structure with `TSK_IMG_INFO` first, initializes cache locking, takes byte offsets in the supplied callback and calls the supplied close function after lock cleanup. The installed headers were checked directly because older online API documentation cannot certify this build. Callback function signatures compiled with warnings denied and the in-memory lifecycle/read check passed. This does not certify all structure layout, malformed-input behavior, filesystem ownership evidence, cancellation or FAT32/exFAT semantics.

SHA-256 evidence:

| Artifact | Digest |
| --- | --- |
| `native/tsk_abi_probe.c` | `234124df06111385e121c0dc1cedae919c55a310d5b1dcb6dafeae41756d2076` |
| Installed `tsk/img/tsk_img.h` | `2b53a6405c22207827ef00906953de4e378216ca4b267764f737b82ae8a8463b` |
| Installed `tsk/fs/tsk_fs.h` | `837d719e6113760c3c69a3aea0661d0a3c0c6093459f0d0186d4190ab3e64781` |
| `/usr/local/lib/libtsk.23.dylib` | `dbc17f4125bf6d7d11c77180a9e1e7569b2af426b3d7d433f1a74297caf08b09` |

The selected dylib is arm64 and links the existing system C/C++/SQLite/zlib libraries and installed libewf. No dependency was installed for this probe. A full helper must freeze its complete loaded-library manifest, run resource-bounded hostile-input tests, preserve RECOVERY/SPARSE/FILLER and ownership qualifiers, and certify byte/sector/cluster normalization before emitting supported logical extents. There is no fallback that promotes `fls`/`icat` output or guessed contiguous runs to exact layouts.
