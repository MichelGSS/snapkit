#!/usr/bin/env bash
# Builds the SnapKit macOS distribution packages in dist/:
#   SnapKit.app                         (universal: Apple silicon and Intel)
#   SnapKit-<version>-macos.dmg         (drag SnapKit to Applications)
#   SnapKit-<version>-macos.zip         (the same app, zipped)
#   SHA256SUMS-macos.txt                (SHA-256 of the .dmg and the .zip)
#
# The app is signed ad hoc (no Apple Developer ID), so macOS asks the user to confirm the first
# launch (System Settings > Privacy & Security > Open Anyway).
#
# The helper folders are optional. Each one is copied when it exists in vendor/ (or in the folder
# set in SNAPKIT_VENDOR_DIR) with a macOS build inside; without it the matching feature reports
# that it's missing:
#   vendor/ffmpeg/ffmpeg                      video recording
#   vendor/tesseract/tesseract + tessdata/    text recognition (Tesseract engine)
#   vendor/onnxruntime/libonnxruntime.dylib   text recognition (neural engine)
#   vendor/ocr-neural/det.onnx + rec.onnx     neural text recognition models
#
# Runs on macOS 12 or later with the Xcode command line tools and rustup.
# Usage:  bash scripts/package-macos.sh

set -euo pipefail

die() {
    echo "error: $*" >&2
    exit 1
}

[ "$(uname -s)" = "Darwin" ] || die "this script builds the macOS app and must run on macOS"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

for tool in cargo lipo sips iconutil codesign hdiutil ditto plutil shasum file xattr; do
    command -v "$tool" >/dev/null 2>&1 || die "'$tool' not found (install the Xcode command line tools and rustup)"
done

version="$(awk -F'"' '/^version = "/ { print $2; exit }' Cargo.toml)"
[ -n "$version" ] || die "could not read the version from Cargo.toml"
echo "Version $version"

bundle_id="io.github.michelgss.snapkit"
min_macos="12.0"
targets="aarch64-apple-darwin x86_64-apple-darwin"
vendor="${SNAPKIT_VENDOR_DIR:-$root/vendor}"
dist="$root/dist"
app="$dist/SnapKit.app"
dmg="$dist/SnapKit-$version-macos.dmg"
zip="$dist/SnapKit-$version-macos.zip"
sums="$dist/SHA256SUMS-macos.txt"

work="$(mktemp -d "${TMPDIR:-/tmp}/snapkit-package.XXXXXX")"
cleanup() {
    rm -rf "$work"
}
trap cleanup EXIT

# --- Build ----------------------------------------------------------------------------------

# The deployment target keeps the binaries loadable on the oldest supported macOS.
export MACOSX_DEPLOYMENT_TARGET="$min_macos"

if command -v rustup >/dev/null 2>&1; then
    # shellcheck disable=SC2086 # the list of targets is split on purpose
    rustup target add $targets
fi

binaries=""
for target in $targets; do
    echo "Building for $target"
    cargo build --release --locked --target "$target"
    bin="$root/target/$target/release/snapkit"
    [ -f "$bin" ] || die "the build for $target didn't produce $bin"
    binaries="$binaries $bin"
done

# --- App bundle -----------------------------------------------------------------------------

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

# shellcheck disable=SC2086 # the list of binaries is split on purpose (paths have no spaces)
lipo -create -output "$app/Contents/MacOS/snapkit" $binaries
for arch in arm64 x86_64; do
    lipo "$app/Contents/MacOS/snapkit" -verify_arch "$arch" || die "the universal binary has no $arch slice"
done
chmod 755 "$app/Contents/MacOS/snapkit"

# The icon: the PNG images inside assets/snapkit.ico, resized with sips into an iconset.
iconset="$work/SnapKit.iconset"
frames="$work/frames"
mkdir -p "$iconset" "$frames"
if command -v python3 >/dev/null 2>&1; then
    # Writes each PNG image of the .ico as frames/<size>.png.
    python3 - "$root/assets/snapkit.ico" "$frames" <<'PY'
import struct
import sys

data = open(sys.argv[1], "rb").read()
_, kind, count = struct.unpack("<HHH", data[:6])
for i in range(count):
    w, h, _, _, _, _, size, offset = struct.unpack("<BBBBHHII", data[6 + 16 * i : 22 + 16 * i])
    png = data[offset : offset + size]
    if kind == 1 and png.startswith(b"\x89PNG\r\n\x1a\n"):
        with open(f"{sys.argv[2]}/{w or 256}.png", "wb") as out:
            out.write(png)
PY
fi
if [ ! -f "$frames/256.png" ]; then
    # Without python3 (or if the .ico changes format), sips reads the .ico itself.
    sips -s format png "$root/assets/snapkit.ico" --out "$frames/source.png" >/dev/null
    sips -z 256 256 "$frames/source.png" --out "$frames/256.png" >/dev/null
fi
# Each iconset entry comes from the image of the same size when the .ico has it, otherwise from
# the largest one (256 px) resized.
for entry in 16:icon_16x16 32:icon_16x16@2x 32:icon_32x32 64:icon_32x32@2x 128:icon_128x128 \
    256:icon_128x128@2x 256:icon_256x256 512:icon_256x256@2x 512:icon_512x512; do
    size="${entry%%:*}"
    name="${entry#*:}"
    if [ -f "$frames/$size.png" ]; then
        cp "$frames/$size.png" "$iconset/$name.png"
    else
        sips -z "$size" "$size" "$frames/256.png" --out "$iconset/$name.png" >/dev/null
    fi
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/SnapKit.icns"

