param([switch]$Check)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $projectRoot
if (Test-Path -LiteralPath .tools/cargo/bin/cargo.exe) {
    $env:CARGO_HOME = Join-Path $projectRoot '.tools/cargo'
    $env:RUSTUP_HOME = Join-Path $projectRoot '.tools/rustup'
    $cargo = Join-Path $projectRoot '.tools/cargo/bin/cargo.exe'
} else {
    $cargo = (Get-Command cargo -ErrorAction SilentlyContinue).Source
    if (-not $cargo) { throw 'Run scripts/bootstrap-rust.ps1 first, or install Rust yourself.' }
}

if ($Check) {
    & $cargo fmt -- --check
    if ($LASTEXITCODE -ne 0) { throw 'Formatting check failed.' }
    & $cargo clippy --locked --all-targets -- -D warnings
    if ($LASTEXITCODE -ne 0) { throw 'Clippy failed.' }
    & $cargo test --locked
    if ($LASTEXITCODE -ne 0) { throw 'Tests failed.' }
}
& $cargo build --release --locked
if ($LASTEXITCODE -ne 0) { throw 'Release build failed.' }
New-Item -ItemType Directory -Force dist | Out-Null
Copy-Item -LiteralPath target/release/wifiradio.exe -Destination dist/WifiRadio.exe
Copy-Item -LiteralPath README.md -Destination dist/README.md
Copy-Item -LiteralPath LICENSE -Destination dist/LICENSE
$hash = (Get-FileHash -Algorithm SHA256 -LiteralPath dist/WifiRadio.exe).Hash.ToLowerInvariant()
[System.IO.File]::WriteAllText((Join-Path $projectRoot 'dist/WifiRadio.exe.sha256'), "$hash  WifiRadio.exe`r`n")
Write-Output 'Ready: dist/WifiRadio.exe'
