# Downloads FFmpeg (the "essentials" build of the current release from gyan.dev, the Windows builds the FFmpeg site
# links to) into %LOCALAPPDATA%\Subwoofa\ffmpeg, where Subwoofa looks for it. Used by the installer's optional step and by
# the Download button in Settings. Nothing is installed system-wide and nothing is added to PATH.
#
# The download is checked against the SHA-256 that gyan.dev publishes beside it, and the result is run once before it
# is kept. Exit code 0 means ffmpeg.exe is ready; anything else comes with a message saying why.
param([string]$Dir = (Join-Path $env:LOCALAPPDATA "Subwoofa\ffmpeg"))

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"   # the progress bar makes Invoke-WebRequest several times slower
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$base = "https://www.gyan.dev/ffmpeg/builds"
# a small download as text: the server labels some of them as plain binary, which PowerShell hands back as bytes
function Get-Text([string]$Url) {
    $content = (Invoke-WebRequest $Url -UseBasicParsing).Content
    if ($content -is [byte[]]) { return [Text.Encoding]::UTF8.GetString($content) }
    return [string]$content
}

$work = Join-Path ([IO.Path]::GetTempPath()) ("subwoofa-ffmpeg-" + [Guid]::NewGuid().ToString("N").Substring(0, 8))

try {
    New-Item -ItemType Directory -Force -Path $work | Out-Null
    $version = (Get-Text "$base/release-version").Trim()
    if ($version -notmatch '^\d+(\.\d+)+$') { throw "Unexpected version text from the server: '$version'" }
    $zipUrl = "$base/packages/ffmpeg-$version-essentials_build.zip"
    Write-Host "Downloading FFmpeg $version (about 115 MB)..."
    $expected = ((Get-Text "$zipUrl.sha256").Trim() -split '\s+')[0].ToLower()
    $zip = Join-Path $work "ffmpeg.zip"
    Invoke-WebRequest $zipUrl -OutFile $zip -UseBasicParsing

    Write-Host "Checking the download..."
    $actual = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
    if ($actual -ne $expected) { throw "The download is damaged (checksum $actual, expected $expected). Nothing was installed." }

    Write-Host "Unpacking..."
    Expand-Archive -Path $zip -DestinationPath (Join-Path $work "x") -Force
    $exe = Get-ChildItem (Join-Path $work "x") -Recurse -Filter ffmpeg.exe | Select-Object -First 1
    if (-not $exe) { throw "ffmpeg.exe was not in the download." }
    $out = & $exe.FullName -version 2>&1 | Out-String
    if ($LASTEXITCODE -ne 0 -or $out -notmatch "ffmpeg version") { throw "The downloaded ffmpeg.exe does not run." }

    New-Item -ItemType Directory -Force -Path $Dir | Out-Null
    Copy-Item $exe.FullName (Join-Path $Dir "ffmpeg.exe") -Force
    $probe = Get-ChildItem (Join-Path $work "x") -Recurse -Filter ffprobe.exe | Select-Object -First 1
    if ($probe) { Copy-Item $probe.FullName (Join-Path $Dir "ffprobe.exe") -Force }
    Set-Content -Path (Join-Path $Dir "SOURCE.txt") -Value "FFmpeg $version essentials build from $zipUrl`r`nSHA-256 $actual"
    Write-Host "FFmpeg $version is ready in $Dir"
    exit 0
}
catch {
    Write-Host "FFmpeg could not be set up: $($_.Exception.Message)"
    exit 1
}
finally {
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}
