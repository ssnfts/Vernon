# Dot-source before building on Windows with the GNU toolchain:
#   . .\scripts\dev-env.ps1
# Puts cargo and the WinLibs MinGW tools on PATH for this shell.
$mingw = Get-ChildItem "$env:LOCALAPPDATA\Microsoft\WinGet\Packages" -Directory -Filter "BrechtSanders.WinLibs*" -ErrorAction SilentlyContinue |
    Select-Object -First 1 | ForEach-Object { Join-Path $_.FullName "mingw64\bin" }
$env:PATH = "$env:USERPROFILE\.cargo\bin;$mingw;$env:PATH"
