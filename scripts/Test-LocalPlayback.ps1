#requires -Version 7.2
[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$LibmpvPath,
    [string]$OutputDirectory = (Join-Path $PSScriptRoot '../dist/acceptance/local-playback')
)

$ErrorActionPreference = 'Stop'
if (!$IsWindows) { throw 'The native playback check must run on Windows.' }

$dll = (Resolve-Path -LiteralPath $LibmpvPath).Path
if (!(Test-Path -LiteralPath $dll -PathType Leaf)) { throw 'Provide the path to libmpv-2.dll.' }
$output = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($OutputDirectory)
$app = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '../app2')).Path

Push-Location $app
try {
    $metadataJson = & cargo metadata --locked --format-version 1 --no-deps
    if ($LASTEXITCODE -ne 0) { throw 'Could not read Cargo metadata.' }
    $metadata = ($metadataJson -join "`n") | ConvertFrom-Json
    & cargo build --locked --features capture --example capture-window
    if ($LASTEXITCODE -ne 0) { throw 'Could not build capture-window.' }
} finally {
    Pop-Location
}

$examples = Join-Path $metadata.target_directory 'debug/examples'
$runtime = Join-Path $examples 'libmpv-2.dll'
if ($dll -ne $runtime) { Copy-Item -LiteralPath $dll -Destination $runtime -Force }

$previousGui = $env:MEDIADEKA_GUI
try {
    $env:MEDIADEKA_GUI = '3'
    & (Join-Path $examples 'capture-window.exe') --fluid-glass $output --player-layout
    if ($LASTEXITCODE -ne 0) { throw 'The native playback check failed.' }
} finally {
    $env:MEDIADEKA_GUI = $previousGui
}
Write-Output $output
