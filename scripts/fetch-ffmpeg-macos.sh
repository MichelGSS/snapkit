#!/bin/sh
# Builds the vendor/ffmpeg folder used by SnapKit's video recording on macOS.
#
# Downloads the latest static macOS release build of FFmpeg published at
# https://ffmpeg.martin-riedl.de (Apple silicon by default, or universal arm64+x86_64),
# checks it against the SHA-256 published next to it, and copies only the `ffmpeg` binary.
# Nothing is installed on the system.
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
# Requirements: curl, unzip and shasum (all part of macOS; lipo for FFMPEG_ARCH=universal).
# Usage:  sh scripts/fetch-ffmpeg-macos.sh
#         FFMPEG_ARCH=universal sh scripts/fetch-ffmpeg-macos.sh   (arm64 + x86_64)
#         FFMPEG_ARCH=amd64 sh scripts/fetch-ffmpeg-macos.sh       (Intel Macs)
#         FFMPEG_SHA256=<hex> sh scripts/fetch-ffmpeg-macos.sh     (pin a single-arch build)

set -eu

arch="${FFMPEG_ARCH:-arm64}"
case "$arch" in
    arm64 | amd64 | universal) ;;
    *) echo "FFMPEG_ARCH must be arm64, amd64, or universal" >&2; exit 1 ;;
esac

if [ "$arch" = "universal" ] && ! command -v lipo >/dev/null 2>&1; then
    echo "lipo is required for FFMPEG_ARCH=universal" >&2
    exit 1
fi

root="$(cd "$(dirname "$0")/.." && pwd)"
dest="$root/vendor/ffmpeg"
license="https://www.gnu.org/licenses/gpl-3.0.txt"

# HTTPS only, redirects included.
fetch() {
    curl --proto '=https' --proto-redir '=https' --tlsv1.2 -fsSL "$@"
}

lower() {
    printf '%s' "$1" | tr 'A-F' 'a-f'
}

work="$(mktemp -d "${TMPDIR:-/tmp}/snapkit-ffmpeg.XXXXXX")"
trap 'rm -rf "$work"' EXIT

fetch_arch() {
    a="$1"
    out_bin="$2"
    pin="${3:-}"
    latest="https://ffmpeg.martin-riedl.de/redirect/latest/macos/$a/release/ffmpeg.zip"
    zip="$work/ffmpeg-$a.zip"
    xdir="$work/x-$a"

    # The redirect leads to the current build's folder; its checksum sits next to the zip.
    url="$(fetch -o "$zip" -w '%{url_effective}' "$latest")"
    if [ -n "$pin" ]; then
        expected="$pin"
    else
        expected="$(fetch "$url.sha256" | grep -oE '[0-9a-fA-F]{64}' | head -n 1)"
    fi
    if [ -z "$expected" ]; then
        echo "no SHA-256 found for $url" >&2
        exit 1
    fi
    actual="$(shasum -a 256 "$zip" | awk '{print $1}')"
    if [ "$(lower "$actual")" != "$(lower "$expected")" ]; then
        echo "checksum mismatch for $url" >&2
        echo "  expected $expected" >&2
        echo "  got      $actual" >&2
        exit 1
    fi

    unzip -q "$zip" -d "$xdir"
    exe="$(find "$xdir" -type f -name ffmpeg | head -n 1)"
    if [ -z "$exe" ]; then
        echo "ffmpeg not found in the $a package" >&2
        exit 1
    fi
    cp "$exe" "$out_bin"
    chmod 755 "$out_bin"
    printf 'From (%s):    %s\nSHA-256 (%s): %s (zip)\n' "$a" "$url" "$a" "$actual" >> "$work/source.tmp"
}

mkdir -p "$dest"
: > "$work/source.tmp"

if [ "$arch" = "universal" ]; then
    fetch_arch arm64 "$work/ffmpeg-arm64" "${FFMPEG_SHA256_ARM64:-}"
    fetch_arch amd64 "$work/ffmpeg-amd64" "${FFMPEG_SHA256_AMD64:-}"
    lipo -create -output "$dest/ffmpeg" "$work/ffmpeg-arm64" "$work/ffmpeg-amd64"
    lipo "$dest/ffmpeg" -verify_arch arm64 x86_64
    chmod 755 "$dest/ffmpeg"
else
    fetch_arch "$arch" "$dest/ffmpeg" "${FFMPEG_SHA256:-}"
fi

fetch -o "$dest/LICENSE.txt" "$license" || echo "warning: could not download $license" >&2
{
    printf 'FFmpeg static build for macOS (%s)\n' "$arch"
    cat "$work/source.tmp"
    printf 'License: GPLv3 (LICENSE.txt)\n'
} > "$dest/SOURCE.txt"

if [ "$(uname -s)" = Darwin ]; then
    "$dest/ffmpeg" -hide_banner -version | head -n 1
    encoders="$("$dest/ffmpeg" -hide_banner -encoders 2>/dev/null || true)"
    for enc in h264_videotoolbox aac; do
        if ! printf '%s\n' "$encoders" | grep -q "$enc"; then
            echo "warning: this FFmpeg has no $enc encoder" >&2
        fi
    done
    devices="$("$dest/ffmpeg" -hide_banner -devices 2>/dev/null || true)"
    if ! printf '%s\n' "$devices" | grep -q avfoundation; then
        echo "warning: this FFmpeg has no avfoundation device support" >&2
    fi
fi
echo "$dest: $(du -h "$dest/ffmpeg" | awk '{print $1}')"
