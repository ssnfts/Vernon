# Local read-only data leases for the explicitly invoked geometry check.
# Holding FileShare.Read prevents mutation/deletion while the child reads assets.
function Open-ReplicationInputLease([string]$Directory, [object[]]$Entries) {
    if ([string]::IsNullOrWhiteSpace($Directory) -or $Entries.Count -lt 1 -or $Entries.Count -gt 32) { throw 'Explicit replication inputs are required.' }
    $root = [IO.Path]::GetFullPath($Directory)
    $cursor = $root
    while ($cursor) {
        $item = Get-Item -LiteralPath $cursor -Force -ErrorAction Stop
        if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Replication inputs cannot use linked ancestors.' }
        $cursor = Split-Path $cursor -Parent
    }
    $streams = [Collections.Generic.List[IO.FileStream]]::new()
    $receipts = [Collections.Generic.List[object]]::new()
    try {
        foreach ($entry in $Entries) {
            $name = [string]$entry.file
            if ($name -cnotmatch '^part_[0-9]{2}_[A-Za-z_]+\.stl$' -or $entry.bytes -le 0 -or $entry.bytes -gt 536870912 -or $entry.sha256 -cnotmatch '^[a-f0-9]{64}$') { throw 'Malformed replication component identity.' }
            $path = Join-Path $root $name
            $item = Get-Item -LiteralPath $path -Force -ErrorAction Stop
            if ($item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'Replication component must be an ordinary file.' }
            $stream = [IO.File]::Open($path, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
            $streams.Add($stream)
            if ($stream.Length -ne $entry.bytes) { throw "Replication component length mismatch: $name" }
            $hash = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($stream)).ToLowerInvariant()
            if ($hash -cne $entry.sha256) { throw "Replication component checksum mismatch: $name" }
            $receipts.Add([ordered]@{file=$name;bytes=$stream.Length;sha256=$hash})
        }
        return [pscustomobject]@{streams=$streams;receipts=$receipts.ToArray()}
    } catch {
        foreach ($stream in $streams) { $stream.Dispose() }
        throw
    }
}
function Close-ReplicationInputLease($Lease) {
    if ($Lease) { foreach ($stream in $Lease.streams) { $stream.Dispose() } }
}
