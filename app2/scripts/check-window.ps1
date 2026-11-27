param(
    [string]$Executable = "$PSScriptRoot/../target/debug/mediadeka.exe",
    [string]$Evidence = "$PSScriptRoot/../target/acceptance/stage-1"
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
if (!('MediadekaWindowProbe' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;

public static class MediadekaWindowProbe
{
    private delegate bool WindowVisitor(IntPtr handle, IntPtr parameter);

    [DllImport("user32.dll")]
    private static extern bool EnumWindows(WindowVisitor visit, IntPtr parameter);

    [DllImport("user32.dll")]
    private static extern uint GetWindowThreadProcessId(IntPtr handle, out uint processId);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern int GetWindowText(IntPtr handle, StringBuilder title, int capacity);

    [DllImport("user32.dll", SetLastError = true)]
    private static extern bool PostMessage(IntPtr handle, uint message, IntPtr wParam, IntPtr lParam);

    public static IntPtr Find(uint processId, string title)
    {
        var found = IntPtr.Zero;
        EnumWindows((handle, parameter) => {
            GetWindowThreadProcessId(handle, out uint owner);
            if (owner != processId) return true;
            var text = new StringBuilder(title.Length + 1);
            if (GetWindowText(handle, text, text.Capacity) != title.Length || text.ToString() != title)
                return true;
            found = handle;
            return false;
        }, IntPtr.Zero);
        return found;
    }

    public static bool Close(IntPtr handle)
    {
        const uint closeMessage = 0x0010;
        return PostMessage(handle, closeMessage, IntPtr.Zero, IntPtr.Zero);
    }
}
'@
}

$exe = (Resolve-Path -LiteralPath $Executable).ProviderPath
$directory = (New-Item -ItemType Directory -Force -Path $Evidence).FullName
$application = Start-Process -FilePath $exe -WorkingDirectory $directory -PassThru -WindowStyle Hidden
try {
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    do {
        Start-Sleep -Milliseconds 100
        if ($application.HasExited) { throw "Mediadeka exited during startup ($($application.ExitCode))." }
        $window = [MediadekaWindowProbe]::Find($application.Id, 'Mediadeka')
    } until ($window -ne [IntPtr]::Zero -or [DateTime]::UtcNow -ge $deadline)
    if ($window -eq [IntPtr]::Zero) { throw 'The Mediadeka Win32 window did not open.' }

    $root = [System.Windows.Automation.AutomationElement]::FromHandle($window)
    $title = $root.Current.Name
    $role = $root.Current.ControlType.ProgrammaticName
    if ($title -ne 'Mediadeka' -or $role -ne 'ControlType.Window') {
        throw "Unexpected window semantics: $title ($role)."
    }

    if (![MediadekaWindowProbe]::Close($window) -or !$application.WaitForExit(5000)) {
        throw 'The application did not exit after the Windows close message.'
    }
    if ($application.ExitCode -ne 0) { throw "Unexpected exit code $($application.ExitCode)." }

    $report = [ordered]@{ title = $title; role = $role; closedByWindows = $true; exitCode = $application.ExitCode }
    $json = $report | ConvertTo-Json
    Set-Content -LiteralPath (Join-Path $directory 'window-uia.json') -Encoding utf8 -Value $json
    $json
} finally {
    if (!$application.HasExited) { Stop-Process -Id $application.Id -ErrorAction SilentlyContinue }
}
