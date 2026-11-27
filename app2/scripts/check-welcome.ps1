param(
    [string]$Executable = "$PSScriptRoot/../target/debug/mediadeka.exe",
    [string]$Evidence = "$PSScriptRoot/../target/acceptance/stage-2",
    [hashtable]$JellyfinFixture,
    [switch]$SpotifyLive,
    [switch]$ExpectSpotifyFeed,
    [switch]$CheckHome,
    [switch]$CheckYoutubeSourceMenu,
    [switch]$SoundCloudLogin,
    [switch]$SoundCloudLive,
    [switch]$SoundCloudFeed,
    [string]$SoundCloudProfile,
    [string]$RestoredProfile,
    [string]$RestoredAccountSource,
    [switch]$ExpectOfflineCache
)

$ErrorActionPreference = 'Stop'
if ($ExpectSpotifyFeed -and !$SpotifyLive) { throw '-ExpectSpotifyFeed requires -SpotifyLive.' }
if ($CheckYoutubeSourceMenu -and !$CheckHome) { throw '-CheckYoutubeSourceMenu requires -CheckHome.' }
if ($SoundCloudProfile -and !$SoundCloudFeed) { throw '-SoundCloudProfile requires -SoundCloudFeed.' }
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
if (!('MediadekaWelcomeProbe' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;

public static class MediadekaWelcomeProbe
{
    [StructLayout(LayoutKind.Sequential)]
    public struct Point { public int X, Y; }
    [StructLayout(LayoutKind.Sequential)]
    public struct Rect { public int Left, Top, Right, Bottom; }
    [StructLayout(LayoutKind.Sequential)]
    private struct MinMaxInfo { public Point Reserved, MaxSize, MaxPosition, MinTrackSize, MaxTrackSize; }
    private delegate bool Visitor(IntPtr handle, IntPtr parameter);
    [DllImport("user32.dll")] private static extern bool EnumWindows(Visitor visit, IntPtr arg);
    [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr handle, out uint processId);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern int GetWindowText(IntPtr handle, StringBuilder title, int count);
    [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr handle);
    [DllImport("user32.dll")] public static extern bool IsZoomed(IntPtr handle);
    [DllImport("user32.dll")] public static extern bool ShowWindowAsync(IntPtr handle, int command);
    [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr handle);
    [DllImport("user32.dll")] private static extern bool GetClientRect(IntPtr handle, out Rect rect);
    [DllImport("user32.dll")] private static extern bool GetWindowRect(IntPtr handle, out Rect rect);
    [DllImport("user32.dll")] private static extern bool ClientToScreen(IntPtr handle, ref Point point);
    [DllImport("user32.dll")] private static extern bool SetWindowPos(IntPtr handle, IntPtr after, int x, int y, int width, int height, uint flags);
    [DllImport("user32.dll")] private static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
    [DllImport("user32.dll")] private static extern bool PostMessage(IntPtr handle, uint msg, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll")] private static extern IntPtr SendMessage(IntPtr handle, uint msg, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll")] private static extern IntPtr SendMessage(IntPtr handle, uint msg, IntPtr wParam, ref MinMaxInfo data);

    public static IntPtr Find(uint processId, string expectedTitle = "Mediadeka")
    {
        IntPtr result = IntPtr.Zero;
        EnumWindows((handle, _) => {
            GetWindowThreadProcessId(handle, out uint owner);
            if (owner != processId) return true;
            var title = new StringBuilder(128);
            GetWindowText(handle, title, title.Capacity);
            if (title.ToString() != expectedTitle) return true;
            result = handle;
            return false;
        }, IntPtr.Zero);
        return result;
    }

    public static bool Close(IntPtr handle) { return PostMessage(handle, 0x0010, IntPtr.Zero, IntPtr.Zero); }

    public static void Tab(IntPtr handle)
    {
        PostMessage(handle, 0x0100, new IntPtr(9), new IntPtr(1 | (15 << 16)));
        PostMessage(handle, 0x0101, new IntPtr(9), new IntPtr(unchecked((int)0xC00F0001)));
    }

    public static bool Escape(IntPtr handle)
    {
        const int escape = 0x1B;
        return PostMessage(handle, 0x0100, new IntPtr(escape), new IntPtr(0x00010001))
            && PostMessage(handle, 0x0101, new IntPtr(escape), new IntPtr(unchecked((int)0xC0010001)));
    }

    [DllImport("kernel32.dll")] private static extern uint GetCurrentThreadId();
    [DllImport("user32.dll")] private static extern bool AttachThreadInput(uint attach, uint to, bool enable);
    [DllImport("user32.dll")] private static extern bool GetKeyboardState(byte[] state);
    [DllImport("user32.dll")] private static extern bool SetKeyboardState(byte[] state);

    // GPUI reads modifiers from GetKeyState, which PostMessage does not
    // change. Attaching to the window thread lets SetKeyboardState expose the
    // Ctrl bit while the posted key messages are processed.
    public static bool CtrlK(IntPtr handle)
    {
        const int control = 0x11;
        const int key = 0x4B;
        uint processId;
        uint target = GetWindowThreadProcessId(handle, out processId);
        uint current = GetCurrentThreadId();
        bool attached = current != target && AttachThreadInput(current, target, true);
        var state = new byte[256];
        byte[] saved = null;
        try {
            if (attached && GetKeyboardState(state)) {
                saved = (byte[])state.Clone();
                state[control] = 0x80;
                state[0xA2] = 0x80;
                SetKeyboardState(state);
            }
            bool sent = PostMessage(handle, 0x0100, new IntPtr(control), new IntPtr(1 | (0x1D << 16)))
                && PostMessage(handle, 0x0100, new IntPtr(key), new IntPtr(1 | (0x25 << 16)))
                && PostMessage(handle, 0x0101, new IntPtr(key), new IntPtr(unchecked((int)(0xC0000001u | (0x25u << 16)))))
                && PostMessage(handle, 0x0101, new IntPtr(control), new IntPtr(unchecked((int)(0xC0000001u | (0x1Du << 16)))));
            System.Threading.Thread.Sleep(150);
            return sent;
        } finally {
            if (saved != null) { SetKeyboardState(saved); }
            if (attached) { AttachThreadInput(current, target, false); }
        }
    }

    private static IntPtr Coordinates(int x, int y) { return new IntPtr((x & 0xffff) | (y << 16)); }

    public static int DragHitTest(IntPtr handle, bool doubleClick)
    {
        var point = new Point { X = 120, Y = 24 };
        SendMessage(handle, 0x0200, IntPtr.Zero, Coordinates(point.X, point.Y));
        ClientToScreen(handle, ref point);
        var position = Coordinates(point.X, point.Y);
        var result = SendMessage(handle, 0x0084, IntPtr.Zero, position).ToInt32();
        if (doubleClick && result == 2) PostMessage(handle, 0x00A3, new IntPtr(2), position);
        return result;
    }

    public static Point MinimumTrack(IntPtr handle)
    {
        var data = new MinMaxInfo();
        SendMessage(handle, 0x0024, IntPtr.Zero, ref data);
        return data.MinTrackSize;
    }

    public static Rect ClientOnScreen(IntPtr handle)
    {
        var previous = SetThreadDpiAwarenessContext(new IntPtr(-4));
        try {
            GetClientRect(handle, out var rect);
            var point = new Point();
            ClientToScreen(handle, ref point);
            rect.Left += point.X; rect.Right += point.X;
            rect.Top += point.Y; rect.Bottom += point.Y;
            return rect;
        } finally {
            if (previous != IntPtr.Zero) SetThreadDpiAwarenessContext(previous);
        }
    }

    public static bool ResizeClient(IntPtr handle, int width, int height)
    {
        var previous = SetThreadDpiAwarenessContext(new IntPtr(-4));
        try {
            if (!GetClientRect(handle, out var client) || !GetWindowRect(handle, out var outer)) return false;
            return SetWindowPos(handle, IntPtr.Zero, 0, 0,
                width + outer.Right - outer.Left - client.Right,
                height + outer.Bottom - outer.Top - client.Bottom, 0x0016);
        } finally {
            if (previous != IntPtr.Zero) SetThreadDpiAwarenessContext(previous);
        }
    }
}
'@
}

$directory = (New-Item -ItemType Directory -Force -Path $Evidence).FullName
$exe = (Resolve-Path -LiteralPath $Executable).ProviderPath
if ($RestoredProfile) {
    if ($CheckHome) { throw 'The home check requires a new isolated guest profile.' }
    if (!$RestoredAccountSource) { throw 'Supply the expected account source for the test profile.' }
    $profile = (Resolve-Path -LiteralPath $RestoredProfile).ProviderPath
} elseif ($SoundCloudFeed) {
    $profile = if ($SoundCloudProfile) {
        (Resolve-Path -LiteralPath $SoundCloudProfile).ProviderPath
    } else {
        (New-Item -ItemType Directory -Path (Join-Path $directory 'soundcloud-profile') -Force).FullName
    }
} else {
    $profile = Join-Path $directory ('uia-profile-' + [guid]::NewGuid().ToString('N'))
    [void](New-Item -ItemType Directory -Path $profile)
}

function Start-Mediadeka([string]$TestProfile = $profile, [string]$Proxy, [string]$WebView2Arguments, [string]$TempDirectory) {
    $variables = @{ MEDIADEKA_PROFILE_DIR = $TestProfile }
    if ($WebView2Arguments) { $variables['WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS'] = $WebView2Arguments }
    if ($TempDirectory) { $variables['TEMP'] = $variables['TMP'] = $TempDirectory }
    if ($Proxy) {
        foreach ($key in 'HTTPS_PROXY', 'HTTP_PROXY', 'ALL_PROXY') {
            $variables[$key] = $Proxy
        }
        $variables['NO_PROXY'] = ''
    }
    $previous = @{}
    try {
        foreach ($key in $variables.Keys) {
            $previous[$key] = [Environment]::GetEnvironmentVariable($key, 'Process')
            [Environment]::SetEnvironmentVariable($key, $variables[$key], 'Process')
        }
        $launch = @{ FilePath = $exe; WorkingDirectory = $directory; PassThru = $true; WindowStyle = 'Hidden' }
        if ($TempDirectory) {
            $launch['RedirectStandardError'] = Join-Path $TempDirectory ('mediadeka-' + [guid]::NewGuid().ToString('N') + '.log')
        }
        Start-Process @launch
    } finally {
        foreach ($key in $previous.Keys) {
            [Environment]::SetEnvironmentVariable($key, $previous[$key], 'Process')
        }
    }
}

function Await([scriptblock]$probe, [string]$errorText, [int]$seconds = 12) {
    $deadline = [DateTime]::UtcNow.AddSeconds($seconds)
    do {
        try {
            $value = & $probe
            if ($value) { return $value }
        } catch [System.Windows.Automation.ElementNotAvailableException] {}
        Start-Sleep -Milliseconds 100
    } until ([DateTime]::UtcNow -ge $deadline)
    throw $errorText
}

