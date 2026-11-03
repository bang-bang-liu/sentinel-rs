#Requires -Version 5.1
[CmdletBinding()]
param(
    [Parameter(Position = 0)]
    [ValidateRange(1, 2147483647)]
    [int] $Count = 1,
    [Parameter(Position = 1)]
    [string] $Proxy,
    [ValidateRange(1, 2147483647)]
    [int] $Concurrency = 1,
    [switch] $DryRun
)

$ErrorActionPreference = 'Stop'
$Exe = Join-Path $PSScriptRoot 'target\release\sentinel-rs.exe'
if (-not (Test-Path -LiteralPath $Exe -PathType Leaf)) {
    throw 'sentinel-rs.exe is missing. Run .\build.ps1 first.'
}

# The executable loads .env from its working directory and validates live settings.
# Leave --proxy absent so SENTINEL_PROXY keeps its normal precedence.
$CliArgs = @('register', '-n', "$Count", '--concurrency', "$Concurrency", '--2fa')
if ($DryRun) { $CliArgs += '--dry-run' } else { $CliArgs += '--live' }
if ($PSBoundParameters.ContainsKey('Proxy')) {
    if ([string]::IsNullOrWhiteSpace($Proxy)) { throw 'An explicit Proxy must not be empty.' }
    $CliArgs += @('--proxy', $Proxy)
}

Push-Location -LiteralPath $PSScriptRoot
try {
    & $Exe @CliArgs
    $Code = $LASTEXITCODE
} finally {
    Pop-Location
}
exit $Code
