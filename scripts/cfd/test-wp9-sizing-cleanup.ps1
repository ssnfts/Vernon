#requires -Version 7.2
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Split-Path (Split-Path $PSScriptRoot -Parent) -Parent))
$helper = Join-Path $PSScriptRoot 'wp9-sizing-cleanup.ps1'
$passed = 0
function Assert-CleanupTest([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw "ASSERTION FAILED: $Message" }
    $script:passed++
}
function Assert-CleanupRejected([scriptblock]$Action, [string]$Message) {
    $rejected = $false
    try { & $Action | Out-Null } catch { $rejected = $true }
    Assert-CleanupTest $rejected $Message
}
Assert-CleanupTest (Test-Path -LiteralPath $helper -PathType Leaf) 'sizing cleanup implementation must exist'
. $helper
foreach ($name in @('Stop-Wp9SizingLinuxGroups', 'Get-Wp9SizingCleanupPlan', 'ConvertFrom-Wp9SizingControlRecord', 'ConvertTo-Wp9SizingLinuxPath')) {
    Assert-CleanupTest ([bool](Get-Command $name -CommandType Function -ErrorAction SilentlyContinue)) "required cleanup function $name exists"
}
$token = 'vernon-cfd-wsl-4242-1a2b3c-0'
$fileName = ".$token.pid"
$record = "$token 321 987654 321`n"
$parsed = ConvertFrom-Wp9SizingControlRecord -FileName $fileName -RecordText $record
Assert-CleanupTest ($parsed.Token -ceq $token -and $parsed.ProcessId -eq 321 -and $parsed.StartTicks -eq '987654' -and $parsed.GroupId -eq 321) 'valid exact attempt record yields structured identity'
foreach ($badRecord in @(
    "vernon-cfd-wsl-4242-1a2b3c-1 321 987654 321`n",
    "$($token.ToUpperInvariant()) 321 987654 321`n",
    "$token 321 987654 322`n", "$token 0 987654 0`n", "$token -1 987654 -1`n",
    "$token 2147483648 987654 2147483648`n", "$token 321 notticks 321`n",
    "$token 321 18446744073709551616 321`n", "$token 321 987654 321 extra`n",
    "$token 321 987654 321`n$token 321 987654 321`n", "$token 321 987654 321`0", ''
)) {
    Assert-CleanupRejected { ConvertFrom-Wp9SizingControlRecord -FileName $fileName -RecordText $badRecord } 'malformed or mismatched identity is rejected'
}
foreach ($badName in @('worker.pid', ".$token.PID", ".$token.pid.tmp", '.vernon-cfd-wsl-4242-xyz-0.pid', ".$token.pid:stream")) {
    Assert-CleanupRejected { ConvertFrom-Wp9SizingControlRecord -FileName $badName -RecordText $record } 'only the exact extproc published filename is accepted by parser'
}
Assert-CleanupTest ((ConvertTo-Wp9SizingLinuxPath 'C:\Users\example\vernon\scratch\space here\a') -ceq '/mnt/c/Users/example/vernon/scratch/space here/a') 'drive mapping preserves literal path components and spaces'
foreach ($badPath in @('relative\a', 'C:relative', '\\server\share\a', '\\?\C:\a', 'C:\a\..\b', 'C:\a\b:stream', "C:\a`n\b", 'C:\a.\b')) {
    Assert-CleanupRejected { ConvertTo-Wp9SizingLinuxPath $badPath } 'ambiguous or nonlocal path is rejected before WSL planning'
}