$automation = [System.Windows.Automation.AutomationElement]
$scope = [System.Windows.Automation.TreeScope]::Descendants
function Descendants($root) { $root.FindAll($scope, [System.Windows.Automation.Condition]::TrueCondition) | ForEach-Object { $_ } }
function Element($root, [string]$id) {
    $condition = [System.Windows.Automation.PropertyCondition]::new($automation::AutomationIdProperty, $id)
    $root.FindFirst($scope, $condition)
}
function Invoke-Element($root, [string]$id) {
    $element = Await { Element $root $id } "UIA element missing: $id"
    $pattern = $null
    if ($element.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern, [ref]$pattern)) {
        $pattern.Invoke()
    } elseif ($element.TryGetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern, [ref]$pattern)) {
        $pattern.Toggle()
    } elseif ($element.TryGetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern, [ref]$pattern)) {
        $pattern.Select()
    } else {
        throw "Could not activate $id."
    }
}

function Assert-ConnectionNavigation($root, [switch]$Compact) {
    foreach ($id in 'cancel', 'back') {
        if (Element $root $id) { throw "Retired action $id remains in the UIA tree." }
    }
    $arrow = Element $root 'connection-return'
    if (!$Compact) {
        if ($arrow) { throw 'The wide login panel exposed a return button.' }
        return
    }
    if (!$arrow -or $arrow.Current.Name -cne 'Вернуться к списку' -or
        $arrow.Current.ControlType.ProgrammaticName -ne 'ControlType.Button' -or
        !$arrow.Current.IsKeyboardFocusable -or $arrow.Current.IsOffscreen) {
        throw 'The compact login panel did not expose its accessible return button.'
    }
}

function Wait-ConnectionClosed($root, [string]$provider) {
    [void](Await {
        $source = Element $root $provider
        $source -and $source.Current.HasKeyboardFocus -and !(Element $root 'login-panel')
    } "$provider did not return focus to the provider list after closing its panel.")
    Assert-ConnectionNavigation $root
}

function Send-WindowEscape([IntPtr]$handle) {
    if (![MediadekaWelcomeProbe]::Escape($handle)) { throw 'Could not send Escape to the Mediadeka window.' }
}

function Send-WindowCtrlK([IntPtr]$handle) {
    if (![MediadekaWelcomeProbe]::CtrlK($handle)) { throw 'Could not send Ctrl+K to the Mediadeka window.' }
}

function Selected($element) {
    $pattern = $null
    if ($element -and $element.TryGetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern, [ref]$pattern)) {
        $pattern.Current.IsSelected
    } else {
        $false
    }
}

function Cancel-PendingConnection($root, [IntPtr]$handle, [string]$provider, [switch]$Compact) {
    Assert-ConnectionNavigation $root -Compact:$Compact
    Send-WindowEscape $handle
    try {
        [void](Await {
            $panel = Element $root 'login-panel'
            $panel -and (Element $panel 'login-cancelled') -and (Element $panel 'retry') -and
                !(Element $panel 'authorize') -and !(Element $panel 'device-code')
        } "$provider did not report cancellation and clear the pending authorization after Escape.")
    } catch {
        $ids = @(Descendants $root | ForEach-Object { $_.Current.AutomationId } | Where-Object { $_ })
        throw "$($_.Exception.Message) UIA IDs: $($ids -join ', ')."
    }
    Assert-ConnectionNavigation $root -Compact:$Compact
    Send-WindowEscape $handle
    Wait-ConnectionClosed $root $provider
}

function Invoke-CompactReturn($root, [string]$provider) {
    Assert-ConnectionNavigation $root -Compact
    Invoke-Element $root 'connection-return'
    Wait-ConnectionClosed $root $provider
}
function Guest-Session($root) {
    $card = Element $root 'home-accounts'
    $card -and $card.Current.Name -ceq 'Подключения · Гостевой вход'
}
function Open-HomeSources($root) {
    if ((Element $root 'home-source-toggle') -and !(Element $root 'home-source-filters')) {
        Invoke-Element $root 'home-source-toggle'
        [void](Await { Element $root 'home-source-filters' } 'The platform disclosure did not show its filters.')
    }
}
function Home-SourceElement($root, [string]$id) {
    Open-HomeSources $root
    Element $root $id
}
function Guest-Home($root) {
    $chip = Home-SourceElement $root 'home-source-guest'
    $first = Home-FeedElement $root 'youtube-music' 'home-item' 0
    $chip -and $chip.Current.Name -like 'YouTube Music · *' -and
        (Guest-Session $root) -and
        $first -and $first.Current.Name
}
function Home-FeedElement($root, [string]$provider, [string]$kind, [int]$index = -1) {
    $suffix = if ($index -ge 0) { "-$index" } else { '' }
    $scoped = Element $root "$kind-$provider$suffix"
    if ($scoped) { return $scoped }
    Element $root "$kind$suffix"
}
function Select-HomeFilter($root, [string]$id) {
    $chip = Await { Home-SourceElement $root $id } "The home filter $id is missing."
    if (!(Selected $chip)) { Invoke-Element $root $id }
    [void](Await { Selected (Element $root $id) } "The home filter $id was not selected.")
}
function Toggle-SelectedHomeFilter($root, [string]$id) {
    Open-HomeSources $root
    if (!(Selected (Element $root $id))) { throw "The home filter $id was not selected before toggling." }
    for ($attempt = 0; $attempt -lt 3; $attempt++) {
        if (Selected (Element $root 'home-source-all')) { return }
        $chip = Await { Element $root $id } "The home filter $id is missing."
        $pattern = $chip.GetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern)
        if ($pattern.Current.IsSelected) { $pattern.RemoveFromSelection() }
        $until = [DateTime]::UtcNow.AddSeconds(2)
        do {
            if (Selected (Element $root 'home-source-all')) { return }
            Start-Sleep -Milliseconds 80
        } until ([DateTime]::UtcNow -ge $until)
    }
    throw 'Toggling the selected platform did not restore All.'
}
function Assert-Chips($root) {
    $toggle = Element $root 'home-source-toggle'
    if (!$toggle -or $toggle.Current.ControlType.ProgrammaticName -ne 'ControlType.Button' -or
        !$toggle.Current.Name.StartsWith('Площадки · ')) {
        throw 'The home page does not expose its platform disclosure.'
    }
    Open-HomeSources $root
    $sources = Element $root 'home-source-filters'
    if (!$sources -or $sources.Current.ControlType.ProgrammaticName -ne 'ControlType.Tab' -or
        $sources.Current.Name -cne 'Фильтр площадок') {
        throw 'The home page does not expose its platform tab list.'
    }
    $guest = Element $root 'home-source-guest'
    $all = Element $root 'home-source-all'
    $account = Descendants $root | Where-Object {
        $_.Current.AutomationId -match '^home-source-account-'
    } | Select-Object -First 1
    if (!$guest -and !$account) { throw 'The home page has no platform chip.' }
    if (!$all -or $all.Current.Name -cne 'Все') { throw 'The home page has no All chip.' }
    [void](Await { Element $root 'refresh' } 'The home page has no refresh action.' 45)
    if (!(Element $root 'home-add-account')) { throw 'The home page has no connections action.' }
}
function Read-HomeChips($root) {
    Open-HomeSources $root
    $group = Element $root 'home-sources'
    @(Descendants $group | Where-Object { $_.Current.AutomationId -match '^home-(source-(all$|guest$|account-)|connect-)' } |
        ForEach-Object {
            [ordered]@{
                id = $_.Current.AutomationId
                name = $_.Current.Name
                role = $_.Current.ControlType.ProgrammaticName
                selected = (Selected $_)
            }
        })
}
function Test-Shell($root, [IntPtr]$handle) {
    foreach ($id in 'home-page', 'home-search-page', 'home-history', 'home-liked', 'home-playlists', 'home-settings') {
        $node = Await { Element $root $id } "Navigation item $id is missing."
        if ($node.Current.ControlType.ProgrammaticName -ne 'ControlType.Button' -or
            !$node.Current.IsKeyboardFocusable -or !$node.Current.Name) {
            throw "Navigation item $id has unexpected semantics."
        }
    }
    $label = Element $root 'home-library-label'
    if (!$label -or $label.Current.ControlType.ProgrammaticName -eq 'ControlType.Button' -or
        $label.Current.IsKeyboardFocusable -or
        ($label.Current.Name -and $label.Current.Name -cne 'Медиатека')) {
        $name = if ($label) { $label.Current.Name } else { '<missing>' }
        $role = if ($label) { $label.Current.ControlType.ProgrammaticName } else { '<missing>' }
        $focusable = if ($label) { $label.Current.IsKeyboardFocusable } else { '<missing>' }
        throw "The library label is focusable or is a button: name=$name role=$role focusable=$focusable."
    }
    $card = Await { Element $root 'home-accounts' } 'The connections card is missing.'
    if ($card.Current.ControlType.ProgrammaticName -ne 'ControlType.Button' -or
        $card.Current.Name -cne 'Подключения · Гостевой вход') {
        throw "The connections card is not the guest connection: $($card.Current.Name)."
    }
    $field = Await { Element $root 'home-search' } 'The global search field is missing.'
    if ($field.Current.ControlType.ProgrammaticName -ne 'ControlType.Edit' -or
        !$field.Current.IsKeyboardFocusable -or $field.Current.Name -cne 'Поиск по всей музыке') {
        throw 'The global search field has unexpected semantics.'
    }
    Assert-Chips $root
    if (!(Selected (Element $root 'home-source-all')) -or (Selected (Element $root 'home-source-guest'))) {
        throw 'Home did not open with All selected.'
    }
    Select-HomeFilter $root 'home-source-guest'
    Toggle-SelectedHomeFilter $root 'home-source-guest'
    $card.SetFocus()
    [void](Await { (Element $root 'home-accounts').Current.HasKeyboardFocus } 'UIA did not focus the connections card.')
    Send-WindowCtrlK $handle
    [void](Await { (Element $root 'home-search').Current.HasKeyboardFocus } 'Ctrl+K did not focus the global search field.' 8)
    Send-WindowEscape $handle
    [void](Await { (Element $root 'home-accounts').Current.HasKeyboardFocus } 'Esc did not return focus from the global search field.' 8)
}
function Assert-NoWelcome($root) {
    if ((Element $root 'entry-options') -or (Element $root 'guest')) {
        throw 'Welcome appeared in an existing music session.'
    }
}
function Wait-ConnectionSettings($root) {
    [void](Await {
        Assert-NoWelcome $root
        $tab = Element $root 'settings-connections'
        $tab -and $tab.Current.Name -eq 'Подключения' -and !(Element $root 'settings-login-modal')
    } 'Connection settings did not open inside the music shell.')
    $sourceSwitch = Descendants $root | Where-Object {
        $_.Current.AutomationId -match '^home-source-(account-|guest$|all$)'
    } | Select-Object -First 1
    if ((Element $root 'home-sources') -or $sourceSwitch) {
        throw 'Connection settings exposed the home platform chips.'
    }
    [void](Await {
        foreach ($provider in 'spotify', 'youtube-music', 'yandex-music', 'soundcloud', 'jellyfin') {
            $signIn = Element $root "settings-sign-in-$provider"
            $signOut = Element $root "settings-sign-out-$provider"
            if ([bool]$signIn -eq [bool]$signOut) { return $false }
            $action = if ($signIn) { $signIn } else { $signOut }
            $pattern = $null
            if ($action.Current.ControlType.ProgrammaticName -ne 'ControlType.Button' -or
                !$action.Current.IsKeyboardFocusable -or
                !$action.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern, [ref]$pattern)) {
                return $false
            }
        }
        $true
    } 'Connection settings did not expose one available action per provider.' 25)
}
function Open-ConnectionSettings($root, [switch]$FromSourceMenu) {
    if ($FromSourceMenu) {
        Open-HomeSources $root
        Invoke-Element $root 'home-add-account'
    }
    else { Invoke-Element $root 'home-settings' }
    Wait-ConnectionSettings $root
}
function Open-SettingsLogin($root, [string]$provider) {
    Invoke-Element $root "settings-sign-in-$provider"
    [void](Await {
        Assert-NoWelcome $root
        $modal = Element $root 'settings-login-modal'
        $modal -and (Element $modal 'settings-close-login')
    } "Settings did not open a modal for $provider.")
}
function Close-SettingsLogin($root) {
    Invoke-Element (Element $root 'settings-login-modal') 'settings-close-login'
    Wait-ConnectionSettings $root
}
function Find-SoundCloudWindow($app) {
    $handle = [MediadekaWelcomeProbe]::Find($app.Id, 'Вход в SoundCloud — Mediadeka')
    if ($handle -ne [IntPtr]::Zero) { $handle }
}
function Wait-SoundCloudPage($app) {
    $handle = Await { Find-SoundCloudWindow $app } 'Embedded SoundCloud window did not open.' 25
    $document = Await {
        $view = $automation::FromHandle($handle)
        Descendants $view | Where-Object {
            $_.Current.ControlType.ProgrammaticName -eq 'ControlType.Document' -and
            $_.Current.Name -match 'SoundCloud' -and !$_.Current.IsOffscreen
        } | Select-Object -First 1
    } 'The embedded page did not expose a visible SoundCloud document.' 30
    [pscustomobject]@{ handle = $handle; document = $document.Current.Name }
}
function Wait-SoundCloudActivity($root, [switch]$Cached) {
    Await {
        $notice = Element $root 'home-empty-notice'
        if ($notice) { throw "SoundCloud feed failed: $($notice.Current.Name)" }
        $feed = Element $root 'home-activity-feed'
        if (!$feed -or $feed.Current.Name -cne 'Лента подписок' -or
            $Cached.IsPresent -ne [bool](Element $root 'home-cache-notice')) {
            return
        }
        $heading = Element $feed 'home-activity-title'
        if (!$heading -or $heading.Current.Name -cne 'Лента подписок' -or $heading.Current.IsOffscreen) {
            throw 'The SoundCloud activity shelf has no visible heading.'
        }
        $rows = @(Descendants $feed | Where-Object { $_.Current.AutomationId -match '^home-item-\d+$' } |
            Sort-Object { [int]($_.Current.AutomationId -replace '^home-item-', '') })
        if ($rows.Count -eq 0) { return }
        $covers = @(Descendants $feed | Where-Object { $_.Current.AutomationId -match '^home-artwork-\d+$' })
        $actions = @($rows | Where-Object { $_.Current.ControlType.ProgrammaticName -eq 'ControlType.Button' })
        if ($rows.Count -gt 24 -or $rows.Count -ne $covers.Count -or !$rows[0].Current.Name -or
            $rows[0].Current.IsOffscreen -or !$actions.Count) {
            throw 'SoundCloud rows or their covers have unexpected semantics.'
        }
        [pscustomobject]@{
            titles = @($rows | ForEach-Object { $_.Current.Name })
            covers = $covers.Count
            actions = $actions.Count
        }
    } 'The SoundCloud activity shelf did not show the expected track rows.' 45
}
function Wait-SoundCloudShelves($root, [string]$id, [string]$label, [switch]$Cached) {
    Await {
        $content = Element $root $id
        if (!$content -or $content.Current.Name -cne $label -or
            $Cached.IsPresent -ne [bool](Element $root 'home-cache-notice')) {
            return
        }
        $sections = @(Descendants $content | Where-Object { $_.Current.AutomationId -match '^home-section-\d+$' } |
            Sort-Object { [int]($_.Current.AutomationId -replace '^home-section-', '') })
        $empty = [bool](Element $content 'home-empty-tracks')
        $cards = @(Descendants $content | Where-Object { $_.Current.AutomationId -match '^home-item-\d+$' })
        if ($sections.Count -eq 0 -and !$empty -and $cards.Count -eq 0) {
            [pscustomobject]@{ titles = @(); cards = 0; actions = 0; empty = $true }
            return
        }
        if ($sections.Count -eq 0 -and !$empty) { return }
        $links = @($cards | Where-Object { $_.Current.ControlType.ProgrammaticName -eq 'ControlType.Button' })
        if (!$empty -and (!$cards.Count -or !$links.Count -or
            !($cards | Where-Object { $_.Current.Name -and !$_.Current.IsOffscreen } | Select-Object -First 1))) {
            throw "The SoundCloud page '$label' has no visible, named card or track action."
        }
        if ($Cached -and !(Element $root 'refresh')) { throw "The cached SoundCloud page '$label' has no retry action." }
        [pscustomobject]@{
            titles = @($sections | ForEach-Object { $_.Current.Name })
            cards = $cards.Count
            actions = $links.Count
            empty = $empty
        }
    } "The SoundCloud page '$label' did not open." 45
}
function Read-SoundCloudPages($root, [switch]$Cached) {
    $recommendations = Wait-SoundCloudShelves $root 'home-feed' 'Главная SoundCloud' -Cached:$Cached
    $activity = Wait-SoundCloudActivity $root -Cached:$Cached
    if ($Cached -and !(Element $root 'refresh')) { throw 'The offline SoundCloud feed has no retry action.' }
    $library = Wait-SoundCloudShelves $root 'home-library-content' 'Медиатека SoundCloud' -Cached:$Cached
    [pscustomobject]@{ recommendations = $recommendations; activity = $activity; library = $library }
}
function SoundCloud-LibraryCardIds($root) {
    $content = Element $root 'home-library-content'
    if (!$content) { return '' }
    $cards = @(Descendants $content | Where-Object { $_.Current.AutomationId -match '^home-item-\d+$' } |
        ForEach-Object { $_.Current.AutomationId })
    $cards -join ','
}
function Check-SoundCloudShelfNavigation($root) {
    [void](Wait-SoundCloudShelves $root 'home-library-content' 'Медиатека SoundCloud')
    $next = Descendants (Element $root 'home-library-content') | Where-Object {
        $_.Current.Name -ceq 'Следующие записи' -and !$_.Current.IsOffscreen
    } | Select-Object -First 1
    if ($next) {
        $before = SoundCloud-LibraryCardIds $root
        $next.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke()
        [void](Await {
            $after = SoundCloud-LibraryCardIds $root
            $after -and $after -cne $before
        } 'The next SoundCloud shelf page did not show a different set of cards.')
        $previous = Await {
            Descendants (Element $root 'home-library-content') | Where-Object {
                $_.Current.Name -ceq 'Предыдущие записи' -and !$_.Current.IsOffscreen
            } | Select-Object -First 1
        } 'The SoundCloud shelf did not expose its previous-page action.'
        $previous.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke()
        [void](Await { (SoundCloud-LibraryCardIds $root) -ceq $before } 'The SoundCloud shelf did not return to its initial cards.')
    }
    [bool]$next
}
function Return-FromConnectionSettings($root) {
    Invoke-Element $root 'home-page'
    [void](Await { (Element $root 'home-sources') -and !(Element $root 'settings-connections') } 'The sidebar did not return from settings to the home page.')
}
function Assert-Closed($app) {
    if (!$app.WaitForExit(8000) -or $app.ExitCode -ne 0) {
        throw "Window close failed ($($app.ExitCode))."
    }
}
function Get-Root($app) {
    $handle = Await {
        if ($app.HasExited) { throw "Mediadeka exited ($($app.ExitCode))." }
        $h = [MediadekaWelcomeProbe]::Find($app.Id)
        if ($h -ne [IntPtr]::Zero) { $h }
    } 'Mediadeka window unavailable.'
    [void][MediadekaWelcomeProbe]::ShowWindowAsync($handle, 5)
    $root = Await {
        if ($app.HasExited) { throw "Mediadeka exited ($($app.ExitCode))." }
        $candidate = $automation::FromHandle($handle)
        if (Element $candidate 'close') { $candidate }
    } 'Mediadeka did not publish its accessibility tree.'
    @($handle, $root)
}

