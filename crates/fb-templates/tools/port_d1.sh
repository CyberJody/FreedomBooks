#!/usr/bin/env bash
# Regenerate the D1 templates from the frozen reference: mechanical port, then the reviewed hand fixes.
set -euo pipefail
cd "$(dirname "$0")/.."
R="${FB_REFERENCE:-/mnt/Data/Projects/FreedomBooks/wt/reference}"
mkdir -p templates/ledger templates/sales/invoices
for f in base nav company_home accounts register pager; do
  python3 tools/port_template.py "$R/ledger/templates/ledger/$f.html" "templates/ledger/$f.html"
done
for f in list form line_fields; do
  python3 tools/port_template.py "$R/sales/templates/sales/invoices/$f.html" "templates/sales/invoices/$f.html"
done
python3 tools/d1_hand_fixes.py
