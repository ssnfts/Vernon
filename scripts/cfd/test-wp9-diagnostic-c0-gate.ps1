#requires -Version 7.2
# Offline checks of the C0 gate on synthetic status and row objects.
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'wp9-diagnostic-c0-gate.ps1') -Library
$passed = 0
function Assert-True($Condition, [string]$Message) {
    if (-not $Condition) { throw "ASSERTION FAILED: $Message" }
    $script:passed++
}
function New-Good {
    $status = [pscustomobject]@{
        status = 'completed'; exitCode = 0; profile = 'wp9-diagnostic'; diagnosticVariant = 'c0'
        linuxCleanup = [pscustomobject]@{ success = $true }; sourceBefore = 'abc'; sourceAfter = 'abc'
        memorySummary = [pscustomobject]@{ maxSwapUsedKiB = 0; minMemAvailableKiB = 3940364; probeFailures = 1; programs = [pscustomobject]@{ simpleFoam = [pscustomobject]@{ firstSeenUtc = '2026-11-08T15:40:00.0000000Z' } } }
    }
    $row = [pscustomobject]@{
        variant = 'c0'; iterations = 1000; diagnostic_only = $true; acceptance_evidence = $false
        reproduction_of_stage2 = [pscustomobject]@{ compared_iterations = 1000; missing_iterations = 0; max_rel_diff_initial_residuals = 0.0; first_iteration_over_1e_6 = $null; max_rel_diff_bounding_min = 0.0; bounding_lines_compared = 2000 }
        phases = @([pscustomobject]@{ seconds_per_iteration = 17.4 })
        stage2_seconds_per_iteration_same_window = 16.9
        counters = [pscustomobject]@{ k = [pscustomobject]@{ rows = 1000 }; omega = [pscustomobject]@{ rows = 1000 } }
    }
    return @($status, $row)
}
$good = New-Good
$copyFailure = @('2026-11-08T15:30:40.5149948Z')
Assert-True ((Get-Wp9C0GateFailures $good[0] $good[1] $copyFailure).Count -eq 0) 'a reproduced, instrumented, normally progressing C0 passes (one probe timed out during the copy)'
Assert-True ((Get-Wp9C0GateFailures $good[0] $good[1] @([datetime]::Parse('2026-11-08T15:30:40Z').ToUniversalTime())).Count -eq 0) 'probe times may arrive as DateTime objects'
Assert-True ((Get-Wp9C0GateFailures $good[0] $good[1] @('2026-11-08T16:00:00Z')).Count -ge 1) 'a probe failure while the solver runs is refused'
Assert-True ((Get-Wp9C0GateFailures $good[0] $good[1] @()).Count -ge 1) 'probe failures must match the summary count'
$cases = @(
    @{ name = 'failed run'; edit = { param($s, $r) $s.status = 'failed'; $s.exitCode = 4 } },
    @{ name = 'd1 run'; edit = { param($s, $r) $s.diagnosticVariant = 'd1' } },
    @{ name = 'cleanup unconfirmed'; edit = { param($s, $r) $s.linuxCleanup.success = $false } },
    @{ name = 'source changed'; edit = { param($s, $r) $s.sourceAfter = 'xyz' } },
    @{ name = 'short row'; edit = { param($s, $r) $r.iterations = 500 } },
    @{ name = 'acceptance claimed'; edit = { param($s, $r) $r.acceptance_evidence = $true } },
    @{ name = 'no reproduction'; edit = { param($s, $r) $r.reproduction_of_stage2 = $null } },
    @{ name = 'residuals differ'; edit = { param($s, $r) $r.reproduction_of_stage2.max_rel_diff_initial_residuals = 2e-6 } },
    @{ name = 'residuals infinite'; edit = { param($s, $r) $r.reproduction_of_stage2.max_rel_diff_initial_residuals = [double]::PositiveInfinity } },
    @{ name = 'missing iterations'; edit = { param($s, $r) $r.reproduction_of_stage2.missing_iterations = 3 } },
    @{ name = 'bounding differs'; edit = { param($s, $r) $r.reproduction_of_stage2.max_rel_diff_bounding_min = 1e-3 } },
    @{ name = 'no bounding compared'; edit = { param($s, $r) $r.reproduction_of_stage2.bounding_lines_compared = 0 } },
    @{ name = 'slow'; edit = { param($s, $r) $r.phases[0].seconds_per_iteration = 18.7 } },
    @{ name = 'swap'; edit = { param($s, $r) $s.memorySummary.maxSwapUsedKiB = 10 } },
    @{ name = 'low memory'; edit = { param($s, $r) $s.memorySummary.minMemAvailableKiB = 500000 } },
    @{ name = 'sampler failures miscounted'; edit = { param($s, $r) $s.memorySummary.probeFailures = 2 } },
    @{ name = 'solver never sampled'; edit = { param($s, $r) $s.memorySummary.programs = [pscustomobject]@{} } },
    @{ name = 'counter rows'; edit = { param($s, $r) $r.counters.omega.rows = 999 } }
)
foreach ($case in $cases) {
    $pair = New-Good
    & $case.edit $pair[0] $pair[1]
    Assert-True ((Get-Wp9C0GateFailures $pair[0] $pair[1] $copyFailure).Count -ge 1) "the gate refuses: $($case.name)"
}
Write-Output "WP9_C0_GATE_TESTS passed=$passed failed=0"