cat >"$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>en</string>
    <key>CFBundleDisplayName</key>
    <string>SnapKit</string>
    <key>CFBundleExecutable</key>
    <string>snapkit</string>
    <key>CFBundleIconFile</key>
    <string>SnapKit</string>
    <key>CFBundleIdentifier</key>
    <string>$bundle_id</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>SnapKit</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>$version</string>
    <key>CFBundleVersion</key>
    <string>$version</string>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.productivity</string>
    <key>LSMinimumSystemVersion</key>
    <string>$min_macos</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSSupportsAutomaticGraphicsSwitching</key>
    <true/>
    <key>NSHumanReadableCopyright</key>
    <string>Copyright (c) 2026 SnapKit contributors. MIT License.</string>
    <key>NSScreenCaptureUsageDescription</key>
    <string>SnapKit needs to record the screen to take screenshots and record videos.</string>
    <key>NSMicrophoneUsageDescription</key>
    <string>SnapKit uses the microphone only when you turn it on for a video recording.</string>
    <key>NSAudioCaptureUsageDescription</key>
    <string>SnapKit records the computer's sound only when you turn it on for a video recording.</string>
</dict>
</plist>
PLIST
plutil -lint "$app/Contents/Info.plist" >/dev/null
printf 'APPL????' >"$app/Contents/PkgInfo"

# Licenses, as in the Windows package.
ditto "$root/licenses" "$app/Contents/Resources/licenses"
cp "$root/LICENSE" "$app/Contents/Resources/LICENSE.txt"
cp "$root/THIRD-PARTY-NOTICES.md" "$app/Contents/Resources/THIRD-PARTY-NOTICES.md"

# (No `file | grep -q`: with pipefail, grep closing the pipe early could make it fail.)
is_macho() {
    case "$(file -bL "$1")" in
        *Mach-O*) return 0 ;;
        *) return 1 ;;
    esac
}

# Warns when a helper binary lacks a slice: the app would lose that feature on that kind of Mac.
check_archs() {
    local archs
    archs="$(lipo -archs "$1" 2>/dev/null || true)"
    case " $archs " in
        *" arm64 "*) ;;
        *) echo "warning: $1 has no arm64 slice ($archs)" >&2 ;;
    esac
    case " $archs " in
        *" x86_64 "*) ;;
        *) echo "warning: $1 has no x86_64 slice ($archs)" >&2 ;;
    esac
}

# SnapKit looks for its helpers next to the executable (Contents/MacOS/<name>). The folders
# themselves go in Contents/Resources, with a link in Contents/MacOS: data files (models,
# tessdata) inside Contents/MacOS would be treated as code and break the signature.
add_helper() {
    local name="$1" check="$2"
    local src="$vendor/$name"
    if [ ! -d "$src" ]; then
        echo "Skipping $name (no $src)"
        return 0
    fi
    if [ -n "$check" ] && ! { [ -f "$src/$check" ] && is_macho "$src/$check"; }; then
        echo "Skipping $name ($src/$check is missing or isn't a macOS binary)"
        return 0
    fi
    echo "Adding $name"
    ditto "$src" "$app/Contents/Resources/$name"
    ln -s "../Resources/$name" "$app/Contents/MacOS/$name"
    if [ -n "$check" ]; then
        check_archs "$app/Contents/Resources/$name/$check"
    fi
}

add_helper ffmpeg ffmpeg
add_helper onnxruntime libonnxruntime.dylib
add_helper ocr-neural ""
if [ -d "$vendor/tesseract" ] && [ ! -d "$vendor/tesseract/tessdata" ]; then
    echo "Skipping tesseract (no $vendor/tesseract/tessdata)"
else
    add_helper tesseract tesseract
fi

# --- Signature ------------------------------------------------------------------------------

# Extended attributes (quarantine, Finder info) make codesign refuse the bundle.
xattr -cr "$app"

# Every helper binary and library needs its own signature: Apple silicon refuses to run
# unsigned code, and codesign --deep doesn't look inside Contents/Resources.
find "$app/Contents/Resources" -type f -print0 | while IFS= read -r -d '' f; do
    if is_macho "$f"; then
        codesign --force --sign - "$f"
    fi
done
codesign --force --deep --sign - "$app"
codesign --verify --deep --strict --verbose=2 "$app"

# --- Disk image, zip and checksums ----------------------------------------------------------

dmg_root="$work/dmg"
mkdir -p "$dmg_root"
ditto "$app" "$dmg_root/SnapKit.app"
ln -s /Applications "$dmg_root/Applications"
rm -f "$dmg"
# hdiutil sometimes fails with "Resource busy" on build machines; it's worth a few tries.
made=""
for attempt in 1 2 3 4 5; do
    if hdiutil create -volname "SnapKit $version" -srcfolder "$dmg_root" -fs HFS+ -format UDZO \
        -imagekey zlib-level=9 -ov "$dmg"; then
        made=1
        break
    fi
    echo "hdiutil failed (attempt $attempt), retrying" >&2
    sleep $((attempt * 5))
done
[ -n "$made" ] || die "could not create $dmg"
hdiutil verify "$dmg"

rm -f "$zip"
# ditto keeps the links and the signature intact, unlike a plain zip.
ditto -c -k --keepParent "$app" "$zip"

(
    cd "$dist"
    shasum -a 256 "$(basename "$dmg")" "$(basename "$zip")" >"$sums"
)

for f in "$dmg" "$zip"; do
    bytes="$(wc -c <"$f")"
    printf '%s  %s MB\n' "$(basename "$f")" "$((bytes / 1024 / 1024))"
done
cat "$sums"
