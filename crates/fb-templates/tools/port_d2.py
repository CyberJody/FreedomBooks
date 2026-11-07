"""Regenerate exactly the D2 template inventory, then its asserted hand fixes."""
import os
from pathlib import Path
import runpy
from port_template import port

os.chdir(Path(__file__).resolve().parent.parent)
ref = Path(os.environ.get('FB_REFERENCE', '/mnt/Data/Projects/FreedomBooks/wt/reference'))
files = ['ledger/' + name for name in ['home', 'company', 'form', 'error', 'events', 'history',
         'journal', 'journal_form', 'report_problem', 'setup_start', 'setup_fill', 'trial_balance',
         'includes/print_toolbar_note']] + ['documents/setup']
for name in files:
    source = ref / name.split('/')[0] / 'templates' / (name + '.html')
    dest = Path('templates') / (name + '.html')
    dest.parent.mkdir(parents=True, exist_ok=True)
    dest.write_text(port(source.read_text()))
runpy.run_path('tools/d2_hand_fixes.py')
