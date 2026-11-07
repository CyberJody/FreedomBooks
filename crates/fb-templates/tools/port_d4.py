"""Regenerate exactly the D4 templates using the frozen mechanical translator."""
from pathlib import Path
import runpy
from port_template import port

ROOT = Path(__file__).resolve().parents[1]
REFERENCE = Path('/mnt/Data/Projects/FreedomBooks/wt/reference/sales/templates')
FILES = (['sales/' + n for n in ('list', 'detail', 'document', 'form', 'revision')]
         + ['sales/invoices/' + n for n in ('convert', 'detail', 'document', 'review', 'revision', 'void')]
         + ['sales/receipts/' + n for n in ('list', 'detail', 'form', 'reverse', 'customer_activity')]
         + ['sales/sales_receipts/' + n for n in ('list', 'detail', 'form', 'void')])
for name in FILES:
    target = ROOT / 'templates' / (name + '.html')
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(port((REFERENCE / (name + '.html')).read_text()))
runpy.run_path(str(ROOT / 'tools/d4_hand_fixes.py'))
