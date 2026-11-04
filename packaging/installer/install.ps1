<#
.SYNOPSIS
  Per-user installer for Vernon (no administrator rights needed).
.DESCRIPTION
  Copies this folder to %LOCALAPPDATA%\Programs\Vernon, adds a Start Menu
  shortcut and registers the .vrn file type. Optionally puts the command-line
  tool on your user PATH. Undo everything with uninstall.ps1.
  -Portable instead marks this folder as portable: run bin\vernon.exe from
  here and keep its settings in .\data\config, touching nothing else.
#>
param(
    [string]$InstallRoot = (Join-Path $env:LOCALAPPDATA 'Programs\Vernon'),
    [switch]$AddToPath,
    [switch]$NoShortcut,
    [switch]$NoFileAssociation,
    [switch]$Portable,
    [switch]$SkipVerify,
    # Test hooks: where shortcuts and registry keys go (defaults are the real per-user locations).
    [string]$StartMenuDir = (Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\Vernon'),
    [string]$RegBase = 'HKCU:\Software'
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'setup\lib.ps1')

function Install-Vernon {
    $src = $PSScriptRoot
    if ($Portable) {
        Set-Content -LiteralPath (Join-Path $src 'portable.txt') -Encoding ASCII -Value @(
            'Vernon portable mode: settings are kept in .\data\config next to this file.',
            'Delete this file to go back to per-user settings in %APPDATA%\Vernon.')
        Write-Host "Portable mode on. Run $src\bin\vernon.exe; nothing was installed."
        return
    }
    if (-not $SkipVerify) {
        $bad = Test-FileManifest $src
        if ($bad.Count -gt 0) { throw ("package is damaged or incomplete:`n  " + ($bad -join "`n  ")) }
    }
    $InstallRoot = [IO.Path]::GetFullPath($InstallRoot)
    if ($InstallRoot.TrimEnd('\') -eq $src.TrimEnd('\')) { throw 'install root is the package folder itself' }
    $recordPath = Join-Path $InstallRoot 'install.json'
    if ((Test-Path -LiteralPath $InstallRoot) -and -not (Test-Path -LiteralPath $recordPath) -and
        (Get-ChildItem -LiteralPath $InstallRoot -Force | Select-Object -First 1)) {
        throw "$InstallRoot exists and is not a Vernon install; choose another -InstallRoot"
    }
    if (Test-Path -LiteralPath $recordPath) {   # upgrade: undo the old registration first
        & (Join-Path $InstallRoot 'uninstall.ps1') -KeepFiles -Quiet
    }
    New-Item -ItemType Directory -Force -Path $InstallRoot | Out-Null
    Get-ChildItem -LiteralPath $src -Force | Where-Object { $_.Name -notin @('install.json', 'portable.txt') } |
        Copy-Item -Destination $InstallRoot -Recurse -Force
    $exe = Join-Path $InstallRoot 'bin\vernon.exe'
    if (-not (Test-Path -LiteralPath $exe)) { throw 'bin\vernon.exe missing after copy' }

    $rec = [ordered]@{ installRoot = $InstallRoot; startMenuDir = $null; regBase = $RegBase
                       fileTypePrevious = $null; fileType = $false; pathEntry = $null; uninstallKey = $null }
    if (-not $NoShortcut) {
        New-VernonShortcut (Join-Path $StartMenuDir 'Vernon.lnk') $exe $InstallRoot 'Vernon node-graph 3D toolkit'
        $rec.startMenuDir = $StartMenuDir
    }
    if (-not $NoFileAssociation) {
        $rec.fileTypePrevious = Register-VrnFileType $RegBase $exe
        $rec.fileType = $true
    }
    if ($AddToPath) {
        $bin = Join-Path $InstallRoot 'bin'
        $old = Get-UserPathRaw
        $new = Add-PathEntry $old $bin
        if ($new -ne $old) { Set-UserPathRaw $new; $rec.pathEntry = $bin }
    }
    $ver = (Get-Content -LiteralPath (Join-Path $InstallRoot 'VERSION.txt') -TotalCount 1)
    $uk = "$RegBase\Microsoft\Windows\CurrentVersion\Uninstall\Vernon"
    New-Item -Force -Path $uk | Out-Null
    $props = @{ DisplayName = 'Vernon'; DisplayVersion = $ver; Publisher = 'The Vernon contributors'
                InstallLocation = $InstallRoot; DisplayIcon = $exe
                UninstallString = "powershell.exe -NoProfile -ExecutionPolicy Bypass -File `"$InstallRoot\uninstall.ps1`"" }
    foreach ($k in $props.Keys) { New-ItemProperty -Force -Path $uk -Name $k -Value $props[$k] -PropertyType String | Out-Null }
    $rec.uninstallKey = $uk
    $rec | ConvertTo-Json | Set-Content -LiteralPath $recordPath -Encoding UTF8
    Write-Host "Installed Vernon $ver to $InstallRoot"
    if ($rec.pathEntry) { Write-Host 'Added bin to your user PATH; open a new terminal to use vernon-cli.' }
}

if ($MyInvocation.InvocationName -ne '.') { Install-Vernon }
