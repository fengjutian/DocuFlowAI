# Builds the pinned Zed source in client/zed.
#
# Zed v1.27.0 pins Rust 1.98.1 via rust-toolchain.toml. The host currently runs 1.96.0, so the first
# build installs the pinned toolchain and the wasm targets; expect a long download and a long compile.
param(
  [switch]$Release,
  [string]$Package = "zed"
)

$ErrorActionPreference = "Stop"
$zed = Join-Path $PSScriptRoot "zed"

if (-not (Test-Path (Join-Path $zed "Cargo.toml"))) {
  throw "Zed source is missing; run client/fetch-zed.ps1 first"
}

Push-Location $zed
try {
  Write-Host "Installing the pinned Rust toolchain (1.98.1) and wasm targets"
  & rustup toolchain install 1.98.1 --profile minimal `
    --component rustfmt,clippy,rust-analyzer,rust-src `
    --target wasm32-wasip2,wasm32-unknown-unknown
  if ($LASTEXITCODE -ne 0) { throw "rustup failed with $LASTEXITCODE" }

  $args = @("build", "-p", $Package)
  if ($Release) { $args += "--release" }
  Write-Host "cargo $($args -join ' ')"
  & cargo @args
  if ($LASTEXITCODE -ne 0) { throw "cargo build failed with $LASTEXITCODE" }

  Write-Host "Build finished. Binary: target/debug/$Package.exe (or target/release/$Package.exe)"
} finally {
  Pop-Location
}
