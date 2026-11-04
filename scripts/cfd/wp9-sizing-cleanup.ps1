#requires -Version 7.2
# Dot-source only. All public cleanup calls are restricted to this checkout's
# owned verification runs and the approved explicit Linux identity. Prelaunch
# .intent files must be durable before Rust starts wsl.exe: a scanner cannot
# recover an attempt token which existed only in a terminated process's memory.
$script:Wp9SizingCleanupRepo = [IO.Path]::GetFullPath((Split-Path (Split-Path $PSScriptRoot -Parent) -Parent))
$script:Wp9SizingTokenPattern = 'vernon-cfd-wsl-[1-9][0-9]{0,9}-[0-9a-f]{1,8}-(?:0|[1-9][0-9]{0,19})'

function Get-Wp9SizingCanonicalPath([string]$Path) {
    if ($Path -cnotmatch '\A[A-Za-z]:[\\/]') { throw 'Cleanup requires an absolute local drive path.' }
    $normalized = $Path.Replace('/', '\')
    if ($normalized.Length -gt 3) { $normalized = $normalized.TrimEnd('\') }
    foreach ($part in $normalized.Substring(3).Split('\')) {
        if ($part -eq '' -and $normalized.Length -eq 3) { continue }
        if (-not $part -or $part -in @('.', '..') -or $part -match '[\x00-\x1f<>:"|?*]' -or $part.EndsWith('.') -or $part.EndsWith(' ') -or $part -match '^(?i:CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])(?:\.|$)') {
            throw 'Cleanup refuses ambiguous, special or parent path components.'
        }
    }
    return [IO.Path]::GetFullPath($normalized)
}

function ConvertTo-Wp9SizingLinuxPath([string]$Path) {
    $canonical = Get-Wp9SizingCanonicalPath $Path
    $suffix = $canonical.Substring(3).Replace('\', '/')
    $result = '/mnt/' + $canonical.Substring(0, 1).ToLowerInvariant()
    if ($suffix) { $result += '/' + $suffix }
    return $result
}

function ConvertFrom-Wp9SizingControlRecord([string]$FileName, [string]$RecordText) {
    $nameMatch = [regex]::Match($FileName, '\A\.(' + $script:Wp9SizingTokenPattern + ')\.pid\z', [Text.RegularExpressions.RegexOptions]::CultureInvariant)
    if (-not $nameMatch.Success) { throw 'Control filename is not an exact published extproc record.' }
    if ([Text.Encoding]::UTF8.GetByteCount($RecordText) -gt 4096) { throw 'Control record exceeds 4096 bytes.' }
    $recordMatch = [regex]::Match($RecordText, '\A([^ \r\n\x00]+) ([1-9][0-9]*) ([0-9]+) ([1-9][0-9]*)\n\z', [Text.RegularExpressions.RegexOptions]::CultureInvariant)
    if (-not $recordMatch.Success -or $recordMatch.Groups[1].Value -cne $nameMatch.Groups[1].Value -or $recordMatch.Groups[2].Value -cne $recordMatch.Groups[4].Value) {
        throw 'Control record token, group leader or field count is invalid.'
    }
    $leader = 0
    $startTicks = [uint64]0
    if (-not [int]::TryParse($recordMatch.Groups[2].Value, [Globalization.NumberStyles]::None, [Globalization.CultureInfo]::InvariantCulture, [ref]$leader) -or $leader -le 0 -or -not [uint64]::TryParse($recordMatch.Groups[3].Value, [Globalization.NumberStyles]::None, [Globalization.CultureInfo]::InvariantCulture, [ref]$startTicks)) {
        throw 'Control record identity is out of range.'
    }
    return [pscustomobject]@{ Token = $nameMatch.Groups[1].Value; ProcessId = $leader; StartTicks = $recordMatch.Groups[3].Value; GroupId = $leader }
}

function Assert-Wp9SizingOrdinaryDirectory([string]$Path) {
    $cursor = Get-Wp9SizingCanonicalPath $Path
    while ($cursor) {
        $attributes = [IO.File]::GetAttributes($cursor)
        if (($attributes -band [IO.FileAttributes]::ReparsePoint) -or -not ($attributes -band [IO.FileAttributes]::Directory) -or ($attributes -band [IO.FileAttributes]::Device)) {
            throw "Cleanup requires ordinary directory ancestors: $cursor"
        }
        $cursor = [IO.Path]::GetDirectoryName($cursor)
    }
}

function Read-Wp9SizingControlText([string]$Path) {
    Assert-Wp9SizingOrdinaryDirectory ([IO.Path]::GetDirectoryName($Path))
    $attributes = [IO.File]::GetAttributes($Path)
    if ($attributes -band ([IO.FileAttributes]::ReparsePoint -bor [IO.FileAttributes]::Directory -bor [IO.FileAttributes]::Device)) { throw 'Control records must be ordinary files.' }
    $stream = [IO.File]::Open($Path, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
    try {
        if ($stream.Length -gt 4096) { throw 'Control record exceeds 4096 bytes.' }
        $bytes = [byte[]]::new(4097)
        $count = 0
        while ($count -lt $bytes.Length) {
            $read = $stream.Read($bytes, $count, $bytes.Length - $count)
            if ($read -eq 0) { break }
            $count += $read
        }
        if ($count -gt 4096) { throw 'Control record exceeds 4096 bytes.' }
        return [pscustomobject]@{ Text = ([Text.UTF8Encoding]::new($false, $true)).GetString($bytes, 0, $count); Bytes = $count }
    } finally { $stream.Dispose() }
}

function Get-Wp9SizingCleanupPlan([string]$RunDirectory, [string]$Distribution, [string]$User) {
    if ($Distribution -cne 'Vernon-Ubuntu-24.04' -or $User -cne 'vernon') { throw 'Cleanup requires the approved exact distribution and user.' }
    $run = Get-Wp9SizingCanonicalPath $RunDirectory
    $runs = Join-Path $script:Wp9SizingCleanupRepo 'scratch/continuation/runs'
    if (-not [IO.Path]::GetDirectoryName($run).Equals($runs, [StringComparison]::OrdinalIgnoreCase) -or [IO.Path]::GetFileName($run) -cnotmatch '\A[A-Za-z0-9][A-Za-z0-9_-]{0,95}\z') {
        throw 'Cleanup requires one exact owned verification run directory.'
    }
    Assert-Wp9SizingOrdinaryDirectory $run
    $temp = Join-Path $run 'temp'
    if (-not (Test-Path -LiteralPath $temp)) { return @() }
    Assert-Wp9SizingOrdinaryDirectory $temp
    $pending = [Collections.Generic.Stack[object]]::new()
    $pending.Push([pscustomobject]@{ Path = $temp; Depth = 0 })
    $attempts = [Collections.Generic.Dictionary[string, object]]::new([StringComparer]::Ordinal)
    $entries = 0
    $readBytes = 0
    while ($pending.Count -gt 0) {
        $directory = $pending.Pop()
        Assert-Wp9SizingOrdinaryDirectory $directory.Path
        foreach ($entry in [IO.Directory]::EnumerateFileSystemEntries($directory.Path)) {
            $entries++
            if ($entries -gt 32768) { throw 'Cleanup temp subtree exceeds its entry budget.' }
            $attributes = [IO.File]::GetAttributes($entry)
            if ($attributes -band ([IO.FileAttributes]::ReparsePoint -bor [IO.FileAttributes]::Device)) { throw 'Cleanup refuses reparse points and special files in temp.' }
            if ($attributes -band [IO.FileAttributes]::Directory) {
                if ($directory.Depth -ge 16) { throw 'Cleanup temp subtree exceeds its depth budget.' }
                $pending.Push([pscustomobject]@{ Path = $entry; Depth = $directory.Depth + 1 })
                continue
            }
            $file = [IO.Path]::GetFileName($entry)
            $match = [regex]::Match($file, '\A\.(' + $script:Wp9SizingTokenPattern + ')\.(pid(?:\.tmp)?|cancel|intent)\z', [Text.RegularExpressions.RegexOptions]::CultureInvariant)
            if (-not $match.Success) { continue }
            $attemptToken = $match.Groups[1].Value
            $parent = [IO.Path]::GetDirectoryName($entry)
            $recordPath = Join-Path $parent ".$attemptToken.pid"
            if (-not $attempts.ContainsKey($attemptToken)) {
                if ($attempts.Count -ge 128) { throw 'Cleanup exceeds its attempt budget.' }
                $cancelPath = Join-Path $parent ".$attemptToken.cancel"
                $attempts.Add($attemptToken, [pscustomobject]@{
                    Token = $attemptToken; RecordPath = $recordPath; CancelPath = $cancelPath
                    LinuxRecordPath = ConvertTo-Wp9SizingLinuxPath $recordPath
                    LinuxCancelPath = ConvertTo-Wp9SizingLinuxPath $cancelPath
                    Identity = $null
                })
            } elseif ($attempts[$attemptToken].RecordPath -cne $recordPath) { throw 'The same attempt token appears in multiple control directories.' }
            $data = Read-Wp9SizingControlText $entry
            $readBytes += $data.Bytes
            if ($readBytes -gt 1048576) { throw 'Cleanup exceeds its total control read budget.' }
            if ($match.Groups[2].Value -ceq 'intent') {
                if ($data.Text -cne "$attemptToken`n") { throw 'Prelaunch intent does not match its fixed filename token.' }
            } elseif ($match.Groups[2].Value -cne 'cancel') {
                $identity = ConvertFrom-Wp9SizingControlRecord -FileName ".$attemptToken.pid" -RecordText $data.Text
                $previous = $attempts[$attemptToken].Identity
                if ($previous -and ($previous.ProcessId -ne $identity.ProcessId -or $previous.StartTicks -cne $identity.StartTicks)) { throw 'Published and pending records disagree about their process identity.' }
                $attempts[$attemptToken].Identity = $identity
            }
        }
    }
    return @($attempts.Values | Sort-Object -Property RecordPath)
}

# Original fixed WSL_CLEANUP protocol from this repository's extproc.rs. Paths
# and tokens are positional arguments, never interpolated into shell source.
$script:Wp9SizingLinuxCleanup = @'
_vernon_record=$1
_vernon_cancel=$2
_vernon_expected=$3
: > "$_vernon_cancel" || exit 129
_vernon_stopped() {
    printf 'VERNON_WSL_GROUP_STOPPED %s\n' "$_vernon_expected"
}
[[ -e "$_vernon_record" ]] || { _vernon_stopped; exit 0; }
IFS=' ' read -r _vernon_token _vernon_pid _vernon_start _vernon_group _vernon_extra < "$_vernon_record" || exit 130
[[ "$_vernon_token" == "$_vernon_expected" && -z "$_vernon_extra" ]] || exit 130
[[ "$_vernon_pid" =~ ^[1-9][0-9]*$ && "$_vernon_start" =~ ^[0-9]+$ && "$_vernon_group" == "$_vernon_pid" ]] || exit 130
_vernon_valid_leader=0
if [[ -r "/proc/$_vernon_pid/stat" ]]; then
    IFS= read -r _vernon_stat < "/proc/$_vernon_pid/stat" || exit 131
    _vernon_stat_tail=${_vernon_stat##*) }
    IFS=' ' read -r -a _vernon_fields <<< "$_vernon_stat_tail"
    [[ ${_vernon_fields[19]:-} == "$_vernon_start" && ${_vernon_fields[2]:-} == "$_vernon_group" && ${_vernon_fields[3]:-} == "$_vernon_group" ]] || exit 131
    _vernon_valid_leader=1
fi
_vernon_scan_group() {
    _vernon_live=0
    _vernon_owned=$_vernon_valid_leader
    _vernon_scanned=0
    for _vernon_stat_file in /proc/[0-9]*/stat; do
        ((_vernon_scanned+=1))
        (( _vernon_scanned <= 32768 )) || return 1
        IFS= read -r _vernon_stat < "$_vernon_stat_file" 2>/dev/null || continue
        _vernon_stat_tail=${_vernon_stat##*) }
        IFS=' ' read -r -a _vernon_fields <<< "$_vernon_stat_tail"
        [[ ${_vernon_fields[2]:-} == "$_vernon_group" && ${_vernon_fields[3]:-} == "$_vernon_group" ]] || continue
        [[ ${_vernon_fields[0]:-} != Z && ${_vernon_fields[0]:-} != X ]] || continue
        _vernon_live=1
        if (( _vernon_owned == 0 )); then
            _vernon_environ="${_vernon_stat_file%/stat}/environ"
            if [[ -r "$_vernon_environ" ]]; then
                while IFS= read -r -d '' _vernon_entry; do
                    if [[ "$_vernon_entry" == "VERNON_CFD_ATTEMPT=$_vernon_expected" ]]; then
                        _vernon_owned=1
                        break
                    fi
                done < "$_vernon_environ" 2>/dev/null
            fi
        fi
    done
}
_vernon_scan_group || exit 132
(( _vernon_live != 0 )) || { _vernon_stopped; exit 0; }
(( _vernon_owned != 0 )) || exit 133
if ! kill -KILL -- "-$_vernon_group" 2>/dev/null; then
    _vernon_scan_group || exit 132
    (( _vernon_live == 0 )) || exit 134
fi
for _vernon_poll in {1..20}; do
    _vernon_scan_group || exit 132
    if (( _vernon_live == 0 )); then
        _vernon_stopped
        exit 0
    fi
    /bin/sleep 0.05
done
exit 135
'@

function Stop-Wp9SizingLinuxGroups([string]$RunDirectory, [string]$Distribution, [string]$User) {
    # Plan every target before the first cancellation write or WSL invocation.
    $plans = @(Get-Wp9SizingCleanupPlan $RunDirectory $Distribution $User)
    $receipts = [Collections.Generic.List[object]]::new()
    $deadline = [DateTime]::UtcNow.AddSeconds(60)
    foreach ($plan in $plans) {
        Assert-Wp9SizingOrdinaryDirectory ([IO.Path]::GetDirectoryName($plan.RecordPath))
        if ([IO.File]::Exists($plan.RecordPath)) { [void](ConvertFrom-Wp9SizingControlRecord ([IO.Path]::GetFileName($plan.RecordPath)) (Read-Wp9SizingControlText $plan.RecordPath).Text) }
        if (Test-Path -LiteralPath $plan.CancelPath) { [void](Read-Wp9SizingControlText $plan.CancelPath) }
        # Create the tombstone on the host first, including intent-only attempts.
        # This closes late publication before a potentially slow WSL cleanup.
        $cancel = [IO.File]::Open($plan.CancelPath, [IO.FileMode]::OpenOrCreate, [IO.FileAccess]::Write, [IO.FileShare]::Read)
        try { $cancel.Flush($true) } finally { $cancel.Dispose() }
        $remaining = [int][Math]::Min(10000, [Math]::Floor(($deadline - [DateTime]::UtcNow).TotalMilliseconds))
        if ($remaining -le 0) { throw 'Sizing Linux cleanup exceeded its total 60-second deadline.' }
        $start = [Diagnostics.ProcessStartInfo]::new()
        $start.FileName = Join-Path ([Environment]::GetFolderPath('Windows')) 'System32/wsl.exe'
        $start.UseShellExecute = $false
        $start.CreateNoWindow = $true
        $start.RedirectStandardOutput = $true
        $start.RedirectStandardError = $true
        $start.WorkingDirectory = $script:Wp9SizingCleanupRepo
        foreach ($argument in @('--distribution', $Distribution, '--user', $User, '--exec', '/usr/bin/env', '-u', 'BASH_ENV', '-u', 'ENV', '--', '/bin/bash', '--noprofile', '--norc', '-c', $script:Wp9SizingLinuxCleanup, 'vernon-cfd-cleanup', $plan.LinuxRecordPath, $plan.LinuxCancelPath, $plan.Token)) { $start.ArgumentList.Add($argument) }
        $process = [Diagnostics.Process]::new()
        $process.StartInfo = $start
        try {
            if (-not $process.Start()) { throw 'Could not start the explicit WSL cleanup client.' }
            $stdoutTask = $process.StandardOutput.ReadToEndAsync()
            $stderrTask = $process.StandardError.ReadToEndAsync()
            if (-not $process.WaitForExit($remaining)) {
                # Stop only this exact client. A timeout never certifies Linux
                # cleanup and never escalates to distro-wide process actions.
                $process.Kill()
                [void]$process.WaitForExit(1000)
                throw "Sizing Linux cleanup was not verified (client timeout): $($plan.Token)"
            }
            if (-not [Threading.Tasks.Task]::WaitAll([Threading.Tasks.Task[]]@($stdoutTask, $stderrTask), 1000)) { throw 'WSL cleanup output capture timed out.' }
            $stdout = $stdoutTask.GetAwaiter().GetResult()
            $stderr = $stderrTask.GetAwaiter().GetResult()
            $expected = 'VERNON_WSL_GROUP_STOPPED ' + $plan.Token
            if ($stdout.Length -gt 16384 -or $stderr.Length -gt 16384 -or $process.ExitCode -ne 0 -or $expected -cnotin ($stdout -split '\r?\n')) { throw "Sizing Linux cleanup was not verified (exit $($process.ExitCode), missing or invalid receipt): $($plan.Token)" }
            $receipts.Add([pscustomobject]@{ token = $plan.Token; exitCode = 0; marker = $expected })
        } finally { $process.Dispose() }
    }
    return [pscustomobject]@{ success = $true; attempted = $plans.Count; receipts = @($receipts.ToArray()) }
}
