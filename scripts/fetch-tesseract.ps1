# Builds the vendor\tesseract folder used by SnapKit's OCR.
#
# Downloads the official Tesseract installer for Windows (UB Mannheim) through winget, which
# checks the hash, extracts it without installing, copies only the executable and the DLLs it
# depends on, strips the debug symbols from the main library (from ~97 MB to ~3 MB) and
# downloads the Portuguese model. Result: about 28 MB.
#
# Requirements: winget, 7-Zip (7z on the PATH) and Python with the pefile package (pip install pefile).
# Usage:  powershell -ExecutionPolicy Bypass -File scripts\fetch-tesseract.ps1

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$dest = Join-Path $root 'vendor\tesseract'
$work = Join-Path ([IO.Path]::GetTempPath()) "snapkit-tesseract-$PID"

# Transitive closure of the imports of tesseract.exe in the 5.4.0.20240606 distribution.
$files = @(
    'tesseract.exe', 'libtesseract-5.dll', 'libleptonica-6.dll', 'libarchive-13.dll', 'libb2-1.dll',
    'libbz2-1.dll', 'libcrypto-3-x64.dll', 'libdeflate.dll', 'libexpat-1.dll', 'libgcc_s_seh-1.dll',
    'libgif-7.dll', 'libiconv-2.dll', 'libjbig-0.dll', 'libjpeg-8.dll', 'libLerc.dll', 'liblz4.dll',
    'liblzma-5.dll', 'libopenjp2-7.dll', 'libpng16-16.dll', 'libsharpyuv-0.dll', 'libstdc++-6.dll',
    'libtiff-6.dll', 'libwebp-7.dll', 'libwebpmux-3.dll', 'libwinpthread-1.dll', 'libzstd.dll', 'zlib1.dll'
)

New-Item -ItemType Directory -Force "$work\dl" | Out-Null
try {
    winget download --id UB-Mannheim.TesseractOCR --version 5.4.0.20240606 -d "$work\dl" `
        --accept-source-agreements --accept-package-agreements | Out-Null
    $installer = (Get-ChildItem "$work\dl\*.exe" | Select-Object -First 1).FullName
    if (-not $installer) { throw 'winget did not download the Tesseract installer' }
    7z x $installer "-o$work\x" -y | Out-Null

    if (Test-Path $dest) { Remove-Item $dest -Recurse -Force }
    New-Item -ItemType Directory -Force "$dest\tessdata" | Out-Null
    foreach ($f in $files) { Copy-Item "$work\x\$f" "$dest\$f" }
    Copy-Item "$work\x\tessdata\eng.traineddata" "$dest\tessdata\"

    python -I "$PSScriptRoot\strip_debug.py" "$work\x\libtesseract-5.dll" "$dest\libtesseract-5.dll" | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'could not strip the debug symbols (is the pefile package installed?)' }

    curl.exe -sSL --fail -o "$dest\tessdata\por.traineddata" `
        'https://github.com/tesseract-ocr/tessdata_fast/raw/4.1.0/por.traineddata'
    if ($LASTEXITCODE -ne 0) { throw 'could not download the Portuguese model' }

    & "$dest\tesseract.exe" --list-langs
    $mb = (Get-ChildItem $dest -Recurse -File | Measure-Object Length -Sum).Sum / 1MB
    '{0}: {1:N1} MB' -f $dest, $mb
}
finally {
    Remove-Item $work -Recurse -Force -ErrorAction SilentlyContinue
}
