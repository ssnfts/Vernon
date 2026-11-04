#!/usr/bin/env bash
# Root runs this in a fresh Bash. The strings in _vernon_tool_argv are NEVER run.
# The only sourced program is the reviewed environment under test, which in turn
# sources its fixed, approved environment initializer. No test-mode bypass exists.
if [[ ${1-} != /* || ! -r ${1-} ]]; then
    builtin printf '%s\n' 'Usage: bash wp9-sizing-integration-probes.sh /absolute/path/to/wp9-replication-sizing-env.sh' >&2
    exit 2
fi
readonly _wp9_environment=$1
if builtin declare -p _vernon_tool_argv >/dev/null 2>&1; then
    builtin printf '%s\n' 'Start the probe driver in a fresh Bash without _vernon_tool_argv.' >&2
    exit 2
fi

# This checks actual host availability before every probe; it cannot make the
# production loader see synthetic memory or reserve the observed free memory.
_wp9_memory_ready() {
    local _key _value _rest
    while IFS=$' \t' builtin read -r _key _value _rest; do
        if [[ $_key == MemAvailable: ]]; then
            if [[ $_value =~ ^[0-9]{1,12}$ ]] && ((10#$_value >= 24117248)); then
                return 0
            fi
            builtin printf 'PRECONDITION: actual MemAvailable=%s KiB; need at least 24117248.\n' "$_value" >&2
            return 77
        fi
    done < /proc/meminfo
    builtin printf '%s\n' 'PRECONDITION: cannot read actual MemAvailable from /proc/meminfo.' >&2
    return 77
}

# Parenthesized function body gives every probe its own process and limit state.
_wp9_probe() (
    local _label=$1 _mode=$2 _expected=$3
    shift 3
    _wp9_memory_ready || return 77
    case $_mode in
        accepted|denied|soft_serial|hard_serial|soft_mpi|hard_mpi)
            readonly -a _vernon_tool_argv=("$@") ;;
        missing) ;;
        mutable) local -a _vernon_tool_argv=(blockMesh) ;;
        scalar) readonly _vernon_tool_argv=blockMesh ;;
        sparse) readonly -a _vernon_tool_argv=([1]=blockMesh) ;;
        empty_array) readonly -a _vernon_tool_argv=() ;;
        empty_first) readonly -a _vernon_tool_argv=('' -force) ;;
        *) builtin printf 'Probe driver error: unknown mode %s\n' "$_mode" >&2; return 2 ;;
    esac
    case $_mode in
        soft_serial) builtin ulimit -S -v 10485760 || return 2 ;;
        hard_serial) builtin ulimit -S -v 10485760 && builtin ulimit -H -v 10485760 || return 2 ;;
        soft_mpi) builtin ulimit -S -v 4194304 || return 2 ;;
        hard_mpi) builtin ulimit -S -v 4194304 && builtin ulimit -H -v 4194304 || return 2 ;;
    esac
    local _before_array _before_pwd _before_args _before_soft _before_hard
    local _after_array _after_args _after_soft _after_hard _source_status
    _before_array=$(builtin declare -p _vernon_tool_argv 2>/dev/null) || _before_array=UNSET
    _before_pwd=$PWD
    _before_soft=$(builtin ulimit -S -v) || return 2
    _before_hard=$(builtin ulimit -H -v) || return 2
    set -- 'argument with spaces' '' '--literal-flag' '$unchanged' $'line one\nline two'
    builtin printf -v _before_args '%q\n' "$@"

    if . "$_wp9_environment"; then
        _source_status=0
    else
        _source_status=$?
    fi
    # Reaching these assertions is mandatory: an erroneous exit from the source
    # file cannot be mistaken for a successful probe merely because it exits 0.
    builtin printf -v _after_args '%q\n' "$@"
    _after_array=$(builtin declare -p _vernon_tool_argv 2>/dev/null) || _after_array=UNSET
    _after_soft=$(builtin ulimit -S -v) || return 2
    _after_hard=$(builtin ulimit -H -v) || return 2
    if [[ $PWD != "$_before_pwd" || $_before_args != "$_after_args" || $_before_array != "$_after_array" ]]; then
        builtin printf 'FAIL %s: source changed cwd, positional arguments, or the tool-array declaration.\n' "$_label" >&2
        return 1
    fi
    if [[ $_mode == accepted ]]; then
        if [[ $_source_status != 0 || $_after_soft != "$_expected" || $_after_hard != "$_expected" ]]; then
            builtin printf 'FAIL %s: source=%s soft=%s hard=%s expected=%s.\n' "$_label" "$_source_status" "$_after_soft" "$_after_hard" "$_expected" >&2
            return 1
        fi
    else
        if [[ $_source_status == 0 || $_after_soft != "$_before_soft" || $_after_hard != "$_before_hard" ]]; then
            builtin printf 'FAIL %s: expected rejection with unchanged limits; source=%s before=%s/%s after=%s/%s.\n' "$_label" "$_source_status" "$_before_soft" "$_before_hard" "$_after_soft" "$_after_hard" >&2
            return 1
        fi
        # If memory fell below the prerequisite during sourcing, don't count a
        # rejection as evidence that the malformed argv or tighter cap was caught.
        _wp9_memory_ready || return 77
    fi
    builtin printf 'WP9_SIZING_PROBE_COMPLETE name=%s\n' "$_label"
    return 0
)

_wp9_passed=0
_wp9_failed=0
_wp9_blocked=0
_wp9_run() {
    local _label=$1 _output _status
    if _output=$(_wp9_probe "$@"); then _status=0; else _status=$?; fi
    if [[ $_status == 0 && ${_output##*$'\n'} == "WP9_SIZING_PROBE_COMPLETE name=$_label" ]]; then
        _wp9_passed=$((_wp9_passed + 1))
        builtin printf 'PASS %s\n' "$_label"
    elif [[ $_status == 77 ]]; then
        _wp9_blocked=$((_wp9_blocked + 1))
        builtin printf 'BLOCKED %s: live memory prerequisite not met.\n' "$_label"
    else
        _wp9_failed=$((_wp9_failed + 1))
        builtin printf 'FAIL %s: exit=%s completion_marker=%s\n' "$_label" "$_status" "${_output##*$'\n'}"
    fi
}

_wp9_run serial_block accepted 23068672 blockMesh
_wp9_run serial_features accepted 23068672 surfaceFeatureExtract
_wp9_run serial_decompose accepted 23068672 decomposePar -force
_wp9_run serial_reconstruct accepted 23068672 reconstructParMesh -constant
_wp9_run serial_check accepted 23068672 checkMesh
_wp9_run serial_snappy accepted 23068672 snappyHexMesh -overwrite
_wp9_run mpi_one accepted 11534336 mpirun -np 1 snappyHexMesh -overwrite -parallel
_wp9_run mpi_two accepted 11534336 mpirun -np 2 snappyHexMesh -overwrite -parallel
_wp9_run missing_array missing 0
_wp9_run mutable_array mutable 0
_wp9_run scalar scalar 0
_wp9_run sparse_array sparse 0
_wp9_run empty_array empty_array 0
_wp9_run empty_first empty_first 0
_wp9_run unknown denied 0 madeUpMeshUtility
_wp9_run solver denied 0 simpleFoam
_wp9_run mpi_solver denied 0 mpirun -np 2 simpleFoam -parallel
_wp9_run mpi_three denied 0 mpirun -np 3 snappyHexMesh -overwrite -parallel
_wp9_run mpi_mpmd denied 0 mpirun -np 2 snappyHexMesh -overwrite -parallel : simpleFoam
_wp9_run serial_extra denied 0 blockMesh unexpected
_wp9_run tighter_serial_soft soft_serial 0 blockMesh
_wp9_run tighter_serial_hard hard_serial 0 blockMesh
_wp9_run tighter_mpi_soft soft_mpi 0 mpirun -np 2 snappyHexMesh -overwrite -parallel
_wp9_run tighter_mpi_hard hard_mpi 0 mpirun -np 2 snappyHexMesh -overwrite -parallel

builtin printf 'WP9_SIZING_INTEGRATION passed=%d failed=%d blocked=%d mesh_solver_invocations=0\n' "$_wp9_passed" "$_wp9_failed" "$_wp9_blocked"
if ((_wp9_failed > 0)); then exit 1; fi
if ((_wp9_blocked > 0)); then exit 77; fi
[[ $_wp9_passed == 24 ]]
