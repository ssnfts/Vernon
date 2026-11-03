#!/usr/bin/env sh
# Runs the whole validation and benchmark suite and writes bench/results/REPORT.md and results.json.
#   bench/run.sh [--quick] [--no-perf] [--update-baseline] [--with-app]
# Exit code 1 when any check FAILs. SKIPPED is not a failure.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
: "${CARGO_TARGET_DIR:=$root/../_targets/validation-bench}"
export CARGO_TARGET_DIR
pkgs="-p vernon-bench -p vernon-cli"
args=""
for a in "$@"; do
  case "$a" in
    --with-app) pkgs="$pkgs -p vernon" ;;
    *) args="$args $a" ;;
  esac
done
# shellcheck disable=SC2086
cargo build --release $pkgs
# shellcheck disable=SC2086
exec "$CARGO_TARGET_DIR/release/vernon-bench" run $args
