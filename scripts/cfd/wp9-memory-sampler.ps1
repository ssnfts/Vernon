#requires -Version 7.2
<#
Read-only memory and disk sampler for WP9 solve runs. Once a minute it reads,
inside the approved distribution, /proc/meminfo and the resident memory (VmRSS)
and high-water mark (VmHWM) of the OpenFOAM programs, plus the Windows host's
free memory and the run directory's size. It starts, stops and signals nothing.
Samples go to <run>/memory-samples.jsonl and peaks to <run>/memory-summary.json.
The verification worker starts it for the solve profiles and stops it by
writing <run>/memory-sampler.stop; it also exits when its parent worker dies.
#>
[CmdletBinding()]
param(
    [string]$RunDirectory = '',
    [ValidateSet('Vernon-Ubuntu-24.04')][string]$Distribution = 'Vernon-Ubuntu-24.04',
    [ValidateSet('vernon')][string]$User = 'vernon',
    [int]$ParentProcessId = 0,
    [ValidateRange(5, 3600)][int]$IntervalSeconds = 60,
    [switch]$Once,
    [switch]$Library
)
$ErrorActionPreference = 'Stop'

# Program names as /proc/<pid>/status reports them (truncated to 15 characters).
$script:Wp9WatchedPrograms = @('simpleFoam', 'potentialFoam', 'decomposePar', 'reconstructPar', 'snappyHexMesh', 'checkMesh', 'blockMesh', 'surfaceFeatureE', 'topoSet', 'mpirun', 'prterun', 'prted', 'orted')

# Fixed probe: no argument or path is interpolated into it.
$script:Wp9MemoryProbe = 'while read -r k v u; do case $k in MemTotal:|MemAvailable:|SwapTotal:|SwapFree:) printf ''meminfo %s %s\n'' "${k%:}" "$v";; esac; done < /proc/meminfo; ' +
    'for d in /proc/[0-9]*; do n=; r=0; h=0; while read -r k v u; do case $k in Name:) n=$v;; VmRSS:) r=$v;; VmHWM:) h=$v;; esac; done < "$d/status" 2>/dev/null; ' +
    'case $n in simpleFoam|potentialFoam|decomposePar|reconstructPar|snappyHexMesh|checkMesh|blockMesh|surfaceFeatureE|topoSet|mpirun|prterun|prted|orted) printf ''proc %s %s %s %s\n'' "${d#/proc/}" "$n" "$r" "$h";; esac; done; ' +
    'df -k --output=avail,size / 2>/dev/null | { read -r _; read -r a s; printf ''rootfs %s %s\n'' "$a" "$s"; }'

function ConvertFrom-Wp9MemoryProbe([string]$Text) {
    $meminfo = @{}
    $processes = [Collections.Generic.List[object]]::new()
    $rootfs = $null
    foreach ($line in ($Text -split "`r?`n")) {
        if ($line -eq '') { continue }
        if ($line -cmatch '\Ameminfo (MemTotal|MemAvailable|SwapTotal|SwapFree) ([0-9]{1,15})\z') {
            if ($meminfo.ContainsKey($Matches[1])) { throw "Duplicate $($Matches[1]) record." }
            $meminfo[$Matches[1]] = [int64]$Matches[2]
        } elseif ($line -cmatch '\Aproc ([1-9][0-9]{0,9}) ([A-Za-z0-9_.-]{1,15}) ([0-9]{1,15}) ([0-9]{1,15})\z') {
            if ($Matches[2] -cin $script:Wp9WatchedPrograms) {
                $processes.Add([ordered]@{ pid = [int]$Matches[1]; name = $Matches[2]; rssKiB = [int64]$Matches[3]; hwmKiB = [int64]$Matches[4] })
            }
        } elseif ($line -cmatch '\Arootfs ([0-9]{1,15}) ([0-9]{1,15})\z') {
            $rootfs = [ordered]@{ availKiB = [int64]$Matches[1]; sizeKiB = [int64]$Matches[2] }
        } else { throw "Unexpected probe line: $line" }
    }
    if (-not $meminfo.ContainsKey('MemAvailable') -or -not $meminfo.ContainsKey('MemTotal')) { throw 'The probe returned no MemTotal/MemAvailable.' }
    $swapUsed = if ($meminfo.ContainsKey('SwapTotal') -and $meminfo.ContainsKey('SwapFree')) { $meminfo.SwapTotal - $meminfo.SwapFree } else { $null }
    [ordered]@{
        memTotalKiB = $meminfo.MemTotal; memAvailableKiB = $meminfo.MemAvailable; swapUsedKiB = $swapUsed
        processes = @($processes); rootfs = $rootfs
    }
}

function New-Wp9MemorySummary {
    [ordered]@{
        samples = 0; probeFailures = 0; firstUtc = $null; lastUtc = $null
        minMemAvailableKiB = $null; maxSwapUsedKiB = $null; minHostFreeKiB = $null; maxRunDirectoryBytes = $null
        programs = [ordered]@{}
    }
}

