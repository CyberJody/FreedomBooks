#!/usr/bin/env bash
# Refresh the changed UI crates after the final boundary refinements. The full scripts/gate.sh run
# independently retains all unchanged core/database/plugin coverage and the current strict screen diff.
set -euo pipefail
cd "$(dirname "$0")/.."
export RUST_TEST_THREADS="${RUST_TEST_THREADS:-4}"
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p fb-app -p fb-screendiff -p fb-templates --no-fail-fast
python3 tools/d8_verify_port.py
python3 tools/d8_verify_scope.py
echo "D8 final UI checks passed"
