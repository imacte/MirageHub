# Destructive migration smoke test, only for a clean CI user after installer-cycle.
param([Parameter(Mandatory)][string]$Setup, [Parameter(Mandatory)][string]$Root)
$ErrorActionPreference = 'Stop'
$Setup = (Resolve-Path -LiteralPath $Setup).Path
$Root = [IO.Path]::GetFullPath($Root)
$oldKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\RsRazer'
$newKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Mirage Hub'
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$programs = [Environment]::GetFolderPath('Programs')
if ((Test-Path -LiteralPath $Root) -or (Test-Path $oldKey) -or (Test-Path $newKey) -or
    (Test-Path -LiteralPath (Join-Path $programs 'RsRazer')) -or
    (Test-Path -LiteralPath (Join-Path $programs 'Mirage Hub')) -or
    (Get-ItemProperty $runKey -ErrorAction SilentlyContinue).OpenRazerWin) {
    throw 'Migration smoke test requires a clean CI user with no existing installation.'
}
if ($Root -eq [IO.Path]::GetPathRoot($Root)) { throw 'A drive root is not a test directory.' }
$old = Join-Path $Root 'old-install'
$new = Join-Path $Root 'new-install'
$data = Join-Path $Root 'data'
$env:LOCALAPPDATA = $data
New-Item -ItemType Directory -Force $old,(Join-Path $data 'rsrazer\studio') | Out-Null
Set-Content -LiteralPath (Join-Path $old 'rsrazer.exe') -Value 'obsolete app'
Set-Content -LiteralPath (Join-Path $old 'rsrazer-daemon.exe') -Value 'obsolete daemon'
Set-Content -LiteralPath (Join-Path $old 'keep-user-file.txt') -Value 'keep this'
Set-Content -LiteralPath (Join-Path $data 'rsrazer\profiles.json') -Value '{"migration_test":true}'
Set-Content -LiteralPath (Join-Path $data 'rsrazer\studio\settings.json') -Value '{"language":"zh-cn"}'
Set-Content -LiteralPath (Join-Path $data 'rsrazer\daemon.json') -Value '{"token":"do-not-copy"}'
New-Item -Path $oldKey -Force | Out-Null
Set-ItemProperty $oldKey -Name InstallLocation -Value $old
Set-ItemProperty $oldKey -Name DisplayVersion -Value '1.8.2'
New-Item -Path $runKey -Force | Out-Null
Set-ItemProperty $runKey -Name OpenRazerWin -Value "`"$old\rsrazer.exe`" --background"
& $Setup --install --silent --dir $new --no-autostart | Out-Host
if ($LASTEXITCODE -ne 0) { throw 'Migration installer failed.' }
if ((Test-Path $oldKey) -or -not (Test-Path $newKey)) { throw 'Registration did not migrate.' }
if ((Test-Path -LiteralPath (Join-Path $old 'rsrazer.exe')) -or
    (Test-Path -LiteralPath (Join-Path $old 'rsrazer-daemon.exe'))) { throw 'Obsolete executables remain.' }
if (-not (Test-Path -LiteralPath (Join-Path $old 'keep-user-file.txt'))) { throw 'User file was removed.' }
foreach ($relative in 'profiles.json','studio\settings.json') {
    $source = Join-Path $data "rsrazer\$relative"
    $target = Join-Path $data "mirage-hub\$relative"
    if ((Get-Content -Raw -LiteralPath $source) -ne (Get-Content -Raw -LiteralPath $target)) { throw "Settings did not migrate: $relative" }
}
if (Test-Path -LiteralPath (Join-Path $data 'mirage-hub\daemon.json')) { throw 'Old endpoint was copied.' }
if ((Get-ItemProperty $runKey).OpenRazerWin) { throw 'Old startup command remains.' }
& $Setup --uninstall --silent --dir $new | Out-Host
if ($LASTEXITCODE -ne 0 -or (Test-Path $newKey)) { throw 'Removal after migration failed.' }
Write-Output 'RsRazer migration, settings preservation and removal passed.'
