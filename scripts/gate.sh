#!/usr/bin/env bash
# The standard v2 gate: clippy with -D warnings, then check_json_features.sh (the whole workspace test suite
# plus the feature-unification JSON run), then the core-without-Node check (ARCHITECTURE 4.1) and the
# screen diff (9.4, needs PostgreSQL + the reference venv like the differential harness).
# Same first two steps as int-l4's scripts/gate.sh; D1 adds the last two. Run from any directory.
#
# --quick: clippy, cargo test for the crates changed since the merge base with FB_GATE_BASE (default main)
# and their workspace dependents (scripts/gate_quick_plan.py), and the core isolation check. No JSON
# feature run, no screen diff. The full gate (no flag) still runs once before any merge.
set -euo pipefail
cd "$(dirname "$0")/.."
export RUST_TEST_THREADS="${RUST_TEST_THREADS:-4}"

QUICK=0
[ "$#" -le 1 ] || { echo "usage: scripts/gate.sh [--quick]" >&2; exit 2; }
case "${1:-}" in
  "") ;;
  --quick) QUICK=1 ;;
  *) echo "usage: scripts/gate.sh [--quick]" >&2; exit 2 ;;
esac

core_isolation() {
  local tree
  # fb-core must not depend on Node, napi, Electron or a network client (4.1).
  tree="$(cargo tree -p fb-core -e normal --prefix none)"
  if grep -Eiq '^(napi|neon|electron|reqwest|ureq|hyper|tokio) ' <<<"$tree"; then
    echo "gate: fb-core depends on a window/Node/network crate" >&2; exit 1
  fi
  # WAVE4 fix 1: the process that holds the book and its key links no HTTP/TLS stack either. Network
  # plugins run in the separate fb-net process (crates/fb-net), which never opens the book.
  for crate in fb-app fb-node; do
    tree="$(cargo tree -p "$crate" -e normal,build --prefix none)"
    if grep -Eiq '^(reqwest|ureq|hyper|hyper-util|h2|tokio|rustls|native-tls|fb-plugin-simplefin|fb-plugin-openai) ' <<<"$tree"; then
      echo "gate: $crate links a network client or a network plugin" >&2; exit 1
    fi
  done
}

if [ "$QUICK" = 1 ]; then
  trap 'rc=$?; [ "$rc" = 0 ] || echo "gate(quick): FAILED (exit $rc)" >&2' EXIT
  PLAN="$(python3 scripts/gate_quick_plan.py .)"
  CRATES=()
  if [ -n "$PLAN" ]; then mapfile -t CRATES <<<"$PLAN"; fi
  cargo clippy --workspace --all-targets -- -D warnings
  if [ "${#CRATES[@]}" -gt 0 ]; then
    PKGS=()
    for name in "${CRATES[@]}"; do PKGS+=(-p "$name"); done
    cargo test "${PKGS[@]}" --no-fail-fast
  fi
  core_isolation
  echo "gate(quick): ${#CRATES[@]} crates tested, clippy clean, core isolation OK — passed"
  exit 0
fi

cargo clippy --workspace --all-targets -- -D warnings
crates/fb-db/check_json_features.sh
core_isolation
crates/fb-screendiff/run_screendiff.sh
echo "gate: clippy, check_json_features.sh (workspace tests + JSON feature unification), core isolation and screen diff passed"
