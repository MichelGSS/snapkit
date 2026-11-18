# SnapKit roadmap

SnapKit is a screen capture and annotation tool for Windows, written in Rust with egui.

## Project principles

- **Local by default.** No accounts, servers or telemetry; the internet is used only when the user checks for updates.
- **Original work.** No code, icons, texts or layouts copied from other products.
- **Lightweight.** One native process, no background services.
- **Windows first.** Other platforms may come later.

## Done

| Area | Delivered |
|---|---|
| Capture | Region, window, full screen, all screens, delay, mouse pointer, global shortcuts for capture modes |
| Scrolling | Automatic scrolling (vertical, horizontal, both) and panoramic capture |
| Step by step | Click-driven guide capture with editable captions and markers |
| Editor | 19 tools, editable objects, styles, image menu, zoom, undo and redo |
| Quick annotation | In-place annotation, move and resize with live preview |
| OCR | Tesseract (English and Portuguese) and a neural engine for handwriting |
| Video | MP4 with system sound and microphone, configurable FPS (15–60), quality presets, cursor toggle, countdown, pause, integrated visual filmstrip editor (trim, area crop, speed 0.5×–2×, mute, resolution scaling, frame-to-editor capture), GIF export |
| Library | Unified image, video (MP4) and GIF library with automatic saving, thumbnails, media filters, duration badges, search, Recycle Bin, editable annotation layers |
| Interface | English and Portuguese, nine themes, quick-start workspace cards, tray, start with Windows |
| Distribution | Installer, portable zip, opt-in SHA-256 verified updates, MIT license, third-party notices, CI |

## Next

- Signed installer and release builds from CI.
- Rotating and reordering annotations.
- Better support for setups that mix monitors with different scaling.

## Known limitations

See [System requirements and limits](docs/reference/requirements-and-limits.md#known-limitations).

Have an idea? [Open a feature request](https://github.com/MichelGSS/snapkit/issues/new/choose).
