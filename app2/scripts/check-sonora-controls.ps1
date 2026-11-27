param(
    [string]$Executable = "$PSScriptRoot/../target/debug/mediadeka.exe",
    [string]$Capture = "$PSScriptRoot/../target/debug/examples/capture-window.exe",
    [string]$Wave = "$PSScriptRoot/../target/acceptance/playlist-page/fixtures/Awake.wav",
    [string]$Evidence = "$PSScriptRoot/../target/acceptance/sonora-controls/uia"
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class SonoraWindow {
    private delegate bool WindowVisitor(IntPtr window, IntPtr state);
    [DllImport("user32.dll")] private static extern bool EnumWindows(WindowVisitor visitor, IntPtr state);
    [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr window, out uint processId);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern int GetWindowText(IntPtr window, StringBuilder title, int length);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr window, uint message, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll")] private static extern bool SetWindowPos(IntPtr window, IntPtr after, int x, int y, int width, int height, uint flags);
    [DllImport("user32.dll")] public static extern bool ShowWindowAsync(IntPtr window, int command);
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
$expandPattern = [System.Windows.Automation.ExpandCollapsePattern]::Pattern
$selectionPattern = [System.Windows.Automation.SelectionItemPattern]::Pattern

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
function Press-Node([string]$Id) {
    $element = Need-Node $Id
    $pattern = $null
    if ($element.TryGetCurrentPattern($invokePattern, [ref]$pattern)) {
        $pattern.Invoke()
    } elseif ($element.TryGetCurrentPattern($expandPattern, [ref]$pattern)) {
        $pattern.Expand()
    } elseif ($element.TryGetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern, [ref]$pattern)) {
        $pattern.Toggle()
    } elseif ($element.TryGetCurrentPattern($selectionPattern, [ref]$pattern)) {
        $pattern.Select()
    } else {
        throw "Could not activate $Id."
    }
}
function Press-Key([int]$key) {
    if (![SonoraWindow]::PostMessage($window, 0x0100, [IntPtr]$key, [IntPtr]1)) {
        throw "Failed to dispatch key $key."
    }
    if (![SonoraWindow]::PostMessage($window, 0x0101, [IntPtr]$key, [IntPtr]::new(-1073741823))) {
        throw "Failed to release key $key."
    }
    Start-Sleep -Milliseconds 120
}
function Node-Info([string]$Id) {
    $element = Need-Node $Id
    $invoke = $null
    $expand = $null
    $selection = $null
    $hasInvoke = $element.TryGetCurrentPattern($invokePattern, [ref]$invoke)
    $hasExpand = $element.TryGetCurrentPattern($expandPattern, [ref]$expand)
    $hasSelection = $element.TryGetCurrentPattern($selectionPattern, [ref]$selection)
    [ordered]@{
        id = $element.Current.AutomationId
        name = $element.Current.Name
        type = $element.Current.ControlType.ProgrammaticName
        enabled = $element.Current.IsEnabled
        focus = $element.Current.HasKeyboardFocus
        help = $element.Current.HelpText
        item_status = $element.Current.ItemStatus
        invoke = $hasInvoke
        expanded = if ($hasExpand) { $expand.Current.ExpandCollapseState.ToString() } else { $null }
        selected = if ($hasSelection) { $selection.Current.IsSelected } else { $null }
    }
}
function Save-Tree([string]$File) {
    $tree = foreach ($node in $root.FindAll($scope, [System.Windows.Automation.Condition]::TrueCondition)) {
        try {
            [ordered]@{
                id = $node.Current.AutomationId
                name = $node.Current.Name
                type = $node.Current.ControlType.ProgrammaticName
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
        if ($Handle) { [void][SonoraWindow]::PostMessage($Handle, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) }
        if (!$App.WaitForExit(4000)) { Stop-Process -Id $App.Id }
    }
}
function Wait-ProbeWindow($App) {
    Wait-Result {
        $handle = [SonoraWindow]::Find($App.Id)
        if ($handle -ne [IntPtr]::Zero) { $handle }
    } 'Native main window did not open.'
}

$profile = Join-Path ([IO.Path]::GetTempPath()) "mediadeka-sonora-uia-$([Guid]::NewGuid().ToString('N'))"
$null = New-Item -ItemType Directory -Path $profile
$null = New-Item -ItemType Directory -Path $Evidence -Force
$previousProfile = $env:MEDIADEKA_PROFILE_DIR
$process = $null
$window = [IntPtr]::Zero
try {
    $fixtureId = & $Capture --seed-playlist $profile $Wave
    if ($LASTEXITCODE -ne 0 -or $fixtureId -notmatch '^\d+$') { throw 'Unable to seed isolated profile.' }
    $env:MEDIADEKA_PROFILE_DIR = $profile
    $process = Start-Process -FilePath $Executable -PassThru -WindowStyle Hidden
    $window = Wait-ProbeWindow $process
    [SonoraWindow]::RenderOffscreen($window, 1500, 850)
    $root = $automation::FromHandle($window)
    $null = Need-Node 'home-playlists'
    Press-Node 'home-playlists'
    Press-Node "local-playlist-$fixtureId"
    $null = Need-Node 'playlist-sort'

    $closed = Node-Info 'playlist-sort'
    if ($closed.name -notlike 'Сортировка · *' -or $closed.expanded -ne 'Collapsed') {
        throw "Sort trigger semantics are wrong: $($closed | ConvertTo-Json -Compress)"
    }
    $sort = Need-Node 'playlist-sort'
    $expand = $sort.GetCurrentPattern($expandPattern)
    $expand.Expand()
    $null = Wait-Result { Find-Node 'playlist-sort-options' } 'Sort menu did not open.'
    $opened = Node-Info 'playlist-sort'
    $menu = Node-Info 'playlist-sort-options'
    $current = Node-Info 'playlist-sort-playlist'
    $title = Node-Info 'playlist-sort-title'
    if ($opened.expanded -ne 'Expanded' -or $menu.name -ne 'Сортировка' -or
        $current.selected -eq $false -or $title.selected -eq $true) {
        throw "Sort menu semantics are wrong: menu=$($menu | ConvertTo-Json -Compress) current=$($current | ConvertTo-Json -Compress) title=$($title | ConvertTo-Json -Compress)"
    }
    Save-Tree 'uia-sort-open.json' | Out-Null

    Press-Key 0x28
    $null = Wait-Result {
        $row = Find-Node 'playlist-sort-title'
        if ($row -and $row.Current.HasKeyboardFocus) { $row }
    } 'Down did not move the active row to "По названию".'
    $cursor = Node-Info 'playlist-sort-title'
    if ($cursor.selected -eq $true) { throw 'Keyboard cursor incorrectly changed the selected value.' }
    Press-Key 0x0D
    $null = Wait-Result { !(Find-Node 'playlist-sort-options') } 'Enter did not close the sort menu.'
    $applied = Wait-Result {
        $trigger = Find-Node 'playlist-sort'
        if ($trigger -and $trigger.Current.Name -like '*По названию') { Node-Info 'playlist-sort' }
    } 'Enter did not apply the sort.'
    if (!$applied.focus) {
        throw "Enter did not restore focus to the sort trigger: $($applied | ConvertTo-Json -Compress)"
    }
    Press-Key 0x0D
    $null = Wait-Result { Find-Node 'playlist-sort-options' } 'Sort menu did not reopen.'
    Press-Key 0x1B
    $null = Wait-Result { !(Find-Node 'playlist-sort-options') } 'Escape did not close the sort menu.'
    $null = Wait-Result {
        $trigger = Find-Node 'playlist-sort'
        if ($trigger -and $trigger.Current.HasKeyboardFocus) { $trigger }
    } 'Escape did not return focus to the sort trigger.'

    $gallery = [ordered]@{ checked = $false }
    try {
        Press-Node 'home-settings'
        $null = Wait-Result { Find-Node 'settings-appearance-shortcut' } 'Settings appearance shortcut is missing.'
        Press-Node 'settings-appearance-shortcut'
        $null = Wait-Result { Find-Node 'editorial-select' } 'Gallery selector is missing.'
        Press-Node 'editorial-select'
        $null = Wait-Result { Find-Node 'editorial-select-unavailable' } 'Gallery unavailable row is missing.'
        $unavailableInfo = Node-Info 'editorial-select-unavailable'
        $availableInfo = Node-Info 'editorial-select-auto'
        $tree = Save-Tree 'uia-gallery.json'
        $gallery = [ordered]@{
            checked = $true
            unavailable = $unavailableInfo
            available = $availableInfo
            invoke_pattern = $unavailableInfo.invoke
            visible_in_tree = ($tree.id -contains 'editorial-select-unavailable')
        }
    } catch {
        $tree = @(Save-Tree 'uia-gallery-failure.json')
        $gallery = [ordered]@{
            checked = $false
            reason = $_.Exception.Message
            related = @($tree | Where-Object { $_.id -like 'editorial-select*' })
        }
    }

    $report = [ordered]@{
        sort_trigger = $closed
        sort_menu = $menu
        selected_row = $current
        cursor_moved_without_selection = $true
        applied_by_enter = $applied.name
        escape_focus = $true
        gallery = $gallery
    }
    $report | ConvertTo-Json -Depth 8 | Tee-Object -FilePath (Join-Path $Evidence 'uia-report.json')
    if (!$gallery.checked) {
        throw "Gallery check did not complete: $($gallery.reason)"
    }
    if (!$gallery.visible_in_tree -or $gallery.unavailable.type -ne 'ControlType.MenuItem' -or
        $gallery.unavailable.name -ne 'Копировать ссылку · Ссылка недоступна' -or
        $gallery.unavailable.enabled -ne $false -or $gallery.invoke_pattern -or
        $gallery.available.enabled -ne $true) {
        throw "Gallery menu item availability is wrong: $($gallery | ConvertTo-Json -Depth 4 -Compress)"
    }
} finally {
    $env:MEDIADEKA_PROFILE_DIR = $previousProfile
    Close-Probe $process $window
    $target = (Resolve-Path -LiteralPath $profile).Path
    $temp = ([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
    if ($target.StartsWith($temp, [StringComparison]::OrdinalIgnoreCase) -and
        [IO.Path]::GetFileName($target).StartsWith('mediadeka-sonora-uia-')) {
        Remove-Item -LiteralPath $target -Recurse -Force
    }
}
