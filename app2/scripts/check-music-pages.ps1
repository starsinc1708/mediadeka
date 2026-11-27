param(
    [Parameter(Mandatory)][string]$Profile,
    [string]$Executable = "$PSScriptRoot/../target/debug/mediadeka.exe",
    [string]$Evidence = "$PSScriptRoot/../target/acceptance/music-pages/uia",
    [string]$VerifiedReleaseUrl = 'https://music.yandex.ru/album/43177750',
    [string]$VerifiedReleaseTitle = 'ХИМИЯ',
    [switch]$LoadHelpers
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
if (!('MediadekaMusicProbe' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;

public static class MediadekaMusicProbe {
    private delegate bool Visit(IntPtr window, IntPtr parameter);
    [DllImport("user32.dll")] private static extern bool EnumWindows(Visit visit, IntPtr parameter);
    [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr window, out uint processId);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern int GetWindowText(IntPtr window, StringBuilder text, int capacity);
    [DllImport("user32.dll")] private static extern bool SetWindowPos(IntPtr window, IntPtr insertAfter, int x, int y, int width, int height, uint flags);
    [DllImport("user32.dll")] private static extern bool ShowWindowAsync(IntPtr window, int command);

    public static void RenderOffscreen(IntPtr window) {
        SetWindowPos(window, IntPtr.Zero, -24000, -24000, 1672, 941, 0x14);
        ShowWindowAsync(window, 4);
    }

    public static IntPtr Find(uint processId) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((window, _) => {
            GetWindowThreadProcessId(window, out uint owner);
            if (owner != processId) return true;
            var title = new StringBuilder(64);
            GetWindowText(window, title, title.Capacity);
            if (title.ToString() != "Mediadeka") return true;
            found = window;
            return false;
        }, IntPtr.Zero);
        return found;
    }
}
'@
}
$profilePath = (Resolve-Path -LiteralPath $Profile).Path
$program = (Resolve-Path -LiteralPath $Executable).Path
$directory = (New-Item -ItemType Directory -Force -Path $Evidence).FullName
if (!(Test-Path -LiteralPath (Join-Path $profilePath 'mediadeka.sqlite3'))) { throw 'Use the isolated, warmed music pages profile.' }

$automation = [System.Windows.Automation.AutomationElement]
$scope = [System.Windows.Automation.TreeScope]::Descendants

function Await([scriptblock]$Condition, [string]$Message, [int]$Seconds = 50) {
    $deadline = [DateTime]::UtcNow.AddSeconds($Seconds)
    do {
        try { $result = & $Condition; if ($result) { return $result } }
        catch [System.Windows.Automation.ElementNotAvailableException] {}
        Start-Sleep -Milliseconds 120
    } until ([DateTime]::UtcNow -ge $deadline)
    throw $Message
}

function Element($Root, [string]$Id) {
    $condition = [System.Windows.Automation.PropertyCondition]::new($automation::AutomationIdProperty, $Id)
    $Root.FindFirst($scope, $condition)
}

function Find-Name($Root, [string]$Pattern) {
    $Root.FindAll($scope, [System.Windows.Automation.Condition]::TrueCondition) |
        Where-Object { $_.Current.Name -like $Pattern } | Select-Object -First 1
}

function Invoke-Element($Element) {
    $pattern = $null
    if ($Element.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern, [ref]$pattern)) {
        [void]$pattern.Invoke()
        return
    }
    if ($Element.TryGetCurrentPattern([System.Windows.Automation.ExpandCollapsePattern]::Pattern, [ref]$pattern)) {
        if ($pattern.Current.ExpandCollapseState -eq 'Expanded') { $pattern.Collapse() } else { $pattern.Expand() }
        return
    }
    if ($Element.TryGetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern, [ref]$pattern)) {
        $pattern.Select()
        return
    }
    if ($Element.TryGetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern, [ref]$pattern)) {
        $pattern.Toggle()
        return
    }
    throw "Invoke unavailable for $($Element.Current.AutomationId) ($($Element.Current.Name))."
}

function Set-Field($Root, [string]$Id, [string]$Value) {
    $null = Await {
        $candidate = Element $Root $Id
        $editor = $null
        if ($candidate -and $candidate.TryGetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern, [ref]$editor)) {
            if ($editor.Current.Value -ceq $Value) { return $true }
            $editor.SetValue($Value)
        }
    } "$Id did not receive its value."
}

function Open-CatalogUrl($Root, [string]$Url) {
    Set-Field $Root 'home-search' $Url
    Invoke-Element (Await { Element $Root 'home-catalog-search' } 'Catalog entry missing.')
}

function Semantic($Element) {
    [ordered]@{ id = $Element.Current.AutomationId; name = $Element.Current.Name;
        role = $Element.Current.ControlType.ProgrammaticName; keyboardFocusable = $Element.Current.IsKeyboardFocusable }
}

if ($LoadHelpers) { return }

$previous = $env:MEDIADEKA_PROFILE_DIR
try {
    $env:MEDIADEKA_PROFILE_DIR = $profilePath
    $process = Start-Process -FilePath $program -WorkingDirectory (Split-Path $program) -WindowStyle Hidden -PassThru
} finally {
    $env:MEDIADEKA_PROFILE_DIR = $previous
}

