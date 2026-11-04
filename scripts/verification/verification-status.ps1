#requires -Version 7.2
[CmdletBinding()]
param(
    [ValidatePattern('^$|^[A-Za-z0-9][A-Za-z0-9_-]{0,95}$')][string]$RunId = '',
    [switch]$Json
)
$ErrorActionPreference = 'Stop'
$requestedRunId = $RunId
. (Join-Path $PSScriptRoot 'verification.ps1') -Library
$RunId = $requestedRunId
if (-not $RunId) {
    $latest = Get-Content -Raw -LiteralPath (Join-Path $script:VerificationRoot 'latest.json') | ConvertFrom-Json
    $RunId = $latest.runId
}
$path = Join-Path (Get-VerificationPath $RunId) 'status.json'
$state = Get-Content -Raw -LiteralPath $path | ConvertFrom-Json
$alive = Test-VerificationProcess $state.workerPid $state.workerStartedUtc
$childAlive = Test-VerificationProcess $state.childPid $state.childStartedUtc
$lockState = Get-VerificationLockState
$state | Add-Member -NotePropertyName recordedStatus -NotePropertyValue $state.status
$state | Add-Member -NotePropertyName workerAlive -NotePropertyValue $alive
$state | Add-Member -NotePropertyName childAlive -NotePropertyValue $childAlive
$state | Add-Member -NotePropertyName lockState -NotePropertyValue $lockState
if ($state.status -in @('starting', 'running') -and -not $alive) {
    $latest = Get-Content -Raw -LiteralPath (Join-Path $script:VerificationRoot 'latest.json') | ConvertFrom-Json
    if ($lockState -eq 'held' -and $latest.runId -eq $RunId) {
        $state.status = 'running-visibility-unknown'
        $state.workerAlive = $null
        $state.message = 'The exclusive worker lock is held; this shell cannot see its PID. Check from a normal host shell. Do not launch competing work.'
    } elseif ($lockState -eq 'unknown') {
        $state.status = 'inconclusive'
        $state.workerAlive = $null
        $state.message = 'This shell cannot establish process or lock state. Check from a normal host shell.'
    } else {
        $state.status = 'interrupted'
        $state.message = if ($childAlive) { 'Worker died; its child is still alive. Inspect it before resuming.' } else { 'Worker died without a completion record. Successful checkpoints can be resumed with matching source/profile.' }
    }
}
if ($state.status -eq 'completed') {
    $current = Get-VerificationFingerprint
    if ($state.exitCode -ne 0 -or -not $state.finishedUtc -or $state.sourceBefore -ne $state.sourceAfter) {
        $state.status = 'invalid-completion'
    } elseif ($current -ne $state.sourceAfter) {
        $state.status = 'stale'
        $state.message = 'The run completed for an older source fingerprint. Current source is not verified by this run.'
    }
}
if ($Json) { $state | ConvertTo-Json -Depth 12 }
else {
    Write-Output "$($state.status): $($state.runId) [$($state.profile)]"
    Write-Output "Worker alive: $($state.workerAlive); child alive: $childAlive; lock: $lockState; exit: $($state.exitCode)"
    Write-Output "Started: $($state.startedUtc); finished: $($state.finishedUtc)"
    foreach ($step in $state.steps) { Write-Output "  $($step.name): $($step.status); exit $($step.exitCode); reused $($step.reused)" }
    if ($state.message) { Write-Output $state.message }
    Write-Output "Logs and status: $($state.runDirectory)"
}