function Set-Field($root, [string]$id, [string]$expectedValue) {
    $field = Await { Element $root $id } "UIA field missing: $id"
    $pattern = $null
    if (!$field.TryGetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern, [ref]$pattern)) {
        throw "UIA Value pattern missing: $id."
    }
    $pattern.SetValue($expectedValue)
    if ($field.Current.IsPassword) {
        if ($pattern.Current.Value) { throw "Protected UIA field exposed a value: $id." }
    } else {
        [void](Await { $pattern.Current.Value -ceq $expectedValue } "UIA field did not take the value: $id.")
    }
}

function Read-HomeLayout($root, [IntPtr]$handle, [int]$width, [int]$height) {
    $dpi = [MediadekaWelcomeProbe]::GetDpiForWindow($handle)
    $physicalWidth = [int][Math]::Round($width * $dpi / 96)
    $physicalHeight = [int][Math]::Round($height * $dpi / 96)
    if (![MediadekaWelcomeProbe]::ResizeClient($handle, $physicalWidth, $physicalHeight)) {
        throw "Could not resize the home window to $width × $height."
    }
    $client = Await {
        $bounds = [MediadekaWelcomeProbe]::ClientOnScreen($handle)
        if ($bounds.Right - $bounds.Left -eq $physicalWidth -and $bounds.Bottom - $bounds.Top -eq $physicalHeight) {
            $bounds
        }
    } "The home window did not resize to $width × $height."
    if (Element $root 'home-source-filters') {
        Invoke-Element $root 'home-source-toggle'
        [void](Await { !(Element $root 'home-source-filters') } 'The platform disclosure did not collapse.')
    }
    $required = @('home-navigation', 'home-search', 'home-accounts', 'home-sources', 'home-source-toggle', 'home-transport', 'home-page', 'home-search-page', 'home-history', 'home-liked', 'home-playlists', 'close')
    if ($width -gt 800) { $required += 'home-item-0', 'home-artwork-0' }
    $controls = foreach ($id in $required) {
        Await {
            $node = Element $root $id
            if ($node) {
                $state = $node.Current
                $bounds = $state.BoundingRectangle
                if (!$state.IsOffscreen -and !$bounds.IsEmpty -and
                    $bounds.Left -ge ($client.Left - 2) -and $bounds.Top -ge ($client.Top - 2) -and
                    $bounds.Right -le ($client.Right + 2) -and $bounds.Bottom -le ($client.Bottom + 2)) {
                    [ordered]@{
                        id = $id; name = $state.Name; role = $state.ControlType.ProgrammaticName
                        focusable = $state.IsKeyboardFocusable
                        bounds = @(($bounds.Left - $client.Left), ($bounds.Top - $client.Top), $bounds.Width, $bounds.Height)
                    }
                }
            }
        } "$id is missing or clipped at $width × $height."
    }
    $preview = $controls | Where-Object id -eq 'home-item-0'
    $artwork = $controls | Where-Object id -eq 'home-artwork-0'
    $transport = $controls | Where-Object id -eq 'home-transport'
    $barNode = Element $root 'home-scrollbar'
    if (!$barNode) { throw "The home scroll viewport is missing at $width × $height." }
    $barBounds = $barNode.Current.BoundingRectangle
    $scrollBottom = $barBounds.Bottom - $client.Top
    if ($scrollBottom -gt ($transport.bounds[1] + 2)) {
        throw "The home scroll viewport overlaps the transport at $width × $height."
    }
    if ($preview -and $preview.bounds[1] -ge $scrollBottom) {
        throw "The first public collection item starts below the scroll viewport at $width × $height."
    }
    if ($artwork -and [Math]::Abs($artwork.bounds[2] - $artwork.bounds[3]) -gt 2) {
        throw "The recommendation cover is not square at $width × $height."
    }
    $transportHeight = $transport.bounds[3] * 96 / $dpi
    # The compact transport is 58 DIP of minimum height plus its padding and border.
    $expectedTransport = if ($width -eq 800) { 61 } else { 76 }
    if ([Math]::Abs($transportHeight - $expectedTransport) -gt 2) {
        throw "Transport height was $transportHeight instead of about $expectedTransport at $width × $height."
    }
    $bottomGap = ($physicalHeight - $transport.bounds[1] - $transport.bounds[3]) * 96 / $dpi
    if ($bottomGap -lt 10) { throw "The transport has only $bottomGap units below it at $width × $height." }
    if (!(Element $root 'home-feed')) { throw 'The recommendation feed is missing from the accessibility tree.' }
    if (Element $root 'home-hero') { throw 'The retired hero remains in the home accessibility tree.' }
    [ordered]@{
        width = $width; height = $height; dpi = $dpi; controls = @($controls)
        transportHeight = $transportHeight; transportBottomGap = $bottomGap
    }
}

