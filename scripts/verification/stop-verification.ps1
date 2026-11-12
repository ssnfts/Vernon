#requires -Version 7.2
<#
Asks a running verification worker to stop. The worker polls for the request,
terminates the active step's process tree, records the run as 'cancelled'
(exit 130) and, for the WP9 Linux profiles, still runs its Linux cleanup.
A cancelled run is never completed, failed or resumable.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidatePattern('^[A-Za-z0-9][A-Za-z0-9_-]{0,95}$')][string]$RunId,
    [ValidateLength(0, 200)][string]$Reason = ''
)
$ErrorActionPreference = 'Stop'
$requestedRunId = $RunId
. (Join-Path $PSScriptRoot 'verification.ps1') -Library
$RunId = $requestedRunId
$directory = Get-VerificationPath $RunId
$statusPath = Join-Path $directory 'status.json'
if (-not (Test-Path -LiteralPath $statusPath)) { Write-Error "No such run: $RunId"; exit 2 }
$state = Read-VerificationJson $statusPath
if ($state.status -ne 'running' -or $state.finishedUtc) { Write-Error "Run $RunId is $($state.status); only a running worker can be stopped."; exit 3 }
if (-not (Test-VerificationProcess $state.workerPid $state.workerStartedUtc)) {
    Write-Error "The worker of $RunId is not alive. Check verification-status.ps1 and clean up any Linux processes as docs/CFD-RUNBOOK.md describes."
    exit 4
}
Request-VerificationCancel $directory $Reason
Write-Output "Stop requested for $RunId (worker PID $($state.workerPid)). Watch verification-status.ps1 -RunId $RunId for 'cancelled'."
exit 0
