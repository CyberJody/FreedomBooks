#!/usr/bin/env bash
set -euo pipefail
FBV2_DB="$FB_SCREENDIFF_DB" . crates/fb-harness/py/env.sh
python3 tools/verify_screen_clock.py