function Read-HomeRightScrollbar($root, [IntPtr]$handle) {
    Await {
        $bar = Element $root 'home-scrollbar'
        if ($bar) {
            $client = [MediadekaWelcomeProbe]::ClientOnScreen($handle)
            $bounds = $bar.Current.BoundingRectangle
            $rightGap = $client.Right - $bounds.Right
            $maxGap = 36 * [MediadekaWelcomeProbe]::GetDpiForWindow($handle) / 96
            $role = $bar.Current.ControlType.ProgrammaticName
            if ($bounds.IsEmpty -or $bounds.Height -le $bounds.Width -or $rightGap -lt -2 -or
                $rightGap -gt $maxGap -or $role -ne 'ControlType.ScrollBar') {
                throw 'The vertical scrollbar is incorrectly placed or described.'
            }
            [ordered]@{
                rightGap = $rightGap; height = $bounds.Height; width = $bounds.Width
                role = $role
            }
        }
    } 'The main scrolling area did not expose its right-hand scrollbar.'
}

function Add-YouTubeMenuFixture([string]$TestProfile) {
    $node = (Get-Command node -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
    $fixtureSql = @'
const { DatabaseSync } = require('node:sqlite');
const db = new DatabaseSync(process.argv[1]);
db.exec('PRAGMA foreign_keys = ON; BEGIN IMMEDIATE;');
try {
  const route = db.prepare('SELECT kind FROM startup WHERE id = 1').get();
  if (route?.kind !== 'guest') throw new Error('Expected the isolated guest profile.');
  const provider = 'youtube-music';
  const profile = 'uia-menu-test-profile';
  const capabilities = JSON.stringify({home: true, profile_selection: true, device_code: false});
  db.prepare('INSERT INTO account VALUES (?, ?, ?, ?, ?)').run(provider, profile, 'UIA fixture', capabilities, '39f863e1ac704edeb178f699dfbba459');
  db.prepare('INSERT INTO account_source VALUES (?, ?)').run(provider, profile);
  db.exec('COMMIT;');
} catch (error) {
  db.exec('ROLLBACK;');
  throw error;
} finally {
  db.close();
}
'@
    & $node -e $fixtureSql (Join-Path $TestProfile 'mediadeka.sqlite3')
    if ($LASTEXITCODE -ne 0) { throw 'Could not create the isolated YouTube Music menu fixture.' }
}

function Check-Home {
    $app = $null
    $root = $null
    $unresponsiveProxy = $null
    $cold = Join-Path $directory ('uia-home-cold-' + [guid]::NewGuid().ToString('N'))
    $stage = 'fresh'
    try {
        $app = Start-Mediadeka
        $handle, $root = Get-Root $app
        [void](Await { Element $root 'guest' } 'The fresh profile did not open welcome.')
        Invoke-Element $root 'guest'
        [void](Await { Guest-Home $root } 'Guest home did not open with public music.' 45)
        if (!(Selected (Element $root 'home-source-all'))) { throw 'Guest home did not open in All.' }
        Select-HomeFilter $root 'home-source-guest'
        Toggle-SelectedHomeFilter $root 'home-source-guest'
        if (Element $root 'home-cache-notice') { throw 'A new guest profile used data from another profile.' }
        if (Element $root 'home-activity-title') { throw 'SoundCloud activity is visible on the guest source.' }
        $wide = Read-HomeLayout $root $handle 1500 850
        $compact = Read-HomeLayout $root $handle 800 400

        $stage = 'recommendation pagination'
        $nextName = [System.Windows.Automation.PropertyCondition]::new($automation::NameProperty, 'Следующие записи')
        $previousName = [System.Windows.Automation.PropertyCondition]::new($automation::NameProperty, 'Предыдущие записи')
        $nextPage = $root.FindFirst($scope, $nextName)
        $paginated = $false
        if ($nextPage) {
            $nextPage.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke()
            [void](Await {
                !(Element $root 'home-item-0') -and
                    (Descendants $root | Where-Object { $_.Current.AutomationId -match '^home-item-\d+$' } | Select-Object -First 1)
            } 'The next recommendation page did not replace the listed items.')
            $previousPage = Await { $root.FindFirst($scope, $previousName) } 'The recommendation shelf did not expose its previous-page button.'
            $previousPage.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke()
            [void](Await { Element $root 'home-item-0' } 'Recommendation pagination did not return to the first item.')
            $paginated = $true
        }

        $stage = 'search'
        $search = Element $root 'home-search'
        if ($search.Current.ControlType.ProgrammaticName -ne 'ControlType.Edit' -or
            !$search.Current.IsKeyboardFocusable -or $search.Current.Name -cne 'Поиск по всей музыке') {
            throw 'The global search field is not an accessible edit control.'
        }
        $shellCard = Element $root 'home-accounts'
        $shellCard.SetFocus()
        [void](Await { (Element $root 'home-accounts').Current.HasKeyboardFocus } 'UIA did not focus the connections card.')
        Send-WindowCtrlK $handle
        [void](Await { (Element $root 'home-search').Current.HasKeyboardFocus } 'Ctrl+K did not focus the search field.' 8)
        $first = Element $root 'home-item-0'
        $title = $first.Current.Name
        if ($first.Current.ControlType.ProgrammaticName -ne 'ControlType.Button') {
            throw 'The public track link is not an accessible button.'
        }
        [void]$first.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern)
        Set-Field $root 'home-search' $title
        [void](Await {
            $match = Element $root 'home-item-0'
            $match -and $match.Current.Name -ceq $title -and
                (Element $root 'home-feed')
        } 'Typing into the global search field filtered the loaded collection.')
        $valuePattern = (Element $root 'home-search').GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern)
        Send-WindowEscape $handle
        [void](Await { $valuePattern.Current.Value -eq '' } 'Esc did not clear the global search field.')
        [void](Await { (Element $root 'home-accounts').Current.HasKeyboardFocus } 'Esc did not return focus to the connections card.' 8)
        Send-WindowCtrlK $handle
        [void](Await { (Element $root 'home-search').Current.HasKeyboardFocus } 'Ctrl+K did not focus the field again.' 8)
        Set-Field $root 'home-search' 'mediadeka acceptance no match 2789fbf4'
        Send-WindowEscape $handle
        [void](Await { $valuePattern.Current.Value -eq '' } 'Esc did not restore the empty route query.')
        Invoke-Element $root 'home-playlists'
        [void](Await { Selected (Element $root 'library-tab-playlists') } 'The sidebar did not open the playlists library.')
        Invoke-Element $root 'home-page'
        [void](Await { (Element $root 'home-feed') -and (Element $root 'home-item-0') } 'The sidebar did not return to the recommendation feed.')
        $scrollbar = Read-HomeRightScrollbar $root $handle
        Invoke-Element $root 'close'
        Assert-Closed $app

        $stage = 'restart'
        $app = Start-Mediadeka
        $handle, $root = Get-Root $app
        [void](Await {
            Assert-NoWelcome $root
            Guest-Home $root
        } 'The guest home did not restore directly after restart.' 45)
        $lastTitle = (Element $root 'home-item-0').Current.Name
        Invoke-Element $root 'close'
        Assert-Closed $app

        # Keep the loopback connection open without handling HTTP. This exposes the
        # startup view while the normal provider's network call is outstanding.
        $unresponsiveProxy = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
        $unresponsiveProxy.Start()
        $proxy = "http://127.0.0.1:$($unresponsiveProxy.LocalEndpoint.Port)"

        $stage = 'cached while refreshing'
        $app = Start-Mediadeka -Proxy $proxy
        $handle, $root = Get-Root $app
        [void](Await {
            $unresponsiveProxy.Pending() -and (Guest-Home $root) -and
                !(Element $root 'home-cache-notice') -and !(Element $root 'home-skeleton')
        } 'An available guest snapshot did not appear while the startup network was pending.' 8)
        Invoke-Element $root 'close'
        Assert-Closed $app

        $stage = 'cached'
        $app = Start-Mediadeka -Proxy 'http://127.0.0.1:1'
        $handle, $root = Get-Root $app
        [void](Await { (Guest-Home $root) -and (Element $root 'home-cache-notice') } 'Offline restart did not restore the SQLite guest collection.' 45)
        if ((Element $root 'home-item-0').Current.Name -cne $lastTitle) { throw 'Offline restart changed the cached collection.' }
        $stage = 'settings modal cancellation'
        Open-ConnectionSettings $root -FromSourceMenu
        Open-SettingsLogin $root 'jellyfin'
        [void](Await {
            $modal = Element $root 'settings-login-modal'
            $modal -and (Element $modal 'jellyfin-server')
        } 'Jellyfin did not expose its login fields inside the settings modal.')
        Close-SettingsLogin $root
        Return-FromConnectionSettings $root
        [void](Await { (Guest-Home $root) -and (Element $root 'home-cache-notice') } 'Cancelling account addition did not preserve the offline guest collection.')
        if ((Element $root 'home-item-0').Current.Name -cne $lastTitle) { throw 'Cancelling account addition changed the cached guest collection.' }
        Invoke-Element $root 'close'
        Assert-Closed $app

        $stage = 'restart after cancelling account addition'
        $app = Start-Mediadeka -Proxy 'http://127.0.0.1:1'
        $handle, $root = Get-Root $app
        [void](Await { (Guest-Home $root) -and (Element $root 'home-cache-notice') } 'Cancelling account addition discarded the saved guest session.')
        Invoke-Element $root 'close'
        Assert-Closed $app

        $stage = 'empty'
        $app = Start-Mediadeka -Proxy 'http://127.0.0.1:1' -TestProfile $cold
        $handle, $root = Get-Root $app
        [void](Await { Element $root 'guest' } 'The isolated guest profile did not open welcome.')
        Invoke-Element $root 'guest'
        [void](Await { (Element $root 'home-empty-notice') -and (Element $root 'home-sources') } 'An empty offline profile did not show an inline home error.' 45)
        Invoke-Element $root 'close'
        Assert-Closed $app

        $stage = 'startup skeleton'
        $app = Start-Mediadeka -Proxy $proxy -TestProfile $cold
        $handle, $root = Get-Root $app
        [void](Await {
            (Element $root 'home-skeleton') -and (Element $root 'home-navigation') -and
                (Element $root 'home-search') -and (Element $root 'home-transport')
        } 'The cold startup did not expose the skeleton inside the music shell.' 8)
        [void](Await { (Element $root 'home-empty-notice') -and !(Element $root 'home-skeleton') } 'The startup skeleton did not settle to the inline error.' 45)

        $stage = 'retry'
        [void](Await { Home-SourceElement $root 'refresh' } 'The refresh action is missing.')
        Invoke-Element $root 'refresh'
        [void](Await {
            $chip = Element $root 'home-source-guest'
            $chip -and $chip.Current.Name -like '* · Обновляется' -and
                (Element $root 'home-navigation') -and (Element $root 'home-search')
        } 'Retry did not expose a pending state inside the home shell.')
        [void](Await { (Element $root 'home-empty-notice') -and (Element $root 'home-navigation') } 'Retry did not stay in the home shell.' 45)
        Invoke-Element $root 'close'
        Assert-Closed $app

        $youtubeGuestHidden = $null
        if ($CheckYoutubeSourceMenu) {
            $stage = 'YouTube Music account chips'
            Add-YouTubeMenuFixture $cold
            $app = Start-Mediadeka -Proxy 'http://127.0.0.1:1' -TestProfile $cold
            $handle, $root = Get-Root $app
            [void](Await {
                $source = Home-SourceElement $root 'home-source-account-youtube-music'
                Assert-NoWelcome $root
                $source -and $source.Current.Name.StartsWith('YouTube Music · UIA fixture') -and
                    !(Element $root 'home-skeleton')
            } 'A saved YouTube Music profile did not replace the obsolete guest startup route.')
            if (Element $root 'home-source-guest') { throw 'The guest chip is still visible alongside a saved YouTube Music profile.' }
            $youtubeGuestHidden = $true
            Invoke-Element $root 'close'
            Assert-Closed $app
        }

        [ordered]@{
            source = 'YouTube Music · Гостевой вход'
            fresh = $true; restored = $true; sqliteCacheRestored = $true
            coldError = $true; retryStayedInHome = $true; cancelledAccountAddition = $true
            squareRecommendations = $true; recommendationPagination = $paginated
            searchedTrack = $title; noMatches = $true; accessibleTrackLink = $true
            cachedBeforeNetwork = $true; coldStartupSkeleton = $true; youtubeGuestHidden = $youtubeGuestHidden
            rightScrollbar = $scrollbar
            sidebarNavigation = $true; wide = $wide; compact = $compact
        } | ConvertTo-Json -Depth 7 | Set-Content -LiteralPath (Join-Path $directory 'home-uia.json') -Encoding utf8
        Remove-Item -LiteralPath (Join-Path $directory 'home-uia-failure.txt') -ErrorAction SilentlyContinue
        'Windows UI Automation passed for the music shell, startup skeleton, cached startup, scrollbar, search, source selection and transport.'
    } catch {
        if ($root -and $app -and !$app.HasExited) {
            $ids = @(Descendants $root | ForEach-Object { $_.Current.AutomationId } | Where-Object { $_ })
            "Phase: $stage. UIA ids: $($ids -join ', ')." | Set-Content -LiteralPath (Join-Path $directory 'home-uia-failure.txt') -Encoding utf8
        }
        throw
    } finally {
        if ($app -and !$app.HasExited) {
            Stop-Process -Id $app.Id -ErrorAction SilentlyContinue
            [void]$app.WaitForExit(5000)
        }
        if ($unresponsiveProxy) { $unresponsiveProxy.Stop() }
        foreach ($testProfile in @($profile, $cold)) {
            if (!(Test-Path -LiteralPath $testProfile)) { continue }
            $item = Get-Item -LiteralPath $testProfile
            if ($item.FullName -ne $testProfile -or $item.Parent.FullName -ne $directory -or
                ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
                throw 'Home check refused to remove a profile outside its evidence directory.'
            }
            Remove-Item -LiteralPath $item.FullName -Recurse -Force
        }
    }
}

