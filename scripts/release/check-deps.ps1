<#
.SYNOPSIS
  Fails if a banned build-script crate (cc, cmake, bindgen) is in the resolve
  of a supported target. Runs without cargo-deny; works in Windows PowerShell
  and PowerShell 7 on Windows, Linux and macOS.
.DESCRIPTION
  .\scripts\release\check-deps.ps1
  For each target, runs `cargo tree -e normal,build --target <t> --workspace -i <crate>`.
  Windows GNU and macOS must have none of the crates.
  Linux may contain `cc` only as a build dependency of wayland-backend, and
  only while wayland-backend's `log` feature (the one that compiles C) is off;
  that is checked here too.
#>
param([string]$Root = (Split-Path -Parent (Split-Path -Parent $PSScriptRoot)))
$ErrorActionPreference = 'Continue'
$banned = 'cc', 'cmake', 'bindgen'
$targets = 'x86_64-pc-windows-gnu', 'x86_64-unknown-linux-gnu', 'aarch64-apple-darwin'
$fail = 0
Push-Location $Root
try {
    foreach ($t in $targets) {
        foreach ($c in $banned) {
            $out = (& cargo tree -e 'normal,build' --target $t --workspace -i $c 2>&1 | ForEach-Object { "$_" })
            $text = $out -join "`n"
            if ($text -match 'nothing to print' -or $text -match 'did not match any packages') {
                Write-Host "  ok   $t has no $c"; continue
            }
            if ($text -match "(?m)^error") { Write-Host "  FAIL $t $c`: cargo tree error`n$text"; $fail++; continue }
            if ($t -like '*linux*' -and $c -eq 'cc') {
                # direct dependents of cc are the lines that start at column 0 with a tree glyph
                $direct = @($out |
                    ForEach-Object { if ($_ -match '^[^ ]+ (\S+) v\d') { $Matches[1] } } | Select-Object -Unique)
                $feat = (& cargo tree -e features --target $t --workspace -i wayland-backend 2>&1 | ForEach-Object { "$_" }) -join "`n"
                $onlyWayland = ($direct.Count -eq 1 -and $direct[0] -eq 'wayland-backend')
                if ($onlyWayland -and $feat -notmatch 'wayland-backend feature "log"') {
                    Write-Host "  ok   $t cc only via wayland-backend, its `log` feature (compiles C) is off"; continue
                }
            }
            Write-Host "  FAIL $t resolves $c`:`n$text"; $fail++
        }
    }
} finally { Pop-Location }
if ($fail -gt 0) { Write-Host "`n$fail banned-crate finding(s)"; exit 1 }
Write-Host "`nno banned build-script crates in the resolve"
# `cargo tree -i` exits non-zero when the crate is absent (the passing case), so state success explicitly.
exit 0
