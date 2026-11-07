"""D2 template changes after the mechanical port; every change asserts its source."""
from pathlib import Path

FILES = ['ledger/' + name for name in ['home', 'company', 'form', 'error', 'events', 'history',
         'journal', 'journal_form', 'report_problem', 'setup_start', 'setup_fill', 'trial_balance',
         'includes/print_toolbar_note']] + ['documents/setup']


def fix(text, old, new, path):
    assert old in text, (path, old)
    return text.replace(old, new)


for name in FILES:
    path = Path('templates') / (name + '.html')
    text = path.read_text()
    # Typed BoundForm / BoundField values carry already escaped markup.
    pairs = [
        ('{{ csrf_token }}', '{{ csrf_token|safe }}'),
        ('{{ form.as_p }}', '{{ form.as_p|safe }}'),
        ('{{ reversal_form.as_p }}', '{{ reversal_form.as_p|safe }}'),
        ('{% for field in filter_form %}', '{% for field in filter_form.fields %}'),
        ('{{ field }}{{ field.errors }}', '{{ field.widget|safe }}{{ field.errors|safe }}'),
        ('{{ filter_form.non_field_errors }}', '{{ filter_form.non_field_errors|safe }}'),
        ('filter_form.errors', 'filter_form.has_errors'),
        ('{{ journal }}', '{{ journal.label }}'),
        ('{{ journal.reversal }}', '{{ journal.reversal.label }}'),
        ('{{ journal.reversal_of }}', '{{ journal.reversal_of.label }}'),
        ('{{ line.account }}', '{{ line.account.label }}') if name == 'ledger/journal' else ('', ''),
        ('{{ form.non_field_errors }}', '{{ form.non_field_errors|safe }}'),
        ('{% for line in lines %}', '{% for line in lines.forms %}' if name == 'ledger/journal_form' else '{% for line in lines %}'),
    ]
    for old, new in pairs:
        if old != new and old in text:
            text = fix(text, old, new, path)
    if name == 'ledger/company':
        text = fix(text, 'request.GET', 'filtered', path)
    if name == 'ledger/journal':
        text = fix(text, '{{ post_form.expected_revision }}', '{{ post_form.f.expected_revision.widget|safe }}', path)
        text = fix(text, '{{ csrf_token|safe }}{{ post_form', '{{ post_token|safe }}{{ post_form', path)
        text = fix(text, '{{ csrf_token|safe }}{{ reversal_form', '{{ reverse_token|safe }}{{ reversal_form', path)
        text = fix(text, '{{ journal.adjusting|yesno("yes,no") }}', '{{ "yes" if journal.adjusting else "no" }}', path)
        text = fix(text, '{% attachments_panel journal=journal %}', '{{ attachments_panel|safe }}', path)
        text = fix(text, '{% if line.debit %}', '{% if line.has_debit %}', path)
        text = fix(text, '{% if line.credit %}', '{% if line.has_credit %}', path)
        text = fix(text, '{% if totals.difference %}', '{% if totals.unbalanced %}', path)
    if name == 'ledger/events':
        text = fix(text, '{% audit_operator event operator_labels %}', '{{ event.operator }}', path)
    if name == 'ledger/setup_start':
        text = fix(text, '{{ csrf_token|safe }}<input', '{{ fill_token|safe }}<input', path)
        text = fix(text, '{{ csrf_token|safe }}<button', '{{ done_token|safe }}<button', path)
    if name == 'ledger/includes/print_toolbar_note':
        text = fix(text, '{% print_toolbar_note audience as note %}', '', path)
    if name == 'ledger/journal_form':
        for base, fields in [('form', ['request_key','expected_revision','date','entry_number','adjusting','memo']),
                             ('line', ['account','debit','credit','description','name','DELETE']),
                             ('lines.empty_form', ['account','debit','credit','description','name','DELETE'])]:
            for field in fields:
                for suffix in ['label_tag','errors']:
                    old = '{{ ' + base + '.' + field + '.' + suffix + ' }}'
                    if old in text:
                        text = fix(text, old, '{{ ' + base + '.f.' + field + '.' + suffix + '|safe }}', path)
                old = '{{ ' + base + '.' + field + ' }}'
                if old in text:
                    text = fix(text, old, '{{ ' + base + '.f.' + field + '.widget|safe }}', path)
                for suffix in ['id_for_label','label','help_text']:
                    old = '{{ ' + base + '.' + field + '.' + suffix + ' }}'
                    if old in text:
                        text = fix(text, old, '{{ ' + base + '.f.' + field + '.' + suffix + ' }}', path)
        for old, new in [ ('{{ line.non_field_errors }}','{{ line.non_field_errors|safe }}'),
                         ('{{ lines.management_form }}','{{ lines.management_form|safe }}'),
                         ('{{ lines.non_form_errors }}','{{ lines.non_form_errors|safe }}') ]:
            text = fix(text, old, new, path)
    path.write_text(text)
print('D2 hand fixes applied')
