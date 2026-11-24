#!/usr/bin/env bash
# Builds the SnapKit Linux distribution packages in dist/:
#   SnapKit-<version>-linux-x86_64.tar.gz   (portable archive)
#   SnapKit-<version>-linux-amd64.deb       (Debian / Ubuntu package)
#   SHA256SUMS-linux.txt                    (SHA-256 of the .tar.gz and .deb)
#
# The helper folders are optional. Each one is copied when it exists in vendor/ (or in the folder
# set in SNAPKIT_VENDOR_DIR) with a Linux x86_64 build inside; without it the matching feature
# reports that it's missing:
#   vendor/ffmpeg/ffmpeg                      video recording
#   vendor/tesseract/tesseract + tessdata/    text recognition (Tesseract engine)
#   vendor/onnxruntime/libonnxruntime.so      text recognition (neural engine)
#   vendor/ocr-neural/det.onnx + rec.onnx     neural text recognition models
#
# Usage:  bash scripts/package-linux.sh

set -euo pipefail

die() {
    echo "error: $*" >&2
    exit 1
}

[ "$(uname -s)" = "Linux" ] || die "this script builds the Linux packages and must run on Linux"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

for tool in cargo python3 dpkg-deb tar sha256sum file awk; do
    command -v "$tool" >/dev/null 2>&1 || die "'$tool' not found"
done

version="$(awk -F'"' '/^version = "/ { print $2; exit }' Cargo.toml)"
[ -n "$version" ] || die "could not read the version from Cargo.toml"
echo "Version $version"

app_id="io.github.michelgss.snapkit"
vendor="${SNAPKIT_VENDOR_DIR:-$root/vendor}"
dist="$root/dist"
tarball="$dist/SnapKit-$version-linux-x86_64.tar.gz"
deb="$dist/SnapKit-$version-linux-amd64.deb"
sums="$dist/SHA256SUMS-linux.txt"

work="$(mktemp -d "${TMPDIR:-/tmp}/snapkit-package.XXXXXX")"
cleanup() {
    rm -rf "$work"
}
trap cleanup EXIT

# --- Build ----------------------------------------------------------------------------------

echo "Building release binary"
cargo build --release --locked
bin="$root/target/release/snapkit"
[ -f "$bin" ] || die "the release build didn't produce $bin"

# --- Icons & desktop entry ------------------------------------------------------------------

icons_dir="$work/icons"
frames="$work/frames"
mkdir -p "$icons_dir" "$frames"

python3 - "$root/assets/snapkit.ico" "$frames" <<'PY'
import struct
import sys

ico_path, out_dir = sys.argv[1], sys.argv[2]
with open(ico_path, "rb") as f:
    data = f.read()

_, kind, count = struct.unpack("<HHH", data[:6])
if kind != 1:
    raise SystemExit("unexpected .ico type")

for i in range(count):
    w, h, _, _, _, _, size, offset = struct.unpack("<BBBBHHII", data[6 + 16 * i : 22 + 16 * i])
    png = data[offset : offset + size]
    if png.startswith(b"\x89PNG\r\n\x1a\n"):
        dim = w or 256
        with open(f"{out_dir}/{dim}.png", "wb") as out:
            out.write(png)

# Generate any missing standard sizes from 256.png if Pillow is available.
try:
    from PIL import Image
    import os

    src_path = f"{out_dir}/256.png"
    if os.path.isfile(src_path):
        src = Image.open(src_path)
        for sz in (16, 24, 32, 48, 64, 128, 256):
            dst = f"{out_dir}/{sz}.png"
            if not os.path.isfile(dst):
                src.resize((sz, sz), Image.Resampling.LANCZOS).save(dst, "PNG")
except ImportError:
    pass
PY

for sz in 16 24 32 48 64 128 256; do
    if [ -f "$frames/$sz.png" ]; then
        mkdir -p "$icons_dir/hicolor/${sz}x${sz}/apps"
        cp "$frames/$sz.png" "$icons_dir/hicolor/${sz}x${sz}/apps/$app_id.png"
    elif command -v convert >/dev/null 2>&1 && [ -f "$frames/256.png" ]; then
        mkdir -p "$icons_dir/hicolor/${sz}x${sz}/apps"
        convert "$frames/256.png" -resize "${sz}x${sz}" "$icons_dir/hicolor/${sz}x${sz}/apps/$app_id.png"
    fi
done

mkdir -p "$icons_dir/hicolor/scalable/apps"
cp "$root/assets/logo.svg" "$icons_dir/hicolor/scalable/apps/$app_id.svg"

desktop_file="$work/$app_id.desktop"
cat >"$desktop_file" <<DESKTOP
[Desktop Entry]
Type=Application
Name=SnapKit
Comment=Free, local and lightweight screen capture tool
Exec=/usr/bin/snapkit
Icon=$app_id
Terminal=false
Categories=Graphics;Utility;
StartupWMClass=$app_id
Actions=Region;

[Desktop Action Region]
Name=Capture Region
Exec=/usr/bin/snapkit --select
DESKTOP

