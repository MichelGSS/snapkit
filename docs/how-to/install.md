# Install, update or uninstall SnapKit

SnapKit comes in two packages on the [Releases page](https://github.com/MichelGSS/snapkit/releases):

| Package | Best for |
|---|---|
| `SnapKit-<version>-setup.exe` | Most people. Adds SnapKit to the Start menu and keeps your settings between updates. |
| `SnapKit-<version>-windows-x64.zip` | Portable use, or computers where you can't run installers. |

Both packages include everything: OCR engines and FFmpeg for video. Neither needs administrator rights or an internet connection after you download it.

Before you start, check the [system requirements](../reference/requirements-and-limits.md).

## Install with the installer

1. Download `SnapKit-<version>-setup.exe` from the [Releases page](https://github.com/MichelGSS/snapkit/releases).
2. Open the file.
3. If Windows shows **Windows protected your PC**, select **More info**, and then select **Run anyway**.

   This message appears because the installer isn't digitally signed yet. You can check the download against the source code on GitHub.

4. Choose the installer language and follow the steps. To add a desktop icon, select **Create a desktop shortcut**.
5. On the last page, keep **Launch SnapKit** selected and select **Finish**.

SnapKit installs to `%LOCALAPPDATA%\Programs\SnapKit`, for your user account only. An orange icon appears in the system tray, next to the clock.

## Use the portable zip

1. Download `SnapKit-<version>-windows-x64.zip`.
2. Right-click the file, select **Extract All**, and choose a folder.
3. Open the `SnapKit` folder and run `snapkit.exe`.

Keep the `tesseract`, `ocr-neural`, `onnxruntime` and `ffmpeg` folders and the `.dll` files next to `snapkit.exe`. Without them, SnapKit still opens, but text recognition or video recording isn't available.

> [!NOTE]
> The portable version still saves its preferences in `%APPDATA%\SnapKit` and your captures in your **Pictures** folder. See [Files and folders](../reference/files-and-folders.md).

## Update SnapKit

- **Installer:** download the new `setup.exe` and run it. It replaces the old version and keeps your preferences, styles and library. If SnapKit is running in the tray, the installer closes it first.
- **Zip:** extract the new zip over the old folder, or into a new folder.

## Uninstall SnapKit

1. Open **Settings** > **Apps** > **Installed apps**.
2. Find **SnapKit**, select **More options** (**…**), and then select **Uninstall**.

Uninstalling removes the program and the **Start with Windows** entry. It doesn't delete your captures, recordings or preferences. To remove them too, delete these folders:

- `%APPDATA%\SnapKit` (preferences and styles)
- `Pictures\SnapKit` (capture library)
- `Videos\SnapKit` (recordings)

## Related

- [Customize SnapKit](customize.md)
- [Build SnapKit from source](build-from-source.md)
