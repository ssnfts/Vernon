[CmdletBinding()]
param([switch]$LibraryChecksOnly, [switch]$OrphanOnly, [switch]$TimeoutOnly, [string]$HoldJsonPath = '')
$ErrorActionPreference = 'Stop'
$repo = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
$shell = (Get-Process -Id $PID).Path
$worker = Join-Path $PSScriptRoot 'verification.ps1'
$launcher = Join-Path $PSScriptRoot 'start-verification.ps1'
$statusScript = Join-Path $PSScriptRoot 'verification-status.ps1'
function Assert-True($Condition, [string]$Message) {
    if (-not $Condition) { throw "ASSERTION FAILED: $Message" }
}
function Read-State([string]$Id) {
    # The worker rewrites status.json while this test polls it; a read that
    # meets the write's exclusive open is retried briefly, never ignored.
    Read-VerificationJson (Join-Path $repo "scratch/continuation/runs/$Id/status.json")
}
function Wait-State([string]$Id, [string]$Step = '', [int]$Seconds = 20) {
    $deadline = [DateTime]::UtcNow.AddSeconds($Seconds)
    do {
        $path = Join-Path $repo "scratch/continuation/runs/$Id/status.json"
        if (Test-Path -LiteralPath $path) {
            $state = Read-State $Id
            if ($Step -and $state.activeStep -eq $Step -and $state.childPid) { return $state }
            # The worker publishes a terminal status before cleanup; exit code
            # and finish time arrive only with its final write.
            if (-not $Step -and $state.status -notin @('starting', 'running') -and $state.finishedUtc) { return $state }
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
$wp8Passed = "running 1 test`ntest result: ok. 1 passed; 0 failed;"
Assert-True (Test-VerificationWp8Output $wp8Passed 'WP8_REAL_COMPLETE registered_images=20') 'real acceptance needs a completed full pipeline'
Assert-True (-not (Test-VerificationWp8Output $wp8Passed '')) 'passing skipped test cannot certify real execution'
Assert-True (-not (Test-VerificationWp8Output $wp8Passed "WP8_REAL_COMPLETE`nSKIPPED external acceptance")) 'skip overrides completion marker'
Assert-True (-not (Test-VerificationWp8Output 'test result: ok. 0 passed; 0 failed;' 'WP8_REAL_COMPLETE')) 'zero tests cannot certify external acceptance'
Assert-True (-not (Test-VerificationWp8Output $wp8Passed 'incidental WP8_REAL_COMPLETE')) 'incidental marker mention is not evidence'
$wp9RunnerSteps = @(Get-VerificationSteps 'wp9-wsl-runner')
Assert-True (($wp9RunnerSteps.name -join ',') -eq 'wp9-extproc,wp9-wsl') 'WP9 runner profile executes default and real WSL tests'
$geometrySteps = @(Get-VerificationSteps 'wp9-geometry')
Assert-True ($geometrySteps.Count -eq 1 -and ($geometrySteps[0].arguments -join ' ') -match '--release.*tests::replication_geometry_preparation.*--exact --ignored') 'geometry profile runs its explicit release data check'
$geometryPassed = "running 1 test`ntest tests::replication_geometry_preparation ... ok`ntest result: ok. 1 passed; 0 failed; 0 ignored;"
Assert-True (Test-VerificationWp9GeometryOutput $geometryPassed 'WP9_REPLICATION_GEOMETRY_COMPLETE cases=3 external_programs=0') 'all geometry outputs must reach final assertions'
Assert-True (-not (Test-VerificationWp9GeometryOutput $geometryPassed '')) 'an ignored test that skipped data cannot pass'
Assert-True (-not (Test-VerificationWp9GeometryOutput $geometryPassed 'WP9_REPLICATION_GEOMETRY_COMPLETE cases=2 external_programs=0')) 'partial case inventory cannot pass'
Assert-True (-not (Test-VerificationWp9GeometryOutput $geometryPassed "WP9_REPLICATION_GEOMETRY_COMPLETE cases=3 external_programs=0`nSKIPPED data")) 'skip overrides geometry completion'
Assert-True (-not (Test-VerificationWp9GeometryOutput 'running 0 tests' 'WP9_REPLICATION_GEOMETRY_COMPLETE cases=3 external_programs=0')) 'zero geometry tests cannot pass'
$sizingSteps = @(Get-VerificationSteps 'wp9-sizing')
$sizingArguments = @('test', '--release', '-p', 'vernon-nodes-cfd', 'tests::real_openfoam_road_car_run', '--', '--exact', '--ignored', '--nocapture', '--test-threads=1')
Assert-True ($sizingSteps.Count -eq 1 -and $sizingSteps[0].name -eq 'wp9-sizing' -and $sizingSteps[0].executable -eq 'cargo' -and ($sizingSteps[0].arguments -join "`0") -ceq ($sizingArguments -join "`0")) 'sizing runs only its exact ignored release road-car test'
foreach ($scriptPath in @($worker, $launcher)) {
    $profileAttribute = (Get-Command $scriptPath).Parameters['Profile'].Attributes | Where-Object { $_ -is [System.Management.Automation.ValidateSetAttribute] }
    Assert-True ('wp9-sizing' -in $profileAttribute.ValidValues) 'worker and launcher explicitly accept the fixed sizing profile'
}
$sizingDistribution = 'Vernon-Ubuntu-24.04'
$sizingUser = 'vernon'
$sizingEnvironment = Get-VerificationWp9SizingEnvironmentFile
Assert-True ($sizingEnvironment -cmatch '^/mnt/[a-z]/.+/scripts/cfd/wp9-replication-sizing-env\.sh$') 'the sizing environment is this checkout''s script as seen from WSL'
Assert-VerificationWp9Input 'wp9-sizing' $sizingDistribution $sizingUser $sizingEnvironment 'C:\approved-cars'
foreach ($inputCase in @(
    @('', $sizingUser, $sizingEnvironment, 'C:\approved-cars'),
    @('Ubuntu', $sizingUser, $sizingEnvironment, 'C:\approved-cars'),
    @($sizingDistribution, 'root', $sizingEnvironment, 'C:\approved-cars'),
    @($sizingDistribution, $sizingUser, '/tmp/environment.sh', 'C:\approved-cars'),
    @($sizingDistribution, $sizingUser, $sizingEnvironment, ' ')
)) {
    $rejected = $false
    try { Assert-VerificationWp9Input 'wp9-sizing' $inputCase[0] $inputCase[1] $inputCase[2] $inputCase[3] } catch { $rejected = $true }
    Assert-True $rejected 'sizing refuses unapproved WSL inputs and absent component data'
}
Assert-VerificationJobTimeout 'wp9-sizing' 21600
Assert-VerificationJobTimeout 'wp9-sizing' 3
$rejected = $false
try { Assert-VerificationJobTimeout 'wp9-sizing' 21601 } catch { $rejected = $true }
Assert-True $rejected 'sizing cannot exceed its six-hour outer deadline'
foreach ($externalProfile in @('wp9-wsl-runner', 'wp9-native', 'wp9-geometry', 'wp9-sizing')) {
    $rejected = $false
    try { Assert-VerificationResumeInput $externalProfile 'previous-run' } catch { $rejected = $true }
    Assert-True $rejected 'external and sizing profiles always execute afresh'
}
$sizingDirectory = Join-Path $repo ('scratch/continuation/library-checks/sizing-' + [guid]::NewGuid().ToString('N'))
Assert-VerificationOutputPath $sizingDirectory
[void][IO.Directory]::CreateDirectory($sizingDirectory)
$sizingPath = Join-Path $sizingDirectory 'replication-sizing.json'
$fixedSizingEnvironment = [ordered]@{
    VERNON_WSL_DISTRO = $sizingDistribution; VERNON_WSL_USER = $sizingUser; VERNON_WSL_ENV_FILE = $sizingEnvironment
    VERNON_CAR_REAR = 'Fastback'; VERNON_CAR_DX0 = '0.32'; VERNON_CAR_NP = '2'; VERNON_CAR_ITER = '7000'; VERNON_CAR_MESH_ONLY = '1'; VERNON_CAR_OUT = $sizingPath
}
$previousSizingEnvironment = @{}
try {
    foreach ($key in $fixedSizingEnvironment.Keys) {
        $previousSizingEnvironment[$key] = [Environment]::GetEnvironmentVariable($key, 'Process')
        [Environment]::SetEnvironmentVariable($key, 'inherited-conflict', 'Process')
    }
    Set-VerificationWp9SizingEnvironment $sizingDirectory
    foreach ($key in $fixedSizingEnvironment.Keys) {
        Assert-True ([Environment]::GetEnvironmentVariable($key, 'Process') -ceq $fixedSizingEnvironment[$key]) "fixed sizing overrides inherited $key"
    }
} finally {
    foreach ($key in $previousSizingEnvironment.Keys) { [Environment]::SetEnvironmentVariable($key, $previousSizingEnvironment[$key], 'Process') }
}
$sizingPassed = "running 1 test`ntest tests::real_openfoam_road_car_run ... ok`ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 362 filtered out;"
$sizingMarker = 'WP9_REPLICATION_MESH_ONLY_COMPLETE rear=Fastback cells=12500000.0 solver_runs=0'
$validSizingRow = [ordered]@{
    rear = 'Fastback'; dx0_m = 0.32; cells = 12500000.0; profile = 'road_car_detailed_gs_16ms_v1'; scope = 'published_replication'
    original_experiment_authenticated = $false; physical_validation = $false; mesh_only = $true; solver_ran = $false
    np = 2; n_iter = 7000; window = 1000; write_interval = 1000; max_global_cells_m = 14.0; ram_gb = 15.52
}
Write-VerificationJson $sizingPath $validSizingRow
Assert-True (Test-VerificationWp9SizingOutput $sizingPassed $sizingMarker $sizingPath) 'sizing accepts an executed mesh-only test with its reloaded fixed-profile row'
foreach ($badOutput in @(
    'running 0 tests',
    $sizingPassed.Replace('1 passed; 0 failed; 0 ignored;', '0 passed; 0 failed; 1 ignored;'),
    $sizingPassed.Replace('1 passed; 0 failed; 0 ignored;', '2 passed; 0 failed; 0 ignored;'),
    $sizingPassed.Replace('tests::real_openfoam_road_car_run', 'tests::some_other_test')
)) {
    Assert-True (-not (Test-VerificationWp9SizingOutput $badOutput $sizingMarker $sizingPath)) 'zero, ignored, extra or unrelated tests cannot qualify sizing'
}
foreach ($badMarker in @('', ('incidental ' + $sizingMarker), $sizingMarker.Replace('Fastback', 'Estate'), $sizingMarker.Replace('12500000.0', '0'), $sizingMarker.Replace('12500000.0', '-1'), $sizingMarker.Replace('solver_runs=0', 'solver_runs=1'), ($sizingMarker + "`nSKIPPED runtime"))) {
    Assert-True (-not (Test-VerificationWp9SizingOutput $sizingPassed $badMarker $sizingPath)) 'only positive-cell Fastback mesh completion without skips qualifies'
}
foreach ($badField in @(
    @('profile', 'unknown'), @('scope', 'original_experiment'), @('rear', 'Estate'), @('dx0_m', 0.64), @('np', 1), @('n_iter', 2000),
    @('window', 500), @('write_interval', 500), @('max_global_cells_m', 15.0), @('ram_gb', 32.0),
    @('original_experiment_authenticated', $true), @('physical_validation', $true), @('mesh_only', $false), @('solver_ran', $true),
    @('mesh_only', 'true'), @('solver_ran', 'false'), @('physical_validation', 'false'), @('cells', '12500000'), @('cells', 0), @('cells', 12499999)
)) {
    $badRow = [ordered]@{}
    foreach ($key in $validSizingRow.Keys) { $badRow[$key] = $validSizingRow[$key] }
    $badRow[$badField[0]] = $badField[1]
    Write-VerificationJson $sizingPath $badRow
    Assert-True (-not (Test-VerificationWp9SizingOutput $sizingPassed $sizingMarker $sizingPath)) "sizing reload rejects changed or mistyped $($badField[0])"
}
$rowWithSolverResult = [ordered]@{}
foreach ($key in $validSizingRow.Keys) { $rowWithSolverResult[$key] = $validSizingRow[$key] }
$rowWithSolverResult['result'] = @{ cd = 0.275 }
Write-VerificationJson $sizingPath $rowWithSolverResult
Assert-True (-not (Test-VerificationWp9SizingOutput $sizingPassed $sizingMarker $sizingPath)) 'a solver result cannot be presented as mesh-only sizing'
[IO.File]::WriteAllText($sizingPath, '{malformed')
Assert-True (-not (Test-VerificationWp9SizingOutput $sizingPassed $sizingMarker $sizingPath)) 'malformed saved sizing evidence fails closed'
Remove-Item -LiteralPath $sizingPath
Assert-True (-not (Test-VerificationWp9SizingOutput $sizingPassed $sizingMarker $sizingPath)) 'missing saved sizing evidence fails closed'
$serialSteps = @(Get-VerificationSteps 'wp9-sizing-serial')
Assert-True ($serialSteps.Count -eq 1 -and $serialSteps[0].name -eq 'wp9-sizing' -and ($serialSteps[0].arguments -join "`0") -ceq ($sizingArguments -join "`0")) 'serial sizing uses the same exact mesh-only test with separate fixed admission'
Assert-VerificationWp9Input 'wp9-sizing-serial' $sizingDistribution $sizingUser $sizingEnvironment 'C:\approved-cars'
foreach ($guard in @('resume', 'timeout', 'identity')) {
    $rejected = $false
    try {
        switch ($guard) {
            'resume' { Assert-VerificationResumeInput 'wp9-sizing-serial' 'old-run' }
            'timeout' { Assert-VerificationJobTimeout 'wp9-sizing-serial' 21601 }
            'identity' { Assert-VerificationWp9Input 'wp9-sizing-serial' 'other' $sizingUser $sizingEnvironment 'C:\approved-cars' }
        }
    } catch { $rejected = $true }
    Assert-True $rejected "serial sizing preserves $guard guard"
}
$validSizingRow.np = 1
Write-VerificationJson $sizingPath $validSizingRow
Assert-True (Test-VerificationWp9SizingOutput $sizingPassed $sizingMarker $sizingPath 1) 'serial completion requires an explicitly serial row'
Assert-True (-not (Test-VerificationWp9SizingOutput $sizingPassed $sizingMarker $sizingPath)) 'serial row never passes the parallel profile'
$serialSavedEnv = @{}
foreach ($key in @('VERNON_WSL_DISTRO','VERNON_WSL_USER','VERNON_WSL_ENV_FILE','VERNON_CAR_REAR','VERNON_CAR_DX0','VERNON_CAR_NP','VERNON_CAR_ITER','VERNON_CAR_MESH_ONLY','VERNON_CAR_OUT')) { $serialSavedEnv[$key] = [Environment]::GetEnvironmentVariable($key,'Process') }
$runModeKeys = @('VERNON_CAR_MESH_NP','VERNON_CAR_PILOT','VERNON_CAR_MESH_TIMEOUT_MIN','VERNON_CAR_SOLVE_TIMEOUT_MIN')
foreach ($key in $runModeKeys) { $serialSavedEnv[$key] = [Environment]::GetEnvironmentVariable($key,'Process') }
try {
    foreach ($key in $runModeKeys) { [Environment]::SetEnvironmentVariable($key, '99', 'Process') }
    Set-VerificationWp9SizingEnvironment $sizingDirectory 1
    Assert-True ($env:VERNON_CAR_NP -ceq '1' -and $env:VERNON_CAR_MESH_ONLY -ceq '1') 'serial profile fixes one process while keeping the solver disabled'
    Assert-True ($env:VERNON_CAR_MESH_NP -ceq '1' -and $env:VERNON_CAR_PILOT -ceq '0' -and $env:VERNON_CAR_MESH_TIMEOUT_MIN -ceq '300') 'sizing fixes its mesh rank count, pilot flag and mesh limit over inherited values'
    Set-VerificationWp9SizingEnvironment $sizingDirectory 2
    Assert-True ($env:VERNON_CAR_MESH_NP -ceq '2') 'two-rank sizing still meshes on two ranks'
} finally { foreach ($key in $serialSavedEnv.Keys) { [Environment]::SetEnvironmentVariable($key,$serialSavedEnv[$key],'Process') } }
Remove-Item -LiteralPath $sizingPath

# ---- WP9 solve profiles: a diagnostic pilot and the protocol run.
foreach ($solveProfile in @('wp9-solve-pilot', 'wp9-solve')) {
    $solveSteps = @(Get-VerificationSteps $solveProfile)
    Assert-True ($solveSteps.Count -eq 1 -and $solveSteps[0].name -eq 'wp9-solve' -and $solveSteps[0].executable -eq 'cargo' -and ($solveSteps[0].arguments -join "`0") -ceq ($sizingArguments -join "`0")) "$solveProfile runs only the exact ignored release road-car test"
    foreach ($scriptPath in @($worker, $launcher)) {
        $profileAttribute = (Get-Command $scriptPath).Parameters['Profile'].Attributes | Where-Object { $_ -is [System.Management.Automation.ValidateSetAttribute] }
        Assert-True ($solveProfile -in $profileAttribute.ValidValues) "worker and launcher explicitly accept $solveProfile"
    }
    $rejected = $false
    try { Assert-VerificationResumeInput $solveProfile 'old-run' } catch { $rejected = $true }
    Assert-True $rejected "$solveProfile always executes afresh"
}
$solveEnvironment = Get-VerificationWp9SolveEnvironmentFile
Assert-True ($solveEnvironment -cmatch '^/mnt/[a-z]/.+/scripts/cfd/wp9-replication-solve-env\.sh$') 'the solve environment is this checkout''s script as seen from WSL'
Assert-VerificationWp9Input 'wp9-solve-pilot' $sizingDistribution $sizingUser $solveEnvironment 'C:\approved-cars'
Assert-VerificationWp9Input 'wp9-solve' $sizingDistribution $sizingUser $solveEnvironment 'C:\approved-cars'
foreach ($inputCase in @(
    @('wp9-solve', $sizingDistribution, $sizingUser, $sizingEnvironment, 'C:\approved-cars'),
    @('wp9-solve-pilot', $sizingDistribution, $sizingUser, $sizingEnvironment, 'C:\approved-cars'),
    @('wp9-sizing-serial', $sizingDistribution, $sizingUser, $solveEnvironment, 'C:\approved-cars'),
    @('wp9-solve', 'Ubuntu', $sizingUser, $solveEnvironment, 'C:\approved-cars'),
    @('wp9-solve', $sizingDistribution, 'root', $solveEnvironment, 'C:\approved-cars'),
    @('wp9-solve', $sizingDistribution, $sizingUser, $solveEnvironment, ' ')
)) {
    $rejected = $false
    try { Assert-VerificationWp9Input $inputCase[0] $inputCase[1] $inputCase[2] $inputCase[3] $inputCase[4] } catch { $rejected = $true }
    Assert-True $rejected "$($inputCase[0]) refuses another profile's environment, unapproved WSL identity or absent data"
}
Assert-True ((Get-VerificationDefaultJobTimeout 'wp9-solve-pilot') -eq 21600 -and (Get-VerificationDefaultJobTimeout 'wp9-solve') -eq 172800 -and (Get-VerificationDefaultJobTimeout 'wp9-sizing') -eq 21600 -and (Get-VerificationDefaultJobTimeout 'full') -eq 7200) 'each profile has its documented default outer bound'
Assert-VerificationJobTimeout 'wp9-solve-pilot' 21600
Assert-VerificationJobTimeout 'wp9-solve' 172800
foreach ($timeoutCase in @(@('wp9-solve-pilot', 21601), @('wp9-solve', 172801), @('full', 86401), @('wp9-sizing', 172800))) {
    $rejected = $false
    try { Assert-VerificationJobTimeout $timeoutCase[0] $timeoutCase[1] } catch { $rejected = $true }
    Assert-True $rejected "$($timeoutCase[0]) refuses an outer bound of $($timeoutCase[1]) s"
}
foreach ($mode in @('pilot', 'protocol')) {
    $settings = Get-VerificationWp9SolveSettings $mode
    $outer = Get-VerificationDefaultJobTimeout $(if ($mode -eq 'pilot') { 'wp9-solve-pilot' } else { 'wp9-solve' })
    Assert-True ((($settings.VERNON_CAR_MESH_TIMEOUT_MIN + $settings.VERNON_CAR_SOLVE_TIMEOUT_MIN) * 60 + 3600) -le $outer) "$mode node limits leave an hour of the outer bound for cleanup"
}
$solveDirectory = Join-Path $repo ('scratch/continuation/library-checks/solve-' + [guid]::NewGuid().ToString('N'))
Assert-VerificationOutputPath $solveDirectory
[void][IO.Directory]::CreateDirectory($solveDirectory)
$solvePath = Join-Path $solveDirectory 'replication-solve.json'
$solveKeys = @('VERNON_WSL_DISTRO','VERNON_WSL_USER','VERNON_WSL_ENV_FILE','VERNON_CAR_REAR','VERNON_CAR_DX0','VERNON_CAR_NP','VERNON_CAR_MESH_NP','VERNON_CAR_ITER','VERNON_CAR_PILOT','VERNON_CAR_MESH_ONLY','VERNON_CAR_OUT','VERNON_CAR_MESH_TIMEOUT_MIN','VERNON_CAR_SOLVE_TIMEOUT_MIN')
$solveSaved = @{}
foreach ($key in $solveKeys) { $solveSaved[$key] = [Environment]::GetEnvironmentVariable($key, 'Process') }
try {
    foreach ($key in $solveKeys) { [Environment]::SetEnvironmentVariable($key, 'inherited', 'Process') }
    Set-VerificationWp9SolveEnvironment $solveDirectory 'pilot'
    $expected = [ordered]@{ VERNON_WSL_DISTRO = $sizingDistribution; VERNON_WSL_USER = $sizingUser; VERNON_WSL_ENV_FILE = $solveEnvironment; VERNON_CAR_REAR = 'Fastback'; VERNON_CAR_DX0 = '0.32'; VERNON_CAR_NP = '16'; VERNON_CAR_MESH_NP = '1'; VERNON_CAR_ITER = '200'; VERNON_CAR_PILOT = '1'; VERNON_CAR_MESH_ONLY = '0'; VERNON_CAR_OUT = $solvePath; VERNON_CAR_MESH_TIMEOUT_MIN = '100'; VERNON_CAR_SOLVE_TIMEOUT_MIN = '180' }
    foreach ($key in $expected.Keys) { Assert-True ([Environment]::GetEnvironmentVariable($key, 'Process') -ceq $expected[$key]) "pilot fixes $key over an inherited value" }
    Set-VerificationWp9SolveEnvironment $solveDirectory 'protocol'
    Assert-True ($env:VERNON_CAR_ITER -ceq '7000' -and $env:VERNON_CAR_PILOT -ceq '0' -and $env:VERNON_CAR_NP -ceq '16' -and $env:VERNON_CAR_MESH_NP -ceq '1' -and $env:VERNON_CAR_SOLVE_TIMEOUT_MIN -ceq '2700' -and $env:VERNON_CAR_MESH_ONLY -ceq '0') 'protocol fixes 7000 iterations on 16 ranks with a serial mesh'
} finally { foreach ($key in $solveSaved.Keys) { [Environment]::SetEnvironmentVariable($key, $solveSaved[$key], 'Process') } }

$solvePassed = "running 1 test`ntest tests::real_openfoam_road_car_run ... ok`ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 374 filtered out;"
$pilotMarker = 'WP9_SOLVE_PILOT_COMPLETE rear=Fastback cells=12056197 iterations=200 np=16 mesh_np=1 seconds_per_iteration=14.2 coefficients_reported=0'
$validPilotRow = [ordered]@{
    rear = 'Fastback'; profile = 'road_car_detailed_gs_16ms_v1'; scope = 'solver_pilot_diagnostic'; dx0_m = 0.32; cells = 12056197.0
    np = 16; mesh_np = 1; n_iter = 200; iterations_read = 200; pilot = $true; mesh_only = $false; solver_ran = $true
    physical_validation = $false; original_experiment_authenticated = $false; diagnostic_only = $true; coefficients_reported = $false
    solve_diagnostics = [ordered]@{ solver = [ordered]@{ ended = $true; iterations = 200; seconds_per_iteration = 14.2 } }
}
Write-VerificationJson $solvePath $validPilotRow
Assert-True (Test-VerificationWp9SolveOutput $solvePassed $pilotMarker $solvePath 'pilot') 'pilot accepts one executed test, its final marker and a reloaded diagnostic row'
Assert-True (-not (Test-VerificationWp9SolveOutput $solvePassed $pilotMarker $solvePath 'protocol')) 'a pilot can never complete the protocol profile'
foreach ($badMarker in @('', ('incidental ' + $pilotMarker), $pilotMarker.Replace('coefficients_reported=0', 'coefficients_reported=1'), $pilotMarker.Replace('iterations=200', 'iterations=7000'), $pilotMarker.Replace('np=16', 'np=8'), $pilotMarker.Replace('cells=12056197', 'cells=12056198'), ($pilotMarker + "`n" + $pilotMarker), ($pilotMarker + "`nSKIPPED runtime"))) {
    Assert-True (-not (Test-VerificationWp9SolveOutput $solvePassed $badMarker $solvePath 'pilot')) 'only one exact pilot marker matching the row qualifies'
}
foreach ($badField in @(@('scope', 'published_replication'), @('pilot', $false), @('np', 8), @('mesh_np', 16), @('n_iter', 7000), @('iterations_read', 150), @('physical_validation', $true), @('coefficients_reported', $true), @('cd', 0.27), @('result', @{}), @('converged', $true))) {
    $badRow = [ordered]@{}
    foreach ($key in $validPilotRow.Keys) { $badRow[$key] = $validPilotRow[$key] }
    $badRow[$badField[0]] = $badField[1]
    Write-VerificationJson $solvePath $badRow
    Assert-True (-not (Test-VerificationWp9SolveOutput $solvePassed $pilotMarker $solvePath 'pilot')) "pilot reload rejects $($badField[0]) = $($badField[1])"
}
$protocolMarker = 'WP9_REPLICATION_SOLVE_COMPLETE rear=Fastback cells=12056197 iterations=7000 np=16 mesh_np=1 converged=false benchmark_validated=false cd=0.301'
$validProtocolRow = [ordered]@{
    rear = 'Fastback'; profile = 'road_car_detailed_gs_16ms_v1'; scope = 'published_replication'; dx0_m = 0.32; cells = 12056197.0
    np = 16; mesh_np = 1; n_iter = 7000; window = 1000; write_interval = 1000; pilot = $false; mesh_only = $false; solver_ran = $true
    physical_validation = $false; original_experiment_authenticated = $false; coefficients_reported = $true
    converged = $false; benchmark_validated = $false; cd = 0.301
}
Write-VerificationJson $solvePath $validProtocolRow
Assert-True (Test-VerificationWp9SolveOutput $solvePassed $protocolMarker $solvePath 'protocol') 'a completed protocol run that did not converge is still a recorded completion, reported as not converged'
Assert-True (-not (Test-VerificationWp9SolveOutput $solvePassed $protocolMarker $solvePath 'pilot')) 'a protocol row never completes the pilot profile'
foreach ($badMarker in @($protocolMarker.Replace('converged=false', 'converged=true'), $protocolMarker.Replace('benchmark_validated=false', 'benchmark_validated=true'), $protocolMarker.Replace('cd=0.301', 'cd=0.275'), $protocolMarker.Replace('iterations=7000', 'iterations=200'), $pilotMarker)) {
    Assert-True (-not (Test-VerificationWp9SolveOutput $solvePassed $badMarker $solvePath 'protocol')) 'the protocol marker must agree with the saved row'
}
foreach ($badField in @(@('converged', 'false'), @('benchmark_validated', $null), @('cd', 'NaN'), @('physical_validation', $true), @('pilot', $true), @('n_iter', 2000), @('window', 100), @('scope', 'solver_pilot_diagnostic'))) {
    $badRow = [ordered]@{}
    foreach ($key in $validProtocolRow.Keys) { $badRow[$key] = $validProtocolRow[$key] }
    $badRow[$badField[0]] = $badField[1]
    Write-VerificationJson $solvePath $badRow
    Assert-True (-not (Test-VerificationWp9SolveOutput $solvePassed $protocolMarker $solvePath 'protocol')) "protocol reload rejects $($badField[0]) = $($badField[1])"
}
foreach ($badOutput in @($solvePassed.Replace('1 passed; 0 failed; 0 ignored;', '0 passed; 0 failed; 1 ignored;'), $solvePassed.Replace('tests::real_openfoam_road_car_run', 'tests::some_other_test'))) {
    Assert-True (-not (Test-VerificationWp9SolveOutput $badOutput $protocolMarker $solvePath 'protocol')) 'zero, ignored or unrelated tests cannot complete a solve profile'
}
Remove-Item -LiteralPath $solvePath
Assert-True (-not (Test-VerificationWp9SolveOutput $solvePassed $protocolMarker $solvePath 'protocol')) 'a missing saved row fails closed'

# ---- WP9 turbulence diagnostic profile: screening runs, never acceptance.
$diagSteps = @(Get-VerificationSteps 'wp9-diagnostic')
Assert-True ($diagSteps.Count -eq 1 -and $diagSteps[0].name -eq 'wp9-diagnostic' -and $diagSteps[0].executable -eq 'cargo' -and ($diagSteps[0].arguments -join ' ') -ceq 'test --release -p vernon-nodes-cfd diagnostic::tests::wp9_turbulence_diagnostic -- --exact --ignored --nocapture --test-threads=1') 'the diagnostic profile runs only the exact ignored diagnostic test'
foreach ($scriptPath in @($worker, $launcher)) {
    $profileAttribute = (Get-Command $scriptPath).Parameters['Profile'].Attributes | Where-Object { $_ -is [System.Management.Automation.ValidateSetAttribute] }
    Assert-True ('wp9-diagnostic' -in $profileAttribute.ValidValues) 'worker and launcher explicitly accept wp9-diagnostic'
}
$rejected = $false
try { Assert-VerificationResumeInput 'wp9-diagnostic' 'old-run' } catch { $rejected = $true }
Assert-True $rejected 'wp9-diagnostic always executes afresh'
Assert-True ((Get-VerificationDefaultJobTimeout 'wp9-diagnostic') -eq 28800) 'a diagnostic has an eight-hour outer bound'
Assert-VerificationJobTimeout 'wp9-diagnostic' 28800
$rejected = $false
try { Assert-VerificationJobTimeout 'wp9-diagnostic' 28801 } catch { $rejected = $true }
Assert-True $rejected 'a diagnostic refuses an outer bound above eight hours'
Assert-True ('wp9-diagnostic' -in $script:Wp9LinuxProfiles -and 'wp9-diagnostic' -in $script:Wp9SampledProfiles) 'a diagnostic is sampled and its Linux processes are cleaned up'
$fixtureEnvironment = Get-VerificationWp9FixtureEnvironmentFile
Assert-True ($fixtureEnvironment -cmatch '^/mnt/[a-z]/.+/scripts/cfd/wp9-diagnostic-fixture-env\.sh$') 'the fixture environment is this checkout''s script as seen from WSL'
Assert-True ((Get-VerificationWp9DiagnosticSource) -like '*scratch*continuation*runs*wp9-solve-fastback-20261108-a*temp*vernon-cfd-98196-cfd.solve_steady-8*native*case') 'the diagnostic source is the stopped Stage 2 case'
foreach ($variant in @('smoke', 'c0', 'd1', 'r0', 'r1', 'd4', 'd5', 'm1')) { Assert-VerificationWp9Input 'wp9-diagnostic' $sizingDistribution $sizingUser $solveEnvironment '' $variant }
foreach ($inputCase in @(
    @($sizingDistribution, $sizingUser, $solveEnvironment, 'd2'),
    @($sizingDistribution, $sizingUser, $solveEnvironment, ''),
    @($sizingDistribution, $sizingUser, $sizingEnvironment, 'c0'),
    @('Ubuntu', $sizingUser, $solveEnvironment, 'c0'),
    @($sizingDistribution, 'root', $solveEnvironment, 'c0')
)) {
    $rejected = $false
    try { Assert-VerificationWp9Input 'wp9-diagnostic' $inputCase[0] $inputCase[1] $inputCase[2] '' $inputCase[3] } catch { $rejected = $true }
    Assert-True $rejected "wp9-diagnostic refuses variant '$($inputCase[3])' with $($inputCase[0])/$($inputCase[1])/$($inputCase[2])"
}
$diagDirectory = Join-Path $repo ('scratch/continuation/library-checks/diagnostic-' + [guid]::NewGuid().ToString('N'))
[void][IO.Directory]::CreateDirectory($diagDirectory)
$diagPath = Join-Path $diagDirectory 'diagnostic.json'
$diagKeys = @('VERNON_WSL_DISTRO','VERNON_WSL_USER','VERNON_WSL_ENV_FILE','VERNON_DIAG_FIXTURE_ENV','VERNON_DIAG_SOURCE','VERNON_DIAG_OUT','VERNON_DIAG_VARIANT','VERNON_DIAG_START_SOURCE')
$diagSaved = @{}
foreach ($key in $diagKeys) { $diagSaved[$key] = [Environment]::GetEnvironmentVariable($key, 'Process') }
try {
    foreach ($key in $diagKeys) { [Environment]::SetEnvironmentVariable($key, 'inherited', 'Process') }
    Set-VerificationWp9DiagnosticEnvironment $diagDirectory 'd1'
    $expected = [ordered]@{ VERNON_WSL_DISTRO = $sizingDistribution; VERNON_WSL_USER = $sizingUser; VERNON_WSL_ENV_FILE = $solveEnvironment; VERNON_DIAG_FIXTURE_ENV = $fixtureEnvironment; VERNON_DIAG_SOURCE = (Get-VerificationWp9DiagnosticSource); VERNON_DIAG_OUT = $diagPath; VERNON_DIAG_VARIANT = 'd1' }
    foreach ($key in $expected.Keys) { Assert-True ([Environment]::GetEnvironmentVariable($key, 'Process') -ceq $expected[$key]) "diagnostic fixes $key over an inherited value" }
    Assert-True ($null -eq [Environment]::GetEnvironmentVariable('VERNON_DIAG_START_SOURCE', 'Process')) 'd1 starts from the Stage 2 case: no inherited start source survives'
    $c0Case = Get-VerificationWp9DiagnosticStartSource 'd4'
    Assert-True ($c0Case -like '*scratch*continuation*runs*wp9-diag-c0-20261108-a*temp*vernon-wp9-diag-c0-1891740*case') 'd4 restarts from the C0 diagnostic case'
    foreach ($restart in @('r0', 'r1', 'd4', 'd5')) {
        Set-VerificationWp9DiagnosticEnvironment $diagDirectory $restart
        Assert-True ([Environment]::GetEnvironmentVariable('VERNON_DIAG_START_SOURCE', 'Process') -ceq $c0Case -and $env:VERNON_DIAG_VARIANT -ceq $restart) "$restart is given the C0 start source"
    }
    Assert-True ($null -eq (Get-VerificationWp9DiagnosticStartSource 'c0')) 'c0 has no start source'
    $m1Case = Get-VerificationWp9DiagnosticSource 'm1'
    Assert-True ($m1Case -like '*scratch*wp9-diagnostics*mesh-fix-20261109*m1*case' -and $m1Case -cne (Get-VerificationWp9DiagnosticSource 'c0')) 'm1 reads the remeshed case, not the Stage 2 case'
    Set-VerificationWp9DiagnosticEnvironment $diagDirectory 'm1'
    Assert-True ($env:VERNON_DIAG_SOURCE -ceq $m1Case -and $null -eq [Environment]::GetEnvironmentVariable('VERNON_DIAG_START_SOURCE', 'Process')) 'm1 starts from its own case at 0'
    Set-VerificationWp9DiagnosticEnvironment $diagDirectory 'd5'
    Assert-True ($env:VERNON_DIAG_SOURCE -ceq (Get-VerificationWp9DiagnosticSource 'c0')) 'd5 reads the Stage 2 case settings'
} finally { foreach ($key in $diagSaved.Keys) { [Environment]::SetEnvironmentVariable($key, $diagSaved[$key], 'Process') } }
$diagPassed = "running 1 test`ntest diagnostic::tests::wp9_turbulence_diagnostic ... ok`ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 384 filtered out;"
$diagMarker = 'WP9_DIAGNOSTIC_COMPLETE variant=d1 iterations=1000 phases=2 np=16 instrumentation=checked acceptance_evidence=0'
$validDiagRow = [ordered]@{ scope = 'turbulence_diagnostic'; variant = 'd1'; diagnostic_only = $true; acceptance_evidence = $false; physical_validation = $false; np = 16; iterations = 1000; phases = @(@{ phase = 1 }, @{ phase = 2 }) }
Write-VerificationJson $diagPath $validDiagRow
Assert-True (Test-VerificationWp9DiagnosticOutput $diagPassed $diagMarker $diagPath 'd1') 'a diagnostic accepts one executed test, its marker and a matching reloaded row'
Assert-True (-not (Test-VerificationWp9DiagnosticOutput $diagPassed $diagMarker $diagPath 'c0')) 'a d1 row never completes a c0 run'
$d4Marker = 'WP9_DIAGNOSTIC_COMPLETE variant=d4 iterations=1000 phases=1 np=16 instrumentation=checked acceptance_evidence=0'
$d4Row = [ordered]@{}
foreach ($key in $validDiagRow.Keys) { $d4Row[$key] = $validDiagRow[$key] }
$d4Row['variant'] = 'd4'; $d4Row['phases'] = @(@{ phase = 1 })
Write-VerificationJson $diagPath $d4Row
Assert-True (Test-VerificationWp9DiagnosticOutput $diagPassed $d4Marker $diagPath 'd4') 'a d4 run completes with one 501-1000 phase'
Assert-True (-not (Test-VerificationWp9DiagnosticOutput $diagPassed $d4Marker.Replace('iterations=1000', 'iterations=510') $diagPath 'd4')) 'a d4 marker must reach 1000'
Assert-True (-not (Test-VerificationWp9DiagnosticOutput $diagPassed $d4Marker $diagPath 'r1')) 'a d4 row never completes its r1 control'
$d4Row['variant'] = 'r1'
Write-VerificationJson $diagPath $d4Row
Assert-True (Test-VerificationWp9DiagnosticOutput $diagPassed $d4Marker.Replace('variant=d4', 'variant=r1') $diagPath 'r1') 'an r1 control completes with one 501-1000 phase'
foreach ($single in @('d5', 'm1')) {
    $d4Row['variant'] = $single
    Write-VerificationJson $diagPath $d4Row
    Assert-True (Test-VerificationWp9DiagnosticOutput $diagPassed $d4Marker.Replace('variant=d4', "variant=$single") $diagPath $single) "$single completes with one phase ending at 1000"
    Assert-True (-not (Test-VerificationWp9DiagnosticOutput $diagPassed $d4Marker $diagPath $single)) "a d4 marker never completes $single"
}
$d4Row['variant'] = 'r0'; $d4Row['iterations'] = 510
Write-VerificationJson $diagPath $d4Row
Assert-True (Test-VerificationWp9DiagnosticOutput $diagPassed $d4Marker.Replace('variant=d4', 'variant=r0').Replace('iterations=1000', 'iterations=510') $diagPath 'r0') 'an r0 restart check ends at 510'
Write-VerificationJson $diagPath $validDiagRow
foreach ($badMarker in @('', $diagMarker.Replace('acceptance_evidence=0', 'acceptance_evidence=1'), $diagMarker.Replace('iterations=1000', 'iterations=999'), $diagMarker.Replace('phases=2', 'phases=1'), $diagMarker.Replace('instrumentation=checked', 'instrumentation=skipped'), ($diagMarker + "`n" + $diagMarker), ($diagMarker + "`nWP9_REPLICATION_SOLVE_COMPLETE x"), ('note ' + $diagMarker))) {
    Assert-True (-not (Test-VerificationWp9DiagnosticOutput $diagPassed $badMarker $diagPath 'd1')) 'only one exact diagnostic marker matching the variant qualifies'
}
foreach ($badField in @(@('scope', 'published_replication'), @('variant', 'c0'), @('diagnostic_only', $false), @('acceptance_evidence', $true), @('physical_validation', $true), @('np', 8), @('iterations', 500), @('cd', 0.27), @('converged', $true))) {
    $badRow = [ordered]@{}
    foreach ($key in $validDiagRow.Keys) { $badRow[$key] = $validDiagRow[$key] }
    $badRow[$badField[0]] = $badField[1]
    Write-VerificationJson $diagPath $badRow
    Assert-True (-not (Test-VerificationWp9DiagnosticOutput $diagPassed $diagMarker $diagPath 'd1')) "diagnostic reload rejects $($badField[0]) = $($badField[1])"
}
Write-VerificationJson $diagPath $validDiagRow
Assert-True (-not (Test-VerificationWp9DiagnosticOutput $diagPassed.Replace('1 passed; 0 failed; 0 ignored;', '0 passed; 0 failed; 1 ignored;') $diagMarker $diagPath 'd1')) 'an ignored diagnostic test cannot complete the profile'
Remove-Item -LiteralPath $diagPath
Assert-True (-not (Test-VerificationWp9DiagnosticOutput $diagPassed $diagMarker $diagPath 'd1')) 'a missing diagnostic row fails closed'
Remove-Item -LiteralPath $diagDirectory -Recurse
# Graceful cancellation is requested through a file the worker polls.
$cancelRun = Join-Path $repo ('scratch/continuation/library-checks/cancel-' + [guid]::NewGuid().ToString('N'))
[void][IO.Directory]::CreateDirectory($cancelRun)
Assert-True (-not (Test-VerificationCancelRequested $cancelRun)) 'no cancellation without a request file'
Request-VerificationCancel $cancelRun 'library check'
Assert-True (Test-VerificationCancelRequested $cancelRun) 'a written request is seen by the worker'
# Status files are read back with ConvertFrom-Json, which turns ISO stamps into DateTime.
$selfIdentity = [ordered]@{ workerPid = $PID; workerStartedUtc = (Get-Process -Id $PID).StartTime.ToUniversalTime().ToString('o') }
$identityPath = Join-Path $cancelRun 'identity.json'
Write-VerificationJson $identityPath $selfIdentity
$reloadedIdentity = Get-Content -Raw -LiteralPath $identityPath | ConvertFrom-Json
Assert-True (Test-VerificationProcess $reloadedIdentity.workerPid $reloadedIdentity.workerStartedUtc) 'a live worker is recognised from its reloaded status record'
Assert-True (-not (Test-VerificationProcess $reloadedIdentity.workerPid ([DateTime]::UtcNow.AddDays(-1)))) 'a different start time is never the same process'
$wp9NativeSteps = @(Get-VerificationSteps 'wp9-native')
Assert-True ($wp9NativeSteps.Count -eq 1 -and ($wp9NativeSteps[0].arguments -join ' ') -match '--exact --ignored --nocapture') 'native capability executes only its explicit ignored test'
$wp9NativePassed = "running 1 test`ntest tests::real_openfoam_chain_on_a_small_car ... ok`ntest result: ok. 1 passed; 0 failed; 0 ignored;"
Assert-True (Test-VerificationWp9NativeOutput $wp9NativePassed 'WP9_NATIVE_CHAIN_COMPLETE cells=30000') 'actual native chain needs completion after provenance assertions'
Assert-True (-not (Test-VerificationWp9NativeOutput $wp9NativePassed '')) 'a passing native test without its final marker cannot qualify'
Assert-True (-not (Test-VerificationWp9NativeOutput $wp9NativePassed "WP9_NATIVE_CHAIN_COMPLETE`nSKIPPED runtime")) 'skipped native chain cannot qualify'
Assert-True (-not (Test-VerificationWp9NativeOutput 'running 0 tests' 'WP9_NATIVE_CHAIN_COMPLETE')) 'zero native tests cannot qualify'
Assert-True (-not (Test-VerificationWp9NativeOutput $wp9NativePassed.Replace('tests::real_openfoam_chain_on_a_small_car', 'tests::some_other_test') 'WP9_NATIVE_CHAIN_COMPLETE')) 'an unrelated test cannot qualify native execution'
$wp9Names = @(
    'wsl_regression_success_does_not_leave_a_background_group',
    'wsl_regression_cancelled_start_remains_blocked_after_case_replacement',
    'wsl_environment_with_installed_runtime_supports_simplefoam_help',
    'wsl_environment_may_change_directory_and_positional_parameters',
    'wsl_cancellation_before_record_publication_prevents_tool_start',
    'wsl_timeout_stops_an_active_linux_grandchild',
    'wsl_cook_cancellation_stops_children_and_does_not_cancel_cleanup'
)
$wp9Passed = (($wp9Names | ForEach-Object { "test extproc::tests::$_ ... ok" }) -join "`n") + "`ntest result: ok. 7 passed; 0 failed; 0 ignored;"
Assert-True (Test-VerificationWp9RunnerOutput $wp9Passed '') 'all seven actual WSL regressions must execute and pass'
Assert-True (-not (Test-VerificationWp9RunnerOutput 'test result: ok. 0 passed; 0 failed; 0 ignored;' '')) 'zero WSL tests cannot pass'
Assert-True (-not (Test-VerificationWp9RunnerOutput ($wp9Passed.Replace("test extproc::tests::$($wp9Names[0]) ... ok", '')) '')) 'an absent containment regression cannot pass'
Assert-True (-not (Test-VerificationWp9RunnerOutput $wp9Passed 'SKIPPED external WSL')) 'skipped WSL cannot pass'
$wp9InputRejected = $false
try { Assert-VerificationWp9Input 'wp9-wsl-runner' '' '' '' } catch { $wp9InputRejected = $true }
Assert-True $wp9InputRejected 'WP9 real runner needs explicit distribution user and environment file'
foreach ($profileName in @('wp8-real','wp8')) {
    $rejected = $false
    try { Assert-VerificationWp8Input $profileName ' ' } catch { $rejected = $true }
    Assert-True $rejected 'real profiles require explicit data input'
}
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
$stopScript = Join-Path $PSScriptRoot 'stop-verification.ps1'
& $shell -NoProfile -NonInteractive -File $launcher -Profile smoke-cancel -RunId "$prefix-cancel" -JobTimeoutSeconds 300 | Out-Host
Assert-True ($LASTEXITCODE -eq 0) 'cancel scenario launches'
$cancelState = Wait-State "$prefix-cancel" 'hold'
& $shell -NoProfile -NonInteractive -File $stopScript -RunId "$prefix-cancel" -Reason 'durable self-test' | Out-Host
Assert-True ($LASTEXITCODE -eq 0) 'stop request is accepted for a running worker'
$cancelled = Wait-State "$prefix-cancel" '' 60
Assert-True ($cancelled.status -eq 'cancelled' -and $cancelled.exitCode -eq 130 -and $cancelled.steps[-1].exitCode -eq 130 -and $cancelled.steps[-1].status -eq 'cancelled') 'a stop request ends the run as cancelled, never completed or failed'
Assert-True (-not (Test-VerificationProcess $cancelState.childPid $cancelState.childStartedUtc)) 'cancellation kills the active native step'
Assert-True ($cancelled.cancelRequest.reason -eq 'durable self-test') 'the stop reason is recorded with the run'
Assert-True ((Run-Worker 'smoke-cancel' "$prefix-cancel-resume" "$prefix-cancel") -ne 0) 'a cancelled run cannot be resumed'
& $shell -NoProfile -NonInteractive -File $stopScript -RunId "$prefix-cancel" | Out-Host
Assert-True ($LASTEXITCODE -ne 0) 'stopping a finished run is refused'
$passed++
Write-Output "PASS: $passed durable-runner scenarios; run prefix $prefix"
