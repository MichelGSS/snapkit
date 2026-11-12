# Command-line options

`snapkit.exe` accepts a few options. They're useful for shortcuts, scripts and testing.

| Option | What it does |
|---|---|
| *(none)* | Opens the SnapKit window. |
| `--minimized` | Starts hidden in the system tray. **Start with Windows** uses this option. |
| `--select` | Opens straight into a region capture. |
| `--test-capture <file.png>` | Captures the monitor under the pointer, including the pointer, saves it to `<file.png>` and exits without opening a window. The exit code is `0` on success and `1` on error. |

## Examples

Start a region capture from a desktop shortcut or a launcher:

```powershell
& "$env:LOCALAPPDATA\Programs\SnapKit\snapkit.exe" --select
```

Check that screen capture works on a machine, for example after a driver update:

```powershell
& "$env:LOCALAPPDATA\Programs\SnapKit\snapkit.exe" --test-capture "$env:TEMP\snapkit-test.png"
```

## Environment variables

| Variable | Effect |
|---|---|
| `SNAPKIT_DEBUG_VISIBLE` | When set, SnapKit's own windows aren't excluded from screen captures. Used to photograph the capture overlay while developing. |
| `CI` | When set, tests that need the real screen, OCR engines or FFmpeg are skipped. Only affects `cargo test`. |

## Related

- [Build SnapKit from source](../how-to/build-from-source.md)
