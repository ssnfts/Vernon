#!/usr/bin/env bash
# WP9 turbulence diagnostics: mesh-fix candidates (diagnostic only; never a
# protocol mesh, never acceptance evidence).
#
# The candidates change only lines of addLayersControls in the Stage 2
# snappyHexMeshDict: surfaces, refinement, layer count, layer thicknesses and
# the quality limits are unchanged. Meshing is serial, as in Stage 2.
#
#   wp9-mesh-fix.sh score <mesh case> <out>
#       Score an existing mesh (the Stage 2 baseline) with wp9MeshMetricDict.
#   wp9-mesh-fix.sh mesh <candidate a|b|c> <stage2 mesh case> <out>
#       Remesh with one candidate's layer controls, check it, then score it.
#   wp9-mesh-fix.sh case <candidate out> <stage2 setup case> <stage2 solve case> <out>
#       Build the 16-way start case for the m1 diagnostic from a candidate mesh:
#       the Stage 2 setup files, potentialFoam, then decomposePar.
#
# Every step logs to <out>/logs; nothing outside <out> is written.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
metric_dict=$here/wp9-mesh-fix/wp9MeshMetricDict
cmd=${1:?command (score|mesh|case)}; shift

die() { echo "WP9 mesh fix: $*" >&2; exit 2; }

load_openfoam() {
    # The installed environment is not nounset/errexit/pipefail compatible.
    set +e; set +u; set +o pipefail
    # shellcheck disable=SC1091
    source /usr/lib/openfoam/openfoam2606/etc/bashrc > "$1/environment.log" 2>&1
    set -euo pipefail
    command -v snappyHexMesh > /dev/null || die "OpenFOAM environment not available"
}

# Run one tool in a case, logging to logs/<name>.log with its wall time.
step() {
    local case=$1 name=$2; shift 2
    local t0 t1
    t0=$(date +%s)
    ( cd "$case" && /usr/bin/time -v "$@" ) > "$case/logs/$name.log" 2>&1 || die "$name failed (see $case/logs/$name.log)"
    t1=$(date +%s)
    echo "$name $((t1 - t0))" >> "$case/logs/step-seconds.txt"
}

# Replace one exact line by one or more lines; the old line must occur once.
replace_line() {
    local file=$1 old=$2 new=$3 n
    n=$(grep -cxF -- "$old" "$file" || true)
    [ "$n" = 1 ] || die "expected one line '$old' in $file, found $n"
    O=$old N=$new awk '$0 == ENVIRON["O"] { print ENVIRON["N"]; next } { print }' "$file" > "$file.new"
    mv "$file.new" "$file"
}

# The candidates. A smooths the extrusion (surface normals, interior
# directions, thickness, displacement); B and C each add one setting to A.
apply_candidate() {
    local dict=$1 cand=$2
    replace_line "$dict" "    nSmoothSurfaceNormals 1;" "    nSmoothSurfaceNormals 3;"
    replace_line "$dict" "    nSmoothNormals 3;" "    nSmoothNormals 5;
    nSmoothDisplacement 90;"
    replace_line "$dict" "    nSmoothThickness 10;" "    nSmoothThickness 20;"
    case $cand in
        a) ;;
        b) replace_line "$dict" "    nBufferCellsNoExtrude 0;" "    nBufferCellsNoExtrude 1;" ;;
        c) replace_line "$dict" "    nRelaxedIter 20;" "    nRelaxedIter 20;
    nOuterIter 5;" ;;
        *) die "unknown candidate '$cand' (a, b, c)" ;;
    esac
}

score() {
    local case=$1
    mkdir -p "$case/0"
    cp "$metric_dict" "$case/system/wp9MeshMetricDict"
    step "$case" metric postProcess -time 0 -dict system/wp9MeshMetricDict
    grep -q 'WP9_MESH_METRIC_DONE' "$case/logs/metric.log" || die "metric did not finish (see $case/logs/metric.log)"
    rmdir "$case/0" 2> /dev/null || true
}

case $cmd in
score)
    src=${1:?mesh case}; out=${2:?output dir}
    [ -e "$out" ] && die "$out exists; refusing to overwrite"
    [ -f "$src/constant/polyMesh/owner" ] || die "$src has no constant/polyMesh"
    mkdir -p "$out/case/logs" "$out/case/constant"
    cp -r "$src/system" "$out/case/"
    cp -r "$src/constant/polyMesh" "$out/case/constant/"
    load_openfoam "$out"
    score "$out/case"
    echo "WP9_MESH_SCORE_COMPLETE out=$out/case/postProcessing/wp9MeshMetric/summary.txt"
    ;;
