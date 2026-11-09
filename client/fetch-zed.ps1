# Provisions the pinned Zed source tree into client/zed.
#
# Two sources are supported:
#   -FromPath <dir>   copy a local extracted archive (offline, preferred on this network)
#   (default)         download the pinned tag tarball, resuming is impossible on codeload
#
# The tree itself stays gitignored; client/PIN.json records what was provisioned so a clone can be
# rebuilt without guessing.
param(
  [string]$Tag = "v1.27.0",
  [string]$FromPath,
  [string]$Destination = (Join-Path $PSScriptRoot "zed"),
  [string]$Archive = (Join-Path $env:TEMP "zed-$Tag.tar.gz")
)

$ErrorActionPreference = "Stop"

function Write-Pin {
  param([string]$Mode)
  $version = $null
  $toolchain = $null
  $crateManifest = Join-Path $Destination "crates/zed/Cargo.toml"
  if (Test-Path $crateManifest) {
    $versionLine = Select-String -Path $crateManifest -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
    if ($versionLine) { $version = $versionLine.Matches[0].Groups[1].Value }
  }
  $toolchainFile = Join-Path $Destination "rust-toolchain.toml"
  if (Test-Path $toolchainFile) {
    $channel = Select-String -Path $toolchainFile -Pattern 'channel\s*=\s*"([^"]+)"' | Select-Object -First 1
    if ($channel) { $toolchain = $channel.Matches[0].Groups[1].Value }
  }
  $files = Get-ChildItem $Destination -Recurse -File -Force
  @{
    source = "https://github.com/zed-industries/zed"
    tag = $Tag
    mode = $Mode
    zedVersion = $version
    rustToolchain = $toolchain
    files = $files.Count
    sizeBytes = ($files | Measure-Object Length -Sum).Sum
    fetchedAt = (Get-Date).ToUniversalTime().ToString("o")
    note = "Tree is gitignored; re-provision with client/fetch-zed.ps1. No .git in the archive, so there is no commit to pin."
  } | ConvertTo-Json -Depth 3 | Set-Content (Join-Path $PSScriptRoot "PIN.json") -Encoding utf8
}

if (Test-Path (Join-Path $Destination "Cargo.toml")) {
  Write-Host "Zed source already present at $Destination"
  exit 0
}

if ($FromPath) {
  if (-not (Test-Path (Join-Path $FromPath "Cargo.toml"))) {
    throw "$FromPath does not look like a Zed checkout (no Cargo.toml)"
  }
  Write-Host "Copying $FromPath to $Destination"
  New-Item -ItemType Directory -Force -Path (Split-Path $Destination -Parent) | Out-Null
  & robocopy $FromPath $Destination /E /NFL /NDL /NJH /NJS /R:1 /W:1 | Out-Null
  if ($LASTEXITCODE -ge 8) { throw "robocopy failed with $LASTEXITCODE" }
  Write-Pin -Mode "local-copy"
  Write-Host "Zed source copied; PIN.json written"
  exit 0
}

$url = "https://codeload.github.com/zed-industries/zed/tar.gz/refs/tags/$Tag"
Write-Host "Downloading $Tag to $Archive"

$attempt = 0
while ($attempt -lt 3) {
  $attempt += 1
  & curl.exe -L --fail --retry 10 --retry-delay 5 --retry-all-errors -C - -o $Archive $url
  if ($LASTEXITCODE -eq 0) { break }
  if ($LASTEXITCODE -eq 33) {
    # codeload does not support byte ranges; a partial file can never resume.
    Write-Host "Server does not support resume; restarting the download from zero"
    Remove-Item $Archive -Force -ErrorAction SilentlyContinue
    continue
  }
  if ($attempt -ge 3) { throw "curl exited with $LASTEXITCODE after $attempt attempts" }
  Start-Sleep -Seconds 5
}
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

Write-Pin -Mode "tarball"
Write-Host "Zed $Tag source is at $Destination"
