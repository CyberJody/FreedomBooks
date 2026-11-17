#!/usr/bin/env bash
# Run from any directory. Part of the standard gate: scripts/gate.sh (which runs the whole workspace
# suite itself, under the features the workspace really unifies).
#
# serde_json's arbitrary_precision once reached the whole workspace through SimpleFIN and changed how
# stored JSON evidence decoded. SimpleFIN now enables only raw_value.
# 1. Static: no package, target or feature combination in the workspace may enable arbitrary_precision.
# 2. fb-db and SimpleFIN still pass when a dependency reintroduces it (forced here for the test build
#    only, never in production). Nearly every fb-db test module parses or stores JSON, so all of fb-db
#    runs; with dependencies built optimized this takes about three minutes including the build.
set -euo pipefail
cd "$(dirname "$0")/../.."

tree="$(cargo tree --workspace --all-features --target all -e features --prefix none)"
if grep -Fq 'serde_json feature "arbitrary_precision"' <<<"$tree"; then
  echo "check_json_features: serde_json arbitrary_precision is enabled in the workspace feature graph:" >&2
  cargo tree --workspace --all-features --target all -e features -i serde_json >&2 || true
  exit 1
fi
if ! grep -Fq 'serde_json feature "preserve_order"' <<<"$tree"; then
  echo "check_json_features: the feature graph lists no serde_json features; the static check cannot see them" >&2
  exit 1
fi
echo "check_json_features: arbitrary_precision is absent from the workspace feature graph"

cargo test -p fb-db -p fb-plugin-simplefin --no-fail-fast \
  --features serde_json/arbitrary_precision,serde_json/preserve_order
