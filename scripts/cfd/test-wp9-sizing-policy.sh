#!/usr/bin/env bash
readonly -a _vernon_tool_argv=(simpleFoam)
if . "$1" >/dev/null 2>&1; then
    printf '%s\n' 'FAIL: environment accepted simpleFoam'
    exit 1
fi
declare -F _vernon_replication_sizing_policy >/dev/null || exit 2
passed=0
failed=0
ok() {
    local expected=$1 available=$2 actual
    shift 2
    if actual=$(_vernon_replication_sizing_policy "$available" "$@" 2>/dev/null) && [[ $actual == "$expected" ]]; then
        passed=$((passed + 1))
    else
        printf 'FAIL expected cap=%s available=%s argv=' "$expected" "$available"
        printf ' <%s>' "$@"
        printf '\n'
        failed=$((failed + 1))
    fi
}
bad() {
    if _vernon_replication_sizing_policy "$@" >/dev/null 2>&1; then
        printf 'FAIL accepted rejection case:'
        printf ' <%s>' "$@"
        printf '\n'
        failed=$((failed + 1))
    else
        passed=$((passed + 1))
    fi
}
ok 23068672 24117248 blockMesh
ok 23068672 24117248 surfaceFeatureExtract
ok 23068672 24117248 decomposePar -force
ok 23068672 24117248 reconstructParMesh -constant
ok 23068672 24117248 checkMesh
ok 23068672 24117248 snappyHexMesh -overwrite
ok 11534336 24117248 mpirun -np 1 snappyHexMesh -overwrite -parallel
ok 11534336 24117248 mpirun -np 2 snappyHexMesh -overwrite -parallel
ok 23068672 024117248 blockMesh
ok 23068672 30000000 blockMesh
bad 24117247 blockMesh
bad '' blockMesh
bad NaN blockMesh
bad -1 blockMesh
bad 999999999999999999999999999 blockMesh
bad 24117248
bad 24117248 ''
bad 24117248 simpleFoam
bad 24117248 /usr/bin/blockMesh
bad 24117248 surfaceFeatures
bad 24117248 blockMesh unexpected
bad 24117248 mpirun -np 0 snappyHexMesh -overwrite -parallel
bad 24117248 mpirun -np 3 snappyHexMesh -overwrite -parallel
bad 24117248 mpirun -np 2 simpleFoam -parallel
bad 24117248 mpirun -np 2 snappyHexMesh -overwrite -parallel : simpleFoam
bad 24117248 mpirun -np 2 snappyHexMesh -overwrite -parallel extra
bad 24117248 mpirun --allow-run-as-root -np 2 snappyHexMesh -overwrite -parallel
printf 'WP9_SIZING_POLICY_TESTS passed=%d failed=%d external_programs=0\n' "$passed" "$failed"
[[ $failed == 0 ]]
