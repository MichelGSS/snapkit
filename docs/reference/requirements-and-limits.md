# System requirements and limits

## Requirements

| Item | Requirement |
|---|---|
| Operating system | Windows 10 version 2004 (build 19041) or later, or Windows 11; or macOS 12 Monterey or later |
| Architecture | Windows: 64-bit (x64). macOS: Apple silicon or Intel (one universal app). |
| Processor | Any 64-bit processor. No AVX2 needed. |
| Graphics | OpenGL support (any current graphics driver; built into macOS). Needed for the transparent capture overlay. |
| Disk space | About 200 MB for the program on Windows, plus your captures and recordings |
| Permissions | A standard user account. No administrator rights needed. On macOS, **Screen Recording** for every capture, **Accessibility** for scrolling capture, **Input Monitoring** for some keys during step and scrolling capture, and **Microphone** to record your voice. |
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

### On macOS

- The app is signed ad hoc, not with an Apple Developer ID, so the first launch needs **System Settings** > **Privacy & Security** > **Open Anyway**. After an update, macOS can forget the Screen Recording and Accessibility permissions: remove SnapKit from those lists and add it again.
- SnapKit doesn't update itself: **Preferences** > **Updates** opens the release page, and you replace the app in **Applications**.
- The selection layer covers one monitor at a time, the one under the pointer when the capture starts.
- Video recording isn't in the macOS download yet: it needs FFmpeg, which the app doesn't include (`scripts/fetch-ffmpeg-macos.sh` fetches it when you build the app yourself).
- Text recognition uses Apple's built-in Vision engine.
- Recordings include system sound only on macOS 14.6 or later; on older versions, only the microphone.
- Enter in step capture, and Esc during a scrolling capture, need the **Input Monitoring** permission.

## Related

- [Install, update or uninstall SnapKit](../how-to/install.md)
- [Troubleshoot common problems](../how-to/troubleshoot.md)
