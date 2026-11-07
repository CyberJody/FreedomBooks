#!/usr/bin/env bash
# Run only after gate exits 0: clock uses the screen scenario DB; the harness then resets it.
set -euo pipefail
cd "$(dirname "$0")/.."
python3 tools/print_gates.py clock
python3 tools/print_gates.py harness
python3 tools/print_gates.py cleanup
python3 tools/print_evidence.py
