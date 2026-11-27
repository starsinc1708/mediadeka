#requires -Version 7.2
[CmdletBinding()]
param(
    [string]$OutputDirectory = (Join-Path $PSScriptRoot '../dist'),
    [switch]$RequireClean
)

$ErrorActionPreference = 'Stop'
if (!$IsWindows) { throw 'The Windows x64 package must be built on Windows.' }

$repository = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$output = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($OutputDirectory)
$target = 'x86_64-pc-windows-msvc'
$revision = & git -C $repository rev-parse --verify HEAD
if ($LASTEXITCODE -ne 0) { throw 'A source commit is required to identify the package.' }
$changes = & git -C $repository status --porcelain --untracked-files=normal
if ($LASTEXITCODE -ne 0) { throw 'Could not inspect the source tree.' }
$dirty = @($changes).Count -gt 0
if ($RequireClean -and $dirty) { throw 'Commit the source changes before packaging.' }

Push-Location (Join-Path $repository 'app2')
try {
    $metadataJson = & cargo metadata --locked --format-version 1 --no-deps
    if ($LASTEXITCODE -ne 0) { throw 'Could not read Cargo metadata.' }
    $metadata = ($metadataJson -join "`n") | ConvertFrom-Json
    $version = ($metadata.packages | Where-Object name -eq 'mediadeka').version
    if (!$version) { throw 'Mediadeka version missing from Cargo metadata.' }
    $compiler = & rustc --version
    if ($LASTEXITCODE -ne 0) { throw 'Could not identify rustc.' }
    & cargo build --release --locked --target $target --features fluid-glass-default
    if ($LASTEXITCODE -ne 0) { throw 'Mediadeka build failed.' }
} finally {
    Pop-Location
}

$sourceExecutable = Join-Path $metadata.target_directory "$target/release/mediadeka.exe"
if (!(Test-Path -LiteralPath $sourceExecutable -PathType Leaf)) { throw 'Build did not produce mediadeka.exe.' }

$name = "Mediadeka-$version-preview-windows-x64"
$package = (New-Item -ItemType Directory -Force -Path (Join-Path $output $name)).FullName
$libmpv = & (Join-Path $PSScriptRoot 'Get-Libmpv.ps1') -OutputDirectory $package
New-Item -ItemType Directory -Force -Path (Join-Path $package 'licenses') | Out-Null
$files = [ordered]@{
    'mediadeka.exe' = $sourceExecutable
    'README.md' = Join-Path $repository 'README.md'
}
$rootLicense = Join-Path $repository 'LICENSE'
if (Test-Path -LiteralPath $rootLicense -PathType Leaf) { $files.Add('LICENSE', $rootLicense) }
foreach ($entry in $files.GetEnumerator()) {
    Copy-Item -LiteralPath $entry.Value -Destination (Join-Path $package $entry.Key) -Force
}

$exeHash = (Get-FileHash -LiteralPath (Join-Path $package 'mediadeka.exe')).Hash.ToLowerInvariant()
$buildInfo = Join-Path $package 'build-info.json'
[ordered]@{
    version = $version
    target = $target
    profile = 'release'
    features = @('fluid-glass-default')
    git_commit = $revision
    working_tree_dirty = $dirty
    rustc = $compiler
    cargo_lock_sha256 = (Get-FileHash -LiteralPath (Join-Path $repository 'app2/Cargo.lock')).Hash.ToLowerInvariant()
    executable_sha256 = $exeHash
    includes_libmpv = $true
    libmpv_sha256 = $libmpv.Sha256
    libmpv_source_url = $libmpv.SourceUrl
} | ConvertTo-Json | Set-Content -LiteralPath $buildInfo -Encoding utf8

$zipPath = Join-Path $output "$name.zip"
$temporary = "$zipPath.$([Guid]::NewGuid()).tmp"
try {
    $archive = [IO.Compression.ZipFile]::Open($temporary, [IO.Compression.ZipArchiveMode]::Create)
    try {
        foreach ($entry in @($files.Keys) + @('libmpv-2.dll', 'build-info.json')) {
            [void][IO.Compression.ZipFileExtensions]::CreateEntryFromFile(
                $archive, (Join-Path $package $entry), $entry, [IO.Compression.CompressionLevel]::Optimal
            )
        }
    } finally {
        $archive.Dispose()
    }
    Move-Item -LiteralPath $temporary -Destination $zipPath -Force
} finally {
    if (Test-Path -LiteralPath $temporary -PathType Leaf) { Remove-Item -LiteralPath $temporary }
}

$archiveHash = (Get-FileHash -LiteralPath $zipPath).Hash.ToLowerInvariant()
@("$archiveHash *$name.zip", "$exeHash *$name/mediadeka.exe") |
    Set-Content -LiteralPath (Join-Path $output 'SHA256SUMS.txt') -Encoding ascii
Write-Output $zipPath
