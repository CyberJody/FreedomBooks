"""Regenerate precisely D5's 22 templates, recording asserted adaptations."""
from pathlib import Path
import importlib.util
import re
ROOT = Path(__file__).resolve().parents[1]
REF = Path('/mnt/Data/Projects/FreedomBooks/wt/reference')
def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod
port = module('port', ROOT/'crates/fb-templates/tools/port_template.py')
FILES = [owner+'/'+name for owner, names in [
    ('credits', 'list detail form apply unapply void invoice_credits customer_activity refund_form refund_detail refund_void'),
    ('prepayments', 'list detail form apply unapply void invoice_deposits customer_deposits refund_form refund_detail refund_void'),
] for name in names.split()]
def source(name):
    return REF/name.split('/')[0]/'templates'/(name+'.html')
def fix(text, old, new):
    assert old in text, old
    return text.replace(old, new)
def hand(text):
    for owner in ['memo','deposit','refund']:
        old = '{% attachments_panel document='+owner+'.identity %}'
        if old in text:
            panel = port.port(source('attachments/panel').read_text())
            panel = fix(panel, '<input type="hidden" name="csrfmiddlewaretoken" value="{{ csrf_token }}">', '{{ attachments.csrf_token|safe }}')
            text = fix(text,old,'{% with add_url=attachments.add_url, request_key=attachments.request_key, allowed=attachments.allowed, max_mb=attachments.max_mb, max_count=attachments.max_count, files=attachments.files, removed=attachments.removed, full=attachments.full, notes=attachments.notes %}'+panel+'{% endwith %}')
    for old,new in [
        ('{% if form.errors %}','{% if form.has_errors %}'),
        ('{% if form.errors or formset.total_error_count %}','{% if form.has_errors or formset.total_error_count %}'),
        ('{% if filter_form.errors %}','{% if filter_form.has_errors %}'),
        ('{% for line in formset %}','{% for line in formset.forms %}'),
        ('{% for row in formset %}','{% for row in formset.forms %}'),
        ('{% for field in filter_form %}','{% for field in filter_form.fields %}'),
        ('{% for field in form %}','{% for field in form.fields %}'),
        ('{% if form.fields.invoice and not form.fields.invoice.queryset.exists %}','{% if invoice_choice and not invoice_choices_exist %}'),
        ('{% if form.fields.deposit and not form.fields.deposit.queryset.exists %}','{% if deposit_choice and not deposit_choices_exist %}'),
        ('a.lines.all','a.lines'),
        ('memo.applied or memo.refunded','memo.has_live_usage'),
        ('memo.available > 0','memo.available_positive'),
        ('deposit.remaining > 0','deposit.remaining_positive'),
        ('application.lines.all','application.lines'),
    ]:
        if old in text: text = fix(text,old,new)
    for page in ['page','applications','refunds','credits_page']:
        text = re.sub(r'{% for (\w+) in '+page+r' %}',r'{% for \1 in '+page+'.object_list %}',text)
    text = re.sub(r'{% journal_evidence_link_text company ([\w.]+) %}',r'{{ evidence[\1] }}',text)
    text = re.sub(r'{{ ([\w.]+)\|journal_link_label }}',r'{{ \1.label }}',text)
    for name in ['request_key','review_token','date']:
        text = re.sub(r'{{ form\.'+name+r'(\.[\w]+)? }}',lambda m:'{{ form.f.'+name+(m[1] or '.widget')+('' if m[1] in ['.label','.help_text','.value'] else '|safe')+' }}',text)
    for expr in ['csrf_token','form.non_field_errors','filter_form.non_field_errors','form.as_p','filter_form.as_p','formset.management_form','formset.non_form_errors','line.non_field_errors','row.non_field_errors','field.label_tag','field.errors','hidden.errors','row.field.errors']:
        old='{{ '+expr+' }}'
        if old in text: text=fix(text,old,'{{ '+expr+'|safe }}')
    for name in ['field','hidden','row.field']:
        old='{{ '+name+' }}'
        if old in text:text=fix(text,old,'{{ '+name+'.widget|safe }}')
    return text
if __name__ == '__main__':
    for name in FILES:
        dest=ROOT/'crates/fb-templates/templates'/(name+'.html')
        dest.parent.mkdir(parents=True,exist_ok=True)
        dest.write_text(hand(port.port(source(name).read_text())))
    # Earlier waves embedded these fragments until their owning wave arrived.
    for file,modname in [('directory/detail.html','d3'),('sales/invoices/detail.html','d4')]:
        path=ROOT/'crates/fb-templates/templates'/file
        text=path.read_text()
        if modname=='d3':
            mod=module('d3',ROOT/'tools/d3_hand_fixes.py')
            for fragment in ['credits/customer_activity','prepayments/customer_deposits']:
                old=mod.hand(port.port(source(fragment).read_text()),fragment)
                include="{% include '"+fragment+".html' %}"
                if old in text:text=fix(text,old,include)
                else: assert include in text, fragment
        else:
            # D4's adapted embedded blocks are replaced by boundaries in its hand-fix script below.
            for fragment,begin,end in [('credits/invoice_credits','<h3>Credit memo applications</h3>','{% if credits_page.has_previous %}<a href="?credits_page={{ credits_page.previous_page_number }}">Newer credit applications</a>{% endif %}'),('prepayments/invoice_deposits','<h3>Customer deposits applied</h3>','<tr><td colspan="5">No customer deposits applied.</td></tr>{% endfor %}</tbody></table>')]:
                include="{% include '"+fragment+".html' %}"
                if begin in text:
                    a=text.index(begin); b=text.index(end,a)+len(end)
                    text=fix(text,text[a:b],include)
                else: assert include in text, fragment
        path.write_text(text)
    print('D5: regenerated exactly 22 templates with asserted hand fixes')
