# Downloads a Windows build of mpv into ./mpv so `npm run build` bundles it.
# Source: the shinchiro mpv-winbuild-cmake releases on GitHub (GPL builds).
# The default destination is <repo>/mpv wherever the script is run from.
param([string]$Destination = (Join-Path (Split-Path $PSScriptRoot -Parent) "mpv"))

$ErrorActionPreference = "Stop"
# Anonymous API calls share a 60/hour limit per IP, which hosted CI runners exhaust; send
# a token when there is one (the release workflow passes GITHUB_TOKEN).
$apiHeaders = @{ "User-Agent" = "silolux-windows-build" }
if ($env:GITHUB_TOKEN) { $apiHeaders["Authorization"] = "Bearer $env:GITHUB_TOKEN" }
$release = Invoke-RestMethod -Headers $apiHeaders `
  -Uri "https://api.github.com/repos/shinchiro/mpv-winbuild-cmake/releases/latest"
$asset = $release.assets |
  Where-Object { $_.name -match '^mpv-x86_64-v3-\d{8}-git-[0-9a-f]+\.7z$' } |
  Select-Object -First 1
if (-not $asset) {
  $asset = $release.assets | Where-Object { $_.name -match '^mpv-x86_64-\d{8}-git-[0-9a-f]+\.7z$' } | Select-Object -First 1
}
if (-not $asset) { throw "No mpv x86_64 archive in release $($release.tag_name)" }

$tmp = Join-Path ([IO.Path]::GetTempPath()) ("silo-mpv-" + [guid]::NewGuid())
New-Item -ItemType Directory -Force $tmp | Out-Null
$archive = Join-Path $tmp $asset.name
Write-Host "Downloading $($asset.name)"
Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $archive

New-Item -ItemType Directory -Force $Destination | Out-Null
# Windows' own bsdtar reads 7z archives; a GNU tar earlier on PATH does not.
& "$env:SystemRoot\System32\tar.exe" -xf $archive -C $Destination
if ($LASTEXITCODE -ne 0) { throw "tar could not extract $($asset.name)" }
Remove-Item -Recurse -Force $tmp
if (-not (Test-Path (Join-Path $Destination "mpv.exe"))) { throw "mpv.exe missing after extract" }

# The build is GPL and ships without its license text. An installer that bundles it
# must carry the license and say where the matching source is, so record both.
$commit = if ($asset.name -match 'git-([0-9a-f]+)\.7z$') { $Matches[1] } else { "master" }
Invoke-WebRequest -Headers @{ "User-Agent" = "silolux-windows-build" } `
  -Uri "https://raw.githubusercontent.com/mpv-player/mpv/$commit/LICENSE.GPL" `
  -OutFile (Join-Path $Destination "LICENSE.GPL.txt")
@"
mpv, bundled with Silolux for Windows as a separate program.
Silolux starts it as a player; it is not part of Silolux's own MIT-licensed code.

Build:   $($asset.name)
From:    $($release.html_url)
License: GNU GPL, see LICENSE.GPL.txt in this folder. The build includes
         GPL-licensed libraries (FFmpeg and others) under their own licenses.

Source code for this exact build:
  mpv:            https://github.com/mpv-player/mpv/tree/$commit
  build scripts:  https://github.com/shinchiro/mpv-winbuild-cmake
                  (the scripts list every library and version they compile)
"@ | Set-Content -Encoding utf8 (Join-Path $Destination "SOURCE.txt")
Write-Host "mpv is in $Destination"
