#requires -Version 7.2
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'replication-inputs.ps1')
function Assert-InputCheck($Condition, [string]$Why) { if (-not $Condition) { throw $Why } }
$testRoot = Join-Path (Split-Path (Split-Path $PSScriptRoot -Parent) -Parent)('scratch/continuation/input-check-' + [guid]::NewGuid().ToString('N'))
[void][IO.Directory]::CreateDirectory($testRoot)
$testFile = Join-Path $testRoot 'part_01_Test.stl'
[IO.File]::WriteAllText($testFile, 'abc', [Text.UTF8Encoding]::new($false))
$entry = [pscustomobject]@{file='part_01_Test.stl';bytes=3;sha256='ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad'}
$lease = $null
try {
    $lease = Open-ReplicationInputLease $testRoot @($entry)
    Assert-InputCheck ($lease.receipts.Count -eq 1 -and $lease.receipts[0].sha256 -eq $entry.sha256) 'Known independent SHA256 identity was not recorded.'
    Assert-InputCheck ([IO.File]::ReadAllText($testFile) -eq 'abc') 'Child-style read was blocked by the input lease.'
    $writeBlocked = $false
    try { [IO.File]::AppendAllText($testFile, 'd') } catch [IO.IOException] { $writeBlocked = $true }
    Assert-InputCheck $writeBlocked 'Verified input remained writable while the child was reading it.'
} finally { Close-ReplicationInputLease $lease }
[IO.File]::WriteAllText($testFile, 'abd', [Text.UTF8Encoding]::new($false))
$rejected = $false
try { $unexpected = Open-ReplicationInputLease $testRoot @($entry); Close-ReplicationInputLease $unexpected } catch { $rejected = $_.Exception.Message -like '*checksum mismatch*' }
Assert-InputCheck $rejected 'Same-length tampered source passed the checksum check.'
# A rejected acquisition must release its earlier handles as well.
[IO.File]::WriteAllText($testFile, 'abc', [Text.UTF8Encoding]::new($false))
$badPath = [pscustomobject]@{file='../part_01_Test.stl';bytes=3;sha256=$entry.sha256}
$rejected = $false
try { $unexpected = Open-ReplicationInputLease $testRoot @($entry,$badPath); Close-ReplicationInputLease $unexpected } catch { $rejected = $true }
Assert-InputCheck $rejected 'Path traversal component was accepted.'
[IO.File]::AppendAllText($testFile, 'd')
Write-Output 'PASS: source identity, concurrent reads, write exclusion, tamper refusal and failed-acquisition cleanup'
