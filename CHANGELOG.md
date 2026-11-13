# Changelog

All notable changes to SnapKit are listed here. The project follows [Semantic Versioning](https://semver.org).

## [0.6.0] - 2026-11-13

### Added

- **Preferences** > **Updates**: the installed version, a **Check for updates** button and **Update now**, which downloads the new installer from GitHub, checks its SHA-256 checksum, installs it and opens SnapKit again. A portable copy opens the download page instead.
- **Check automatically (once a day)**, off by default. It only tells you about a new version, with a **⬆ Version** button in the top bar; nothing is installed without a click.

### Changed

- SnapKit can now go online, only to check for updates, and only when you ask it to. Nothing but its name and version is sent.

## [0.5.3] - 2026-11-13

### Fixed

- SnapKit didn't open on computers whose processor has no AVX2 (older processors, and many Pentium and Celeron models). The neural OCR runtime, ONNX Runtime, was built into `snapkit.exe` in a variant that needs AVX2. It's now Microsoft's official `onnxruntime.dll`, which works on any 64-bit processor and is loaded only when the neural engine is used: **Read as handwriting**, or **Read text** when Tesseract isn't available.
- SnapKit didn't open on a Windows without the Visual C++ runtime. The packages now include it.
- If SnapKit closed during a recording (**Quit** in the tray, or a crash), FFmpeg went on recording the screen in the background. It now ends together with SnapKit.
- Opening a very tall image, such as a long scrolling capture, closed SnapKit when the image was taller than the graphics card's texture limit (16,384 or 32,768 pixels, depending on the card). The editor now shows it reduced; copies and saved files keep the full size.
- Videos in Chrome, Edge and Firefox paused while a region was being selected: the browser took the selection layer for a window covering it. The layer no longer counts as one.
- Opening SnapKit while it was already running (from the Start menu while it was in the tray, for example) started a second copy, with a second tray icon, no shortcut, and a library index that each copy saved over the other's. The second launch now brings up the copy already running, or starts a capture there with `--select`.
- With **Start with Windows** (`--minimized`), the window opened instead of staying in the tray.
- A capture taken before the window was ever shown left the window the size of every monitor afterwards.
- A damaged or hand-edited `config.json` or `index.json` (cut short, or saved with a byte order mark) silently reset the preferences or emptied the library, and the next save made it permanent. Both files are now written in one step, a byte order mark is accepted, a file that can't be read is kept aside as `.bad`, and the library finds again the captures in its folder that its index lost.
- A failure inside a background job (text recognition, scrolling capture, GIF) could leave SnapKit busy until restarted: no more captures from the shortcut, or **Read text** disabled. It's now reported as an error.
- Captures that couldn't be saved to the library (a full disk, for example) failed without a word. The status bar now says so.
- If SnapKit can't start, it now shows a message with the reason and writes it to `%APPDATA%\SnapKit\error.log`, instead of closing without a word. An error while it's running gets its own message.

### Changed

- `snapkit.exe` went from 46 MB to 25 MB, and it no longer depends on DirectML.

## [0.5.2] - 2026-11-12

First public release.

### Capture

- Region, window, full screen and all-screens capture with a live, non-freezing overlay, window detection and a pixel magnifier.
- Adjustable region: move it, resize it with eight handles and confirm with **Enter**, **Ctrl+C**, a double-click or **Capture**.
- Automatic scrolling capture (vertical, horizontal or both) with fixed header and footer detection.
- Panoramic capture: scroll by hand in any direction and SnapKit stitches the result.
- Step-by-step capture: each click and **Enter** becomes a numbered, captioned shot in a single guide.
- Capture delay (3, 5 or 10 seconds) and an option to include the mouse pointer.

### Annotate

- Quick annotation right on the screen: annotate the capture in place and press **Enter** to copy it. The capture can be moved and resized while keeping its annotations.
- Editor with 19 tools: select, eraser, arrow, line, rectangle, ellipse, pen, text, callout, step, stamp, highlighter, spotlight, magnifier, pixelate, blur, crop, cut out and grab text.
- Every annotation stays editable until the image is copied or saved.
- Styles, plus rotate, flip, resize, border and drop shadow.

### Text, video and library

- Text recognition with Tesseract (English and Portuguese), layout-preserving output and a neural engine for handwriting.
- Screen recording to MP4 with system sound and microphone, pause and resume, frame-accurate trimming and GIF export.
- Capture library with thumbnails, search by window title or date, and deletion to the Recycle Bin.

### Interface

- English and Portuguese interface, following the Windows language by default.
- Nine color themes, a configurable global shortcut, tray icon and start with Windows.
- Windows no longer flash on the screen when a capture starts or ends.

[0.6.0]: https://github.com/MichelGSS/snapkit/releases/tag/v0.6.0
[0.5.3]: https://github.com/MichelGSS/snapkit/releases/tag/v0.5.3
[0.5.2]: https://github.com/MichelGSS/snapkit/releases/tag/v0.5.2