if ($CheckHome) {
    Check-Home
    return
}

function Check-JellyfinFixture {
    $expectedSource = "Jellyfin · $($JellyfinFixture.User)"
    function Read-LibraryNode($node) {
        $current = $node.Current
        [ordered]@{
            id = $current.AutomationId
            name = $current.Name
            role = $current.ControlType.ProgrammaticName
            focusable = $current.IsKeyboardFocusable
        }
    }
    function Wait-CombinedLibraryTrack($root) {
        Await {
            $node = Element $root 'library-row-0'
            if ($node -and $node.Current.Name -like "*$($JellyfinFixture.Track)*" -and
                $node.Current.Name -match '(Mediadeka, Jellyfin|Jellyfin, Mediadeka)') { $node }
        } 'The Jellyfin and Mediadeka marks did not appear together on one recording.' 30
    }
    function Wait-LikeDefaults($root) {
        [void](Await {
            $local = Element $root 'like-preference-mediadeka'
            $remote = Element $root 'like-preference-jellyfin'
            $local -and $remote -and $local.Current.Name -ceq 'Mediadeka · Выбрано' -and
                $remote.Current.Name -ceq 'Jellyfin · Выбрано'
        } 'Settings did not display both default like destinations.')
    }
    function Read-JellyfinLibrary($root) {
        Open-ConnectionSettings $root
        Invoke-Element $root 'like-preference-jellyfin'
        Wait-LikeDefaults $root
        Invoke-Element $root 'home-liked'
        [void](Await { Selected (Element $root 'library-tab-tracks') } 'The Tracks tab did not select.')
        Invoke-Element $root 'library-filters'
        Select-HomeFilter $root 'liked-source-jellyfin'
        Invoke-Element $root 'library-filters'
        Set-Field $root 'music-filter' 'Jellyfin Acceptance'
        $row = Await {
            $node = Element $root 'library-row-0'
            if ($node -and $node.Current.Name -like "*$($JellyfinFixture.Track)*" -and $node.Current.Name -like '*Jellyfin*') { $node }
        } 'The unified list did not expose the saved Jellyfin recording.'
        $rowState = Read-LibraryNode $row
        Invoke-Element $root 'library-like-0'
        $combined = Read-LibraryNode (Wait-CombinedLibraryTrack $root)
        $counts = Read-LibraryNode (Await { Element $root 'library-counts' } 'The track summary is missing.')
        $action = Await {
            Descendants $root | Where-Object {
                $_.Current.Name -ceq 'Куда сохранить лайк' -and $_.Current.AutomationId -like 'library-like-menu-*'
            } | Select-Object -First 1
        } 'The track row did not expose the like destination action.'
        Invoke-Element $root $action.Current.AutomationId
        $menu = Await { Element $root 'like-destinations-menu' } 'The like destinations menu did not open.'
        $menuState = Read-LibraryNode $menu
        if ($menuState.role -cne 'ControlType.Menu') { throw 'The like overlay did not expose a menu role.' }
        $destinations = foreach ($id in 'like-destination-mediadeka', 'like-destination-jellyfin') {
            $node = Await {
                $candidate = Element $root $id
                if ($candidate -and $candidate.Current.Name -like '* · Сохранено') { $candidate }
            } "Like destination $id did not display the saved mark."
            Read-LibraryNode $node
        }
        Invoke-Element $root 'like-menu-close'
        [void](Await { !(Element $root 'like-destinations-menu') } 'The close action did not dismiss the like destinations menu.')

        Invoke-Element $root 'library-tab-albums'
        [void](Await { Selected (Element $root 'library-tab-albums') } 'The Albums tab did not select.')
        Invoke-Element $root 'workspace-back'
        [void](Await {
            $field = Element $root 'music-filter'
            $field -and $field.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern).Current.Value -ceq 'Jellyfin Acceptance' -and
                (Element $root 'library-applied-filters').Current.Name -like '*Jellyfin*' -and (Element $root 'library-row-0')
        } 'Back did not restore the search query, provider filter and recording.')
        Invoke-Element $root 'workspace-forward'
        [void](Await { Selected (Element $root 'library-tab-albums') } 'Forward did not restore Albums.')
        $sections = foreach ($tab in 'albums', 'artists', 'playlists') {
            if ($tab -ne 'albums') { Invoke-Element $root "library-tab-$tab" }
            [void](Await { Selected (Element $root "library-tab-$tab") } "The $tab tab did not select.")
            $title = switch ($tab) {
                'albums' { $JellyfinFixture.Album }
                'artists' { $JellyfinFixture.Artist }
                'playlists' { $JellyfinFixture.Playlist }
            }
            $item = Await {
                $node = Element $root 'library-row-0'
                if ($node -and $node.Current.Name.StartsWith($title)) { $node }
            } "The saved Jellyfin item in $tab did not load." 30
            Read-LibraryNode $item
        }
        Invoke-Element $root 'home-page'
        Wait-JellyfinHome $root
        [ordered]@{ row = $rowState; combinedRow = $combined; counts = $counts; menu = $menuState; destinations = @($destinations); sections = @($sections); restoredFilters = $true }
    }
    function Jellyfin-HomeTrack($root) {
        $scopePattern = if (Selected (Home-SourceElement $root 'home-source-all')) { '^home-item-jellyfin-\d+$' } else { '^home-item-\d+$' }
        Descendants $root | Where-Object {
            $_.Current.AutomationId -match $scopePattern -and $_.Current.Name -ceq $JellyfinFixture.Track
        } | Select-Object -First 1
    }
    function Wait-JellyfinHome($root, [switch]$InitialSignIn) {
        try {
            [void](Await {
                if (!$InitialSignIn) { Assert-NoWelcome $root }
                $source = Home-SourceElement $root 'home-source-account-jellyfin'
                $first = Jellyfin-HomeTrack $root
                $source -and $source.Current.Name.StartsWith("$expectedSource · ") -and
                    $first -and $first.Current.Name -eq $JellyfinFixture.Track -and
                    !(Element $root 'login-panel') -and !(Element $root 'settings-connections')
            } "Jellyfin profile and indexed track did not appear during $phase." 25)
            Assert-NoWelcome $root
        } catch {
            $details = foreach ($id in 'home-source-all', 'home-source-account-jellyfin', 'home-item-jellyfin-0', 'home-item-0', 'home-empty-notice-jellyfin', 'home-cache-notice-jellyfin', 'login-error') {
                $node = Element $root $id
                if ($node) { "$id=$($node.Current.Name)" }
            }
            throw "$($_.Exception.Message) $($details -join '; ')"
        }
    }
    function Open-QuickConnect($root) {
        Open-SettingsLogin $root 'jellyfin'
        $modal = Element $root 'settings-login-modal'
        Set-Field $modal 'jellyfin-server' $JellyfinFixture.Url
        Invoke-Element $modal 'jellyfin-quick'
        Await {
            $node = Element $modal 'device-code'
            if ($node -and $node.Current.Name -match '^\d{6}$') { $node.Current.Name }
        } 'Jellyfin did not display a Quick Connect code.'
    }
    function Wait-GuestSettings($root, [string]$action) {
        [void](Await {
            Assert-NoWelcome $root
            $card = Element $root 'home-accounts'
            $card -and $card.Current.Name -eq 'Подключения · Гостевой вход' -and
                (Element $root $action) -and (Element $root 'settings-connections') -and
                !(Element $root 'settings-login-modal')
        } "Settings did not keep the guest source and expose $action.")
        Wait-ConnectionSettings $root
    }

    $app = Start-Mediadeka
    $phase = 'password login'
    try {
        $handle, $root = Get-Root $app
        [void](Await { Element $root 'guest' } 'Jellyfin fixture did not reach the welcome screen.' 25)
        Invoke-Element $root 'jellyfin'
        Set-Field $root 'jellyfin-server' $JellyfinFixture.Url
        Set-Field $root 'jellyfin-username' $JellyfinFixture.User
        Set-Field $root 'jellyfin-password' $JellyfinFixture.Password
        Invoke-Element $root 'jellyfin-submit'
        Wait-JellyfinHome $root -InitialSignIn
        if ($JellyfinFixture.Stage4) {
            $phase = 'unified library, history and like menu'
            $libraryEvidence = Read-JellyfinLibrary $root
        }
        Invoke-Element $root 'close'
        Assert-Closed $app

        $phase = 'password restart'
        $app = Start-Mediadeka
        $handle, $root = Get-Root $app
        Wait-JellyfinHome $root
        if ($JellyfinFixture.Stage4) {
            Open-ConnectionSettings $root
            Wait-LikeDefaults $root
            $libraryEvidence.reopenedLikeDefaults = $true
            Invoke-Element $root 'home-liked'
            Set-Field $root 'music-filter' 'Jellyfin Acceptance'
            $libraryEvidence.reopenedRow = Read-LibraryNode (Wait-CombinedLibraryTrack $root)
            Invoke-Element $root 'home-page'
            Wait-JellyfinHome $root
        }

        $phase = 'filter Jellyfin and restore All'
        [void](Await { Selected (Home-SourceElement $root 'home-source-all') } 'Restart did not open in All.')
        $allChips = Read-HomeChips $root
        Select-HomeFilter $root 'home-source-account-jellyfin'
        Wait-JellyfinHome $root
        $filteredChips = Read-HomeChips $root
        if (Element $root 'home-item-youtube-music-0') { throw 'The Jellyfin filter exposed a YouTube Music shelf.' }
        Toggle-SelectedHomeFilter $root 'home-source-account-jellyfin'
        Wait-JellyfinHome $root

        $phase = 'filter guest without changing the primary connection'
        Select-HomeFilter $root 'home-source-guest'
        if ((Element $root 'home-item-0').Current.Name -ceq $JellyfinFixture.Track) {
            throw 'The guest filter contained the Jellyfin fixture track.'
        }
        Open-ConnectionSettings $root -FromSourceMenu
        if (!(Element $root 'settings-sign-out-jellyfin') -or (Element $root 'settings-primary-jellyfin')) {
            throw 'Filtering guest changed the primary Jellyfin connection.'
        }
        Invoke-Element $root 'close'
        Assert-Closed $app

        $phase = 'restart after filtering guest'
        $app = Start-Mediadeka
        $handle, $root = Get-Root $app
        [void](Await { Selected (Home-SourceElement $root 'home-source-all') } 'Restart preserved a temporary home filter.')
        Wait-JellyfinHome $root

        $phase = 'disconnect Jellyfin and keep the guest session'
        Select-HomeFilter $root 'home-source-guest'
        Open-ConnectionSettings $root -FromSourceMenu
        Invoke-Element $root 'settings-sign-out-jellyfin'
        Wait-GuestSettings $root 'settings-sign-in-jellyfin'
        Invoke-Element $root 'home-page'
        [void](Await { Home-SourceElement $root 'home-add-account' } 'The connections action did not return after disconnecting Jellyfin.')
        if (Element $root 'home-source-account-jellyfin') { throw 'Disconnected Jellyfin was still among the platform chips.' }
        Invoke-Element $root 'home-add-account'
        Wait-ConnectionSettings $root

        $phase = 'close the settings login modal'
        Open-SettingsLogin $root 'jellyfin'
        Close-SettingsLogin $root
        Wait-GuestSettings $root 'settings-sign-in-jellyfin'
        $phase = 'Quick Connect cancellation'
        $null = Open-QuickConnect $root
        Send-WindowEscape $handle
        Wait-GuestSettings $root 'settings-sign-in-jellyfin'

        $phase = 'Quick Connect login'
        $code = Open-QuickConnect $root
        $approved = Invoke-RestMethod -Method Post -TimeoutSec 5 -Uri "$($JellyfinFixture.Url)/QuickConnect/Authorize?code=$code" -Headers @{
            Authorization = $JellyfinFixture.AuthorizationHeader
        }
        if ($approved -ne $true) { throw 'The Jellyfin fixture could not authorize Quick Connect.' }
        Wait-GuestSettings $root 'settings-sign-out-jellyfin'
        Return-FromConnectionSettings $root
        Select-HomeFilter $root 'home-source-all'
        Wait-JellyfinHome $root
        Invoke-Element $root 'close'
        Assert-Closed $app

        $phase = 'restart after connecting Jellyfin without switching'
        $app = Start-Mediadeka
        $handle, $root = Get-Root $app
        [void](Await {
            Assert-NoWelcome $root
            (Guest-Session $root) -and (Selected (Home-SourceElement $root 'home-source-all'))
        } 'The settings login did not persist the guest session.' 45)
        Open-ConnectionSettings $root
        Wait-GuestSettings $root 'settings-sign-out-jellyfin'
        Invoke-Element $root 'settings-primary-jellyfin'
        [void](Await { !(Element $root 'settings-primary-jellyfin') } 'Settings did not make Jellyfin primary.')
        Return-FromConnectionSettings $root

        $phase = 'show results after changing the primary connection in settings'
        Wait-JellyfinHome $root
        Invoke-Element $root 'close'
        Assert-Closed $app

        $phase = 'Quick Connect restart'
        $app = Start-Mediadeka
        $handle, $root = Get-Root $app
        Wait-JellyfinHome $root
        Invoke-Element $root 'close'
        Assert-Closed $app

        $phase = 'offline restart'
        & $JellyfinFixture.StopServer
        $app = Start-Mediadeka
        $handle, $root = Get-Root $app
        Wait-JellyfinHome $root
        $notice = Await {
            $entry = Home-FeedElement $root 'jellyfin' 'home-cache-notice'
            if ($entry -and $entry.Current.Name) { $entry.Current.Name }
        } 'No cache notice appeared after stopping the fixture server.'

        $report = [ordered]@{
            serverVersion = $JellyfinFixture.Version
            profile = $expectedSource
            track = (Jellyfin-HomeTrack $root).Current.Name
            passwordLogin = $true
            passwordRestored = $true
            guestFilterKeptJellyfinPrimary = $true
            pressingSameFilterRestoredAll = $true
            restartOpenedAll = $true
            madeJellyfinPrimaryInSettings = $true
            allChips = $allChips
            filteredChips = $filteredChips
            library = $libraryEvidence
            connectedStartupSkippedWelcome = $true
            logoutOpenedGuest = $true
            settingsLoginModalClosed = $true
            quickConnectCancelled = $true
            quickConnectCancellationAction = 'escape'
            quickConnectApproved = $true
            settingsLoginKeptGuest = $true
            settingsLoginAndGuestRoutePersisted = $true
            quickConnectRestored = $true
            offlineCacheNotice = $notice
        }

        $phase = 'disconnect active Jellyfin with its server offline'
        Open-ConnectionSettings $root
        Invoke-Element $root 'settings-sign-out-jellyfin'
        Wait-GuestSettings $root 'settings-sign-in-jellyfin'
        $report.activeLogoutFellBackToGuest = $true
        Invoke-Element $root 'close'
        Assert-Closed $app

        $phase = 'restart after disconnecting active Jellyfin'
        $app = Start-Mediadeka
        $handle, $root = Get-Root $app
        [void](Await {
            Assert-NoWelcome $root
            Guest-Session $root
        } 'Restart did not restore the guest route after disconnecting Jellyfin.' 45)
        Open-ConnectionSettings $root
        Wait-GuestSettings $root 'settings-sign-in-jellyfin'
        $report.logoutPersisted = $true
        Invoke-Element $root 'close'
        Assert-Closed $app
        $report | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $directory 'jellyfin-uia.json') -Encoding utf8
        'Windows UI Automation passed for Jellyfin settings, modal login and cancellation, logout, restarts and offline cache.'
    } catch {
        $ids = if ($root -and $app -and !$app.HasExited) {
            @(Descendants $root | ForEach-Object { $_.Current.AutomationId } | Where-Object { $_ })
        }
        throw "$phase - $($_.Exception.Message) UIA ids: $($ids -join ', ')."
    } finally {
        if ($app -and !$app.HasExited) { Stop-Process -Id $app.Id -ErrorAction SilentlyContinue }
    }
}

