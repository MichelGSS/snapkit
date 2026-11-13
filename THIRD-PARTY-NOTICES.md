# Third-party notices

SnapKit's code is distributed under the MIT License (the `LICENSE` file). The distribution packages (installer and zip) also include the programs and data below, each under its own license. The full license texts are in the `licenses` folder.

## Bundled programs and data

| Component | Location | Version | License | Source |
|---|---|---|---|---|
| Tesseract OCR | `tesseract\tesseract.exe`, `libtesseract-5.dll` | 5.4.0 | Apache-2.0 | https://github.com/tesseract-ocr/tesseract (UB Mannheim build) |
| Leptonica | `tesseract\libleptonica-6.dll` | 1.84.1 | BSD-2-Clause | http://www.leptonica.org |
| Tesseract language data (`eng`, `por`) | `tesseract\tessdata` | tessdata_fast 4.1.0 | Apache-2.0 | https://github.com/tesseract-ocr/tessdata_fast |
| Tesseract support libraries (libpng, libjpeg, libtiff, zlib, libwebp, OpenJPEG, giflib, libarchive, OpenSSL, xz, zstd, bzip2, lz4 and others) | `tesseract\*.dll` | Those of the build above | Each its own (permissive; see each project's site) | Included in the UB Mannheim build |
| GCC/MinGW runtime libraries | `tesseract\libgcc_s_seh-1.dll`, `libstdc++-6.dll`, `libwinpthread-1.dll` | — | GPL-3.0 with the GCC Runtime Library Exception; MIT/BSD (winpthreads) | https://www.mingw-w64.org |
| PP-OCRv3 models (detection and recognition) | `ocr-neural\det.onnx`, `rec.onnx` | PP-OCRv3 | Apache-2.0 | https://github.com/PaddlePaddle/PaddleOCR, converted by https://github.com/RapidAI/RapidOCR |
| ONNX Runtime | `onnxruntime\onnxruntime.dll` (Microsoft's official build, unmodified; its license and third-party notices are in the same folder) | 1.31.1 | MIT | https://github.com/microsoft/onnxruntime |
| Microsoft Visual C++ runtime | `msvcp140.dll`, `msvcp140_1.dll`, `vcruntime140.dll`, `vcruntime140_1.dll` | 14.50 | Microsoft Visual Studio redistributable files (distributed unmodified, app-local) | https://learn.microsoft.com/cpp/windows/redistributing-visual-cpp-files |
| FFmpeg | `ffmpeg\ffmpeg.exe` | 9.0.1 (LGPL variant, BtbN build) | LGPL-3.0 (`licenses\FFmpeg-LICENSE.txt`, which supplements `GPL-3.0.txt`) | https://ffmpeg.org, https://github.com/BtbN/FFmpeg-Builds |
| OpenH264 | Inside `ffmpeg.exe` | That of the build above | BSD-2-Clause | https://github.com/cisco/openh264 |

### About FFmpeg (LGPL)

`ffmpeg.exe` is distributed unmodified, as a separate program that SnapKit runs. The LGPL guarantees your right to replace it: replace the file `ffmpeg\ffmpeg.exe` with another build. The corresponding source code is available at https://github.com/BtbN/FFmpeg-Builds (build scripts) and https://ffmpeg.org (sources).

### About Tesseract

Only the debug symbol sections were removed from `libtesseract-5.dll` (`scripts/strip_debug.py`), to reduce its size from 97 MB to 3 MB. The executable code wasn't changed.

## Rust libraries

`snapkit.exe` is built with open-source Rust libraries, almost all under MIT and/or Apache-2.0. The exact list, with versions, is in the repository's `Cargo.lock` file. The main ones are egui/eframe, image, tiny-skia, ab_glyph, xcap, arboard, rfd, global-hotkey, tray-icon, cpal, ort, serde, winreg, trash and windows-sys.

The interface uses egui's default fonts (Ubuntu Light, Hack, Noto Emoji, emoji-icon-font), under the Ubuntu Font Licence, MIT and OFL licenses.

## Known limitation

This file was written by hand from what the project bundles; it isn't a legal audit. The list of Tesseract support libraries and Rust libraries doesn't give each item's license individually. Before a wide distribution, it's worth generating this list with a tool (for example `cargo about`) and checking each DLL.
