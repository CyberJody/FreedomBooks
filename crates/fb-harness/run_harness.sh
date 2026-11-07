#!/usr/bin/env bash
# Differential harness driver (layer 1): every scenario through the frozen Python app and through fb-core,
# then the comparer. Usage: crates/fb-harness/run_harness.sh [scenario.json ...]  (default: all in scenarios/)
# Needs: the reference worktree + venv (py/env.sh), a migrated template DB fbv2_l1_ref, cargo.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
OUT="${FB_HARNESS_OUT:-$ROOT/target/harness}"
TEMPLATE="${FB_HARNESS_TEMPLATE:-fbv2_l1_ref}"
DB="${FB_HARNESS_DB:-fbv2_l1_harness}"
for name in "$DB" "$TEMPLATE"; do
  case "$name" in fbv2_l4b_*|fbv2_l4t_*|fbv2_l4a_*|fbv2_l4r_*|fbv2_l1_*|fbv2_l2a_*|fbv2_l3s_*|fbv2_l3p_*|fbv2_l3c_*|fbv2_l3d_*|fbv2_l3r_*|fbv2_l3b_*|fbv2_int*|fbv2_l4f_*|fbv2_core_*) ;; *) echo "DB name must use an approved fbv2 harness prefix" >&2; exit 2;; esac
done
mkdir -p "$OUT"
FBV2_DB="$DB" . "$HERE/py/env.sh"
# A stale template (older migrations) would silently answer for the frozen reference: rebuild it unless asked.
if [ "${FB_HARNESS_REUSE_TEMPLATE:-0}" != 1 ]; then dropdb --if-exists "$TEMPLATE"; fi
if ! psql -X -At -d "$TEMPLATE" -c "select 1" >/dev/null 2>&1; then
  createdb "$TEMPLATE"
  (FB_DB_NAME="$TEMPLATE" "$FB_PY" "$FB_REFERENCE/manage.py" migrate --noinput >/dev/null)
fi
cargo build -q --release -p fb-harness --manifest-path "$ROOT/Cargo.toml"
BIN="$ROOT/target/release/fb-harness"
scenarios=("$@")
[ ${#scenarios[@]} -eq 0 ] && scenarios=("$HERE"/scenarios/*.json)
status=0
for scenario in "${scenarios[@]}"; do
  name="$(basename "$scenario" .json)"
  dropdb --if-exists "$DB"
  createdb -T "$TEMPLATE" "$DB"
  t0=$(date +%s.%N)
  "$FB_PY" "$HERE/py/run_scenario.py" "$scenario" "$OUT/$name.python.json"
  t1=$(date +%s.%N)
  "$BIN" run "$scenario" "$OUT/$name.rust.json" "$OUT/books"
  t2=$(date +%s.%N)
  printf '== %s (python %.1fs, rust %.1fs)\n' "$name" "$(echo "$t1 - $t0" | bc)" "$(echo "$t2 - $t1" | bc)"
  if ! "$BIN" compare "$OUT/$name.python.json" "$OUT/$name.rust.json" "$scenario" > "$OUT/$name.compare.txt"; then status=1; fi
  printf '   warnings: %s\n' "$(grep -c '^WARN' "$OUT/$name.compare.txt" || true)"
  grep '^FAIL' "$OUT/$name.compare.txt" | head -20 || true
  tail -1 "$OUT/$name.compare.txt"
done
echo "database left from the last scenario: $DB (template $TEMPLATE)"
exit $status