function Update-Wp9MemorySummary($Summary, $Sample, [string]$Utc, $RunDirectoryBytes, $HostFreeKiB) {
    $Summary.samples++
    if (-not $Summary.firstUtc) { $Summary.firstUtc = $Utc }
    $Summary.lastUtc = $Utc
    $min = { param($old, $new) if ($null -eq $new) { $old } elseif ($null -eq $old -or $new -lt $old) { $new } else { $old } }
    $max = { param($old, $new) if ($null -eq $new) { $old } elseif ($null -eq $old -or $new -gt $old) { $new } else { $old } }
    $Summary.minMemAvailableKiB = & $min $Summary.minMemAvailableKiB $Sample.memAvailableKiB
    $Summary.maxSwapUsedKiB = & $max $Summary.maxSwapUsedKiB $Sample.swapUsedKiB
    $Summary.minHostFreeKiB = & $min $Summary.minHostFreeKiB $HostFreeKiB
    $Summary.maxRunDirectoryBytes = & $max $Summary.maxRunDirectoryBytes $RunDirectoryBytes
    foreach ($group in ($Sample.processes | Group-Object -Property { $_.name })) {
        if (-not $Summary.programs.Contains($group.Name)) {
            $Summary.programs[$group.Name] = [ordered]@{ firstSeenUtc = $Utc; lastSeenUtc = $Utc; maxProcesses = 0; peakRankHwmKiB = 0; peakSumRssKiB = 0; peakSumHwmKiB = 0 }
        }
        $entry = $Summary.programs[$group.Name]
        $entry.lastSeenUtc = $Utc
        $members = @($group.Group)
        $entry.maxProcesses = [Math]::Max($entry.maxProcesses, $members.Count)
        $entry.peakRankHwmKiB = [Math]::Max($entry.peakRankHwmKiB, ($members | Measure-Object -Property { $_.hwmKiB } -Maximum).Maximum)
        $entry.peakSumRssKiB = [Math]::Max($entry.peakSumRssKiB, ($members | Measure-Object -Property { $_.rssKiB } -Sum).Sum)
        $entry.peakSumHwmKiB = [Math]::Max($entry.peakSumHwmKiB, ($members | Measure-Object -Property { $_.hwmKiB } -Sum).Sum)
    }
}

function Invoke-Wp9MemoryProbe([string]$Distribution, [string]$User) {
    $info = [Diagnostics.ProcessStartInfo]::new('wsl.exe')
    $info.UseShellExecute = $false; $info.CreateNoWindow = $true
    $info.RedirectStandardOutput = $true; $info.RedirectStandardError = $true
    foreach ($argument in @('-d', $Distribution, '-u', $User, '--exec', '/bin/bash', '-c', $script:Wp9MemoryProbe)) { $info.ArgumentList.Add($argument) }
    $process = [Diagnostics.Process]::Start($info)
    $out = $process.StandardOutput.ReadToEndAsync()
    $err = $process.StandardError.ReadToEndAsync()
    if (-not $process.WaitForExit(30000)) { $process.Kill($true); throw 'The memory probe did not answer within 30 seconds.' }
    if ($process.ExitCode -ne 0) { throw "The memory probe exited $($process.ExitCode): $($err.Result)" }
    return $out.Result
}

function Get-Wp9DirectoryBytes([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) { return 0 }
    $total = [int64]0
    foreach ($file in [IO.Directory]::EnumerateFiles($Path, '*', [IO.EnumerationOptions]@{ RecurseSubdirectories = $true; AttributesToSkip = [IO.FileAttributes]::ReparsePoint; IgnoreInaccessible = $true })) {
        try { $total += ([IO.FileInfo]::new($file)).Length } catch { }
    }
    return $total
}

if ($Library) { return }
if (-not $RunDirectory -or -not (Test-Path -LiteralPath $RunDirectory -PathType Container)) { throw 'RunDirectory must be an existing run folder.' }
$samplesPath = Join-Path $RunDirectory 'memory-samples.jsonl'
$summaryPath = Join-Path $RunDirectory 'memory-summary.json'
$stopPath = Join-Path $RunDirectory 'memory-sampler.stop'
$parentStarted = if ($ParentProcessId -gt 0) { (Get-Process -Id $ParentProcessId).StartTime } else { $null }
$summary = New-Wp9MemorySummary
$summary['intervalSeconds'] = $IntervalSeconds
$summary['watchedPrograms'] = $script:Wp9WatchedPrograms
while ($true) {
    $utc = [DateTime]::UtcNow.ToString('o')
    $hostFree = [int64](Get-CimInstance Win32_OperatingSystem).FreePhysicalMemory
    $vmmem = (Get-Process -Name 'vmmem', 'vmmemWSL' -ErrorAction SilentlyContinue | Measure-Object -Property WorkingSet64 -Sum).Sum
    $bytes = Get-Wp9DirectoryBytes (Join-Path $RunDirectory 'temp')
    $record = [ordered]@{ utc = $utc; hostFreeKiB = $hostFree; vmmemWorkingSetBytes = $vmmem; runDirectoryBytes = $bytes }
    try {
        $sample = ConvertFrom-Wp9MemoryProbe (Invoke-Wp9MemoryProbe $Distribution $User)
        $record['linux'] = $sample
        Update-Wp9MemorySummary $summary $sample $utc $bytes $hostFree
    } catch {
        $summary.probeFailures++
        $record['probeError'] = $_.Exception.Message
    }
    Add-Content -LiteralPath $samplesPath -Value ($record | ConvertTo-Json -Depth 6 -Compress) -Encoding utf8NoBOM
    $summary | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath ($summaryPath + '.tmp') -Encoding utf8NoBOM
    Move-Item -LiteralPath ($summaryPath + '.tmp') -Destination $summaryPath -Force
    if ($Once) { break }
    $wake = [DateTime]::UtcNow.AddSeconds($IntervalSeconds)
    $stop = $false
    while ([DateTime]::UtcNow -lt $wake) {
        if (Test-Path -LiteralPath $stopPath) { $stop = $true; break }
        if ($ParentProcessId -gt 0) {
            $parent = Get-Process -Id $ParentProcessId -ErrorAction SilentlyContinue
            if (-not $parent -or $parent.StartTime -ne $parentStarted) { $stop = $true; break }
        }
        Start-Sleep -Seconds 1
    }
    if ($stop) { break }
}
exit 0
