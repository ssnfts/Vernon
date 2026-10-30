#requires -Version 7.2
<#
Bounded local verification only. No AI calls, network orchestration or commits.
Run directly in a terminal, or use start-verification.ps1 for a hidden worker.
The full profile saves a generated catalogue under the run directory; review it
before copying to docs/NODES.md. GUI inspection remains a separate manual check.
#>
[CmdletBinding()]
param(
    [ValidateSet('full', 'clean-focused', 'wp6-benchmark', 'wp6', 'wp8-real', 'wp8', 'smoke', 'smoke-failure', 'smoke-slow', 'smoke-orphan')]
    [string]$Profile = 'full',
    [ValidatePattern('^[A-Za-z0-9][A-Za-z0-9_-]{0,95}$')][string]$RunId = ('run-' + [DateTime]::UtcNow.ToString('yyyyMMdd-HHmmss') + '-' + [guid]::NewGuid().ToString('N').Substring(0, 8)),
    [ValidatePattern('^$|^[A-Za-z0-9][A-Za-z0-9_-]{0,95}$')][string]$ResumeRunId = '',
    [ValidateRange(1, 86400)][int]$JobTimeoutSeconds = 7200,
    [switch]$Library,
    [ValidateSet('none', 'pass', 'fail', 'wait', 'orphan')][string]$SmokeChild = 'none',
    [switch]$StepChild,
    [ValidateSet('build', 'tests', 'clippy', 'demo', 'catalogue', 'clean-tests', 'wp6-release', 'wp8-real', 'first', 'failure', 'must-not-run', 'second', 'wait', 'orphan')][string]$StepName = 'first'
)
$ErrorActionPreference = 'Stop'
$script:VerificationRepo = [IO.Path]::GetFullPath((Split-Path $PSScriptRoot -Parent))
$script:VerificationRoot = Join-Path $script:VerificationRepo 'scratch/continuation'
function Get-VerificationPath([string]$Id) {
    if ($Id -notmatch '^[A-Za-z0-9][A-Za-z0-9_-]{0,95}$') { throw 'Invalid run id.' }
    Join-Path $script:VerificationRoot "runs/$Id"
}
function Assert-VerificationOutputPath([string]$Path) {
    $resolved = [IO.Path]::GetFullPath($Path)
    if (-not $resolved.StartsWith($script:VerificationRepo + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw 'Output must be inside this repository.' }
    $cursor = $resolved
    while ($cursor -and $cursor -ne $script:VerificationRepo) {
        if (Test-Path -LiteralPath $cursor) {
            if ((Get-Item -LiteralPath $cursor -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "Output path cannot use a reparse point: $cursor" }
        }
        $cursor = Split-Path $cursor -Parent
    }
}
function Write-VerificationJson([string]$Path, $Value) {
    Assert-VerificationOutputPath $Path
    $tempPath = $Path + '.tmp'
    Assert-VerificationOutputPath $tempPath
    [IO.File]::WriteAllText($tempPath, ($Value | ConvertTo-Json -Depth 12), [Text.UTF8Encoding]::new($false))
    $replaceDeadline = [DateTime]::UtcNow.AddSeconds(2)
    while ($true) {
        try { [IO.File]::Move($tempPath, $Path, $true); return }
        catch {
            $cause = $_.Exception
            while ($cause.InnerException) { $cause = $cause.InnerException }
            if (($cause.HResult -band 65535) -notin @(5, 32, 33) -or [DateTime]::UtcNow -ge $replaceDeadline) { throw }
            Start-Sleep -Milliseconds 25
        }
    }
}
function Test-VerificationProcess($ProcessId, [string]$StartedUtc) {
    if (-not $ProcessId -or -not $StartedUtc) { return $false }
    try {
        $process = Get-Process -Id ([int]$ProcessId) -ErrorAction Stop
        return $process.StartTime.ToUniversalTime().ToString('o') -eq $StartedUtc
    } catch { return $false }
}
function Get-VerificationLockState {
    $path = Join-Path $script:VerificationRoot 'worker.lock'
    if (-not (Test-Path -LiteralPath $path)) { return 'available' }
    try {
        $probe = [IO.File]::Open($path, [IO.FileMode]::Open, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
        $probe.Dispose()
        return 'available'
    } catch [IO.IOException] {
        if (($_.Exception.HResult -band 65535) -in @(32, 33)) { return 'held' }
        return 'unknown'
    } catch { return 'unknown' }
}
function New-VerificationProcessJob {
    if (-not ('VernonVerificationProcessJob' -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Diagnostics;
using System.Runtime.InteropServices;
public sealed class VernonVerificationProcessJob : IDisposable {
    [StructLayout(LayoutKind.Sequential)] struct Limits {
        public long ProcessTime, JobTime;
        public uint Flags;
        public UIntPtr MinWorkingSet, MaxWorkingSet;
        public uint ActiveProcessLimit;
        public UIntPtr Affinity;
        public uint PriorityClass, SchedulingClass;
    }
    [StructLayout(LayoutKind.Sequential)] struct IoCounters { public ulong ReadOps, WriteOps, OtherOps, ReadBytes, WriteBytes, OtherBytes; }
    [StructLayout(LayoutKind.Sequential)] struct ExtendedLimits {
        public Limits Basic;
        public IoCounters Io;
        public UIntPtr ProcessMemory, JobMemory, PeakProcessMemory, PeakJobMemory;
    }
    [StructLayout(LayoutKind.Sequential)] struct Accounting {
        public long UserTime, KernelTime, PeriodUserTime, PeriodKernelTime;
        public uint PageFaults, TotalProcesses, ActiveProcesses, TerminatedProcesses;
    }
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)] static extern IntPtr CreateJobObjectW(IntPtr attributes, string name);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool SetInformationJobObject(IntPtr job, int info, ref ExtendedLimits data, uint size);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool QueryInformationJobObject(IntPtr job, int info, out Accounting data, uint size, IntPtr returned);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool AssignProcessToJobObject(IntPtr job, IntPtr process);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool TerminateJobObject(IntPtr job, uint exitCode);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr handle);
    IntPtr handle;
    public VernonVerificationProcessJob() {
        handle = CreateJobObjectW(IntPtr.Zero, null);
        if (handle == IntPtr.Zero) throw new Win32Exception();
        var limits = new ExtendedLimits(); limits.Basic.Flags = 0x2000;
        if (!SetInformationJobObject(handle, 9, ref limits, (uint)Marshal.SizeOf<ExtendedLimits>())) { int error = Marshal.GetLastWin32Error(); Dispose(); throw new Win32Exception(error); }
    }
    public void Attach(Process process) { if (!AssignProcessToJobObject(handle, process.Handle)) throw new Win32Exception(); }
    public uint ActiveProcesses {
        get { Accounting data; if (!QueryInformationJobObject(handle, 1, out data, (uint)Marshal.SizeOf<Accounting>(), IntPtr.Zero)) throw new Win32Exception(); return data.ActiveProcesses; }
    }
    public void Terminate() { if (!TerminateJobObject(handle, 125)) throw new Win32Exception(); }
    public void Dispose() { if (handle != IntPtr.Zero) { CloseHandle(handle); handle = IntPtr.Zero; } }
}
'@
    }
    [VernonVerificationProcessJob]::new()
}
function Get-VerificationFingerprint {
    $paths = [Collections.Generic.List[string]]::new()
    foreach ($name in @('Cargo.toml', 'Cargo.lock', 'AGENTS.md', 'CLEAN_ROOM.md')) {
        $path = Join-Path $script:VerificationRepo $name
        if (Test-Path -LiteralPath $path) { $paths.Add($path) }
    }
    foreach ($name in @('crates', 'app', 'examples', 'scripts', '.cargo')) {
        $path = Join-Path $script:VerificationRepo $name
        if (Test-Path -LiteralPath $path) {
            foreach ($file in Get-ChildItem -LiteralPath $path -File -Recurse -Force) { $paths.Add($file.FullName) }
        }
    }
    $manifest = foreach ($path in ($paths | Sort-Object -Unique)) {
        $relative = [IO.Path]::GetRelativePath($script:VerificationRepo, $path).Replace('\', '/')
        $digest = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash
        "$relative`0$digest"
    }
    $bytes = [Text.Encoding]::UTF8.GetBytes(($manifest -join "`n"))
    [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($bytes)).ToLowerInvariant()
}
function Test-VerificationBenchmarkOutput([string]$Output) {
    return $Output -match '(?m)^running [1-9][0-9]* tests?\s*$' -and $Output -match '(?m)^test result: ok\. [1-9][0-9]* passed; 0 failed;'
}
function Test-VerificationWp8Output([string]$Output, [string]$ErrorOutput = '') {
    $combined = $Output + "`n" + $ErrorOutput
    return (Test-VerificationBenchmarkOutput $Output) -and
        $combined -cmatch '(?m)^WP8_REAL_COMPLETE(?:\s|$)' -and
        $combined -notmatch '(?im)^\s*SKIPPED\b'
}
function Assert-VerificationWp8Input([string]$Name, [string]$Data = $env:VERNON_COLMAP_DATA) {
    if ($Name -in @('wp8-real', 'wp8') -and [string]::IsNullOrWhiteSpace($Data)) {
        throw 'WP8 real acceptance requires a nonblank VERNON_COLMAP_DATA photo-directory or explicit download opt-in.'
    }
}
function Get-VerificationSteps([string]$Name, [string]$OutputDirectory = $script:VerificationRoot) {
    $shell = (Get-Process -Id $PID).Path
    $worker = Join-Path $PSScriptRoot 'verification.ps1'
    switch ($Name) {
        'full' {
            @(
                @{ name = 'build'; executable = 'cargo'; arguments = @('build', '--workspace') },
                @{ name = 'tests'; executable = 'cargo'; arguments = @('test', '--workspace') },
                @{ name = 'clippy'; executable = 'cargo'; arguments = @('clippy', '--workspace', '--all-targets', '--', '-D', 'warnings') },
                @{ name = 'demo'; executable = 'cargo'; arguments = @('run', '-p', 'vernon-cli', '--', 'cook', 'examples/bodykit_pipeline.vrn', '--out', (Join-Path $OutputDirectory 'demo.obj'), '--work', (Join-Path $OutputDirectory 'demo-work')) },
                @{ name = 'catalogue'; executable = 'cargo'; arguments = @('run', '-p', 'vernon-cli', '--', 'list', '--markdown') }
            )
        }
        'clean-focused' { @{ name = 'clean-tests'; executable = 'cargo'; arguments = @('test', '-p', 'vernon-nodes-clean') } }
        'wp8-real' { @{name='wp8-real';executable='cargo';arguments=@('test','-p','vernon-cli','--test','photogrammetry','colmap_public_sample_dense_mesh_and_scale','--','--ignored','--nocapture')} }
        'wp8' {
            Get-VerificationSteps 'wp8-real' $OutputDirectory
            Get-VerificationSteps 'full' $OutputDirectory
        }
        'wp6-benchmark' { @{ name = 'wp6-release'; executable = 'cargo'; arguments = @('test', '--release', '-p', 'vernon-nodes-clean', 'wp6_release', '--', '--ignored', '--nocapture') } }
        'wp6' {
            Get-VerificationSteps 'wp6-benchmark' $OutputDirectory
            Get-VerificationSteps 'full' $OutputDirectory
        }
        default {
            @{ name = 'first'; executable = $shell; arguments = @('-NoProfile', '-NonInteractive', '-File', $worker, '-SmokeChild', 'pass') }
            if ($Name -eq 'smoke-failure') {
                @{ name = 'failure'; executable = $shell; arguments = @('-NoProfile', '-NonInteractive', '-File', $worker, '-SmokeChild', 'fail') }
                @{ name = 'must-not-run'; executable = $shell; arguments = @('-NoProfile', '-NonInteractive', '-File', $worker, '-SmokeChild', 'pass') }
            } else {
                $kind = if ($Name -eq 'smoke-slow') { 'wait' } elseif ($Name -eq 'smoke-orphan') { 'orphan' } else { 'pass' }
                @{ name = $(if ($kind -eq 'pass') { 'second' } else { $kind }); executable = $shell; arguments = @('-NoProfile', '-NonInteractive', '-File', $worker, '-SmokeChild', $kind) }
            }
        }
    }
}
if ($Library) { return }
if ($SmokeChild -ne 'none') {
    if ($SmokeChild -eq 'wait') { Start-Sleep -Seconds 6 }
    if ($SmokeChild -eq 'fail') { Write-Output 'intentional smoke failure'; exit 7 }
    if ($SmokeChild -eq 'orphan') {
        $orphanInfo = [Diagnostics.ProcessStartInfo]::new((Get-Process -Id $PID).Path)
        $orphanInfo.UseShellExecute = $false; $orphanInfo.CreateNoWindow = $true
        foreach ($argument in @('-NoProfile', '-NonInteractive', '-File', $PSCommandPath, '-SmokeChild', 'wait')) { $orphanInfo.ArgumentList.Add($argument) }
        $orphan = [Diagnostics.Process]::Start($orphanInfo)
        Write-Output "orphan-process:$($orphan.Id)|$($orphan.StartTime.ToUniversalTime().ToString('o'))"
        exit 0
    }
    Write-Output "smoke $SmokeChild passed"
    exit 0
}
if ($StepChild) {
    # The wrapper cannot spawn the fixed native command until its parent has
    # attached it to a kill-on-close Job Object and published this gate file.
    $stepDirectory = Get-VerificationPath $RunId
    $gate = Join-Path $stepDirectory "$StepName.gate"
    $gateDeadline = [DateTime]::UtcNow.AddSeconds(10)
    while (-not (Test-Path -LiteralPath $gate)) {
        if ([DateTime]::UtcNow -ge $gateDeadline) { throw 'Parent did not authorize the contained step within ten seconds.' }
        Start-Sleep -Milliseconds 25
    }
    $selected = @(Get-VerificationSteps $Profile $stepDirectory | Where-Object { $_.name -eq $StepName })
    if ($selected.Count -ne 1) { throw 'Unknown fixed-profile step.' }
    $nativeArguments = $selected[0].arguments
    & $selected[0].executable @nativeArguments
    exit $LASTEXITCODE
}
$runDirectory = Get-VerificationPath $RunId
$statePath = Join-Path $runDirectory 'status.json'
$lockPath = Join-Path $script:VerificationRoot 'worker.lock'
$lock = $null
$state = $null
$child = $null
$processJob = $null
$resultCode = 1
try {
    Assert-VerificationWp8Input $Profile
    Assert-VerificationOutputPath $runDirectory
    [void][IO.Directory]::CreateDirectory($script:VerificationRoot)
    Assert-VerificationOutputPath $lockPath
    try { $lock = [IO.File]::Open($lockPath, [IO.FileMode]::OpenOrCreate, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None) }
    catch { throw 'Another verification worker holds worker.lock. Check verification-status.ps1 before launching again.' }
    $staleLock = $false
    if ($lock.Length -gt 0) {
        $reader = [IO.StreamReader]::new($lock, [Text.Encoding]::UTF8, $true, 1024, $true)
        $previousLock = $reader.ReadToEnd() | ConvertFrom-Json
        $reader.Dispose()
        if (Test-VerificationProcess $previousLock.workerPid $previousLock.workerStartedUtc) { throw "Worker $($previousLock.workerPid) is already running." }
        if (-not $previousLock.jobBound) {
            $oldStatePath = Join-Path (Get-VerificationPath $previousLock.runId) 'status.json'
            if (Test-Path -LiteralPath $oldStatePath) {
                $oldState = Get-Content -Raw -LiteralPath $oldStatePath | ConvertFrom-Json
                if ($oldState.childPid -and -not $oldState.finishedUtc) { throw 'Legacy worker left an uncontained child record. Inspect its process tree before starting new work.' }
            }
        }
        $staleLock = $true
    }
    if (Test-Path -LiteralPath $runDirectory) { throw "Run id already exists: $RunId. Use a new id; resume uses -ResumeRunId." }
    [void][IO.Directory]::CreateDirectory($runDirectory)
    $started = (Get-Process -Id $PID).StartTime.ToUniversalTime().ToString('o')
    $lockBytes = [Text.Encoding]::UTF8.GetBytes((@{ runId = $RunId; workerPid = $PID; workerStartedUtc = $started; jobBound = $true } | ConvertTo-Json))
    $lock.Position = 0; $lock.SetLength(0); $lock.Write($lockBytes); $lock.Flush($true)
    . (Join-Path $PSScriptRoot 'dev-env.ps1')
    $env:TEMP = Join-Path $runDirectory 'temp'
    $env:TMP = $env:TEMP
    $env:CARGO_INCREMENTAL = '0'
    $env:CARGO_TARGET_DIR = Join-Path $script:VerificationRepo 'target'
    Assert-VerificationOutputPath $env:CARGO_TARGET_DIR
    if ($Profile -in @('full', 'clean-focused', 'wp6-benchmark', 'wp6', 'wp8-real', 'wp8') -and (Test-Path -LiteralPath $env:CARGO_TARGET_DIR)) {
        foreach ($item in Get-ChildItem -LiteralPath $env:CARGO_TARGET_DIR -Recurse -Force) {
            if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "Cargo target tree contains a reparse point: $($item.FullName)" }
        }
    }
    $env:CARGO_NET_OFFLINE = 'true'
    [void][IO.Directory]::CreateDirectory($env:TEMP)
    $source = Get-VerificationFingerprint
    $steps = @(Get-VerificationSteps $Profile $runDirectory)
    $state = [ordered]@{
        schemaVersion = 1; runnerVersion = 2; runId = $RunId; profile = $Profile; status = 'running'
        workerPid = $PID; workerStartedUtc = $started; startedUtc = [DateTime]::UtcNow.ToString('o'); finishedUtc = $null
        jobTimeoutSeconds = $JobTimeoutSeconds; exitCode = $null; activeStep = $null; childPid = $null; childStartedUtc = $null
        sourceBefore = $source; sourceAfter = $null; resumedFrom = $ResumeRunId; recoveredStaleLock = $staleLock
        steps = @(); message = ''; runDirectory = $runDirectory; cataloguePath = $null
    }
    Write-VerificationJson $statePath $state
    Write-VerificationJson (Join-Path $script:VerificationRoot 'latest.json') @{ runId = $RunId; statusPath = $statePath }
    $reuse = @()
    if ($ResumeRunId) {
        $previousPath = Join-Path (Get-VerificationPath $ResumeRunId) 'status.json'
        $previous = Get-Content -Raw -LiteralPath $previousPath | ConvertFrom-Json
        if ($previous.profile -ne $Profile -or $previous.runnerVersion -ne 2 -or $previous.sourceBefore -ne $source) { throw 'Resume requires the same profile, runner version and source fingerprint.' }
        if ($previous.status -in @('failed', 'timed-out', 'source-changed')) { throw "Cannot resume a $($previous.status) run. Inspect logs and explicitly start a new run." }
        if ((Test-VerificationProcess $previous.workerPid $previous.workerStartedUtc) -or (Test-VerificationProcess $previous.childPid $previous.childStartedUtc)) { throw 'The previous worker or its active child is still alive. Do not start competing work.' }
        $reuse = @($previous.steps)
    }
    $deadline = [DateTime]::Parse($state.startedUtc).ToUniversalTime().AddSeconds($JobTimeoutSeconds)
    foreach ($step in $steps) {
        $currentSource = Get-VerificationFingerprint
        if ($currentSource -ne $source) { $state.status = 'source-changed'; $resultCode = 3; break }
        $prior = @($reuse | Where-Object { $_.name -eq $step.name -and $_.exitCode -eq 0 -and $_.status -eq 'passed' })
        if ($prior.Count -eq 1) {
            $state.steps += [ordered]@{ name = $step.name; status = 'passed'; exitCode = 0; reused = $true; originalRunId = $ResumeRunId; stdout = $prior[0].stdout; stderr = $prior[0].stderr; startedUtc = $prior[0].startedUtc; finishedUtc = $prior[0].finishedUtc }
            Write-VerificationJson $statePath $state
            continue
        }
        if ([DateTime]::UtcNow -ge $deadline) { $state.status = 'timed-out'; $resultCode = 124; break }
        $stdoutPath = Join-Path $runDirectory ($step.name + '.stdout.log')
        $stderrPath = Join-Path $runDirectory ($step.name + '.stderr.log')
        $record = [ordered]@{ name = $step.name; status = 'running'; exitCode = $null; reused = $false; executable = $step.executable; arguments = $step.arguments; stdout = $stdoutPath; stderr = $stderrPath; startedUtc = [DateTime]::UtcNow.ToString('o'); finishedUtc = $null; processId = $null; processStartedUtc = $null }
        $state.steps += $record
        $state.activeStep = $step.name
        Write-VerificationJson $statePath $state
        # ArgumentList on ProcessStartInfo avoids shell evaluation and quoting.
        $startInfo = [Diagnostics.ProcessStartInfo]::new()
        $startInfo.FileName = (Get-Process -Id $PID).Path
        $startInfo.WorkingDirectory = $script:VerificationRepo
        $startInfo.UseShellExecute = $false
        $startInfo.CreateNoWindow = $true
        $startInfo.RedirectStandardOutput = $true
        $startInfo.RedirectStandardError = $true
        foreach ($argument in @('-NoProfile', '-NonInteractive', '-File', $PSCommandPath, '-StepChild', '-Profile', $Profile, '-RunId', $RunId, '-StepName', $step.name)) { $startInfo.ArgumentList.Add($argument) }
        $processJob = New-VerificationProcessJob
        $child = [Diagnostics.Process]::Start($startInfo)
        $processJob.Attach($child)
        $gate = Join-Path $runDirectory ($step.name + '.gate')
        Assert-VerificationOutputPath $gate
        [IO.File]::WriteAllText($gate, 'contained')
        $state.childPid = $child.Id
        $state.childStartedUtc = $child.StartTime.ToUniversalTime().ToString('o')
        $record.processId = $state.childPid
        $record.processStartedUtc = $state.childStartedUtc
        Write-VerificationJson $statePath $state
        $outStream = [IO.File]::Create($stdoutPath)
        $errStream = [IO.File]::Create($stderrPath)
        $outCopy = $child.StandardOutput.BaseStream.CopyToAsync($outStream)
        $errCopy = $child.StandardError.BaseStream.CopyToAsync($errStream)
        try {
            while (-not $child.WaitForExit(200)) {
                if ([DateTime]::UtcNow -ge $deadline) {
                    $processJob.Terminate()
                    if (-not $child.WaitForExit(5000)) { throw 'Native process did not exit within the five-second timeout cleanup window.' }
                    $state.status = 'timed-out'; $resultCode = 124
                    break
                }
            }
            $leftoverDeadline = [DateTime]::UtcNow.AddMilliseconds(500)
            while ($processJob.ActiveProcesses -gt 0 -and [DateTime]::UtcNow -lt $leftoverDeadline) { Start-Sleep -Milliseconds 25 }
            $leftoverDescendants = $processJob.ActiveProcesses
            $processJob.Dispose(); $processJob = $null
            if (-not [Threading.Tasks.Task]::WaitAll([Threading.Tasks.Task[]]@($outCopy, $errCopy), 5000)) { throw 'Native log streams did not close within five seconds. The run cannot be verified.' }
        } finally { $child.StandardOutput.Dispose(); $child.StandardError.Dispose(); $outStream.Dispose(); $errStream.Dispose() }
        $record.exitCode = if ($state.status -eq 'timed-out') { 124 } else { $child.ExitCode }
        if ($leftoverDescendants -gt 0 -and $record.exitCode -eq 0) {
            $record.exitCode = 125
            $state.message = 'Unexpected native descendants remained after the step exited. The Job Object killed them; verification stopped.'
        }
        if ($step.name -eq 'wp8-real' -and $record.exitCode -eq 0 -and
            -not (Test-VerificationWp8Output ([IO.File]::ReadAllText($stdoutPath)) ([IO.File]::ReadAllText($stderrPath)))) {
            $record.exitCode = 4
            $state.message = 'WP8 external acceptance needs an executed passing test and WP8_REAL_COMPLETE marker without any SKIPPED diagnostic.'
        }
        if ($step.name -eq 'wp6-release' -and $record.exitCode -eq 0 -and -not (Test-VerificationBenchmarkOutput ([IO.File]::ReadAllText($stdoutPath)))) {
            $record.exitCode = 4
            $state.message = 'The WP6 benchmark filter did not execute a passing test. A zero-test run cannot verify the benchmark.'
        }
        $record.status = if ($record.exitCode -eq 0) { 'passed' } else { 'failed' }
        $record.finishedUtc = [DateTime]::UtcNow.ToString('o')
        $state.childPid = $null; $state.childStartedUtc = $null; $state.activeStep = $null
        $currentSource = Get-VerificationFingerprint
        $state.sourceAfter = $currentSource
        if ($currentSource -ne $source) { $state.status = 'source-changed'; $resultCode = 3 }
        elseif ($record.exitCode -ne 0 -and $state.status -ne 'timed-out') { $state.status = 'failed'; $resultCode = $record.exitCode }
        if ($step.name -eq 'catalogue' -and $record.exitCode -eq 0) { $state.cataloguePath = $stdoutPath }
        Write-VerificationJson $statePath $state
        if ($state.status -ne 'running') { break }
    }
    $state.sourceAfter = Get-VerificationFingerprint
    if ($state.sourceAfter -ne $source) { $state.status = 'source-changed'; $resultCode = 3 }
    elseif ($state.status -eq 'running') { $state.status = 'completed'; $resultCode = 0 }
} catch {
    if ($state) { $state.status = 'failed'; $state.message = $_.Exception.Message }
    else {
        $launchDirectory = Join-Path $script:VerificationRoot 'launches'
        Assert-VerificationOutputPath $launchDirectory
        [void][IO.Directory]::CreateDirectory($launchDirectory)
        $launchError = Join-Path $launchDirectory "$RunId.stderr.log"
        Assert-VerificationOutputPath $launchError
        [IO.File]::WriteAllText($launchError, $_.Exception.Message)
    }
    Write-Error -Message $_.Exception.Message -ErrorAction Continue
} finally {
    if ($processJob) { $processJob.Dispose() }
    if ($child -and -not $child.HasExited) { $child.Kill($true); [void]$child.WaitForExit(5000) }
    if ($state) {
        $state.exitCode = $resultCode
        $state.finishedUtc = [DateTime]::UtcNow.ToString('o')
        $state.activeStep = $null; $state.childPid = $null; $state.childStartedUtc = $null
        Write-VerificationJson $statePath $state
        Write-Output "$($state.status): $RunId; exit $resultCode; $statePath"
    }
    if ($lock) { $lock.Dispose() }
}
exit $resultCode
