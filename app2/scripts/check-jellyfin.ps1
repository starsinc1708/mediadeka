param(
    [Parameter(Mandatory)][string]$Jellyfin,
    [Parameter(Mandatory)][string]$Media,
    [string]$Executable = "$PSScriptRoot/../target/debug/mediadeka.exe",
    [string]$Evidence = "$PSScriptRoot/../target/acceptance/stage-2",
    [switch]$PlaylistRecovery,
    [switch]$AccountCollections,
    [switch]$LinkResolution,
    [switch]$Stage4,
    [switch]$Stage4Headless,
    [switch]$ConnectionRecovery,
    [string]$TestExecutable,
    [string]$Mpv = "$PSScriptRoot/../target/debug/libmpv-2.dll"
)

$ErrorActionPreference = 'Stop'
if ($Stage4Headless) { $Stage4 = $true }
$serverExe = (Resolve-Path -LiteralPath $Jellyfin).ProviderPath
$mediaPath = (Resolve-Path -LiteralPath $Media).ProviderPath
$ffmpeg = (Resolve-Path -LiteralPath (Join-Path (Split-Path $serverExe) 'ffmpeg.exe')).ProviderPath
$requiresPlaylist = $PlaylistRecovery -or $AccountCollections -or $Stage4
$headless = $requiresPlaylist -or $LinkResolution -or $ConnectionRecovery
if ($PlaylistRecovery -or $AccountCollections) {
    $mpvFile = (Resolve-Path -LiteralPath $Mpv).Path
}
if ($headless -and $TestExecutable) {
    $testExecutable = (Resolve-Path -LiteralPath $TestExecutable).ProviderPath
} elseif ($headless) {
    $cargo = Join-Path $env:USERPROFILE '.cargo/bin/cargo.exe'
    $records = & $cargo test --manifest-path "$PSScriptRoot/../Cargo.toml" --locked --offline --lib --features capture --no-run --message-format=json-render-diagnostics
    if ($LASTEXITCODE) { throw 'Could not build the Jellyfin test.' }
    $testExecutable = $records | ForEach-Object { ConvertFrom-Json -InputObject $_ } |
        Where-Object { $_.reason -eq 'compiler-artifact' -and $_.profile.test -and $_.executable } |
        Select-Object -Last 1 -ExpandProperty executable
}
$evidencePath = (New-Item -ItemType Directory -Path $Evidence -Force).FullName
$runtime = (New-Item -ItemType Directory -Path (Join-Path $evidencePath ('jellyfin-fixture-' + [guid]::NewGuid().ToString('N')))).FullName
$config = (New-Item -ItemType Directory -Path (Join-Path $runtime 'config')).FullName
$music = (New-Item -ItemType Directory -Path (Join-Path $runtime 'music')).FullName
$track = 'Mediadeka Jellyfin Acceptance'
$otherTrack = 'Mediadeka Jellyfin Second Track'
$user = 'mediadeka-fixture'
$password = [guid]::NewGuid().ToString('N')
$listener = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
$listener.Start()
$port = ([Net.IPEndPoint]$listener.LocalEndpoint).Port
$listener.Stop()
$url = "http://127.0.0.1:$port"
$identity = 'MediaBrowser Client="Mediadeka UIA", Device="Windows", DeviceId="mediadeka-uia-fixture", Version="1.0"'
$script:authorization = $identity

function Request($path, $method = 'Get', $body = $null) {
    $parameters = @{
        Uri = "$url/$path"
        Method = $method
        Headers = @{ Authorization = $script:authorization }
        TimeoutSec = 5
    }
    if ($null -ne $body) {
        $parameters.Body = ConvertTo-Json -InputObject $body -Depth 5 -Compress
        $parameters.ContentType = 'application/json'
    }
    Invoke-RestMethod @parameters
}

function Await([scriptblock]$probe, [string]$errorText, [int]$seconds) {
    $deadline = [DateTime]::UtcNow.AddSeconds($seconds)
    do {
        if (& $probe) { return }
        Start-Sleep -Milliseconds 200
    } until ([DateTime]::UtcNow -ge $deadline)
    throw $errorText
}

