# Builds the vendor\ocr-neural folder used by SnapKit's "Read as handwriting" mode.
#
# The models (PP-OCRv3 detection and recognition, in ONNX) ship inside the
# rapidocr-onnxruntime package on PyPI. The package is only downloaded and opened as a zip:
# nothing is installed.
# Result: about 13 MB.
#
# Requirement: Python with pip.
# Usage:  powershell -ExecutionPolicy Bypass -File scripts\fetch-ocr-neural.ps1

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$dest = Join-Path $root 'vendor\ocr-neural'
$work = Join-Path ([IO.Path]::GetTempPath()) "snapkit-ocr-neural-$PID"

New-Item -ItemType Directory -Force "$work\dl" | Out-Null
try {
    python -m pip download --quiet --disable-pip-version-check --no-deps rapidocr-onnxruntime==1.2.3 -d "$work\dl"
    if ($LASTEXITCODE -ne 0) { throw 'could not download the rapidocr-onnxruntime package' }
    $wheel = (Get-ChildItem "$work\dl\*.whl" | Select-Object -First 1).FullName
    Copy-Item $wheel "$work\dl\package.zip"
    Expand-Archive "$work\dl\package.zip" "$work\x" -Force

    New-Item -ItemType Directory -Force $dest | Out-Null
    $models = "$work\x\rapidocr_onnxruntime\models"
    Copy-Item "$models\ch_PP-OCRv3_det_infer.onnx" "$dest\det.onnx"
    Copy-Item "$models\ch_PP-OCRv3_rec_infer.onnx" "$dest\rec.onnx"
    Get-ChildItem $dest | ForEach-Object { '{0}  {1:N1} MB' -f $_.Name, ($_.Length / 1MB) }
}
finally {
    Remove-Item $work -Recurse -Force -ErrorAction SilentlyContinue
}
