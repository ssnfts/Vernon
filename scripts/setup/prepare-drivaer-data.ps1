#requires -Version 7.2
<#
Extracts the eight TUM DrivAer surfaces the WP9 replication uses from the
registered STL_complete.zip, checking the archive and every extracted part
against the SHA-256 values in docs/research/wp9-replication-baseline-v1.json.
The archive is licensed to whoever downloaded it: never commit or redistribute
it or the extracted files (scratch/ is ignored by git).

  .\scripts\setup\prepare-drivaer-data.ps1 -Archive D:\downloads\STL_complete.zip

Prints the folder to use as VERNON_CAR_DIR.
#>
[CmdletBinding()]
param([Parameter(Mandatory)][string]$Archive)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.IO.Compression.FileSystem
$repo = [IO.Path]::GetFullPath((Split-Path (Split-Path $PSScriptRoot -Parent) -Parent))
$contract = Get-Content -Raw -LiteralPath (Join-Path $repo 'docs/research/wp9-replication-baseline-v1.json') | ConvertFrom-Json
$geometry = @($contract.PSObject.Properties.Value | Where-Object { $_ -and $_.PSObject.Properties['archive_sha256'] })[0]
if (-not $geometry) { throw 'The baseline contract has no archive_sha256/components block.' }
$target = Join-Path $repo 'scratch/wp9-data/tum-drivaer/extracted'

Write-Host "Checking archive SHA-256 (590 MB, about a minute)..."
$archiveHash = (Get-FileHash -LiteralPath $Archive -Algorithm SHA256).Hash.ToLowerInvariant()
if ($archiveHash -cne $geometry.archive_sha256) { throw "Archive SHA-256 $archiveHash does not match the recorded $($geometry.archive_sha256)." }

[void][IO.Directory]::CreateDirectory($target)
$zip = [IO.Compression.ZipFile]::OpenRead($Archive)
try {
    foreach ($component in $geometry.components) {
        $out = Join-Path $target $component.file
        if ((Test-Path -LiteralPath $out) -and (Get-FileHash -LiteralPath $out -Algorithm SHA256).Hash.ToLowerInvariant() -ceq $component.sha256) {
            Write-Host "ok (already present)  $($component.file)"; continue
        }
        # The archive holds two copies of each part (STL_complete/ and STL_complete_NewPID/); keep the one whose bytes match.
        $found = $false
        foreach ($entry in @($zip.Entries | Where-Object { $_.Name -ceq $component.file })) {
            [IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $out, $true)
            if ((Get-FileHash -LiteralPath $out -Algorithm SHA256).Hash.ToLowerInvariant() -ceq $component.sha256) { $found = $true; break }
        }
        if (-not $found) { Remove-Item -LiteralPath $out -ErrorAction SilentlyContinue; throw "No archive entry for $($component.file) matches its recorded SHA-256." }
        Write-Host "ok  $($component.file)"
    }
} finally { $zip.Dispose() }
Write-Host "All $(@($geometry.components).Count) parts verified. Use:"
Write-Host "  `$env:VERNON_CAR_DIR = '$target'"
