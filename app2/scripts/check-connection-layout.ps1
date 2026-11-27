param(
    [string]$Executable = "$PSScriptRoot/../target/debug/mediadeka.exe",
    [string]$Evidence = "$PSScriptRoot/../target/acceptance/stage-2"
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
if (!('MediadekaConnectionLayoutProbe' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;

public static class MediadekaConnectionLayoutProbe
{
    [StructLayout(LayoutKind.Sequential)] public struct Rect { public int Left, Top, Right, Bottom; }
    [StructLayout(LayoutKind.Sequential)] private struct Point { public int X, Y; }
    private delegate bool WindowVisitor(IntPtr window, IntPtr state);

    [DllImport("user32.dll")] private static extern bool EnumWindows(WindowVisitor visitor, IntPtr state);
    [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr window, out uint processId);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern int GetWindowText(IntPtr window, StringBuilder title, int length);
    [DllImport("user32.dll")] private static extern bool GetWindowRect(IntPtr window, out Rect rect);
    [DllImport("user32.dll")] private static extern bool GetClientRect(IntPtr window, out Rect rect);
    [DllImport("user32.dll")] private static extern bool ClientToScreen(IntPtr window, ref Point point);
    [DllImport("user32.dll")] private static extern bool SetWindowPos(IntPtr window, IntPtr after, int x, int y, int width, int height, uint flags);
    [DllImport("user32.dll")] private static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
    [DllImport("user32.dll")] private static extern bool PostMessage(IntPtr window, uint msg, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr window);
    [DllImport("user32.dll")] public static extern bool ShowWindowAsync(IntPtr window, int command);

    public static IntPtr Find(uint processId)
    {
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

    public static Rect Client(IntPtr window)
    {
        var previous = SetThreadDpiAwarenessContext(new IntPtr(-4));
        try {
            var origin = new Point();
            if (!GetClientRect(window, out var client) || !ClientToScreen(window, ref origin)) return default;
            client.Left += origin.X; client.Right += origin.X;
            client.Top += origin.Y; client.Bottom += origin.Y;
            return client;
        } finally {
            if (previous != IntPtr.Zero) SetThreadDpiAwarenessContext(previous);
        }
    }

    public static bool Resize(IntPtr window, int width, int height)
    {
        var previous = SetThreadDpiAwarenessContext(new IntPtr(-4));
        try {
            if (!GetClientRect(window, out var client) || !GetWindowRect(window, out var outer)) return false;
            return SetWindowPos(window, IntPtr.Zero, 0, 0,
                width + outer.Right - outer.Left - client.Right,
                height + outer.Bottom - outer.Top - client.Bottom, 0x0016);
        } finally {
            if (previous != IntPtr.Zero) SetThreadDpiAwarenessContext(previous);
        }
    }

    public static bool Escape(IntPtr window)
    {
        const int escape = 0x1B;
        return PostMessage(window, 0x0100, new IntPtr(escape), new IntPtr(0x00010001))
            && PostMessage(window, 0x0101, new IntPtr(escape), new IntPtr(unchecked((int)0xC0010001)));
    }
}
'@
}

$automation = [System.Windows.Automation.AutomationElement]
$scope = [System.Windows.Automation.TreeScope]::Descendants
$options = [ordered]@{
    'spotify' = 'Spotify'
    'youtube-music' = 'YouTube Music'
    'yandex-music' = 'Яндекс Музыка'
    'soundcloud' = 'SoundCloud'
    'jellyfin' = 'Jellyfin'
    'guest' = 'Гостевой вход'
}

function Find-LayoutElement($root, [string]$id) {
    $condition = [System.Windows.Automation.PropertyCondition]::new($automation::AutomationIdProperty, $id)
    $root.FindFirst($scope, $condition)
}

function Wait-Layout([scriptblock]$probe, [string]$failure, [int]$seconds = 8) {
    $timeout = [System.Diagnostics.Stopwatch]::StartNew()
    do {
        try {
            $value = & $probe
            if ($value) { return $value }
        } catch [System.Windows.Automation.ElementNotAvailableException] {}
        Start-Sleep -Milliseconds 75
    } while ($timeout.Elapsed.TotalSeconds -lt $seconds)
    throw $failure
}

function Wait-LayoutElement($root, [string]$id) {
    Wait-Layout { Find-LayoutElement $root $id } "Missing UIA element $id."
}

function Invoke-Layout($root, [string]$id) {
    $element = Wait-LayoutElement $root $id
    $invoke = [System.Windows.Automation.InvokePattern]$element.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern)
    $invoke.Invoke()
}

function Wait-LayoutFocus($root, [string]$id) {
    [void](Wait-Layout {
        $element = Find-LayoutElement $root $id
        $element -and $element.Current.HasKeyboardFocus
    } "Keyboard focus did not move to $id.")
}

function Describe-LayoutElement($element) {
    $state = $element.Current
    $bounds = $state.BoundingRectangle
    [ordered]@{
        id = $state.AutomationId
        name = $state.Name
        role = $state.ControlType.ProgrammaticName
        focusable = $state.IsKeyboardFocusable
        focused = $state.HasKeyboardFocus
        protected = $state.IsPassword
        bounds = @($bounds.Left, $bounds.Top, $bounds.Width, $bounds.Height)
    }
}

function Resize-Layout([IntPtr]$window, [int]$logicalWidth, [int]$logicalHeight, [uint32]$dpi) {
    $width = [int][Math]::Round($logicalWidth * $dpi / 96)
    $height = [int][Math]::Round($logicalHeight * $dpi / 96)
    if (![MediadekaConnectionLayoutProbe]::Resize($window, $width, $height)) { throw 'Win32 resize failed.' }
    Wait-Layout {
        $rect = [MediadekaConnectionLayoutProbe]::Client($window)
        if ($rect.Right - $rect.Left -eq $width -and $rect.Bottom - $rect.Top -eq $height) { $rect }
    } "Win32 did not apply client size $logicalWidth × $logicalHeight at DPI $dpi."
}

function Read-Rail($root, $client, $beside = $null) {
    $previousBottom = [double]::NegativeInfinity
    foreach ($id in $options.Keys) {
        $element = Wait-LayoutElement $root $id
        $state = $element.Current
        $bounds = $state.BoundingRectangle
        if ($state.Name -cne $options[$id] -or $state.ControlType.ProgrammaticName -ne 'ControlType.Button' -or
            !$state.IsKeyboardFocusable -or $state.IsOffscreen -or $bounds.IsEmpty) {
            throw "Unexpected UIA button semantics for $id."
        }
        if ($bounds.Left -lt ($client.Left - 2) -or $bounds.Top -lt ($client.Top - 2) -or
            $bounds.Right -gt ($client.Right + 2) -or $bounds.Bottom -gt ($client.Bottom + 2)) {
            throw "The button $id is clipped at the requested client size."
        }
        if ($beside -and $bounds.Right -gt ($beside.Left + 2)) { throw "The button $id overlaps the right login panel." }
        if ($bounds.Top -lt ($previousBottom - 2)) { throw "The option $id is out of vertical order." }
        $previousBottom = $bounds.Bottom
        Describe-LayoutElement $element
    }
}

function Get-LoginPanel($root) {
    $sample = @{ bounds = ''; repeats = 0 }
    $element = Wait-Layout {
        $candidate = Find-LayoutElement $root 'login-panel'
        if (!$candidate -or $candidate.Current.IsOffscreen) { return }
        $bounds = $candidate.Current.BoundingRectangle
        if ($bounds.IsEmpty) { return }
        $key = '{0:F1},{1:F1},{2:F1},{3:F1}' -f $bounds.Left, $bounds.Top, $bounds.Width, $bounds.Height
        if ($sample.bounds -eq $key) {
            [void]($sample.repeats++)
        } else {
            $sample.bounds = $key
            $sample.repeats = 0
        }
        if ($sample.repeats -ge 3) { $candidate }
    } 'The login panel did not settle after the transition.'
    $state = $element.Current
    $panelId = [System.Windows.Automation.PropertyCondition]::new($automation::AutomationIdProperty, 'login-panel')
    if ($root.FindAll($scope, $panelId).Count -ne 1) { throw 'Expected one reusable login panel.' }
    if ($state.ControlType.ProgrammaticName -ne 'ControlType.Group' -or
        $state.IsOffscreen -or $state.BoundingRectangle.IsEmpty) { throw 'The login panel is not a visible UIA group.' }
    $element
}

function Assert-PanelBounds($expected, $current) {
    $actual = $current.Current.BoundingRectangle
    foreach ($edge in 'Left', 'Top', 'Right', 'Bottom') {
        if ([Math]::Abs($expected.$edge - $actual.$edge) -gt 2) {
            throw "The $edge edge of the wide login panel changed from $($expected.$edge) to $($actual.$edge) physical pixels."
        }
    }
}

function Read-WideEntryOptions($root, $client, $panel) {
    $group = Wait-LayoutElement $root 'entry-options'
    $bounds = $group.Current.BoundingRectangle
    if ($group.Current.ControlType.ProgrammaticName -ne 'ControlType.Group' -or $group.Current.IsOffscreen -or $bounds.IsEmpty) {
        throw 'The left entry options are not a visible UIA group.'
    }
    $brandName = [System.Windows.Automation.PropertyCondition]::new($automation::NameProperty, 'Mediadeka')
    $brand = $root.FindFirst($scope, $brandName)
    if (!$brand -or $brand.Current.BoundingRectangle.IsEmpty -or $group.FindFirst($scope, $brandName) -or
        $brand.Current.BoundingRectangle.Bottom -gt ($bounds.Top + 2)) {
        throw 'The brand title is not above and outside the entry options.'
    }
    $panelBounds = $panel.Current.BoundingRectangle
    if ([Math]::Abs($bounds.Top - $panelBounds.Top) -gt 2 -or [Math]::Abs($bounds.Bottom - $panelBounds.Bottom) -gt 2) {
        throw "Wide panel vertical bounds $($panelBounds.Top)–$($panelBounds.Bottom) differ from entry options $($bounds.Top)–$($bounds.Bottom) in physical pixels."
    }
    $buttons = @(Read-Rail $group $client $panelBounds)
    $guest = (Wait-LayoutElement $group 'guest').Current.BoundingRectangle
    if ([Math]::Abs($bounds.Bottom - $guest.Bottom) -gt 2 -or $bounds.Right -gt ($panelBounds.Left + 2)) {
        throw 'The entry options do not end at the guest button or overlap the panel.'
    }
    [ordered]@{ group = Describe-LayoutElement $group; buttons = $buttons; panel = Describe-LayoutElement $panel }
}

function Switch-WideProvider($root, [string]$provider, $expectedBounds) {
    Invoke-Layout $root $provider
    $elapsed = [System.Diagnostics.Stopwatch]::StartNew()
    $samples = 0
    do {
        $panel = Find-LayoutElement $root 'login-panel'
        if (!$panel) { throw 'The reusable panel disappeared during a provider switch.' }
        Assert-PanelBounds $expectedBounds $panel
        [void]($samples++)
        Start-Sleep -Milliseconds 45
    } while ($elapsed.ElapsedMilliseconds -lt 560)
    if ($samples -lt 2) { throw 'Too few UIA bounds samples during the provider switch.' }
    $samples
}

function Assert-LayoutNavigation($root, [switch]$Compact) {
    foreach ($id in 'cancel', 'back') {
        if (Find-LayoutElement $root $id) { throw "Retired action $id remains in the UIA tree." }
    }
    $condition = [System.Windows.Automation.PropertyCondition]::new($automation::AutomationIdProperty, 'connection-return')
    $arrows = $root.FindAll($scope, $condition)
    if (!$Compact) {
        if ($arrows.Count) { throw 'The wide login panel exposed a return button.' }
        return
    }
    if ($arrows.Count -ne 1) { throw 'The compact panel did not expose exactly one return button.' }
    $arrow = $arrows[0]
    if ($arrow.Current.Name -cne 'Вернуться к списку' -or $arrow.Current.ControlType.ProgrammaticName -ne 'ControlType.Button' -or
        !$arrow.Current.IsKeyboardFocusable -or $arrow.Current.IsOffscreen) {
        throw 'The compact return button has unexpected accessibility semantics.'
    }
    Describe-LayoutElement $arrow
}

function Confirm-LayoutClosed($root, [string]$provider, [string]$action) {
    Wait-LayoutFocus $root $provider
    [void](Wait-Layout { !(Find-LayoutElement $root 'login-panel') } "$action did not close the login panel.")
    Assert-LayoutNavigation $root
}

function Read-YouTubeHelp($panel) {
    if (!(Find-LayoutElement $panel 'youtube-help-1')) {
        Invoke-Layout $panel 'youtube-help-toggle'
    }
    foreach ($step in 1..3) {
        $element = Wait-LayoutElement $panel "youtube-help-$step"
        if ($element.Current.Name -notmatch "^\s*$step(?:[.)\s]|$)") {
            throw "YouTube step $step lacks numbered help in UIA."
        }
        Describe-LayoutElement $element
    }
}

function Check-ProtectedField($field) {
    $value = [System.Windows.Automation.ValuePattern]$field.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern)
    if (!$field.Current.IsPassword -or $value.Current.Value) {
        throw "UIA exposed a value in protected field $($field.Current.AutomationId)."
    }
}

$directory = (New-Item -ItemType Directory -Force -Path $Evidence).FullName
$exe = (Resolve-Path -LiteralPath $Executable).ProviderPath
$profile = Join-Path $directory ('connection-layout-profile-' + [guid]::NewGuid().ToString('N'))
[void](New-Item -ItemType Directory -Path $profile)
$previousProfile = $env:MEDIADEKA_PROFILE_DIR
$app = $null
$root = $null
$stage = 'startup'
try {
    try {
        $env:MEDIADEKA_PROFILE_DIR = $profile
        $app = Start-Process -FilePath $exe -WorkingDirectory $directory -PassThru -WindowStyle Hidden
    } finally {
        $env:MEDIADEKA_PROFILE_DIR = $previousProfile
    }

    $window = Wait-Layout {
        if ($app.HasExited) { throw "Mediadeka exited during startup with code $($app.ExitCode)." }
        $handle = [MediadekaConnectionLayoutProbe]::Find($app.Id)
        if ($handle -ne [IntPtr]::Zero) { $handle }
    } 'Mediadeka window did not open.' 15
    [void][MediadekaConnectionLayoutProbe]::ShowWindowAsync($window, 5)
    $root = Wait-Layout {
        $candidate = $automation::FromHandle($window)
        if (Find-LayoutElement $candidate 'guest') { $candidate }
    } 'The welcome UIA tree did not load.' 15
    $dpi = [MediadekaConnectionLayoutProbe]::GetDpiForWindow($window)
    if (!$dpi) { throw 'Could not obtain the native window DPI.' }

    $stage = 'wide-youtube'
    $wide = Resize-Layout $window 1100 680 $dpi
    Invoke-Layout $root 'youtube-music'
    Wait-LayoutFocus $root 'youtube-session'
    $youtubePanel = Get-LoginPanel $root
    $youtubeRegion = $youtubePanel.Current.BoundingRectangle
    $wideLayout = Read-WideEntryOptions $root $wide $youtubePanel
    Assert-LayoutNavigation $root
    $youtubeHelp = @(Read-YouTubeHelp $youtubePanel)
    $youtube = Wait-LayoutElement $youtubePanel 'youtube-session'
    Check-ProtectedField $youtube
    $youtubeInitial = Describe-LayoutElement $youtube

    $stage = 'wide-jellyfin'
    $switchSamples = Switch-WideProvider $root 'jellyfin' $youtubeRegion
    Wait-LayoutFocus $root 'jellyfin-server'
    $jellyfinPanel = Get-LoginPanel $root
    Assert-PanelBounds $youtubeRegion $jellyfinPanel
    [void](Read-WideEntryOptions $root $wide $jellyfinPanel)
    Assert-LayoutNavigation $root
    if (Find-LayoutElement $root 'youtube-session') { throw 'A stale YouTube session field survived switching to Jellyfin.' }
    $jellyfinFields = @(foreach ($id in 'jellyfin-server', 'jellyfin-username', 'jellyfin-password') {
        $field = Wait-LayoutElement $jellyfinPanel $id
        if ($field.Current.ControlType.ProgrammaticName -ne 'ControlType.Edit' -or
            !$field.Current.IsKeyboardFocusable -or $field.Current.IsPassword -ne ($id -eq 'jellyfin-password')) {
            throw "Unexpected Jellyfin field semantics for $id."
        }
        if ($field.Current.IsPassword) { Check-ProtectedField $field }
        Describe-LayoutElement $field
    })

    $stage = 'wide-inline-error'
    Invoke-Layout $jellyfinPanel 'jellyfin-submit'
    [void](Wait-LayoutElement $root 'login-error')
    $errorPanel = Get-LoginPanel $root
    Assert-PanelBounds $youtubeRegion $errorPanel
    $inlineError = Wait-LayoutElement $errorPanel 'login-error'
    if ($inlineError.Current.Name -notmatch 'Jellyfin|адрес' -or $inlineError.Current.IsOffscreen -or
        !(Find-LayoutElement $errorPanel 'jellyfin-server')) { throw 'The Jellyfin address error was not presented inline with the form.' }
    $errorEvidence = Describe-LayoutElement $inlineError
    [void](Read-WideEntryOptions $root $wide $errorPanel)
    Assert-LayoutNavigation $root

    $stage = 'wide-reselect'
    $reselectSamples = Switch-WideProvider $root 'youtube-music' $youtubeRegion
    Wait-LayoutFocus $root 'youtube-session'
    $reselected = Get-LoginPanel $root
    Assert-PanelBounds $youtubeRegion $reselected
    if ((Find-LayoutElement $root 'login-error') -or (Find-LayoutElement $root 'jellyfin-server')) {
        throw 'Reselecting YouTube left stale Jellyfin controls in the window.'
    }
    Assert-LayoutNavigation $root

    $stage = 'short-wide-resize-and-switch'
    $shortWide = Resize-Layout $window 1100 400 $dpi
    $shortWideYouTube = Get-LoginPanel $root
    $shortWideRegion = $shortWideYouTube.Current.BoundingRectangle
    $shortWideLayout = Read-WideEntryOptions $root $shortWide $shortWideYouTube
    Assert-LayoutNavigation $root
    $shortSwitchSamples = Switch-WideProvider $root 'jellyfin' $shortWideRegion
    Wait-LayoutFocus $root 'jellyfin-server'
    $shortWideJellyfin = Get-LoginPanel $root
    Assert-PanelBounds $shortWideRegion $shortWideJellyfin
    [void](Read-WideEntryOptions $root $shortWide $shortWideJellyfin)
    Assert-LayoutNavigation $root
    if (![MediadekaConnectionLayoutProbe]::Escape($window)) { throw 'Could not send Escape to the wide login window.' }
    Confirm-LayoutClosed $root 'jellyfin' 'Escape'

    $stage = 'compact-panel-after-resize'
    Invoke-Layout $root 'youtube-music'
    Wait-LayoutFocus $root 'youtube-session'
    $compact = Resize-Layout $window 800 400 $dpi
    $compactYouTube = Get-LoginPanel $root
    $compactHelp = @(Read-YouTubeHelp $compactYouTube)
    Check-ProtectedField (Wait-LayoutElement $compactYouTube 'youtube-session')
    $compactArrow = Assert-LayoutNavigation $root -Compact
    Invoke-Layout $compactYouTube 'connection-return'
    Confirm-LayoutClosed $root 'youtube-music' 'Compact return arrow'
    Invoke-Layout $root 'jellyfin'
    Wait-LayoutFocus $root 'jellyfin-server'
    $compactPanel = Get-LoginPanel $root
    $compactRegion = Describe-LayoutElement $compactPanel
    $oldFocusableOptions = @($options.Keys | Where-Object {
        $element = Find-LayoutElement $root $_
        $element -and $element.Current.IsKeyboardFocusable
    })
    if ($oldFocusableOptions.Count) { throw "The compact form retained focusable options: $($oldFocusableOptions -join ', ')." }

    $stage = 'compact-return-clears-form'
    Assert-LayoutNavigation $root -Compact | Out-Null
    Invoke-Layout $compactPanel 'jellyfin-submit'
    [void](Wait-LayoutElement $compactPanel 'login-error')
    Invoke-Layout $compactPanel 'connection-return'
    Confirm-LayoutClosed $root 'jellyfin' 'Compact return arrow'
    Invoke-Layout $root 'jellyfin'
    $reopenedCompact = Get-LoginPanel $root
    if (Find-LayoutElement $reopenedCompact 'login-error') { throw 'The compact return left the previous attempt on reopening.' }
    Invoke-Layout $reopenedCompact 'connection-return'
    Confirm-LayoutClosed $root 'jellyfin' 'Compact return arrow'
    $compactRail = @(Read-Rail $root $compact)
    Invoke-Layout $root 'close'
    if (!$app.WaitForExit(8000)) { throw 'Mediadeka did not close.' }
    if ($app.ExitCode -ne 0) { throw "Mediadeka closed with code $($app.ExitCode)." }

    [ordered]@{
        window = 'Mediadeka'
        dpi = $dpi
        wide = [ordered]@{
            logicalClient = @(1100, 680)
            physicalClient = @(($wide.Right - $wide.Left), ($wide.Bottom - $wide.Top))
            rail = $wideLayout.buttons
            entryOptions = $wideLayout.group
            panel = $wideLayout.panel
            youtubeField = $youtubeInitial
            youtubeHelp = $youtubeHelp
            jellyfinFields = $jellyfinFields
            inlineError = $errorEvidence
            rightColumnReused = $true
            switchGeometrySamples = @($switchSamples, $reselectSamples)
            staleControlsAfterReselect = $false
            legacyActionsAbsent = $true
        }
        shortWide = [ordered]@{
            logicalClient = @(1100, 400)
            physicalClient = @(($shortWide.Right - $shortWide.Left), ($shortWide.Bottom - $shortWide.Top))
            resizedWithPanelOpen = $true
            entryOptions = $shortWideLayout.group
            panel = $shortWideLayout.panel
            switchGeometrySamples = $shortSwitchSamples
            focusAfterEscape = 'jellyfin'
        }
        compact = [ordered]@{
            logicalClient = @(800, 400)
            physicalClient = @(($compact.Right - $compact.Left), ($compact.Bottom - $compact.Top))
            panel = $compactRegion
            youtubeHelp = $compactHelp
            returnArrow = $compactArrow
            returnedAndClearedAttempt = $true
            focusablePriorOptions = $oldFocusableOptions
            restoredOptions = $compactRail
            guestFits = $true
            focusAfterReturn = 'jellyfin'
        }
        exitCode = $app.ExitCode
    } | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $directory 'connection-layout-uia.json') -Encoding utf8
    'Windows UI Automation passed for wide and compact connection layouts.'
} catch {
    $tree = if ($root -and $app -and !$app.HasExited) {
        try {
            @($root.FindAll($scope, [System.Windows.Automation.Condition]::TrueCondition) | ForEach-Object { Describe-LayoutElement $_ })
        } catch [System.Windows.Automation.ElementNotAvailableException] {}
    }
    [ordered]@{ stage = $stage; error = $_.Exception.Message; tree = $tree } | ConvertTo-Json -Depth 5 |
        Set-Content -LiteralPath (Join-Path $directory 'connection-layout-uia.failure.json') -Encoding utf8
    throw
} finally {
    if ($app -and !$app.HasExited) {
        Stop-Process -Id $app.Id -ErrorAction SilentlyContinue
        [void]$app.WaitForExit(4000)
    }
    $resolvedProfile = (Resolve-Path -LiteralPath $profile).ProviderPath
    if ($resolvedProfile -eq $profile -and (Split-Path -Parent $resolvedProfile) -eq $directory) {
        Remove-Item -LiteralPath $resolvedProfile -Recurse
    }
}
