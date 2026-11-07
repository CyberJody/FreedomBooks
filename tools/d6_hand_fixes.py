"""Mechanical D6 port followed by asserted old/new template adaptations."""
from pathlib import Path
import importlib.util
import re

ROOT = Path(__file__).resolve().parents[1]
REF = Path('/mnt/Data/Projects/FreedomBooks/wt/reference')
spec = importlib.util.spec_from_file_location('port', ROOT/'crates/fb-templates/tools/port_template.py')
port = importlib.util.module_from_spec(spec)
spec.loader.exec_module(port)
FILES = ['payables/bills/'+n for n in ['list','detail','document','form','line_fields','review','revision','void','vendor_activity','calendar']]
FILES += ['payables/payments/'+n for n in ['list','detail','form','reverse']]
FILES += ['transfers/'+n for n in ['list','detail','form','void']]

def source(name):
    return REF/name.split('/')[0]/'templates'/(name+'.html')

def fix(text, old, new):
    assert old in text, old
    return text.replace(old,new)

def hand(text):
    for fragment in ['assistant/receipt_form_panel','assistant/bill_actions']:
        old = "{% include '"+fragment+".html' %}"
        if old in text:
            text = fix(text,old,port.port(source(fragment).read_text()))
    # D9 owns attachment functionality; inline its frozen panel for exact detail markup.
    for owner in ['bill','payment','transfer']:
        old = '{% attachments_panel document='+owner+'.identity %}'
        if old in text:
            panel = port.port(source('attachments/panel').read_text())
            panel = fix(panel, '<input type="hidden" name="csrfmiddlewaretoken" value="{{ csrf_token }}">', '{{ attachments.csrf_token|safe }}')
            text = fix(text,old,'{% with add_url=attachments.add_url, request_key=attachments.request_key, allowed=attachments.allowed, max_mb=attachments.max_mb, max_count=attachments.max_count, files=attachments.files, removed=attachments.removed, full=attachments.full, notes=attachments.notes %}'+panel+'{% endwith %}')
    changes = [
        ('{% superseded_document_number company current as superseded_number %}','{% set superseded_number = current.superseded_number %}'),
        ('{{ bill.identity.account_references.all }}','{{ references }}'),
        ('in bill.identity.account_references.all','in references'),
        ('{{ form.errors }}','{{ form.has_errors }}'),
        ('{% if form.errors or formset.total_error_count %}','{% if form.has_errors or formset.total_error_count %}'),
        ('{% if filter_form.errors %}','{% if filter_form.has_errors %}'),
        ('{% for line in formset %}','{% for line in formset.forms %}'),
        ('{% for row in formset %}','{% for row in formset.forms %}'),
        ('{% for field in row %}','{% for field in row.fields %}'),
        ('{% for field in formset.empty_form %}','{% for field in formset.empty_form.fields %}'),
        ('{% for field in form %}','{% for field in form.fields %}'),
        ('{% for field in filter_form %}','{% for field in filter_form.fields %}'),
        ('form.fields.vendor.choices','vendor_choices'),
        ('form.vendor.value','vendor_value'),
        ('{{ day.date.day }}','{{ day.day_number }}'),
        ("{{ name|slice(\":3\") }}",'{{ name[:3] }}'),
        ('{{ current.created_at }}','{{ current.created_at_display }}'),
    ]
    for old,new in changes:
        if old in text: text = fix(text,old,new)
    for page in ['page','history_page','allocations_page','vendor_bills','vendor_payments']:
        for var in ['rev','a','payment','trf','p','r']:
            old='{% for '+var+' in '+page+' %}'
            if old in text: text=fix(text,old,'{% for '+var+' in '+page+'.object_list %}')
    text=re.sub(r'{% journal_evidence_link_text company ([\w.]+) %}',r'{{ evidence[\1] }}',text)
    # Named bound fields live under f; the form itself remains iterable through fields.
    names=['date','from_account','to_account','amount','memo','vendor','request_key','expected_revision','review_token','reading']
    for name in names:
        text=re.sub(r'{{ form\.'+name+r'(\.[\w]+)? }}',lambda m:'{{ form.f.'+name+(m[1] or '.widget')+('' if m[1] in ['.label','.help_text','.value'] else '|safe')+' }}',text)
    for expr in ['csrf_token','form.non_field_errors','filter_form.non_field_errors','form.as_p','filter_form.as_p','formset.management_form','formset.non_form_errors','line.non_field_errors','row.non_field_errors','field.label_tag','field.errors','hidden.errors']:
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
    # D3 embedded later-wave fragments until their owning wave arrived. Replace only
    # its bill activity block with the now-ported include, so the actual product and
    # screen diff exercise this registered D6 template.
    d3spec = importlib.util.spec_from_file_location('d3_hand', ROOT/'tools/d3_hand_fixes.py')
    d3 = importlib.util.module_from_spec(d3spec)
    d3spec.loader.exec_module(d3)
    fragment = 'payables/bills/vendor_activity'
    old = d3.hand(port.port(source(fragment).read_text()), fragment)
    include = "{% include 'payables/bills/vendor_activity.html' %}"
    detail = ROOT/'crates/fb-templates/templates/directory/detail.html'
    text = detail.read_text()
    if old in text:
        text = fix(text, old, include)
        detail.write_text(text)
    else:
        assert include in text
    print('D6: regenerated exactly 18 templates with asserted hand fixes')
