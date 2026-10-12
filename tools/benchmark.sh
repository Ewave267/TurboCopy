#!/usr/bin/env bash
set -euo pipefail

cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
mkdir -p target/performance
run_dir=$(mktemp -d "$PWD/target/performance/run-$(date -u +%Y%m%dT%H%M%SZ)-XXXXXX")
trap 'printf "%s\n" "$?" > "$run_dir/exit-status"' EXIT

printf 'Saving benchmark results to %s\n' "$run_dir"
(
    cargo build --release
    python3 -u benches/phase5.py --directory . --runs 3 --output "$run_dir/benchmark"
) 2>&1 | tee "$run_dir/run.log"

printf '\nFinished. In a new session, ask:\nUpdate the performance docs using %s/benchmark.json and compare with the previous Phase 5 results.\n' "$run_dir"
