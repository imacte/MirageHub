# Build the native daemon and the complete Studio installer without Python.
# Requires the pinned Rust toolchain, MSVC build tools and the Windows SDK.
[CmdletBinding()]
param(
    [string]$OutDir = (Join-Path $PSScriptRoot '..\..\dist'),
    [switch]$DaemonOnly,
    [switch]$IncludeClassic,
    [string]$DaemonPath
)
$ErrorActionPreference = 'Stop'
$studioRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$repoRoot = Split-Path $studioRoot
$outPath = [IO.Path]::GetFullPath($OutDir)
$target = 'x86_64-pc-windows-msvc'
$release = Join-Path $studioRoot "target\$target\release"
$providedDaemon = if ($DaemonPath) { (Resolve-Path $DaemonPath).Path } else { $null }
$savedEnv = @{}
foreach ($name in 'RUSTFLAGS','CARGO_ENCODED_RUSTFLAGS','OPENRAZER_STUDIO_CHROMA','OPENRAZER_STUDIO_PAYLOAD','OPENRAZER_EMBED_DAEMON') {
    $savedEnv[$name] = [Environment]::GetEnvironmentVariable($name,'Process')
}
function CargoBuild([string[]]$CargoArguments) {
    & cargo @CargoArguments
    if ($LASTEXITCODE -ne 0) { throw "cargo failed: $($CargoArguments -join ' ')" }
}
function SelfTest([string]$Exe, [switch]$RequireDaemon) {
    $testArgs = @('--self-test')
    if ($RequireDaemon) { $testArgs += '--require-daemon' }
    $p = Start-Process -FilePath $Exe -ArgumentList $testArgs -WindowStyle Hidden -Wait -PassThru
    if ($p.ExitCode -ne 0) { throw "Self-test failed: $Exe ($($p.ExitCode))" }
}
function CopyArtifact([string]$Source, [string]$Destination) {
    if (-not [string]::Equals([IO.Path]::GetFullPath($Source),[IO.Path]::GetFullPath($Destination),[StringComparison]::OrdinalIgnoreCase)) {
        Copy-Item -LiteralPath $Source -Destination $Destination -Force
    }
}
Push-Location $studioRoot
try {
    # Explicit targets keep this flag off host proc-macro/build-script crates.
    # Link the CRT into shipped binaries so no VC++ redistributable is needed.
    $env:RUSTFLAGS = '-C target-feature=+crt-static'
    # Recent PowerShell/.NET versions preserve an empty environment value.
    # An empty CARGO_ENCODED_RUSTFLAGS would override RUSTFLAGS completely.
    $env:CARGO_ENCODED_RUSTFLAGS = "-C$([char]0x1f)target-feature=+crt-static"
    if ($providedDaemon) { $daemon = $providedDaemon }
    else {
        CargoBuild -CargoArguments @('build','--release','--locked','--target',$target,'-p','rsrazer-daemon','--bin','rsrazer-daemon')
        $daemon = Join-Path $release 'rsrazer-daemon.exe'
    }
    $version = (& $daemon --version | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or $version -notlike '*Rust backend*') { throw 'The payload is not the native Rust daemon.' }
    & $daemon supported viper --json | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'Native device database smoke test failed.' }
    & (Join-Path $PSScriptRoot 'check-native.ps1') -Files @($daemon)
    New-Item -ItemType Directory -Force -Path $outPath | Out-Null
    if ($DaemonOnly) {
        CopyArtifact $daemon (Join-Path $outPath 'rsrazer-daemon.exe')
        return
    }
    CargoBuild -CargoArguments @('build','--release','--locked','--target',$target,'-p','rsrazer-chroma')
    CargoBuild -CargoArguments @('build','--release','--locked','--target','i686-pc-windows-msvc','-p','rsrazer-chroma')
    $chroma = Join-Path $studioRoot 'target\native-chroma'
    New-Item -ItemType Directory -Force -Path $chroma | Out-Null
    Copy-Item -LiteralPath (Join-Path $release 'rzchromasdk.dll') -Destination (Join-Path $chroma 'RzChromaSDK64.dll') -Force
    Copy-Item -LiteralPath (Join-Path $studioRoot 'target\i686-pc-windows-msvc\release\rzchromasdk.dll') -Destination (Join-Path $chroma 'RzChromaSDK.dll') -Force
    $env:OPENRAZER_STUDIO_CHROMA = $chroma
    CargoBuild -CargoArguments @('build','--release','--locked','--target',$target,'-p','rsrazer')
    $app = Join-Path $release 'rsrazer.exe'
    SelfTest $app
    $payload = Join-Path $studioRoot 'target\native-payload'
    New-Item -ItemType Directory -Force -Path $payload | Out-Null
    Copy-Item -LiteralPath $app -Destination (Join-Path $payload 'rsrazer.exe') -Force
    Copy-Item -LiteralPath $daemon -Destination (Join-Path $payload 'rsrazer-daemon.exe') -Force
    $env:OPENRAZER_STUDIO_PAYLOAD = $payload
    CargoBuild -CargoArguments @('build','--release','--locked','--target',$target,'-p','rsrazer-setup')
    $setup = Join-Path $release 'rsrazer-setup.exe'
    SelfTest $setup
    & (Join-Path $PSScriptRoot 'check-native.ps1') -Files @($app,$setup,(Join-Path $chroma 'RzChromaSDK64.dll'),(Join-Path $chroma 'RzChromaSDK.dll'))
    if ($IncludeClassic) {
        Push-Location (Join-Path $repoRoot 'gui')
        try {
            $env:OPENRAZER_EMBED_DAEMON = $daemon
            CargoBuild -CargoArguments @('build','--release','--locked','--target',$target)
            $classic = Join-Path $repoRoot "gui\target\$target\release\rsrazer-classic.exe"
            SelfTest $classic -RequireDaemon
            & (Join-Path $PSScriptRoot 'check-native.ps1') -Files @($classic)
        } finally { Pop-Location }
    }
    foreach ($file in @($daemon,$app,$setup)) {
        CopyArtifact $file (Join-Path $outPath (Split-Path $file -Leaf))
    }
    # Remove the duplicate installer left by earlier builds in this output folder.
    $obsoleteInstaller = Join-Path $outPath 'rsrazer-setup-zh-CN.exe'
    if (Test-Path -LiteralPath $obsoleteInstaller -PathType Leaf) {
        Remove-Item -LiteralPath $obsoleteInstaller -Force
    }
    if ($IncludeClassic) { Copy-Item -LiteralPath $classic -Destination (Join-Path $outPath 'rsrazer-classic.exe') -Force }
    $names = @('rsrazer-daemon.exe','rsrazer.exe','rsrazer-setup.exe')
    if ($IncludeClassic) { $names += 'rsrazer-classic.exe' }
    $lines = foreach ($name in $names) {
        $hash = (Get-FileHash -LiteralPath (Join-Path $outPath $name) -Algorithm SHA256).Hash.ToLowerInvariant()
        "$hash  $name"
    }
    [IO.File]::WriteAllLines((Join-Path $outPath 'SHA256SUMS-native.txt'),[string[]]$lines,[Text.UTF8Encoding]::new($false))
    Write-Output "Native builds ready: $outPath"
} finally {
    Pop-Location
    foreach ($name in $savedEnv.Keys) {
        if ($null -eq $savedEnv[$name]) { Remove-Item -LiteralPath "Env:$name" -ErrorAction SilentlyContinue }
        else { [Environment]::SetEnvironmentVariable($name,$savedEnv[$name],'Process') }
    }
}
