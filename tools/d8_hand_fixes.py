"""D8: mechanical template port, with every hand adaptation asserted here."""
from pathlib import Path
import importlib.util

ROOT = Path(__file__).resolve().parents[1]
REF = Path('/mnt/Data/Projects/FreedomBooks/wt/reference')
spec = importlib.util.spec_from_file_location('port', ROOT/'crates/fb-templates/tools/port_template.py')
port = importlib.util.module_from_spec(spec)
spec.loader.exec_module(port)
FILES = ['ledger/reports/'+name for name in ['_controls', '_rows', 'profit_loss', 'profit_loss_body', 'balance_sheet', 'balance_sheet_body', 'general_ledger', 'general_ledger_body']]

def source(name):
    return REF/'ledger/templates'/(name+'.html')

def hand(text, name):
    pairs = [
        ('{% date_span start end %}', '{{ date_span(start, end) }}'),
        ('{% long_date as_of %}', '{{ long_date(as_of) }}'),
        ('{% long_date report.fiscal_year_start %}', '{{ long_date(report.fiscal_year_start) }}'),
        ('{% long_date start %}', '{{ long_date(start) }}'),
        ('{{ a }}', '{{ a.label }}'),
        ('{{ account }}', '{{ account.label }}'),
        ('{{ group.section.account }}', '{{ group.section.account.label }}'),
        # Money is passed as exact decimal text. Unlike Python Decimal, "0.00" is truthy.
        ('{% if row.debit %}', '{% if row.debit|report_nonzero %}'),
        ('{% if row.credit %}', '{% if row.credit|report_nonzero %}'),
    ]
    expected = {
        '_controls': {'{{ a }}': 1},
        'profit_loss_body': {'{% date_span start end %}': 1},
        'balance_sheet_body': {'{% long_date as_of %}': 1, '{% long_date report.fiscal_year_start %}': 1},
        'general_ledger_body': {'{% date_span start end %}': 1, '{% long_date start %}': 1,
                               '{{ account }}': 1, '{{ group.section.account }}': 1,
                               '{% if row.debit %}': 1, '{% if row.credit %}': 1},
    }.get(name.rsplit('/', 1)[-1], {})
    for old, new in pairs:
        assert text.count(old) == expected.get(old, 0), (name, old, text.count(old))
        text = text.replace(old, new)
    return text

if __name__ == '__main__':
    for name in FILES:
        dest = ROOT/'crates/fb-templates/templates'/(name+'.html')
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_text(hand(port.port(source(name).read_text()), name))
    print('D8: regenerated exactly eight report templates with asserted hand fixes')
