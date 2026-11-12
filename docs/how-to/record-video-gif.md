# Record the screen as video or GIF

SnapKit records any area of the screen as an MP4 video with sound, or straight to an animated GIF.

| Mode | Output | Sound | Use it for |
|---|---|---|---|
| **Video** | MP4 (H.264 + AAC) | System sound and microphone | Demos with narration, bug reports, tutorials |
| **GIF** | Animated GIF | None | Short loops for pull requests, chats and docs |

## Record a video

1. Select **Video** in the SnapKit top bar.
2. Select a window, or drag over an area and press **Enter** to confirm it.

   A panel appears next to the area. An orange frame marks the recorded area; the frame and the panel don't appear in the video.

3. Select the sound sources: **System sound**, **Microphone**, or both.
4. Select **Record**.
5. Use **Pause** and **Resume** as needed. Paused time isn't included.
6. Select **Stop**.

The video is saved in `Videos\SnapKit`, and the **Recording saved** window opens. From there you can open the video, trim it or convert it to GIF. See [Trim a recording and convert it to GIF](trim-and-convert.md).

To discard the recording before it starts, select **Cancel**.

## Record a GIF

1. Select **GIF** in the top bar.
2. Select a window, or drag over an area and press **Enter**.
3. Select **Record GIF**.
4. Select **Stop** when you're done.

SnapKit converts the recording to a GIF and doesn't keep the intermediate video.

## Tips for clean recordings

- **Record a small area.** Smaller areas make smaller files and are easier to read in documentation.
- **Turn off notifications** with Do not disturb in Windows, so pop-ups don't end up in the video.
- **Check your microphone** in Windows sound settings before a long recording. If a sound source fails, SnapKit keeps recording without it and shows a warning.

## Related

- [Tutorial: Record a short demo and turn it into a GIF](../tutorials/demo-video-and-gif.md)
- [How video recording works](../explanation/video-recording.md)
