"""Regenerate exactly the fourteen D7a templates, with asserted syntax adaptations."""
from pathlib import Path
import importlib.util
import re

ROOT = Path(__file__).resolve().parents[1]
REF = Path('/mnt/Data/Projects/FreedomBooks/wt/reference')
spec = importlib.util.spec_from_file_location('port', ROOT/'crates/fb-templates/tools/port_template.py')
port = importlib.util.module_from_spec(spec)
spec.loader.exec_module(port)
FILES = ['checks/'+n for n in ['list','detail','document','form','line_fields','number','review','revision','void','vendor_activity']]
FILES += ['banking/deposits/'+n for n in ['list','detail','form','reverse']]

def source(name):
    return REF/name.split('/')[0]/'templates'/(name+'.html')

def fix(text, old, new):
    assert old in text, old
    return text.replace(old,new)

def hand(text):
    for owner in ['check','deposit']:
        old = '{% attachments_panel document='+owner+'.identity %}'
        if old in text:
            panel = port.port(source('attachments/panel').read_text())
            panel = fix(panel, '<input type="hidden" name="csrfmiddlewaretoken" value="{{ csrf_token }}">', '{{ attachments.csrf_token|safe }}')
            text = fix(text,old,'{% with add_url=attachments.add_url, request_key=attachments.request_key, allowed=attachments.allowed, max_mb=attachments.max_mb, max_count=attachments.max_count, files=attachments.files, removed=attachments.removed, full=attachments.full, notes=attachments.notes %}'+panel+'{% endwith %}')
    pairs = [
        ('form.errors or formset.total_error_count','form.has_errors or formset.total_error_count'),
        ('{% for rev in page %}','{% for rev in page.object_list %}'),
        ('{% for rev in history_page %}','{% for rev in history_page.object_list %}'),
        ('{% for deposit in page %}','{% for deposit in page.object_list %}'),
        ('{% for c in vendor_checks %}','{% for c in vendor_checks.object_list %}'),
        ('{{ current.payee_snapshot }}','{{ current.payee_snapshot|pyrepr }}'),
        ('{{ line.item_snapshot }}','{{ line.item_snapshot|pyrepr }}'),
        ('{{ line.account_snapshot }}','{{ line.account_snapshot|pyrepr }}'),
        ('{% if not tab_totals.EXPENSE %}','{% if tab_totals.EXPENSE == "0.00" %}'),
        ('{% if not tab_totals.ITEM %}','{% if tab_totals.ITEM == "0.00" %}'),
        ('{{ deposit.journal|journal_link_label }}','{{ evidence[deposit.journal_id] }}'),
        ('{{ deposit.reversal.journal|journal_link_label }}','{{ evidence[deposit.reversal.journal_id] }}'),
    ]
    for old,new in pairs:
        if old in text: text=fix(text,old,new)
    text=re.sub(r'{% journal_evidence_link_text company ([\w.]+) %}',r'{{ evidence[\1] }}',text)
    for name in ['request_key','expected_revision','review_token','receipts']:
        text=re.sub(r'{{ form\.'+name+r'(\.[\w]+)? }}',lambda m:'{{ form.f.'+name+(m[1] or '.widget')+'|safe }}',text)
    for expr in ['csrf_token','form.non_field_errors','form.as_p','filter_form.as_p','formset.management_form','formset.non_form_errors','line.non_field_errors','field.label_tag','field.errors','hidden.errors']:
        old='{{ '+expr+' }}'
        if old in text:text=fix(text,old,'{{ '+expr+'|safe }}')
    for name in ['field','hidden']:
        old='{{ '+name+' }}'
        if old in text:text=fix(text,old,'{{ '+name+'.widget|safe }}')
    return text

if __name__=='__main__':
    for name in FILES:
        dest=ROOT/'crates/fb-templates/templates'/(name+'.html')
        dest.parent.mkdir(parents=True,exist_ok=True)
        dest.write_text(hand(port.port(source(name).read_text())))
    # D3 embedded future-wave fragments. Exercise D7a's registered template now.
    spec=importlib.util.spec_from_file_location('d3',ROOT/'tools/d3_hand_fixes.py')
    d3=importlib.util.module_from_spec(spec);spec.loader.exec_module(d3)
    path=ROOT/'crates/fb-templates/templates/directory/detail.html'
    text=path.read_text()
    old=d3.hand(port.port(source('checks/vendor_activity').read_text()),'checks/vendor_activity')
    new="{% include 'checks/vendor_activity.html' %}"
    if old in text:path.write_text(fix(text,old,new))
    else:assert new in text
    print('D7a: regenerated exactly 14 templates with asserted hand fixes')
