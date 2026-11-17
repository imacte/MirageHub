[CmdletBinding()]
param(
    [ValidateRange(1, 3600)][int]$Seconds = 180,
    [ValidateRange(1, 60)][int]$IntervalSeconds = 2,
    [string]$OutputPath,
    [string]$DaemonPath
)

$ErrorActionPreference = 'Stop'

# Windows PowerShell 5.1 can activate Windows Runtime UISettings directly.
if ($PSVersionTable.PSEdition -ne 'Desktop') {
    Write-Host '请使用 Windows PowerShell 运行此脚本，或双击同目录的 start-theme-colour-log.cmd。'
    exit 1
}

if (-not $OutputPath) {
    $desktop = [Environment]::GetFolderPath('Desktop')
    if (-not $desktop -or -not (Test-Path -LiteralPath $desktop -PathType Container)) {
        $desktop = $env:TEMP
    }
    $OutputPath = Join-Path $desktop ('Mirage-主题色日志-' + (Get-Date -Format 'yyyyMMdd-HHmmss') + '.log')
}
$OutputPath = [IO.Path]::GetFullPath($OutputPath)
$writer = New-Object IO.StreamWriter($OutputPath, $false, (New-Object Text.UTF8Encoding($true)))
$writer.AutoFlush = $true

function Write-Log([string]$Message) {
    $line = '{0}  {1}' -f (Get-Date -Format 'yyyy-MM-dd HH:mm:ss.fff'), $Message
    $writer.WriteLine($line)
    Write-Host $line
}

function Find-Daemon {
    if ($DaemonPath) {
        if (Test-Path -LiteralPath $DaemonPath -PathType Leaf) { return (Resolve-Path -LiteralPath $DaemonPath).Path }
        return $null
    }
    # Prefer the executable belonging to the running daemon.
    foreach ($process in @(Get-Process -Name 'mirage-hub-daemon','rsrazer-daemon' -ErrorAction SilentlyContinue)) {
        try {
            if ($process.Path -and (Test-Path -LiteralPath $process.Path -PathType Leaf)) { return $process.Path }
        } catch { }
    }
    $directories = New-Object 'System.Collections.Generic.List[string]'
    foreach ($key in @('HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Mirage Hub', 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\RsRazer')) {
        $installed = Get-ItemProperty -LiteralPath $key -ErrorAction SilentlyContinue
        if ($installed -and $installed.InstallLocation) { $directories.Add([string]$installed.InstallLocation) }
    }
    $directories.Add((Join-Path $env:LOCALAPPDATA 'Programs\Mirage Hub'))
    $directories.Add((Join-Path $env:LOCALAPPDATA 'Programs\RsRazer'))
    $directories.Add($PSScriptRoot)
    foreach ($dir in $directories) {
        foreach ($name in @('mirage-hub-daemon.exe', 'rsrazer-daemon.exe')) {
            $candidate = Join-Path $dir $name
            if (Test-Path -LiteralPath $candidate -PathType Leaf) { return $candidate }
        }
    }
    $command = Get-Command 'mirage-hub-daemon.exe' -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($command) { return $command.Source }
    return $null
}

function Invoke-DaemonRpc([string]$Method) {
    if (-not $script:daemonExe) { return @{ ok = $false; error = '未找到后台程序' } }
    $process = $null
    try {
        $start = New-Object Diagnostics.ProcessStartInfo
        $start.FileName = $script:daemonExe
        $start.Arguments = 'rpc ' + $Method
        $start.UseShellExecute = $false
        $start.CreateNoWindow = $true
        $start.RedirectStandardOutput = $true
        $start.RedirectStandardError = $true
        $process = [Diagnostics.Process]::Start($start)
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit(5000)) {
            $process.Kill()
            return @{ ok = $false; error = '后台查询超时' }
        }
        $output = $stdout.GetAwaiter().GetResult()
        $errorText = $stderr.GetAwaiter().GetResult().Trim()
        if ($process.ExitCode -ne 0) {
            # Do not include local paths, serial numbers or endpoint tokens in the log.
            if ($errorText -match 'unknown method') { $reason = '后台版本不支持该查询' }
            elseif ($errorText -match 'daemon unavailable|daemon is not running') { $reason = '后台未运行或无法连接' }
            else { $reason = '后台查询失败（退出码 ' + $process.ExitCode + '）' }
            return @{ ok = $false; error = $reason }
        }
        return @{ ok = $true; value = ($output | ConvertFrom-Json) }
    } catch {
        return @{ ok = $false; error = '无法执行或解析后台查询' }
    } finally {
        if ($process) { $process.Dispose() }
    }
}

