# Troubleshoot common problems

## Windows shows "Windows protected your PC"

The installer isn't digitally signed yet. Select **More info**, and then select **Run anyway**. If you prefer, [build SnapKit from source](build-from-source.md).

## The shortcut doesn't start a capture

- Another app may use the same combination. SnapKit shows **Shortcut … unavailable** in the status bar when it starts. Choose another shortcut in [Preferences](customize.md#change-the-capture-shortcut).
- If you closed SnapKit completely, the shortcut stops working. Turn on **Closing the window keeps SnapKit in the tray** and **Start with Windows** to keep it always available.
- If you chose **PrintScreen** and the Windows Snipping Tool opens instead, turn off **Use the Print screen key to open screen capture** in **Settings** > **Accessibility** > **Keyboard**.

## SnapKit doesn't open, or opens with a black window

SnapKit draws its window with OpenGL, which it needs for the transparent capture overlay. Some remote desktop sessions and virtual machines don't provide OpenGL.

- Update your graphics driver.
- In a virtual machine, turn on 3D acceleration.
- In a remote session, run SnapKit on the local computer instead.

## Text recognition isn't available

If **Read text** reports that no OCR engine was found, the `tesseract` and `ocr-neural` folders are missing next to `snapkit.exe`. Reinstall SnapKit, or extract the full zip again. If you built SnapKit yourself, run the scripts in [Build SnapKit from source](build-from-source.md#add-ocr-and-video).

## Video recording isn't available

If recording reports that **FFmpeg was not found**, the `ffmpeg` folder is missing next to `snapkit.exe`. Reinstall SnapKit or extract the full zip again.

## A recording has no sound

- Check that **System sound** or **Microphone** was selected in the panel before you selected **Record**.
- If a source fails, SnapKit records without it and shows a warning such as **no microphone found**. Check the device in **Settings** > **System** > **Sound**.
- Windows only sends system sound while something is playing. Silence at the start of a recording is expected.

## Scrolling capture stops early or the image looks broken

- Pause videos and animations in the page before you start.
- Don't move the mouse during the capture.
- Select only the part of the window that scrolls.
- Use [panoramic capture](capture-panoramic.md) and scroll by hand.

The editor's status bar shows why the capture stopped, for example **stopped: the content changed in a way that could not be stitched**.

## The captured area is shifted on a high-DPI monitor

On setups that mix monitors with different scaling, a capture or recording area can come out slightly shifted. Set the same scaling on all monitors in **Settings** > **System** > **Display**, or capture on the main monitor.

## Still stuck?

[Open an issue](https://github.com/MichelGSS/snapkit/issues/new/choose) on GitHub. Describe what you were doing, what you expected and what happened, and attach a screenshot. Taking one with SnapKit is encouraged.
