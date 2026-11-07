"""Asserted, reproducible D3 template changes after the mechanical port.

Run from the repository root. Later-wave activity fragments are expanded into
directory/detail, so D3 owns exactly its 13 inventory templates. Their standalone
routes/templates stay with their assigned waves.
"""
from pathlib import Path
import importlib.util
import os
import re

ROOT = Path(__file__).resolve().parents[1]
REF = Path(os.environ.get('FB_REFERENCE', '/mnt/Data/Projects/FreedomBooks/wt/reference'))
spec = importlib.util.spec_from_file_location('port_template', ROOT / 'crates/fb-templates/tools/port_template.py')
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
FILES = [f'directory/{n}' for n in ['base', 'list', 'detail', 'form', 'status_error']]
FILES += [f'salestax/{n}' for n in ['base', 'settings', 'list', 'item_form', 'members', 'bulk_step', 'status_error']]
FILES += ['reminders/page']


def source(name):
    return REF / name.split('/')[0] / 'templates' / (name + '.html')


def pairs(text, changes, name):
    for old, new in changes:
        assert old in text, (name, old)
        text = text.replace(old, new)
    return text


def hand(text, name):
    changes = []
    if name == 'directory/detail':
        for fragment in ['sales/receipts/customer_activity', 'credits/customer_activity',
                         'prepayments/customer_deposits', 'payables/bills/vendor_activity', 'checks/vendor_activity']:
            changes.append(("{% include '" + fragment + ".html' %}", module.port(source(fragment).read_text())))
    text = pairs(text, changes, name)
    changes = []
    # BoundForm/BoundField HTML is escaped by the forms renderer and marked safe here.
    for expression in ['csrf_token', 'filter_form.as_p', 'form.non_field_errors',
                       'field.label_tag', 'field.errors']:
        old = '{{ ' + expression + ' }}'
        if old in text:
            changes.append((old, '{{ ' + expression + '|safe }}'))
    if '{{ field }}' in text:
        changes.append(('{{ field }}', '{{ field.widget|safe }}'))
    for form, names in [('form', ['expected_revision', 'digest', 'customers', 'items', 'charge_tax', 'default_item', 'owed_basis', 'filing_frequency']),
                        ('settings_form', ['expected', 'days'])]:
        for field in names:
            for suffix in ['', '.label', '.label_tag', '.errors', '.help_text', '.id_for_label']:
                old = '{{ ' + form + '.' + field + suffix + ' }}'
                if old in text:
                    attr = suffix or '.widget'
                    safe = '|safe' if attr in ['.widget', '.label_tag', '.errors'] else ''
                    changes.append((old, '{{ ' + form + '.f.' + field + attr + safe + ' }}'))
    if name == 'salestax/list':
        changes.append(('{{ csrf_token|safe }}', '{{ status_tokens[item.pk]|safe }}'))
    if name == 'reminders/page':
        changes += [('{% if settings_form.errors %}', '{% if settings_form.has_errors %}'),
                    ('{% for field in settings_form %}{% for error in field.errors %}',
                     '{% for error in settings_errors %}') ,
                    ('<p>{{ error }}</p>{% endfor %}{% endfor %}', '<p>{{ error }}</p>{% endfor %}')]
    for page in ['page', 'customer_invoices', 'customer_receipts', 'customer_sales_receipts', 'vendor_bills', 'vendor_payments', 'vendor_checks']:
        for var in ['record', 'rev', 'r', 'p', 'c']:
            old = '{% for ' + var + ' in ' + page + ' %}'
            if old in text:
                changes.append((old, '{% for ' + var + ' in ' + page + '.object_list %}'))
    if name.startswith('directory/'):
        for old, new in [('{{ record }}', '{{ record.label }}'),
                         ('{{ party_labels|party_label(record.pk) }}', '{{ record.party_label }}'),
                         ("{{ town_for|party_label(record.pk)|default('—') }}", "{{ record.town|default('—') }}")]:
            if old in text:
                changes.append((old, new))
    return pairs(text, changes, name)


def main():
    for name in FILES:
        text = module.port(source(name).read_text())
        text = hand(text, name)
        dest = ROOT / 'crates/fb-templates/templates' / (name + '.html')
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_text(text)
    print('D3: regenerated exactly 13 templates from the frozen source and asserted hand fixes')


if __name__ == '__main__':
    main()
