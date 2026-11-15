#!/bin/sh
# Build the selected local native backends. No media input is opened.
set -eu
project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
output_dir=${1:-"$project_dir/target/debug/helpers"}
ffmpeg_prefix=/opt/homebrew/opt/ffmpeg-full
mkdir -p "$output_dir"
/usr/bin/cc -std=c11 -Wall -Wextra -Werror -I/usr/local/include \
    "$project_dir/native/tsk_abi_probe.c" /usr/local/lib/libtsk.23.dylib \
    -o "$output_dir/tsk-abi-probe"
"$output_dir/tsk-abi-probe"
/usr/bin/cc -std=c11 -Wall -Wextra -Werror -I/usr/local/include \
    "$project_dir/native/tsk_helper.c" /usr/local/lib/libtsk.23.dylib \
    -o "$output_dir/tsk-helper"
/usr/bin/cc -std=c11 -Wall -Wextra -Werror -I"$ffmpeg_prefix/include" \
    "$project_dir/native/ffmpeg_helper.c" -L"$ffmpeg_prefix/lib" \
    -Wl,-rpath,"$ffmpeg_prefix/lib" -lavformat -lavcodec -lavutil \
    -o "$output_dir/ffmpeg-helper"
/usr/bin/shasum -a 256 "$output_dir/tsk-helper" "$output_dir/ffmpeg-helper" \
    > "$output_dir/SHA256SUMS"
printf 'Native helpers built in %s\n' "$output_dir"
