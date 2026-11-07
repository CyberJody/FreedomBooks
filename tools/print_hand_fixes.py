"""Regenerate the five PRINT templates from the frozen reference, with asserted adaptations."""
from pathlib import Path
import importlib.util

ROOT = Path(__file__).resolve().parents[1]
REF = ROOT.parents[1] / 'wt/reference'
FILES = ['sales/print/document', 'prepayments/print', 'banking/registers/print',
         'banking/reconcile/report_print', 'ledger/reports/print']
spec = importlib.util.spec_from_file_location('port', ROOT / 'crates/fb-templates/tools/port_template.py')
port = importlib.util.module_from_spec(spec)
spec.loader.exec_module(port)


def hand(text, name):
    audience, kind = ('customer', 'document') if name.startswith(('sales/', 'prepayments/')) else (
        'internal', 'register' if 'registers/' in name else 'reconciliation' if 'reconcile/' in name else 'report')
    old = '{% print_stamp_label "' + audience + '" "' + kind + '" as stamp %}'
    assert text.count(old) == 1
    text = text.replace(old, '')  # Supplied by the view model, like the existing toolbar note.
    replacements = {
        '{{ account }}': '{{ account.label }}',
        '{{ line.quantity|qty }}': '{{ line.quantity|print_qty }}',
        '{{ current.tax_rate|qty }}': '{{ current.tax_rate|print_qty }}',
    }
    expected = {'sales/print/document': [0, 1, 1], 'banking/registers/print': [2, 0, 0],
                'banking/reconcile/report_print': [2, 0, 0]}.get(name, [0, 0, 0])
    for (old, new), count in zip(replacements.items(), expected):
        assert text.count(old) == count, (name, old, text.count(old))
        text = text.replace(old, new)
    return text


if __name__ == '__main__':
    for name in FILES:
        module = name.split('/')[0]
        dest = ROOT / 'crates/fb-templates/templates' / (name + '.html')
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_text(hand(port.port((REF / module / 'templates' / (name + '.html')).read_text()), name))
    print('PRINT: regenerated five templates with asserted hand fixes')