function Property-Value($Object, [string]$Name) {
    if ($null -eq $Object) { return $null }
    $property = $Object.PSObject.Properties[$Name]
    if ($null -eq $property) { return $null }
    return $property.Value
}

function Rgb-Hex($Rgb) {
    if ($null -eq $Rgb -or @($Rgb).Count -lt 3) { return '未知' }
    try {
        $r = [int]$Rgb[0]; $g = [int]$Rgb[1]; $b = [int]$Rgb[2]
        if ($r -lt 0 -or $r -gt 255 -or $g -lt 0 -or $g -gt 255 -or $b -lt 0 -or $b -gt 255) { return '未知' }
        return ('#{0:X2}{1:X2}{2:X2}' -f $r, $g, $b)
    } catch { return '未知' }
}

function Read-DwmColour {
    try {
        $value = (Get-ItemProperty -LiteralPath 'HKCU:\Software\Microsoft\Windows\DWM' -Name AccentColor).AccentColor
        if ($value -is [uint32]) { $bytes = [BitConverter]::GetBytes([uint32]$value) }
        else { $bytes = [BitConverter]::GetBytes([int32]$value) }
        return (Rgb-Hex $bytes)
    } catch { return '不可用' }
}

function Show-KeyboardState($Devices, $Effects, $Persistence) {
    $index = 0
    foreach ($device in @($Devices)) {
        if ((Property-Value $device 'type') -ne 'keyboard') { continue }
        $index++
        $serial = [string](Property-Value $device 'serial')
        $active = Property-Value (Property-Value $Effects 'active') $serial
        $stored = Property-Value $Persistence $serial
        $zone = Property-Value (Property-Value $stored 'zones') 'backlight'
        $hardwareEffect = Property-Value $zone 'effect'
        $hardwareColour = Rgb-Hex (Property-Value $zone 'colors')
        $productId = Property-Value $device 'pid'
        $productIdText = try { '{0:X4}' -f [int]$productId } catch { '未知' }
        $firmware = Property-Value $device 'firmware'
        $name = Property-Value $device 'name'
        if (-not $active) { $active = '无' }
        if (-not $hardwareEffect) { $hardwareEffect = '未知' }
        if (-not $firmware) { $firmware = '未知' }
        Write-Log ("键盘${index}: $name PID=$productIdText 固件=$firmware 软件灯效=$active 保存的硬件灯效=$hardwareEffect 保存的颜色=$hardwareColour")
    }
    if ($index -eq 0) { Write-Log '键盘：后台目前没有报告键盘设备' }
}

