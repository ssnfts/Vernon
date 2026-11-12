#!/usr/bin/env bash
# Pure policy checks for wp9-replication-solve-env.sh. Starts no tool.
# Usage: bash test-wp9-solve-policy.sh /absolute/path/to/wp9-replication-solve-env.sh
readonly -a _vernon_tool_argv=(mpirun -np 8 simpleFoam -parallel)
if . "$1" >/dev/null 2>&1; then
    printf '%s\n' 'FAIL: environment accepted an 8-rank solver'
    exit 1
fi
declare -F _vernon_replication_solve_policy >/dev/null || exit 2
passed=0
failed=0
ok() {
    local expected=$1 available=$2 actual
    shift 2
    if actual=$(_vernon_replication_solve_policy "$available" "$@" 2>/dev/null) && [[ $actual == "$expected" ]]; then
        passed=$((passed + 1))
    else
        printf 'FAIL expected cap=%s available=%s argv=' "$expected" "$available"
        printf ' <%s>' "$@"
        printf '\n'
        failed=$((failed + 1))
    fi
}
bad() {
    if _vernon_replication_solve_policy "$@" >/dev/null 2>&1; then
        printf 'FAIL accepted rejection case:'
        printf ' <%s>' "$@"
        printf '\n'
        failed=$((failed + 1))
    else
        passed=$((passed + 1))
    fi
}
# Serial mesh tools and serial pre/post processing: 22 GiB address space each.
ok 23068672 24117248 surfaceFeatureExtract
ok 23068672 24117248 blockMesh
ok 23068672 24117248 snappyHexMesh -overwrite
ok 23068672 24117248 checkMesh
ok 23068672 24117248 topoSet
ok 23068672 24117248 potentialFoam
ok 23068672 24117248 decomposePar -force
ok 23068672 24117248 reconstructPar -latestTime
ok 23068672 30000000 potentialFoam
ok 23068672 024117248 blockMesh
# The one parallel program: 16 simpleFoam ranks, 4 GiB address space per rank.
ok 4194304 24117248 mpirun -np 16 simpleFoam -parallel
# Memory precondition.
bad 24117247 potentialFoam
bad '' potentialFoam
bad NaN potentialFoam
bad -1 potentialFoam
bad 999999999999999999999999999 potentialFoam
# Everything outside the allow-list.
bad 24117248
bad 24117248 ''
bad 24117248 simpleFoam
bad 24117248 /usr/bin/simpleFoam
bad 24117248 foamRun -solver incompressibleFluid
bad 24117248 surfaceFeatures
bad 24117248 reconstructParMesh -constant
bad 24117248 reconstructPar
bad 24117248 reconstructPar -latestTime extra
bad 24117248 decomposePar
bad 24117248 potentialFoam -writep
bad 24117248 snappyHexMesh
bad 24117248 blockMesh unexpected
bad 24117248 mpirun -np 1 simpleFoam -parallel
bad 24117248 mpirun -np 8 simpleFoam -parallel
bad 24117248 mpirun -np 32 simpleFoam -parallel
bad 24117248 mpirun -np 16 simpleFoam
bad 24117248 mpirun -np 16 simpleFoam -parallel extra
bad 24117248 mpirun -np 16 simpleFoam -parallel : potentialFoam
bad 24117248 mpirun -np 16 snappyHexMesh -overwrite -parallel
bad 24117248 mpirun -np 2 snappyHexMesh -overwrite -parallel
bad 24117248 mpirun --allow-run-as-root -np 16 simpleFoam -parallel
bad 24117248 mpirun -np 16 pimpleFoam -parallel
bad 24117248 bash -c true
printf 'WP9_SOLVE_POLICY_TESTS passed=%d failed=%d external_programs=0\n' "$passed" "$failed"
[[ $failed == 0 ]]
