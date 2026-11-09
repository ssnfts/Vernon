#requires -Version 7.2
<#
Bounded local verification only. No AI calls, network orchestration or commits.
Run directly in a terminal, or use start-verification.ps1 for a hidden worker.
The full profile saves a generated catalogue under the run directory; review it
before copying to docs/NODES.md. GUI inspection remains a separate manual check.
#>
[CmdletBinding()]
param(
    [ValidateSet('full', 'clean-focused', 'wp6-benchmark', 'wp6', 'wp8-real', 'wp8', 'wp9-focused', 'wp9-perf', 'wp9-wsl-runner', 'wp9-native', 'wp9-geometry', 'wp9-sizing', 'wp9-sizing-serial', 'wp9-solve-pilot', 'wp9-solve', 'wp9-diagnostic', 'smoke', 'smoke-failure', 'smoke-slow', 'smoke-orphan', 'smoke-cancel')]
    [string]$Profile = 'full',
    [ValidatePattern('^[A-Za-z0-9][A-Za-z0-9_-]{0,95}$')][string]$RunId = ('run-' + [DateTime]::UtcNow.ToString('yyyyMMdd-HHmmss') + '-' + [guid]::NewGuid().ToString('N').Substring(0, 8)),
    [ValidatePattern('^$|^[A-Za-z0-9][A-Za-z0-9_-]{0,95}$')][string]$ResumeRunId = '',
    [ValidateRange(1, 172800)][int]$JobTimeoutSeconds = 7200,
    [switch]$Library,
    [ValidateSet('none', 'pass', 'fail', 'wait', 'orphan', 'hold')][string]$SmokeChild = 'none',
    [switch]$StepChild,
    [ValidateSet('build', 'tests', 'clippy', 'demo', 'catalogue', 'clean-tests', 'wp6-release', 'wp8-real', 'wp9-tests', 'wp9-perf-tests', 'wp9-extproc', 'wp9-wsl', 'wp9-native', 'wp9-geometry', 'wp9-sizing', 'wp9-solve', 'wp9-diagnostic', 'first', 'failure', 'must-not-run', 'second', 'wait', 'orphan', 'hold')][string]$StepName = 'first'
)
$ErrorActionPreference = 'Stop'
# Outer bounds: sizing and the solver pilot six hours, the protocol solve 48 hours.
function Get-VerificationDefaultJobTimeout([string]$Name) {
    switch ($Name) {
        { $_ -in @('wp9-sizing', 'wp9-sizing-serial', 'wp9-solve-pilot') } { return 21600 }
        'wp9-solve' { return 172800 }
        'wp9-diagnostic' { return 28800 }
        default { return 7200 }
    }
}
if (-not $PSBoundParameters.ContainsKey('JobTimeoutSeconds')) { $JobTimeoutSeconds = Get-VerificationDefaultJobTimeout $Profile }
$script:Wp9SolveProfiles = @('wp9-solve-pilot', 'wp9-solve')
$script:Wp9LinuxProfiles = @('wp9-sizing', 'wp9-sizing-serial', 'wp9-solve-pilot', 'wp9-solve', 'wp9-diagnostic')
# Profiles whose worker runs the read-only memory sampler.
$script:Wp9SampledProfiles = @('wp9-solve-pilot', 'wp9-solve', 'wp9-diagnostic')
$script:Wp9DiagnosticVariants = @('smoke', 'c0', 'd1', 'r0', 'r1', 'd4')
$script:VerificationScripts = Split-Path $PSScriptRoot -Parent
$script:VerificationRepo = [IO.Path]::GetFullPath((Split-Path $script:VerificationScripts -Parent))
# The fixed sizing environment is this checkout's own script, seen from WSL (C:\x -> /mnt/c/x).
function Get-VerificationWp9SizingEnvironmentFile {
    $path = [IO.Path]::GetFullPath((Join-Path $script:VerificationScripts 'cfd/wp9-replication-sizing-env.sh'))
    if ($path -notmatch '^([A-Za-z]):\\(.+)$') { throw "The checkout must be on a local drive letter: $path" }
    '/mnt/' + $Matches[1].ToLowerInvariant() + '/' + $Matches[2].Replace('\', '/')
}
# The fixed solve environment, also this checkout's own script seen from WSL.
function Get-VerificationWp9SolveEnvironmentFile {
    $path = [IO.Path]::GetFullPath((Join-Path $script:VerificationScripts 'cfd/wp9-replication-solve-env.sh'))
    if ($path -notmatch '^([A-Za-z]):\\(.+)$') { throw "The checkout must be on a local drive letter: $path" }
    '/mnt/' + $Matches[1].ToLowerInvariant() + '/' + $Matches[2].Replace('\', '/')
}
# The fixture environment of the turbulence diagnostics (tiny case, counter compile).
function Get-VerificationWp9FixtureEnvironmentFile {
    $path = [IO.Path]::GetFullPath((Join-Path $script:VerificationScripts 'cfd/wp9-diagnostic-fixture-env.sh'))
    if ($path -notmatch '^([A-Za-z]):\\(.+)$') { throw "The checkout must be on a local drive letter: $path" }
    '/mnt/' + $Matches[1].ToLowerInvariant() + '/' + $Matches[2].Replace('\', '/')
}
$script:VerificationRoot = Join-Path $script:VerificationRepo 'scratch/continuation'
# Diagnostics read only the stopped Stage 2 case (its 16-way 12M mesh and fields).
function Get-VerificationWp9DiagnosticSource {
    [IO.Path]::GetFullPath((Join-Path $script:VerificationRoot 'runs/wp9-solve-fastback-20261108-a/temp/vernon-cfd-98196-cfd.solve_steady-8/native/case'))
}
# R0, R1 and D4 restart from the C0 diagnostic's written iteration 500 (same mesh).
function Get-VerificationWp9DiagnosticStartSource([string]$Variant) {
    if ($Variant -cnotin @('r0', 'r1', 'd4')) { return $null }
    [IO.Path]::GetFullPath((Join-Path $script:VerificationRoot 'runs/wp9-diag-c0-20261108-a/temp/vernon-wp9-diag-c0-1891740/case'))
}
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
# Status files are replaced atomically (temp file, then move). A reader that
# meets the replacement gets a sharing violation for a moment; it retries for
# up to five seconds and then fails with the original error.
function Read-VerificationJson([string]$Path) {
    $deadline = [DateTime]::UtcNow.AddSeconds(5)
    while ($true) {
        try { return [IO.File]::ReadAllText($Path) | ConvertFrom-Json }
        catch {
            $cause = $_.Exception
            while ($cause.InnerException) { $cause = $cause.InnerException }
            if ($cause -isnot [IO.IOException] -or $cause -is [IO.FileNotFoundException] -or $cause -is [IO.DirectoryNotFoundException] -or [DateTime]::UtcNow -ge $deadline) { throw }
            Start-Sleep -Milliseconds 50
        }
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
function Test-VerificationProcess($ProcessId, $StartedUtc) {
    # ConvertFrom-Json (PowerShell 7) reads ISO stamps back as DateTime; compare the same 'o' text.
    if ($StartedUtc -is [DateTime]) { $StartedUtc = $StartedUtc.ToUniversalTime().ToString('o') }
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
    foreach ($name in @('Cargo.toml', 'Cargo.lock', 'AGENTS.md', 'CLEAN_ROOM.md', 'docs/research/wp9-replication-baseline-v1.json')) {
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
function Assert-VerificationWp9Input([string]$Name, [string]$Distribution = $env:VERNON_WSL_DISTRO, [string]$User = $env:VERNON_WSL_USER, [string]$EnvironmentFile = $env:VERNON_WSL_ENV_FILE, [string]$CarDirectory = $env:VERNON_CAR_DIR, [string]$Variant = $env:VERNON_DIAG_VARIANT) {
    if ($Name -eq 'wp9-diagnostic') {
        if ($Distribution -cne 'Vernon-Ubuntu-24.04' -or $User -cne 'vernon' -or $EnvironmentFile -cne (Get-VerificationWp9SolveEnvironmentFile)) {
            throw 'Diagnostics require the approved Vernon-Ubuntu-24.04 distribution, vernon user and fixed wp9-replication-solve-env.sh environment.'
        }
        if ($Variant -cnotin $script:Wp9DiagnosticVariants) { throw "VERNON_DIAG_VARIANT must be one of: $($script:Wp9DiagnosticVariants -join ', ')." }
    }
    if ($Name -in @('wp9-geometry', 'wp9-sizing', 'wp9-sizing-serial', 'wp9-solve-pilot', 'wp9-solve') -and [string]::IsNullOrWhiteSpace($CarDirectory)) { throw 'Replication checks require explicit VERNON_CAR_DIR.' }
    if ($Name -in $script:Wp9SolveProfiles -and
        ($Distribution -cne 'Vernon-Ubuntu-24.04' -or $User -cne 'vernon' -or
         $EnvironmentFile -cne (Get-VerificationWp9SolveEnvironmentFile))) {
        throw 'Solve profiles require the approved Vernon-Ubuntu-24.04 distribution, vernon user and fixed wp9-replication-solve-env.sh environment.'
    }
    if ($Name -in @('wp9-sizing', 'wp9-sizing-serial') -and
        ($Distribution -cne 'Vernon-Ubuntu-24.04' -or $User -cne 'vernon' -or
         $EnvironmentFile -cne (Get-VerificationWp9SizingEnvironmentFile))) {
        throw 'Sizing requires the approved Vernon-Ubuntu-24.04 distribution, vernon user and fixed wp9-replication-sizing-env.sh environment.'
    }
    if ($Name -in @('wp9-wsl-runner', 'wp9-native') -and
        ([string]::IsNullOrWhiteSpace($Distribution) -or [string]::IsNullOrWhiteSpace($User) -or
         [string]::IsNullOrWhiteSpace($EnvironmentFile) -or -not $EnvironmentFile.StartsWith('/'))) {
        throw 'WP9 real runner checks require explicit VERNON_WSL_DISTRO, VERNON_WSL_USER and an absolute Linux VERNON_WSL_ENV_FILE.'
    }
}
function Assert-VerificationResumeInput([string]$Name, [string]$PreviousRun) {
    if ($Name -in @('wp9-wsl-runner', 'wp9-native', 'wp9-geometry', 'wp9-sizing', 'wp9-sizing-serial', 'wp9-solve-pilot', 'wp9-solve', 'wp9-diagnostic') -and $PreviousRun) {
        throw 'External/data checks must execute afresh; prior results cannot be reused.'
    }
}
function Assert-VerificationJobTimeout([string]$Name, [int]$Seconds) {
    if ($Seconds -lt 1) { throw 'The outer deadline must be positive.' }
    if ($Name -in @('wp9-sizing', 'wp9-sizing-serial') -and $Seconds -gt 21600) { throw 'Sizing permits a positive outer deadline of at most 21600 seconds (six hours).' }
    if ($Name -eq 'wp9-solve-pilot' -and $Seconds -gt 21600) { throw 'The solver pilot permits an outer deadline of at most 21600 seconds (six hours).' }
    if ($Name -eq 'wp9-solve' -and $Seconds -gt 172800) { throw 'The protocol solve permits an outer deadline of at most 172800 seconds (48 hours).' }
    if ($Name -eq 'wp9-diagnostic' -and $Seconds -gt 28800) { throw 'A diagnostic permits an outer deadline of at most 28800 seconds (eight hours).' }
    if ($Name -notin $script:Wp9SolveProfiles -and $Seconds -gt 86400) { throw 'Only the solve profiles may run longer than one day.' }
}
function Set-VerificationWp9SizingEnvironment([string]$OutputDirectory, [ValidateSet(1,2)][int]$Processes = 2) {
    $outputPath = Join-Path $OutputDirectory 'replication-sizing.json'
    Assert-VerificationOutputPath $outputPath
    $fixed = @{
        VERNON_WSL_DISTRO = 'Vernon-Ubuntu-24.04'; VERNON_WSL_USER = 'vernon'
        VERNON_WSL_ENV_FILE = Get-VerificationWp9SizingEnvironmentFile
        VERNON_CAR_REAR = 'Fastback'; VERNON_CAR_DX0 = '0.32'; VERNON_CAR_NP = [string]$Processes
        VERNON_CAR_ITER = '7000'; VERNON_CAR_MESH_ONLY = '1'; VERNON_CAR_OUT = $outputPath
        VERNON_CAR_MESH_NP = [string]$Processes; VERNON_CAR_PILOT = '0'
        VERNON_CAR_MESH_TIMEOUT_MIN = '300'; VERNON_CAR_SOLVE_TIMEOUT_MIN = '2700'
    }
    foreach ($key in $fixed.Keys) { [Environment]::SetEnvironmentVariable($key, $fixed[$key], 'Process') }
}
# Fixed settings of the two solve profiles. Node limits leave at least an hour of
# the outer bound (Get-VerificationDefaultJobTimeout) for Linux cleanup.
function Get-VerificationWp9SolveSettings([ValidateSet('pilot', 'protocol')][string]$Mode) {
    if ($Mode -eq 'pilot') {
        return [ordered]@{ VERNON_CAR_ITER = 200; VERNON_CAR_PILOT = 1; VERNON_CAR_MESH_TIMEOUT_MIN = 100; VERNON_CAR_SOLVE_TIMEOUT_MIN = 180 }
    }
    return [ordered]@{ VERNON_CAR_ITER = 7000; VERNON_CAR_PILOT = 0; VERNON_CAR_MESH_TIMEOUT_MIN = 100; VERNON_CAR_SOLVE_TIMEOUT_MIN = 2700 }
}
function Get-VerificationWp9SolveMode([string]$Name) { if ($Name -eq 'wp9-solve-pilot') { 'pilot' } else { 'protocol' } }
function Set-VerificationWp9SolveEnvironment([string]$OutputDirectory, [ValidateSet('pilot', 'protocol')][string]$Mode) {
    $outputPath = Join-Path $OutputDirectory 'replication-solve.json'
    Assert-VerificationOutputPath $outputPath
    $fixed = [ordered]@{
        VERNON_WSL_DISTRO = 'Vernon-Ubuntu-24.04'; VERNON_WSL_USER = 'vernon'
        VERNON_WSL_ENV_FILE = Get-VerificationWp9SolveEnvironmentFile
        VERNON_CAR_REAR = 'Fastback'; VERNON_CAR_DX0 = '0.32'; VERNON_CAR_NP = '16'; VERNON_CAR_MESH_NP = '1'
        VERNON_CAR_MESH_ONLY = '0'; VERNON_CAR_OUT = $outputPath
    }
    $settings = Get-VerificationWp9SolveSettings $Mode
    foreach ($key in $settings.Keys) { $fixed[$key] = [string]$settings[$key] }
    foreach ($key in $fixed.Keys) { [Environment]::SetEnvironmentVariable($key, $fixed[$key], 'Process') }
}
# Fixed settings of a turbulence diagnostic; the variant is the only choice.
function Set-VerificationWp9DiagnosticEnvironment([string]$OutputDirectory, [ValidateSet('smoke', 'c0', 'd1', 'r0', 'r1', 'd4')][string]$Variant) {
    $outputPath = Join-Path $OutputDirectory 'diagnostic.json'
    Assert-VerificationOutputPath $outputPath
    $fixed = [ordered]@{
        VERNON_WSL_DISTRO = 'Vernon-Ubuntu-24.04'; VERNON_WSL_USER = 'vernon'
        VERNON_WSL_ENV_FILE = Get-VerificationWp9SolveEnvironmentFile
        VERNON_DIAG_FIXTURE_ENV = Get-VerificationWp9FixtureEnvironmentFile
        VERNON_DIAG_SOURCE = Get-VerificationWp9DiagnosticSource
        VERNON_DIAG_OUT = $outputPath; VERNON_DIAG_VARIANT = $Variant
        VERNON_DIAG_START_SOURCE = Get-VerificationWp9DiagnosticStartSource $Variant
    }
    # A null value removes the variable ([NullString]; a PowerShell $null would set '').
    foreach ($key in $fixed.Keys) {
        $value = if ($null -eq $fixed[$key]) { [NullString]::Value } else { $fixed[$key] }
        [Environment]::SetEnvironmentVariable($key, $value, 'Process')
    }
}
# A diagnostic completes with one executed test, one marker of its variant and a
# matching row. It is screening evidence: never acceptance, never coefficients.
function Test-VerificationWp9DiagnosticOutput([string]$Output, [string]$ErrorOutput = '', [string]$RowPath = '', [string]$Variant = '') {
    $shape = @{ smoke = @(4, 2); c0 = @(1000, 1); d1 = @(1000, 2); r0 = @(510, 1); r1 = @(1000, 1); d4 = @(1000, 1) }
    if (-not $shape.ContainsKey($Variant)) { return $false }
    $combined = $Output + "`n" + $ErrorOutput
    if ($combined -match '(?im)^\s*SKIPPED\b' -or
        $Output -notmatch '(?m)^running 1 test\s*$' -or
        $Output -notmatch '(?m)^test diagnostic::tests::wp9_turbulence_diagnostic \.\.\.' -or
        $Output -notmatch '(?m)^test result: ok\. 1 passed; 0 failed; 0 ignored;') { return $false }
    $iterations = $shape[$Variant][0]; $phases = $shape[$Variant][1]
    $pattern = '(?m)^WP9_DIAGNOSTIC_COMPLETE variant=' + $Variant + ' iterations=' + $iterations + ' phases=' + $phases + ' np=16 instrumentation=checked acceptance_evidence=0\s*$'
    if ([regex]::Matches($combined, $pattern).Count -ne 1 -or [regex]::Matches($combined, '(?m)^WP9_[A-Z_]+_COMPLETE').Count -ne 1) { return $false }
    try {
        Assert-VerificationOutputPath $RowPath
        $row = [IO.File]::ReadAllText($RowPath) | ConvertFrom-Json -ErrorAction Stop
        if ($row -isnot [pscustomobject] -or $row.scope -cne 'turbulence_diagnostic' -or $row.variant -cne $Variant) { return $false }
        if ($row.diagnostic_only -isnot [bool] -or -not $row.diagnostic_only) { return $false }
        foreach ($key in @('acceptance_evidence', 'physical_validation')) { if ($row.$key -isnot [bool] -or $row.$key) { return $false } }
        $names = $row.PSObject.Properties.Name
        foreach ($forbidden in @('cd', 'cl', 'converged', 'benchmark_validated', 'result', 'coefficients_reported')) { if ($forbidden -in $names) { return $false } }
        if ([double]$row.np -ne 16 -or [double]$row.iterations -ne $iterations -or @($row.phases).Count -ne $phases) { return $false }
        return $true
    } catch { return $false }
}
# A stop request is a file in the run directory; the worker polls for it.
function Request-VerificationCancel([string]$RunDirectory, [string]$Reason = '') {
    $path = Join-Path $RunDirectory 'cancel.request'
    Assert-VerificationOutputPath $path
    Write-VerificationJson $path ([ordered]@{ requestedUtc = [DateTime]::UtcNow.ToString('o'); reason = $Reason; requestedBy = [Environment]::UserName })
}
function Test-VerificationCancelRequested([string]$RunDirectory) { Test-Path -LiteralPath (Join-Path $RunDirectory 'cancel.request') }
function Test-VerificationWp9RunnerOutput([string]$Output, [string]$ErrorOutput = '') {
    $combined = $Output + "`n" + $ErrorOutput
    if ($combined -match '(?im)^\s*SKIPPED\b' -or
        $Output -notmatch '(?m)^test result: ok\. ([1-9][0-9]*) passed; 0 failed; 0 ignored;' -or [int]$Matches[1] -lt 7) { return $false }
    foreach ($name in @(
        'wsl_regression_success_does_not_leave_a_background_group',
        'wsl_regression_cancelled_start_remains_blocked_after_case_replacement',
        'wsl_environment_with_installed_runtime_supports_simplefoam_help',
        'wsl_environment_may_change_directory_and_positional_parameters',
        'wsl_cancellation_before_record_publication_prevents_tool_start',
        'wsl_timeout_stops_an_active_linux_grandchild',
        'wsl_cook_cancellation_stops_children_and_does_not_cancel_cleanup'
    )) {
        if ($Output -notmatch ('(?m)^test extproc::tests::' + [regex]::Escape($name) + ' \.\.\. ok\s*$')) { return $false }
    }
    return $true
}
function Test-VerificationWp9NativeOutput([string]$Output, [string]$ErrorOutput = '') {
    $combined = $Output + "`n" + $ErrorOutput
    return (Test-VerificationBenchmarkOutput $Output) -and
        $combined -cmatch '(?m)^WP9_NATIVE_CHAIN_COMPLETE(?:\s|$)' -and
        $Output -match '(?m)^test tests::real_openfoam_chain_on_a_small_car \.\.\.' -and
        $combined -notmatch '(?im)^\s*SKIPPED\b'
}
function Test-VerificationWp9GeometryOutput([string]$Output, [string]$ErrorOutput = '') {
    $combined = $Output + "`n" + $ErrorOutput
    return (Test-VerificationBenchmarkOutput $Output) -and
        $combined -cmatch '(?m)^WP9_REPLICATION_GEOMETRY_COMPLETE cases=3 external_programs=0\s*$' -and
        $Output -match '(?m)^test tests::replication_geometry_preparation \.\.\.' -and
        $combined -notmatch '(?im)^\s*SKIPPED\b'
}
function Test-VerificationWp9SizingOutput([string]$Output, [string]$ErrorOutput = '', [string]$RowPath = '', [int]$ExpectedProcesses = 2) {
    if ($ExpectedProcesses -notin @(1,2)) { return $false }
    $combined = $Output + "`n" + $ErrorOutput
    if ($combined -match '(?im)^\s*SKIPPED\b' -or
        $Output -notmatch '(?m)^running 1 test\s*$' -or
        $Output -notmatch '(?m)^test tests::real_openfoam_road_car_run \.\.\.' -or
        $Output -notmatch '(?m)^test result: ok\. 1 passed; 0 failed; 0 ignored;' -or
        $Output -match '(?m)^test result: (?:FAILED|ok\. (?:[2-9]|[1-9][0-9]+) passed;)') { return $false }
    $markers = [regex]::Matches($combined, '(?m)^WP9_REPLICATION_MESH_ONLY_COMPLETE rear=Fastback cells=([0-9]+(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?) solver_runs=0\s*$')
    if ($markers.Count -ne 1) { return $false }
    $markerCells = 0.0
    if (-not [double]::TryParse($markers[0].Groups[1].Value, [Globalization.NumberStyles]::Float, [Globalization.CultureInfo]::InvariantCulture, [ref]$markerCells) -or
        -not [double]::IsFinite($markerCells) -or $markerCells -le 0) { return $false }
    try {
        Assert-VerificationOutputPath $RowPath
        $row = [IO.File]::ReadAllText($RowPath) | ConvertFrom-Json -ErrorAction Stop
        if ($row -isnot [pscustomobject] -or $row.profile -cne 'road_car_detailed_gs_16ms_v1' -or
            $row.scope -cne 'published_replication' -or $row.rear -cne 'Fastback' -or
            'result' -in $row.PSObject.Properties.Name) { return $false }
        foreach ($key in @('original_experiment_authenticated', 'physical_validation', 'solver_ran')) {
            if ($row.$key -isnot [bool] -or $row.$key) { return $false }
        }
        if ($row.mesh_only -isnot [bool] -or -not $row.mesh_only) { return $false }
        $numbers = @{ dx0_m = 0.32; cells = $markerCells; np = $ExpectedProcesses; n_iter = 7000; window = 1000; write_interval = 1000; max_global_cells_m = 14.0; ram_gb = 15.52 }
        foreach ($key in $numbers.Keys) {
            $value = $row.$key
            if ($value -isnot [ValueType] -or $value -is [bool] -or -not [double]::IsFinite([double]$value) -or [double]$value -ne $numbers[$key]) { return $false }
        }
        return $true
    } catch { return $false }
}
function Test-VerificationWp9SolveOutput([string]$Output, [string]$ErrorOutput = '', [string]$RowPath = '', [string]$Mode = 'protocol') {
    if ($Mode -notin @('pilot', 'protocol')) { return $false }
    $combined = $Output + "`n" + $ErrorOutput
    if ($combined -match '(?im)^\s*SKIPPED\b' -or
        $Output -notmatch '(?m)^running 1 test\s*$' -or
        $Output -notmatch '(?m)^test tests::real_openfoam_road_car_run \.\.\.' -or
        $Output -notmatch '(?m)^test result: ok\. 1 passed; 0 failed; 0 ignored;') { return $false }
    $number = '([0-9]+(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?)'
    $pilotPattern = '(?m)^WP9_SOLVE_PILOT_COMPLETE rear=Fastback cells=' + $number + ' iterations=([0-9]+) np=([0-9]+) mesh_np=([0-9]+) seconds_per_iteration=' + $number + ' coefficients_reported=0\s*$'
    $protocolPattern = '(?m)^WP9_REPLICATION_SOLVE_COMPLETE rear=Fastback cells=' + $number + ' iterations=([0-9]+) np=([0-9]+) mesh_np=([0-9]+) converged=(true|false) benchmark_validated=(true|false) cd=(-?[0-9]+(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?)\s*$'
    $pilotMarkers = [regex]::Matches($combined, $pilotPattern)
    $protocolMarkers = [regex]::Matches($combined, $protocolPattern)
    $markers = if ($Mode -eq 'pilot') { $pilotMarkers } else { $protocolMarkers }
    if ($markers.Count -ne 1 -or ($pilotMarkers.Count + $protocolMarkers.Count) -ne 1) { return $false }
    if ($combined -match '(?m)^WP9_(?:SOLVE_PILOT|REPLICATION_SOLVE|REPLICATION_MESH_ONLY)_COMPLETE' -and ([regex]::Matches($combined, '(?m)^WP9_[A-Z_]+_COMPLETE').Count -ne 1)) { return $false }
    $marker = $markers[0]
    $parse = {
        param([string]$Text)
        $value = 0.0
        if (-not [double]::TryParse($Text, [Globalization.NumberStyles]::Float, [Globalization.CultureInfo]::InvariantCulture, [ref]$value) -or -not [double]::IsFinite($value)) { return $null }
        return $value
    }
    $isNumber = { param($Value) $Value -is [ValueType] -and $Value -isnot [bool] -and [double]::IsFinite([double]$Value) }
    try {
        Assert-VerificationOutputPath $RowPath
        $row = [IO.File]::ReadAllText($RowPath) | ConvertFrom-Json -ErrorAction Stop
        if ($row -isnot [pscustomobject] -or $row.profile -cne 'road_car_detailed_gs_16ms_v1' -or $row.rear -cne 'Fastback') { return $false }
        $names = $row.PSObject.Properties.Name
        foreach ($key in @('physical_validation', 'original_experiment_authenticated', 'mesh_only')) {
            if ($row.$key -isnot [bool] -or $row.$key) { return $false }
        }
        if ($row.solver_ran -isnot [bool] -or -not $row.solver_ran -or $row.pilot -isnot [bool]) { return $false }
        $cells = & $parse $marker.Groups[1].Value
        $settings = Get-VerificationWp9SolveSettings $Mode
        $numbers = @{ dx0_m = 0.32; cells = $cells; np = 16; mesh_np = 1; n_iter = $settings.VERNON_CAR_ITER }
        foreach ($key in $numbers.Keys) {
            if (-not (& $isNumber $row.$key) -or [double]$row.$key -ne $numbers[$key]) { return $false }
        }
        if ($cells -le 0 -or [int]$marker.Groups[2].Value -ne $settings.VERNON_CAR_ITER -or [int]$marker.Groups[3].Value -ne 16 -or [int]$marker.Groups[4].Value -ne 1) { return $false }
        if ($Mode -eq 'pilot') {
            if (-not $row.pilot -or $row.scope -cne 'solver_pilot_diagnostic' -or $row.diagnostic_only -isnot [bool] -or -not $row.diagnostic_only -or
                $row.coefficients_reported -isnot [bool] -or $row.coefficients_reported) { return $false }
            foreach ($forbidden in @('result', 'cd', 'cl', 'converged', 'benchmark_validated', 'within_band', 'coefficient_band_ok', 'convergence')) {
                if ($forbidden -in $names) { return $false }
            }
            if (-not (& $isNumber $row.iterations_read) -or [double]$row.iterations_read -ne $settings.VERNON_CAR_ITER) { return $false }
            $solver = $row.solve_diagnostics.solver
            if ($solver.ended -isnot [bool] -or -not $solver.ended -or [double]$solver.iterations -ne $settings.VERNON_CAR_ITER) { return $false }
            $rate = & $parse $marker.Groups[5].Value
            if ($null -eq $rate -or $rate -le 0 -or -not (& $isNumber $solver.seconds_per_iteration) -or [double]$solver.seconds_per_iteration -ne $rate) { return $false }
            return $true
        }
        if ($row.pilot -or $row.scope -cne 'published_replication' -or $row.coefficients_reported -isnot [bool] -or -not $row.coefficients_reported) { return $false }
        foreach ($key in @('window', 'write_interval')) {
            if (-not (& $isNumber $row.$key) -or [double]$row.$key -ne 1000) { return $false }
        }
        if ($row.converged -isnot [bool] -or $row.benchmark_validated -isnot [bool] -or -not (& $isNumber $row.cd)) { return $false }
        if ($marker.Groups[5].Value -cne $row.converged.ToString().ToLowerInvariant() -or
            $marker.Groups[6].Value -cne $row.benchmark_validated.ToString().ToLowerInvariant()) { return $false }
        $cd = & $parse $marker.Groups[7].Value
        if ($null -eq $cd -or $cd -ne [double]$row.cd) { return $false }
        return $true
    } catch { return $false }
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
        'wp9-focused' { @{name='wp9-tests';executable='cargo';arguments=@('test','-p','vernon-nodes-cfd')} }
        'wp9-perf' { @{name='wp9-perf-tests';executable='cargo';arguments=@('test','-p','vernon-nodes-perf')} }
        'wp9-native' { @{name='wp9-native';executable='cargo';arguments=@('test','-p','vernon-nodes-cfd','tests::real_openfoam_chain_on_a_small_car','--','--exact','--ignored','--nocapture','--test-threads=1')} }
        'wp9-geometry' { @{name='wp9-geometry';executable='cargo';arguments=@('test','--release','-p','vernon-nodes-cfd','tests::replication_geometry_preparation','--','--exact','--ignored','--nocapture','--test-threads=1')} }
        'wp9-sizing-serial' { Get-VerificationSteps 'wp9-sizing' $OutputDirectory }
        'wp9-sizing' { @{name='wp9-sizing';executable='cargo';arguments=@('test','--release','-p','vernon-nodes-cfd','tests::real_openfoam_road_car_run','--','--exact','--ignored','--nocapture','--test-threads=1')} }
        'wp9-diagnostic' { @{name='wp9-diagnostic';executable='cargo';arguments=@('test','--release','-p','vernon-nodes-cfd','diagnostic::tests::wp9_turbulence_diagnostic','--','--exact','--ignored','--nocapture','--test-threads=1')} }
        { $_ -in @('wp9-solve-pilot', 'wp9-solve') } { @{name='wp9-solve';executable='cargo';arguments=@('test','--release','-p','vernon-nodes-cfd','tests::real_openfoam_road_car_run','--','--exact','--ignored','--nocapture','--test-threads=1')} }
        'wp9-wsl-runner' {
            @{name='wp9-extproc';executable='cargo';arguments=@('test','-p','vernon-nodes-cfd','extproc::tests','--','--nocapture','--test-threads=1')}
            @{name='wp9-wsl';executable='cargo';arguments=@('test','-p','vernon-nodes-cfd','extproc::tests::wsl_','--','--ignored','--nocapture','--test-threads=1')}
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
                $kind = if ($Name -eq 'smoke-slow') { 'wait' } elseif ($Name -eq 'smoke-orphan') { 'orphan' } elseif ($Name -eq 'smoke-cancel') { 'hold' } else { 'pass' }
                @{ name = $(if ($kind -eq 'pass') { 'second' } else { $kind }); executable = $shell; arguments = @('-NoProfile', '-NonInteractive', '-File', $worker, '-SmokeChild', $kind) }
            }
        }
    }
}
if ($Library) { return }
if ($SmokeChild -ne 'none') {
    if ($SmokeChild -eq 'wait') { Start-Sleep -Seconds 6 }
    if ($SmokeChild -eq 'hold') { Start-Sleep -Seconds 120 }
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
    $inputLease = $null
    try {
        if ($StepName -eq 'wp9-diagnostic') {
            Assert-VerificationWp9Input $Profile
            Set-VerificationWp9DiagnosticEnvironment $stepDirectory $env:VERNON_DIAG_VARIANT
        }
        if ($StepName -in @('wp9-geometry', 'wp9-sizing', 'wp9-sizing-serial', 'wp9-solve')) {
            Assert-VerificationWp9Input $Profile
            . (Join-Path $PSScriptRoot 'replication-inputs.ps1')
            $contract = Get-Content -Raw -LiteralPath (Join-Path $script:VerificationRepo 'docs/research/wp9-replication-baseline-v1.json') | ConvertFrom-Json
            if (@($contract.geometry.components).Count -ne 8) { throw 'Replication checks need the eight recorded original components.' }
            $inputLease = Open-ReplicationInputLease $env:VERNON_CAR_DIR $contract.geometry.components
            Write-VerificationJson (Join-Path $stepDirectory 'verified-components.json') $inputLease.receipts
            if ($StepName -eq 'wp9-sizing') {
                $ranks = if ($Profile -eq 'wp9-sizing-serial') { 1 } else { 2 }
                Set-VerificationWp9SizingEnvironment $stepDirectory $ranks
            }
            elseif ($StepName -eq 'wp9-solve') { Set-VerificationWp9SolveEnvironment $stepDirectory (Get-VerificationWp9SolveMode $Profile) }
            else { $env:VERNON_REPLICATION_GEOMETRY_OUT = Join-Path $stepDirectory 'replication-geometry.json' }
        }
        & $selected[0].executable @nativeArguments
        $nativeExit = $LASTEXITCODE
    } finally {
        if ($inputLease) { Close-ReplicationInputLease $inputLease }
    }
    exit $nativeExit
}
$runDirectory = Get-VerificationPath $RunId
$statePath = Join-Path $runDirectory 'status.json'
$lockPath = Join-Path $script:VerificationRoot 'worker.lock'
$lock = $null
$state = $null
$child = $null
$processJob = $null
$sampler = $null
$samplerJob = $null
$resultCode = 1
try {
    Assert-VerificationWp8Input $Profile
    Assert-VerificationWp9Input $Profile
    Assert-VerificationResumeInput $Profile $ResumeRunId
    Assert-VerificationJobTimeout $Profile $JobTimeoutSeconds
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
    . (Join-Path $script:VerificationScripts 'dev-env.ps1')
    $env:TEMP = Join-Path $runDirectory 'temp'
    $env:TMP = $env:TEMP
    $env:CARGO_INCREMENTAL = '0'
    $env:CARGO_TARGET_DIR = Join-Path $script:VerificationRepo 'target'
    Assert-VerificationOutputPath $env:CARGO_TARGET_DIR
    if ($Profile -in @('full', 'clean-focused', 'wp6-benchmark', 'wp6', 'wp8-real', 'wp8', 'wp9-focused', 'wp9-perf', 'wp9-wsl-runner', 'wp9-native', 'wp9-geometry', 'wp9-sizing', 'wp9-sizing-serial', 'wp9-solve-pilot', 'wp9-solve', 'wp9-diagnostic') -and (Test-Path -LiteralPath $env:CARGO_TARGET_DIR)) {
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
    if ($Profile -eq 'wp9-diagnostic') { $state['diagnosticVariant'] = $env:VERNON_DIAG_VARIANT; $state['diagnosticSource'] = Get-VerificationWp9DiagnosticSource; $state['diagnosticStartSource'] = Get-VerificationWp9DiagnosticStartSource $env:VERNON_DIAG_VARIANT }
    Write-VerificationJson $statePath $state
    Write-VerificationJson (Join-Path $script:VerificationRoot 'latest.json') @{ runId = $RunId; statusPath = $statePath }
    $reuse = @()
    if ($ResumeRunId) {
        $previousPath = Join-Path (Get-VerificationPath $ResumeRunId) 'status.json'
        $previous = Get-Content -Raw -LiteralPath $previousPath | ConvertFrom-Json
        if ($previous.profile -ne $Profile -or $previous.runnerVersion -ne 2 -or $previous.sourceBefore -ne $source) { throw 'Resume requires the same profile, runner version and source fingerprint.' }
        if ($previous.status -in @('failed', 'timed-out', 'source-changed', 'cancelled', 'cleanup-failed')) { throw "Cannot resume a $($previous.status) run. Inspect logs and explicitly start a new run." }
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
        if ($Profile -in $script:Wp9SampledProfiles -and -not $sampler) {
            # Read-only memory and disk sampler, contained in its own kill-on-close job.
            $samplerInfo = [Diagnostics.ProcessStartInfo]::new((Get-Process -Id $PID).Path)
            $samplerInfo.UseShellExecute = $false; $samplerInfo.CreateNoWindow = $true
            foreach ($argument in @('-NoProfile', '-NonInteractive', '-File', (Join-Path $script:VerificationScripts 'cfd/wp9-memory-sampler.ps1'), '-RunDirectory', $runDirectory, '-Distribution', 'Vernon-Ubuntu-24.04', '-User', 'vernon', '-ParentProcessId', "$PID")) { $samplerInfo.ArgumentList.Add($argument) }
            $samplerJob = New-VerificationProcessJob
            $sampler = [Diagnostics.Process]::Start($samplerInfo)
            $samplerJob.Attach($sampler)
            $state['memorySampler'] = [ordered]@{ processId = $sampler.Id; samples = (Join-Path $runDirectory 'memory-samples.jsonl'); summary = (Join-Path $runDirectory 'memory-summary.json') }
        }
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
                if (Test-VerificationCancelRequested $runDirectory) {
                    $processJob.Terminate()
                    if (-not $child.WaitForExit(5000)) { throw 'Native process did not exit within the five-second cancellation window.' }
                    $state.status = 'cancelled'; $resultCode = 130
                    $state['cancelRequest'] = Get-Content -Raw -LiteralPath (Join-Path $runDirectory 'cancel.request') | ConvertFrom-Json
                    break
                }
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
        $record.exitCode = if ($state.status -eq 'timed-out') { 124 } elseif ($state.status -eq 'cancelled') { 130 } else { $child.ExitCode }
        if ($leftoverDescendants -gt 0 -and $record.exitCode -eq 0) {
            $record.exitCode = 125
            $state.message = 'Unexpected native descendants remained after the step exited. The Job Object killed them; verification stopped.'
        }
        if ($step.name -eq 'wp8-real' -and $record.exitCode -eq 0 -and
            -not (Test-VerificationWp8Output ([IO.File]::ReadAllText($stdoutPath)) ([IO.File]::ReadAllText($stderrPath)))) {
            $record.exitCode = 4
            $state.message = 'WP8 external acceptance needs an executed passing test and WP8_REAL_COMPLETE marker without any SKIPPED diagnostic.'
        }
        if ($step.name -in @('wp9-tests', 'wp9-perf-tests', 'wp9-extproc') -and $record.exitCode -eq 0 -and -not (Test-VerificationBenchmarkOutput ([IO.File]::ReadAllText($stdoutPath)))) {
            $record.exitCode = 4
            $state.message = 'The WP9 filter must execute passing tests; a zero-test run is not verification.'
        }
        if ($step.name -eq 'wp9-wsl' -and $record.exitCode -eq 0 -and
            -not (Test-VerificationWp9RunnerOutput ([IO.File]::ReadAllText($stdoutPath)) ([IO.File]::ReadAllText($stderrPath)))) {
            $record.exitCode = 4
            $state.message = 'All seven actual WSL runner regressions must pass without skips; help alone is not CFD acceptance.'
        }
        if ($step.name -eq 'wp9-native' -and $record.exitCode -eq 0 -and
            -not (Test-VerificationWp9NativeOutput ([IO.File]::ReadAllText($stdoutPath)) ([IO.File]::ReadAllText($stderrPath)))) {
            $record.exitCode = 4
            $state.message = 'The actual native node-chain test and its final evidence assertions must execute without skips; this is capability, not automotive validation.'
        }
        if ($step.name -eq 'wp9-geometry' -and $record.exitCode -eq 0 -and
            -not (Test-VerificationWp9GeometryOutput ([IO.File]::ReadAllText($stdoutPath)) ([IO.File]::ReadAllText($stderrPath)))) {
            $record.exitCode = 4
            $state.message = 'All three geometry preparations must execute and reach their final marker without skips; this is not meshing or CFD acceptance.'
        }
        if ($step.name -eq 'wp9-sizing' -and $record.exitCode -eq 0 -and
            -not (Test-VerificationWp9SizingOutput ([IO.File]::ReadAllText($stdoutPath)) ([IO.File]::ReadAllText($stderrPath)) (Join-Path $runDirectory 'replication-sizing.json') $(if ($Profile -eq 'wp9-sizing-serial') { 1 } else { 2 }))) {
            $record.exitCode = 4
            $state.message = 'Sizing requires one executed passing test, positive-cell Fastback mesh-only completion and a matching saved fixed-profile row; physical validation remains false.'
        }
        if ($step.name -eq 'wp9-solve' -and $record.exitCode -eq 0 -and
            -not (Test-VerificationWp9SolveOutput ([IO.File]::ReadAllText($stdoutPath)) ([IO.File]::ReadAllText($stderrPath)) (Join-Path $runDirectory 'replication-solve.json') (Get-VerificationWp9SolveMode $Profile))) {
            $record.exitCode = 4
            $state.message = 'A solve profile requires one executed passing test, exactly one final marker of its own kind and a matching saved fixed-profile row. A pilot never reports coefficients; physical validation remains false.'
        }
        if ($step.name -eq 'wp9-diagnostic' -and $record.exitCode -eq 0 -and
            -not (Test-VerificationWp9DiagnosticOutput ([IO.File]::ReadAllText($stdoutPath)) ([IO.File]::ReadAllText($stderrPath)) (Join-Path $runDirectory 'diagnostic.json') $state.diagnosticVariant)) {
            $record.exitCode = 4
            $state.message = 'A diagnostic requires one executed passing test, exactly one marker of its variant with checked instrumentation and a matching saved row. It is never acceptance evidence.'
        }
        if ($step.name -eq 'wp6-release' -and $record.exitCode -eq 0 -and -not (Test-VerificationBenchmarkOutput ([IO.File]::ReadAllText($stdoutPath)))) {
            $record.exitCode = 4
            $state.message = 'The WP6 benchmark filter did not execute a passing test. A zero-test run cannot verify the benchmark.'
        }
        $record.status = if ($state.status -eq 'cancelled') { 'cancelled' } elseif ($record.exitCode -eq 0) { 'passed' } else { 'failed' }
        $record.finishedUtc = [DateTime]::UtcNow.ToString('o')
        $state.childPid = $null; $state.childStartedUtc = $null; $state.activeStep = $null
        $currentSource = Get-VerificationFingerprint
        $state.sourceAfter = $currentSource
        if ($currentSource -ne $source) { $state.status = 'source-changed'; $resultCode = 3 }
        elseif ($record.exitCode -ne 0 -and $state.status -notin @('timed-out', 'cancelled')) { $state.status = 'failed'; $resultCode = $record.exitCode }
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
    if ($sampler) {
        try {
            [IO.File]::WriteAllText((Join-Path $runDirectory 'memory-sampler.stop'), 'stop')
            if (-not $sampler.WaitForExit(90000)) { $sampler.Kill($true) }
        } catch { }
        if ($samplerJob) { $samplerJob.Dispose() }
        $summaryPath = Join-Path $runDirectory 'memory-summary.json'
        if ($state -and (Test-Path -LiteralPath $summaryPath)) { $state['memorySummary'] = Get-Content -Raw -LiteralPath $summaryPath | ConvertFrom-Json }
    }
    if ($state) {
        if ($Profile -in $script:Wp9LinuxProfiles) {
            $state['statusBeforeCleanup'] = $state.status
            try {
                . (Join-Path $script:VerificationScripts 'cfd/wp9-sizing-cleanup.ps1')
                $cleanup = Stop-Wp9SizingLinuxGroups -RunDirectory $runDirectory -Distribution 'Vernon-Ubuntu-24.04' -User 'vernon'
                if ($cleanup.success -isnot [bool] -or -not $cleanup.success) { throw 'Linux cleanup did not confirm success.' }
                $state['linuxCleanup'] = $cleanup
            } catch {
                $state['linuxCleanup'] = [ordered]@{ success = $false; message = $_.Exception.Message }
                $state.message = ($state.message + ' Linux cleanup unconfirmed: ' + $_.Exception.Message).Trim()
                $state.status = 'cleanup-failed'; $resultCode = 126
            }
        }
        $state.exitCode = $resultCode
        $state.finishedUtc = [DateTime]::UtcNow.ToString('o')
        $state.activeStep = $null; $state.childPid = $null; $state.childStartedUtc = $null
        Write-VerificationJson $statePath $state
        Write-Output "$($state.status): $RunId; exit $resultCode; $statePath"
    }
    if ($lock) { $lock.Dispose() }
}
exit $resultCode
