# System requirements and limits

## Requirements

| Item | Requirement |
|---|---|
| Operating system | Windows 10 version 2004 (build 19041) or later, or Windows 11 |
| Architecture | 64-bit (x64) |
| Graphics | OpenGL support (any current graphics driver). Needed for the transparent capture overlay. |
| Disk space | About 200 MB for the program, plus your captures and recordings |
| Permissions | A standard user account. No administrator rights needed. |
| Network | None. SnapKit never connects to the internet. |

## Size and resource use

| Item | Typical value |
|---|---|
| Installer download | About 66 MB |
| Portable zip download | About 85 MB |
| `snapkit.exe` | About 46 MB, including the neural OCR runtime |
| Memory while idle in the tray | About 100 MB |
| Background services | None. SnapKit runs only while its own process is open. |

The installed size breaks down as follows: FFmpeg about 110 MB, Tesseract about 28 MB, the neural OCR models about 13 MB, and the program itself.

## Capture limits

| Feature | Limit |
|---|---|
| Region | At least 4 × 4 pixels |
| Scrolling capture | 80 shots or 32,000 pixels in the scrolling direction |
| Step-by-step capture | 40 steps per session |
| Image resize | Up to 20,000 pixels per side |
| Text recognition languages | English and Portuguese (printed text); Latin script handwriting with the neural engine |
| Video | MP4 (H.264 video, AAC audio at 48 kHz stereo) |
| GIF | 8, 12, 15 or 24 frames per second; 320, 480, 720 or 960 pixels wide, or the original width |

## Known limitations

- There's only one global shortcut, for region capture.
- When you reopen a capture from the library, earlier annotations are part of the image and can't be selected.
- Annotations can't be rotated or reordered (brought to front or sent to back).
- Scrolling capture needs the content to stay still between shots; videos and animations on the page can stop it.
- When a capture spans monitors of different heights, the empty space comes out black.
- On setups that mix monitors with different scaling, a recording area can come out slightly shifted.
- Video recording doesn't support a webcam overlay; trimming keeps one clip, from a start to an end time.
- In remote sessions or virtual machines without OpenGL, SnapKit may not open.

## Related

- [Install, update or uninstall SnapKit](../how-to/install.md)
- [Troubleshoot common problems](../how-to/troubleshoot.md)