try {
    $handle = Await {
        $windowHandle = [MediadekaMusicProbe]::Find([uint32]$process.Id)
        if ($windowHandle -ne [IntPtr]::Zero) { $windowHandle }
    } 'Mediadeka HWND missing.'
    [MediadekaMusicProbe]::RenderOffscreen($handle)
    $root = Await {
        if ($process.HasExited) { throw "Mediadeka exited ($($process.ExitCode))." }
        $candidate = $automation::FromHandle($handle)
        if ((Element $candidate 'home-navigation') -and (Element $candidate 'home-accounts')) { $candidate }
    } 'Startup did not show the workspace.'
    if (Element $root 'home-source') { throw 'The retired sidebar profile is still accessible.' }
    $report = [ordered]@{ sidebarProfileRemoved = $true; platformChips = Semantic (Element $root 'home-sources') }

    Set-Field $root 'home-search' 'ПЕРВЫЙ РАЗ GATASKI'
    Invoke-Element (Await { Element $root 'home-catalog-search' } 'Catalog entry missing.')
    $report.textSearch = [ordered]@{ title = Semantic (Await { Element $root 'music-search-title' } 'Catalog search did not open.') }
    $candidate = Await { Find-Name $root 'ПЕРВЫЙ РАЗ · *GATASKI*' } 'The real recording was not found in the connected catalogs.'
    $report.textSearch.candidate = Semantic $candidate
    Invoke-Element $candidate
    $null = Await { (Element $root 'music-object-title').Current.Name -eq 'ПЕРВЫЙ РАЗ' } 'The chosen recording did not open.'

    Open-CatalogUrl $root $VerifiedReleaseUrl
    $album = Await { $title = Element $root 'music-object-title'; if ($title -and $title.Current.Name -eq $VerifiedReleaseTitle) { $title } } 'The release did not open in the workspace.'
    $open = Await { Find-Name $root 'Открыть на площадке Spotify' } 'Verified Spotify link missing.'
    $copy = Await { Find-Name $root 'Копировать ссылку на Spotify' } 'Copy action missing.'
    $search = Await { Element $root 'music-search-platforms' } 'Separate platform search action missing.'
    $report.album = [ordered]@{ title = Semantic $album; verifiedLink = Semantic $open; copy = Semantic $copy; fallback = Semantic $search }
    Invoke-Element $search
    $report.album.fallbackDestination = Semantic (Await { Find-Name $root 'Найти на площадке *' } 'Platform search menu did not expand.')

    Open-CatalogUrl $root 'https://music.yandex.ru/artist/7245399'
    $artist = Await { $title = Element $root 'music-object-title'; if ($title -and $title.Current.Name -like '*Страусов') { $title } } 'Artist did not open in the workspace.'
    $report.artist = [ordered]@{ title = Semantic $artist;
        related = Semantic (Await { Element $root 'music-related-artists' } 'Related artist panel missing.');
        releases = Semantic (Await { Element $root 'music-object-releases' } 'Artist releases missing.') }
    $showTracks = Await { Element $root 'music-artist-show-tracks' } 'Artist track disclosure missing.'
    Invoke-Element $showTracks
    $null = Await { (Element $root 'music-artist-show-tracks').Current.Name -eq 'Свернуть треки' } 'Artist tracks did not expand.'
    $report.artist.expandedTracks = $true
    Invoke-Element (Await { Element $root 'workspace-back' } 'Back action missing.')
    $null = Await { (Element $root 'music-object-title').Current.Name -eq $VerifiedReleaseTitle } 'Back did not restore the release.'
    Invoke-Element (Await { Element $root 'workspace-forward' } 'Forward action missing.')
    $null = Await { (Element $root 'music-object-title').Current.Name -like '*Страусов' } 'Forward did not restore the artist.'
    $report.history = $true

    Invoke-Element (Await { Element $root 'home-liked' } 'Liked navigation missing.')
    Invoke-Element (Await { Element $root 'library-filters' } 'Library filters unavailable.')
    $report.liked = [ordered]@{ title = Semantic (Await { Element $root 'music-liked-title' } 'Liked did not open.');
        filter = Semantic (Await { Element $root 'music-filter' } 'Liked search missing.');
        source = Semantic (Await { Element $root 'liked-source-yandex-music' } 'Liked source filter missing.') }
    Set-Field $root 'music-filter' 'no-ui-acceptance-result-987654'
    $report.liked.emptyAfterSearch = Semantic (Await { Element $root 'liked-empty' } 'Text filter did not hide nonmatching tracks.')

    Invoke-Element (Await { Element $root 'home-playlists' } 'Playlist navigation missing.')
    $report.playlists = [ordered]@{ sourceLists = Semantic (Await { Element $root 'library-tab-playlists' } 'Provider playlists missing.');
        localCreate = Semantic (Await { Element $root 'playlist-create' } 'Local playlist action missing.') }
    Set-Field $root 'music-filter' 'SET::0.1'
    Invoke-Element (Await { Find-Name $root 'SET::0.1*Spotify*' } 'Saved Spotify playlist row missing.')
    $report.playlists.opened = Semantic (Await { $title = Element $root 'playlist-title'; if ($title -and $title.Current.Name -eq 'SET::0.1') { $title } } 'The playlist card did not open the existing page.')
    $report.transport = Semantic (Await { Element $root 'home-transport' } 'Shared player missing.')
    $report | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $directory 'music-pages-uia.json') -Encoding UTF8
    Write-Output 'Music pages UI Automation passed.'
} finally {
    if (!$process.HasExited) { Stop-Process -Id $process.Id }
}
