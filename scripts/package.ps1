# Builds the SnapKit distribution packages in dist\:
#   SnapKit-<version>-windows-x64.zip   (portable: unzip and run)
#   SnapKit-<version>-setup.exe         (installer, if Inno Setup 6 is installed)
#
# Before running it, the four vendor\ folders must exist (scripts\fetch-*.ps1).
# Usage:  powershell -ExecutionPolicy Bypass -File scripts\package.ps1

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

foreach ($dir in 'tesseract', 'ocr-neural', 'onnxruntime', 'ffmpeg') {
    if (-not (Test-Path "vendor\$dir")) { throw "vendor\$dir is missing - run the matching scripts\fetch-*.ps1" }
}

$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
"Version $version"

cargo build --release
if ($LASTEXITCODE -ne 0) { throw 'the build failed' }

$stage = Join-Path $root 'dist\SnapKit'
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Force $stage | Out-Null

Copy-Item 'target\release\snapkit.exe' $stage

# snapkit.exe and onnxruntime.dll need the Visual C++ runtime. A clean Windows 10 may not have
# it, and then SnapKit doesn't even start, so its DLLs ship next to the executable (app-local
# deployment, allowed by the Visual Studio redistribution terms).
$crt = Get-ChildItem "${env:ProgramFiles(x86)}\Microsoft Visual Studio\*\*\VC\Redist\MSVC\*\x64\Microsoft.VC14*.CRT", "$env:ProgramFiles\Microsoft Visual Studio\*\*\VC\Redist\MSVC\*\x64\Microsoft.VC14*.CRT" -Directory -ErrorAction SilentlyContinue |
    Sort-Object { [version]$_.Parent.Parent.Name } | Select-Object -Last 1
if (-not $crt) { throw 'Visual C++ redistributable files not found (install the "MSVC v14x - VS C++ x64/x86 build tools" component)' }
"Visual C++ runtime from $($crt.FullName)"
foreach ($dll in 'msvcp140.dll', 'msvcp140_1.dll', 'vcruntime140.dll', 'vcruntime140_1.dll') {
    Copy-Item (Join-Path $crt.FullName $dll) $stage
}
foreach ($dir in 'tesseract', 'ocr-neural', 'onnxruntime', 'ffmpeg') { Copy-Item "vendor\$dir" "$stage\$dir" -Recurse }
Copy-Item 'licenses' "$stage\licenses" -Recurse
# .txt and .md extensions so they open with one click on any Windows machine.
Copy-Item 'LICENSE' "$stage\LICENSE.txt"
Copy-Item 'THIRD-PARTY-NOTICES.md' $stage

$zip = Join-Path $root "dist\SnapKit-$version-windows-x64.zip"
if (Test-Path $zip) { Remove-Item $zip -Force }
Compress-Archive -Path $stage -DestinationPath $zip
'{0}  {1:N0} MB' -f (Split-Path $zip -Leaf), ((Get-Item $zip).Length / 1MB)

$iscc = @(
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "$env:ProgramFiles\Inno Setup 6\ISCC.exe",
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if ($iscc) {
    & $iscc "/DAppVersion=$version" "/DSourceDir=$stage" /Qp 'installer\snapkit.iss'
    if ($LASTEXITCODE -ne 0) { throw 'Inno Setup failed' }
    $setup = Join-Path $root "dist\SnapKit-$version-setup.exe"
    '{0}  {1:N0} MB' -f (Split-Path $setup -Leaf), ((Get-Item $setup).Length / 1MB)
} else {
    'Inno Setup 6 not found: only the zip was built. Install it with: winget install JRSoftware.InnoSetup'
}
