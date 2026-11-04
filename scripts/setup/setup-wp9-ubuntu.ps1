#requires -Version 7.2
<# Approved WP9 phase 2. Registers only the project Ubuntu distro for the current Windows user.
No automatic restart, distro deletion, password creation or solver install. #>
[CmdletBinding()]
param([switch]$Library)
$ErrorActionPreference = 'Stop'
$ubuntuLibraryRequested = $Library.IsPresent
. (Join-Path $PSScriptRoot 'setup-wp9-wsl.ps1') -Library
$statusPath = Join-Path $setupRoot 'wp9-ubuntu-status.json'
$distroName = 'Vernon-Ubuntu-24.04'
$distroPath = Join-Path $setupRoot 'ubuntu-24.04'
function Invoke-UbuntuSetupCommand([string]$Label, [string[]]$Arguments, [int]$Seconds) {
    $outPath = Join-Path $setupRoot ($Label + '.stdout.log')
    $errPath = Join-Path $setupRoot ($Label + '.stderr.log')
    $process = Start-Process -FilePath (Get-Command wsl.exe).Source -ArgumentList $Arguments -WindowStyle Hidden -RedirectStandardOutput $outPath -RedirectStandardError $errPath -PassThru
    $code = Wait-SetupProcess $process $Seconds $Label
    $state.lastExitCode = $code
    if ($code -ne 0) { throw "$Label failed with exit $code; inspect $outPath and $errPath. A restart may be needed; none will be initiated automatically." }
    return ([IO.File]::ReadAllText($outPath)).Replace([string][char]0, '')
}
function Get-ProjectUbuntuRegistration {
    $registryRoot = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Lxss'
    if (-not (Test-Path -LiteralPath $registryRoot)) { return $null }
    $registrations = @(Get-ChildItem -LiteralPath $registryRoot | ForEach-Object {
        $entry = Get-ItemProperty -LiteralPath $_.PSPath
        if ($entry.DistributionName -eq $distroName) { $entry }
    })
    if ($registrations.Count -gt 1) { throw 'Duplicate project distro registrations require inspection.' }
    if ($registrations.Count -eq 1) {
        $actual = [IO.Path]::GetFullPath($registrations[0].BasePath.Replace('\\?\','')).TrimEnd('\')
        if ($actual -ne [IO.Path]::GetFullPath($distroPath).TrimEnd('\')) { throw 'Existing project distro uses a different storage path; refusing to replace it.' }
        return $registrations[0]
    }
    return $null
}
if ($ubuntuLibraryRequested) { return }
$lock = $null
$preserveExisting = $false
$state = [ordered]@{
    schemaVersion=1;phase='preflight';message='';updatedUtc='';workerPid=$PID
    distroName=$distroName;distroPath=$distroPath;distribution='Ubuntu-24.04'
    nativePid=$null;nativeStartedUtc=$null;lastExitCode=$null
    automaticRestart=$false;ubuntuVerified=$false;openfoamInstalled=$false
}
try {
    $principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
    if ($principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw 'Run per-user distro setup from the normal, unelevated account.' }
    Assert-SetupDirectory
    $lock = [IO.File]::Open((Join-Path $setupRoot 'setup.lock'), [IO.FileMode]::OpenOrCreate, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
    $preserveExisting = $true
    $leases = @(Get-ChildItem -LiteralPath $setupRoot -Filter 'native-*.json' -File)
    if ($leases.Count -gt 128) { throw 'Inspect excess setup process leases before continuing.' }
    foreach ($lease in $leases) {
        if (Test-LiveSetupProcess (Get-Content -Raw -LiteralPath $lease.FullName | ConvertFrom-Json)) { throw 'Native setup is active; no duplicate allowed.' }
    }
    if (Test-Path -LiteralPath $statusPath) {
        $old = Get-Content -Raw -LiteralPath $statusPath | ConvertFrom-Json
        if (Test-LiveSetupProcess $old) { throw 'Prior Ubuntu setup is active; no duplicate allowed.' }
    }
    $preserveExisting = $false
    Save-SetupState 'checking-runtime' 'Verifying the runtime before project distro registration.'
    $version = Invoke-UbuntuSetupCommand 'runtime-version' @('--version') 30
    if ($version -notmatch 'WSL version:\s*(\d+)\.(\d+)\.(\d+)' -or [version]"$($Matches[1]).$($Matches[2]).$($Matches[3])" -lt [version]'3.0.3') { throw 'WSL 3.0.3 or newer is required; run setup-wp9-wsl.ps1 or update WSL first.' }
    if (-not (Get-ProjectUbuntuRegistration)) {
        Save-SetupState 'installing-ubuntu' 'Installing official Ubuntu24.04 with project name/location and --no-launch.'
        $null = Invoke-UbuntuSetupCommand 'ubuntu-install' @('--install','--distribution','Ubuntu-24.04','--name',$distroName,'--location',('"'+$distroPath+'"'),'--no-launch','--web-download') 1800
    }
    $registration = Get-ProjectUbuntuRegistration
    if (-not $registration -or $registration.Version -ne 2) { throw 'Project distro must be registered as WSL2 at the approved location.' }
    Save-SetupState 'probing-ubuntu' 'Checking Linux identity/version with an explicit root probe, without interactive account setup.'
    $probe = Invoke-UbuntuSetupCommand 'ubuntu-probe' @('--distribution',$distroName,'--user','root','--exec','/bin/sh','-c','"id -u; cat /etc/os-release; uname -r"') 90
    if ($probe -notmatch '(?m)^0\s*$' -or $probe -notmatch 'VERSION_ID="24\.04"') { throw 'Ubuntu root/version probe did not establish the intended distribution.' }
    $state.ubuntuVerified = $true
    Save-SetupState 'ubuntu-ready' 'Ubuntu24.04 WSL2 booted at the approved path. OpenFOAM setup is next; no solver installed yet.'
} catch {
    if ($lock -and -not $preserveExisting -and $state.phase -ne 'needs-attention') { Save-SetupState 'failed' $_.Exception.Message }
    Write-Error $_
    exit 1
} finally { if ($lock) { $lock.Dispose() } }
