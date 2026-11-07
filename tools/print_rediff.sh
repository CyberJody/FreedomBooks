#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
FBV2_DB=fbv2_core_pr . crates/fb-harness/py/env.sh
"$FB_PY" crates/fb-screendiff/py/render_django.py 'Kettle Pond Supply' "$FB_SCREENDIFF_OUT/django"
cargo build -q --release -p fb-screendiff
target/release/fb-screendiff crates/fb-screendiff/scenarios/screens-d7b.json "$FB_SCREENDIFF_OUT/django" "$FB_SCREENDIFF_OUT"
