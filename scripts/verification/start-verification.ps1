#requires -Version 7.2
<#
Starts one bounded worker in a hidden window and returns. It can continue after
this launcher and the chat turn end, while the machine remains awake. Check
verification-status.ps1 later. A sandbox may terminate child processes when a
tool session ends; confirm persistence in an independent shell on your host.
#>
[CmdletBinding()]
param(
    [ValidateSet('full', 'clean-focused', 'wp6-benchmark', 'wp6', 'wp8-real', 'wp8', 'wp9-focused', 'wp9-perf', 'wp9-wsl-runner', 'wp9-native', 'wp9-geometry', 'wp9-sizing', 'wp9-sizing-serial', 'wp9-solve-pilot', 'wp9-solve', 'wp9-diagnostic', 'smoke', 'smoke-failure', 'smoke-slow', 'smoke-orphan', 'smoke-cancel')][string]$Profile = 'full',
    [ValidatePattern('^[A-Za-z0-9][A-Za-z0-9_-]{0,95}$')][string]$RunId = ('run-' + [DateTime]::UtcNow.ToString('yyyyMMdd-HHmmss') + '-' + [guid]::NewGuid().ToString('N').Substring(0, 8)),
    [ValidatePattern('^$|^[A-Za-z0-9][A-Za-z0-9_-]{0,95}$')][string]$ResumeRunId = '',
    [ValidateRange(1, 172800)][int]$JobTimeoutSeconds = 7200
)
$ErrorActionPreference = 'Stop'
$worker = Join-Path $PSScriptRoot 'verification.ps1'
. $worker -Library -Profile $Profile -RunId $RunId -ResumeRunId $ResumeRunId -JobTimeoutSeconds $JobTimeoutSeconds
if (-not $PSBoundParameters.ContainsKey('JobTimeoutSeconds')) { $JobTimeoutSeconds = Get-VerificationDefaultJobTimeout $Profile }
Assert-VerificationWp8Input $Profile
Assert-VerificationWp9Input $Profile
Assert-VerificationResumeInput $Profile $ResumeRunId
Assert-VerificationJobTimeout $Profile $JobTimeoutSeconds
$directory = Get-VerificationPath $RunId
Assert-VerificationOutputPath $directory
if (Test-Path -LiteralPath $directory) { throw "Run id already exists: $RunId" }
$launchDirectory = Join-Path $script:VerificationRoot 'launches'
Assert-VerificationOutputPath $launchDirectory
[void][IO.Directory]::CreateDirectory($launchDirectory)
$shell = (Get-Process -Id $PID).Path
$arguments = @('-NoProfile', '-NonInteractive', '-File', ('"' + $worker + '"'), '-Profile', $Profile, '-RunId', $RunId, '-JobTimeoutSeconds', "$JobTimeoutSeconds")
if ($ResumeRunId) { $arguments += @('-ResumeRunId', $ResumeRunId) }
# Avoid PowerShell's asynchronous stream redirection here: its event handlers
# can keep the launch shell waiting for the worker. Step output is redirected
# by the worker itself, and early worker errors have a separate launch log.
$process = Start-Process -FilePath $shell -ArgumentList $arguments -WorkingDirectory $script:VerificationRepo -WindowStyle Hidden -PassThru
$statusPath = Join-Path $directory 'status.json'
$deadline = [DateTime]::UtcNow.AddSeconds(30)
do {
    if (Test-Path -LiteralPath $statusPath) {
        $initial = Read-VerificationJson $statusPath
        if ($initial.status -eq 'failed') { throw "Worker failed: $($initial.message). See $statusPath" }
        Write-Output "Launched $RunId (PID $($process.Id)); status: $statusPath"
        exit 0
    }
    if ($process.HasExited) { throw "Worker exited $($process.ExitCode) before creating status. See $launchDirectory/$RunId.stderr.log" }
    Start-Sleep -Milliseconds 100
} while ([DateTime]::UtcNow -lt $deadline)
throw "Worker PID $($process.Id) has not published status. Inspect $launchDirectory/$RunId.stderr.log before launching again."
