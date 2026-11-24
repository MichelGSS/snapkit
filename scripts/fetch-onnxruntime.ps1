# Builds the vendor\onnxruntime folder: the ONNX Runtime DLL that runs the neural OCR models
# ("Read as handwriting").
#
# It's Microsoft's official CPU build, which picks the fastest code path for the processor at
# run time, so it also works on older processors without AVX2. SnapKit loads the DLL only when
# the neural engine is first used.
# Result: about 15 MB.
#
# Usage:  powershell -ExecutionPolicy Bypass -File scripts\fetch-onnxruntime.ps1

$ErrorActionPreference = 'Stop'
$version = '1.31.1'
$sha256 = '6871ccbc97f3d5bdb4095a1dcec33aa78d56f6ed8de5980ef5fa1d9f2e7c679a'
$name = "microsoft.ml.onnxruntime.$version"
$url = "https://api.nuget.org/v3-flatcontainer/microsoft.ml.onnxruntime/$version/$name.nupkg"

$root = Split-Path -Parent $PSScriptRoot
$dest = Join-Path $root 'vendor\onnxruntime'
$work = Join-Path ([IO.Path]::GetTempPath()) "snapkit-onnxruntime-$PID"

New-Item -ItemType Directory -Force $work | Out-Null
try {
    $zip = Join-Path $work "$name.zip"
    $ProgressPreference = 'SilentlyContinue'
    Invoke-WebRequest $url -OutFile $zip -UseBasicParsing
    $hash = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
    if ($hash -ne $sha256) { throw "unexpected SHA-256 for $name.nupkg: $hash" }
    Expand-Archive $zip "$work\x" -Force

    if (Test-Path $dest) { Remove-Item $dest -Recurse -Force }
    New-Item -ItemType Directory -Force $dest | Out-Null
    Copy-Item "$work\x\runtimes\win-x64\native\onnxruntime.dll" $dest
    Copy-Item "$work\x\LICENSE" "$dest\LICENSE.txt"
    Copy-Item "$work\x\ThirdPartyNotices.txt" $dest
    Get-ChildItem $dest | ForEach-Object { '{0}  {1:N1} MB' -f $_.Name, ($_.Length / 1MB) }
}
finally {
    Remove-Item $work -Recurse -Force -ErrorAction SilentlyContinue
}