# Fixtures remain under a unique owned run for diagnostics. No WSL/process/tool
# invocation occurs in this suite, and no pre-existing fixture is deleted.
$fixturePrefix = 'cleanup-parser-' + [guid]::NewGuid().ToString('N')
$runs = Join-Path $repo 'scratch/continuation/runs'
function New-CleanupFixture([string]$Suffix) {
    $run = Join-Path $runs ($fixturePrefix + '-' + $Suffix)
    [void][IO.Directory]::CreateDirectory((Join-Path $run 'temp'))
    return $run
}
function Write-CleanupFixture([string]$Run, [string]$Relative, [string]$Text) {
    $path = Join-Path $Run ('temp/' + $Relative)
    [void][IO.Directory]::CreateDirectory((Split-Path $path -Parent))
    [IO.File]::WriteAllText($path, $Text, [Text.UTF8Encoding]::new($false))
    return $path
}
function Get-CleanupFixturePlan([string]$Run) {
    @(Get-Wp9SizingCleanupPlan -RunDirectory $Run -Distribution 'Vernon-Ubuntu-24.04' -User 'vernon')
}
$empty = New-CleanupFixture 'empty'
Assert-CleanupTest (@(Get-CleanupFixturePlan $empty).Count -eq 0) 'an empty owned temp subtree has no group targets'
$valid = New-CleanupFixture 'valid'
$recordPath = Write-CleanupFixture $valid "work/.wsl-control/$fileName" $record
[void](Write-CleanupFixture $valid "work/.wsl-control/.$token.cancel" '')
[void](Write-CleanupFixture $valid 'work/mesh/points' 'unrelated case output')
$plan = @(Get-CleanupFixturePlan $valid)
Assert-CleanupTest ($plan.Count -eq 1 -and $plan[0].Token -ceq $token) 'published record and tombstone deduplicate to one exact attempt'
Assert-CleanupTest ($plan[0].RecordPath -ceq $recordPath -and $plan[0].LinuxRecordPath.EndsWith("/work/.wsl-control/$fileName", [StringComparison]::Ordinal)) 'plan carries the exact ordinary record path and literal Linux mapping'
Assert-CleanupTest ($plan[0].LinuxCancelPath.EndsWith("/work/.wsl-control/.$token.cancel", [StringComparison]::Ordinal)) 'cancel path is derived from the fixed token filename'
Assert-CleanupTest (-not (Test-Path -LiteralPath (Join-Path $valid "temp/work/.wsl-control/.$token.pid.tmp"))) 'planning performs no control writes'
$unrelated = Join-Path $valid $fileName
[IO.File]::WriteAllText($unrelated, $record)
Assert-CleanupTest (@(Get-CleanupFixturePlan $valid).Count -eq 1) 'record outside temp is never included'
$pending = New-CleanupFixture 'pending'
[void](Write-CleanupFixture $pending "work/.wsl-control/.$token.pid.tmp" $record)
Assert-CleanupTest (@(Get-CleanupFixturePlan $pending).Count -eq 1) 'an unpublished ordinary record is planned so delayed starts can be cancelled'
$cancelled = New-CleanupFixture 'cancelled'
[void](Write-CleanupFixture $cancelled "work/.wsl-control/.$token.cancel" '')
Assert-CleanupTest (@(Get-CleanupFixturePlan $cancelled).Count -eq 1) 'retained ordinary tombstone can verify an already ended attempt'
$intent = New-CleanupFixture 'intent'
[void](Write-CleanupFixture $intent "work/.wsl-control/.$token.intent" "$token`n")
Assert-CleanupTest (@(Get-CleanupFixturePlan $intent).Count -eq 1) 'durable prelaunch intent identifies a delayed unpublished attempt'
$badIntent = New-CleanupFixture 'bad-intent'
[void](Write-CleanupFixture $badIntent "work/.wsl-control/.$token.intent" "different-token`n")
Assert-CleanupRejected { Get-CleanupFixturePlan $badIntent } 'prelaunch intent must exactly match its fixed filename token'
$noTemp = Join-Path $runs ($fixturePrefix + '-no-temp')
[void][IO.Directory]::CreateDirectory($noTemp)
Assert-CleanupTest (@(Get-CleanupFixturePlan $noTemp).Count -eq 0) 'missing temp before test startup has no targets'
$badIdentity = New-CleanupFixture 'bad-identity'
[void](Write-CleanupFixture $badIdentity "work/.wsl-control/$fileName" "$token 321 987654 322`n")
Assert-CleanupRejected { Get-CleanupFixturePlan $badIdentity } 'a malformed owned record fails closed before any WSL execution'
$oversize = New-CleanupFixture 'oversize'
[void](Write-CleanupFixture $oversize "work/.wsl-control/$fileName" ('x' * 4097))
Assert-CleanupRejected { Get-CleanupFixturePlan $oversize } 'control record read budget is enforced'
$nested = New-CleanupFixture 'depth'
[void](Write-CleanupFixture $nested ((('d/' * 17) + $fileName)) $record)
Assert-CleanupRejected { Get-CleanupFixturePlan $nested } 'temp traversal depth is bounded'
foreach ($wrongScope in @($runs, (Join-Path $runs '..'), (Join-Path $valid 'temp'), (Join-Path $repo 'scratch/continuation/runs-elsewhere/one'))) {
    Assert-CleanupRejected { Get-CleanupFixturePlan $wrongScope } 'cleanup requires one exact owned run directory'
}
foreach ($identity in @(@('Ubuntu', 'vernon'), @('Vernon-Ubuntu-24.04', 'root'), @('vernon-ubuntu-24.04', 'vernon'))) {
    Assert-CleanupRejected { Get-Wp9SizingCleanupPlan -RunDirectory $empty -Distribution $identity[0] -User $identity[1] } 'only the approved literal distribution and user are accepted'
}
$reparse = New-CleanupFixture 'reparse'
$linkPath = Join-Path $reparse 'temp/linked'
[void](New-Item -ItemType Junction -Path $linkPath -Target (Join-Path $empty 'temp'))
Assert-CleanupRejected { Get-CleanupFixturePlan $reparse } 'reparse directories are rejected rather than traversed'
$missing = Join-Path $runs ($fixturePrefix + '-absent')
Assert-CleanupRejected { Get-CleanupFixturePlan $missing } 'missing run directory cannot silently count as verified cleanup'
Write-Output "PASS: sizing cleanup pure parser/path-planning checks=$passed fixture=$fixturePrefix WSL=0 external_programs=0"
