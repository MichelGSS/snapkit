# Builds the vendor\ffmpeg folder used by SnapKit's video recording.
#
# Downloads BtbN's static LGPL build of FFmpeg 9.0 through winget (which checks the hash)
# and copies only ffmpeg.exe and its license. Nothing is installed on the system.
# The LGPL variant doesn't include x264; SnapKit encodes with libopenh264, which it does include.
# Result: about 110 MB.
#
# Requirement: winget.
# Usage:  powershell -ExecutionPolicy Bypass -File scripts\fetch-ffmpeg.ps1

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$dest = Join-Path $root 'vendor\ffmpeg'
$work = Join-Path ([IO.Path]::GetTempPath()) "snapkit-ffmpeg-$PID"

New-Item -ItemType Directory -Force "$work\dl" | Out-Null
try {
    winget download --id BtbN.FFmpeg.LGPL.9.0 -d "$work\dl" `
        --accept-source-agreements --accept-package-agreements | Out-Null
    $zip = (Get-ChildItem "$work\dl\*.zip" | Select-Object -First 1).FullName
    if (-not $zip) { throw 'winget did not download FFmpeg' }
    Copy-Item $zip "$work\ffmpeg.zip"
    Expand-Archive "$work\ffmpeg.zip" "$work\x" -Force
    $exe = Get-ChildItem "$work\x" -Recurse -Filter ffmpeg.exe | Select-Object -First 1
    if (-not $exe) { throw 'ffmpeg.exe not found in the package' }

    New-Item -ItemType Directory -Force $dest | Out-Null
    Copy-Item $exe.FullName "$dest\ffmpeg.exe"
    Copy-Item (Join-Path $exe.Directory.Parent.FullName 'LICENSE.txt') "$dest\LICENSE.txt"
    & "$dest\ffmpeg.exe" -hide_banner -version | Select-Object -First 1
    '{0}: {1:N0} MB' -f $dest, ((Get-Item "$dest\ffmpeg.exe").Length / 1MB)
}
finally {
    Remove-Item $work -Recurse -Force -ErrorAction SilentlyContinue
}
