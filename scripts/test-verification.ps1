[CmdletBinding()]
param([switch]$LibraryChecksOnly, [switch]$OrphanOnly, [switch]$TimeoutOnly, [string]$HoldJsonPath = '')
$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
$shell = (Get-Process -Id $PID).Path
$worker = Join-Path $PSScriptRoot 'verification.ps1'
$launcher = Join-Path $PSScriptRoot 'start-verification.ps1'
$statusScript = Join-Path $PSScriptRoot 'verification-status.ps1'
function Assert-True($Condition, [string]$Message) {
    if (-not $Condition) { throw "ASSERTION FAILED: $Message" }
}
function Read-State([string]$Id) {
    Get-Content -Raw -LiteralPath (Join-Path $repo "scratch/continuation/runs/$Id/status.json") | ConvertFrom-Json
}
function Wait-State([string]$Id, [string]$Step = '', [int]$Seconds = 20) {
    $deadline = [DateTime]::UtcNow.AddSeconds($Seconds)
    do {
        $path = Join-Path $repo "scratch/continuation/runs/$Id/status.json"
        if (Test-Path -LiteralPath $path) {
            $state = Read-State $Id
            if ($Step -and $state.activeStep -eq $Step -and $state.childPid) { return $state }
            if (-not $Step -and $state.status -notin @('starting', 'running')) { return $state }
        }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $deadline)
    throw "Timed out waiting for $Id ($Step)"
}
function Run-Worker([string]$Profile, [string]$Id, [string]$Resume = '', [int]$Timeout = 30) {
    $arguments = @('-NoProfile', '-NonInteractive', '-File', $worker, '-Profile', $Profile, '-RunId', $Id, '-JobTimeoutSeconds', "$Timeout")
    if ($Resume) { $arguments += @('-ResumeRunId', $Resume) }
    & $shell @arguments | Out-Host
    return $LASTEXITCODE
}
function Assert-TimeoutScenario([string]$Id, [int]$Timeout = 3) {
    Assert-True ((Run-Worker 'smoke-slow' $Id '' $Timeout) -eq 124) 'job timeout is bounded'
    $timeoutState = Read-State $Id
    Assert-True ($timeoutState.status -eq 'timed-out') 'timeout is explicit'
    $timedOutStep = $timeoutState.steps[-1]
    Assert-True ($timedOutStep.name -eq 'wait' -and $timedOutStep.exitCode -eq 124 -and $timedOutStep.processId) 'timeout occurs during an active native wait step'
    Assert-True (-not (Test-VerificationProcess $timedOutStep.processId $timedOutStep.processStartedUtc)) 'timeout kills the native step process'
}
foreach ($path in @($worker, $launcher, $statusScript)) {
    Assert-True (Test-Path -LiteralPath $path) "Required durable verification script exists: $path"
}
foreach ($path in @($worker, $launcher, $statusScript, $PSCommandPath)) {
    $tokens = $null; $parseErrors = $null
    [void][System.Management.Automation.Language.Parser]::ParseFile($path, [ref]$tokens, [ref]$parseErrors)
    Assert-True ($parseErrors.Count -eq 0) "PowerShell parser accepts $path : $parseErrors"
}
. $worker -Library
if ($HoldJsonPath) {
    Assert-VerificationOutputPath $HoldJsonPath
    Assert-VerificationOutputPath ($HoldJsonPath + '.held')
    $heldFile = [IO.File]::Open($HoldJsonPath, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::ReadWrite)
    try { [IO.File]::WriteAllText(($HoldJsonPath + '.held'), 'held'); Start-Sleep -Milliseconds 300 }
    finally { $heldFile.Dispose() }
    exit 0
}
Assert-True (Test-VerificationBenchmarkOutput "running 1 test`ntest conservative::tests::wp6_release_million_triangles ... ok`ntest result: ok. 1 passed; 0 failed;") 'benchmark accepts an executed passing filtered test'
Assert-True (-not (Test-VerificationBenchmarkOutput "running 0 tests`ntest result: ok. 0 passed; 0 failed;")) 'zero matching benchmark tests cannot count as verification'
$jsonCheckDirectory = Join-Path $repo ('scratch/continuation/library-checks/' + [guid]::NewGuid().ToString('N'))
Assert-VerificationOutputPath $jsonCheckDirectory
[void][IO.Directory]::CreateDirectory($jsonCheckDirectory)
$jsonCheckPath = Join-Path $jsonCheckDirectory 'reader-overlap.json'
Write-VerificationJson $jsonCheckPath @{ before = $true }
$readerInfo = [Diagnostics.ProcessStartInfo]::new($shell)
$readerInfo.UseShellExecute = $false; $readerInfo.CreateNoWindow = $true
foreach ($argument in @('-NoProfile', '-NonInteractive', '-File', $PSCommandPath, '-HoldJsonPath', $jsonCheckPath)) { $readerInfo.ArgumentList.Add($argument) }
$readerProcess = [Diagnostics.Process]::Start($readerInfo)
$readerDeadline = [DateTime]::UtcNow.AddSeconds(3)
while (-not (Test-Path -LiteralPath ($jsonCheckPath + '.held'))) {
    if ([DateTime]::UtcNow -ge $readerDeadline) { throw 'JSON reader did not start.' }
    Start-Sleep -Milliseconds 10
}
Write-VerificationJson $jsonCheckPath @{ after = $true }
Assert-True ($readerProcess.WaitForExit(3000)) 'overlapping JSON reader exits'
Assert-True ((Get-Content -Raw -LiteralPath $jsonCheckPath | ConvertFrom-Json).after) 'transient reader overlap does not lose a status write'
if ($LibraryChecksOnly) { Write-Output 'PASS: parser and library checks'; exit 0 }
$prefix = 'selftest-' + [DateTime]::UtcNow.ToString('yyyyMMdd-HHmmss') + '-' + [guid]::NewGuid().ToString('N').Substring(0, 6)
$passed = 0
if ($TimeoutOnly) { Assert-TimeoutScenario "$prefix-timeout"; Write-Output 'PASS: active native timeout'; exit 0 }
Assert-True ((Run-Worker 'smoke-orphan' "$prefix-orphan") -eq 125) 'unexpected native descendants are contained and fail the step'
$orphanState = Read-State "$prefix-orphan"
Assert-True ($orphanState.status -eq 'failed') 'orphan descendant prevents completion'
$orphanLog = Get-Content -Raw -LiteralPath $orphanState.steps[-1].stdout
Assert-True ($orphanLog -match 'orphan-process:(\d+)\|([^\r\n]+)') 'orphan identity is available to the cleanup regression'
Assert-True (-not (Test-VerificationProcess ([int]$Matches[1]) $Matches[2])) 'job cleanup kills a grandchild even after its direct parent exits'
$passed++
if ($OrphanOnly) { Write-Output 'PASS: orphan process containment'; exit 0 }
Assert-True ((Run-Worker 'smoke' "$prefix-pass") -eq 0) 'smoke exits zero'
$state = Read-State "$prefix-pass"
Assert-True ($state.status -eq 'completed' -and $state.steps.Count -eq 2 -and $null -ne $state.finishedUtc) 'completion is explicit with two step results'
Assert-True ($state.sourceBefore -eq $state.sourceAfter) 'successful run has identical source fingerprints'
$passed++
Assert-True ((Run-Worker 'smoke-failure' "$prefix-failure") -eq 7) 'failure exit code propagates'
$state = Read-State "$prefix-failure"
Assert-True ($state.status -eq 'failed' -and $state.steps.Count -eq 2) 'first failing step stops later work'
Assert-True ((Run-Worker 'smoke-failure' "$prefix-refuse-failure" "$prefix-failure") -ne 0) 'failed run is not blindly retried through resume'
$passed++
Assert-TimeoutScenario "$prefix-timeout"
$passed++
# This launcher exits before the independent worker/status checks below.
& $shell -NoProfile -NonInteractive -File $launcher -Profile smoke-slow -RunId "$prefix-interrupt" -JobTimeoutSeconds 30 | Out-Host
Assert-True ($LASTEXITCODE -eq 0) 'launcher exits successfully'
$state = Wait-State "$prefix-interrupt" 'wait'
Assert-True ($state.workerPid -ne $PID) 'background worker has its own PID'
Assert-True ((Run-Worker 'smoke' "$prefix-concurrent") -ne 0) 'single worker lock rejects concurrent work'
$workerProcess = Get-Process -Id $state.workerPid
$workerProcess.Kill()
Assert-True ($workerProcess.WaitForExit(5000)) 'test kills only the worker process'
Start-Sleep -Milliseconds 500
Assert-True (-not (Test-VerificationProcess $state.childPid $state.childStartedUtc)) 'worker death closes its job and kills native descendants'
$observed = & $shell -NoProfile -NonInteractive -File $statusScript -RunId "$prefix-interrupt" -Json | ConvertFrom-Json
Assert-True ($observed.status -eq 'interrupted' -and -not $observed.workerAlive) 'dead worker is interrupted, never completed'
Assert-True ((Run-Worker 'smoke-slow' "$prefix-resume" "$prefix-interrupt") -eq 0) 'matching interrupted run resumes'
$state = Read-State "$prefix-resume"
Assert-True ($state.status -eq 'completed' -and $state.steps[0].reused -and -not $state.steps[1].reused) 'resume reuses only successful checkpoint steps'
Assert-True ((Run-Worker 'smoke' "$prefix-profile-refused" "$prefix-resume") -ne 0) 'profile mismatch rejects reuse'
$passed++
& $shell -NoProfile -NonInteractive -File $launcher -Profile smoke-slow -RunId "$prefix-visibility" -JobTimeoutSeconds 30 | Out-Host
$liveState = Wait-State "$prefix-visibility" 'wait'
$liveState.workerPid = [int]::MaxValue
$livePath = Join-Path $repo "scratch/continuation/runs/$prefix-visibility/status.json"
Write-VerificationJson $livePath $liveState
$visibility = & $shell -NoProfile -NonInteractive -File $statusScript -RunId "$prefix-visibility" -Json | ConvertFrom-Json
Assert-True ($visibility.status -eq 'running-visibility-unknown' -and $visibility.lockState -eq 'held' -and $null -eq $visibility.workerAlive) 'held lock prevents a false interrupted claim when PID is invisible'
[void](Wait-State "$prefix-visibility")
$passed++
$fixture = Join-Path $PSScriptRoot ".verification-fixture-$prefix.txt"
try {
    [IO.File]::WriteAllText($fixture, 'before')
    & $shell -NoProfile -NonInteractive -File $launcher -Profile smoke-slow -RunId "$prefix-change" -JobTimeoutSeconds 30 | Out-Host
    Assert-True ($LASTEXITCODE -eq 0) 'source-change worker launches'
    [void](Wait-State "$prefix-change" 'wait')
    [IO.File]::WriteAllText($fixture, 'after')
    $state = Wait-State "$prefix-change"
    Assert-True ($state.status -eq 'source-changed' -and $state.sourceBefore -ne $state.sourceAfter) 'source mutation prevents a verified result'
    Assert-True ((Run-Worker 'smoke-slow' "$prefix-source-refused" "$prefix-change") -ne 0) 'changed-source run cannot resume'
    $passed++
} finally {
    if (Test-Path -LiteralPath $fixture) { Remove-Item -LiteralPath $fixture }
}
Write-Output "PASS: $passed durable-runner scenarios; run prefix $prefix"
