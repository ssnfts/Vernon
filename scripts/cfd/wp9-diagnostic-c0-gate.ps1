#requires -Version 7.2
<#
Decides whether the C0 turbulence diagnostic may be followed by D1. C0 is
expected to reproduce Stage 2's clipping and poor residuals; "passing" here
means the instrumentation and the run are trustworthy, not that C0 is healthy:
  - the runner completed it (exit 0, positive Linux cleanup, unchanged source);
  - the diagnostic row is C0 at 1000 iterations with checked instrumentation;
  - it reproduces the original Stage 2 run iteration by iteration (initial
    residuals and bounding minima within 1e-6 relative over 1..1000);
  - it progressed normally (seconds per iteration within 10% of Stage 2 over
    the same iterations) with bounded resources (no swap, >= 1 GiB available).
Usage: wp9-diagnostic-c0-gate.ps1 -RunId <c0 run id>   (exit 0 = pass)
#>
[CmdletBinding()]
param(
    [ValidatePattern('^$|^[A-Za-z0-9][A-Za-z0-9_-]{0,95}$')][string]$RunId = '',
    [switch]$Library
)
$ErrorActionPreference = 'Stop'

function Get-Wp9C0GateFailures($Status, $Row) {
    $failures = [Collections.Generic.List[string]]::new()
    $number = { param($v) $v -is [ValueType] -and $v -isnot [bool] -and [double]::IsFinite([double]$v) }
    if ($Status.status -cne 'completed' -or $Status.exitCode -ne 0) { $failures.Add("run status $($Status.status), exit $($Status.exitCode)") }
    if ($Status.profile -cne 'wp9-diagnostic' -or $Status.diagnosticVariant -cne 'c0') { $failures.Add('not a c0 diagnostic run') }
    if (-not $Status.linuxCleanup -or $Status.linuxCleanup.success -ne $true) { $failures.Add('Linux cleanup not confirmed') }
    if (-not $Status.sourceBefore -or $Status.sourceBefore -cne $Status.sourceAfter) { $failures.Add('source changed during the run') }
    if ($Row.variant -cne 'c0' -or $Row.iterations -ne 1000 -or $Row.diagnostic_only -ne $true -or $Row.acceptance_evidence -ne $false) { $failures.Add('row is not a 1000-iteration c0 diagnostic') }
    $r = $Row.reproduction_of_stage2
    if (-not $r) { $failures.Add('no reproduction record') }
    else {
        if ($r.compared_iterations -ne 1000 -or $r.missing_iterations -ne 0) { $failures.Add("reproduction compared $($r.compared_iterations) iterations, missing $($r.missing_iterations)") }
        if (-not (& $number $r.max_rel_diff_initial_residuals) -or $r.max_rel_diff_initial_residuals -gt 1e-6) { $failures.Add("initial residuals differ from Stage 2 by up to $($r.max_rel_diff_initial_residuals) (first over 1e-6 at $($r.first_iteration_over_1e_6))") }
        if (-not (& $number $r.max_rel_diff_bounding_min) -or $r.max_rel_diff_bounding_min -gt 1e-6 -or $r.bounding_lines_compared -lt 1) { $failures.Add("bounding minima differ from Stage 2 by up to $($r.max_rel_diff_bounding_min) over $($r.bounding_lines_compared) lines") }
    }
    $spi = @($Row.phases)[0].seconds_per_iteration
    $reference = $Row.stage2_seconds_per_iteration_same_window
    if (-not (& $number $spi) -or -not (& $number $reference) -or $spi -gt 1.10 * $reference) { $failures.Add("seconds per iteration $spi against Stage 2 $reference (limit +10%)") }
    $memory = $Status.memorySummary
    if (-not $memory -or $memory.maxSwapUsedKiB -ne 0 -or $memory.minMemAvailableKiB -lt 1048576 -or $memory.probeFailures -ne 0) { $failures.Add('memory not bounded: swap used, under 1 GiB available, or sampler failures') }
    foreach ($field in @('k', 'omega')) {
        if (-not $Row.counters.$field -or $Row.counters.$field.rows -ne 1000) { $failures.Add("counter for $field does not have 1000 rows") }
    }
    return , $failures.ToArray()
}

if ($Library) { return }
$repo = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
$run = Join-Path $repo "scratch/continuation/runs/$RunId"
$status = Get-Content -Raw -LiteralPath (Join-Path $run 'status.json') | ConvertFrom-Json
$row = Get-Content -Raw -LiteralPath (Join-Path $run 'diagnostic.json') | ConvertFrom-Json
$failures = Get-Wp9C0GateFailures $status $row
if ($failures.Count -eq 0) { Write-Output "WP9_C0_GATE pass run=$RunId"; exit 0 }
Write-Output "WP9_C0_GATE fail run=$RunId"
foreach ($f in $failures) { Write-Output "  - $f" }
exit 1
