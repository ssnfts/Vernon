<#
.SYNOPSIS
  Tests the packaging helpers and a real install cycle of the built zip.
.DESCRIPTION
  . .\scripts\dev-env.ps1; .\scripts\release\package.ps1; .\scripts\release\package-selftest.ps1
  1. Unit checks of the helper functions in packaging\installer\lib.ps1.
  2. Clean-room install test, run in a temp folder whose name contains
     spaces, and again on another fixed drive when one has room: unzip,
     install.ps1 (temp install root, temp Start Menu folder, registry keys
     under HKCU:\Software\VernonSelfTest, -AddToPath), cook the example from
     the installed copy, start the app hidden and confirm it is still alive
     after 10 s (only that probe process is stopped), uninstall, and check
     nothing is left behind (files, shortcut, registry, user PATH).
  The only machine-wide state touched is the user PATH, which is restored to
  its exact original value in a finally block and compared afterwards.
#>
param(
    [string]$Zip,
    [int]$ProbeSeconds = 10
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
. (Join-Path $root 'packaging\installer\lib.ps1')
if (-not $Zip) {
    $Zip = Get-ChildItem (Join-Path $root 'dist') -Filter 'Vernon-*-win64.zip' | Sort-Object LastWriteTime | Select-Object -Last 1 -ExpandProperty FullName
}
if (-not $Zip -or -not (Test-Path -LiteralPath $Zip)) { throw 'no package zip; run scripts\release\package.ps1 first' }

$script:fail = 0; $script:pass = 0
function Check([string]$what, [bool]$ok, [string]$detail = '') {
    if ($ok) { $script:pass++; Write-Host "  ok   $what" }
    else { $script:fail++; Write-Host "  FAIL $what $detail" -ForegroundColor Red }
}
function Run-Ps([string]$file, [string[]]$extra) {
    $o = & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $file @extra 2>&1 | Out-String
    [pscustomobject]@{ Code = $LASTEXITCODE; Out = $o }
}

# --- 1. unit checks -------------------------------------------------------------------
Write-Host 'Unit checks'
Check 'version from Cargo.toml' ((Get-WorkspaceVersion $root) -match '^\d+\.\d+\.\d+')
Check 'git info has a sha' ((Get-GitInfo $root).Sha.Length -ge 7)
Check 'Add-PathEntry appends' ((Add-PathEntry 'C:\a;C:\b' 'C:\x y\bin') -eq 'C:\a;C:\b;C:\x y\bin')
Check 'Add-PathEntry is idempotent (case, slash)' ((Add-PathEntry 'C:\a;C:\X Y\BIN\' 'c:\x y\bin') -eq 'C:\a;C:\X Y\BIN\')
Check 'Add-PathEntry on empty' ((Add-PathEntry '' 'C:\x') -eq 'C:\x')
Check 'Remove-PathEntry removes only that entry' ((Remove-PathEntry 'C:\a;C:\x y\bin\;%USERPROFILE%\b' 'C:\x y\bin') -eq 'C:\a;%USERPROFILE%\b')
Check 'Add then Remove restores a trailing-semicolon PATH exactly' ((Remove-PathEntry (Add-PathEntry 'C:\a;C:\b;' 'C:\x') 'C:\x') -eq 'C:\a;C:\b;')
Check 'Remove-PathEntry keeps unexpanded vars' ((Remove-PathEntry '%A%\q;C:\x' 'C:\x') -eq '%A%\q')

$u = Join-Path ([IO.Path]::GetTempPath()) ("vernon unit $PID")
New-Item -ItemType Directory -Force -Path "$u\sub dir" | Out-Null
try {
    Set-Content "$u\a.txt" 'hello'; Set-Content "$u\sub dir\b.txt" 'world'
    $m = @(Get-FileManifest $u)
    Check 'manifest lists files with forward slashes' (($m.path -join ',') -eq 'a.txt,sub dir/b.txt')
    Check 'manifest size/hash recorded' ($m[0].size -gt 0 -and $m[0].sha256.Length -eq 64)
    [ordered]@{ files = $m } | ConvertTo-Json -Depth 4 | Set-Content "$u\MANIFEST.json"
    Check 'intact tree verifies' ((Test-FileManifest $u).Count -eq 0)
    Set-Content "$u\a.txt" 'HELLO!'
    Check 'tampered file is caught' ((Test-FileManifest $u) -match 'differs')
    Set-Content "$u\extra.txt" 'x'; Remove-Item "$u\sub dir\b.txt"
    $p = Test-FileManifest $u
    Check 'missing and unlisted files are caught' (($p -match 'missing') -and ($p -match 'unlisted'))
    New-ZipFromDirectory $u "$u.zip" 'root'
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $names = [IO.Compression.ZipFile]::OpenRead("$u.zip") | ForEach-Object { try { $_.Entries.FullName } finally { $_.Dispose() } }
    Check 'zip entries use forward slashes under one root' (($names | Where-Object { $_ -notmatch '^root/' -or $_ -match '\\' }).Count -eq 0)
    $rb = 'HKCU:\Software\VernonSelfTestUnit'
    try {
        $prev = Register-VrnFileType $rb 'C:\p q\vernon.exe'
        Check 'file type registered' ((Get-Item "$rb\Classes\.vrn").GetValue('') -eq 'Vernon.Project')
        Check 'open command quotes exe and file' ((Get-Item "$rb\Classes\Vernon.Project\shell\open\command").GetValue('') -eq '"C:\p q\vernon.exe" "%1"')
        Unregister-VrnFileType $rb $prev
        Check 'file type removed' (-not (Test-Path "$rb\Classes\.vrn") -and -not (Test-Path "$rb\Classes\Vernon.Project"))
        New-Item -Force -Path "$rb\Classes\.vrn" | Out-Null; Set-Item "$rb\Classes\.vrn" -Value 'Other.App'
        $prev = Register-VrnFileType $rb 'C:\v.exe'; Unregister-VrnFileType $rb $prev
        Check 'previous association restored' ((Get-Item "$rb\Classes\.vrn").GetValue('') -eq 'Other.App')
    } finally { if (Test-Path $rb) { Remove-Item $rb -Recurse -Force } }
} finally { Remove-Item $u -Recurse -Force -ErrorAction SilentlyContinue; Remove-Item "$u.zip" -Force -ErrorAction SilentlyContinue }

# --- 2. install cycle ----------------------------------------------------------------------
function Test-InstallCycle([string]$Base, [string]$Label) {
    Write-Host "Install cycle: $Label ($Base)"
    $work = Join-Path $Base 'vernon pkg test'
    $reg = 'HKCU:\Software\VernonSelfTest'
    $pathBefore = Get-UserPathRaw
    if (Test-Path -LiteralPath $work) { Remove-Item -LiteralPath $work -Recurse -Force }
    New-Item -ItemType Directory -Force -Path $work | Out-Null
    $probe = $null
    try {
        Add-Type -AssemblyName System.IO.Compression.FileSystem
        [IO.Compression.ZipFile]::ExtractToDirectory($Zip, "$work\unzipped")
        $pkg = (Get-ChildItem "$work\unzipped" -Directory | Select-Object -First 1).FullName
        Check 'unzipped package verifies against MANIFEST.json' ((Test-FileManifest $pkg).Count -eq 0)

        $inst = "$work\install root"; $menu = "$work\start menu\Vernon"
        $r = Run-Ps "$pkg\install.ps1" @('-InstallRoot', $inst, '-StartMenuDir', $menu, '-RegBase', $reg, '-AddToPath')
        Check 'install.ps1 exits 0' ($r.Code -eq 0) $r.Out
        Check 'installed exes present' ((Test-Path "$inst\bin\vernon.exe") -and (Test-Path "$inst\bin\vernon-cli.exe"))
        Check 'installed copy verifies' ((Test-FileManifest $inst).Count -eq 0 -or ((Test-FileManifest $inst) -join '' ) -match '^unlisted: install.json$')
        Check 'Start Menu shortcut created' (Test-Path "$menu\Vernon.lnk")
        Check 'file type registered' ((Get-Item "$reg\Classes\.vrn").GetValue('') -eq 'Vernon.Project')
        Check 'Add/Remove Programs entry' (Test-Path "$reg\Microsoft\Windows\CurrentVersion\Uninstall\Vernon")
        Check 'bin added to user PATH' ((Get-UserPathRaw) -split ';' -contains "$inst\bin")

        $o = & "$inst\bin\vernon-cli.exe" cook "$inst\examples\bodykit_pipeline.vrn" 2>&1 | Out-String
        $code = $LASTEXITCODE
        Check 'installed vernon-cli cooks the example (exit 0: only the 2 expected gates)' ($code -eq 0) "exit $code`n$($o.Substring([math]::Max(0, $o.Length - 600)))"
        Check 'cook reported 38 nodes' ($o -match '38')

        $probe = Start-Process -FilePath "$inst\bin\vernon.exe" -ArgumentList @("`"$inst\examples\bodykit_pipeline.vrn`"") -WorkingDirectory $inst -WindowStyle Hidden -PassThru
        Start-Sleep -Seconds $ProbeSeconds
        $probe.Refresh()
        Check "app still alive after $ProbeSeconds s" (-not $probe.HasExited) "exit code $($probe.ExitCode)"
        if (-not $probe.HasExited) { Stop-Process -Id $probe.Id -Force; $probe.WaitForExit(5000) | Out-Null }
        $probe = $null

        $r = Run-Ps "$inst\uninstall.ps1" @()
        Check 'uninstall.ps1 exits 0' ($r.Code -eq 0) $r.Out
        Check 'install folder gone' (-not (Test-Path -LiteralPath $inst))
        Check 'Start Menu folder gone' (-not (Test-Path -LiteralPath $menu))
        Check 'registry keys gone' (-not (Test-Path "$reg\Classes\.vrn") -and -not (Test-Path "$reg\Classes\Vernon.Project") -and -not (Test-Path "$reg\Microsoft\Windows\CurrentVersion\Uninstall\Vernon"))
        Check 'user PATH restored byte for byte' ((Get-UserPathRaw) -ceq $pathBefore)

        # portable mode touches only the package folder
        $r = Run-Ps "$pkg\install.ps1" @('-Portable')
        Check 'portable marker written, nothing installed' (($r.Code -eq 0) -and (Test-Path "$pkg\portable.txt") -and -not (Test-Path "$inst"))
        $o = & "$pkg\bin\vernon-cli.exe" cook "$pkg\examples\bodykit_pipeline.vrn" 2>&1 | Out-String
        Check 'package runs in place from a path with spaces' ($LASTEXITCODE -eq 0)
    } finally {
        if ($probe -and -not $probe.HasExited) { Stop-Process -Id $probe.Id -Force }
        if ((Get-UserPathRaw) -cne $pathBefore) { Set-UserPathRaw $pathBefore; Write-Host '  (user PATH restored by cleanup)' }
        if (Test-Path $reg) { Remove-Item $reg -Recurse -Force }
        Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
    }
    Check 'temp tree removed' (-not (Test-Path -LiteralPath $work))
}

$sysDrive = [IO.Path]::GetPathRoot([IO.Path]::GetTempPath())
Test-InstallCycle ([IO.Path]::GetTempPath()) 'system drive'
$other = [IO.DriveInfo]::GetDrives() | Where-Object { $_.DriveType -eq 'Fixed' -and $_.IsReady -and $_.RootDirectory.FullName -ne $sysDrive -and $_.AvailableFreeSpace -gt 2GB } | Select-Object -First 1
if ($other) { Test-InstallCycle $other.RootDirectory.FullName "other drive $($other.Name)" }
else { Write-Host 'No second fixed drive with space: the different-drive case was not run.' }

Write-Host "`n$script:pass passed, $script:fail failed"
if ($script:fail -gt 0) { exit 1 }
