param(
    [Parameter(Mandatory)][string]$TestBinary,
    [string]$EvidenceDirectory = (Join-Path $PSScriptRoot '../target/acceptance/aggregator-5/race-fixture')
)

$ErrorActionPreference = 'Stop'
$testExecutable = (Resolve-Path -LiteralPath $TestBinary).Path
$testLibrary = Join-Path (Split-Path -Parent $testExecutable) 'libmpv-2.dll'
if (!(Test-Path -LiteralPath $testLibrary -PathType Leaf)) {
    throw "Place libmpv-2.dll beside the test executable: $testLibrary"
}
$case = 'windows_stage5_two_originals_start_then_improve_and_replay_offline'
if (-not (& $testExecutable $case --list | Select-String -SimpleMatch ('::' + $case + ': test'))) {
    throw 'The test executable does not contain the playback race. Rebuild it with cargo test --no-run.'
}
$evidence = (New-Item -ItemType Directory -Path $EvidenceDirectory -Force).FullName
$better = Join-Path $evidence 'better.flac'
$first = Join-Path $evidence 'first.mp3'
& ffmpeg -nostdin -v error -y -f lavfi -i 'anoisesrc=color=white:sample_rate=48000:amplitude=0.15:duration=14:seed=883' -sample_fmt s16 -c:a flac $better
if ($LASTEXITCODE) { throw 'Could not generate the lossless acceptance recording.' }
& ffmpeg -nostdin -v error -y -i $better -c:a libmp3lame -b:a 96k $first
if ($LASTEXITCODE) { throw 'Could not generate the MP3 from the same recording.' }

$oldFixtures = $env:MEDIADEKA_STAGE5_DOWNLOAD_RACE_FIXTURES
try {
    $env:MEDIADEKA_STAGE5_DOWNLOAD_RACE_FIXTURES = $evidence
    & $testExecutable $case --ignored --nocapture 2>&1 |
        Tee-Object -FilePath (Join-Path $evidence 'wasapi-race.log')
    if ($LASTEXITCODE) { throw 'The playback race failed.' }
} finally {
    $env:MEDIADEKA_STAGE5_DOWNLOAD_RACE_FIXTURES = $oldFixtures
}
