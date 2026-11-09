# Fetches the pinned Zed source tree into client/zed.
#
# GitHub bulk transfers are unreliable on this network (measured ~11 KB/s), so this script is
# resumable: run it again and curl continues from the partial file.
param(
  [string]$Tag = "v1.27.0",
  [string]$Destination = (Join-Path $PSScriptRoot "zed"),
  [string]$Archive = (Join-Path $env:TEMP "zed-$Tag.tar.gz")
)

$ErrorActionPreference = "Stop"

if (Test-Path (Join-Path $Destination "Cargo.toml")) {
  Write-Host "Zed source already present at $Destination"
  exit 0
}

$url = "https://codeload.github.com/zed-industries/zed/tar.gz/refs/tags/$Tag"
Write-Host "Downloading $Tag (resumable) to $Archive"
& curl.exe -L --fail --retry 10 --retry-delay 5 --retry-all-errors -C - -o $Archive $url
if ($LASTEXITCODE -ne 0) { throw "curl exited with $LASTEXITCODE" }

$staging = Join-Path $env:TEMP "zed-extract-$([guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Force -Path $staging | Out-Null
Write-Host "Extracting"
& tar.exe -xzf $Archive -C $staging
if ($LASTEXITCODE -ne 0) { throw "tar exited with $LASTEXITCODE" }

$root = Get-ChildItem $staging -Directory | Select-Object -First 1
if (-not $root) { throw "archive did not contain a source directory" }

if (Test-Path $Destination) { Remove-Item $Destination -Recurse -Force }
Move-Item $root.FullName $Destination

$commit = $null
try {
  $release = Invoke-RestMethod -Uri "https://api.github.com/repos/zed-industries/zed/git/ref/tags/$Tag" `
    -Headers @{ "User-Agent" = "docuflow-client" } -TimeoutSec 30
  $commit = $release.object.sha
} catch {
  Write-Warning "Could not resolve $Tag to a commit: $($_.Exception.Message)"
}

@{
  tag = $Tag
  commit = $commit
  source = "https://github.com/zed-industries/zed"
  fetchedAt = (Get-Date).ToUniversalTime().ToString("o")
  note = "Fetched as a tarball because bulk git transfers fail on this network; the tree is not tracked by git."
} | ConvertTo-Json -Depth 3 | Set-Content (Join-Path $PSScriptRoot "PIN.json") -Encoding utf8

Write-Host "Zed $Tag source is at $Destination"