try {
    $script:daemonExe = Find-Daemon
    $uiSettings = $null
    try {
        [Windows.UI.ViewManagement.UISettings, Windows.UI.ViewManagement, ContentType = WindowsRuntime] | Out-Null
        $uiSettings = New-Object Windows.UI.ViewManagement.UISettings
        $accentType = [Windows.UI.ViewManagement.UIColorType]::Accent
    } catch { }

    Write-Log '主题色诊断开始；此脚本只读取状态，不会修改主题色或灯效。'
    Write-Log '请在 Windows 设置 > 个性化 > 颜色 中依次选择红、绿、蓝、黄、紫，每种保持至少 5 秒。'
    Write-Log '切回此窗口后可按 R/G/B/Y/P/W 标记键盘实际颜色，按 0 标记不亮、N 标记没有变化、X 标记其他，Q 结束。'
    Write-Host ("日志文件：$OutputPath")
    $os = Get-CimInstance Win32_OperatingSystem -ErrorAction SilentlyContinue
    if ($os) { Write-Log ("Windows 版本：$($os.Version) Build $($os.BuildNumber)") }
    $synapse = @(Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.ProcessName -match '^(razer synapse 3|razer synapse service|razer synapse service process|razerappengine|razer central|rzsynapse|razer chroma sdk server|razer chroma sdk service|gamemanagerservice3)$' } | Select-Object -ExpandProperty ProcessName -Unique)
    if ($synapse.Count) { Write-Log ('检测到 Synapse 进程：' + ($synapse -join ', ')) } else { Write-Log '未检测到 Synapse 进程' }
    $version = Invoke-DaemonRpc 'daemon.version'
    if ($version.ok) { Write-Log ('后台版本：' + (Property-Value $version.value 'version')) }
    else { Write-Log ('后台版本：' + $version.error) }

    $stopwatch = [Diagnostics.Stopwatch]::StartNew()
    $nextSample = 0.0
    $stop = $false
    while (-not $stop -and $stopwatch.Elapsed.TotalSeconds -lt $Seconds) {
        if ($stopwatch.Elapsed.TotalSeconds -ge $nextSample) {
            $nextSample = $stopwatch.Elapsed.TotalSeconds + $IntervalSeconds
            $windows = '不可用'
            if ($uiSettings) {
                try {
                    $c = $uiSettings.GetColorValue($accentType)
                    $windows = Rgb-Hex @($c.R, $c.G, $c.B)
                } catch { }
            }
            $dwm = Read-DwmColour
            $integration = Invoke-DaemonRpc 'integration.status'
            if ($integration.ok) {
                $accent = Property-Value $integration.value 'accent'
                $daemonColour = Rgb-Hex (Property-Value $accent 'colour')
                $available = Property-Value $accent 'available'
                $enabled = Property-Value $accent 'enabled'
                Write-Log ("系统主题色=$windows DWM=$dwm 后台主题色=$daemonColour 可用=$available 全局跟随=$enabled")
                $effects = Invoke-DaemonRpc 'effect.status'
                $persistence = Invoke-DaemonRpc 'daemon.persistence'
                $devices = Invoke-DaemonRpc 'devices.list'
                if ($effects.ok -and $persistence.ok -and $devices.ok) {
                    Show-KeyboardState $devices.value $effects.value $persistence.value
                } else {
                    if (-not $effects.ok) { Write-Log ('灯效状态：' + $effects.error) }
                    if (-not $persistence.ok) { Write-Log ('保存状态：' + $persistence.error) }
                    if (-not $devices.ok) { Write-Log ('设备状态：' + $devices.error) }
                }
            } else {
                Write-Log ("系统主题色=$windows DWM=$dwm 后台状态=" + $integration.error)
            }
        }

        try {
            while ([Console]::KeyAvailable) {
                $key = [Console]::ReadKey($true).KeyChar.ToString().ToUpperInvariant()
                $observed = @{ R='红色'; G='绿色'; B='蓝色'; Y='黄色'; P='紫色'; W='白色'; '0'='不亮'; N='没有变化'; X='其他颜色' }
                if ($key -eq 'Q') { $stop = $true; break }
                if ($observed.ContainsKey($key)) { Write-Log ('手动标记键盘实际表现：' + $observed[$key]) }
            }
        } catch { }
        if (-not $stop) { Start-Sleep -Milliseconds 150 }
    }
    Write-Log '诊断结束。后台保存的灯效和颜色是软件状态，不代表键盘实际发出的光。'
} finally {
    $writer.Dispose()
}
Write-Host "`n日志已保存：$OutputPath"
