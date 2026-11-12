#!/usr/bin/env bash
# Pure policy checks for wp9-diagnostic-fixture-env.sh. Starts no tool.
# Usage: bash test-wp9-fixture-policy.sh /absolute/path/to/wp9-diagnostic-fixture-env.sh
readonly -a _vernon_tool_argv=(mpirun -np 16 simpleFoam -parallel)
if . "$1" >/dev/null 2>&1; then
    printf '%s\n' 'FAIL: fixture environment accepted the 16-rank production solver'
    exit 1
fi
declare -F _vernon_diagnostic_fixture_policy >/dev/null || exit 2
passed=0
failed=0
ok() {
    local expected=$1 available=$2 actual
    shift 2
    if actual=$(_vernon_diagnostic_fixture_policy "$available" "$@" 2>/dev/null) && [[ $actual == "$expected" ]]; then
        passed=$((passed + 1))
    else
        printf 'FAIL expected cap=%s available=%s argv=' "$expected" "$available"
        printf ' <%s>' "$@"
        printf '\n'
        failed=$((failed + 1))
    fi
}
bad() {
    if _vernon_diagnostic_fixture_policy "$@" >/dev/null 2>&1; then
        printf 'FAIL accepted rejection case:'
        printf ' <%s>' "$@"
        printf '\n'
        failed=$((failed + 1))
    else
        passed=$((passed + 1))
    fi
}
ok 4194304 8388608 blockMesh
ok 4194304 8388608 simpleFoam
ok 4194304 8388608 decomposePar -force
ok 4194304 8388608 mpirun -np 4 simpleFoam -parallel
ok 4194304 26000000 simpleFoam
bad 8388607 simpleFoam
bad '' simpleFoam
bad 12x simpleFoam
bad 8388608 simpleFoam -parallel
bad 8388608 blockMesh -dict x
bad 8388608 decomposePar
bad 8388608 decomposePar -copyZero
bad 8388608 mpirun -np 16 simpleFoam -parallel
bad 8388608 mpirun -np 4 simpleFoam
bad 8388608 mpirun -np 4 potentialFoam -parallel
bad 8388608 mpirun -np 4 simpleFoam -parallel -x
bad 8388608 snappyHexMesh -overwrite
bad 8388608 potentialFoam
bad 8388608 reconstructPar -latestTime
bad 8388608 bash -c true
bad 4194304
printf 'WP9_FIXTURE_POLICY_TESTS passed=%s failed=%s\n' "$passed" "$failed"
((failed == 0))
