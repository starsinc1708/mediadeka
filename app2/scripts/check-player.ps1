param(
    [string]$Executable = "$PSScriptRoot/../target/debug/mediadeka.exe",
    [string]$CaptureExecutable = "$PSScriptRoot/../target/debug/examples/capture-window.exe",
    [string]$Media = "$PSScriptRoot/../target/acceptance/playlist-page/fixtures/Awake.wav",
    [string]$Evidence = "$PSScriptRoot/../target/acceptance/player/uia"
)

$ErrorActionPreference = 'Stop'
$capture = (Resolve-Path -LiteralPath $CaptureExecutable).Path
$mediaPath = (Resolve-Path -LiteralPath $Media).Path
$temp = New-Item -ItemType Directory -Path (Join-Path ([IO.Path]::GetTempPath()) ('mediadeka-player-' + [Guid]::NewGuid()))
$processes = @()

if (!('MediadekaPlayerProbe' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

public static class MediadekaPlayerProbe {
    [DllImport("user32.dll")] private static extern bool PostMessage(IntPtr window, uint message, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr window, out uint processId);
    [DllImport("kernel32.dll")] private static extern uint GetCurrentThreadId();
    [DllImport("user32.dll")] private static extern bool AttachThreadInput(uint current, uint target, bool attach);
    [DllImport("user32.dll")] private static extern bool GetKeyboardState(byte[] state);
    [DllImport("user32.dll")] private static extern bool SetKeyboardState(byte[] state);

    // GPUI reads modifiers from GetKeyState, which PostMessage does not change.
    // Attaching to the window thread exposes the Alt bit while the posted
    // WM_SYSKEYDOWN/WM_SYSKEYUP pair is processed.
    public static bool AltDown(IntPtr handle) {
        const int menu = 0x12;
        const int leftMenu = 0xA4;
        const int down = 0x28;
        uint processId;
        uint target = GetWindowThreadProcessId(handle, out processId);
        uint current = GetCurrentThreadId();
        bool attached = current != target && AttachThreadInput(current, target, true);
        var state = new byte[256];
        byte[] saved = null;
        try {
            if (attached && GetKeyboardState(state)) {
                saved = (byte[])state.Clone();
                state[menu] = 0x80;
                state[leftMenu] = 0x80;
                SetKeyboardState(state);
            }
            long keyDown = 1L | (0x50L << 16) | (1L << 29);
            long keyUp = 0xC0000001L | (0x50L << 16) | (1L << 29);
            bool sent = PostMessage(handle, 0x0104, new IntPtr(down), new IntPtr(keyDown))
                && PostMessage(handle, 0x0105, new IntPtr(down), new IntPtr(keyUp));
            System.Threading.Thread.Sleep(150);
            return sent;
        } finally {
            if (saved != null) { SetKeyboardState(saved); }
            if (attached) { AttachThreadInput(current, target, false); }
        }
    }
}
'@
}

try {
    & $capture --seed-playlist $temp.FullName $mediaPath
    if ($LASTEXITCODE -ne 0) { throw 'Не удалось создать локальный плейлист для проверки.' }

    # Reuse the same window discovery and UI Automation primitives as the music-page scenarios.
    $probe = Get-Content -Raw -Encoding UTF8 -LiteralPath "$PSScriptRoot/check-music-pages.ps1"
    $marker = $probe.IndexOf('$previous = $env:MEDIADEKA_PROFILE_DIR')
    if ($marker -le 0) { throw 'Общие операции UI Automation не найдены.' }
    . ([scriptblock]::Create($probe.Substring(0, $marker))) -Profile $temp.FullName -Executable $Executable -Evidence $Evidence

    function Picker-Items($Panel, [string]$Prefix) {
        $Panel.FindAll($scope, [System.Windows.Automation.Condition]::TrueCondition) |
            Where-Object { $_.Current.AutomationId.StartsWith($Prefix) } |
            ForEach-Object {
                $pattern = $null
                if (!$_.TryGetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern, [ref]$pattern)) {
                    throw "Набор свойств SelectionItem недоступен для $($_.Current.AutomationId)."
                }
                [pscustomobject]@{ Element = $_; Selection = $pattern }
            }
    }

    function Picker-Semantics($Item) {
        $result = Semantic $Item.Element
        $result.selected = $Item.Selection.Current.IsSelected
        $result
    }

    function Selected($Element) {
        $pattern = $null
        if ($Element -and $Element.TryGetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern, [ref]$pattern)) {
            $pattern.Current.IsSelected
        } else {
            $false
        }
    }

    function Descendants($Root) {
        $Root.FindAll($scope, [System.Windows.Automation.Condition]::TrueCondition)
    }

    function Find-Prefix($Root, [string]$Prefix) {
        Descendants $Root | Where-Object { $_.Current.AutomationId.StartsWith($Prefix) } | Select-Object -First 1
    }

    function Start-Mediadeka {
        $previousProfile = $env:MEDIADEKA_PROFILE_DIR
        try {
            $env:MEDIADEKA_PROFILE_DIR = $temp.FullName
            $started = Start-Process -FilePath $program -WorkingDirectory (Split-Path $program) -WindowStyle Hidden -PassThru
        } finally {
            $env:MEDIADEKA_PROFILE_DIR = $previousProfile
        }
        $script:processes += $started
        $handle = Await {
            $hwnd = [MediadekaMusicProbe]::Find([uint32]$started.Id)
            if ($hwnd -ne [IntPtr]::Zero) { $hwnd }
        } 'Окно Mediadeka не найдено.' 15
        [MediadekaMusicProbe]::RenderOffscreen($handle)
        $root = Await {
            $candidate = $automation::FromHandle($handle)
            if (Element $candidate 'home-playlists') { $candidate }
        } 'Музыкальная оболочка не открылась.' 15
        [pscustomobject]@{ Process = $started; Handle = $handle; Root = $root }
    }

    function Open-Playlist($Root, [string]$Name) {
        Invoke-Element (Await { Element $Root 'home-playlists' } 'Нет пункта плейлистов.' 10)
        Invoke-Element (Await { Find-Name $Root $Name } "Нет локального плейлиста «$Name»." 10)
    }

    function Wait-Panel($Root, [string]$Tab) {
        $panel = Await { Element $Root 'right-panel' } 'Правая панель не открылась.' 8
        $null = Await { Selected (Element $Root $Tab) } "Вкладка $Tab не выбрана." 8
        $panel
    }

    $session = Start-Mediadeka
    $root = $session.Root

    Open-Playlist $root 'Ночной кодинг и музыка для длинного маршрута домой по пустым улицам города'
    Invoke-Element (Await {
        $button = Element $root 'playlist-play'
        $invoke = $null
        if ($button -and $button.Current.IsEnabled -and
            $button.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern, [ref]$invoke)) {
            $button
        }
    } 'Команда воспроизведения не стала доступна.' 10)
    $null = Await { (Element $root 'playlist-play').Current.Name -eq 'Пауза' } 'Плеер не начал воспроизведение.' 10

    Invoke-Element (Await { Element $root 'player-queue' } 'Нет кнопки очереди.' 8)
    $queue = Wait-Panel $root 'right-panel-queue'
    $tracks = @(Await {
        $items = @(Picker-Items $queue 'player-queue-entry')
        if ($items.Count -gt 2) { $items }
    } 'В очереди нет строк с семантикой выбора.' 8)
    $first = @($tracks | Select-Object -First 3 | ForEach-Object { Picker-Semantics $_ })
    if (!$first[0].selected -or $first[1].selected -or $first[0].name -eq $first[1].name) {
        throw 'Текущий трек или позиции повторных добавлений не различимы в UI Automation.'
    }
    $tracks[1].Selection.Select()
    $null = Await { $tracks[1].Selection.Current.IsSelected -and !$tracks[0].Selection.Current.IsSelected } 'Текущая строка очереди не обновилась.' 8
    $queueInfo = [ordered]@{ panel = Semantic $queue; initial = $first;
        afterSelect = @(Picker-Semantics $tracks[0]; Picker-Semantics $tracks[1]); visibleCount = $tracks.Count }

    Invoke-Element (Await { Element $root 'player-queue' } 'Кнопка очереди исчезла.' 6)
    $null = Await { !(Element $root 'right-panel') } 'Повторное действие не закрыло панель.' 6

    # Without a selected playlist row the track tab falls back to the sounding record.
    Invoke-Element (Await { Element $root 'player-queue' } 'Нет кнопки очереди.' 6)
    $null = Await { Element $root 'right-panel' } 'Панель не открылась повторно.' 6
    Invoke-Element (Await { Element $root 'right-panel-track' } 'Нет вкладки трека.' 6)
    $null = Await {
        $inspector = Element $root 'playlist-inspector'
        $title = Element $root 'playlist-inspector-title'
        $inspector -and $title -and $title.Current.Name -like 'Awake*'
    } 'Вкладка трека не показала звучащую запись.' 8
    $playingTrack = Semantic (Element $root 'playlist-inspector-title')

    # The row menu command «Сведения» opens the same inspector for the row.
    $menu = Find-Prefix $root 'playlist-row-menu'
    if (!$menu) { throw 'У строки плейлиста нет меню действий.' }
    Invoke-Element $menu
    Invoke-Element (Await { Find-Name $root 'Сведения' } 'В меню строки нет команды «Сведения».' 6)
    $null = Await { Element $root 'playlist-inspector' } 'Команда «Сведения» не открыла инспектор.' 8
    $details = Semantic (Element $root 'playlist-inspector-title')

    # The history row records where the sound came from.
    Invoke-Element (Await { Element $root 'home-history' } 'Нет пункта истории.' 8)
    $null = Await { Element $root 'history-title' } 'История не открылась.' 10
    $historyRow = Await {
        Find-Prefix $root 'history-item-'
    } 'В истории нет записей.' 10
    if ($historyRow.Current.Name -notlike '*Локальный файл*') {
        throw "История не указала локальный файл: $($historyRow.Current.Name)."
    }
    $historyInfo = @(Descendants $root | Where-Object { $_.Current.AutomationId -like 'history-item-*' } |
        ForEach-Object { Semantic $_ })

    # Activating a row replays the record and journals the new start.
    Invoke-Element $historyRow
    $null = Await {
        @(Descendants $root | Where-Object { $_.Current.AutomationId -match '^history-item-\d+$' }).Count -gt $historyInfo.Count
    } 'Повторный запуск из истории не добавил запись.' 12

    # Pin both local playlists through the hero button.
    Open-Playlist $root 'Ночной кодинг и музыка для длинного маршрута домой по пустым улицам города'
    Invoke-Element (Await { Element $root 'pin-toggle' } 'Нет кнопки закрепления.' 8)
    $null = Await { (Element $root 'pin-toggle').Current.Name -eq 'Открепить' } 'Плейлист не закрепился.' 8

    Invoke-Element (Await { Element $root 'home-playlists' } 'Нет пункта плейлистов.' 8)
    Invoke-Element (Await { Element $root 'playlist-create' } 'Нет создания плейлиста.' 8)
    Set-Field $root 'playlist-name' 'Второй плейлист'
    Invoke-Element (Await { Element $root 'playlist-confirm-edit' } 'Нет подтверждения создания.' 8)
    $null = Await { (Element $root 'playlist-title').Current.Name -eq 'Второй плейлист' } 'Второй плейлист не открылся.' 10
    Invoke-Element (Await { Element $root 'pin-toggle' } 'Нет кнопки закрепления.' 8)
    $null = Await { (Element $root 'pin-toggle').Current.Name -eq 'Открепить' } 'Второй плейлист не закрепился.' 8
    $pinnedBefore = @(Descendants $root | Where-Object { $_.Current.AutomationId -match '^home-pinned-\d+$' } |
        ForEach-Object { [ordered]@{ id = $_.Current.AutomationId; name = $_.Current.Name } })
    if ($pinnedBefore.Count -ne 2 -or $pinnedBefore[0].name -notlike 'Ночной*') {
        throw "Группа закреплённого не в ожидаемом порядке: $($pinnedBefore | ConvertTo-Json -Compress)."
    }

    # Alt+↓ is a window key message, like Press-Key in the playlist scenario.
    $firstPin = Await { Element $root 'home-pinned-0' } 'Нет первого пункта закреплённого.' 6
    $firstPin.SetFocus()
    $null = Await { (Element $root 'home-pinned-0').Current.HasKeyboardFocus } 'Фокус не перешёл на пункт закреплённого.' 8
    if (![MediadekaPlayerProbe]::AltDown($session.Handle)) { throw 'Не удалось отправить Alt+↓ в окно Mediadeka.' }
    $null = Await { (Element $root 'home-pinned-0').Current.Name -like 'Второй плейлист*' } 'Alt+↓ не изменил порядок закреплённого.' 8
    $pinnedAfter = @(Descendants $root | Where-Object { $_.Current.AutomationId -match '^home-pinned-\d+$' } |
        ForEach-Object { [ordered]@{ id = $_.Current.AutomationId; name = $_.Current.Name } })

    # Leave the panel open on the track tab and wait for the persisted state.
    if (!(Element $root 'right-panel')) {
        Invoke-Element (Await { Element $root 'player-queue' } 'Нет кнопки очереди.' 6)
        $null = Await { Element $root 'right-panel' } 'Панель не открылась перед перезапуском.' 8
    }
    Invoke-Element (Await { Element $root 'right-panel-track' } 'Нет вкладки трека.' 6)
    $null = Await { Selected (Element $root 'right-panel-track') } 'Перед перезапуском выбран не «Трек».' 8
    Start-Sleep -Milliseconds 700

    Stop-Process -Id $session.Process.Id
    $session.Process.WaitForExit(5000) | Out-Null

    $restarted = Start-Mediadeka
    $root = $restarted.Root
    $restoredPanel = Wait-Panel $root 'right-panel-track'
    $null = Await { Element $root 'right-panel-track-empty' } 'Пустая вкладка «Трек» не показала статус «Трек не выбран».' 8
    $restoredPins = Await {
        $pins = @(Descendants $root | Where-Object { $_.Current.AutomationId -match '^home-pinned-\d+$' } |
            ForEach-Object { [ordered]@{ id = $_.Current.AutomationId; name = $_.Current.Name } })
        if ($pins.Count -eq 2 -and $pins[0].name -like 'Второй плейлист*') { $pins }
    } 'После перезапуска закреплённое не восстановилось.' 10
    Invoke-Element (Await { Element $root 'home-history' } 'Нет пункта истории.' 8)
    $null = Await { Element $root 'history-title' } 'История не открылась после перезапуска.' 10
    $restoredHistory = Await {
        $row = Find-Prefix $root 'history-item-'
        if ($row -and $row.Current.Name -like '*Локальный файл*') { $row.Current.Name }
    } 'После перезапуска история пуста.' 10

    Invoke-Element (Await { Element $root 'home-playlists' } 'Нет пункта плейлистов.' 8)
    Invoke-Element (Await { Element $root 'player-devices' } 'Нет кнопки устройств вывода.' 6)
    $devices = Await { Element $root 'player-panel' } 'Панель устройств не открылась.' 6
    $options = @(Await { Picker-Items $devices 'player-device-' } 'Не найдены устройства вывода.' 10)
    $selection = @($options | Where-Object { $_.Selection.Current.IsSelected })
    if ($selection.Count -ne 1 -or $selection[0].Element.Current.AutomationId -ne 'player-device-auto') {
        throw 'UI Automation не определила единственный текущий системный выход.'
    }
    $default = $selection[0]
    $deviceInfo = @($options | ForEach-Object { Picker-Semantics $_ })
    $physical = $options | Where-Object { !$_.Selection.Current.IsSelected } | Select-Object -First 1
    $changedDevice = $null
    if ($physical) {
        $physical.Selection.Select()
        $null = Await { $physical.Selection.Current.IsSelected -and !$default.Selection.Current.IsSelected } 'Движок не подтвердил выбранное устройство.' 10
        $changedDevice = Picker-Semantics $physical
        $default.Selection.Select()
        $null = Await { $default.Selection.Current.IsSelected -and !$physical.Selection.Current.IsSelected } 'Движок не восстановил системное устройство.' 10
    }
    [ordered]@{ queue = $queueInfo; playingTrack = $playingTrack; details = $details;
        history = $historyInfo; historyAfterRestart = $restoredHistory;
        pinnedBefore = $pinnedBefore; pinnedAfter = $pinnedAfter; pinnedAfterRestart = $restoredPins;
        panelAfterRestart = Semantic $restoredPanel;
        devicesPanel = Semantic $devices; devices = $deviceInfo; changedDevice = $changedDevice;
        restoredDevice = Picker-Semantics $default } |
        ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $directory 'player-uia.json') -Encoding UTF8
    Write-Output "UI Automation: строк очереди $($tracks.Count), закреплённых $($pinnedAfter.Count), записей истории $($historyInfo.Count), устройств $($options.Count). Панель, история и закреплённое пережили перезапуск."
} finally {
    foreach ($started in $processes) {
        if ($started -and !$started.HasExited) {
            Stop-Process -Id $started.Id
            $started.WaitForExit(5000) | Out-Null
        }
    }
    if ($temp -and (Test-Path -LiteralPath $temp.FullName)) {
        Remove-Item -LiteralPath $temp.FullName -Recurse -Force -ErrorAction SilentlyContinue
    }
}
