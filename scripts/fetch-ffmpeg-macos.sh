#!/bin/sh
# Builds the vendor/ffmpeg folder used by SnapKit's video recording on macOS.
#
# Downloads the latest static macOS release build of FFmpeg published at
# https://ffmpeg.martin-riedl.de (Apple silicon by default), checks it against the SHA-256
# published next to it, and copies only the `ffmpeg` binary. Nothing is installed on the system.
#
# The build includes h264_videotoolbox (the hardware encoder SnapKit records with), libopenh264
# (used when trimming and editing, as on Windows) and libx264.
#
# License: unlike the LGPL build used on Windows, these builds are configured with
# --enable-gpl --enable-version3, so the binary is GPLv3. SnapKit only runs it as a separate
# program. Whoever distributes it must include the license (copied here as LICENSE.txt) and offer
# the matching source; the versions and configure line are listed on the build page
# (SOURCE.txt records which build was taken).
#
# Requirements: curl, unzip and shasum (all part of macOS).
# Usage:  sh scripts/fetch-ffmpeg-macos.sh
#         FFMPEG_ARCH=amd64 sh scripts/fetch-ffmpeg-macos.sh       (Intel Macs)
#         FFMPEG_SHA256=<hex> sh scripts/fetch-ffmpeg-macos.sh     (pin a known build)

set -eu

arch="${FFMPEG_ARCH:-arm64}"
case "$arch" in
    arm64 | amd64) ;;
    *) echo "FFMPEG_ARCH must be arm64 or amd64" >&2; exit 1 ;;
esac

root="$(cd "$(dirname "$0")/.." && pwd)"
dest="$root/vendor/ffmpeg"
latest="https://ffmpeg.martin-riedl.de/redirect/latest/macos/$arch/release/ffmpeg.zip"
license="https://www.gnu.org/licenses/gpl-3.0.txt"

# HTTPS only, redirects included.
fetch() {
    curl --proto '=https' --proto-redir '=https' --tlsv1.2 -fsSL "$@"
}

work="$(mktemp -d "${TMPDIR:-/tmp}/snapkit-ffmpeg.XXXXXX")"
trap 'rm -rf "$work"' EXIT

# The redirect leads to the current build's folder; its checksum sits next to the zip.
url="$(fetch -o "$work/ffmpeg.zip" -w '%{url_effective}' "$latest")"
if [ -n "${FFMPEG_SHA256:-}" ]; then
    expected="$FFMPEG_SHA256"
else
    expected="$(fetch "$url.sha256" | grep -oE '[0-9a-fA-F]{64}' | head -n 1)"
fi
if [ -z "$expected" ]; then
    echo "no SHA-256 found for $url" >&2
    exit 1
fi
actual="$(shasum -a 256 "$work/ffmpeg.zip" | awk '{print $1}')"
lower() { printf '%s' "$1" | tr 'A-F' 'a-f'; }
if [ "$(lower "$actual")" != "$(lower "$expected")" ]; then
    echo "checksum mismatch for $url" >&2
    echo "  expected $expected" >&2
    echo "  got      $actual" >&2
    exit 1
fi

unzip -q "$work/ffmpeg.zip" -d "$work/x"
exe="$(find "$work/x" -type f -name ffmpeg | head -n 1)"
if [ -z "$exe" ]; then
    echo "ffmpeg not found in the package" >&2
    exit 1
fi

mkdir -p "$dest"
cp "$exe" "$dest/ffmpeg"
chmod 755 "$dest/ffmpeg"
fetch -o "$dest/LICENSE.txt" "$license" || echo "warning: could not download $license" >&2
printf 'FFmpeg static build for macOS (%s)\nFrom:    %s\nSHA-256: %s (zip)\nLicense: GPLv3 (LICENSE.txt)\n' \
    "$arch" "$url" "$actual" > "$dest/SOURCE.txt"

if [ "$(uname -s)" = Darwin ]; then
    "$dest/ffmpeg" -hide_banner -version | head -n 1
    if ! "$dest/ffmpeg" -hide_banner -encoders 2>/dev/null | grep -q h264_videotoolbox; then
        echo "warning: this FFmpeg has no h264_videotoolbox; recording will use a software encoder" >&2
    fi
fi
echo "$dest: $(du -h "$dest/ffmpeg" | awk '{print $1}')"
