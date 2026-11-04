#requires -Version 7.2
<#
Approved WP9 setup, phase 1 only: signed Microsoft WSL runtime and its
VirtualMachinePlatform dependency. Never restarts Windows, installs a distro,
or runs a CFD solver. Status survives this shell and quota interruptions.
#>
[CmdletBinding()]
param([switch]$Elevated, [switch]$Library)
$ErrorActionPreference = 'Stop'
# Tools live beside the checkout, never inside it: <parent of repo>\tools\wsl.
# Derived from this file's location, so the elevated relaunch resolves the same folder.
$setupRoot = Join-Path (Split-Path (Split-Path (Split-Path $PSScriptRoot -Parent) -Parent) -Parent) 'tools\wsl'
# Windows -> WSL path of a local drive path (C:\x -> /mnt/c/x).
function ConvertTo-SetupWslPath([string]$Path) {
    $full = [IO.Path]::GetFullPath($Path)
    if ($full -notmatch '^([A-Za-z]):\\(.+)$') { throw "Setup paths must be on a local drive letter: $full" }
    '/mnt/' + $Matches[1].ToLowerInvariant() + '/' + $Matches[2].Replace('\', '/')
}
$msiName = 'wsl.3.0.3.0.x64.msi'
$msiHash = 'ae977286974255bd26bf925f00f40f5692c6b2084155cbc0563c09aea091f211'
$msiSize = 367575040
$msiUrl = 'https://github.com/microsoft/WSL/releases/download/3.0.3/wsl.3.0.3.0.x64.msi'
$msiPath = Join-Path $setupRoot $msiName
$statusPath = Join-Path $setupRoot 'wp9-setup-status.json'
function Assert-SetupDirectory {
    $cursor = $setupRoot
    while ($cursor) {
        if ((Test-Path -LiteralPath $cursor) -and
            ((Get-Item -LiteralPath $cursor -Force).Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            throw "Setup directory cannot contain a reparse point: $cursor"
        }
        $cursor = Split-Path $cursor -Parent
    }
    [void][IO.Directory]::CreateDirectory($setupRoot)
}
function Save-SetupState([string]$Phase, [string]$Message) {
    $state.phase = $Phase
    $state.message = $Message
    $state.updatedUtc = [DateTime]::UtcNow.ToString('o')
    $tempPath = $statusPath + '.tmp'
    [IO.File]::WriteAllText($tempPath, ($state | ConvertTo-Json -Depth 7), [Text.UTF8Encoding]::new($false))
    $deadline = [DateTime]::UtcNow.AddSeconds(3)
    while ($true) {
        try { [IO.File]::Move($tempPath, $statusPath, $true); break }
        catch {
            $cause = $_.Exception
            while ($cause.InnerException) { $cause = $cause.InnerException }
            if (($cause.HResult -band 65535) -notin @(5,32,33) -or [DateTime]::UtcNow -ge $deadline) { throw }
            Start-Sleep -Milliseconds 25
        }
    }
}
function Test-LiveSetupProcess($Record) {
    if (-not $Record.nativePid -or -not $Record.nativeStartedUtc) { return $false }
    $lookupErrors = @()
    $live = Get-Process -Id $Record.nativePid -ErrorAction SilentlyContinue -ErrorVariable lookupErrors
    if ($lookupErrors | Where-Object { $_.CategoryInfo.Category -ne 'ObjectNotFound' }) { throw 'Cannot certify recorded setup identity; inspect from an administrator shell before retrying.' }
    # ConvertFrom-Json can turn ISO dates into DateTime on newer PowerShell.
    return $live -and ($live.StartTime.ToUniversalTime().Ticks -eq ([DateTime]$Record.nativeStartedUtc).ToUniversalTime().Ticks)
}
function Test-ApprovedMsi {
    if (-not (Test-Path -LiteralPath $msiPath -PathType Leaf)) { return $false }
    if ((Get-Item -LiteralPath $msiPath).Length -ne $msiSize) { throw 'WSL MSI length differs from official release metadata.' }
    if ((Get-FileHash -LiteralPath $msiPath -Algorithm SHA256).Hash.ToLowerInvariant() -ne $msiHash) { throw 'WSL MSI SHA256 mismatch.' }
    $signature = Get-AuthenticodeSignature -LiteralPath $msiPath
    if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch '(^|,\s*)O=Microsoft Corporation(,|$)') {
        throw 'WSL MSI must have a valid Microsoft Corporation Authenticode signature.'
    }
    $state.signer = $signature.SignerCertificate.Subject
    return $true
}
function Wait-SetupProcess($Process, [int]$Seconds, [string]$Name) {
    $state.nativePid = $Process.Id
    $state.nativeStartedUtc = $Process.StartTime.ToUniversalTime().ToString('o')
    # Unique process leases remain readable if a status reader blocks replacement.
    $leasePath = Join-Path $setupRoot ('native-' + [guid]::NewGuid().ToString('N') + '.json')
    try {
        [IO.File]::WriteAllText($leasePath, (@{nativePid=$state.nativePid;nativeStartedUtc=$state.nativeStartedUtc} | ConvertTo-Json), [Text.UTF8Encoding]::new($false))
    } catch {
        # Retain the exclusive worker lock if storage cannot record a live process.
        while (-not $Process.WaitForExit(1000)) { }
        throw
    }
    Save-SetupState $state.phase "Waiting for $Name; no automatic restart."
    if (-not $Process.WaitForExit($Seconds * 1000)) {
        # Do not kill Windows servicing or shared installer services.
        Save-SetupState 'needs-attention' "$Name exceeded its wait budget. Inspect the recorded native PID before any retry."
        throw "$Name still running; no duplicate setup may start."
    }
    $Process.Refresh()
    $code = $Process.ExitCode
    $state.nativePid = $null
    $state.nativeStartedUtc = $null
    return $code
}
if ($Library) { return }
Assert-SetupDirectory
$lock = $null
$preserveExisting = $false
$state = [ordered]@{
    schemaVersion = 1; phase = 'preflight'; message = ''; updatedUtc = ''; workerPid = $PID
    approved = 'Explicit human approval: WSL2, Ubuntu24.04 LTS, OpenFOAMv2606 setup/local checks'
    runtimeVersion = '3.0.3'; backend = 'WSL2'; source = $msiUrl; sha256 = $msiHash
    bytes = $msiSize; signer = ''; runtimeExitCode = $null; featureExitCode = $null
    nativePid = $null; nativeStartedUtc = $null; restartRequired = $false; automaticRestart = $false
    ubuntuInstalled = $false; openfoamInstalled = $false
}
try {
    $lockDeadline = [DateTime]::UtcNow.AddSeconds(8)
    while (-not $lock) {
        try { $lock = [IO.File]::Open((Join-Path $setupRoot 'setup.lock'), [IO.FileMode]::OpenOrCreate, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None) }
        catch {
            if (-not $Elevated -or [DateTime]::UtcNow -ge $lockDeadline) { throw }
            Start-Sleep -Milliseconds 50
        }
    }
    $leases = @(Get-ChildItem -LiteralPath $setupRoot -Filter 'native-*.json' -File)
    $preserveExisting = $true
    if ($leases.Count -gt 128) { throw 'Too many setup process leases; inspect before continuing.' }
    foreach ($lease in $leases) {
        if (Test-LiveSetupProcess (Get-Content -Raw -LiteralPath $lease.FullName | ConvertFrom-Json)) { throw 'A recorded native setup process is still running; no duplicate allowed.' }
    }
    if (Test-Path -LiteralPath $statusPath) {
        $preserveExisting = $true
        $old = Get-Content -Raw -LiteralPath $statusPath | ConvertFrom-Json
        if ($old.phase -in @('restart-required', 'runtime-ready')) {
            Write-Output "Existing setup phase: $($old.phase). Inspect status and post-restart runtime before continuing."
            exit 0
        }
        $ownHandoff = $Elevated -and $old.phase -eq 'elevation-handoff' -and $old.nativePid -eq $PID
        if (-not $ownHandoff -and (Test-LiveSetupProcess $old)) { throw 'Recorded native setup is still running; no duplicate allowed.' }
    }
    $preserveExisting = $false
    if (-not $Elevated) {
        Save-SetupState 'downloading' 'Downloading the pinned official Microsoft WSL MSI.'
        if (-not (Test-ApprovedMsi)) {
            $partial = $msiPath + '.partial'
            $download = Start-Process -FilePath (Get-Command curl.exe).Source -ArgumentList @('-fL', '--connect-timeout', '20', '--max-time', '1200', '--retry', '2', '--max-filesize', '400000000', '--output', ('"' + $partial + '"'), $msiUrl) -WindowStyle Hidden -RedirectStandardError (Join-Path $setupRoot 'download.log') -PassThru
            $downloadCode = Wait-SetupProcess $download 3700 'WSL download'
            if ($downloadCode -ne 0) { throw "WSL download failed: $downloadCode" }
            if ((Get-Item -LiteralPath $partial).Length -ne $msiSize) { throw 'Incomplete WSL download.' }
            if ((Get-FileHash -LiteralPath $partial -Algorithm SHA256).Hash.ToLowerInvariant() -ne $msiHash) { throw 'Downloaded WSL hash mismatch.' }
            Move-Item -LiteralPath $partial -Destination $msiPath
        }
        if (-not (Test-ApprovedMsi)) { throw 'Verified WSL MSI missing.' }
        Save-SetupState 'administrator-consent' 'Verified download; Windows administrator consent is required for the approved installation.'
        # Keep ownership through UAC, then publish the child's identity before
        # releasing it. The child waits briefly for this lock, preventing a gap.
        $shell = (Get-Process -Id $PID).Path
        $arguments = @('-NoProfile', '-NonInteractive', '-File', ('"' + $PSCommandPath + '"'), '-Elevated')
        $admin = Start-Process -FilePath $shell -ArgumentList $arguments -Verb RunAs -WindowStyle Hidden -PassThru
        $state.nativePid = $admin.Id
        $state.nativeStartedUtc = $admin.StartTime.ToUniversalTime().ToString('o')
        Save-SetupState 'elevation-handoff' 'Administrator phase launched; transferring exclusive setup ownership.'
        Write-Output "Elevated setup launched: PID $($admin.Id); inspect $statusPath"
        exit 0
    }
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    if (-not ([Security.Principal.WindowsPrincipal]::new($identity)).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw 'Elevated setup requires Windows administrator access.' }
    if (-not (Test-ApprovedMsi)) { throw 'Verified WSL MSI missing in elevated phase.' }
    Save-SetupState 'installing-runtime' 'Installing the signed WSL runtime with MSI restart suppression.'
    $install = Start-Process -FilePath "$env:WINDIR\System32\msiexec.exe" -ArgumentList @('/i', ('"' + $msiPath + '"'), '/qn', '/norestart', '/L*v', ('"' + (Join-Path $setupRoot 'msi.log') + '"')) -WindowStyle Hidden -PassThru
    $state.runtimeExitCode = Wait-SetupProcess $install 1200 'Microsoft WSL MSI'
    if ($state.runtimeExitCode -notin @(0, 3010)) { throw "WSL MSI failed: $($state.runtimeExitCode); inspect msi.log" }
    Save-SetupState 'enabling-virtualization' 'Enabling only the required VirtualMachinePlatform component, with /norestart.'
    $feature = Start-Process -FilePath "$env:WINDIR\System32\dism.exe" -ArgumentList @('/online', '/enable-feature', '/featurename:VirtualMachinePlatform', '/all', '/norestart', ('/LogPath:' + (Join-Path $setupRoot 'dism.log'))) -WindowStyle Hidden -PassThru
    $state.featureExitCode = Wait-SetupProcess $feature 1200 'VirtualMachinePlatform enablement'
    if ($state.featureExitCode -notin @(0, 3010)) { throw "Feature enablement failed: $($state.featureExitCode); inspect dism.log" }
    $state.restartRequired = ($state.runtimeExitCode -eq 3010 -or $state.featureExitCode -eq 3010)
    if ($state.restartRequired) { Save-SetupState 'restart-required' 'Runtime and virtualization preparation completed. Human-initiated Windows restart required before Ubuntu/CFD setup.' }
    else { Save-SetupState 'runtime-ready' 'Runtime/components installed without a reported restart requirement. Verify WSL status before Ubuntu setup.' }
} catch {
    if ($lock -and -not $preserveExisting) {
        if ($state.phase -ne 'needs-attention') { Save-SetupState 'failed' $_.Exception.Message }
    }
    Write-Error $_
    exit 1
} finally { if ($lock) { $lock.Dispose() } }
