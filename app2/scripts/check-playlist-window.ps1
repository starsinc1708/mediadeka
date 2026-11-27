param(
    [string]$Executable = "$PSScriptRoot/../target/acceptance/playlist-page/package/mediadeka.exe",
    [string]$Capture = "$PSScriptRoot/../target/debug/examples/capture-window.exe",
    [string]$Wave = "$PSScriptRoot/../target/acceptance/playlist-page/fixtures/Awake.wav",
    [string]$Evidence = "$PSScriptRoot/../target/acceptance/playlist-page",
    [string]$SpotifyDatabase
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class PlaylistWindow {
    private delegate bool WindowVisitor(IntPtr window, IntPtr state);
    [DllImport("user32.dll")] private static extern bool EnumWindows(WindowVisitor visitor, IntPtr state);
    [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr window, out uint processId);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern int GetWindowText(IntPtr window, StringBuilder title, int length);
    [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr window);
    [DllImport("user32.dll")] public static extern bool ShowWindowAsync(IntPtr window, int command);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr window, uint message, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll")] private static extern bool SetWindowPos(IntPtr window, IntPtr after, int x, int y, int width, int height, uint flags);
    public static void RenderOffscreen(IntPtr window, int width, int height) {
        SetWindowPos(window, IntPtr.Zero, -24000, -24000, width, height, 0x14);
        ShowWindowAsync(window, 4);
    }
    public static IntPtr Find(uint processId) {
        var result = IntPtr.Zero;
        EnumWindows((window, _) => {
            GetWindowThreadProcessId(window, out uint owner);
            if (owner != processId) return true;
            var title = new StringBuilder(48);
            GetWindowText(window, title, title.Capacity);
            if (title.ToString() != "Mediadeka") return true;
            result = window;
            return false;
        }, IntPtr.Zero);
        return result;
    }
}
'@

$automation = [System.Windows.Automation.AutomationElement]
$scope = [System.Windows.Automation.TreeScope]::Descendants
$invokePattern = [System.Windows.Automation.InvokePattern]::Pattern
$valuePattern = [System.Windows.Automation.ValuePattern]::Pattern
$rangePattern = [System.Windows.Automation.RangeValuePattern]::Pattern

function Wait-Result([scriptblock]$Probe, [string]$Failure, [int]$Seconds = 12) {
    $timer = [System.Diagnostics.Stopwatch]::StartNew()
    do {
        try {
            $value = & $Probe
            if ($null -ne $value -and $value -ne $false) { return $value }
        } catch [System.Windows.Automation.ElementNotAvailableException] {}
        Start-Sleep -Milliseconds 80
    } while ($timer.Elapsed.TotalSeconds -lt $Seconds)
    throw $Failure
}
function Find-Node([string]$Id) {
    $condition = [System.Windows.Automation.PropertyCondition]::new($automation::AutomationIdProperty, $Id)
    $root.FindFirst($scope, $condition)
}
function Need-Node([string]$Id) {
    Wait-Result { Find-Node $Id } "Missing UIA control $Id."
}
function Find-Named([string]$Name) {
    $condition = [System.Windows.Automation.PropertyCondition]::new($automation::NameProperty, $Name)
    $root.FindFirst($scope, $condition)
}
function Press-Node([string]$Id) {
    $element = Need-Node $Id
    $pattern = $null
    if ($element.TryGetCurrentPattern($invokePattern, [ref]$pattern)) {
        $pattern.Invoke()
    } elseif ($element.TryGetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern, [ref]$pattern)) {
        $pattern.Toggle()
    } else {
        throw "Could not activate $Id."
    }
}
function Rect-Of($element) {
    $bounds = $element.Current.BoundingRectangle
    @($bounds.X, $bounds.Y, $bounds.Width, $bounds.Height)
}
function Press-Key([int]$key) {
    if (![PlaylistWindow]::PostMessage($window, 0x0100, [IntPtr]$key, [IntPtr]1)) {
        throw "Failed to dispatch key $key to Mediadeka."
    }
    if (![PlaylistWindow]::PostMessage($window, 0x0101, [IntPtr]$key, [IntPtr]::new(-1073741823))) {
        throw "Failed to release key $key in Mediadeka."
    }
}
function Save-Tree([string]$File) {
    $tree = foreach ($node in $root.FindAll($scope, [System.Windows.Automation.Condition]::TrueCondition)) {
        try {
            [ordered]@{
                id = $node.Current.AutomationId
                name = $node.Current.Name
                type = $node.Current.ControlType.ProgrammaticName
                bounds = @(Rect-Of $node)
                enabled = $node.Current.IsEnabled
                focus = $node.Current.HasKeyboardFocus
                help = $node.Current.HelpText
            }
        } catch [System.Windows.Automation.ElementNotAvailableException] {}
    }
    $tree | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $Evidence $File) -Encoding utf8
    @($tree)
}

