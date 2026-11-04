#!/bin/bash
# Capability smoke only: 320-cell laminar duct, not automotive validation.
# Run in the approved distro, passing a NEW absolute directory:
# bash /mnt/c/path/to/vernon/scripts/cfd/wp9-cfd-smoke.sh \
#   /mnt/c/path/to/vernon/scratch/wp9-setup/duct-smoke-UNIQUE
set -euo pipefail
has_error_diagnostic() {
    # Ignore only the exact informational banner. Read the complete stream
    # (no grep -q) so pipefail cannot hide an error behind upstream SIGPIPE.
    grep -hEv '^trapFpe: Floating point exception trapping enabled \(FOAM_SIGFPE\)\.$' "$@" \
        | grep -Ei 'FOAM FATAL|FOAM exiting|Segmentation fault|Floating point exception|MPI_ABORT|(^|[[:space:]])ERROR([[:space:]:]|$)' > /dev/null
}
# Pure text probes; this mode never discovers packages or starts a CFD tool.
if [[ $# == 1 && ${1:-} == --self-test-classifier ]]; then
    expect_clear() { if printf '%s\n' "$1" | has_error_diagnostic; then echo "Unexpected error match: $1" >&2; return 1; fi; }
    expect_error() { if ! printf '%s\n' "$1" | has_error_diagnostic; then echo "Missed error match: $1" >&2; return 1; fi; }
    banner='trapFpe: Floating point exception trapping enabled (FOAM_SIGFPE).'
    expect_clear "$banner"
    expect_clear $'Mesh OK.\nEnd'
    expect_error 'Floating point exception (core dumped)'
    expect_error '--> FOAM FATAL ERROR: invalid field'
    expect_error 'Segmentation fault'
    expect_error 'ERROR: failed'
    expect_error "$banner extra diagnostic"
    expect_error "prefix $banner"
    echo 'WP9_SMOKE_CLASSIFIER_PROBES_PASSED count=8'
    exit 0
fi
[[ $# == 1 && $1 == /* ]] || { echo 'Pass one new absolute case directory.' >&2; exit 2; }
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)
allowed=$(realpath -m -- "$repo/scratch/wp9-setup")
case_path=$(realpath -m -- "$1")
[[ "$case_path" == "$allowed"/* && ! -e "$case_path" ]] || { echo 'Case must be fresh and inside scratch/wp9-setup.' >&2; exit 2; }
mkdir -p -- "$(dirname -- "$case_path")"
mkdir -- "$case_path"
cd -- "$case_path"
trap 'rc=$?; printf "%s\n" "$rc" > smoke.exit; if ((rc)); then echo "WP9_CAPABILITY_SMOKE_FAILED exit=$rc case=$case_path" >&2; fi' EXIT
dpkg-query -W -f='${binary:Package}\t${Version}\t${Status}\n' 'openfoam2606*' > packages.txt
# Wildcard metadata can include known but uninstalled optional packages.
dpkg-query -W -f='${binary:Package}\t${db:Status-Status}\n' 'openfoam2606*' \
    | awk -F '\t' '$2 == "installed" {print $1}' > installed-packages.txt
[[ -s installed-packages.txt ]] || { echo 'No installed OpenFOAM runtime packages found.' >&2; exit 1; }
while IFS= read -r package; do dpkg-query -L "$package"; done < installed-packages.txt > package-files.txt
mapfile -t environments < <(awk '/\/etc\/bashrc$/ {print}' package-files.txt | sort -u)
[[ ${#environments[@]} == 1 && -f ${environments[0]} ]]
printf '%s\n' "${environments[0]}" > environment-path.txt
# The installed environment is not nounset/errexit/pipefail compatible.
set +e; set +u; set +o pipefail
source "${environments[0]}" > environment.stdout.log 2> environment.stderr.log
environment_exit=$?
set -euo pipefail
printf '%s\n' "$environment_exit" > environment.exit
[[ "$environment_exit" == 0 && ${WM_PROJECT_VERSION:-} == *2606* ]]
printf '%s\n' "$WM_PROJECT_VERSION" > version.txt
run() {
    local seconds=$1 label=$2 rc
    shift 2
    set +e
    timeout --signal=TERM --kill-after=5s "${seconds}s" "$@" > "$label.stdout.log" 2> "$label.stderr.log"
    rc=$?
    set -e
    printf '%s\n' "$rc" > "$label.exit"
    [[ $rc == 0 ]] || { echo "$label failed with exit $rc; inspect $case_path/$label.*.log" >&2; return "$rc"; }
}
for tool in blockMesh checkMesh simpleFoam; do
    command -v "$tool" >> executables.txt
    run 20 "$tool-help" "$tool" -help
done
mkdir 0 constant system
write() {
    printf 'FoamFile { version 2.0; format ascii; class %s; object %s; }\n' "$2" "${1##*/}" > "$1"
    cat >> "$1"
}
write system/blockMeshDict dictionary <<'EOF'
scale 1;
vertices ((0 0 0) (4 0 0) (4 1 0) (0 1 0) (0 0 1) (4 0 1) (4 1 1) (0 1 1));
blocks (hex (0 1 2 3 4 5 6 7) (20 4 4) simpleGrading (1 1 1));
edges ();
boundary (
 inlet {type patch; faces ((0 4 7 3));}
 outlet {type patch; faces ((1 2 6 5));}
 walls {type wall; faces ((0 1 5 4) (3 7 6 2) (0 3 2 1) (4 5 6 7));}
);
mergePatchPairs ();
EOF
write 0/U volVectorField <<'EOF'
dimensions [0 1 -1 0 0 0 0]; internalField uniform (1 0 0);
boundaryField {inlet {type fixedValue; value uniform (1 0 0);} outlet {type zeroGradient;} walls {type noSlip;}}
EOF
write 0/p volScalarField <<'EOF'
dimensions [0 2 -2 0 0 0 0]; internalField uniform 0;
boundaryField {inlet {type zeroGradient;} outlet {type fixedValue; value uniform 0;} walls {type zeroGradient;}}
EOF
write constant/transportProperties dictionary <<'EOF'
transportModel Newtonian; nu [0 2 -1 0 0 0 0] 0.01;
EOF
write constant/turbulenceProperties dictionary <<'EOF'
simulationType laminar;
EOF
write system/controlDict dictionary <<'EOF'
application simpleFoam; startFrom startTime; startTime 0; stopAt endTime;
endTime 60; deltaT 1; writeControl timeStep; writeInterval 60;
purgeWrite 0; writeFormat ascii; writePrecision 10; runTimeModifiable false;
EOF
write system/fvSchemes dictionary <<'EOF'
ddtSchemes {default steadyState;}
gradSchemes {default Gauss linear;}
divSchemes {default none; div(phi,U) bounded Gauss upwind; div((nuEff*dev2(T(grad(U))))) Gauss linear;}
laplacianSchemes {default Gauss linear corrected;}
interpolationSchemes {default linear;} snGradSchemes {default corrected;}
EOF
write system/fvSolution dictionary <<'EOF'
solvers {p {solver GAMG; smoother GaussSeidel; tolerance 1e-8; relTol 0.05;}
 U {solver smoothSolver; smoother symGaussSeidel; tolerance 1e-8; relTol 0.05;}}
SIMPLE {nNonOrthogonalCorrectors 0;}
relaxationFactors {fields {p 0.3;} equations {U 0.7;}}
EOF
run 120 blockMesh blockMesh
run 120 checkMesh checkMesh -allTopology -allGeometry
grep -Eq '^Mesh OK\.?[[:space:]]*$' checkMesh.stdout.log
run 180 simpleFoam simpleFoam
grep -Eq '^End[[:space:]]*$' simpleFoam.stdout.log
[[ -s 60/U && -s 60/p ]]
if has_error_diagnostic blockMesh.{stdout,stderr}.log checkMesh.{stdout,stderr}.log simpleFoam.{stdout,stderr}.log; then
    echo 'An external error diagnostic was found; capability smoke failed.' >&2; exit 1
fi
printf 'WP9_CAPABILITY_SMOKE_COMPLETE cells=320 iterations=60 model=laminar case=%s\n' "$case_path" | tee smoke-complete.txt
