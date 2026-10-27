$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $projectRoot
New-Item -ItemType Directory -Force .tools | Out-Null

# All tools and their caches stay inside the checkout. Do not change user PATH.
$env:CARGO_HOME = Join-Path $projectRoot '.tools/cargo'
$env:RUSTUP_HOME = Join-Path $projectRoot '.tools/rustup'
$env:RUSTUP_USE_CURL = '1'
$env:RUSTUP_INIT_SKIP_PATH_CHECK = 'yes'
$installer = Join-Path $projectRoot '.tools/rustup-init.exe'
& curl.exe --fail --location --silent --show-error https://win.rustup.rs/x86_64 -o $installer
if ($LASTEXITCODE -ne 0) { throw 'Could not download the official Rust installer.' }
& $installer -y --no-modify-path --profile minimal --default-host x86_64-pc-windows-gnu --default-toolchain stable
if ($LASTEXITCODE -ne 0) { throw 'Local Rust setup failed.' }
& .tools/cargo/bin/rustup.exe component add rustfmt clippy
if ($LASTEXITCODE -ne 0) { throw 'Could not install local Rust checks.' }
