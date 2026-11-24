# System requirements and limits

## Requirements

| Item | Requirement |
|---|---|
| Operating system | **Windows** 10 version 2004 (build 19041) or later, or Windows 11 · **macOS** 12 Monterey or later · **Linux** x86_64 (X11 or Wayland desktop with GTK 3 & AppIndicator) |
| Architecture | **Windows & Linux:** 64-bit (`x86_64`) · **macOS:** Universal (`arm64` Apple Silicon & `x86_64` Intel) |
| Processor | Any 64-bit processor. No AVX2 needed. |
| Graphics | OpenGL support (Metal/OpenGL on macOS). Needed for the transparent capture overlay. |
| Disk space | About 120–200 MB depending on platform, plus your captures and recordings |
| Permissions | Standard user account (macOS requires **Screen Recording** and **Accessibility** permissions) |
| Network | None. SnapKit works fully offline. It connects to GitHub only to check for updates, when you ask it to. |

## Size and resource use

| Item | Typical value |
|---|---|
| Installer download | About 66 MB |
| Portable zip download | About 85 MB |
| `snapkit.exe` | About 25 MB |
| Memory while idle in the tray | About 100 MB |
| Background services | None. SnapKit runs only while its own process is open. |

The installed size breaks down as follows: FFmpeg about 110 MB, Tesseract about 28 MB, the neural OCR models about 13 MB, ONNX Runtime about 17 MB, and the program itself.

## Capture limits

| Feature | Limit |
|---|---|
| Region | At least 4 × 4 pixels |
| Scrolling capture | 80 shots or 32,000 pixels in the scrolling direction |
| Step-by-step capture | 40 steps per session |
| Image resize | Up to 20,000 pixels per side |
| Export formats | PNG, JPEG, WebP, BMP, and single- or multi-page PDF 1.4 |
| Pin to screen | Zoom from 25% to 400%, opacity from 20% to 100%, multiple simultaneous pins |
| Text recognition languages | English and Portuguese (printed text); Latin script handwriting with the neural engine |
| Video | MP4 (H.264 video at 15, 24, 30 or 60 FPS; AAC audio at 48 kHz stereo); speed adjustment from 0.5× to 2.0× |
| GIF | 8, 12, 15 or 24 frames per second; 320, 480, 720 or 960 pixels wide, or the original width |

## Known limitations

- Scrolling capture needs the content to stay still between shots; videos and animations on the page can stop it.
- When a capture spans monitors of different heights, the empty space comes out black.
- Video recording doesn't support a webcam overlay; trimming keeps one clip, from a start to an end time.
- In remote sessions or virtual machines without OpenGL, SnapKit may not open.
- Over Remote Desktop, the area around the selection can show black instead of the darkened screen. The capture itself isn't affected.

## Related

- [Install, update or uninstall SnapKit](../how-to/install.md)
- [Troubleshoot common problems](../how-to/troubleshoot.md)
