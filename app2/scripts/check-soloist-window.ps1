param(
    [string]$Executable = "$PSScriptRoot/../target/acceptance/soloist-setup/package/mediadeka.exe",
    [string]$Capture = "$PSScriptRoot/../target/debug/examples/capture-window.exe",
    [string]$SpotifyDatabase = "$env:LOCALAPPDATA/Mediadeka/app2/mediadeka.sqlite3",
    [string]$Evidence = "$PSScriptRoot/../target/acceptance/soloist-setup"
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class MediadekaSoloistProbe {
    private delegate bool WindowVisitor(IntPtr window, IntPtr state);
    [DllImport("user32.dll")] private static extern bool EnumWindows(WindowVisitor visitor, IntPtr state);
    [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr window, out uint processId);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern int GetWindowText(IntPtr window, StringBuilder text, int capacity);
    [DllImport("user32.dll")] public static extern bool ShowWindowAsync(IntPtr window, int command);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr window, uint message, IntPtr wParam, IntPtr lParam);
    public static IntPtr Find(uint processId) {
        var found = IntPtr.Zero;
        EnumWindows((window, _) => {
            GetWindowThreadProcessId(window, out uint owner);
            if (owner != processId) return true;
            var title = new StringBuilder(32);
            GetWindowText(window, title, title.Capacity);
            if (title.ToString() != "Mediadeka") return true;
            found = window;
            return false;
        }, IntPtr.Zero);
        return found;
    }
}
'@

$automation = [System.Windows.Automation.AutomationElement]
$scope = [System.Windows.Automation.TreeScope]::Descendants
$invoke = [System.Windows.Automation.InvokePattern]::Pattern
$value = [System.Windows.Automation.ValuePattern]::Pattern

function Wait-Soloist([scriptblock]$Probe, [string]$Failure, [int]$Seconds = 20) {
    $timer = [System.Diagnostics.Stopwatch]::StartNew()
    do {
        try {
            $result = & $Probe
            if ($result) { return $result }
        } catch [System.Windows.Automation.ElementNotAvailableException] {}
        Start-Sleep -Milliseconds 75
    } while ($timer.Elapsed.TotalSeconds -lt $Seconds)
    throw $Failure
}

function Find-Soloist([string]$Id) {
    $condition = [System.Windows.Automation.PropertyCondition]::new($automation::AutomationIdProperty, $Id)
    $root.FindFirst($scope, $condition)
}

function Need-Soloist([string]$Id, [int]$Seconds = 20) {
    Wait-Soloist { Find-Soloist $Id } "Missing Soloist UI control $Id." $Seconds
}

function Press-Soloist([string]$Id) {
    $target = Wait-Soloist {
        $element = Find-Soloist $Id
        if ($element) {
            $pattern = $null
            if ($element.TryGetCurrentPattern($invoke, [ref]$pattern) -or
                $element.TryGetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern, [ref]$pattern)) {
                $pattern
            }
        }
    } "Control $Id could not be invoked."
    if ($target -is [System.Windows.Automation.InvokePattern]) { $target.Invoke() }
    else { $target.Toggle() }
}

function Escape-Soloist {
    [void][MediadekaSoloistProbe]::PostMessage($window, 0x0100, [IntPtr]0x1B, [IntPtr]1)
    [void][MediadekaSoloistProbe]::PostMessage($window, 0x0101, [IntPtr]0x1B, [IntPtr]::new(-1073741823))
}

function Save-SoloistTree([string]$File) {
    $nodes = foreach ($element in $root.FindAll($scope, [System.Windows.Automation.Condition]::TrueCondition)) {
        try {
            $node = $element.Current
            [ordered]@{
                id = $node.AutomationId
                name = $node.Name
                role = $node.ControlType.ProgrammaticName
                password = $node.IsPassword
                focus = $node.HasKeyboardFocus
                rectangle = @($node.BoundingRectangle.X, $node.BoundingRectangle.Y, $node.BoundingRectangle.Width, $node.BoundingRectangle.Height)
            }
        } catch [System.Windows.Automation.ElementNotAvailableException] {}
    }
    $nodes | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $Evidence $File) -Encoding utf8
}

function Wait-SoloistWindow($Process) {
    Wait-Soloist {
        $handle = [MediadekaSoloistProbe]::Find($Process.Id)
        if ($handle -ne [IntPtr]::Zero) { $handle }
    } 'Mediadeka did not create a native window.'
}

function Close-SoloistProbe($Process, [IntPtr]$Handle) {
    if ($Process -and !$Process.HasExited) {
        if ($Handle) { [void][MediadekaSoloistProbe]::PostMessage($Handle, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) }
        if (!$Process.WaitForExit(4500)) { Stop-Process -Id $Process.Id }
    }
}

