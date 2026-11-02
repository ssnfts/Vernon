<#
.SYNOPSIS
  Removes a per-user Vernon install made by install.ps1.
.DESCRIPTION
  Deletes the install folder, the Start Menu shortcut, the .vrn file type
  (restoring any previous one), the PATH entry and the Add/Remove Programs
  entry. Your settings in %APPDATA%\Vernon are kept unless -RemoveUserData.
#>
param([switch]$KeepFiles, [switch]$Quiet, [switch]$RemoveUserData)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'setup\lib.ps1')

$rec = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'install.json') -Raw | ConvertFrom-Json
if ($rec.pathEntry) { Set-UserPathRaw (Remove-PathEntry (Get-UserPathRaw) $rec.pathEntry) }
if ($rec.fileType) { Unregister-VrnFileType $rec.regBase $rec.fileTypePrevious }
if ($rec.uninstallKey -and (Test-Path -LiteralPath $rec.uninstallKey)) { Remove-Item -LiteralPath $rec.uninstallKey -Recurse -Force }
if ($rec.startMenuDir -and (Test-Path -LiteralPath $rec.startMenuDir)) { Remove-Item -LiteralPath $rec.startMenuDir -Recurse -Force }
if ($RemoveUserData -and $env:APPDATA) {
    $cfg = Join-Path $env:APPDATA 'Vernon'
    if (Test-Path -LiteralPath $cfg) { Remove-Item -LiteralPath $cfg -Recurse -Force }
}
if (-not $KeepFiles) {
    Set-Location ([IO.Path]::GetTempPath())   # never sit inside the folder being deleted
    Remove-Item -LiteralPath $rec.installRoot -Recurse -Force
}
if (-not $Quiet) { Write-Host 'Vernon was removed.' }