if command -v desktop-file-validate >/dev/null 2>&1; then
    desktop-file-validate "$desktop_file"
fi

# --- Helpers --------------------------------------------------------------------------------

is_elf() {
    case "$(file -bL "$1")" in
        *ELF*) return 0 ;;
        *) return 1 ;;
    esac
}

add_helper() {
    local dst_dir="$1" name="$2" check="$3"
    local src="$vendor/$name"
    if [ ! -d "$src" ]; then
        echo "Skipping $name (no $src)"
        return 0
    fi
    if [ -n "$check" ] && ! { [ -f "$src/$check" ] && is_elf "$src/$check"; }; then
        echo "Skipping $name ($src/$check is missing or isn't an ELF binary)"
        return 0
    fi
    echo "Adding $name"
    mkdir -p "$dst_dir/$name"
    cp -a "$src/." "$dst_dir/$name/"
}

add_all_helpers() {
    local dst_dir="$1"
    add_helper "$dst_dir" ffmpeg ffmpeg
    add_helper "$dst_dir" onnxruntime libonnxruntime.so
    add_helper "$dst_dir" ocr-neural ""
    if [ -d "$vendor/tesseract" ] && [ ! -d "$vendor/tesseract/tessdata" ]; then
        echo "Skipping tesseract (no $vendor/tesseract/tessdata)"
    else
        add_helper "$dst_dir" tesseract tesseract
    fi
}

# --- Portable tarball -----------------------------------------------------------------------

mkdir -p "$dist"
pkg_name="SnapKit-$version-linux-x86_64"
tar_root="$work/$pkg_name"
mkdir -p "$tar_root/share/applications" "$tar_root/share/icons"

cp "$bin" "$tar_root/snapkit"
chmod 755 "$tar_root/snapkit"
cp "$desktop_file" "$tar_root/share/applications/$app_id.desktop"
cp -a "$icons_dir/hicolor" "$tar_root/share/icons/"
cp -a "$root/licenses" "$tar_root/licenses"
cp "$root/LICENSE" "$tar_root/LICENSE.txt"
cp "$root/THIRD-PARTY-NOTICES.md" "$tar_root/THIRD-PARTY-NOTICES.md"
add_all_helpers "$tar_root"

rm -f "$tarball"
tar -C "$work" -czf "$tarball" "$pkg_name"

# --- Debian package -------------------------------------------------------------------------

deb_root="$work/deb"
mkdir -p "$deb_root/DEBIAN" \
    "$deb_root/usr/lib/snapkit" \
    "$deb_root/usr/bin" \
    "$deb_root/usr/share/applications" \
    "$deb_root/usr/share/icons" \
    "$deb_root/usr/share/doc/snapkit"

cp "$bin" "$deb_root/usr/lib/snapkit/snapkit"
chmod 755 "$deb_root/usr/lib/snapkit/snapkit"
ln -s "../lib/snapkit/snapkit" "$deb_root/usr/bin/snapkit"
add_all_helpers "$deb_root/usr/lib/snapkit"

cp "$desktop_file" "$deb_root/usr/share/applications/$app_id.desktop"
chmod 644 "$deb_root/usr/share/applications/$app_id.desktop"
cp -a "$icons_dir/hicolor" "$deb_root/usr/share/icons/"
cp -a "$root/licenses" "$deb_root/usr/share/doc/snapkit/licenses"
cp "$root/LICENSE" "$deb_root/usr/share/doc/snapkit/LICENSE.txt"
cp "$root/THIRD-PARTY-NOTICES.md" "$deb_root/usr/share/doc/snapkit/THIRD-PARTY-NOTICES.md"

installed_kb="$(du -sk "$deb_root" | awk '{print $1}')"
cat >"$deb_root/DEBIAN/control" <<CONTROL
Package: snapkit
Version: $version
Section: graphics
Priority: optional
Architecture: amd64
Installed-Size: $installed_kb
Maintainer: SnapKit contributors <https://github.com/MichelGSS/snapkit>
Homepage: https://github.com/MichelGSS/snapkit
Depends: libc6, libgtk-3-0t64 | libgtk-3-0, libglib2.0-0t64 | libglib2.0-0, libx11-6, libxcb1, libxfixes3, libgl1, libdbus-1-3, libayatana-appindicator3-1 | libappindicator3-1
Recommends: xclip | wl-clipboard, xdotool, xdg-utils
Description: Free, local and lightweight screen capture tool
 SnapKit is an open-source screen capture tool with annotations,
 scrolling capture, local OCR, and video recording.
CONTROL

rm -f "$deb"
dpkg-deb --root-owner-group --build "$deb_root" "$deb"

# --- Checksums & verification ---------------------------------------------------------------

(
    cd "$dist"
    sha256sum "$(basename "$tarball")" "$(basename "$deb")" >"$(basename "$sums")"
    sha256sum -c "$(basename "$sums")"
)

dpkg-deb --info "$deb"
dpkg-deb --contents "$deb"
"$bin" --self-check
