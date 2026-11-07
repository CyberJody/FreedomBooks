#!/usr/bin/env bash
# Run from any directory. Exercise both shipped workspace unification and a
# dependency reintroducing arbitrary_precision, without enabling it in production.
# Part of the standard gate: scripts/gate.sh.
set -euo pipefail
cd "$(dirname "$0")/../.."
export RUST_TEST_THREADS="${RUST_TEST_THREADS:-4}"
cargo test --workspace --no-fail-fast
# (The plain fb-db run is part of the workspace run above.)
cargo test -p fb-db -p fb-plugin-simplefin --no-fail-fast \
  --features serde_json/arbitrary_precision,serde_json/preserve_order
