# Install RsRazer, break it, repair it and remove it, checking each step.
#
#   studio/tools/installer-cycle.ps1 -Setup path\to\rsrazer-setup.exe -Dir C:\some\scratch\folder
#
# It installs for the current user and leaves nothing behind when it passes:
# the install folder, its Start menu entries and its Apps & features entry all
# go again. Point LOCALAPPDATA at a scratch folder first to keep the run's
# settings files out of your own.
param([Parameter(Mandatory)] [string]$Setup, [Parameter(Mandatory)] [string]$Dir)
$ErrorActionPreference = 'Stop'
$key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\RsRazer'
$menu = Join-Path ([Environment]::GetFolderPath('Programs')) 'RsRazer'
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$Setup = (Resolve-Path -LiteralPath $Setup).Path
$Dir = [IO.Path]::GetFullPath($Dir)
# This is a destructive install/repair/uninstall test for a clean CI account.
# LOCALAPPDATA alone does not isolate HKCU or the user's real Start menu.
if ((Test-Path -LiteralPath $Dir) -or (Test-Path $key) -or (Test-Path -LiteralPath $menu) -or
    (Get-ItemProperty $runKey -ErrorAction SilentlyContinue).OpenRazerWin) {
    throw 'Use a clean Windows account with no OpenRazer installation, startup entry or existing target directory.'
}
if ($Dir -eq [IO.Path]::GetPathRoot($Dir)) { throw 'The install test target cannot be a drive root.' }
$failed = $false
function Check($what, $ok) { if ($ok) { "ok   $what" } else { "FAIL $what"; $script:failed = $true } }
$version = (& $Setup --version | Out-String).Trim().Split(' ')[-1]

& $Setup --install --silent --dir $Dir --no-autostart | Out-Host
Check 'install exit code' ($LASTEXITCODE -eq 0)
foreach ($f in 'rsrazer.exe', 'rsrazer-daemon.exe', 'setup.exe', 'install.json', 'LICENSE.txt') { Check "installed $f" (Test-Path (Join-Path $Dir $f)) }
$reg = Get-ItemProperty $key
Check 'registered in Apps & features' ($reg.DisplayVersion -eq $version -and $reg.InstallLocation -eq $Dir)
Check 'uninstall command' ($reg.UninstallString -like '*setup.exe" --uninstall')
Check 'modify command' ($reg.ModifyPath -like '*setup.exe" --maintenance')
Check 'start menu entry' (Test-Path (Join-Path $menu 'RsRazer.lnk'))
Check 'start menu repair entry' ((Get-ChildItem $menu -Filter '*.lnk').Count -eq 2)
$run = (Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -ErrorAction SilentlyContinue).OpenRazerWin
Check 'autostart left alone' (-not ($run -like "*$Dir*"))

# Exercise the installed payload, using only simulated devices and isolated data.
$daemon = Join-Path $Dir 'rsrazer-daemon.exe'
$daemonVersion = (& $daemon --version | Out-String).Trim()
Check 'native Rust daemon installed' ($LASTEXITCODE -eq 0 -and $daemonVersion -like '*Rust backend*')
$demoConfig = Join-Path $env:LOCALAPPDATA 'openrazer-install-test'
try {
    & $daemon daemon start --emulate --config-dir $demoConfig | Out-Host
    Check 'installed daemon starts' ($LASTEXITCODE -eq 0)
    # Startup publishes its endpoint before discovery; wait for its ready state.
    $ready = $false
    for ($i = 0; $i -lt 60; $i++) {
        $state = & $daemon daemon status --emulate --config-dir $demoConfig | ConvertFrom-Json
        if ($state.state -eq 'ready') { $ready = $true; break }
        Start-Sleep -Milliseconds 100
    }
    Check 'installed daemon becomes ready' $ready
    $devices = & $daemon list --emulate --config-dir $demoConfig --json | ConvertFrom-Json
    Check 'installed daemon enumerates simulated devices' ($LASTEXITCODE -eq 0 -and $devices.Count -eq 4)
    & $daemon effect static red --device 0203 --emulate --config-dir $demoConfig --quiet | Out-Host
    Check 'installed daemon accepts lighting controls' ($LASTEXITCODE -eq 0)
} finally {
    & $daemon daemon stop --emulate --config-dir $demoConfig | Out-Host
}

