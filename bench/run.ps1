# Runs the whole validation and benchmark suite and writes bench/results/REPORT.md and results.json.
#   .\bench\run.ps1                    full run (includes 1M-triangle performance cases, slow)
#   .\bench\run.ps1 -Quick             skip the 1M cases
#   .\bench\run.ps1 -NoPerf            skip performance
#   .\bench\run.ps1 -UpdateBaseline    store this run's timings as bench/baseline.json
#   .\bench\run.ps1 -WithApp           also build the desktop app so the start probe can run
# Exit code 1 when any check FAILs. SKIPPED is not a failure.
param([switch]$Quick, [switch]$NoPerf, [switch]$UpdateBaseline, [switch]$WithApp)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
. "$root\scripts\dev-env.ps1"
if (-not $env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR = Join-Path (Split-Path -Parent $root) '_targets\validation-bench' }
$env:CARGO_NET_OFFLINE = 'true'
$pkgs = @('-p', 'vernon-bench', '-p', 'vernon-cli')
if ($WithApp) { $pkgs += @('-p', 'vernon') }
cargo build --release @pkgs
if ($LASTEXITCODE -ne 0) { exit 2 }
$extra = @()
if ($Quick) { $extra += '--quick' }
if ($NoPerf) { $extra += '--no-perf' }
if ($UpdateBaseline) { $extra += '--update-baseline' }
& "$env:CARGO_TARGET_DIR\release\vernon-bench.exe" run @extra
exit $LASTEXITCODE