@"
<NetworkConfiguration>
  <InternalHttpPort>$port</InternalHttpPort>
  <EnableIPv6>false</EnableIPv6>
  <EnableRemoteAccess>false</EnableRemoteAccess>
  <EnableAutoDiscovery>false</EnableAutoDiscovery>
  <LocalNetworkAddresses><string>127.0.0.1</string></LocalNetworkAddresses>
</NetworkConfiguration>
"@ | Set-Content -LiteralPath (Join-Path $config 'network.xml') -Encoding utf8
$albumTitle = 'Mediadeka Acceptance Collection'
$album = if ($Stage4) { @('-metadata', "album=$albumTitle", '-metadata', 'album_artist=Mediadeka') } else { @() }
$trackDirectory = if ($Stage4) {
    (New-Item -ItemType Directory -Path (Join-Path $music "Mediadeka/$albumTitle") -Force).FullName
} else { $music }
& $ffmpeg -nostdin -v error -i $mediaPath -map '0:a:0' -vn -metadata "title=$track" -metadata 'artist=Mediadeka' @album (Join-Path $trackDirectory 'track.mp3')
if ($LASTEXITCODE -ne 0) { throw 'Could not prepare the fixture track.' }
if ($requiresPlaylist) {
    & $ffmpeg -nostdin -v error -i $mediaPath -map '0:a:0' -vn -metadata "title=$otherTrack" -metadata 'artist=Mediadeka' @album (Join-Path $trackDirectory 'second.mp3')
    if ($LASTEXITCODE -ne 0) { throw 'Could not prepare the second fixture track.' }
}

$arguments = @('--nowebclient', '--nonetchange', '--datadir', (Join-Path $runtime 'data'),
    '--configdir', $config, '--cachedir', (Join-Path $runtime 'cache'), '--logdir', (Join-Path $runtime 'logs'),
    '--ffmpeg', $ffmpeg) | ForEach-Object { '"' + $_ + '"' }
