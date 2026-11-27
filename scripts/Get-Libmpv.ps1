#requires -Version 7.2
[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$OutputDirectory
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
if (!$IsWindows) { throw 'This libmpv build is for Windows x64.' }

$url = 'https://github.com/zhongfly/mpv-winbuild/releases/download/2026-11-19-b7339522cd/mpv-dev-lgpl-x86_64-20261119-git-b7339522cd.7z'
$archiveHash = 'c48d6d4c689669de054a8c1be9b795d951aa14e45cb0330b1c9146e541ced5e8'
$dllHash = '88c13c8a806195706918fc1c5ecfbcd466e3590a527f567bf479ccd081903dd0'
$destination = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($OutputDirectory)
$archive = [IO.Path]::GetTempFileName()
try {
    Invoke-WebRequest -Uri $url -OutFile $archive -MaximumRetryCount 2 -TimeoutSec 120
    if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash -ne $archiveHash) {
        throw 'The downloaded libmpv archive does not match its pinned SHA-256.'
    }

    New-Item -ItemType Directory -Force -Path $destination | Out-Null
    & tar.exe -xf $archive -C $destination 'libmpv-2.dll'
    if ($LASTEXITCODE -ne 0) { throw 'Could not extract libmpv-2.dll from the verified archive.' }

    $dll = Join-Path $destination 'libmpv-2.dll'
    if ((Get-FileHash -LiteralPath $dll -Algorithm SHA256).Hash -ne $dllHash) {
        throw 'The extracted libmpv-2.dll does not match its pinned SHA-256.'
    }
} finally {
    Remove-Item -LiteralPath $archive -Force
}

[pscustomobject]@{
    Path = $dll
    Sha256 = $dllHash
    SourceUrl = $url
}
