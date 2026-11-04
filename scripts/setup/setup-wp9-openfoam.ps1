#requires -Version 7.2
[CmdletBinding()]
param([ValidateSet('inspect','install')][string]$Mode = 'inspect', [switch]$Library)
$ErrorActionPreference = 'Stop'
$openfoamLibraryRequested = $Library.IsPresent
. (Join-Path $PSScriptRoot 'setup-wp9-ubuntu.ps1') -Library
$statusPath = Join-Path $setupRoot 'wp9-openfoam-status.json'
if ($openfoamLibraryRequested) { return }
$lock = $null
$preserveExisting = $false
$state = [ordered]@{
    schemaVersion=1;phase='preflight';message='';updatedUtc='';workerPid=$PID
    mode=$Mode;nativePid=$null;nativeStartedUtc=$null;lastExitCode=$null
    distroName=$distroName;automaticRestart=$false;openfoamVerified=$false
}
try {
    $principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
    if ($principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw 'Use the normal unelevated Windows account.' }
    Assert-SetupDirectory
    $lock = [IO.File]::Open((Join-Path $setupRoot 'setup.lock'), [IO.FileMode]::OpenOrCreate, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
    $preserveExisting = $true
    $leases = @(Get-ChildItem -LiteralPath $setupRoot -Filter 'native-*.json' -File)
    if ($leases.Count -gt 128) { throw 'Inspect excess setup process leases before continuing.' }
    foreach ($lease in $leases) {
        if (Test-LiveSetupProcess (Get-Content -Raw -LiteralPath $lease.FullName | ConvertFrom-Json)) { throw 'Native setup is active; no duplicate allowed.' }
    }
    if (Test-Path -LiteralPath $statusPath) {
        if (Test-LiveSetupProcess (Get-Content -Raw -LiteralPath $statusPath | ConvertFrom-Json)) { throw 'Prior setup is active; no duplicate allowed.' }
    }
    if (-not (Get-ProjectUbuntuRegistration)) { throw 'Verified project distro is required.' }
    $preserveExisting = $false
    Save-SetupState 'checking-openfoam-packages' "Running approved signed-repository phase: $Mode."
    $scriptLinuxPath = ConvertTo-SetupWslPath (Join-Path $PSScriptRoot 'setup-wp9-openfoam.sh')
    $evidenceLinuxPath = ConvertTo-SetupWslPath (Join-Path $setupRoot 'openfoam')
    $output = Invoke-UbuntuSetupCommand ('openfoam-' + $Mode) @('--distribution',$distroName,'--user','root','--exec','/bin/bash',$scriptLinuxPath,$Mode,$evidenceLinuxPath) 1800
    $marker = if ($Mode -eq 'inspect') { 'WP9_APT_METADATA_READY candidate=' } else { 'WP9_OPENFOAM_PACKAGE_INSTALLED version=' }
    if ($output -notmatch ('(?m)^' + [regex]::Escape($marker))) { throw 'Package phase exited without positive completion evidence.' }
    Save-SetupState ($Mode + '-complete') 'Package phase completed; solver capability and real CFD validation remain separate gates.'
} catch {
    if ($lock -and -not $preserveExisting -and $state.phase -ne 'needs-attention') { Save-SetupState 'failed' $_.Exception.Message }
    Write-Error $_
    exit 1
} finally { if ($lock) { $lock.Dispose() } }