# Break it: a missing daemon, a truncated app, unreadable settings, no Start menu.
Remove-Item (Join-Path $Dir 'rsrazer-daemon.exe')
Set-Content -Path (Join-Path $Dir 'rsrazer.exe') -Value 'broken' -NoNewline
$settings = Join-Path $env:LOCALAPPDATA 'openrazer-win\studio\settings.json'
New-Item -ItemType Directory -Force (Split-Path $settings) | Out-Null
Get-ChildItem (Split-Path $settings) -Filter 'settings.json.broken-*' | Remove-Item
Set-Content -Path $settings -Value '{ this is not json' -NoNewline
Remove-Item -Recurse $menu

& $Setup --repair --silent | Out-Host
Check 'repair exit code' ($LASTEXITCODE -eq 0)
Check 'daemon put back' (Test-Path (Join-Path $Dir 'rsrazer-daemon.exe'))
Check 'app replaced' ((Get-Item (Join-Path $Dir 'rsrazer.exe')).Length -gt 1MB)
Check 'start menu put back' (Test-Path (Join-Path $menu 'RsRazer.lnk'))
Check 'broken settings set aside' (-not (Test-Path $settings) -and (Get-ChildItem (Split-Path $settings) -Filter 'settings.json.broken-*').Count -eq 1)
& (Join-Path $Dir 'rsrazer.exe') --version | Out-Host
Check 'repaired app runs' ($LASTEXITCODE -eq 0)

# Update it, as Studio does: what was changed since installing has to survive.
Set-ItemProperty $runKey -Name OpenRazerWin -Value "`"$(Join-Path $Dir 'rsrazer.exe')`" --background"
Remove-Item -Recurse (Join-Path ([Environment]::GetFolderPath('Programs')) 'RsRazer')
& $Setup --update --silent --dir $Dir | Out-Host
Check 'update exit code' ($LASTEXITCODE -eq 0)
Check 'update kept starting with Windows' ((Get-ItemProperty $runKey).OpenRazerWin -like "*$Dir*")
Check 'update kept the Start menu entry removed' (-not (Test-Path $menu))
Check 'update recorded the options in place' ((Get-Content (Join-Path $Dir 'install.json') | ConvertFrom-Json).options.autostart -eq $true)
& $Setup --repair --silent | Out-Host

# An update left waiting is not one of the settings that are kept.
$updates = Join-Path $env:LOCALAPPDATA 'openrazer-win\studio\updates'
New-Item -ItemType Directory -Force $updates | Out-Null
Set-Content -Path (Join-Path $updates 'rsrazer-setup-9.9.9.exe') -Value 'waiting' -NoNewline

# Remove it with the copy Apps & features runs, which hands over to a copy in TEMP.
& (Join-Path $Dir 'setup.exe') --uninstall --silent | Out-Host
for ($i = 0; $i -lt 60 -and (Test-Path $Dir); $i++) { Start-Sleep -Milliseconds 500 }
Check 'install folder gone' (-not (Test-Path $Dir))
Check 'unregistered' (-not (Test-Path $key))
Check 'start menu gone' (-not (Test-Path $menu))
Check 'settings kept by default' ((Get-ChildItem (Split-Path $settings) -Filter 'settings.json.broken-*').Count -eq 1)
Check 'waiting update removed' (-not (Test-Path $updates))
Check 'starting with Windows removed' (-not ((Get-ItemProperty $runKey -ErrorAction SilentlyContinue).OpenRazerWin -like "*$Dir*"))
Start-Sleep -Seconds 6
Check 'temporary uninstaller cleaned up' ((Get-ChildItem $env:TEMP -Filter 'rsrazer-uninstall-*.exe' -ErrorAction SilentlyContinue).Count -eq 0)
if ($failed) { exit 1 }
'ALL OK'
exit 0
