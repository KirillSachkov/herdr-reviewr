# The toggle journey in a headless herdr on Windows: link this checkout with `Reviewr` in bin\,
# toggle reviewr open in a workspace over a git repo, see it paint the repo's change, toggle
# again, and see the pane gone. It proves the manifest's extension-less `bin/herdr-reviewr`
# resolves to bin\herdr-reviewr.exe for the actions and the pane alike.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows-herdr-smoke.ps1 -Reviewr <exe>
param(
    [Parameter(Mandatory = $true)] [string] $Reviewr
)

. (Join-Path $PSScriptRoot 'windows-herdr.ps1')

$checkout = Split-Path -Parent $PSScriptRoot
$plugin = 'persiyanov.reviewr'

New-Item -ItemType Directory -Force -Path (Join-Path $checkout 'bin') | Out-Null
Copy-Item -LiteralPath $Reviewr -Destination (Join-Path $checkout 'bin\herdr-reviewr.exe') -Force

# A repo with one untracked file: reviewr's file list shows its name, which only a painted
# reviewr frame can put on the screen.
$marker = 'reviewr-smoke-marker.txt'
$repo = Join-Path ([IO.Path]::GetTempPath()) "reviewr-smoke-$([guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Path $repo | Out-Null
foreach ($gitArgs in @(
        @('init', '-q'),
        @('-c', 'user.name=ci', '-c', 'user.email=ci@example.com', 'commit', '-q', '--allow-empty', '-m', 'init')
    )) {
    & git -C $repo @gitArgs
    if ($LASTEXITCODE -ne 0) { throw "git $($gitArgs -join ' ') failed" }
}
Set-Content -LiteralPath (Join-Path $repo $marker) -Value 'hello' -Encoding ascii

$herdr = Install-Herdr
Start-HerdrServer -Herdr $herdr
try {
    Invoke-Herdr plugin link $checkout | Out-Null
    $ws = New-HerdrWorkspace -Cwd $repo

    # Each toggle runs to completion before the next, as two keypresses a moment apart do.
    $open = Invoke-PluginAction toggle -Plugin $plugin
    Write-Host "toggle: exit $($open.exit_code): $($open.stdout)$($open.stderr)"
    $pane = @(Get-PaneIds $ws.Workspace | Where-Object { $_ -ne $ws.Pane })
    if ($open.exit_code -ne 0 -or $pane.Count -ne 1 -or $open.stdout -notmatch "^reviewr: opened $($pane[0]) ") {
        throw "the first toggle did not open one reviewr pane (panes: $($pane -join ', '))"
    }
    $pane = $pane[0]
    Wait-PaneText $pane ([regex]::Escape($marker)) | Write-Host
    Write-Host (Invoke-Herdr pane process-info --pane $pane)

    $close = Invoke-PluginAction toggle -Plugin $plugin
    Write-Host "toggle: exit $($close.exit_code): $($close.stdout)$($close.stderr)"
    if ($close.exit_code -ne 0 -or $close.stdout -notmatch "^reviewr: closed $pane ") {
        throw "the second toggle did not close $pane"
    }
    Wait-For "pane $pane to close" {
        -not (Get-PaneIds $ws.Workspace | Where-Object { $_ -eq $pane })
    } | Out-Null
    Write-Host "toggle closed $pane"
} catch {
    # The actions run detached from the invoke, so their own output lives in herdr's log.
    Write-Host '--- herdr plugin log ---'
    try { Invoke-Herdr plugin log list --plugin $plugin | Write-Host } catch { Write-Host $_ }
    throw
} finally {
    try { Invoke-Herdr plugin unlink $plugin | Out-Null } catch {}
    Stop-HerdrServer
}
