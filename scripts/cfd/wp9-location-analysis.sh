#!/usr/bin/env bash
# WP9 turbulence diagnostics: location analysis of clipped k/omega cells.
# Builds a separate analysis case from the D1 and C0 diagnostic cases (mesh,
# D1 fields at 1000, clip-count maps of both runs), leaving those cases
# untouched, and runs the read-only coded function object in
# wp9-location/wp9LocationDict with postProcess on 16 ranks.
# Usage (inside WSL): wp9-location-analysis.sh <d1 case> <c0 case> <out dir>
set -euo pipefail
d1=${1:?d1 case}; c0=${2:?c0 case}; out=${3:?output dir}
here=$(cd "$(dirname "$0")" && pwd)
np=16
[ -e "$out" ] && { echo "WP9 location: $out exists; refusing to overwrite" >&2; exit 2; }
for c in "$d1" "$c0"; do
    for i in $(seq 0 $((np - 1))); do
        [ -d "$c/processor$i/1000" ] && [ -d "$c/processor$i/750" ] || { echo "WP9 location: $c/processor$i lacks 750 or 1000" >&2; exit 2; }
    done
done
mkdir -p "$out/case/system" "$out/case/constant"
case=$out/case
cp "$d1/system/fvSchemes" "$d1/system/fvSolution" "$d1/system/decomposeParDict" "$case/system/"
cp "$here/wp9-location/wp9LocationDict" "$case/system/"
cp "$d1/constant/transportProperties" "$d1/constant/turbulenceProperties" "$case/constant/"
cat > "$case/system/controlDict" <<'EOF'
FoamFile { version 2.0; format ascii; class dictionary; object controlDict; }
application     postProcess;
startFrom       latestTime;
startTime       0;
stopAt          endTime;
endTime         1000;
deltaT          1;
writeControl    timeStep;
writeInterval   1;
writeFormat     ascii;
writePrecision  10;
timeFormat      general;
timePrecision   6;
runTimeModifiable false;
EOF
for i in $(seq 0 $((np - 1))); do
    p=processor$i
    mkdir -p "$case/$p/constant" "$case/$p/1000" "$case/$p/750" "$case/$p/c0_1000" "$case/$p/c0_750"
    cp -r "$d1/$p/constant/polyMesh" "$case/$p/constant/"
    cp "$d1/$p/1000/"{U,k,omega,phi,yPlus,clipCount_k,clipCount_omega,clipMin_k,clipMin_omega} "$case/$p/1000/"
    cp "$d1/$p/750/"{clipCount_k,clipCount_omega} "$case/$p/750/"
    cp "$c0/$p/1000/"{clipCount_k,clipCount_omega} "$case/$p/c0_1000/"
    cp "$c0/$p/750/"{clipCount_k,clipCount_omega} "$case/$p/c0_750/"
done
# The two runs must share the decomposition for processor-local cell indices to match.
for i in $(seq 0 $((np - 1))); do
    cmp -s "$d1/processor$i/constant/polyMesh/owner" "$c0/processor$i/constant/polyMesh/owner" \
        || { echo "WP9 location: processor$i meshes differ between D1 and C0" >&2; exit 3; }
done
# The installed environment is not nounset/errexit/pipefail compatible.
set +e; set +u; set +o pipefail
# shellcheck disable=SC1091
source /usr/lib/openfoam/openfoam2606/etc/bashrc > "$out/environment.log" 2>&1
set -euo pipefail
command -v postProcess > /dev/null || { echo "WP9 location: OpenFOAM environment not available" >&2; exit 5; }
cd "$case"
mpirun -np $np postProcess -parallel -time 1000 -dict system/wp9LocationDict > log.location 2>&1
grep -q 'WP9_LOCATION_DONE time=1000' log.location || { echo "WP9 location: analysis did not finish (see $case/log.location)" >&2; exit 4; }
echo "WP9_LOCATION_COMPLETE out=$case/postProcessing/wp9Location"