function Native-Soloist {
    Get-CimInstance Win32_Process -Filter "Name = 'soloist.exe'" -Property ExecutablePath,ProcessId |
        Where-Object { $_.ExecutablePath -and $_.ExecutablePath.StartsWith("$profile\", [StringComparison]::OrdinalIgnoreCase) }
}

$null = New-Item -ItemType Directory -Force -Path $Evidence
$profile = Join-Path ([IO.Path]::GetTempPath()) "mediadeka-soloist-uia-$([Guid]::NewGuid().ToString('N'))"
$null = New-Item -ItemType Directory -Path $profile
$previousProfile = $env:MEDIADEKA_PROFILE_DIR
$process = $null
$window = [IntPtr]::Zero
$root = $null
try {
    Copy-Item -LiteralPath $SpotifyDatabase -Destination (Join-Path $profile 'mediadeka.sqlite3')
    & $Capture --seed-playlist $profile spotify
    if ($LASTEXITCODE) { throw 'Could not select Spotify in the isolated profile.' }
    $env:MEDIADEKA_PROFILE_DIR = $profile
    $process = Start-Process -FilePath $Executable -PassThru -WindowStyle Hidden
    $window = Wait-SoloistWindow $process
    [void][MediadekaSoloistProbe]::ShowWindowAsync($window, 4)
    $root = $automation::FromHandle($window)
    Press-Soloist 'home-playlists'
    $card = Wait-Soloist {
        $condition = [System.Windows.Automation.PropertyCondition]::new($automation::NameProperty, 'SET::0.1 · Spotify')
        $root.FindFirst($scope, $condition)
    } 'Live Spotify playlist SET::0.1 did not appear.' 45
    $card.GetCurrentPattern($invoke).Invoke()
    $null = Wait-Soloist {
        if ($failure = Find-Soloist 'playlist-unavailable') { throw $failure.Current.Name }
        Find-Soloist 'playlist-row-0'
    } 'Spotify playlist did not load.' 45
    (Need-Soloist 'playlist-play').SetFocus()
    Press-Soloist 'playlist-play'
    $null = Need-Soloist 'soloist-install'
    Save-SoloistTree 'uia-from-playback.json'
    Escape-Soloist
    $null = Wait-Soloist { !(Find-Soloist 'soloist-dialog') } 'Escape did not close the playback dialog.'
    $null = Wait-Soloist { (Need-Soloist 'player-play').Current.Name -eq 'Воспроизвести' } 'Playback context was discarded.'

    Press-Soloist 'home-settings'
    $null = Need-Soloist 'settings-soloist-status'
    (Need-Soloist 'settings-soloist').SetFocus()
    Press-Soloist 'settings-soloist'
    Press-Soloist 'soloist-install'
    $key = Need-Soloist 'soloist-key' 100
    if (!$key.Current.IsPassword) { throw 'Soloist key input is exposed as an ordinary field.' }
    Save-SoloistTree 'uia-installed.json'
    # The neutral string only starts the local listener. This process never selects the device in Spotify.
    $key.GetCurrentPattern($value).SetValue('mediadeka-local-listener-test')
    Press-Soloist 'soloist-pair'
    $null = Wait-Soloist { (Find-Soloist 'soloist-progress').Current.Name -like 'Ожидаем выбор*' } 'Soloist did not publish its local pairing listener.'
    Save-SoloistTree 'uia-pairing.json'
    Escape-Soloist
    $null = Wait-Soloist { !(Find-Soloist 'soloist-dialog') } 'Escape did not cancel pairing.'
    $null = Wait-Soloist {
        @(Native-Soloist).Count -eq 0
    } 'The isolated pairing process is still running.'
    $null = Wait-Soloist { (Need-Soloist 'settings-soloist').Current.HasKeyboardFocus } 'Focus did not return to settings.'

    Close-SoloistProbe $process $window
    $process = Start-Process -FilePath $Executable -PassThru -WindowStyle Hidden
    $window = Wait-SoloistWindow $process
    [void][MediadekaSoloistProbe]::ShowWindowAsync($window, 4)
    $root = $automation::FromHandle($window)
    Press-Soloist 'home-settings'
    $installed = Wait-Soloist {
        $node = Find-Soloist 'settings-soloist-status'
        if ($node -and $node.Current.Name -like 'Установлен*') { $node.Current.Name }
    } 'Installed version was not restored after restarting Mediadeka.'
    Press-Soloist 'settings-soloist'
    $null = Need-Soloist 'soloist-key'
    Save-SoloistTree 'uia-after-restart.json'

    & $Capture --soloist (Join-Path $Evidence 'frames') $profile
    if ($LASTEXITCODE) { throw 'Could not capture installed Soloist wizard.' }

    Close-SoloistProbe $process $window
    $root = $null
    & $Capture --seed-soloist-startup $profile
    if ($LASTEXITCODE) { throw 'Could not prepare the stale pairing fixture.' }
    $process = Start-Process -FilePath $Executable -PassThru -WindowStyle Hidden
    $window = Wait-SoloistWindow $process
    $runtime = Wait-Soloist {
        $portFile = Get-ChildItem -LiteralPath (Join-Path $profile 'soloist/pairings') -Recurse -File -Filter 'ws.port' | Select-Object -First 1
        if ($portFile) { Native-Soloist | Select-Object -First 1 }
    } 'Mediadeka did not launch the installed daemon during startup.'
    $soloistProcess = Get-Process -Id $runtime.ProcessId
    if ($soloistProcess.MainWindowHandle -ne [IntPtr]::Zero) { throw 'Soloist opened a top-level window.' }
    $null = Wait-Soloist { @(Native-Soloist).Count -eq 0 } 'Unauthenticated startup left a process behind.' 28
    [ordered]@{
        installed = $installed
        playback_opens_dialog = $true
        settings_opens_dialog = $true
        secret_field = $true
        real_pairing_listener_cancelled = $true
        settings_focus_restored = $true
        persisted_through_restart = $true
        startup_launched_real_soloist = $true
        startup_closed_unauthenticated_process = $true
    } | ConvertTo-Json | Tee-Object -FilePath (Join-Path $Evidence 'uia-report.json')
} catch {
    if ($root) { Save-SoloistTree 'uia-failure.json' }
    throw
} finally {
    Close-SoloistProbe $process $window
    $env:MEDIADEKA_PROFILE_DIR = $previousProfile
    $target = (Resolve-Path -LiteralPath $profile).Path
    $expected = ([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
    if ($target.StartsWith($expected, [StringComparison]::OrdinalIgnoreCase) -and
        [IO.Path]::GetFileName($target).StartsWith('mediadeka-soloist-uia-')) {
        Remove-Item -LiteralPath $target -Recurse -Force
    }
}
