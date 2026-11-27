param(
    [Parameter(Mandatory)][string]$Executable,
    [Parameter(Mandatory)][string]$Evidence,
    [string]$Profile = "$env:LOCALAPPDATA/Mediadeka/app2",
    [ValidateSet('spotify', 'yandex-music', 'youtube-music', 'soundcloud', 'jellyfin')]
    [string]$LibraryProvider = 'spotify',
    [string]$LibraryQuery = 'a'
)

$ErrorActionPreference = 'Stop'
$source = Join-Path (Resolve-Path -LiteralPath $Profile).Path 'mediadeka.sqlite3'
$tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$isolated = Join-Path $tempRoot ('mediadeka-audit-' + [guid]::NewGuid().ToString('N'))
$profileCopy = (New-Item -ItemType Directory -Path $isolated).FullName
$previous = $env:MEDIADEKA_PROFILE_DIR
$process = $null

try {
    Copy-Item -LiteralPath $source -Destination (Join-Path $profileCopy 'mediadeka.sqlite3')
    . "$PSScriptRoot/check-music-pages.ps1" -Profile $profileCopy -Executable $Executable -Evidence $Evidence -LoadHelpers
    if (!('MediadekaAuditInput' -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

public static class MediadekaAuditInput {
    [DllImport("user32.dll", SetLastError = true)]
    private static extern bool PostMessage(IntPtr window, uint message, IntPtr wParam, IntPtr lParam);

    public static bool Wheel(IntPtr window, int x, int y, int delta) {
        int position = (y << 16) | (x & 0xffff);
        return PostMessage(window, 0x020a, (IntPtr)(delta << 16), (IntPtr)position);
    }

    public static bool Enter(IntPtr window) {
        return PostMessage(window, 0x0100, (IntPtr)0x0d, (IntPtr)1)
            && PostMessage(window, 0x0101, (IntPtr)0x0d, (IntPtr)unchecked((int)0xc0000001u));
    }
}
'@
    }

    function Range-Pattern($Root) {
        $item = Element $Root 'liked-scrollbar'
        $pattern = $null
        if ($item -and $item.TryGetCurrentPattern([System.Windows.Automation.RangeValuePattern]::Pattern, [ref]$pattern)) { $pattern }
    }

    function Selected($Element) {
        $pattern = $null
        if ($Element -and $Element.TryGetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern, [ref]$pattern)) { $pattern.Current.IsSelected }
    }

    function Field-Value($Root, [string]$Id) {
        $element = Element $Root $Id
        if ($element) { $element.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern).Current.Value }
    }

    function Set-CurrentField($Root, [string]$Id, [string]$Value) {
        $null = Await {
            $field = Element $Root $Id
            $valuePattern = $null
            if ($field -and $field.TryGetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern, [ref]$valuePattern)) {
                if ($valuePattern.Current.Value -ceq $Value) { return $true }
                $valuePattern.SetValue($Value)
            }
        } "$Id did not accept its value."
    }

    function First-VisibleTrack($Root) {
        $view = Element $Root 'music-liked-page'
        if (!$view) { return }
        $frame = $view.Current.BoundingRectangle
        $view.FindAll($scope, [System.Windows.Automation.Condition]::TrueCondition) |
            Where-Object {
                $_.Current.AutomationId -match '^library-row-\d+$' -and
                $_.Current.BoundingRectangle.Bottom -gt $frame.Top -and
                $_.Current.BoundingRectangle.Top -lt $frame.Bottom
            } |
            Sort-Object { $_.Current.BoundingRectangle.Top } |
            Select-Object -First 1
    }

    function Track-Top($Root, $Track) {
        $Track.Current.BoundingRectangle.Top - (Element $Root 'music-liked-page').Current.BoundingRectangle.Top
    }

    function Result-Rows($Root, [string]$Kind) {
        $Root.FindAll($scope, [System.Windows.Automation.Condition]::TrueCondition) |
            Where-Object { $_.Current.AutomationId -like "music-catalog-result-*-$Kind-*" }
    }

    function Scroll-Liked($Root, [IntPtr]$Handle, [int]$Delta) {
        $viewport = (Element $Root 'music-liked-page').Current.BoundingRectangle
        $x = [int]($viewport.Left + $viewport.Width / 2)
        $y = [int]($viewport.Top + $viewport.Height / 2)
        if (![MediadekaAuditInput]::Wheel($Handle, $x, $y, $Delta)) { throw 'Unable to post the wheel event.' }
    }

    $env:MEDIADEKA_PROFILE_DIR = $profilePath
    $process = Start-Process -FilePath $program -WorkingDirectory (Split-Path -Parent $program) -WindowStyle Hidden -PassThru
    $env:MEDIADEKA_PROFILE_DIR = $previous
    $handle = Await {
        if ($process.HasExited) { throw "Mediadeka exited ($($process.ExitCode))." }
        $found = [MediadekaMusicProbe]::Find([uint32]$process.Id)
        if ($found -ne [IntPtr]::Zero) { $found }
    } 'Mediadeka HWND missing.'
    [MediadekaMusicProbe]::RenderOffscreen($handle)
    $root = Await {
        $candidate = $automation::FromHandle($handle)
        if ((Element $candidate 'home-navigation') -and (Element $candidate 'home-accounts')) { $candidate }
    } 'Workspace unavailable.'

    $report = [ordered]@{}
    Invoke-Element (Await { Element $root 'home-settings' } 'Settings unavailable.')
    $null = Await { Element $root 'settings-connections' } 'Connections screen did not open.'
    if (Element $root 'home-sources') { throw 'Home platform chips remained in Settings.' }
    $report.settings = @{ sourceSwitcherVisible = $false }

    Invoke-Element (Await { Element $root 'home-liked' } 'Liked unavailable.')
    $likedSidebar = Await { Element $root 'home-liked' } 'Liked sidebar element disappeared.'
    $toggle = $likedSidebar.GetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern)
    $null = Await { $toggle.Current.ToggleState -eq 'On' } 'Current sidebar route is unavailable to UI Automation.'
    $sidebarHelp = $likedSidebar.Current.HelpText
    $sourceId = "liked-source-$LibraryProvider"
    Invoke-Element (Await { Element $root 'library-filters' } 'Library filters unavailable.')
    $sourceAction = Await { Element $root $sourceId } "$LibraryProvider is unavailable in the audit profile."
    Invoke-Element $sourceAction
    $null = Await { Selected (Element $root $sourceId) } 'The platform filter did not select.'
    Invoke-Element (Element $root 'library-filters')
    $filter = $LibraryQuery
    Set-CurrentField $root 'music-filter' $filter
    $null = Await { !(Element $root 'collection-loading') } 'Liked refresh did not finish.' 90
    $scroll = Await {
        $pattern = Range-Pattern $root
        if ($pattern -and $pattern.Current.Maximum -gt 550) { $pattern }
    } "The audit profile has too few filtered $LibraryProvider tracks to measure scroll restoration."
    Scroll-Liked $root $handle -960
    $null = Await { $scroll.Current.Value -gt 250. } 'Liked list did not scroll.'
    $beforeValue = $scroll.Current.Value
    $beforeTrack = Await { First-VisibleTrack $root } 'A scrolled track did not render.'
    $before = Semantic $beforeTrack
    $beforeTop = Track-Top $root $beforeTrack

    Invoke-Element (Await { Element $root 'home-playlists' } 'Playlists unavailable.')
    $null = Await { Selected (Element $root 'library-tab-playlists') } 'Playlists did not open.'
    Invoke-Element (Await { Element $root 'home-liked' } 'Liked unavailable after Playlists.')
    $nextFilter = 'filter-audit-second-entry-987654321'
    Set-CurrentField $root 'music-filter' $nextFilter

    Invoke-Element (Await { Element $root 'workspace-back' } 'Back from second Liked entry unavailable.')
    $null = Await { Selected (Element $root 'library-tab-playlists') } 'Back did not restore Playlists.'
    Invoke-Element (Await { Element $root 'workspace-back' } 'Back from Playlists unavailable.')
    $after = Await {
        $track = First-VisibleTrack $root
        if ($track -and $track.Current.Name -eq $before.name) { $track }
    } 'Back did not restore the top visible liked track.' 90
    $afterTop = Track-Top $root $after
    $afterTrack = Semantic $after
    if ([Math]::Abs($beforeTop - $afterTop) -gt 2.) { throw 'Back shifted the restored track in its viewport.' }
    $restored = Await {
        $pattern = Range-Pattern $root
        if ($pattern -and $pattern.Current.Value -gt 0) { $pattern }
    } 'Back did not restore a scrolled position.'
    $restoredValue = $restored.Current.Value
    Scroll-Liked $root $handle 4800
    $null = Await { (Field-Value $root 'music-filter') -ceq $filter } 'Back did not restore the first filter.'
    Invoke-Element (Await { Element $root 'library-filters' } 'Library filters unavailable after Back.')
    $null = Await { Selected (Element $root $sourceId) } 'Back did not restore the same platform selection.'

    Invoke-Element (Await { Element $root 'workspace-forward' } 'Forward to Playlists unavailable.')
    $null = Await { Selected (Element $root 'library-tab-playlists') } 'Forward did not restore Playlists.'
    Invoke-Element (Await { Element $root 'workspace-forward' } 'Forward to second Liked entry unavailable.')
    $null = Await { (Field-Value $root 'music-filter') -ceq $nextFilter } 'Forward did not restore the second filter.'
    $report.likedHistory = [ordered]@{
        firstFilter = $filter; selectedSource = $sourceId; beforeScroll = $beforeValue; restoredScroll = $restoredValue;
        firstVisibleBefore = $before; firstVisibleAfterBack = $afterTrack; secondFilterAfterForward = $nextFilter;
        firstTrackTopBefore = $beforeTop; firstTrackTopAfterBack = $afterTop;
        sidebarToggle = 'On'; sidebarHelpText = $sidebarHelp
    }
    Write-Output 'Liked filters, source selection, scroll and current sidebar state passed.'

    Invoke-Element (Await { Element $root 'home-page' } 'Home unavailable.')
    Set-CurrentField $root 'home-search' 'ПЕРВЫЙ РАЗ GATASKI'
    Invoke-Element (Await { Element $root 'home-catalog-search' } 'Catalog entry missing.')
    $candidate = Await {
        Result-Rows $root 'track' | Where-Object { $_.Current.Name -like 'ПЕРВЫЙ РАЗ · *GATASKI*' } | Select-Object -First 1
    } 'Known recording was not found by the connected catalogs.' 75
    $first = Semantic $candidate
    $pendingWhenFound = [bool](Element $root 'music-search-loading')
    $countBefore = @(Result-Rows $root 'track').Count
    $candidate.SetFocus()
    $null = Await { (Element $root $first.id).Current.HasKeyboardFocus } 'Stable result could not acquire keyboard focus.'
    $null = Await { !(Element $root 'music-search-loading') } 'Search did not settle.' 75
    $focusedSnapshot = Element $root $first.id
    $nativeFocusAfterReplies = [bool]($focusedSnapshot -and $focusedSnapshot.Current.HasKeyboardFocus)
    $stable = Await {
        $row = Element $root $first.id
        if ($row -and $row.Current.Name -eq $first.name) { Semantic $row }
    } 'The same result ID disappeared or changed its target after the remaining replies.'
    $countAfter = @(Result-Rows $root 'track').Count
    if (![MediadekaAuditInput]::Enter($handle)) { throw 'Unable to post Enter to the focused result.' }
    $opened = Await { $title = Element $root 'music-object-title'; if ($title -and $title.Current.Name -eq 'ПЕРВЫЙ РАЗ') { $title } } 'Enter stopped opening the focused result after late replies.' 45
    $keyboardOpened = Semantic $opened

    Invoke-Element (Await { Element $root 'workspace-back' } 'Back to the track search is unavailable.')
    $null = Await { Element $root $first.id } 'Back did not restore the track category result.' 75
    $album = Await { Find-Name $root 'Альбомы и синглы' } 'Album category unavailable.'
    Invoke-Element $album
    $null = Await { Selected (Find-Name $root 'Альбомы и синглы') } 'Album category did not select.'
    if (@(Result-Rows $root 'track').Count -ne 0) { throw 'Old track rows remained actionable after category selection.' }
    if ((Field-Value $root 'home-search') -cne 'ПЕРВЫЙ РАЗ GATASKI') { throw 'Category change lost the query.' }
    $report.searchKind = [ordered]@{
        stableResultBefore = $first; stableResultAfter = $stable; pendingWhenFocused = $pendingWhenFound;
        resultCountBefore = $countBefore; resultCountAfter = $countAfter;
        nativeFocusAfterReplies = $nativeFocusAfterReplies; keyboardOpened = $keyboardOpened;
        oldTrackRowsAfterSwitch = 0; selectedCategory = 'Альбомы и синглы'; retainedQuery = (Field-Value $root 'home-search')
    }
    Write-Output 'Catalog category update, stable result identity and focus passed.'

    $report.transport = Semantic (Await { Element $root 'home-transport' } 'Shared player missing.')
    $report | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $directory 'routes-uia.json') -Encoding UTF8
} finally {
    $env:MEDIADEKA_PROFILE_DIR = $previous
    if ($process -and !$process.HasExited) { Stop-Process -Id $process.Id; $process.WaitForExit() }
    if ($profileCopy -eq $isolated -and (Split-Path -Leaf $profileCopy) -match '^mediadeka-audit-[0-9a-f]{32}$') {
        Remove-Item -LiteralPath $profileCopy -Recurse -Force
    }
}