mesh)
    cand=${1:?candidate}; src=${2:?stage2 mesh case}; out=${3:?output dir}
    [ -e "$out" ] && die "$out exists; refusing to overwrite"
    [ -f "$src/system/snappyHexMeshDict" ] || die "$src has no snappyHexMeshDict"
    mkdir -p "$out/case/logs" "$out/case/constant"
    case=$out/case
    cp -r "$src/system" "$case/"
    cp -r "$src/constant/triSurface" "$case/constant/"
    apply_candidate "$case/system/snappyHexMeshDict" "$cand"
    diff "$src/system/snappyHexMeshDict" "$case/system/snappyHexMeshDict" > "$out/snappyHexMeshDict.diff" || true
    for f in "$src"/system/*; do
        n=$(basename "$f")
        [ "$n" = snappyHexMeshDict ] && continue
        cmp -s "$f" "$case/system/$n" || die "system/$n differs from Stage 2"
    done
    sha256sum "$src/system/snappyHexMeshDict" "$case/system/snappyHexMeshDict" > "$out/snappyHexMeshDict.sha256"
    load_openfoam "$out"
    step "$case" surfaceFeatures surfaceFeatureExtract
    step "$case" blockMesh blockMesh
    step "$case" snappyHexMesh snappyHexMesh -overwrite
    step "$case" checkMesh checkMesh
    grep -q 'Finished meshing without any errors' "$case/logs/snappyHexMesh.log" || die "snappyHexMesh reported errors"
    grep -q '^Mesh OK\.' "$case/logs/checkMesh.log" || echo "WP9 mesh fix: checkMesh did not report 'Mesh OK.' for candidate $cand" >&2
    score "$case"
    echo "WP9_MESH_CANDIDATE_COMPLETE candidate=$cand out=$case"
    ;;
case)
    cdir=${1:?candidate out}; setup=${2:?stage2 setup case}; solve=${3:?stage2 solve case}; out=${4:?output dir}
    [ -e "$out" ] && die "$out exists; refusing to overwrite"
    mesh=$cdir/case
    grep -q 'WP9_MESH_METRIC_DONE' "$mesh/logs/metric.log" || die "$mesh is not a scored candidate"
    grep -q '^Mesh OK\.' "$mesh/logs/checkMesh.log" || die "$mesh failed checkMesh"
    for n in controlDict fvSchemes fvSolution decomposeParDict; do
        cmp -s "$setup/system/$n" "$solve/system/$n" || die "setup and solve system/$n differ"
    done
    mkdir -p "$out/logs" "$out/constant"
    cp -r "$setup/0" "$setup/0.orig" "$setup/system" "$out/"
    cp -r "$setup/constant/transportProperties" "$setup/constant/turbulenceProperties" "$setup/constant/triSurface" "$out/constant/"
    cp -r "$mesh/constant/polyMesh" "$out/constant/"
    for f in "$out"/0/*; do
        grep -q nonuniform "$f" && die "$(basename "$f") in the setup 0/ is mesh-specific"
    done
    load_openfoam "$out"
    step "$out" potentialFoam potentialFoam
    step "$out" decomposePar decomposePar -force
    for i in $(seq 0 15); do
        [ -f "$out/processor$i/0/U" ] && [ -f "$out/processor$i/constant/polyMesh/owner" ] || die "processor$i incomplete"
    done
    [ -d "$out/processor16" ] && die "more than 16 processors"
    cells=$(awk '/^ *cells:/ { print $2; exit }' "$mesh/logs/checkMesh.log")
    cand=$(basename "$cdir")
    {
        printf '{\n  "scope": "turbulence_diagnostic_mesh_fix",\n  "diagnostic_only": true,\n'
        printf '  "candidate": "%s",\n  "candidate_dir": "%s",\n' "$cand" "$cdir"
        printf '  "stage2_setup_case": "%s",\n  "cells": %s,\n' "$setup" "$cells"
        printf '  "snappyHexMeshDict_sha256": "%s",\n' "$(sha256sum "$mesh/system/snappyHexMeshDict" | cut -d' ' -f1)"
        printf '  "change": "addLayersControls only; see snappyHexMeshDict.diff in candidate_dir"\n}\n'
    } > "$out/vernon_mesh_fix.json"
    echo "WP9_MESH_CASE_COMPLETE out=$out cells=$cells"
    ;;
*)
    die "unknown command '$cmd' (score, mesh, case)"
    ;;
esac