$serverOptions = @{
    FilePath = $serverExe
    ArgumentList = $arguments
    WorkingDirectory = $runtime
    WindowStyle = 'Hidden'
    PassThru = $true
    RedirectStandardOutput = Join-Path $runtime 'server.stdout.log'
    RedirectStandardError = Join-Path $runtime 'server.stderr.log'
}
$server = Start-Process @serverOptions
try {
    Await {
        if ($server.HasExited) { throw "Jellyfin exited with $($server.ExitCode)." }
        $connections = @(Get-NetTCPConnection -State Listen -OwningProcess $server.Id -ErrorAction SilentlyContinue)
        if ($connections | Where-Object { $_.LocalAddress -ne '127.0.0.1' -or $_.LocalPort -ne $port }) {
            throw 'Jellyfin bound outside the fixture loopback address.'
        }
        if (!$connections.Count) { return $false }
        try { Request 'Startup/User' } catch { $false }
    } 'Fixture Jellyfin server did not start.' 40

    $null = Request 'Startup/User' 'Post' @{ Name = $user; Password = $password }
    $null = Request 'Startup/RemoteAccess' 'Post' @{ EnableRemoteAccess = $false; EnableAutomaticPortMapping = $false }
    $null = Request 'Startup/Complete' 'Post'
    $login = Request 'Users/AuthenticateByName' 'Post' @{ Username = $user; Pw = $password }
    if (!$login.AccessToken -or !$login.User.Id) { throw 'Fixture account authentication failed.' }
    $script:authorization = '{0}, Token="{1}"' -f $identity, $login.AccessToken
    $null = Request "Library/VirtualFolders?name=Mediadeka&collectionType=music&paths=$([Uri]::EscapeDataString($music))&refreshLibrary=true" 'Post'
    $scan = [Diagnostics.Stopwatch]::StartNew()
    Await {
        $items = Request "Items?UserId=$($login.User.Id)&Recursive=true&IncludeItemTypes=Audio"
        $indexed = $items.Items | Where-Object Name -eq $track | Select-Object -First 1
        if ($indexed) { return $indexed }
        # Startup can already be scanning when the folder is added.
        if ($scan.Elapsed.TotalSeconds -ge 4) {
            $null = Request 'Library/Refresh' 'Post'
            $scan.Restart()
        }
        $false
    } 'Jellyfin did not index the fixture track.' 45
    $version = (Request 'System/Info/Public').Version
    if ($requiresPlaylist) {
        Await {
            $items = (Request "Items?UserId=$($login.User.Id)&Recursive=true&IncludeItemTypes=Audio").Items
            return ($items.Name -contains $otherTrack)
        } 'Jellyfin did not index the second fixture track.' 30
        $items = (Request "Items?UserId=$($login.User.Id)&Recursive=true&IncludeItemTypes=Audio").Items
        $first = ($items | Where-Object Name -eq $track | Select-Object -First 1).Id
        $second = ($items | Where-Object Name -eq $otherTrack | Select-Object -First 1).Id
        $playlist = Request 'Playlists' 'Post' @{
            Name = 'Jellyfin Playlist Recovery'
            Ids = @($first, $second, $first)
            UserId = $login.User.Id
            MediaType = 'Audio'
        }
        if ($AccountCollections -or $Stage4) {
            $null = Request "UserFavoriteItems/${first}?userId=$($login.User.Id)" 'Post'
        }
        if ($Stage4) {
            foreach ($kind in 'MusicAlbum', 'MusicArtist') {
                $name = if ($kind -eq 'MusicAlbum') { $albumTitle } else { 'Mediadeka' }
                $query = "Items?UserId=$($login.User.Id)&Recursive=true&IncludeItemTypes=$kind"
                Await { (Request $query).Items.Name -contains $name } "Jellyfin did not index the fixture $kind." 30
                $item = (Request $query).Items |
                    Where-Object Name -eq $name | Select-Object -First 1
                $null = Request "UserFavoriteItems/$($item.Id)?userId=$($login.User.Id)" 'Post'
            }
        }
    }
    if ($headless) {
        $test = Join-Path $runtime 'mediadeka-test.exe'
        $testMpv = Join-Path $runtime 'libmpv-2.dll'
        Copy-Item -LiteralPath $testExecutable -Destination $test
        if ($mpvFile) {
            Copy-Item -LiteralPath $mpvFile -Destination $testMpv
        }
        $testVariables = @{
            MEDIADEKA_PROFILE_DIR = Join-Path $runtime 'app2-profile'
            MEDIADEKA_TEST_JELLYFIN_URL = $url
            MEDIADEKA_TEST_JELLYFIN_USER = $user
            MEDIADEKA_TEST_JELLYFIN_USER_ID = $login.User.Id
            MEDIADEKA_TEST_JELLYFIN_PASSWORD = $password
            MEDIADEKA_TEST_JELLYFIN_AUTHORIZATION = $script:authorization
            MEDIADEKA_TEST_JELLYFIN_MEDIA = $mediaPath
            MEDIADEKA_TEST_JELLYFIN_EVIDENCE = $evidencePath
        }
        if ($requiresPlaylist) {
            $testVariables.MEDIADEKA_TEST_JELLYFIN_PLAYLIST = $playlist.Id
        }
        if ($LinkResolution -or $Stage4) {
            $items = (Request "Items?UserId=$($login.User.Id)&Recursive=true&IncludeItemTypes=Audio").Items
            $testVariables.MEDIADEKA_TEST_JELLYFIN_ITEM = ($items | Where-Object Name -eq $track | Select-Object -First 1).Id
        }
        if ($ConnectionRecovery) {
            $testVariables.MEDIADEKA_TEST_JELLYFIN_RECOVERY_PHASE = 'sign-in'
        }
        $previous = @{}
        try {
            foreach ($variable in $testVariables.Keys) {
                $previous[$variable] = [Environment]::GetEnvironmentVariable($variable)
                [Environment]::SetEnvironmentVariable($variable, $testVariables[$variable])
            }
            if ($ConnectionRecovery) {
                function Invoke-RecoveryPhase([string]$phase) {
                    $env:MEDIADEKA_TEST_JELLYFIN_RECOVERY_PHASE = $phase
                    $scenario = 'providers::jellyfin::recovery_tests::live_jellyfin_connections_recover_after_restart'
                    $result = & $test $scenario --exact --ignored --nocapture 2>&1
                    $testExit = $LASTEXITCODE
                    $result | Tee-Object -FilePath (Join-Path $evidencePath "jellyfin-connection-$phase.log")
                    if ($testExit -or $result -notcontains 'running 1 test') {
                        throw "Jellyfin connection recovery test failed or was missing ($phase)."
                    }
                }
                try {
                    Invoke-RecoveryPhase 'sign-in'
                    Invoke-RecoveryPhase 'online'
                    $server.Kill($true)
                    if (!$server.WaitForExit(5000)) { throw 'The fixture server did not shut down.' }
                    Invoke-RecoveryPhase 'offline'
                    $serverOptions.RedirectStandardOutput = Join-Path $runtime 'server-restart.stdout.log'
                    $serverOptions.RedirectStandardError = Join-Path $runtime 'server-restart.stderr.log'
                    $server = Start-Process @serverOptions
                    Await {
                        if ($server.HasExited) { throw "Jellyfin exited with $($server.ExitCode)." }
                        try { (Request 'Users/Me').Id -eq $login.User.Id } catch { $false }
                    } 'The fixture account was not ready after restart.' 40
                    Invoke-RecoveryPhase 'recovered'
                    Invoke-RecoveryPhase 'invalid'
                    Invoke-RecoveryPhase 'sign-in-again'
                } finally {
                    Invoke-RecoveryPhase 'cleanup'
                }
                return
            }
            $scenarios = if ($Stage4) {
                [ordered]@{
                    'music::likes::tests::live_jellyfin_and_mediadeka_favorites' = 'jellyfin-multi-like.log'
                    'music::library::tests::live_jellyfin_unified_library' = 'jellyfin-unified-library.log'
                    'providers::live_tests::collections::live_jellyfin_account_collections' = 'jellyfin-account-collections.log'
                }
            } elseif ($LinkResolution) {
                @{ 'music::links::jellyfin::tests::live_jellyfin_details_open_through_music_search' = 'jellyfin-link-resolution.log' }
            } elseif ($AccountCollections) {
                @{ 'providers::live_tests::collections::live_jellyfin_account_collections' = 'jellyfin-account-collections.log' }
            } else {
                @{ 'ui::playlist::loading::tests::jellyfin_refresh_reorders_and_offline_history_keeps_the_snapshot' = 'jellyfin-playlist-recovery.log' }
            }
            foreach ($scenario in $scenarios.Keys) {
                $log = $scenarios[$scenario]
                $result = & $test $scenario --exact --ignored --nocapture 2>&1
                $testExit = $LASTEXITCODE
                $result | Tee-Object -FilePath (Join-Path $evidencePath $log)
                if ($testExit -or $result -notcontains 'running 1 test') {
                    throw "Jellyfin test failed or was missing ($scenario)."
                }
            }
        } finally {
            foreach ($variable in $previous.Keys) {
                [Environment]::SetEnvironmentVariable($variable, $previous[$variable])
            }
            Remove-Item -LiteralPath $test,$testMpv -ErrorAction SilentlyContinue
        }
        if (!$Stage4 -or $Stage4Headless) { return }
    }
    $stopServer = {
        if (!$server.HasExited) {
            $server.Kill($true)
            if (!$server.WaitForExit(5000)) { throw 'The fixture server did not shut down.' }
        }
    }.GetNewClosure()

    & "$PSScriptRoot/check-welcome.ps1" -Executable $Executable -Evidence $evidencePath -JellyfinFixture @{
        Url = $url; User = $user; Password = $password; Track = $track
        Version = $version; AuthorizationHeader = $script:authorization; Stage4 = $Stage4.IsPresent
        Album = $albumTitle; Artist = 'Mediadeka'; Playlist = 'Jellyfin Playlist Recovery'
        StopServer = $stopServer
    }
} finally {
    if (!$server.HasExited) { $server.Kill($true); $server.WaitForExit() }
}
