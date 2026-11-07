#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build -q --release -p fb-screendiff
target/release/fb-screendiff crates/fb-screendiff/scenarios/screens-d7b.json "$FB_SCREENDIFF_OUT/django" "$FB_SCREENDIFF_OUT"
