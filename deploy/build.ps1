$ErrorActionPreference = 'Stop'
Push-Location (Join-Path $PSScriptRoot '..')
try {
    foreach ($tool in @('cargo', 'cmake', 'nasm', 'clang')) {
        if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) { throw "Missing tool: $tool. Use an MSVC developer shell with CMake, NASM and LLVM installed." }
    }
    if (-not $env:LIBCLANG_PATH) { $env:LIBCLANG_PATH = Split-Path (Get-Command clang).Source }
    if (-not $env:CARGO_BUILD_JOBS) { $env:CARGO_BUILD_JOBS = '1' }
    cargo build --release --locked
    if ($LASTEXITCODE -ne 0) { throw 'Cargo build failed' }
    cargo test --release --locked
    if ($LASTEXITCODE -ne 0) { throw 'Cargo tests failed' }
} finally { Pop-Location }