function Close-Probe($App, [IntPtr]$Handle) {
    if ($App -and !$App.HasExited) {
        if ($Handle) { [void][PlaylistWindow]::PostMessage($Handle, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) }
        if (!$App.WaitForExit(4000)) { Stop-Process -Id $App.Id }
    }
}

function Wait-ProbeWindow($App) {
    Wait-Result {
        $handle = [PlaylistWindow]::Find($App.Id)
        if ($handle -ne [IntPtr]::Zero) { $handle }
    } 'Native main window did not open.'
}

$profile = Join-Path ([IO.Path]::GetTempPath()) "mediadeka-playlist-uia-$([Guid]::NewGuid().ToString('N'))"
$null = New-Item -ItemType Directory -Path $profile
$previousProfile = $env:MEDIADEKA_PROFILE_DIR
$process = $null
$window = [IntPtr]::Zero
try {
    $fixtureId = & $Capture --seed-playlist $profile $Wave
    if ($LASTEXITCODE -ne 0 -or $fixtureId -notmatch '^\d+$') { throw 'Unable to seed isolated profile.' }
    $env:MEDIADEKA_PROFILE_DIR = $profile
    $process = Start-Process -FilePath $Executable -PassThru -WindowStyle Hidden
    $window = Wait-ProbeWindow $process
    [PlaylistWindow]::RenderOffscreen($window, 1500, 850)
    $root = $automation::FromHandle($window)
    $null = Need-Node 'home-playlists'
    Press-Node 'home-playlists'
    Press-Node "local-playlist-$fixtureId"
    $title = Need-Node 'playlist-title'
    if (!$title.Current.Name.StartsWith('Ночной кодинг')) { throw "Unexpected playlist $($title.Current.Name)." }

    $navigation = @(Rect-Of (Need-Node 'home-navigation'))
    $toolbar = @(Rect-Of (Need-Node 'workspace-toolbar'))
    $viewport = @(Rect-Of (Need-Node 'playlist-viewport'))
    $player = @(Rect-Of (Need-Node 'home-transport'))
    $artwork = @(Rect-Of (Need-Node 'playlist-hero-artwork'))
    $search = Need-Node 'playlist-search'
    $search.GetCurrentPattern($valuePattern).SetValue('Нет такого трека')
    $null = Need-Node 'playlist-empty'
    $search.GetCurrentPattern($valuePattern).SetValue('')
    $null = Need-Node 'playlist-row-0'
    $firstViewportTree = @(Save-Tree 'uia-playlist.json')
    $visibleRows = @($firstViewportTree | Where-Object { $_.id -match '^playlist-row-\d+$' }).Count
    if ($visibleRows -lt 1 -or $visibleRows -gt 40) { throw "Expected a virtualized tree, got $visibleRows rows." }

    Press-Node 'playlist-more'
    $queueAction = Wait-Result { Find-Named 'Добавить в конец очереди' } 'Playlist queue action is missing.'
    $queueAction.GetCurrentPattern($invokePattern).Invoke()
    $null = Wait-Result { !(Find-Node 'playlist-menu') } 'Playlist queue action did not close the menu.'
    Press-Node 'player-play'
    $null = Wait-Result {
        $play = Find-Node 'player-play'
        if ($play -and $play.Current.Name -eq 'Пауза') { $play }
    } 'Engine did not confirm Playing.'
    $progress = Need-Node 'player-progress'
    $null = Wait-Result { $progress.GetCurrentPattern($rangePattern).Current.Value -gt 0 } 'Playback position did not advance.'
    $initial = $progress.GetCurrentPattern($rangePattern).Current.Value
    Press-Node 'player-queue'
    $null = Need-Node 'right-panel'
    $queueTree = @(Save-Tree 'uia-queue.json')
    if (!($queueTree | Where-Object { $_.id -match 'player-queue-entry' })) { throw 'Queue has no exposed entries.' }
    Press-Key 0x1B
    Press-Node 'workspace-back'
    $null = Need-Node "local-playlist-$fixtureId"
    $null = Wait-Result { (Need-Node 'player-progress').GetCurrentPattern($rangePattern).Current.Value -gt $initial } 'Position did not continue after Back.'
    Press-Node 'workspace-forward'
    $null = Need-Node 'playlist-row-0'
    $afterReturn = (Need-Node 'player-progress').GetCurrentPattern($rangePattern).Current.Value
    Press-Node 'player-play'
    $null = Wait-Result { (Need-Node 'player-play').Current.Name -eq 'Воспроизвести' } 'Engine did not confirm Pause.'
    Press-Node 'playlist-play'
    $null = Wait-Result { (Need-Node 'player-play').Current.Name -eq 'Пауза' } 'Playlist Play did not resume the active track.'
    Press-Node 'player-play'
    $null = Wait-Result { (Need-Node 'player-play').Current.Name -eq 'Воспроизвести' } 'Engine did not confirm Pause after Playlist Play.'
    $null = Wait-Result {
        (Need-Node 'player-favorite').Current.Name -eq 'Добавить текущий трек в избранное'
    } 'Player did not load the favorite state for its current track.'
    Press-Node 'player-favorite'
    $null = Wait-Result {
        (Need-Node 'player-favorite').Current.Name -eq 'Убрать текущий трек из избранного'
    } 'The player favorite action did not update the saved mark.'
    Press-Node 'player-favorite'
    $null = Wait-Result {
        (Need-Node 'player-favorite').Current.Name -eq 'Добавить текущий трек в избранное'
    } 'The player could not remove the saved favorite mark.'

    $menuButton = Need-Node 'playlist-more'
    $menuButton.SetFocus()
    Press-Node 'playlist-more'
    $null = Need-Node 'playlist-menu'
    $menuTree = @(Save-Tree 'uia-menu.json')
    if ($menuTree.Name -notcontains 'Переименовать плейлист' -or $menuTree.Name -notcontains 'Удалить плейлист') {
        throw 'Local playlist management actions are missing.'
    }
    if ($menuTree.Name -contains 'Сделать приватным') { throw 'Unexpected remote privacy action on a local playlist.' }
    Press-Key 0x28
    Press-Key 0x1B
    $null = Wait-Result { !(Find-Node 'playlist-menu') } 'Escape did not dismiss the playlist menu.'
    $null = Wait-Result { (Need-Node 'playlist-more').Current.HasKeyboardFocus } 'Focus did not return to the menu button.'

    $scrollbar = Need-Node 'playlist-scrollbar'
    $startRange = $scrollbar.GetCurrentPattern($rangePattern).Current.Value
    $scrollbar.SetFocus()
    Press-Key 0x23
    $null = Need-Node 'playlist-row-999'
    $endRange = Wait-Result {
        $current = (Need-Node 'playlist-scrollbar').GetCurrentPattern($rangePattern).Current
        if ($current.Value -gt $startRange -and $current.Value -le $current.Maximum) { $current.Value }
    } 'The accessible scrollbar value did not increase after End.'
    $tree = @(Save-Tree 'uia-scrolled.json')
    $bottomVisibleRows = @($tree | Where-Object { $_.id -match '^playlist-row-\d+$' }).Count
    $navigationAfter = @(Rect-Of (Need-Node 'home-navigation'))
    $playerAfter = @(Rect-Of (Need-Node 'home-transport'))
    $viewportAfter = @(Rect-Of (Need-Node 'playlist-viewport'))
    if (($navigationAfter -join ',') -ne ($navigation -join ',') -or
        ($playerAfter -join ',') -ne ($player -join ',') -or
        ($viewportAfter -join ',') -ne ($viewport -join ',')) {
        throw 'Sidebar, viewport or player changed bounds when the table scrolled.'
    }
    if (($player[0] -lt ($navigation[0] + $navigation[2])) -or ($viewport[1] -lt ($toolbar[1] + $toolbar[3]))) {
        throw 'The player or viewport overlaps an adjacent layout region.'
    }
    if ([Math]::Abs($player[0] - $artwork[0]) -gt 1) {
        throw 'The main content and player are not aligned.'
    }

    $scrollbar.SetFocus()
    Press-Key 0x24
    $null = Wait-Result {
        (Need-Node 'playlist-scrollbar').GetCurrentPattern($rangePattern).Current.Value -le $startRange
    } 'The accessible scrollbar value did not return to the start after Home.'
    Press-Node 'playlist-more'
    $rename = Wait-Result { Find-Named 'Переименовать плейлист' } 'Rename action is missing.'
    $rename.GetCurrentPattern($invokePattern).Invoke()
    $renamed = 'Ночное чтение'
    (Need-Node 'playlist-name').GetCurrentPattern($valuePattern).SetValue($renamed)
    Press-Node 'playlist-confirm-edit'
    $null = Wait-Result { (Find-Node 'playlist-title').Current.Name -eq $renamed } 'Playlist title did not change after rename.'
    Press-Node 'workspace-back'
    try {
        $null = Wait-Result {
            $row = Find-Node "local-playlist-$fixtureId"
            $row -and ($row.Current.Name -split ' · ', 2)[0] -ceq $renamed
        } 'Local library did not expose the new name.'
    } catch {
        $null = Save-Tree 'uia-local-rename-failure.json'
        throw
    }
    $null = Save-Tree 'uia-local-renamed.json'

    $report = [ordered]@{
        dpi = [PlaylistWindow]::GetDpiForWindow($window)
        navigation = $navigation
        toolbar = $toolbar
        viewport = $viewport
        player = $player
        visible_rows = $visibleRows
        visible_rows_at_end = $bottomVisibleRows
        confirmed_position = $initial
        position_after_back_and_forward = $afterReturn
        player_favorite = $true
        search_empty_state = $true
        keyboard_menu_escape_and_focus = $true
        queue_panel = $true
        pinned_regions_unchanged = $true
        scrollbar_accessible_value_at_start = $startRange
        scrollbar_accessible_value_at_end = $endRange
        renamed_library_row = $renamed
    }

    if ($SpotifyDatabase) {
        Close-Probe $process $window
        $sourceProfile = Join-Path $profile 'spotify'
        $null = New-Item -ItemType Directory -Path $sourceProfile
        Copy-Item -LiteralPath $SpotifyDatabase -Destination (Join-Path $sourceProfile 'mediadeka.sqlite3')
        & $Capture --seed-playlist $sourceProfile spotify
        if ($LASTEXITCODE -ne 0) { throw 'Could not select Spotify in the isolated profile.' }
        $env:MEDIADEKA_PROFILE_DIR = $sourceProfile
        $process = Start-Process -FilePath $Executable -PassThru -WindowStyle Hidden
        $window = Wait-ProbeWindow $process
        [PlaylistWindow]::RenderOffscreen($window, 800, 400)
        $root = $automation::FromHandle($window)
        Press-Node 'home-playlists'
        $card = Wait-Result { Find-Named 'SET::0.1 · Spotify' } 'Live Spotify playlists did not expose SET::0.1.' 45
        $card.GetCurrentPattern($invokePattern).Invoke()
        $null = Wait-Result {
            $bar = Find-Node 'playlist-scrollbar'
            $bar -and $bar.GetCurrentPattern($rangePattern).Current.Maximum -gt 0
        } 'Spotify playlist did not expose a loaded scrollbar.' 45
        if ((Need-Node 'playlist-title').Current.Name -ne 'SET::0.1' -or
            (Need-Node 'playlist-source-logo').Current.Name -ne 'Spotify') {
            throw 'Spotify card did not open the expected playlist and service logo.'
        }
        Press-Node 'playlist-more'
        $null = Need-Node 'playlist-menu'
        $spotify = @(Save-Tree 'uia-spotify.json')
        if ($spotify.Name -notcontains 'Копировать ссылку' -or
            $spotify.Name -contains 'Удалить плейлист' -or $spotify.Name -contains 'Сделать приватным') {
            throw 'Spotify menu does not match the implemented read-only capabilities.'
        }
        $report.spotify_live_card_route = $true
        Press-Key 0x1B
        $null = Wait-Result { !(Find-Node 'playlist-menu') } 'Spotify playlist menu did not close.'
        function Open-CompactTrackMenu {
            $scrollbar = Need-Node 'playlist-scrollbar'
            $scrollbar.SetFocus()
            Press-Key 0x23
            $null = Wait-Result {
                (Need-Node 'playlist-scrollbar').GetCurrentPattern($rangePattern).Current.Value -gt 0
            } 'Compact playlist did not scroll.'
            $button = Wait-Result {
                $root.FindAll($scope, [System.Windows.Automation.Condition]::TrueCondition) |
                    Where-Object { $_.Current.AutomationId -match '^playlist-row-menu-\d+$' } |
                    Select-Object -Last 1
            } 'Missing compact track menu.'
            $button.GetCurrentPattern($invokePattern).Invoke()
            $null = Need-Node 'playlist-menu'
        }
        Open-CompactTrackMenu
        $trackMenu = @(Save-Tree 'uia-spotify-compact-track-menu.json')
        if ($trackMenu.id -contains 'playlist-inspector') { throw 'Compact playlist exposed the inspector.' }
        foreach ($name in @('Открыть страницу трека', 'Найти артиста', 'Найти альбом')) {
            if ($trackMenu.Name -notcontains $name) { throw "Compact track menu missing $name." }
        }
        Press-Key 0x26
        Press-Key 0x0D
        $null = Need-Node 'music-search-title'
        $selectionPattern = [System.Windows.Automation.SelectionItemPattern]::Pattern
        $null = Wait-Result {
            (Find-Named 'Альбомы и синглы').GetCurrentPattern($selectionPattern).Current.IsSelected
        } 'Keyboard selection did not open album search.'
        Press-Node 'workspace-back'
        Open-CompactTrackMenu
        (Find-Named 'Найти артиста').GetCurrentPattern($invokePattern).Invoke()
        $null = Need-Node 'music-search-title'
        $null = Wait-Result {
            (Find-Named 'Артисты').GetCurrentPattern($selectionPattern).Current.IsSelected
        } 'The artist action did not open artist search.'
        Press-Node 'workspace-back'
        Open-CompactTrackMenu
        (Find-Named 'Открыть страницу трека').GetCurrentPattern($invokePattern).Invoke()
        $report.spotify_compact_track_title = (Need-Node 'music-object-title').Current.Name
        $report.spotify_compact_keyboard_album = $true
        $report.spotify_compact_artist_search = $true
    }
    $report | ConvertTo-Json -Depth 5 | Tee-Object -FilePath (Join-Path $Evidence 'uia-report.json')
} finally {
    $env:MEDIADEKA_PROFILE_DIR = $previousProfile
    Close-Probe $process $window
    $target = (Resolve-Path -LiteralPath $profile).Path
    $temp = ([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
    if ($target.StartsWith($temp, [StringComparison]::OrdinalIgnoreCase) -and
        [IO.Path]::GetFileName($target).StartsWith('mediadeka-playlist-uia-')) {
        Remove-Item -LiteralPath $target -Recurse -Force
    }
}
