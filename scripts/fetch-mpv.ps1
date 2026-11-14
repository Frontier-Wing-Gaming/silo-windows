# Downloads a Windows build of mpv into ./mpv so `npm run build` bundles it.
# Source: the shinchiro mpv-winbuild-cmake releases on GitHub (GPL builds).
# The default destination is <repo>/mpv wherever the script is run from.
param([string]$Destination = (Join-Path (Split-Path $PSScriptRoot -Parent) "mpv"))

$ErrorActionPreference = "Stop"
$release = Invoke-RestMethod -Headers @{ "User-Agent" = "silolux-windows-build" } `
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
Write-Host "mpv is in $Destination"
