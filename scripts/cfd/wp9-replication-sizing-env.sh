#!/usr/bin/env bash
# Sourced by the WSL runner for one mesh-sizing command, never for a solver.
# Caps are per-process virtual address space, not a reservation or an aggregate
# RSS guarantee. The owner must also enforce the six-hour mesh-only deadline.

# Pure policy: MEM_AVAILABLE_KIB ARGV... -> cap KiB on stdout, or rejection.
# Production obtains memory from /proc/meminfo; no environment override exists.
_vernon_replication_sizing_policy() {
    local _available=${1-} _cap=23068672
    if [[ ! $_available =~ ^[0-9]{1,12}$ ]] || ((10#$_available < 24117248)); then
        builtin printf '%s\n' 'WP9 sizing: MemAvailable must be a valid count of at least 24117248 KiB (23 GiB).' >&2
        return 126
    fi
    shift
    case ${1-} in
        blockMesh|surfaceFeatureExtract|checkMesh)
            if (($# != 1)); then
                builtin printf '%s\n' 'WP9 sizing: unexpected arguments to a serial mesh utility.' >&2
                return 126
            fi
            ;;
        decomposePar)
            if (($# != 2)) || [[ ${2-} != -force ]]; then
                builtin printf '%s\n' 'WP9 sizing: expected decomposePar -force.' >&2
                return 126
            fi
            ;;
        reconstructParMesh)
            if (($# != 2)) || [[ ${2-} != -constant ]]; then
                builtin printf '%s\n' 'WP9 sizing: expected reconstructParMesh -constant.' >&2
                return 126
            fi
            ;;
        snappyHexMesh)
            if (($# != 2)) || [[ ${2-} != -overwrite ]]; then
                builtin printf '%s\n' 'WP9 sizing: expected serial snappyHexMesh -overwrite.' >&2
                return 126
            fi
            ;;
        mpirun)
            if (($# != 6)) || [[ ${2-} != -np || (${3-} != 1 && ${3-} != 2) || ${4-} != snappyHexMesh || ${5-} != -overwrite || ${6-} != -parallel ]]; then
                builtin printf '%s\n' 'WP9 sizing: MPI permits only -np 1 or 2 snappyHexMesh -overwrite -parallel.' >&2
                return 126
            fi
            _cap=11534336
            ;;
        *)
            builtin printf '%s\n' 'WP9 sizing: unknown or forbidden program; solver execution is not permitted.' >&2
            return 126
            ;;
    esac
    builtin printf '%s\n' "$_cap"
}

_vernon_replication_sizing_environment() {
    local _origin=$PWD _environment_ok=0 _declaration _count _index
    local _key _value _unit _extra _memory= _memory_records=0 _cap _soft _hard

    # A function-local positional scope contains any changes made by bashrc.
    # The readonly tool array belongs to WSL_START and is never rewritten here.
    if [[ -r /usr/lib/openfoam/openfoam2606/etc/bashrc ]]; then
        if . /usr/lib/openfoam/openfoam2606/etc/bashrc; then
            _environment_ok=1
        fi
    fi
    if ! builtin cd -- "$_origin"; then
        builtin printf '%s\n' 'WP9 sizing: could not restore the caller working directory.' >&2
        return 126
    fi
    if [[ $_environment_ok != 1 ]]; then
        builtin printf '%s\n' 'WP9 sizing: the approved environment could not be sourced.' >&2
        return 126
    fi

    if ! _declaration=$(builtin declare -p _vernon_tool_argv 2>/dev/null); then
        builtin printf '%s\n' 'WP9 sizing: missing runner tool array.' >&2
        return 126
    fi
    if [[ $_declaration != 'declare -ar _vernon_tool_argv='* ]]; then
        builtin printf '%s\n' 'WP9 sizing: runner tool arguments must be a readonly indexed array.' >&2
        return 126
    fi
    _count=${#_vernon_tool_argv[@]}
    if ((_count == 0)); then
        builtin printf '%s\n' 'WP9 sizing: empty runner tool array.' >&2
        return 126
    fi
    for ((_index=0; _index<_count; _index++)); do
        if [[ ${_vernon_tool_argv[_index]+present} != present || -z ${_vernon_tool_argv[_index]-} ]]; then
            builtin printf '%s\n' 'WP9 sizing: runner tool arguments must be dense and nonempty.' >&2
            return 126
        fi
    done

    if [[ ! -r /proc/meminfo ]]; then
        builtin printf '%s\n' 'WP9 sizing: /proc/meminfo is unreadable.' >&2
        return 126
    fi
    while IFS=$' \t' builtin read -r _key _value _unit _extra; do
        if [[ $_key == MemAvailable: ]]; then
            _memory_records=$((_memory_records + 1))
            if [[ $_unit != kB || -n $_extra ]] || ((_memory_records != 1)); then
                builtin printf '%s\n' 'WP9 sizing: malformed or duplicate MemAvailable record.' >&2
                return 126
            fi
            _memory=$_value
        fi
    done < /proc/meminfo || {
        builtin printf '%s\n' 'WP9 sizing: failed to read /proc/meminfo.' >&2
        return 126
    }
    if ((_memory_records != 1)); then
        builtin printf '%s\n' 'WP9 sizing: MemAvailable record is missing.' >&2
        return 126
    fi
    _cap=$(_vernon_replication_sizing_policy "$_memory" "${_vernon_tool_argv[@]}") || return 126

    _soft=$(builtin ulimit -S -v) && _hard=$(builtin ulimit -H -v) || {
        builtin printf '%s\n' 'WP9 sizing: address-space limits could not be read.' >&2
        return 126
    }
    for _value in "$_soft" "$_hard"; do
        if [[ $_value == unlimited ]]; then
            continue
        fi
        if [[ ! $_value =~ ^[0-9]{1,17}$ ]] || ((10#$_value < _cap)); then
            builtin printf '%s\n' 'WP9 sizing: refusing to raise or replace an invalid existing address-space limit.' >&2
            return 126
        fi
    done
    if ! builtin ulimit -S -v "$_cap" || ! builtin ulimit -H -v "$_cap"; then
        builtin printf '%s\n' 'WP9 sizing: failed to apply both address-space limits.' >&2
        return 126
    fi
    _soft=$(builtin ulimit -S -v) && _hard=$(builtin ulimit -H -v) || return 126
    if [[ $_soft != "$_cap" || $_hard != "$_cap" ]]; then
        builtin printf '%s\n' 'WP9 sizing: applied address-space limits do not match the policy.' >&2
        return 126
    fi
    builtin printf 'WP9 sizing: MemAvailable=%s KiB; RLIMIT_AS soft=hard=%s KiB; tool=%s; no tool started by environment.\n' "$_memory" "$_cap" "${_vernon_tool_argv[0]}" >&2
    return 0
}

_vernon_replication_sizing_environment
