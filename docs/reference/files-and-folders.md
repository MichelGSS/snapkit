# Files and folders

SnapKit keeps everything on your computer, in ordinary files and folders you can open, back up or delete.

| What | Location | Notes |
|---|---|---|
| Program (installer) | `%LOCALAPPDATA%\Programs\SnapKit` | `snapkit.exe` plus the `tesseract`, `ocr-neural`, `ffmpeg` and `licenses` folders |
| Program (zip) | The folder you extracted it to | Same contents as the installed program |
| Preferences and styles | `%APPDATA%\SnapKit\config.json` | See [Preferences](preferences.md#configjson) |
| Capture library | `Pictures\SnapKit` | One PNG per capture |
| Library index | `Pictures\SnapKit\index.json` | Titles, dates and sizes used by the search |
| Thumbnails | `Pictures\SnapKit\.miniaturas` | Rebuilt automatically if deleted |
| Recordings | `Videos\SnapKit` | MP4 videos, trimmed clips and GIFs |
| Start with Windows | Registry: `HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run`, value `SnapKit` | Created only when you turn on **Start with Windows** |

`Pictures` and `Videos` are your Windows user folders, usually `C:\Users\<your name>\Pictures` and `C:\Users\<your name>\Videos`.

## File names

| File | Name pattern | Example |
|---|---|---|
| Capture | `<year>-<month>-<day>_<hour>-<minute>-<second>.png` | `2026-11-10_14-05-32.png` |
| Second capture in the same second | Same, with a suffix | `2026-11-10_14-05-32_2.png` |
| Recording | `<year>-<month>-<day>_<hour>-<minute>-<second>.mp4` | `2026-11-10_14-10-03.mp4` |
| Trimmed clip | The video's name with `-clip` | `2026-11-10_14-10-03-clip.mp4` |
| GIF | The video's name, `.gif` | `2026-11-10_14-10-03.gif` |

While a recording is in progress, SnapKit also writes temporary files next to it (`.video.mp4`, `.audio.wav`, `.part1.mp4` and so on). They're removed when the recording is saved.

## Back up or move your library

Copy the whole `Pictures\SnapKit` folder, including the hidden `.miniaturas` folder and `index.json`. To restore it, copy it back while SnapKit is closed.

If you add or remove PNG files by hand, SnapKit drops index entries whose file is gone the next time it starts. Files that aren't in the index don't appear in the library.

## Remove everything

1. Uninstall SnapKit, or delete the zip folder.
2. Delete `%APPDATA%\SnapKit`, `Pictures\SnapKit` and `Videos\SnapKit`.

SnapKit doesn't store anything else, anywhere else.

## Related

- [Find, reopen and delete captures in the library](../how-to/library.md)
- [Why SnapKit is 100% local](../explanation/local-first.md)
