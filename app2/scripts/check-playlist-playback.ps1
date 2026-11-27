[CmdletBinding(DefaultParameterSetName = 'Local')]
param(
    [string]$Mpv = (Join-Path $PSScriptRoot '../target/debug/libmpv-2.dll'),
    [Parameter(ParameterSetName = 'Local')]
    [string]$Fixture,
    [string]$Output = (Join-Path $PSScriptRoot '../target/acceptance/playlist-page/audio'),
    [Parameter(Mandatory, ParameterSetName = 'Provider')]
    [ValidateSet('soundcloud', 'yandex-music', 'jellyfin')]
    [string]$Provider,
    [Parameter(Mandatory, ParameterSetName = 'Provider')]
    [string]$Playlist,
    [Parameter(ParameterSetName = 'Provider')]
    [string]$Track,
    [Parameter(Mandatory, ParameterSetName = 'Provider')]
    [string]$IsolatedProfile
)

$ErrorActionPreference = 'Stop'
$cargo = (Get-Command cargo -ErrorAction SilentlyContinue).Source
if (!$cargo) { $cargo = Join-Path $env:USERPROFILE '.cargo/bin/cargo.exe' }
$mpvFile = (Resolve-Path -LiteralPath $Mpv).Path
$report = [System.IO.Directory]::CreateDirectory([System.IO.Path]::GetFullPath($Output)).FullName
$manifest = Join-Path $PSScriptRoot '../Cargo.toml'

function New-ToneFixture {
    param(
        [Parameter(Mandatory)][string]$Path,
        [double]$Seconds = 6,
        [double]$Frequency = 440,
        [int]$SampleRate = 44100
    )
    $samples = [int]($SampleRate * $Seconds)
    $dataSize = $samples * 2
    $stream = [System.IO.File]::Create($Path)
    $writer = [System.IO.BinaryWriter]::new($stream)
    try {
        $writer.Write([System.Text.Encoding]::ASCII.GetBytes('RIFF'))
        $writer.Write([int](36 + $dataSize))
        $writer.Write([System.Text.Encoding]::ASCII.GetBytes('WAVEfmt '))
        $writer.Write([int]16)
        $writer.Write([int16]1)
        $writer.Write([int16]1)
        $writer.Write([int]$SampleRate)
        $writer.Write([int]($SampleRate * 2))
        $writer.Write([int16]2)
        $writer.Write([int16]16)
        $writer.Write([System.Text.Encoding]::ASCII.GetBytes('data'))
        $writer.Write([int]$dataSize)
        for ($i = 0; $i -lt $samples; $i++) {
            $sample = [int16]([math]::Round(0.25 * 32767 * [math]::Sin(2 * [math]::PI * $Frequency * $i / $SampleRate)))
            $writer.Write($sample)
        }
    } finally {
        $writer.Dispose()
        $stream.Dispose()
    }
}

$records = & $cargo test --locked --lib --no-run --manifest-path $manifest --message-format=json
if ($LASTEXITCODE) { throw 'Не удалось собрать тест воспроизведения.' }
$executable = $records | ForEach-Object { ConvertFrom-Json -InputObject $_ } |
    Where-Object { $_.reason -eq 'compiler-artifact' -and $_.profile.test -and $_.executable } |
    Select-Object -Last 1 -ExpandProperty executable
if (!$executable) { throw 'Cargo не вернул путь к тестовому EXE.' }

$test = Join-Path $report 'playlist-audio-test.exe'
Copy-Item -LiteralPath $executable -Destination $test -Force
Copy-Item -LiteralPath $mpvFile -Destination (Join-Path $report 'libmpv-2.dll') -Force
$previous = @{}
$testVariables = if ($PSCmdlet.ParameterSetName -eq 'Provider') {
    @{
        MEDIADEKA_PROFILE_DIR = (Resolve-Path -LiteralPath $IsolatedProfile).Path
        MEDIADEKA_TEST_AUDIO_PROVIDER = $Provider
        MEDIADEKA_TEST_AUDIO_PLAYLIST = $Playlist
        MEDIADEKA_TEST_AUDIO_TRACK = $Track
    }
} else {
    if (!$Fixture) {
        $Fixture = Join-Path $report 'tone-440.wav'
        New-ToneFixture -Path $Fixture
    }
    @{ MEDIADEKA_AUDIO_FIXTURE = (Resolve-Path -LiteralPath $Fixture).Path }
}
$scenario = if ($PSCmdlet.ParameterSetName -eq 'Provider') {
    'windows_provider_playlist_audio'
} else {
    'windows_playlist_audio_and_queue'
}
$log = if ($Provider) { "wasapi-$Provider.log" } else { 'wasapi.log' }
try {
    foreach ($variable in $testVariables.Keys) {
        $previous[$variable] = [System.Environment]::GetEnvironmentVariable($variable)
        [System.Environment]::SetEnvironmentVariable($variable, $testVariables[$variable])
    }
    & $test "playback::tests::windows::$scenario" --ignored --nocapture 2>&1 |
        Tee-Object -FilePath (Join-Path $report $log)
    if ($LASTEXITCODE) { throw 'Проверка libmpv и WASAPI завершилась ошибкой.' }
} finally {
    foreach ($variable in $previous.Keys) {
        [System.Environment]::SetEnvironmentVariable($variable, $previous[$variable])
    }
}
