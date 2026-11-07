#!/usr/bin/env bash
# Final native rebuild must finish before the real desktop smoke starts.
set -euo pipefail
cd "$(dirname "$0")/.."
python3 tools/print_gates.py native
python3 tools/print_gates.py smoke
