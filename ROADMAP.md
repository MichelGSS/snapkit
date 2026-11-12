# SnapKit roadmap

SnapKit is a screen capture and annotation tool for Windows, written in Rust with egui.

## Project principles

- **100% local.** No accounts, servers, telemetry or network access.
- **Original work.** No code, icons, texts or layouts copied from other products.
- **Lightweight.** One native process, no background services.
- **Windows first.** Other platforms may come later.

## Done

| Area | Delivered |
|---|---|
| Capture | Region, window, full screen, all screens, delay, mouse pointer |
| Scrolling | Automatic scrolling (vertical, horizontal, both) and panoramic capture |
| Step by step | Click-driven guide capture with editable captions and markers |
| Editor | 19 tools, editable objects, styles, image menu, zoom, undo and redo |
| Quick annotation | In-place annotation, move and resize with live preview |
| OCR | Tesseract (English and Portuguese) and a neural engine for handwriting |
| Video | MP4 with system sound and microphone, pause, trimming, GIF export |
| Library | Automatic saving, thumbnails, search, Recycle Bin |
| Interface | English and Portuguese, nine themes, tray, start with Windows |
| Distribution | Installer, portable zip, MIT license, third-party notices, CI |

## Next

- Signed installer and release builds from CI.
- Update notifications that don't require a network connection from SnapKit itself (for example, a link to the Releases page).
- Global shortcuts for more capture modes.
- Keeping annotations editable when a capture is reopened from the library.
- Rotating and reordering annotations.
- Better support for setups that mix monitors with different scaling.

## Known limitations

See [System requirements and limits](docs/reference/requirements-and-limits.md#known-limitations).

Have an idea? [Open a feature request](https://github.com/MichelGSS/snapkit/issues/new/choose).
