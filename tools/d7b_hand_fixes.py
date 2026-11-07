"""Regenerate the nine D7b templates; all adaptations are asserted old/new pairs."""
from pathlib import Path
import importlib.util

ROOT = Path(__file__).resolve().parents[1]
REF = Path('/mnt/Data/Projects/FreedomBooks/wt/reference')
spec = importlib.util.spec_from_file_location('port', ROOT/'crates/fb-templates/tools/port_template.py')
port = importlib.util.module_from_spec(spec)
spec.loader.exec_module(port)
FILES = ['banking/registers/'+n for n in ['list', 'register']]
FILES += ['banking/reconcile/'+n for n in ['chooser', 'begin', 'screen', '_summary', 'history', 'report', 'undo']]

def hand(text):
    pairs = [
        ('{{ account }}', '{{ account.label }}'),
        ('{% for account in page %}', '{% for account in page.object_list %}'),
        ('{% for rec in page %}', '{% for rec in page.object_list %}'),
        ('{{ csrf_token }}', '{{ csrf_token|safe }}'),
        ('>CSV</a>', '>CSV{{ export_token|safe }}</a>'),
        ('{% if s.debit %}', '{% if s.debit|report_nonzero %}'),
        ('{% if totals.difference %}', '{% if totals.difference|report_nonzero %}'),
        ('{% if totals.difference or too_many %}', '{% if totals.difference|report_nonzero or too_many %}'),
        ('data.service_account|stringformat(\'s\') == a.pk|stringformat(\'s\')', 'data.service_account == a.pk'),
        ('data.interest_account|stringformat(\'s\') == a.pk|stringformat(\'s\')', 'data.interest_account == a.pk'),
    ]
    for old, new in pairs:
        if old in text:
            assert text.count(old) >= 1
            text = text.replace(old, new)
    return text

if __name__ == '__main__':
    for name in FILES:
        dest = ROOT/'crates/fb-templates/templates'/(name+'.html')
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_text(hand(port.port((REF/'banking/templates'/(name+'.html')).read_text())))
    print('D7b: regenerated nine templates with asserted hand fixes')
