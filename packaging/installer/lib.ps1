# Shared helpers for package.ps1, install.ps1, uninstall.ps1 and the self-test.
# Dot-source only; nothing here runs on load. Windows PowerShell 5.1 compatible.

function Get-WorkspaceVersion([string]$Root) {
    $toml = Get-Content -LiteralPath (Join-Path $Root 'Cargo.toml') -Raw
    $m = [regex]::Match($toml, '(?ms)^\[workspace\.package\].*?^version\s*=\s*"([^"]+)"')
    if (-not $m.Success) { throw 'no [workspace.package] version in Cargo.toml' }
    $m.Groups[1].Value
}

function Get-GitInfo([string]$Root) {
    $sha = 'nogit'; $dirty = $false
    $git = Get-Command git -ErrorAction SilentlyContinue
    if ($git) {
        $s = & git -C $Root rev-parse --short=9 HEAD 2>$null
        if ($LASTEXITCODE -eq 0 -and $s) { $sha = "$s".Trim() }
        $st = & git -C $Root status --porcelain 2>$null
        if ($st) { $dirty = $true }
    }
    [pscustomobject]@{ Sha = $sha; Dirty = $dirty }
}

function Get-FileSha256([string]$Path) {
    (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

# Every file under $Dir (forward-slash relative paths) with size and SHA-256.
function Get-FileManifest([string]$Dir, [string[]]$Exclude = @()) {
    $base = (Resolve-Path -LiteralPath $Dir).Path.TrimEnd('\')
    Get-ChildItem -LiteralPath $base -Recurse -File | ForEach-Object {
        $rel = $_.FullName.Substring($base.Length + 1).Replace('\', '/')
        if ($Exclude -contains $rel) { return }
        [pscustomobject]@{ path = $rel; size = $_.Length; sha256 = Get-FileSha256 $_.FullName }
    } | Sort-Object path
}

# Returns a list of problems (empty when $Dir matches its MANIFEST.json).
function Test-FileManifest([string]$Dir) {
    $problems = New-Object System.Collections.Generic.List[string]
    $mf = Join-Path $Dir 'MANIFEST.json'
    if (-not (Test-Path -LiteralPath $mf)) { $problems.Add('MANIFEST.json missing'); return $problems.ToArray() }
    $m = Get-Content -LiteralPath $mf -Raw | ConvertFrom-Json
    $listed = @{}
    foreach ($f in $m.files) {
        $listed[$f.path] = $true
        $p = Join-Path $Dir ($f.path.Replace('/', '\'))
        if (-not (Test-Path -LiteralPath $p)) { $problems.Add("missing: $($f.path)"); continue }
        if ((Get-Item -LiteralPath $p).Length -ne $f.size) { $problems.Add("size differs: $($f.path)"); continue }
        if ((Get-FileSha256 $p) -ne $f.sha256) { $problems.Add("hash differs: $($f.path)") }
    }
    foreach ($e in (Get-FileManifest $Dir @('MANIFEST.json', 'install.json'))) {
        if (-not $listed.ContainsKey($e.path)) { $problems.Add("unlisted: $($e.path)") }
    }
    $problems.ToArray()
}

# Zip with forward-slash entry names (Windows PowerShell 5.1's Compress-Archive uses backslashes).
function New-ZipFromDirectory([string]$SrcDir, [string]$Zip, [string]$RootName) {
    Add-Type -AssemblyName System.IO.Compression
    if (Test-Path -LiteralPath $Zip) { Remove-Item -LiteralPath $Zip -Force }
    $base = (Resolve-Path -LiteralPath $SrcDir).Path.TrimEnd('\')
    $fs = [IO.File]::Open($Zip, 'CreateNew')
    try {
        $za = New-Object IO.Compression.ZipArchive($fs, [IO.Compression.ZipArchiveMode]::Create)
        try {
            foreach ($f in (Get-ChildItem -LiteralPath $base -Recurse -File | Sort-Object FullName)) {
                $name = $RootName + '/' + $f.FullName.Substring($base.Length + 1).Replace('\', '/')
                $e = $za.CreateEntry($name, [IO.Compression.CompressionLevel]::Optimal)
                $es = $e.Open()
                try { $in = [IO.File]::OpenRead($f.FullName); try { $in.CopyTo($es) } finally { $in.Dispose() } } finally { $es.Dispose() }
            }
        } finally { $za.Dispose() }
    } finally { $fs.Dispose() }
}

# --- PATH helpers (string level, so they can be unit-tested) -------------------
function Format-PathEntry([string]$p) { $p.Trim().TrimEnd('\').ToLowerInvariant() }
function Add-PathEntry([string]$PathValue, [string]$Entry) {
    if (@($PathValue -split ';') | Where-Object { $_ -and (Format-PathEntry $_) -eq (Format-PathEntry $Entry) }) { return $PathValue }
    if (-not $PathValue) { return $Entry }
    if ($PathValue.EndsWith(';')) { return $PathValue + $Entry + ';' }
    $PathValue + ';' + $Entry
}
# Keeps every other part (even empty ones) so Add then Remove restores the exact original.
function Remove-PathEntry([string]$PathValue, [string]$Entry) {
    (@($PathValue -split ';') | Where-Object { -not ($_ -and (Format-PathEntry $_) -eq (Format-PathEntry $Entry)) }) -join ';'
}
# The per-user PATH exactly as stored (unexpanded %VARS% kept).
function Get-UserPathRaw {
    $k = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment')
    try { $v = $k.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames) } finally { $k.Dispose() }
    [string]$v
}
function Set-UserPathRaw([string]$Value) {
    $k = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
    try { $k.SetValue('Path', $Value, [Microsoft.Win32.RegistryValueKind]::ExpandString) } finally { $k.Dispose() }
}

# --- shortcuts and file type ---------------------------------------------------
function New-VernonShortcut([string]$Lnk, [string]$Target, [string]$WorkDir, [string]$Description) {
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $Lnk) | Out-Null
    $sh = New-Object -ComObject WScript.Shell
    try {
        $s = $sh.CreateShortcut($Lnk)
        $s.TargetPath = $Target; $s.WorkingDirectory = $WorkDir; $s.Description = $Description
        $s.IconLocation = "$Target,0"
        $s.Save()
    } finally { [void][Runtime.InteropServices.Marshal]::ReleaseComObject($sh) }
}

# Writes the .vrn association under "$RegBase\Classes" (real use: HKCU:\Software).
# Returns the previous default of .vrn (or $null) so uninstall can put it back.
function Register-VrnFileType([string]$RegBase, [string]$Exe) {
    $ext = "$RegBase\Classes\.vrn"
    $prev = $null
    if (Test-Path -LiteralPath $ext) { $prev = (Get-Item -LiteralPath $ext).GetValue('') }
    New-Item -Force -Path $ext | Out-Null
    Set-Item -LiteralPath $ext -Value 'Vernon.Project'
    $cls = "$RegBase\Classes\Vernon.Project"
    New-Item -Force -Path $cls | Out-Null
    Set-Item -LiteralPath $cls -Value 'Vernon project'
    New-Item -Force -Path "$cls\DefaultIcon" | Out-Null
    Set-Item -LiteralPath "$cls\DefaultIcon" -Value "`"$Exe`",0"
    New-Item -Force -Path "$cls\shell\open\command" | Out-Null
    Set-Item -LiteralPath "$cls\shell\open\command" -Value "`"$Exe`" `"%1`""
    $prev
}
function Unregister-VrnFileType([string]$RegBase, $Previous) {
    $cls = "$RegBase\Classes\Vernon.Project"
    if (Test-Path -LiteralPath $cls) { Remove-Item -LiteralPath $cls -Recurse -Force }
    $ext = "$RegBase\Classes\.vrn"
    if (Test-Path -LiteralPath $ext) {
        $cur = (Get-Item -LiteralPath $ext).GetValue('')
        if ($cur -eq 'Vernon.Project') {
            if ($Previous) { Set-Item -LiteralPath $ext -Value $Previous } else { Remove-Item -LiteralPath $ext -Recurse -Force }
        }
    }
}
