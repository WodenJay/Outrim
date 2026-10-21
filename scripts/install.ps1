# Install the core only; never register an Agent or install a JS runtime.
[CmdletBinding()]
param(
    [string]$Version = 'latest',
    [string]$FromFile,
    [switch]$NoPath
)
$ErrorActionPreference = 'Stop'

if ($Version -cnotmatch '^[A-Za-z0-9._-]+$') { throw "Invalid version: $Version" }

if (-not [Runtime.InteropServices.RuntimeInformation]::IsOSPlatform([Runtime.InteropServices.OSPlatform]::Windows) -or
    [Runtime.InteropServices.RuntimeInformation]::OSArchitecture -ne [Runtime.InteropServices.Architecture]::X64) {
    throw 'Windows x64 required'
}
if ([string]::IsNullOrWhiteSpace($env:USERPROFILE)) { throw 'USERPROFILE is not set' }
$BinDir = Join-Path $env:USERPROFILE '.outrim/bin'
$InstalledBinary = Join-Path $BinDir 'outrim.exe'
$Asset = 'outrim-x86_64-pc-windows-msvc.exe'
$null = New-Item -ItemType Directory -Force -Path $BinDir
$Staging = Join-Path $BinDir ('.install.' + [guid]::NewGuid().ToString('N'))
$null = New-Item -ItemType Directory -Path $Staging
try {
    $Binary = Join-Path $Staging 'outrim.exe'
    if ($FromFile) {
        Copy-Item -LiteralPath $FromFile -Destination $Binary
    } else {
        [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
        $Base = 'https://github.com/WodenJay/Outrim/releases'
        if ($Version -eq 'latest') { $Base += '/latest/download' }
        else { $Base += "/download/$Version" }
        Invoke-WebRequest -UseBasicParsing -Uri "$Base/$Asset" -OutFile $Binary
        $Checksums = Join-Path $Staging 'SHA256SUMS'
        Invoke-WebRequest -UseBasicParsing -Uri "$Base/SHA256SUMS" -OutFile $Checksums
        $Expected = @(Get-Content -LiteralPath $Checksums | ForEach-Object {
            if ($_ -cmatch ('^([a-fA-F0-9]{64})\s+' + [regex]::Escape($Asset) + '$')) { $Matches[1] }
        })
        if ($Expected.Count -ne 1 -or (Get-FileHash -LiteralPath $Binary -Algorithm SHA256).Hash -ne $Expected[0]) {
            throw "Checksum mismatch: $Asset"
        }
    }
    if ((Get-Item -LiteralPath $Binary).Length -eq 0) { throw 'Empty binary' }
    Move-Item -LiteralPath $Binary -Destination $InstalledBinary -Force
} finally {
    Remove-Item -LiteralPath $Staging -Recurse -Force
}

$NeedsPath = -not (($env:Path -split ';') | Where-Object { $_.TrimEnd('\', '/') -ieq $BinDir.TrimEnd('\', '/') })
if (-not $NoPath) {
    $UserPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $Entries = @($UserPath -split ';' | Where-Object { $_ })
    if (-not ($Entries | Where-Object { $_.TrimEnd('\', '/') -ieq $BinDir.TrimEnd('\', '/') })) {
        [Environment]::SetEnvironmentVariable('Path', (($Entries + $BinDir) -join ';'), 'User')
    }
    # Preserve the process PATH (including machine entries).
    if (-not (($env:Path -split ';') | Where-Object { $_.TrimEnd('\', '/') -ieq $BinDir.TrimEnd('\', '/') })) {
        $env:Path = "$BinDir;$env:Path"
    }
}
Write-Output "Installed $InstalledBinary"
if ($NeedsPath -and -not $NoPath) { Write-Output 'Open a new terminal.' }
