#requires -Version 7.2
# Offline checks of the WP9 memory sampler's parser and peak bookkeeping. No WSL call.
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'wp9-memory-sampler.ps1') -Library
$passed = 0
function Assert-True($Condition, [string]$Message) {
    if (-not $Condition) { throw "ASSERTION FAILED: $Message" }
    $script:passed++
}
$probe = @"
meminfo MemTotal 26624000
meminfo MemAvailable 20000000
meminfo SwapTotal 7340032
meminfo SwapFree 7000000
proc 101 mpirun 9000 9500
proc 102 simpleFoam 1100000 1200000
proc 103 simpleFoam 1000000 1300000
proc 200 bash 4000 4000
rootfs 900000000 1000000000
"@
$sample = ConvertFrom-Wp9MemoryProbe $probe
Assert-True ($sample.memAvailableKiB -eq 20000000 -and $sample.memTotalKiB -eq 26624000) 'meminfo totals are read'
Assert-True ($sample.swapUsedKiB -eq 340032) 'swap use is total minus free'
Assert-True (@($sample.processes).Count -eq 3) 'only the watched programs are kept'
Assert-True ((@($sample.processes | Where-Object name -eq 'simpleFoam')).Count -eq 2) 'every solver rank is kept'
$summary = New-Wp9MemorySummary
Update-Wp9MemorySummary $summary $sample '2026-11-04T10:00:00Z' 1000 2000
$second = ConvertFrom-Wp9MemoryProbe (($probe -replace 'MemAvailable 20000000', 'MemAvailable 18000000') -replace '102 simpleFoam 1100000 1200000', '102 simpleFoam 900000 1250000')
Update-Wp9MemorySummary $summary $second '2026-11-04T10:01:00Z' 5000 1500
Assert-True ($summary.samples -eq 2) 'samples are counted'
Assert-True ($summary.minMemAvailableKiB -eq 18000000) 'lowest available memory is kept'
Assert-True ($summary.programs.simpleFoam.peakRankHwmKiB -eq 1300000) 'peak per-rank high-water mark is the largest seen'
Assert-True ($summary.programs.simpleFoam.peakSumRssKiB -eq 2100000) 'peak total resident memory is the largest same-sample sum'
Assert-True ($summary.programs.simpleFoam.peakSumHwmKiB -eq 2550000) 'sum of rank high-water marks is tracked per sample'
Assert-True ($summary.programs.simpleFoam.maxProcesses -eq 2) 'rank count is recorded'
Assert-True ($summary.programs.simpleFoam.firstSeenUtc -eq '2026-11-04T10:00:00Z' -and $summary.programs.simpleFoam.lastSeenUtc -eq '2026-11-04T10:01:00Z') 'each program has a seen-from/to window'
Assert-True ($summary.maxRunDirectoryBytes -eq 5000 -and $summary.minHostFreeKiB -eq 1500) 'disk and host memory extremes are kept'
foreach ($bad in @("meminfo MemAvailable lots", "proc x simpleFoam 1 2", "proc 1 simpleFoam -5 2", "meminfo MemAvailable 1`nmeminfo MemAvailable 2", "something else entirely")) {
    $rejected = $false
    try { [void](ConvertFrom-Wp9MemoryProbe ("meminfo MemTotal 1`n" + $bad)) } catch { $rejected = $true }
    Assert-True $rejected "malformed probe output is refused: $bad"
}
$rejected = $false
try { [void](ConvertFrom-Wp9MemoryProbe "proc 1 simpleFoam 1 2") } catch { $rejected = $true }
Assert-True $rejected 'a probe without MemAvailable is refused'
Write-Output "WP9_MEMORY_SAMPLER_TESTS passed=$passed failed=0"
