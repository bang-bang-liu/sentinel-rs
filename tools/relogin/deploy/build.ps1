[CmdletBinding()]
param([switch] $Test)
$ErrorActionPreference = 'Stop'
Push-Location (Join-Path $PSScriptRoot '..')
try {
    foreach ($tool in @('cargo', 'cmake', 'nasm', 'clang')) {
        if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
            throw "Missing tool: $tool. Use a Rust/MSVC developer shell with CMake, NASM and LLVM configured."
        }
    }
    if (-not $env:LIBCLANG_PATH) { $env:LIBCLANG_PATH = Split-Path (Get-Command clang).Source }
    if (-not $env:CARGO_BUILD_JOBS) { $env:CARGO_BUILD_JOBS = '1' }
    if ($Test) { cargo test --release --locked }
    else { cargo build --release --locked -p sentinel-relogin }
    if ($LASTEXITCODE -ne 0) { throw 'sentinel-relogin Cargo command failed' }
} finally { Pop-Location }
