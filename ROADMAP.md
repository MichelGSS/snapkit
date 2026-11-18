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
| Capture | Region, window, full screen, all screens, Per-Monitor V2 mixed-DPI accuracy, delay, mouse pointer, optional global shortcuts for every capture mode |
| Pin to screen | Floating always-on-top reference windows (`25%–400%` zoom, `20%–100%` opacity, context menu, multi-pin) |
| Scrolling | Automatic scrolling (vertical, horizontal, both) and panoramic capture |
| Step by step | Click-driven guide capture with editable captions and markers |
| Editor | 19 tools, editable objects, multi-selection (`Ctrl+Click`, `Ctrl+A`, marquee) with alignment & distribution, clipboard image sticker layers (`Ctrl+V`), rounded corners, annotation rotation (`45°` snap) and Z-order reordering, smart alignment guides, eyedropper (`I`), Beautifier background mockups, styles, image menu, zoom, undo and redo |
| Quick annotation | In-place annotation, Pin (`P`), eyedropper, move and resize with live preview |
| OCR & privacy | Tesseract (English and Portuguese), neural engine for handwriting, and one-click sensitive-data Auto-redact (emails, API keys, cards, CPF/CNPJ, phones, IPs) |
| Export | PNG, JPEG, WebP, BMP, single- and multi-page PDF 1.4, and vertical/horizontal image stitching |
| Video | MP4 with system sound and microphone, configurable FPS (15–60), quality presets, cursor toggle, animated click highlights, countdown, pause, integrated visual filmstrip editor (60 FPS live preview with synchronized audio & waveform overlay, trim, area crop, speed `0.5×–2×`, mute, resolution scaling, frame-to-editor capture), GIF export |
| Library | Unified image, video (MP4) and GIF library with persistent editable annotation layers, favorites (`★`), custom titles and searchable tags, thumbnails, media filters, duration badges, multi-selection (`Ctrl+Click`/`Shift+Click`), batch PDF/export/delete/copy, native drag-and-drop (`CF_HDROP`), Recycle Bin |
| Interface | English and Portuguese, nine themes, quick-start workspace cards, tray, start with Windows |
| Distribution | Installer, portable zip, opt-in SHA-256 verified updates, automated CI & tagged release builds (`SHA256SUMS.txt`), MIT license, third-party notices |

## Next

- Code-signed installer (Authenticode certificate) to avoid SmartScreen prompts.
- Optional webcam picture-in-picture overlay during screen recordings.

## Known limitations

See [System requirements and limits](docs/reference/requirements-and-limits.md#known-limitations).

Have an idea? [Open a feature request](https://github.com/MichelGSS/snapkit/issues/new/choose).
