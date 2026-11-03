#Requires -Version 5.1
<#
.SYNOPSIS
    Build sentinel-rs with shared portable GNU tools or an installed MSVC toolchain.
.PARAMETER ToolsRoot
    Shared dependency directory. Defaults to ../../Venv beside Projects.
.PARAMETER Toolchain
    Auto prefers the portable GNU toolchain when it is present.
.PARAMETER Test
    Run the offline release test suite instead of building.
#>
[CmdletBinding()]
param(
    [switch] $Test,
    [string] $ToolsRoot = $env:SENTINEL_TOOLS_ROOT,
    [ValidateSet('Auto', 'GNU', 'MSVC')]
    [string] $Toolchain = 'Auto',
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]] $CargoArgs
)

$ErrorActionPreference = 'Stop'
$Root = $PSScriptRoot
if (-not $ToolsRoot) {
    $ToolsRoot = Join-Path (Split-Path (Split-Path $Root -Parent) -Parent) 'Venv'
}
$ToolsRoot = [System.IO.Path]::GetFullPath($ToolsRoot)

function Require-File([string] $Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "Missing build dependency: $Path. See README.md."
    }
}

$RustRoot = Join-Path $ToolsRoot 'Rust'
if (-not $env:CARGO_HOME -and (Test-Path -LiteralPath "$RustRoot\cargo\bin\cargo.exe")) {
    $env:CARGO_HOME = Join-Path $RustRoot 'cargo'
}
if (-not $env:RUSTUP_HOME -and (Test-Path -LiteralPath "$RustRoot\rustup")) {
    $env:RUSTUP_HOME = Join-Path $RustRoot 'rustup'
}
if ($env:CARGO_HOME) {
    $env:PATH = "$(Join-Path $env:CARGO_HOME 'bin');$env:PATH"
}

$MingwBin = Join-Path $ToolsRoot 'BuildTools\mingw64\bin'
if ($Toolchain -eq 'Auto') {
    $Toolchain = if (Test-Path -LiteralPath "$MingwBin\gcc.exe") { 'GNU' } else { 'MSVC' }
}

$PortableScripts = Join-Path $ToolsRoot 'sentinel-build\Scripts'
if (Test-Path -LiteralPath "$PortableScripts\cmake.exe") {
    $env:PATH = "$PortableScripts;$env:PATH"
}
$NasmBin = Join-Path $ToolsRoot 'BuildTools\nasm-2.16.03'
if (Test-Path -LiteralPath "$NasmBin\nasm.exe") {
    $env:PATH = "$NasmBin;$env:PATH"
}

$RustToolchain = 'stable-x86_64-pc-windows-msvc'
if ($Toolchain -eq 'GNU') {
    Require-File "$MingwBin\gcc.exe"
    Require-File "$MingwBin\g++.exe"
    $env:PATH = "$MingwBin;$env:PATH"
    $env:CC = Join-Path $MingwBin 'gcc.exe'
    $env:CXX = Join-Path $MingwBin 'g++.exe'
    $env:AR = Join-Path $MingwBin 'ar.exe'
    $env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = $env:CC
    $env:CMAKE_GENERATOR = 'Ninja'
    # The PyPI libclang wheel supplies its DLL without C system headers.
    # Point bindgen at the same MinGW headers used by the native compiler.
    if (-not $env:BINDGEN_EXTRA_CLANG_ARGS_x86_64_pc_windows_gnu) {
        $GccInclude = & $env:CC '-print-file-name=include'
        if ($LASTEXITCODE -ne 0) { throw 'Could not locate GCC headers.' }
        $MingwInclude = Join-Path (Split-Path $MingwBin -Parent) 'x86_64-w64-mingw32\include'
        $env:BINDGEN_EXTRA_CLANG_ARGS_x86_64_pc_windows_gnu = "--target=x86_64-pc-windows-gnu -isystem `"$GccInclude`" -isystem `"$MingwInclude`""
    }
    $RustToolchain = 'stable-x86_64-pc-windows-gnu'
} else {
    $VcVars = $env:SENTINEL_VCVARS
    if (-not $VcVars) {
        $VsWhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
        if (Test-Path -LiteralPath $VsWhere) {
            $VsRoot = & $VsWhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
            if ($VsRoot) { $VcVars = Join-Path $VsRoot 'VC\Auxiliary\Build\vcvars64.bat' }
        }
    }
    if (-not $VcVars) { throw 'MSVC not found. Install the C++ build tools or configure SENTINEL_VCVARS.' }
    Require-File $VcVars
    $VcEnv = & cmd.exe /d /c "call `"$VcVars`" >nul 2>&1 && set"
    if ($LASTEXITCODE -ne 0) { throw "vcvars64.bat failed (exit $LASTEXITCODE)." }
    foreach ($Line in $VcEnv) {
        if ($Line -match '^([^=]+)=(.*)$') {
            [System.Environment]::SetEnvironmentVariable($matches[1], $matches[2], 'Process')
        }
    }
}

if (-not $env:LIBCLANG_PATH) {
    $Candidates = @(
        (Join-Path $ToolsRoot 'sentinel-build\Lib\site-packages\clang\native'),
        "$env:ProgramFiles\LLVM\bin"
    )
    $env:LIBCLANG_PATH = $Candidates | Where-Object { Test-Path -LiteralPath "$_\libclang.dll" } | Select-Object -First 1
}
if (-not $env:LIBCLANG_PATH) { throw 'libclang.dll not found. Configure LIBCLANG_PATH; see README.md.' }
Require-File (Join-Path $env:LIBCLANG_PATH 'libclang.dll')
foreach ($Command in @('cargo', 'cmake', 'ninja', 'nasm')) {
    if (-not (Get-Command $Command -ErrorAction SilentlyContinue)) {
        throw "Missing build command: $Command. See README.md."
    }
}

# Bound native/Rust compilation on machines with many logical cores.
if (-not $env:CARGO_BUILD_JOBS) { $env:CARGO_BUILD_JOBS = '4' }
$Extra = @($CargoArgs | Where-Object { $_ -ne '' })
$CargoCommand = if ($Test) { 'test' } else { 'build' }
$CargoOptions = @("+$RustToolchain", $CargoCommand, '--release', '--locked') + $Extra

if ($Toolchain -eq 'GNU') {
    # Cargo runs tests from deps and manages their DLL search path separately.
    foreach ($Directory in @('target\release', 'target\release\deps')) {
        $Destination = Join-Path $Root $Directory
        New-Item -ItemType Directory -Path $Destination -Force | Out-Null
        foreach ($Dll in @('libgcc_s_seh-1.dll', 'libstdc++-6.dll', 'libwinpthread-1.dll')) {
            Copy-Item -LiteralPath (Join-Path $MingwBin $Dll) -Destination $Destination -Force
        }
    }
}

Push-Location -LiteralPath $Root
try {
    Write-Host "[build] tools: $ToolsRoot ($Toolchain)"
    Write-Host "[build] cargo $($CargoOptions -join ' ')"
    & cargo @CargoOptions
    $Code = $LASTEXITCODE
} finally {
    Pop-Location
}
exit $Code
