#!/usr/bin/env bash
# Live probes of wp9-replication-solve-env.sh inside the approved distribution.
# Each probe sources the reviewed environment in its own subshell and reads the
# limits it applied. The strings in _vernon_tool_argv are NEVER run here.
# Usage: bash test-wp9-solve-environment.sh /absolute/path/to/wp9-replication-solve-env.sh
if [[ ${1-} != /* || ! -r ${1-} ]]; then
    builtin printf '%s\n' 'Usage: bash test-wp9-solve-environment.sh /absolute/path/to/wp9-replication-solve-env.sh' >&2
    exit 2
fi
readonly _wp9_environment=$1
if builtin declare -p _vernon_tool_argv >/dev/null 2>&1; then
    builtin printf '%s\n' 'Start the probe driver in a fresh Bash without _vernon_tool_argv.' >&2
    exit 2
fi
_wp9_memory_ready() {
    local _key _value _rest
    while IFS=$' \t' builtin read -r _key _value _rest; do
        if [[ $_key == MemAvailable: ]]; then
            [[ $_value =~ ^[0-9]{1,12}$ ]] && ((10#$_value >= 24117248)) && return 0
            builtin printf 'PRECONDITION: actual MemAvailable=%s KiB; need at least 24117248.\n' "$_value" >&2
            return 77
        fi
    done < /proc/meminfo
    return 77
}
# Accepted: prints "<soft> <hard> <tool-on-PATH>" from inside the sourced scope.
_wp9_accepted() (
    readonly -a _vernon_tool_argv=("$@")
    . "$_wp9_environment" 2>/dev/null || exit 1
    builtin printf '%s %s %s\n' "$(builtin ulimit -S -v)" "$(builtin ulimit -H -v)" "$(command -v -- "$1" >/dev/null && echo found || echo missing)"
)
_wp9_denied() (
    readonly -a _vernon_tool_argv=("$@")
    . "$_wp9_environment" >/dev/null 2>&1 && exit 1
    exit 0
)
passed=0
failed=0
accept() {
    local expected=$1 actual
    shift
    _wp9_memory_ready || exit 77
    actual=$(_wp9_accepted "$@")
    if [[ $actual == "$expected $expected found" ]]; then
        passed=$((passed + 1))
    else
        printf 'FAIL accepted %s: got <%s>, want <%s %s found>\n' "$*" "$actual" "$expected" "$expected"
        failed=$((failed + 1))
    fi
}
deny() {
    if _wp9_denied "$@"; then
        passed=$((passed + 1))
    else
        printf 'FAIL denied %s was accepted\n' "$*"
        failed=$((failed + 1))
    fi
}
for tool in surfaceFeatureExtract blockMesh checkMesh topoSet potentialFoam; do
    accept 23068672 "$tool"
done
accept 23068672 snappyHexMesh -overwrite
accept 23068672 decomposePar -force
accept 23068672 reconstructPar -latestTime
accept 4194304 mpirun -np 16 simpleFoam -parallel
deny simpleFoam
deny mpirun -np 8 simpleFoam -parallel
deny mpirun -np 2 snappyHexMesh -overwrite -parallel
deny reconstructParMesh -constant
deny bash -c true
# A missing or mutable tool array is refused before anything is sourced.
if ( . "$_wp9_environment" >/dev/null 2>&1 ); then printf '%s\n' 'FAIL missing array accepted'; failed=$((failed + 1)); else passed=$((passed + 1)); fi
if ( declare -a _vernon_tool_argv=(blockMesh); . "$_wp9_environment" >/dev/null 2>&1 ); then printf '%s\n' 'FAIL mutable array accepted'; failed=$((failed + 1)); else passed=$((passed + 1)); fi
# An existing tighter limit is never raised.
if ( ulimit -S -v 1048576; readonly -a _vernon_tool_argv=(potentialFoam); . "$_wp9_environment" >/dev/null 2>&1 ); then printf '%s\n' 'FAIL raised a tighter limit'; failed=$((failed + 1)); else passed=$((passed + 1)); fi
printf 'WP9_SOLVE_ENVIRONMENT_TESTS passed=%d failed=%d tools_started=0\n' "$passed" "$failed"
[[ $failed == 0 ]]