if ($JellyfinFixture) {
    Check-JellyfinFixture
    return
}

if ($SoundCloudLogin -or $SoundCloudLive -or $SoundCloudFeed) {
    if ($SoundCloudFeed) {
        $captureExe = (Resolve-Path -LiteralPath "$PSScriptRoot/../target/debug/examples/capture-window.exe").ProviderPath
    }
    $tempDirectory = (New-Item -ItemType Directory -Path (Join-Path $directory ('soundcloud-temp-' + [guid]::NewGuid().ToString('N')))).FullName
    $app = Start-Mediadeka -TempDirectory $tempDirectory
    try {
        $handle, $root = Get-Root $app
        $reuseConnection = $false
        if ($SoundCloudFeed) {
            $landing = Await {
                $source = Home-SourceElement $root 'home-source-account-soundcloud'
                if ($source -and $source.Current.Name -like 'SoundCloud · *') {
                    return 'saved'
                }
                if (Element $root 'soundcloud') { return 'welcome' }
            } 'The SoundCloud test profile must open on its saved source or show welcome.' 30
            $reuseConnection = $landing -ceq 'saved'
        }
        if (!$reuseConnection) {
            Invoke-Element $root 'soundcloud'
            Invoke-Element $root 'start-connection'
            $page = Wait-SoundCloudPage $app
        }
        if ($SoundCloudLive -or $SoundCloudFeed) {
            if ($reuseConnection) {
                Write-Output 'Reusing the saved connection in the isolated SoundCloud feed profile.'
                Open-ConnectionSettings $root
                Return-FromConnectionSettings $root
            } else {
                Write-Output 'SoundCloud is ready for interactive sign-in in the Mediadeka window.'
            }
            $sourceName = Await {
                $source = Home-SourceElement $root 'home-source-account-soundcloud'
                if ($source -and $source.Current.Name -like 'SoundCloud · *') {
                    $source.Current.Name -replace ' · [^·]+$', ''
                }
                $errorLabel = Element $root 'login-error'
                if ($errorLabel) { throw "SoundCloud sign-in failed: $($errorLabel.Current.Name)" }
                if (Element $root 'login-cancelled') { throw 'SoundCloud sign-in was cancelled.' }
            } 'SoundCloud sign-in was not completed.' 330
            if ($SoundCloudFeed) {
                Select-HomeFilter $root 'home-source-account-soundcloud'
                $initialPages = Read-SoundCloudPages $root
            }
            Invoke-Element $root 'close'
            Assert-Closed $app
            if ($SoundCloudFeed) {
                & $captureExe --home (Join-Path $directory 'frames') soundcloud $profile
                if ($LASTEXITCODE -ne 0) { throw 'GPUI frame capture failed.' }
            }
            $app = Start-Mediadeka -TempDirectory $tempDirectory
            $handle, $root = Get-Root $app
            [void](Await { Element $root 'home-sources' } 'The saved SoundCloud shell did not open.' 30)
            Open-ConnectionSettings $root
            [void](Await { Element $root 'settings-sign-out-soundcloud' } 'The restored profile is missing in settings.')
            Return-FromConnectionSettings $root
            Select-HomeFilter $root 'home-source-account-soundcloud'
            [void](Await {
                $source = Element $root 'home-source-account-soundcloud'
                if (Element $root 'home-skeleton') { return $false }
                $notice = Element $root 'home-empty-notice'
                if ($notice -and $notice.Current.Name -match 'Сеанс SoundCloud|подключение|другой профиль|восстановить') {
                    throw "SoundCloud profile was not revalidated: $($notice.Current.Name)"
                }
                $source -and $source.Current.Name.StartsWith("$sourceName · ") -and (Selected $source) -and
                    !(Find-SoundCloudWindow $app)
            } 'SoundCloud did not restore the saved provider.' 45)
            if ($SoundCloudFeed) {
                $restoredPages = Read-SoundCloudPages $root
                $shelfNavigation = Check-SoundCloudShelfNavigation $root
                Invoke-Element $root 'close'
                Assert-Closed $app
                $app = Start-Mediadeka -TempDirectory $tempDirectory -Proxy 'http://127.0.0.1:1'
                $handle, $root = Get-Root $app
                Select-HomeFilter $root 'home-source-account-soundcloud'
                $cachedPages = Read-SoundCloudPages $root -Cached
                if (!(Element $root 'home-source-account-soundcloud').Current.Name.StartsWith("$sourceName · ") -or
                    (ConvertTo-Json -InputObject $cachedPages -Depth 4 -Compress) -cne
                    (ConvertTo-Json -InputObject $restoredPages -Depth 4 -Compress)) {
                    throw 'Offline SoundCloud changed the source, activity row order, or collection sections.'
                }
            }
            Open-ConnectionSettings $root
            if ($SoundCloudFeed) {
                if (!(Element $root 'settings-sign-out-soundcloud')) { throw 'The feed connection was not retained.' }
                [ordered]@{
                    provider = 'SoundCloud'
                    initialTracks = $initialPages.activity.titles.Count
                    restoredTracks = $restoredPages.activity.titles.Count
                    accessibleCovers = $restoredPages.activity.covers
                    activityActions = $restoredPages.activity.actions
                    home = $restoredPages.recommendations
                    library = $restoredPages.library
                    libraryPaginationRestoresPage = $shelfNavigation
                    sameProfile = $true
                    sameOfflineOrder = $true
                    homeAndLibraryRestoredOffline = $true
                    retryAvailableOffline = $true
                    reusedConnection = $reuseConnection
                    isolatedProfile = $profile
                } | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $directory 'soundcloud-feed-uia.json') -Encoding utf8
            } else {
                Invoke-Element $root 'settings-sign-out-soundcloud'
                [void](Await { Element $root 'settings-sign-in-soundcloud' } 'The isolated test connection was not removed.' 35)
                [ordered]@{ provider = 'SoundCloud'; sameProfile = $true; isolatedProfile = $profile } |
                    ConvertTo-Json | Set-Content -LiteralPath (Join-Path $directory 'soundcloud-live-uia.json') -Encoding utf8
            }
        } else {
            $browserProfile = Join-Path $profile 'SoundCloudSignIn/WebView2'
            $marker = Join-Path $browserProfile 'mediadeka-uia-profile-marker'
            if (!(Test-Path -LiteralPath $browserProfile -PathType Container)) {
                throw 'SoundCloud did not open its dedicated browser profile.'
            }
            Set-Content -LiteralPath $marker -Value 'SoundCloud UI Automation'
            if (![MediadekaWelcomeProbe]::Close($page.handle)) { throw 'Could not close the embedded SoundCloud window.' }
            [void](Await { (Element $root 'login-cancelled') -and !(Find-SoundCloudWindow $app) } 'Closing the embedded window did not cancel SoundCloud.')
            Invoke-Element $root 'retry'
            $null = Wait-SoundCloudPage $app
            Send-WindowEscape $handle
            [void](Await { (Element $root 'login-cancelled') -and !(Find-SoundCloudWindow $app) } 'Escape did not close the embedded window.')
            Invoke-Element $root 'guest'
            [void](Await { Element $root 'home-sources' } 'Guest shell did not open.' 35)
            Open-ConnectionSettings $root
            Open-SettingsLogin $root 'soundcloud'
            Invoke-Element $root 'start-connection'
            $null = Wait-SoundCloudPage $app
            Close-SettingsLogin $root
            [void](Await { !(Find-SoundCloudWindow $app) -and (Element $root 'settings-sign-in-soundcloud') } 'Closing settings did not cancel SoundCloud.')
            Return-FromConnectionSettings $root
            if ((Element $root 'home-accounts').Current.Name -cne 'Подключения · Гостевой вход') {
                throw 'Cancelling in settings changed the active source.'
            }
            Invoke-Element $root 'close'
            Assert-Closed $app

            $app = Start-Mediadeka -TempDirectory $tempDirectory
            $handle, $root = Get-Root $app
            [void](Await { Element $root 'home-sources' } 'The guest shell did not restore after restart.' 35)
            Open-ConnectionSettings $root
            Open-SettingsLogin $root 'soundcloud'
            Invoke-Element $root 'start-connection'
            $null = Wait-SoundCloudPage $app
            if (!(Test-Path -LiteralPath $marker -PathType Leaf)) {
                throw 'SoundCloud replaced its browser profile after restart.'
            }
            Close-SettingsLogin $root
            [void](Await { !(Find-SoundCloudWindow $app) } 'The reopened SoundCloud window did not close.')
            Invoke-Element $root 'close'
            Assert-Closed $app

            $errorProfile = Join-Path $directory ('uia-soundcloud-error-' + [guid]::NewGuid().ToString('N'))
            $app = Start-Mediadeka -TestProfile $errorProfile -WebView2Arguments '--proxy-server=http://127.0.0.1:1' -TempDirectory $tempDirectory
            $handle, $root = Get-Root $app
            Invoke-Element $root 'soundcloud'
            Invoke-Element $root 'start-connection'
            $failure = Await { Element $root 'login-error' } 'SoundCloud network failure did not surface in the login panel.' 35
            $failureText = $failure.Current.Name
            if (!$failureText -or (Find-SoundCloudWindow $app)) { throw 'The failed window did not close and show a readable error.' }
            Invoke-Element $root 'retry'
            [void](Await { (Element $root 'login-waiting') -or (Find-SoundCloudWindow $app) } 'SoundCloud retry did not start.' 12)
            [void](Await { (Element $root 'login-error') -and !(Find-SoundCloudWindow $app) } 'SoundCloud retry did not return the network result.' 35)
            $errorBrowserProfile = Join-Path $errorProfile 'SoundCloudSignIn/WebView2'
            if (!(Test-Path -LiteralPath $errorBrowserProfile -PathType Container) -or
                (Test-Path -LiteralPath (Join-Path $errorBrowserProfile (Split-Path -Leaf $marker)))) {
                throw 'SoundCloud browser data crossed Mediadeka profiles.'
            }
            [ordered]@{
                realPageDocument = $page.document
                welcomeClose = $true
                welcomeEscape = $true
                settingsCancel = $true
                guestSourcePreserved = $true
                networkError = $failureText
                retryReturnedError = $true
                browserProfilePreservedAcrossRestart = $true
                separateBrowserProfiles = $true
            } | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $directory 'soundcloud-uia.json') -Encoding utf8
        }
        Invoke-Element $root 'close'
        Assert-Closed $app
        'Windows UI Automation passed for SoundCloud.'
    } finally {
        if ($app -and !$app.HasExited) { Stop-Process -Id $app.Id -ErrorAction SilentlyContinue }
    }
    return
}

