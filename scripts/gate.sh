#!/usr/bin/env bash
# The gate: clippy with -D warnings, the whole workspace test suite at the full seeded case
# counts (FB_TEST_FULL_CASES=1), check_json_features.sh (serde_json feature graph plus the JSON tests under
# the forced features), fb-app's key-change tests (crash recovery, audit, publication; they build only with
# key-test-hooks), the Electron shell's unit tests, the core-without-Node check (ARCHITECTURE 4.1).
# Run from any directory.
#
# --quick: clippy, cargo test for the crates changed since the merge base with FB_GATE_BASE (default main)
# and their workspace dependents (scripts/gate_quick_plan.py), the Electron unit tests and the core
# isolation check. Seeded fuzz loops run their small default case counts. No JSON feature run; the key-change tests run when fb-app is in the plan. The full gate (no flag) runs at the release
# boundary.
# --isolation-only: check the core, book-process and remote dependency graphs without building.
#
# Both tiers run with TMPDIR set to a private folder (under /dev/shm when it is writable, else the
# system temp folder; FB_GATE_TMP_PARENT overrides) that is removed when the gate exits.
set -euo pipefail
cd "$(dirname "$0")/.."
# Eight test threads unless RUST_TEST_THREADS says otherwise. Measured on the 32-core workstation (load
# 85-120 from other work): the fb-core library suite took 856 s at 8 threads and 1,164 s at 32, where
# the process spent 262 of its CPU minutes in the kernel contending (21 at 8).
export RUST_TEST_THREADS="${RUST_TEST_THREADS:-8}"

key_tests() {
  cargo test -p fb-app --features key-test-hooks --test key_crashes --test protection_audit --test protection_publication --no-fail-fast
}

# The shell tests include headless Electron launches using the locked npm install.
electron_tests() {
  npm --prefix electron test
}

core_isolation() {
  local tree
  # fb-core must not depend on Node, napi, Electron or a network client (4.1).
  tree="$(cargo tree --locked -p fb-core -e normal --prefix none)"
  if grep -Eiq '^(napi|neon|electron|reqwest|ureq|hyper|tokio) ' <<<"$tree"; then
    echo "gate: fb-core depends on a window/Node/network crate" >&2; exit 1
  fi
  # WAVE4 fix 1: the process that holds the book and its key links no HTTP/TLS stack either. Network
  # plugins run in the separate fb-net process (crates/fb-net), which never opens the book.
  # Remote access: fb-remote-proto (the book side of the host boundary) is held to the same rule.
  for crate in fb-app fb-node fb-remote-proto fb-host; do
    tree="$(cargo tree --locked -p "$crate" -e normal,build --prefix none)"
    if grep -Eiq '^(reqwest|ureq|hyper|hyper-util|h2|tokio|rustls|native-tls|fb-plugin-simplefin|fb-plugin-openai) ' <<<"$tree"; then
      echo "gate: $crate links a network client or a network plugin" >&2; exit 1
    fi
  done
  # The TLS side of remote access (fb-remote) never links the book.
  tree="$(cargo tree --locked -p fb-remote -e normal,build --prefix none)"
  if grep -Eq '^(fb-app|fb-db|fb-core|fb-node|rusqlite|libsqlite3-sys) ' <<<"$tree"; then
    echo "gate: fb-remote links the book or its storage" >&2; exit 1
  fi
}

QUICK=0
usage() { echo "usage: scripts/gate.sh [--quick | --isolation-only]" >&2; exit 2; }
[ "$#" -le 1 ] || usage
case "${1:-}" in
  "") ;;
  --quick) QUICK=1 ;;
  --isolation-only) core_isolation; exit 0 ;;
  *) usage ;;
esac

# Every test book and temp file of this run goes in one private folder, removed when the gate ends,
# so concurrent runs cannot fill or share a temp folder and no fixture leaves files behind. A RAM-backed
# /dev/shm keeps synchronous=FULL commits cheap; FB_GATE_TMP_PARENT chooses another parent.
GATE_TMP_PARENT="${FB_GATE_TMP_PARENT:-}"
if [ -z "$GATE_TMP_PARENT" ]; then
  if [ -d /dev/shm ] && [ -w /dev/shm ]; then GATE_TMP_PARENT=/dev/shm; else GATE_TMP_PARENT="${TMPDIR:-/tmp}"; fi
fi
TMPDIR="$(mktemp -d "$GATE_TMP_PARENT/fb-gate.XXXXXX")"
export TMPDIR
GATE_NAME=gate
[ "$QUICK" = 1 ] && GATE_NAME="gate(quick)"
trap 'rc=$?; rm -rf -- "$TMPDIR"; [ "$rc" = 0 ] || echo "$GATE_NAME: FAILED (exit $rc)" >&2' EXIT

if [ "$QUICK" = 1 ]; then
  PLAN="$(python3 scripts/gate_quick_plan.py .)"
  CRATES=()
  if [ -n "$PLAN" ]; then mapfile -t CRATES <<<"$PLAN"; fi
  cargo clippy --workspace --all-targets -- -D warnings
  if [ "${#CRATES[@]}" -gt 0 ]; then
    PKGS=()
    for name in "${CRATES[@]}"; do PKGS+=(-p "$name"); done
    cargo test "${PKGS[@]}" --no-fail-fast
  fi
  for name in "${CRATES[@]}"; do
    if [ "$name" = fb-app ]; then key_tests; fi
  done
  electron_tests
  core_isolation
  echo "gate(quick): ${#CRATES[@]} crates tested, clippy clean, Electron unit tests and core isolation OK — passed"
  exit 0
fi

# The release gate runs every seeded fuzz loop at its full case count.
export FB_TEST_FULL_CASES=1
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast
crates/fb-db/check_json_features.sh
key_tests
electron_tests
core_isolation
echo "gate: clippy, workspace tests, check_json_features.sh, key-change tests, Electron unit tests and core isolation passed"
