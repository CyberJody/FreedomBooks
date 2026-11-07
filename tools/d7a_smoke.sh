#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
XVFB=/mnt/Data/Projects/FreedomBooks/wt-v2/d1-desktop/electron/.tooling/xvfb/usr
export PATH="$XVFB/bin:/mnt/Data/Projects/FreedomBooks/wt-v2/d1-desktop/electron/.tooling/node-v24.22.2-linux-x64/bin:$PATH"
export LD_LIBRARY_PATH="$XVFB/lib/x86_64-linux-gnu${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export FB_SMOKE_SHOTS=/mnt/Data/Projects/FreedomBooks/staging/d7a
export TMPDIR="$ROOT/target/d7a-tmp"
mkdir -p "$TMPDIR" "$FB_SMOKE_SHOTS"
cd "$ROOT/electron"
xvfb-run -a -s "-screen 0 1440x1000x24" node scripts/smoke.cjs
