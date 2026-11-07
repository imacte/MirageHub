# Check shipped PE files without Python or external inspection tools.
[CmdletBinding()]
param([Parameter(Mandatory)] [string[]]$Files)
$ErrorActionPreference = 'Stop'
foreach ($file in $Files) {
    $path = (Resolve-Path -LiteralPath $file).Path
    $bytes = [IO.File]::ReadAllBytes($path)
    if ($bytes.Length -lt 256 -or $bytes[0] -ne 0x4d -or $bytes[1] -ne 0x5a) { throw "Not a PE file: $path" }
    $pe = [BitConverter]::ToUInt32($bytes,0x3c)
    if ($pe -gt $bytes.Length - 24 -or [BitConverter]::ToUInt32($bytes,$pe) -ne 0x4550) { throw "Invalid PE header: $path" }
    $sections = [BitConverter]::ToUInt16($bytes,$pe+6)
    $optionalSize = [BitConverter]::ToUInt16($bytes,$pe+20)
    $optional = $pe+24
    $magic = [BitConverter]::ToUInt16($bytes,$optional)
    $directories = switch ($magic) { 0x20b { $optional+112 }; 0x10b { $optional+96 }; default { throw "Unknown PE format: $path" } }
    if ($optional+$optionalSize+40*$sections -gt $bytes.Length) { throw "Truncated PE sections: $path" }
    $sectionTable = $optional+$optionalSize
    $headerSize = [BitConverter]::ToUInt32($bytes,$optional+60)
    function FileOffset([uint32]$Rva) {
        if ($Rva -lt $headerSize -and $Rva -lt $bytes.Length) { return [int]$Rva }
        for ($i=0; $i -lt $sections; $i++) {
            $entry = $sectionTable+40*$i
            $virtualSize = [BitConverter]::ToUInt32($bytes,$entry+8)
            $address = [BitConverter]::ToUInt32($bytes,$entry+12)
            $rawSize = [BitConverter]::ToUInt32($bytes,$entry+16)
            $raw = [BitConverter]::ToUInt32($bytes,$entry+20)
            if ($Rva -ge $address -and [long]$Rva -lt [long]$address+[Math]::Max($rawSize,$virtualSize)) {
                $offset = [long]$raw+$Rva-$address
                if ($Rva-$address -ge $rawSize -or $offset -ge $bytes.Length) { throw "Unmapped PE address: $path" }
                return [int]$offset
            }
        }
        throw "Unmapped PE address: $path"
    }
    $imports = @()
    $importRva = [BitConverter]::ToUInt32($bytes,$directories+8)
    if ($importRva) {
        $descriptor = FileOffset $importRva
        while ($true) {
            if ($descriptor -gt $bytes.Length-20) { throw "Truncated import table: $path" }
            $nameRva = [BitConverter]::ToUInt32($bytes,$descriptor+12)
            if (-not $nameRva) { break }
            $nameOffset = FileOffset $nameRva
            $end = $nameOffset
            while ($end -lt $bytes.Length -and $bytes[$end] -ne 0 -and $end-$nameOffset -lt 512) { $end++ }
            if ($end -eq $bytes.Length -or $bytes[$end] -ne 0) { throw "Invalid DLL import name: $path" }
            $imports += [Text.Encoding]::ASCII.GetString($bytes,$nameOffset,$end-$nameOffset)
            $descriptor += 20
        }
    }
    $unwanted = @($imports | Where-Object { $_ -match '^(python|vcruntime|msvcp|msvcr[0-9])' })
    if ($unwanted.Count) { throw "External runtime dependency in ${path}: $($unwanted -join ', ')" }
    Write-Output "Native runtime check passed: $path"
}
