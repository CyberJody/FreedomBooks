#!/usr/bin/env bash
# Re-render the unchanged scenario DB while iterating only on screens (no service writes).
set -euo pipefail
cd "$(dirname "$0")/.."
export FB_HARNESS_VAR="$TMPDIR/reference-var"
export FB_TIME_ZONE=America/New_York
FBV2_DB=fbv2_int_dfix1_d7b_screens . crates/fb-harness/py/env.sh
"$FB_PY" crates/fb-screendiff/py/render_django.py 'Kettle Pond Supply' target/screendiff/django
cargo build -q --release -p fb-screendiff
target/release/fb-screendiff crates/fb-screendiff/scenarios/screens-d7b.json target/screendiff/django target/screendiff
