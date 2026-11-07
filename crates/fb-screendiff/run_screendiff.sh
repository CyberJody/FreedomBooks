#!/usr/bin/env bash
# Screen diff gate (ARCHITECTURE 9.4): the same scenario through the frozen Python app and through fb-core,
# every ported screen rendered by Django and by fb-templates, compared after NORMALIZATION.md.
# Needs PostgreSQL and the reference venv (crates/fb-harness/py/env.sh), like the differential harness.
set -euo pipefail
# Presentation clocks come from clock.json; preserve the host/process timezone.
export FB_TIME_ZONE=America/New_York
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
OUT="${FB_SCREENDIFF_OUT:-$ROOT/target/screendiff}"
TEMPLATE="${FB_SCREENDIFF_TEMPLATE:-fbv2_int_dfix1_df2_ref}"
DB="${FB_SCREENDIFF_DB:-fbv2_int_dfix1_df2_screens}"
SCENARIO="${1:-$HERE/scenarios/screens-d9.json}"
for name in "$DB" "$TEMPLATE"; do
  case "$name" in fbv2_core_kr1|fbv2_core_kr1_ref|fbv2_core_pr|fbv2_core_prref|fbv2_core_prf|fbv2_core_prfref|fbv2_int_d5_*|fbv2_int4b_*|fbv2_int_d1_*|fbv2_int_d2_*|fbv2_int_d3_*|fbv2_int_d4_*|fbv2_int_dfix1_*) ;; *) echo "DB name must use an approved screen-diff prefix" >&2; exit 2;; esac
done
export FB_HARNESS_VAR="${FB_HARNESS_VAR:-$ROOT/target/dfix2-reference-var}"
FBV2_DB="$DB" . "$ROOT/crates/fb-harness/py/env.sh"
if ! psql -X -At -d "$TEMPLATE" -c "select 1" >/dev/null 2>&1; then
  createdb "$TEMPLATE"
  (FB_DB_NAME="$TEMPLATE" "$FB_PY" "$FB_REFERENCE/manage.py" migrate --noinput >/dev/null)
fi
rm -rf "$OUT"; mkdir -p "$OUT/django"
dropdb --if-exists "$DB"
createdb -T "$TEMPLATE" "$DB"
"$FB_PY" "$ROOT/crates/fb-harness/py/run_scenario.py" "$SCENARIO" "$OUT/python-dump.json"
"$FB_PY" "$HERE/py/check_steps.py" "$OUT/python-dump.json"
COMPANY="$(python3 "$HERE/py/scenario_name.py" "$SCENARIO")"
"$FB_PY" "$HERE/py/render_django.py" "$COMPANY" "$OUT/django"
# Release builds hide the plugins (N14); FB_SCREENDIFF_PLUGINS=1 compares the plugin screens too.
FEATURES=()
if [ "${FB_SCREENDIFF_PLUGINS:-0}" = 1 ]; then FEATURES=(--features plugins); fi
cargo build -q --release -p fb-screendiff "${FEATURES[@]}" --manifest-path "$ROOT/Cargo.toml"
"$ROOT/target/release/fb-screendiff" "$SCENARIO" "$OUT/django" "$OUT"
