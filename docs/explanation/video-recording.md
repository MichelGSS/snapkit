# How video recording works

SnapKit records video with FFmpeg, which is bundled next to `snapkit.exe`, and records audio itself. The two are joined when you stop.

## Picture and sound, recorded apart

During a recording:

- **FFmpeg** captures only the picture of the area, with the Windows GDI screen grabber, and encodes it as H.264. The bit rate scales with the area: about 8 Mbit/s for 1920 × 1080.
- **SnapKit** records the system sound (WASAPI loopback) and the microphone, converts both to 48 kHz stereo, and mixes them into a WAV file.

When you stop, FFmpeg copies the picture without re-encoding it and adds the sound as AAC, producing one MP4 file.

Recording sound and picture in the same FFmpeg process makes the screen capture drop frames, because FFmpeg holds the picture back while it waits for audio. Recording them apart keeps the picture smooth.

## Keeping sound and picture in sync

Windows sends no system sound at all while nothing is playing. If SnapKit simply wrote what it received, a silent minute would vanish from the audio file and everything after it would drift out of sync.

So the audio recording follows the clock: at every moment, SnapKit writes exactly as many samples as the elapsed time requires, filling with silence whatever didn't arrive. The audio file's length always matches real time.

Picture and sound stop at the same instant. FFmpeg takes a few tenths of a second to start, so the audio starts slightly earlier; that extra audio at the start is dropped when the two are joined. Aligning them at the end gives accurate sync.

## Pausing

FFmpeg can't pause a screen capture. When you select **Pause**, SnapKit closes the current part. **Resume** starts a new part. When you stop, the parts are joined without re-encoding, so pausing doesn't cost quality.

## Trimming and GIFs

- **Save only this clip** re-encodes the chosen range so the cut is accurate to the frame.
- **Create GIF** builds a palette from the clip's own colors first, then uses it to encode the GIF. A palette made for the video avoids the banding and dithering of generic GIF palettes.

## Monitor scaling

SnapKit tells Windows it's aware of per-monitor scaling, so FFmpeg receives real pixel coordinates. Without that, Windows would report scaled coordinates on monitors that aren't at 100%, and the recorded area would come out shifted and blurry.

## Related

- [Record the screen as video or GIF](../how-to/record-video-gif.md)
- [Trim a recording and convert it to GIF](../how-to/trim-and-convert.md)
