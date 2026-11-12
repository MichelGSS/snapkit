# SnapKit

**A powerful, free and lightweight screen capture tool for Windows that runs 100% on your computer.**

SnapKit captures, annotates, reads text from and records your screen, without an account, a cloud service or telemetry. It's made for people who work with documentation: technical writers, support teams, QA engineers, trainers and developers who need clear screenshots, step-by-step guides and short demos every day.

[![CI](https://github.com/MichelGSS/snapkit/actions/workflows/ci.yml/badge.svg)](https://github.com/MichelGSS/snapkit/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/MichelGSS/snapkit)](https://github.com/MichelGSS/snapkit/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
![Platform: Windows 10 and 11](https://img.shields.io/badge/platform-Windows%2010%20%7C%2011-0078d4)

**[Download SnapKit](https://github.com/MichelGSS/snapkit/releases/latest)** · **[User guide](docs/README.md)** · **[Report a problem](https://github.com/MichelGSS/snapkit/issues/new/choose)**

## Why SnapKit

- **100% local.** Capture, annotation, OCR and video encoding all run on your machine. SnapKit never opens a network connection, so it's safe for internal tools, customer data and air-gapped environments.
- **Free and open source.** MIT licensed, with no paid tier, no sign-up and no feature held back.
- **Lightweight.** A single native app written in Rust: a 66 MB installer, about 100 MB of memory while idle, no services, no background updater.
- **Built for documentation work.** Quick annotation, numbered steps, step-by-step guides, scrolling capture, OCR that keeps code indentation, and GIFs for pull requests.

## Features

### Capture

- **Region, window, full screen and all screens**, with a live overlay (nothing freezes), window detection and a pixel magnifier.
- **Scrolling capture** of long pages, vertically, horizontally or both, with fixed headers and footers detected automatically.
- **Panoramic capture**: scroll any area by hand, in any direction, and SnapKit stitches it.
- **Step-by-step capture**: every click and **Enter** becomes a numbered, captioned shot in one guide.
- **Delay** of 3, 5 or 10 seconds for menus and tooltips, and an option to include the mouse pointer.

### Annotate

- **Quick annotation** right on the screen: draw, press **Enter**, paste. The editor never opens.
- **19 tools**: arrow, line, rectangle, ellipse, pen, text, callout, numbered steps, stamps, highlighter, spotlight, magnifier, pixelate, blur, eraser, crop, cut out and grab text.
- **Every annotation stays editable**: select it later to move, resize, recolor, retype, duplicate or delete it.
- **Styles** to reuse your favorite tool settings with one click; rotate, flip, resize, border and drop shadow.

### Text, video and library

- **OCR** with Tesseract (English and Portuguese) that keeps indentation and blank lines, plus a neural engine for handwriting.
- **Screen recording** to MP4 with system sound and microphone, pause and resume, frame-accurate trimming and GIF export.
- **Library** of every capture, with thumbnails and search by window title or date. Captures are plain PNG files in your **Pictures** folder.
- **English and Portuguese** interface, 9 color themes, a configurable global shortcut, tray icon and start with Windows.

## Install

1. Download `SnapKit-<version>-setup.exe` from the [latest release](https://github.com/MichelGSS/snapkit/releases/latest).
2. Run it. It installs for your user only and doesn't need administrator rights.
3. Press **Alt+Shift+S** to take your first capture.

Prefer not to install anything? Download the portable `SnapKit-<version>-windows-x64.zip`, extract it and run `snapkit.exe`.

**Requirements:** Windows 10 version 2004 or later, or Windows 11, 64-bit. See [System requirements and limits](docs/reference/requirements-and-limits.md).

> [!NOTE]
> The installer isn't code-signed yet, so Windows SmartScreen may show **Windows protected your PC**. Select **More info** > **Run anyway**, or [build SnapKit from source](docs/how-to/build-from-source.md).

## Documentation

The [user guide](docs/README.md) is organized with the [Diátaxis](https://diataxis.fr) framework:

| Section | Start with |
|---|---|
| **Tutorials** | [Take, annotate and share your first screenshot](docs/tutorials/first-screenshot.md) |
| **How-to guides** | [Capture a long page with scrolling capture](docs/how-to/capture-scrolling.md) |
| **Reference** | [Keyboard shortcuts](docs/reference/keyboard-shortcuts.md) |
| **Explanation** | [Why SnapKit is 100% local](docs/explanation/local-first.md) |

## Build from source

You need [Rust](https://rustup.rs) (stable) and the Visual Studio Build Tools with the C++ workload.

```powershell
git clone https://github.com/MichelGSS/snapkit.git
cd snapkit
cargo run
```

OCR and video use third-party binaries that aren't stored in the repository; the scripts in `scripts\` download them. See [Build SnapKit from source](docs/how-to/build-from-source.md).

### Code map

| Path | Contents |
|---|---|
| `src/main.rs` | Main window, selection overlay, floating panels, preferences |
| `src/capture.rs` | Monitor capture and window list |
| `src/editor.rs`, `src/render.rs` | Annotation editor and image rendering |
| `src/scroll.rs` | Scrolling and panoramic capture: motion detection and stitching |
| `src/steps.rs` | Step-by-step capture |
| `src/ocr.rs`, `src/neural.rs` | Text recognition: Tesseract, neural engine and layout |
| `src/record.rs`, `src/audio.rs` | Video recording (FFmpeg) and audio capture |
| `src/library.rs` | Capture library on disk |
| `src/i18n.rs` | English and Portuguese interface texts |
| `src/win.rs` | Direct Windows API calls |
| `src/tray.rs`, `src/config.rs`, `src/theme.rs`, `src/autostart.rs` | Tray, preferences, themes, start with Windows |

## Contributing

Bug reports, ideas, documentation fixes and pull requests are welcome. Read [CONTRIBUTING.md](CONTRIBUTING.md) to get started, and see the [roadmap](ROADMAP.md) for what's planned.

## License

SnapKit is released under the [MIT License](LICENSE). The installer and the zip also include third-party programs and data under their own open-source licenses, listed in [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
