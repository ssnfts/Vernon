<#
.SYNOPSIS
  Builds the distributable Vernon folder, zip and SHA-256 file.
.DESCRIPTION
  . .\scripts\dev-env.ps1; .\scripts\package.ps1
  Produces dist\Vernon-<version>-win64\, dist\Vernon-<version>-win64.zip and
  dist\Vernon-<version>-win64.zip.sha256. <version> comes from the workspace
  Cargo.toml; the git short SHA is stamped into VERSION.txt and MANIFEST.json.
  Fails if the release build is not clean (non-zero exit or any warning).
  Documents from the agent-tools / agent-client branches (docs\agent\,
  docs\MCP.md, docs\AGENT.md) are copied into agent\ when present.
.PARAMETER SkipBuild
  Reuse the release binaries already in the target directory (still verified).
.PARAMETER SkipLicenses
  Reuse the checked-in THIRD_PARTY_LICENSES.md instead of regenerating it.
.PARAMETER OutDir
  Where dist output goes (default: <repo>\dist).
#>
param(
    [switch]$SkipBuild,
    [switch]$SkipLicenses,
    [string]$OutDir,
    [string]$TargetDir
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
. (Join-Path $root 'packaging\installer\lib.ps1')
if (-not $OutDir) { $OutDir = Join-Path $root 'dist' }
if ($TargetDir) { $env:CARGO_TARGET_DIR = $TargetDir }
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root 'target' }

$version = Get-WorkspaceVersion $root
$git = Get-GitInfo $root
$name = "Vernon-$version-win64"
$stage = Join-Path $OutDir $name
$zip = Join-Path $OutDir "$name.zip"
Write-Host "Packaging $name (git $($git.Sha)$(if ($git.Dirty) { ', working tree dirty' }))"

# 1. release build -------------------------------------------------------------
if (-not $SkipBuild) {
    Push-Location $root
    try {
        $out = & cmd.exe /c 'cargo build --release -p vernon -p vernon-cli 2>&1'
        $code = $LASTEXITCODE
    } finally { Pop-Location }
    $out | Select-Object -Last 8 | ForEach-Object { Write-Host "  $_" }
    if ($code -ne 0) { throw "release build failed (exit $code)" }
    if ($out | Where-Object { $_ -match '^\s*warning' }) { throw 'release build is not clean: it printed warnings' }
}
$exes = @('vernon.exe', 'vernon-cli.exe') | ForEach-Object { Join-Path $target "release\$_" }
foreach ($e in $exes) { if (-not (Test-Path -LiteralPath $e)) { throw "missing $e (build first)" } }

# 2. licences ----------------------------------------------------------------------
if (-not $SkipLicenses) { & (Join-Path $PSScriptRoot 'third-party-licenses.ps1') | ForEach-Object { Write-Host "  $_" } }

# 3. stage the folder ---------------------------------------------------------------
if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
New-Item -ItemType Directory -Force -Path (Join-Path $stage 'bin'), (Join-Path $stage 'setup'),
    (Join-Path $stage 'examples'), (Join-Path $stage 'docs'), (Join-Path $stage 'tools'), (Join-Path $stage 'agent') | Out-Null

$strip = Get-Command strip.exe -ErrorAction SilentlyContinue
if (-not $strip) { throw 'strip.exe not found; dot-source scripts\dev-env.ps1 first' }
foreach ($e in $exes) {
    $dst = Join-Path $stage ('bin\' + (Split-Path -Leaf $e))
    Copy-Item -LiteralPath $e -Destination $dst
    & $strip.Source $dst
    if ($LASTEXITCODE -ne 0) { throw "strip failed for $dst" }
}

Copy-Item (Join-Path $root 'examples\bodykit_pipeline.vrn') (Join-Path $stage 'examples')   # not the generated .work folder
foreach ($f in 'LICENSE', 'NOTICE', 'THIRD_PARTY_LICENSES.md') { Copy-Item (Join-Path $root $f) $stage }
Copy-Item (Join-Path $root 'README.md') (Join-Path $stage 'docs\PROJECT_README.md')
foreach ($f in 'NODES.md', 'ROADMAP.md') { Copy-Item (Join-Path $root "docs\$f") (Join-Path $stage 'docs') }
Copy-Item (Join-Path $root 'docs\specs') (Join-Path $stage 'docs\specs') -Recurse   # behaviour specs; not research/ or superpowers/

$pk = Join-Path $root 'packaging'
Copy-Item (Join-Path $pk 'README.txt') $stage
Copy-Item (Join-Path $pk 'tools\README.md') (Join-Path $stage 'tools')
Copy-Item (Join-Path $pk 'agent\README.md') (Join-Path $stage 'agent')
foreach ($f in 'install.ps1', 'uninstall.ps1', 'install.cmd', 'uninstall.cmd') { Copy-Item (Join-Path $pk "installer\$f") $stage }
Copy-Item (Join-Path $pk 'installer\lib.ps1') (Join-Path $stage 'setup')

# agent docs, once the sibling branches have landed them
$agentDocs = @()
$ad = Join-Path $root 'docs\agent'
if (Test-Path -LiteralPath $ad) { Copy-Item "$ad\*" (Join-Path $stage 'agent') -Recurse -Force; $agentDocs += 'docs/agent/' }
foreach ($f in 'MCP.md', 'AGENT.md') {
    $p = Join-Path $root "docs\$f"
    if (Test-Path -LiteralPath $p) { Copy-Item $p (Join-Path $stage "agent\$f"); $agentDocs += "docs/$f" }
}
if ($agentDocs.Count -gt 0) {
    Write-Host "  agent docs picked up: $($agentDocs -join ', ')"
    (Get-Content -LiteralPath (Join-Path $stage 'agent\README.md') -Raw) -replace '(?s)> \*\*Placeholder\.\*\*.*?\r?\n\r?\n', "> The final guides from this build are in this folder next to this file.`n`n" |
        Set-Content -LiteralPath (Join-Path $stage 'agent\README.md') -Encoding UTF8
}

$stamp = "$version+$($git.Sha)$(if ($git.Dirty) { '-dirty' })"
Set-Content -LiteralPath (Join-Path $stage 'VERSION.txt') -Encoding ASCII -Value @($stamp, "built $([DateTime]::UtcNow.ToString('yyyy-MM-dd HH:mm')) UTC")

# 4. manifest ---------------------------------------------------------------------------
$files = @(Get-FileManifest $stage @('MANIFEST.json'))
[ordered]@{
    name = 'Vernon'; version = $version; git = $git.Sha; dirty = $git.Dirty
    built_utc = [DateTime]::UtcNow.ToString('o'); platform = 'win64'; files = $files
} | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $stage 'MANIFEST.json') -Encoding UTF8

# 5. zip and checksum ---------------------------------------------------------------------
New-ZipFromDirectory $stage $zip $name
$hash = Get-FileSha256 $zip
Set-Content -LiteralPath "$zip.sha256" -Encoding ASCII -Value "$hash *$name.zip"
$mb = [math]::Round((Get-Item -LiteralPath $zip).Length / 1MB, 1)
Write-Host "folder: $stage ($($files.Count) files)"
Write-Host "zip:    $zip ($mb MB)"
Write-Host "sha256: $hash"
