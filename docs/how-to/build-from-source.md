# Build SnapKit from source

SnapKit is written in Rust. You can build it with a few commands on any Windows 10 or 11 computer.

## Prerequisites

- [Rust](https://rustup.rs), stable toolchain.
- [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) with the **Desktop development with C++** workload.
- [Git](https://git-scm.com).

## Build and run

```powershell
git clone https://github.com/MichelGSS/snapkit.git
cd snapkit
cargo run
```

The first build takes a few minutes. Later builds are incremental.

## Add OCR and video

Text recognition and video recording use third-party programs that aren't stored in the repository. Each script downloads one of them into `vendor\`:

| Feature | Script | The script needs |
|---|---|---|
| OCR (Tesseract) | `scripts\fetch-tesseract.ps1` | winget, 7-Zip, Python with `pefile` |
| Handwriting OCR (neural models) | `scripts\fetch-ocr-neural.ps1` | Python with pip |
| Handwriting OCR (ONNX Runtime) | `scripts\fetch-onnxruntime.ps1` | Nothing else |
| Video recording (FFmpeg) | `scripts\fetch-ffmpeg.ps1` | winget |

Run each one from the repository root:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\fetch-tesseract.ps1
powershell -ExecutionPolicy Bypass -File scripts\fetch-ocr-neural.ps1
powershell -ExecutionPolicy Bypass -File scripts\fetch-onnxruntime.ps1
powershell -ExecutionPolicy Bypass -File scripts\fetch-ffmpeg.ps1
```

Without these folders SnapKit still runs; only the matching features are unavailable.

## Run the tests

```powershell
cargo test
```

Some tests use the real screen, Tesseract, the neural models and FFmpeg. When the `CI` environment variable is set, as it is on GitHub Actions, those tests skip themselves.

## Build the packages

```powershell
powershell -ExecutionPolicy Bypass -File scripts\package.ps1
```

The script builds a release version and creates the portable zip in `dist\`. If [Inno Setup 6](https://jrsoftware.org/isinfo.php) is installed, it also creates the installer.

## Related

- [Contributing guidelines](../../CONTRIBUTING.md)
- [Command-line options](../reference/command-line.md)
