#!/usr/bin/env bash
# Verifies the macOS release artifacts produced by scripts/package-macos.sh in dist/:
#   dist/SnapKit.app
#   dist/SnapKit-<version>-macos.dmg
#   dist/SnapKit-<version>-macos.zip
#   dist/SHA256SUMS-macos.txt
#
# Runs on macOS. Usage: bash scripts/verify-macos-package.sh

set -euo pipefail

die() {
    echo "error: $*" >&2
    exit 1
}

[ "$(uname -s)" = "Darwin" ] || die "this script verifies the macOS package and must run on macOS"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

for tool in lipo otool plutil codesign xattr hdiutil ditto shasum file awk; do
    command -v "$tool" >/dev/null 2>&1 || die "'$tool' not found"
done

version="$(awk -F'"' '/^version = "/ { print $2; exit }' Cargo.toml)"
[ -n "$version" ] || die "could not read version from Cargo.toml"

dist="$root/dist"
app="$dist/SnapKit.app"
dmg="$dist/SnapKit-$version-macos.dmg"
zip="$dist/SnapKit-$version-macos.zip"
sums="$dist/SHA256SUMS-macos.txt"

[ -d "$app" ] || die "missing $app"
[ -f "$dmg" ] || die "missing $dmg"
[ -f "$zip" ] || die "missing $zip"
[ -f "$sums" ] || die "missing $sums"

work="$(mktemp -d "${TMPDIR:-/tmp}/snapkit-verify.XXXXXX")"
mount_point="$work/dmg-mount"
cleanup() {
    if [ -d "$mount_point" ] && mount | grep -q "on $mount_point "; then
        hdiutil detach "$mount_point" -quiet || hdiutil detach "$mount_point" -force -quiet || true
    fi
    rm -rf "$work"
}
trap cleanup EXIT

is_macho() {
    case "$(file -bL "$1")" in
        *Mach-O*) return 0 ;;
        *) return 1 ;;
    esac
}

echo "==> Checking Mach-O architectures and dynamic library links"
macho_count=0
while IFS= read -r -d '' f; do
    if is_macho "$f"; then
        macho_count=$((macho_count + 1))
        lipo "$f" -verify_arch arm64 x86_64 || die "$f is not universal (arm64 + x86_64): $(lipo -archs "$f" 2>/dev/null || true)"
        deps="$(otool -L "$f" | grep -E '^[[:space:]]' || true)"
        for forbidden in /opt/homebrew /usr/local /Users/; do
            if printf '%s\n' "$deps" | grep -q "$forbidden"; then
                die "$f links against forbidden path ($forbidden): $deps"
            fi
        done
    fi
done < <(find "$app" -type f -print0)
[ "$macho_count" -gt 0 ] || die "no Mach-O binaries found inside $app"

echo "==> Checking Info.plist, icon, licenses, and Contents/MacOS"
plist="$app/Contents/Info.plist"
plutil -lint "$plist" >/dev/null
plist_ver="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$plist")"
[ "$plist_ver" = "$version" ] || die "Info.plist version ($plist_ver) does not match Cargo.toml ($version)"

[ -f "$app/Contents/Resources/SnapKit.icns" ] || die "missing SnapKit.icns"
[ -f "$app/Contents/Resources/LICENSE.txt" ] || die "missing LICENSE.txt"
[ -f "$app/Contents/Resources/THIRD-PARTY-NOTICES.md" ] || die "missing THIRD-PARTY-NOTICES.md"
[ -d "$app/Contents/Resources/licenses" ] || die "missing licenses directory"

symlinks_in_macos="$(find "$app/Contents/MacOS" -type l)"
[ -z "$symlinks_in_macos" ] || die "unexpected symlink(s) in Contents/MacOS: $symlinks_in_macos"

echo "==> Checking code signature and extended attributes"
codesign --verify --deep --strict --verbose=4 "$app"
attrs="$(xattr -lr "$app")"
[ -z "$attrs" ] || die "unexpected extended attributes on $app: $attrs"

echo "==> Verifying DMG ($dmg)"
mkdir -p "$mount_point"
hdiutil attach "$dmg" -mountpoint "$mount_point" -nobrowse -readonly -quiet
[ -L "$mount_point/Applications" ] || die "missing /Applications symlink in DMG"
[ "$(readlink "$mount_point/Applications")" = "/Applications" ] || die "DMG Applications symlink target is not /Applications"
[ -d "$mount_point/SnapKit.app" ] || die "missing SnapKit.app in DMG"
codesign --verify --deep --strict --verbose=2 "$mount_point/SnapKit.app"
hdiutil detach "$mount_point" -quiet

echo "==> Verifying ZIP ($zip)"
zip_extract="$work/zip"
mkdir -p "$zip_extract"
ditto -xk "$zip" "$zip_extract"
[ -d "$zip_extract/SnapKit.app" ] || die "missing SnapKit.app in ZIP"
codesign --verify --deep --strict --verbose=2 "$zip_extract/SnapKit.app"

echo "==> Verifying bundled FFmpeg"
ffmpeg_bin="$app/Contents/Resources/ffmpeg/ffmpeg"
[ -x "$ffmpeg_bin" ] || die "missing executable $ffmpeg_bin"
encoders="$("$ffmpeg_bin" -hide_banner -encoders 2>/dev/null || true)"
for enc in h264_videotoolbox aac; do
    printf '%s\n' "$encoders" | grep -q "$enc" || die "FFmpeg missing $enc encoder"
done
devices="$("$ffmpeg_bin" -hide_banner -devices 2>/dev/null || true)"
printf '%s\n' "$devices" | grep -q avfoundation || die "FFmpeg missing avfoundation device"

"$ffmpeg_bin" -hide_banner -loglevel error -f lavfi -i color=c=black:s=160x90:r=30:d=1 \
    -c:v h264_videotoolbox -allow_sw 1 -f mp4 -movflags +faststart -y "$work/test.mp4"
[ -s "$work/test.mp4" ] || die "FFmpeg synthetic h264_videotoolbox encode produced empty output"

echo "==> Verifying snapkit --self-check and --quit"
snapkit_bin="$app/Contents/MacOS/snapkit"
check_out="$("$snapkit_bin" --self-check)"
printf '%s\n' "$check_out"
printf '%s\n' "$check_out" | grep -q "ffmpeg:.*SnapKit.app/Contents/Resources/ffmpeg/ffmpeg" \
    || die "--self-check did not resolve ffmpeg inside SnapKit.app/Contents/Resources/ffmpeg/ffmpeg"
"$snapkit_bin" --quit || true

if arch -x86_64 /usr/bin/true 2>/dev/null; then
    echo "==> Verifying x86_64 slice under Rosetta (--self-check)"
    x86_out="$(arch -x86_64 "$snapkit_bin" --self-check)"
    printf '%s\n' "$x86_out" | grep -q "ffmpeg:.*SnapKit.app/Contents/Resources/ffmpeg/ffmpeg" \
        || die "x86_64 --self-check did not resolve ffmpeg inside SnapKit.app/Contents/Resources/ffmpeg/ffmpeg"
fi

echo "==> Verifying SHA256SUMS-macos.txt"
(
    cd "$dist"
    shasum -a 256 -c "$(basename "$sums")"
)

echo "All macOS package verification checks passed."
