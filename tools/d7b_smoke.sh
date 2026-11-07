#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
XVFB=/mnt/Data/Projects/FreedomBooks/wt-v2/d1-desktop/electron/.tooling/xvfb/usr
export PATH="$XVFB/bin:$PATH"
export LD_LIBRARY_PATH="$XVFB/lib/x86_64-linux-gnu:$XVFB/lib:${LD_LIBRARY_PATH:-}"
export FB_SMOKE_SHOTS=/mnt/Data/Projects/FreedomBooks/staging/d7b
node electron/scripts/build-native.cjs
cd electron
npm test
xvfb-run -a -s '-screen 0 1440x1000x24' npm run smoke
