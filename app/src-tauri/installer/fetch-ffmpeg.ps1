# Downloads FFmpeg (the "essentials" build of the current release from gyan.dev, the Windows builds the FFmpeg site
# links to) into %LOCALAPPDATA%\Subwoofa\ffmpeg, where Subwoofa looks for it. Used by the installer's optional step and by
# the Download button in Settings. Nothing is installed system-wide and nothing is added to PATH.
#
# The download is checked against the SHA-256 that gyan.dev publishes beside it, and the result is run once before it
# is kept. Exit code 0 means ffmpeg.exe is ready; anything else comes with a message saying why.
param([string]$Dir = (Join-Path $env:LOCALAPPDATA "Subwoofa\ffmpeg"))

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"   # the progress bar makes Invoke-WebRequest several times slower
# TLS 1.2 added to what the system allows (Windows PowerShell 5.1 may not offer it by default); newer ones stay on
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

# everything said here also goes to a file, so a failure can be read (and copied) after the window is gone
$log = Join-Path ([IO.Path]::GetTempPath()) "subwoofa-ffmpeg-log.txt"
Set-Content -Path $log -Value ("Subwoofa FFmpeg download, " + (Get-Date -Format s))
function Say([string]$Message) { Write-Host $Message; Add-Content -Path $log -Value $Message }

$base = "https://www.gyan.dev/ffmpeg/builds"
# a small download as text: the server labels some of them as plain binary, which PowerShell hands back as bytes
function Get-Text([string]$Url) {
    $content = (Invoke-WebRequest $Url -UseBasicParsing).Content
    if ($content -is [byte[]]) { return [Text.Encoding]::UTF8.GetString($content) }
    return [string]$content
}

# Puts a program in place even while the old copy is running (an encode or check using FFmpeg): Windows will not
# overwrite a running program but will rename it, so the old copy steps aside first and is deleted once it can be
# (now, or on the next run of this script).
function Install-Program([string]$From, [string]$To) {
    Get-ChildItem -LiteralPath (Split-Path $To) -Filter ((Split-Path $To -Leaf) + ".old-*") -ErrorAction SilentlyContinue |
        Remove-Item -Force -ErrorAction SilentlyContinue
    if (Test-Path -LiteralPath $To) {
        $aside = $To + ".old-" + [Guid]::NewGuid().ToString("N").Substring(0, 8)
        Move-Item -LiteralPath $To -Destination $aside
        Remove-Item -LiteralPath $aside -Force -ErrorAction SilentlyContinue
    }
    Copy-Item -LiteralPath $From -Destination $To
}

$work = Join-Path ([IO.Path]::GetTempPath()) ("subwoofa-ffmpeg-" + [Guid]::NewGuid().ToString("N").Substring(0, 8))

try {
    New-Item -ItemType Directory -Force -Path $work | Out-Null
    $version = (Get-Text "$base/release-version").Trim()
    if ($version -notmatch '^\d+(\.\d+)+$') { throw "Unexpected version text from the server: '$version'" }
    $zipUrl = "$base/packages/ffmpeg-$version-essentials_build.zip"
    Say "Downloading FFmpeg $version (about 115 MB)..."
    $expected = ((Get-Text "$zipUrl.sha256").Trim() -split '\s+')[0].ToLower()
    $zip = Join-Path $work "ffmpeg.zip"
    Invoke-WebRequest $zipUrl -OutFile $zip -UseBasicParsing

    Say "Checking the download..."
    $actual = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
    if ($actual -ne $expected) { throw "The download is damaged (checksum $actual, expected $expected). Nothing was installed." }

    Say "Unpacking..."
    Expand-Archive -Path $zip -DestinationPath (Join-Path $work "x") -Force
    $exe = Get-ChildItem (Join-Path $work "x") -Recurse -Filter ffmpeg.exe | Select-Object -First 1
    if (-not $exe) { throw "ffmpeg.exe was not in the download." }
    $out = & $exe.FullName -version 2>&1 | Out-String
    if ($LASTEXITCODE -ne 0 -or $out -notmatch "ffmpeg version") { throw "The downloaded ffmpeg.exe does not run." }

    New-Item -ItemType Directory -Force -Path $Dir | Out-Null
    Install-Program $exe.FullName (Join-Path $Dir "ffmpeg.exe")
    $probe = Get-ChildItem (Join-Path $work "x") -Recurse -Filter ffprobe.exe | Select-Object -First 1
    if ($probe) { Install-Program $probe.FullName (Join-Path $Dir "ffprobe.exe") }
    Set-Content -Path (Join-Path $Dir "SOURCE.txt") -Value "FFmpeg $version essentials build from $zipUrl`r`nSHA-256 $actual"
    Say "FFmpeg $version is ready in $Dir"
    exit 0
}
catch {
    Say "FFmpeg could not be set up: $($_.Exception.Message)"
    Add-Content -Path $log -Value ($_ | Out-String)
    Add-Content -Path $log -Value ("PowerShell " + $PSVersionTable.PSVersion + " on " + [Environment]::OSVersion.VersionString)
    Say "A log was saved to $log"
    exit 1
}
finally {
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}