if ($SpotifyLive) {
    function Wait-SpotifyHome($app, [IntPtr]$handle, [int]$seconds) {
        Await {
            if ($app.HasExited) { throw "Mediadeka exited while waiting for Spotify ($($app.ExitCode))." }
            # The browser takes foreground during consent. Resolve a new root
            # for each query so the original authorization tree is not reused.
            $root = $automation::FromHandle($handle)
            $errorNode = Element $root 'login-error'
            if ($errorNode) { throw "Spotify login stopped. $($errorNode.Current.Name)" }
            $source = Home-SourceElement $root 'home-source-account-spotify'
            if ($source -and $source.Current.Name.StartsWith('Spotify · ')) {
                $source.Current.Name -replace ' · [^·]+$', ''
            }
        } 'Spotify did not open an authenticated home within the time limit.' $seconds
    }
    function Read-SpotifyFeed($root) {
        $feed = Element $root 'home-feed'
        $first = Element $root 'home-item-0'
        $cover = Element $root 'home-artwork-0'
        if (!$feed -or !$first -or !$cover) { return }
        $bounds = $cover.Current.BoundingRectangle
        if ($bounds.IsEmpty -or [Math]::Abs($bounds.Width - $bounds.Height) -gt 2) {
            return
        }
        $sections = @(Descendants $root | Where-Object { $_.Current.AutomationId -match '^home-section-\d+$' }).Count
        if ($sections -lt 1) { return }
        [ordered]@{
            sections = $sections
            quickPicks = [bool](Element $root 'home-quick-picks')
            squareCover = $true
            fromCache = [bool](Element $root 'home-cache-notice')
        }
    }
    function Wait-SpotifyFeed($root) {
        if (!$ExpectSpotifyFeed) { return (Read-SpotifyFeed $root) }
        Await {
            $notice = Element $root 'home-empty-notice'
            if ($notice) { throw "Spotify home reported an error. $($notice.Current.Name)" }
            $result = Read-SpotifyFeed $root
            if ($result -and !$result.fromCache) { $result }
        } 'Spotify did not return a fresh recommendation section and item with a square cover.' 65
    }

    $app = Start-Mediadeka
    $phase = 'authorization'
    $root = $null
    try {
        $handle, $root = Get-Root $app
        Invoke-Element $root 'spotify'
        Invoke-Element $root 'authorize'
        'Opened Spotify authorization in the browser. Waiting for approval.'
        $account = Wait-SpotifyHome $app $handle 300
        $phase = 'first recommendation feed'
        $root = $automation::FromHandle($handle)
        Select-HomeFilter $root 'home-source-account-spotify'
        $openedFeed = Wait-SpotifyFeed $root
        $freshLibrary = [bool](Element $root 'home-item-0')
        Invoke-Element $root 'close'
        Assert-Closed $app

        $phase = 'restart'
        $app = Start-Mediadeka
        $handle, $root = Get-Root $app
        $restored = Wait-SpotifyHome $app $handle 65
        if ($restored -cne $account) { throw 'Spotify restored a different profile.' }
        if (Element $root 'authorize') { throw 'Spotify requested browser approval again.' }
        $phase = 'restored recommendation feed'
        $root = $automation::FromHandle($handle)
        Select-HomeFilter $root 'home-source-account-spotify'
        $restoredFeed = Wait-SpotifyFeed $root
        $report = [ordered]@{
            loginConfirmed = $true
            sameProfileAfterRestart = $true
            browserPromptAfterRestart = $false
            openedWithLibrary = $freshLibrary
            openedFeed = $openedFeed
            restoredFeed = $restoredFeed
        }
        Invoke-Element $root 'close'
        Assert-Closed $app
        $report | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $directory 'spotify-live-uia.json') -Encoding utf8
        'Windows UI Automation passed for Spotify browser login and restoration.'
    } catch {
        $ids = if ($root -and $app -and !$app.HasExited) {
            @(Descendants $root | ForEach-Object { $_.Current.AutomationId } | Where-Object { $_ })
        }
        throw "${phase}: $($_.Exception.Message) UIA IDs: $($ids -join ', ')."
    } finally {
        if ($app -and !$app.HasExited) { Stop-Process -Id $app.Id -ErrorAction SilentlyContinue }
    }
    return
}

