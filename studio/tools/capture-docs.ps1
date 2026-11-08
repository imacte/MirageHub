# Regenerate real UI screenshots using a native emulator and temporary settings.
[CmdletBinding()]
param([string]$BinDir = (Join-Path $PSScriptRoot '..\..\dist'))
$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
$bin = (Resolve-Path $BinDir).Path
$output = Join-Path $repo 'docs\images'
$sandbox = Join-Path $env:TEMP ('rsrazer-screenshots-' + [guid]::NewGuid().ToString('N'))
$config = Join-Path $sandbox 'rsrazer'
$settings = Join-Path $config 'studio\settings.json'
$daemon = Join-Path $bin 'rsrazer-daemon.exe'
$saved = @{}
foreach ($name in @('LOCALAPPDATA','OPENRAZER_STUDIO_CAPTURE','OPENRAZER_STUDIO_PAGE','OPENRAZER_STUDIO_SIZE','OPENRAZER_SETUP_CAPTURE','OPENRAZER_GUI_CAPTURE','OPENRAZER_GUI_PAGE')) {
    $saved[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}
function DaemonCommand([string[]]$Arguments) {
    $result = & $daemon @Arguments --config-dir $config --emulate
    if ($LASTEXITCODE -ne 0) { throw "Daemon command failed: $Arguments" }
    $result
}
function Capture([string]$exe, [string]$file, [string[]]$Arguments = @()) {
    $start = @{FilePath=(Join-Path $bin $exe);WindowStyle='Hidden';PassThru=$true}
    if ($Arguments.Count) { $start.ArgumentList = $Arguments }
    $process = Start-Process @start
    if (-not $process.WaitForExit(45000)) {
        Stop-Process -Id $process.Id
        throw "Screenshot timed out: $file"
    }
    if ($process.ExitCode -ne 0 -or -not (Test-Path -LiteralPath $file)) { throw "Screenshot failed: $file" }
    Write-Output $file
}
try {
    New-Item -ItemType Directory -Force (Split-Path $settings) | Out-Null
    New-Item -ItemType Directory -Force $output | Out-Null
    $cached = Join-Path $env:LOCALAPPDATA 'rsrazer\images'
    if (Test-Path -LiteralPath $cached) { Copy-Item -LiteralPath $cached -Destination (Join-Path $config 'images') -Recurse }
    $env:LOCALAPPDATA = $sandbox
    DaemonCommand @('daemon','start','--chroma-port','0') | Out-Null
    $devices = DaemonCommand @('rpc','devices.list') | ConvertFrom-Json
    $keyboard = $devices | Where-Object type -eq 'keyboard' | Select-Object -First 1
    if (-not $keyboard) { throw 'The emulator has no keyboard' }
    $keyboardIndex = [array]::IndexOf(@($devices.serial), $keyboard.serial)
    $cells = @(foreach ($row in 0..5) { foreach ($column in 0..21) { ,@($row,$column) } })
    $layers = @(
        @{ kind='gradient'; cells=$cells; colours=@(@(68,214,44),@(0,128,255)); name='RsRazer'; enabled=$true; speed=1; direction=1 },
        @{ kind='static'; cells=@(@(2,3),@(3,2),@(3,3),@(3,4)); colours=,@(255,180,40); name='WASD'; enabled=$true; speed=1; direction=1 }
    )
    $parameters = @{ serial=$keyboard.serial; effect='layers'; options=@{layers=$layers} } | ConvertTo-Json -Depth 12 -Compress
    DaemonCommand @('rpc','effect.set',$parameters) | Out-Null
    DaemonCommand @('effect','static','#44d62c','--device','0078') | Out-Null
    DaemonCommand @('effect','static','#0080ff','--device','0c00') | Out-Null
    $env:OPENRAZER_STUDIO_SIZE = '1280x900'
    foreach ($language in @('en','zh-cn')) {
        @{language=$language;check_updates=$false;auto_switch=$false} | ConvertTo-Json | Set-Content -LiteralPath $settings -Encoding utf8
        $env:OPENRAZER_STUDIO_PAGE = "lighting:${keyboardIndex}:studio"
        $file = if ($language -eq 'en') {'rsrazer-lighting.png'} else {'rsrazer-lighting-zh-CN.png'}
        $env:OPENRAZER_STUDIO_CAPTURE = Join-Path $output $file
        Capture 'rsrazer.exe' $env:OPENRAZER_STUDIO_CAPTURE
    }
    @{language='en';check_updates=$false;auto_switch=$false} | ConvertTo-Json | Set-Content -LiteralPath $settings -Encoding utf8
    foreach ($page in @('home',"lighting:${keyboardIndex}:quick")) {
        $env:OPENRAZER_STUDIO_SIZE = if ($page -eq 'home') {'1600x800'} else {'1280x900'}
        $env:OPENRAZER_STUDIO_PAGE = $page
        $name = if ($page -eq 'home') {'home'} else {'quick'}
        $env:OPENRAZER_STUDIO_CAPTURE = Join-Path $output "rsrazer-$name.png"
        Capture 'rsrazer.exe' $env:OPENRAZER_STUDIO_CAPTURE
    }
    Remove-Item Env:OPENRAZER_STUDIO_CAPTURE
    $env:OPENRAZER_SETUP_CAPTURE = Join-Path $output 'rsrazer-setup.png'
    Capture 'rsrazer-setup.exe' $env:OPENRAZER_SETUP_CAPTURE @('--dir','C:\Apps\RsRazer')
    Remove-Item Env:OPENRAZER_SETUP_CAPTURE
    foreach ($page in @('lighting','macros')) {
        $env:OPENRAZER_GUI_PAGE = "${keyboardIndex}:$page"
        $name = if ($page -eq 'lighting') {'rsrazer-classic.png'} else {'rsrazer-classic-macros.png'}
        $env:OPENRAZER_GUI_CAPTURE = Join-Path $output $name
        Capture 'rsrazer-classic.exe' $env:OPENRAZER_GUI_CAPTURE
    }
} finally {
    if (Test-Path -LiteralPath (Join-Path $config 'daemon.json')) { DaemonCommand @('daemon','stop') | Out-Null }
    foreach ($name in $saved.Keys) {
        if ($null -eq $saved[$name]) { Remove-Item "Env:$name" -ErrorAction SilentlyContinue }
        else { [Environment]::SetEnvironmentVariable($name, $saved[$name], 'Process') }
    }
}
