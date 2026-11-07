# Interactive Windows smoke test. Uses an isolated emulator/configuration and
# refuses to run beside an existing Studio instance. Never controls real devices.
[CmdletBinding()]
param([string]$BinDir = (Join-Path $PSScriptRoot '..\..\dist'))
$ErrorActionPreference = 'Stop'
$bin = (Resolve-Path -LiteralPath $BinDir).Path
$exe = Join-Path $bin 'rsrazer.exe'
$daemon = Join-Path $bin 'rsrazer-daemon.exe'
if (Get-Process -Name rsrazer -ErrorAction SilentlyContinue) { throw 'Exit RsRazer from its tray menu before running this test.' }
$sandbox = Join-Path $env:TEMP ('rsrazer-tray-test-' + [guid]::NewGuid().ToString('N'))
$config = Join-Path $sandbox 'openrazer-win'
$studio = Join-Path $config 'studio'
$originalLocal = $env:LOCALAPPDATA
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$oldRun = (Get-ItemProperty -LiteralPath $runKey -ErrorAction SilentlyContinue).OpenRazerWin
$hostProcess = $null
$testChildren = [Collections.Generic.List[int]]::new()
Add-Type @'
using System; using System.Text; using System.Runtime.InteropServices;
public static class RazerTrayCycle {
 public delegate bool Callback(IntPtr h,IntPtr l);
 [DllImport("user32.dll")] public static extern bool EnumWindows(Callback cb,IntPtr l);
 [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h,out uint p);
 [DllImport("user32.dll",CharSet=CharSet.Unicode,ExactSpelling=true)] public static extern int GetWindowTextW(IntPtr h,StringBuilder t,int n);
 [DllImport("user32.dll",CharSet=CharSet.Unicode,ExactSpelling=true)] public static extern IntPtr FindWindowW(string c,string t);
 [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
 [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h,uint m,UIntPtr w,IntPtr l);
 [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h,int c);
 public static IntPtr Window(uint pid) { IntPtr found=IntPtr.Zero; EnumWindows((h,l)=>{uint p; GetWindowThreadProcessId(h,out p);var t=new StringBuilder(256);GetWindowTextW(h,t,256);if(p==pid && t.ToString()=="RsRazer"){found=h;return false;}return true;},IntPtr.Zero);return found; }
}
'@
function Check([string]$name,[bool]$passed) { if(-not$passed){throw "FAIL: $name"}; Write-Host "PASS: $name" }
function DaemonCommand([string[]]$Arguments) {
 $value = & $daemon @Arguments --emulate --config-dir $config
 if($LASTEXITCODE-ne0){throw "Daemon command failed: $Arguments"}; $value
}
function Children { @(Get-CimInstance Win32_Process -Filter "ParentProcessId = $($hostProcess.Id) AND Name = 'rsrazer.exe'") }
function OpenUI {
 $second = Start-Process -FilePath $exe -WindowStyle Normal -PassThru
 Check 'second invocation returns' ($second.WaitForExit(5000) -and $second.ExitCode-eq0)
 for($i=0;$i-lt100;$i++){
  $children=Children
  if($children.Count-eq1){
   $candidate=Get-Process -Id $children[0].ProcessId
   $window=[RazerTrayCycle]::Window($candidate.Id)
   if($window-ne[IntPtr]::Zero -and [RazerTrayCycle]::IsWindowVisible($window)){
    $testChildren.Add($candidate.Id); return $candidate
   }
  }
  Start-Sleep -Milliseconds 100
 }
 $children | Format-List | Out-Host
 Get-Content -LiteralPath (Join-Path $studio 'ui.log') -Tail 20 -ErrorAction SilentlyContinue | Out-Host
 throw 'UI child did not open'
}
function CloseUI($process) {
 $window=[RazerTrayCycle]::Window($process.Id)
 [void][RazerTrayCycle]::PostMessageW($window,0x10,[UIntPtr]::Zero,[IntPtr]::Zero)
 Check 'closing ends the UI process' $process.WaitForExit(8000)
 Start-Sleep -Milliseconds 700
 Check 'tray remains without a UI child' (-not$hostProcess.HasExited -and (Children).Count-eq0)
}
function Memory([string]$phase) {
 $counter=Get-CimInstance Win32_PerfFormattedData_PerfProc_Process -Filter "IDProcess = $($hostProcess.Id)"
 [pscustomobject]@{Phase=$phase;HostPID=$hostProcess.Id;PrivateWorkingSetMiB=[Math]::Round($counter.WorkingSetPrivate/1MB,2);PrivateCommitMiB=[Math]::Round($counter.PrivateBytes/1MB,2)}
}
try {
 New-Item -ItemType Directory -Path $studio -Force | Out-Null
 $env:LOCALAPPDATA=$sandbox
 @{language='zh-cn';check_updates=$false;auto_switch=$false;close_to_tray=$true} | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $studio 'settings.json')
 @{page='Macros';macros=@{draft=@{index=$null;name='未保存的测试宏';kind='Text';steps=@();text="草稿第一行`n第二行";target='';pick=65};selected=$null}} | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $studio 'session.json')
 DaemonCommand @('daemon','start','--chroma-port','0') | Out-Null
 $hostProcess=Start-Process -FilePath $exe -ArgumentList '--background' -WindowStyle Hidden -PassThru
 Start-Sleep -Seconds 4
 Check 'background startup creates only the tray' (-not$hostProcess.HasExited -and (Children).Count-eq0)
 $measurements=@(Memory 'background-start')
 $first=OpenUI
 Start-Sleep -Seconds 3
 $again=OpenUI
 Check 'repeated open reuses the UI process' ($again.Id-eq$first.Id)
 CloseUI $first
 $saved=Get-Content -LiteralPath (Join-Path $studio 'session.json') -Raw | ConvertFrom-Json
 Check 'page and unsaved macro survive close' ($saved.page-eq'Macros' -and $saved.macros.draft.name-eq'未保存的测试宏' -and $saved.macros.draft.text-eq"草稿第一行`n第二行")
 $measurements+=Memory 'after-close'
 foreach($cycle in 1..3){
  $next=OpenUI
  Check 'reopening creates a fresh UI process' ($next.Id-ne$first.Id)
  Start-Sleep -Seconds 2
  if($cycle-eq1){
   [void][RazerTrayCycle]::ShowWindow([RazerTrayCycle]::Window($next.Id),6)
   Start-Sleep -Milliseconds 500
   $restored=OpenUI
   Check 'minimized UI restores without duplication' ($restored.Id-eq$next.Id)
  }
  CloseUI $next
 }
 $measurements+=Memory 'after-repeated-close'
 $devices=DaemonCommand @('list','--json') | ConvertFrom-Json
 Check 'devices remain available without the UI' ($devices.Count-eq4)
 # Verify the actual named quit event used by the installer, with a UI open.
 $next=OpenUI
 $quit=[Threading.EventWaitHandle]::OpenExisting('Local\rsrazer-quit')
 try{[void]$quit.Set()}finally{$quit.Dispose()}
 Check 'installer quit closes the UI' $next.WaitForExit(10000)
 Check 'installer quit closes the tray' $hostProcess.WaitForExit(10000)
 # Opting out of tray residency must still exit both processes on window close.
 @{language='zh-cn';check_updates=$false;auto_switch=$false;close_to_tray=$false} | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $studio 'settings.json')
 $hostProcess=Start-Process -FilePath $exe -ArgumentList '--background' -WindowStyle Hidden -PassThru
 Start-Sleep -Seconds 2
 $next=OpenUI
 [void][RazerTrayCycle]::PostMessageW([RazerTrayCycle]::Window($next.Id),0x10,[UIntPtr]::Zero,[IntPtr]::Zero)
 Check 'opting out of the tray closes the UI' $next.WaitForExit(10000)
 Check 'opting out of the tray closes the host' $hostProcess.WaitForExit(10000)
 $measurements | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $bin 'tray-split-smoke.json')
 $measurements | Format-Table | Out-Host
 Write-Host "Session fixture and logs: $studio"
} finally {
 if($hostProcess -and -not$hostProcess.HasExited){
  $window=[RazerTrayCycle]::FindWindowW('RsRazer.TrayHost',$null)
  if($window-ne[IntPtr]::Zero){[void][RazerTrayCycle]::PostMessageW($window,0x8017,[UIntPtr]::Zero,[IntPtr]::Zero)}
  [void]$hostProcess.WaitForExit(5000)
  foreach($candidate in (Children)){$testChildren.Add($candidate.ProcessId)}
  foreach($processId in @($testChildren.ToArray())+@($hostProcess.Id)){
   $process=Get-Process -Id $processId -ErrorAction SilentlyContinue
   if($process -and $process.Path-eq$exe){Stop-Process -Id $processId -ErrorAction SilentlyContinue}
  }
 }
 try { if(Test-Path -LiteralPath (Join-Path $config 'daemon.json')){DaemonCommand @('daemon','stop') | Out-Null} }
 catch { Write-Warning "Could not stop the test emulator: $_" }
 $env:LOCALAPPDATA=$originalLocal
 $currentRun=(Get-ItemProperty -LiteralPath $runKey -ErrorAction SilentlyContinue).OpenRazerWin
 if($currentRun -and $currentRun.Contains($exe)){
  if($null-ne$oldRun){Set-ItemProperty -LiteralPath $runKey -Name OpenRazerWin -Value $oldRun}
  else{Remove-ItemProperty -LiteralPath $runKey -Name OpenRazerWin}
 }
}
