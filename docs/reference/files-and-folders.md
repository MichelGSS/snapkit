# Files and folders

SnapKit keeps everything on your computer, in ordinary files and folders you can open, back up or delete.

| What | Windows | macOS | Linux |
|---|---|---|---|
| Preferences & error log | `%APPDATA%\SnapKit` | `~/Library/Application Support/SnapKit` | `~/.config/SnapKit` |
| Capture library (`index.json` + `.miniaturas`) | `Pictures\SnapKit` | `~/Pictures/SnapKit` | `~/Pictures/SnapKit` |
| Recordings | `Videos\SnapKit` | `~/Movies/SnapKit` | `~/Videos/SnapKit` |
| Program install | `%LOCALAPPDATA%\Programs\SnapKit` | `/Applications/SnapKit.app` | `/usr/bin/snapkit` (`.deb`) |

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
