#!/bin/sh
# Produce a reviewable local-host bundle; never install or publish it.
set -eu
project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
output_dir=${1:-"$project_dir/target/local-package"}
case "$output_dir" in /*) ;; *) output_dir="$PWD/$output_dir" ;; esac
if [ -e "$output_dir" ] || [ -L "$output_dir" ]; then
    printf 'Destination already exists; preserve it and select a fresh path: %s\n' "$output_dir" >&2
    exit 1
fi
if [ "$(uname -s)" != Darwin ] || [ "$(uname -m)" != arm64 ]; then
    printf 'This package profile requires the selected Darwin arm64 host.\n' >&2
    exit 1
fi
toolchain_dir="$HOME/.rustup/toolchains/1.98.1-aarch64-apple-darwin/bin"
PATH="$toolchain_dir:/usr/bin:/bin:/opt/homebrew/bin"
export PATH
case "$(rustc --version)" in 'rustc 1.98.1 '*) ;; *)
    printf 'Rust 1.98.1 is required; no dependency installation attempted.\n' >&2
    exit 1;;
esac
cd "$project_dir"
cargo build --release --offline --locked
mkdir -p "$(dirname -- "$output_dir")"
stage=$(mktemp -d "${output_dir}.stage.XXXXXX")
# Preserve failed stages as local build evidence; no automatic deletion.
mkdir "$stage/bin" "$stage/bin/helpers" "$stage/docs" "$stage/native" "$stage/licenses"
cp target/release/frameharbor "$stage/bin/frameharbor"
native/build_helpers.sh "$stage/bin/helpers"
cp docs/implementation/LOCAL_PACKAGE.md "$stage/OPERATING.md"
cp -R docs/implementation "$stage/docs/implementation"
cp native/*.c native/*.h native/*.sh "$stage/native/"
cp Cargo.toml Cargo.lock rust-toolchain.toml "$stage/"
cp LICENSE THIRD_PARTY_NOTICES.md "$stage/"
cp -R licenses/third-party "$stage/licenses/"
{
    printf 'FrameHarbor local-host development package\n'
    printf 'Built UTC: '; date -u '+%Y-%m-%dT%H:%M:%SZ'
    printf 'Host: '; uname -sm
    rustc --version
    cargo --version
    printf '\nDynamic dependencies (required on destination host):\n'
    /usr/bin/otool -L "$stage/bin/frameharbor" "$stage/bin/helpers/tsk-helper" "$stage/bin/helpers/ffmpeg-helper"
    printf '\nSelected external library fingerprints (SHA-256):\n'
    /usr/bin/shasum -a 256 /usr/local/lib/libtsk.23.dylib \
        /opt/homebrew/opt/ffmpeg-full/lib/libavformat.63.dylib \
        /opt/homebrew/opt/ffmpeg-full/lib/libavcodec.63.dylib \
        /opt/homebrew/opt/ffmpeg-full/lib/libavutil.61.dylib
    printf '\nSource fingerprints (SHA-256):\n'
    /usr/bin/find src native scripts -type f ! -name .DS_Store -exec /usr/bin/shasum -a 256 {} \;
} > "$stage/BUILD-MANIFEST.txt"
# Helper script's absolute staging paths become obsolete after the final rename.
# Replace that build-only checksum list with one canonical package-relative list.
(
    cd "$stage"
    ./bin/frameharbor --help > CLI-HELP.txt
    /usr/bin/shasum -a 256 CLI-HELP.txt bin/frameharbor bin/helpers/tsk-helper bin/helpers/ffmpeg-helper bin/helpers/tsk-abi-probe \
        Cargo.toml Cargo.lock rust-toolchain.toml BUILD-MANIFEST.txt OPERATING.md LICENSE THIRD_PARTY_NOTICES.md > SHA256SUMS
    /usr/bin/find docs native licenses -type f ! -name .DS_Store -exec /usr/bin/shasum -a 256 {} \; >> SHA256SUMS
    /usr/bin/shasum -a 256 bin/helpers/tsk-helper bin/helpers/ffmpeg-helper > bin/helpers/SHA256SUMS
    /usr/bin/shasum -a 256 -c SHA256SUMS
)
# mkdir is the exclusive destination reservation. An existing directory or
# symlink is never entered; failed adoption preserves both owned locations.
if ! mkdir "$output_dir"; then
    printf 'Destination reservation failed; preserved stage: %s\n' "$stage" >&2
    exit 1
fi
mv -n "$stage"/* "$output_dir/"
if ! rmdir "$stage"; then
    printf 'Package adoption incomplete; preserved destination and stage: %s %s\n' "$output_dir" "$stage" >&2
    exit 1
fi
(cd "$output_dir" && /usr/bin/shasum -a 256 -c SHA256SUMS)
printf 'Local package created: %s\n' "$output_dir"
