#!/usr/bin/env bash
# DESKTOP-BACKUPS: build fb-node, run npm test and an xvfb smoke (default: the backup/restore smoke).
#   tools/desktop_backups_smoke.sh [smoke:backups|smoke|smoke:legacy]
# Isolated: own data/HOME under staging/desktop-backups, own X display (never :0 or :20).
set -euo pipefail
cd "$(dirname "$0")/.."
XVFB=/mnt/Data/Projects/FreedomBooks/wt-v2/d1-desktop/electron/.tooling/xvfb/usr
export PATH="$XVFB/bin:$HOME/.local/opt/node-v20.19.2-linux-x64/bin:$HOME/.cargo/bin:$PATH"
export LD_LIBRARY_PATH="$XVFB/lib/x86_64-linux-gnu:$XVFB/lib:${LD_LIBRARY_PATH:-}"
STAGING=/mnt/Data/Projects/FreedomBooks/staging/desktop-backups
export FB_SMOKE_SHOTS="${FB_SMOKE_SHOTS:-$STAGING/shots}"
export FB_BACKUPS_SMOKE_DIR="${FB_BACKUPS_SMOKE_DIR:-$STAGING/smoke-run}"
node electron/scripts/build-native.cjs
cd electron
npm test
xvfb-run -a -n 87 -s '-screen 0 1440x1000x24 -nolisten tcp' npm run "${1:-smoke:backups}"