if ($RestoredProfile) {
    $app = Start-Mediadeka
    try {
        $handle, $root = Get-Root $app
        $restoredData = Await {
            Assert-NoWelcome $root
            Open-HomeSources $root
            $source = Descendants $root | Where-Object {
                $_.Current.AutomationId -match '^home-source-account-' -and
                    $_.Current.Name.StartsWith("$RestoredAccountSource · ")
            } | Select-Object -First 1
            if ($source) {
                $provider = $source.Current.AutomationId -replace '^home-source-account-', ''
                $item = Home-FeedElement $root $provider 'home-item' 0
                if (!$item -or !$item.Current.Name) { return }
                $viaCache = [bool](Home-FeedElement $root $provider 'home-cache-notice')
                if ($ExpectOfflineCache.IsPresent -eq $viaCache) {
                    [pscustomobject]@{ viaCache = $viaCache; provider = $provider }
                }
            }
        } 'The saved account did not display music from the expected data source.' 30
        $availableItems = @(Descendants $root | Where-Object {
            $_.Current.AutomationId -match "^home-item-($($restoredData.provider)-)?\d+$"
        }).Count
        $result = [ordered]@{
            provider = $RestoredAccountSource
            viaCache = $restoredData.viaCache
            skippedWelcome = $true
            itemsInTree = $availableItems
        }
        Invoke-Element $root 'close'
        Assert-Closed $app
        $result | ConvertTo-Json -Compress
    } finally {
        if ($app -and !$app.HasExited) { Stop-Process -Id $app.Id -ErrorAction SilentlyContinue }
    }
    return
}

$names = [ordered]@{
    'spotify' = 'Spotify'
    'youtube-music' = 'YouTube Music'
    'yandex-music' = 'Яндекс Музыка'
    'soundcloud' = 'SoundCloud'
    'jellyfin' = 'Jellyfin'
    'guest' = 'Гостевой вход'
    'diagnostics' = 'Диагностика'
    'minimize' = 'Свернуть'
    'maximize' = 'Развернуть'
    'close' = 'Закрыть'
}
$app = Start-Mediadeka
try {
    $handle, $root = Get-Root $app
    try {
        [void](Await { Element $root 'guest' } 'Clean profile did not show welcome.')
    } catch {
        Descendants $root | ForEach-Object {
            [ordered]@{ id = $_.Current.AutomationId; name = $_.Current.Name; role = $_.Current.ControlType.ProgrammaticName }
        } | ConvertTo-Json -Depth 3 | Set-Content -LiteralPath (Join-Path $directory 'uia-startup-tree.json') -Encoding utf8
        throw
    }
    $elements = foreach ($id in $names.Keys) {
        $control = Element $root $id
        if (!$control -or $control.Current.Name -ne $names[$id] -or
            $control.Current.ControlType.ProgrammaticName -ne 'ControlType.Button' -or
            !$control.Current.IsKeyboardFocusable) {
            throw "Unexpected button semantics for $id."
        }
        [ordered]@{ id = $id; name = $control.Current.Name; type = 'Button'; focusable = $true }
    }

    $dpi = [MediadekaWelcomeProbe]::GetDpiForWindow($handle)
    $min = [MediadekaWelcomeProbe]::MinimumTrack($handle)
    $width = [int][Math]::Round(800 * $dpi / 96)
    $height = [int][Math]::Round(400 * $dpi / 96)
    if ($min.X -lt $width -or $min.Y -lt $height) {
        throw "Native tracking minimum $($min.X) × $($min.Y) at DPI $dpi."
    }
    if (![MediadekaWelcomeProbe]::ResizeClient($handle, $width, $height)) { throw 'Resize failed.' }
    $client = Await {
        $bounds = [MediadekaWelcomeProbe]::ClientOnScreen($handle)
        if ($bounds.Right - $bounds.Left -eq $width -and $bounds.Bottom - $bounds.Top -eq $height) { $bounds }
    } 'Minimum client size was not applied.'
    [void](Await {
        $guest = Element $root 'guest'
        $bounds = $guest.Current.BoundingRectangle
        !$bounds.IsEmpty -and $bounds.Bottom -le ($client.Bottom + 2)
    } 'Guest was clipped at the minimum size before keyboard navigation.')
    # Anchor the cycle through UIA. Activation can focus a different element
    # when an earlier instance was foreground during startup.
    (Element $root 'close').SetFocus()
    [void](Await { (Element $root 'close').Current.HasKeyboardFocus } 'UIA could not focus the close button.')
    $keyboard = foreach ($id in $names.Keys) {
        [MediadekaWelcomeProbe]::Tab($handle)
        $control = Await {
            $button = Element $root $id
            if ($button -and $button.Current.HasKeyboardFocus) { $button }
        } "Tab did not focus $id."
        $bounds = $control.Current.BoundingRectangle
        if ($bounds.IsEmpty -or $bounds.Top -lt ($client.Top - 2) -or $bounds.Bottom -gt ($client.Bottom + 2)) {
            throw "Button $id is clipped at the minimum size."
        }
        $id
    }

    $hit = [MediadekaWelcomeProbe]::DragHitTest($handle, $true)
    if ($hit -ne 2) { throw "Free titlebar returned native hit-test $hit." }
    [void](Await { [MediadekaWelcomeProbe]::IsZoomed($handle) } 'Native titlebar double-click did not maximize.')
    Invoke-Element $root 'maximize'
    [void](Await { ![MediadekaWelcomeProbe]::IsZoomed($handle) } 'UIA did not restore.')
    Invoke-Element $root 'maximize'
    [void](Await { [MediadekaWelcomeProbe]::IsZoomed($handle) } 'UIA did not maximize.')
    $restore = Await {
        $restore = Element $root 'maximize'
        if ($restore.Current.Name -eq 'Восстановить') { $restore }
    } 'UIA did not update the maximize button name.'
    Invoke-Element $root $restore.Current.AutomationId
    [void](Await { ![MediadekaWelcomeProbe]::IsZoomed($handle) } 'UIA did not restore after maximizing.')
    Invoke-Element $root 'minimize'
    [void](Await { [MediadekaWelcomeProbe]::IsIconic($handle) } 'UIA did not minimize.')
    [void][MediadekaWelcomeProbe]::ShowWindowAsync($handle, 9)
    [void](Await { ![MediadekaWelcomeProbe]::IsIconic($handle) } 'Windows did not restore minimized window.')

    $startAndCancel = foreach ($provider in 'spotify', 'yandex-music') {
        Invoke-Element $root $provider
        [void](Await { Element $root 'authorize' } "$provider did not present its browser link.")
        Cancel-PendingConnection $root $handle $provider -Compact
        $provider
    }
    Invoke-Element $root 'youtube-music'
    $field = Await { Element $root 'youtube-session' } 'YouTube Music session field missing.'
    if (!$field.Current.IsPassword) { throw 'YouTube Music session field lacks protection semantics.' }
    Invoke-CompactReturn $root 'youtube-music'
    $sessionFields = @('youtube-music')
    Invoke-Element $root 'soundcloud'
    $soundcloudStart = Await { Element $root 'start-connection' } 'SoundCloud sign-in button missing.'
    if ($soundcloudStart.Current.Name -cne 'Войти в SoundCloud' -or (Element $root 'soundcloud-token')) {
        throw 'SoundCloud did not expose its embedded sign-in action.'
    }
    Invoke-CompactReturn $root 'soundcloud'
    Invoke-Element $root 'jellyfin'
    [void](Await { Element $root 'jellyfin-server' } 'Jellyfin server field missing.')
    $jellyfin = foreach ($id in 'jellyfin-server', 'jellyfin-username', 'jellyfin-password') {
        $control = Element $root $id
        if (!$control) { throw "Jellyfin field missing: $id." }
        [ordered]@{ id = $id; name = $control.Current.Name; password = $control.Current.IsPassword }
    }
    if (!$jellyfin[2].password) { throw 'Jellyfin password lacks protected input semantics.' }
    Invoke-CompactReturn $root 'jellyfin'
    Invoke-Element $root 'guest'
    [void](Await { Guest-Home $root } 'Guest home with public music did not open.' 45)
    Test-Shell $root $handle
    $compactLabel = Element $root 'home-library-label'
    if (!$compactLabel -or $compactLabel.Current.Name -cne 'Медиатека') {
        throw 'The compact sidebar separator lost the accessible name of the library group.'
    }
    $wideWidth = [int][Math]::Round(1500 * $dpi / 96)
    $wideHeight = [int][Math]::Round(850 * $dpi / 96)
    if (![MediadekaWelcomeProbe]::ResizeClient($handle, $wideWidth, $wideHeight)) { throw 'Could not resize the home window.' }
    [void](Await {
        $wideLabel = Element $root 'home-library-label'
        $wideLabel -and !$wideLabel.Current.IsKeyboardFocusable -and
            $wideLabel.Current.ControlType.ProgrammaticName -ne 'ControlType.Button'
    } 'The wide sidebar has no passive library label.')
    $homeLabels = @(Descendants $root | Where-Object { $_.Current.Name } | ForEach-Object { $_.Current.Name })
    Invoke-Element $root 'close'
    Assert-Closed $app

    $app = Start-Mediadeka
    $handle, $root = Get-Root $app
    [void](Await { Guest-Home $root } 'Guest home with public music was not restored.' 45)
    Test-Shell $root $handle
    $restoredLabels = @(Descendants $root | Where-Object { $_.Current.Name } | ForEach-Object { $_.Current.Name })
    Invoke-Element $root 'close'
    Assert-Closed $app

    $report = [ordered]@{
        title = 'Mediadeka'; dpi = $dpi; controls = $elements; focusOrder = @($keyboard)
        minimumClient = @($width, $height); minimumTrack = @($min.X, $min.Y)
        allOptionsFit = $true; nativeDragHit = $hit
        doubleClickMaximized = $true; maximizeRestore = $true; minimizeRestore = $true; exitCode = $app.ExitCode
        jellyfinFields = $jellyfin; providerStartedAndCancelled = $startAndCancel
        providerCancellationAction = 'escape'; compactReturnAction = 'connection-return'
        protectedSessionFields = $sessionFields; guestOpened = $homeLabels; guestRestored = $restoredLabels
        shell = $true; shellKeys = 'ctrl-k, escape'
    }
    $report | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $directory 'welcome-uia.json') -Encoding utf8
    'Windows UI Automation passed for welcome, chrome, Jellyfin fields and guest restart.'
} finally {
    if ($app -and !$app.HasExited) { Stop-Process -Id $app.Id -ErrorAction SilentlyContinue }
}
