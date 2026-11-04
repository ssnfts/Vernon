#requires -Version 7.2
<#
Checks the Windows build toolchain: Rust GNU toolchain plus WinLibs MinGW.
Installs nothing; prints the command for anything missing. With
-WriteCargoConfig it writes ~/.cargo/config.toml (linker and dlltool for
x86_64-pc-windows-gnu) when that file does not exist yet.

  .\scripts\setup\setup-dev.ps1
  .\scripts\setup\setup-dev.ps1 -WriteCargoConfig
#>
[CmdletBinding()]
param([switch]$WriteCargoConfig)
$ErrorActionPreference = 'Stop'
$missing = 0
function Report([bool]$Ok, [string]$What, [string]$Fix) {
    if ($Ok) { Write-Host "ok       $What" } else { Write-Host "MISSING  $What`n         -> $Fix"; $script:missing++ }
}
Report ($PSVersionTable.PSVersion -ge [version]'7.2') "PowerShell 7.2+ ($($PSVersionTable.PSVersion))" 'winget install Microsoft.PowerShell'
Report ([bool](Get-Command git -ErrorAction SilentlyContinue)) 'git' 'winget install Git.Git'
$rustup = Join-Path $env:USERPROFILE '.cargo\bin\rustup.exe'
Report (Test-Path $rustup) 'rustup' 'winget install Rustlang.Rustup'
if (Test-Path $rustup) {
    $toolchains = & $rustup toolchain list
    Report ([bool]($toolchains -match 'stable-x86_64-pc-windows-gnu')) 'Rust stable GNU toolchain' 'rustup toolchain install stable-x86_64-pc-windows-gnu; rustup default stable-x86_64-pc-windows-gnu'
}
$mingw = Get-ChildItem "$env:LOCALAPPDATA\Microsoft\WinGet\Packages" -Directory -Filter 'BrechtSanders.WinLibs*' -ErrorAction SilentlyContinue |
    Select-Object -First 1 | ForEach-Object { Join-Path $_.FullName 'mingw64\bin' }
Report ($mingw -and (Test-Path (Join-Path $mingw 'gcc.exe'))) 'WinLibs MinGW (gcc, dlltool)' 'winget install BrechtSanders.WinLibs.POSIX.UCRT'

$config = Join-Path $env:USERPROFILE '.cargo\config.toml'
if (Test-Path $config) {
    Report ((Get-Content -Raw $config) -match 'x86_64-pc-windows-gnu') "cargo config $config" 'add the [target.x86_64-pc-windows-gnu] linker/dlltool entries (see docs/SETUP.md)'
} elseif ($WriteCargoConfig -and $mingw) {
    $bin = $mingw.Replace('\', '/')
    [IO.File]::WriteAllText($config, "# WinLibs MinGW tools for the GNU Rust toolchain (written by scripts/setup/setup-dev.ps1).`n[target.x86_64-pc-windows-gnu]`nlinker = `"$bin/gcc.exe`"`nrustflags = [`"-Cdlltool=$bin/dlltool.exe`"]`n")
    Write-Host "wrote    $config"
} else {
    Report $false "cargo config $config" 'rerun with -WriteCargoConfig after WinLibs is installed'
}
if ($missing) { Write-Host "`n$missing item(s) missing."; exit 1 }
Write-Host "`nToolchain ready. In each new shell: . .\scripts\dev-env.ps1"
