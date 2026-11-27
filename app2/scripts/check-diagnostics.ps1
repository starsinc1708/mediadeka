param(
    [string]$Executable = "$PSScriptRoot/../target/debug/mediadeka.exe",
    [string]$Evidence = "$PSScriptRoot/../target/acceptance/diagnostics"
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
if (!('MediadekaDiagnosticsProbe' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

public static class MediadekaDiagnosticsProbe
{
    private delegate bool Visitor(IntPtr handle, IntPtr parameter);

    [DllImport("user32.dll")] private static extern bool EnumWindows(Visitor visit, IntPtr arg);
    [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr handle, out uint processId);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern int GetWindowText(IntPtr handle, StringBuilder title, int count);
    [DllImport("user32.dll")] private static extern bool PostMessage(IntPtr handle, uint message, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool ShowWindowAsync(IntPtr handle, int command);

    public static IntPtr[] Find(uint processId, string title)
    {
        var matches = new List<IntPtr>();
        EnumWindows((handle, _) => {
            GetWindowThreadProcessId(handle, out uint owner);
            if (owner == processId) {
                var value = new StringBuilder(128);
                GetWindowText(handle, value, value.Capacity);
                if (value.ToString() == title) matches.Add(handle);
            }
            return true;
        }, IntPtr.Zero);
        return matches.ToArray();
    }

    public static bool Enter(IntPtr handle)
    {
        const int key = 0x0D;
        return PostMessage(handle, 0x0100, new IntPtr(key), new IntPtr(1 | (28 << 16)))
            && PostMessage(handle, 0x0101, new IntPtr(key), new IntPtr(unchecked((int)0xC01C0001)));
    }

    public static bool Close(IntPtr handle)
    {
        return PostMessage(handle, 0x0010, IntPtr.Zero, IntPtr.Zero);
    }
}
'@
}

function Await([scriptblock]$Condition, [string]$Failure) {
    $deadline = [DateTime]::UtcNow.AddSeconds(12)
    do {
        try {
            $result = & $Condition
            if ($result) { return $result }
        } catch [System.Windows.Automation.ElementNotAvailableException] {}
        Start-Sleep -Milliseconds 60
    } until ([DateTime]::UtcNow -ge $deadline)
    throw $Failure
}

$automation = [System.Windows.Automation.AutomationElement]
function Find-Element($Root, [string]$Id) {
    $property = [System.Windows.Automation.PropertyCondition]::new($automation::AutomationIdProperty, $Id)
    $Root.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $property)
}

function Invoke-Button($Element) {
    $pattern = [System.Windows.Automation.InvokePattern]$Element.GetCurrentPattern(
        [System.Windows.Automation.InvokePattern]::Pattern)
    $pattern.Invoke()
}

$directory = (New-Item -ItemType Directory -Path $Evidence -Force).FullName
$exe = (Resolve-Path -LiteralPath $Executable).ProviderPath
$profile = (New-Item -ItemType Directory -Path (Join-Path $directory ('uia-profile-' + [guid]::NewGuid().ToString('N')))).FullName
$previous = [Environment]::GetEnvironmentVariable('MEDIADEKA_PROFILE_DIR', 'Process')
try {
    [Environment]::SetEnvironmentVariable('MEDIADEKA_PROFILE_DIR', $profile, 'Process')
    $app = Start-Process -FilePath $exe -WorkingDirectory $directory -PassThru -WindowStyle Hidden
} finally {
    [Environment]::SetEnvironmentVariable('MEDIADEKA_PROFILE_DIR', $previous, 'Process')
}

try {
    $probe = [MediadekaDiagnosticsProbe]
    $title = 'Mediadeka — Диагностика'
    $main = Await { $probe::Find($app.Id, 'Mediadeka') | Select-Object -First 1 } 'Main window did not open.'
    [void]$probe::ShowWindowAsync($main, 5)
    $root = $automation::FromHandle($main)
    $button = Await { Find-Element $root 'diagnostics' } 'The diagnostics button is missing.'
    $minimize = Await { Find-Element $root 'minimize' } 'The minimize button is missing.'
    $buttonId = $button.Current.AutomationId
    $position = $button.Current.BoundingRectangle
    $next = $minimize.Current.BoundingRectangle
    if ($button.Current.Name -cne 'Диагностика' -or
        $button.Current.ControlType -ne [System.Windows.Automation.ControlType]::Button -or
        !$button.Current.IsKeyboardFocusable -or $button.Current.IsOffscreen -or
        $position.Left -ge $next.Left -or [Math]::Abs($position.Right - $next.Left) -gt 2 -or
        [Math]::Abs($position.Top - $next.Top) -gt 2) {
        throw 'The accessible diagnostics button is not next to Minimize.'
    }

    Invoke-Button $button
    $diagnostics = Await { $probe::Find($app.Id, $title) | Select-Object -First 1 } 'Diagnostics did not open.'
    $panel = $automation::FromHandle($diagnostics)
    $sections = @('diagnostics-fps', 'diagnostics-cpu', 'diagnostics-network')
    foreach ($id in $sections) {
        [void](Await { Find-Element $panel $id } "The diagnostics UIA tree is missing $id.")
    }
    Invoke-Button $button
    Start-Sleep -Milliseconds 350
    $openWindows = @($probe::Find($app.Id, $title))
    if ($openWindows.Count -ne 1 -or $openWindows[0] -ne $diagnostics) {
        throw 'Invoking the same button opened another diagnostics window.'
    }

    if (!$probe::Close($diagnostics)) { throw 'Could not close diagnostics.' }
    [void](Await { $probe::Find($app.Id, $title).Count -eq 0 } 'Diagnostics did not close separately.')
    if ($app.HasExited) { throw 'Closing diagnostics also closed the main window.' }

    $button = Await { Find-Element $root 'diagnostics' } 'The diagnostics button disappeared after close.'
    $button.SetFocus()
    [void](Await { $button.Current.HasKeyboardFocus } 'Diagnostics button could not receive keyboard focus.')
    if (!$probe::Enter($main)) { throw 'Could not send Enter to the main window.' }
    [void](Await { $probe::Find($app.Id, $title).Count -eq 1 } 'Enter did not reopen diagnostics.')

    $close = Await { Find-Element $root 'close' } 'The main close button is missing.'
    Invoke-Button $close
    if (!$app.WaitForExit(12000)) { throw 'Closing the main window left diagnostics running.' }
    if ($app.ExitCode -ne 0) { throw "Application closed with exit code $($app.ExitCode)." }

    [ordered]@{
        button = $buttonId
        nextToMinimize = $true
        panels = $sections
        singleWindow = $true
        closedSeparately = $true
        reopenedByEnter = $true
        closedWithMain = $true
    } | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $directory 'diagnostics-uia.json') -Encoding utf8
    'Windows UI Automation passed for diagnostics.'
} finally {
    if (!$app.HasExited) { Stop-Process -Id $app.Id -ErrorAction SilentlyContinue }
}
