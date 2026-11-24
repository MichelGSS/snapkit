# Install, update or uninstall SnapKit

SnapKit ships packages for **Windows**, **macOS**, and **Linux** on the [Releases page](https://github.com/MichelGSS/snapkit/releases) and at [snapkitapp.xyz](https://snapkitapp.xyz/):

| Platform | Package | Best for |
|---|---|---|
| Windows | `SnapKit-<version>-setup.exe` | Most Windows users. Adds SnapKit to the Start menu and keeps your settings between updates. |
| Windows | `SnapKit-<version>-windows-x64.zip` | Portable use on Windows, or computers where you can't run installers. |
| macOS | `SnapKit-<version>-macos.dmg` | Standard macOS drag-and-drop install (Universal: Apple Silicon & Intel, bundled FFmpeg & Apple Vision OCR). |
| macOS | `SnapKit-<version>-macos.zip` | Portable macOS archive containing `SnapKit.app`. |
| Linux | `SnapKit-<version>-linux-amd64.deb` | Debian, Ubuntu, and derivatives (`sudo apt install ./SnapKit-<version>-linux-amd64.deb`). |
| Linux | `SnapKit-<version>-linux-x86_64.tar.gz` | Portable Linux archive for any x86_64 distribution (includes `install-desktop.sh`). |

Before you start, check the [system requirements](../reference/requirements-and-limits.md).

## Windows

### Install with the installer

1. Download `SnapKit-<version>-setup.exe` from the [Releases page](https://github.com/MichelGSS/snapkit/releases).
2. Open the file.
3. If Windows shows **Windows protected your PC**, select **More info**, and then select **Run anyway**.

   This message appears because the installer isn't digitally signed yet. You can check `SHA256SUMS.txt` or [build from source](build-from-source.md).

4. Choose the installer language and follow the steps. To add a desktop icon, select **Create a desktop shortcut**.
5. On the last page, keep **Launch SnapKit** selected and select **Finish**.

SnapKit installs to `%LOCALAPPDATA%\Programs\SnapKit`, for your user account only.

### Use the portable zip

1. Download `SnapKit-<version>-windows-x64.zip`.
2. Right-click the file, select **Extract All**, and choose a folder.
3. Open the `SnapKit` folder and run `snapkit.exe`.

Keep the `tesseract`, `ocr-neural`, `onnxruntime` and `ffmpeg` folders and the `.dll` files next to `snapkit.exe`.

## macOS (Universal: Apple Silicon & Intel)

1. Download `SnapKit-<version>-macos.dmg` (or `SnapKit-<version>-macos.zip`).
2. Open the `.dmg` and drag `SnapKit.app` into **Applications**.
3. On first launch, because the app is signed ad-hoc (not Apple-notarized yet), right-click `SnapKit.app` and choose **Open** (or go to **System Settings > Privacy & Security > Open Anyway**).
4. Grant **Screen Recording** (for captures and video) and **Accessibility** (for global shortcuts, scrolling capture, and step-by-step clicks) when prompted.

`SnapKit.app` bundles universal FFmpeg and uses macOS's built-in **Apple Vision** framework for OCR.

## Linux (x86_64)

### Debian / Ubuntu (`.deb`)

```bash
sudo apt install ./SnapKit-<version>-linux-amd64.deb
```

### Portable archive (`.tar.gz`)

```bash
tar -xzf SnapKit-<version>-linux-x86_64.tar.gz
cd SnapKit-<version>-linux-x86_64
./snapkit
```

Run `./install-desktop.sh` inside the extracted folder if you want a desktop launcher entry and icons in `~/.local/share/`. For video recording and OCR on Linux, install `ffmpeg` and `tesseract-ocr` (`tesseract-ocr-eng`, `tesseract-ocr-por`).

## Update SnapKit

The quickest way is from SnapKit itself: open **Preferences**, select **Check for updates**, and then **Update now**. SnapKit downloads the new installer, checks it, installs it and opens again. To be told about new versions, turn on **Check automatically (once a day)**.

You can also update by hand:

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
