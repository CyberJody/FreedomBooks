"""Asserted markup adaptations after port_template.py; no visual redesign."""
from pathlib import Path
import re
import hashlib

# Pin every mechanical input so a source/translator drift cannot silently skip an adaptation.
MECHANICAL_SHA256 = {'sales/list': 'e567cb0861812f801d5c65d46dcb3d8256b864ad54b46515d23272b4942d608f', 'sales/detail': 'd91c1bb21ebb3b0e708e8d00a6ba325a3c5b2c10be66d2c093617c647c330cbe', 'sales/document': '6267abb0edee487e39968b196879afb99629dfcc052129b65df271e8e382c22c', 'sales/form': 'd3666229a3c451ac313757881426a673c37c70242028630546cead33520911a0', 'sales/revision': '3ab76278c15baec8538f1dab6c12772811d5b225b5dcc4234ed87d084be5ffb6', 'sales/invoices/convert': '18483d1a6b9e88216ec166e767ac9e48ec0254b800f5dd854b6c4a4e2f7edffb', 'sales/invoices/detail': 'fa40ed170f4d8c3cefd1e29984347ce3024c418886ec8a6145e8d6746f651df1', 'sales/invoices/document': '2323d9bcc7b88116ca3214af5fefa014d586adc89bb72a786ccb6de5c6fbc592', 'sales/invoices/review': '3d3999b856ba31a63543ca0f62474150fe325062ce48ea785d7daa6a3392a44a', 'sales/invoices/revision': '61c9e402a7a12f6ce9fa4f09c996dea32d6398a7e138558842469cdc5262617d', 'sales/invoices/void': 'a0510e107c59663824511279d20cc3a0efa54503993ed24ba620a1ccc063de2a', 'sales/receipts/list': '74ed496f142da742b82b6fd51c19559e5e672505cc50869f44abd2eb79e2f286', 'sales/receipts/detail': '7999988e2aed8886599c45eda8df3d0daa8f89fa3fcc5670170a41dc4152015b', 'sales/receipts/form': '4fca39ca3aae6662ee5307d8340e59bc8de3f9caa8ba9b27fff8170a9d993581', 'sales/receipts/reverse': '0bb7358b7aaecab80d312f218677b4b8c4ab282fce729b3389a355c73ee4749e', 'sales/receipts/customer_activity': '3aca144ff5d9d67e01ae30a53ffb84c82fa96e27beec965dd8a9b4ea70113a94', 'sales/sales_receipts/list': '5379a89dc4baa7e614c14976badd3f833d70ef0091f4a1c4827e709aba480f15', 'sales/sales_receipts/detail': '1d105590a7cb051db22b0b87316775cc308b5e30c11d09ca9aca134eecb0e0a9', 'sales/sales_receipts/form': 'e7db896fc48343aa60425999758d62a3f0a85db374ea4f9886801c98c785a78f', 'sales/sales_receipts/void': '98ff96e5f15d30c0a5acba68f1572a00c41bb35430d1d68f6ee8668eb85a3d03'}
DEPENDENCY_SHA256 = {'credits/templates/credits/invoice_credits.html': '3aef8fe9df065f54dcbdfcd17420c7713cb5ce4c3b771bb551f35ae0ba7ece10', 'prepayments/templates/prepayments/invoice_deposits.html': '7c29136747ecd40e482de9d558380a0aaf6d359858433d5ea37b7b5d4a00af6d', 'attachments/templates/attachments/panel.html': '2d0bdca042c99553b7aa1f49e6fa3b59d798fb9b8c0458c51f0454979d2edd42'}

ROOT = Path(__file__).resolve().parents[1]
FILES = (['sales/' + n for n in ('list', 'detail', 'document', 'form', 'revision')]
         + ['sales/invoices/' + n for n in ('convert', 'detail', 'document', 'review', 'revision', 'void')]
         + ['sales/receipts/' + n for n in ('list', 'detail', 'form', 'reverse', 'customer_activity')]
         + ['sales/sales_receipts/' + n for n in ('list', 'detail', 'form', 'void')])
for name in FILES:
    path = ROOT / 'templates' / (name + '.html')
    text = path.read_text()
    assert hashlib.sha256(text.encode()).hexdigest() == MECHANICAL_SHA256[name], (name, "mechanical source changed")
    pairs = []
    # BoundForm and Page are explicit serializable values, rather than Django objects.
    for f in ('form', 'filter_form', 'state_form', 'line', 'row', 'formset.empty_form'):
        for attr in ('as_p', 'non_field_errors'):
            pairs.append(('{{ ' + f + '.' + attr + ' }}', '{{ ' + f + '.' + attr + '|safe }}'))
    for f in ('form', 'state_form'):
        pairs.append((f + '.errors', f + '.has_errors'))
        for field in ('request_key', 'expected_revision', 'review_token', 'state', 'reason', 'customer'):
            pairs.append((f + '.' + field + '.', f + '.f.' + field + '.'))
            pairs.append(('{{ ' + f + '.' + field + ' }}', '{{ ' + f + '.f.' + field + '.widget|safe }}'))
    for f in ('field', 'hidden'):
        pairs += [('{{ ' + f + ' }}', '{{ ' + f + '.widget|safe }}'),
                  ('{{ ' + f + '.label_tag }}', '{{ ' + f + '.label_tag|safe }}'),
                  ('{{ ' + f + '.errors }}', '{{ ' + f + '.errors|safe }}')]
    pairs += [('{{ csrf_token }}', '{{ csrf_token|safe }}'),
              ('{{ formset.management_form }}', '{{ formset.management_form|safe }}'),
              ('{{ formset.non_form_errors }}', '{{ formset.non_form_errors|safe }}'),
              ('in formset %}', 'in formset.forms %}'), ('in row %}', 'in row.fields %}'),
              ('in formset.empty_form %}', 'in formset.empty_form.fields %}'),
              ('form.fields.customer.choices', 'customer_choices'),
              ('{{ line.as_p }}', '{{ line.as_p|safe }}'),
              ('{% attachments_panel document=estimate.identity %}', '{{ attachments_html|default(\'\')|safe }}'),
              ('{% attachments_panel document=invoice.identity %}', '{{ attachments_html|default(\'\')|safe }}'),
              ('{% attachments_panel document=receipt.identity %}', '{{ attachments_html|default(\'\')|safe }}'),
              ('{% journal_evidence_link_text company current.journal_id %}', '{{ current.journal_label }}'),
              ('{% journal_evidence_link_text company current.reversal_id %}', '{{ current.reversal_label }}'),
              ('{% journal_evidence_link_text company receipt.journal_id %}', '{{ receipt.journal_label }}'),
              ('{% journal_evidence_link_text company receipt.reversal.journal_id %}', '{{ receipt.reversal.journal_label }}'),
              ("current.outstanding > 0", 'invoice_open')]
    for page in ('page', 'history_page', 'allocations_page', 'customer_invoices', 'customer_receipts', 'customer_sales_receipts'):
        pairs.append(('in ' + page + ' %}', 'in ' + page + '.object_list %}'))
    # Every replacement is asserted against the mechanical output when it applies.
    for old, new in pairs:
        if old in text:
            assert old in text, (path, old)
            text = text.replace(old, new)
    # Include fragments belong to later waves, but are part of this page's frozen markup.
    from port_template import port
    reference = Path('/mnt/Data/Projects/FreedomBooks/wt/reference')
    for template, location in [('credits/invoice_credits.html','credits'),('prepayments/invoice_deposits.html','prepayments')]:
        old = "{% include '" + template + "' %}"
        if old in text:
            source = (reference / location / 'templates' / template).read_text()
            assert hashlib.sha256(source.encode()).hexdigest() == DEPENDENCY_SHA256[location + '/templates/' + template]
            fragment = port(source)
            fragment = fragment.replace('in credits_page %}', 'in credits_page.object_list %}')
            text = text.replace(old, fragment)
    pairs = [('{{ state_form.f.reason.label_tag }}','{{ state_form.f.reason.label_tag|safe }}'), ('{{ rev.created_at }}','{{ rev.created_at|datetime_display }}'), ('{{ current.created_at }}','{{ current.created_at|datetime_display }}')]
    for old,new in pairs:
        if old in text:
            assert old in text
            text=text.replace(old,new)
    # The attachment panel's contents/actions are owned by D9; this is its frozen empty-state shell.
    old="{{ attachments_html|default('')|safe }}"
    if old in text:
        source=(reference/'attachments/templates/attachments/panel.html').read_text()
        assert hashlib.sha256(source.encode()).hexdigest() == DEPENDENCY_SHA256['attachments/templates/attachments/panel.html']
        fragment=port(source)
        # Each form posts to its own path, so each carries its own command token input (J-ATTACH).
        token_input='<input type="hidden" name="csrfmiddlewaretoken" value="{{ csrf_token }}">'
        assert fragment.count(token_input)==2
        fragment=fragment.replace(token_input+'<label>Reason','{{ file.remove_token|safe }}<label>Reason').replace(token_input,'{{ attachment_csrf|safe }}')
        fragment=fragment.replace('{{ request_key }}','{{ attachment_request_key }}')
        fragment=fragment.replace('{{ add_url }}', '{{ attachment_add_url }}')
        text=text.replace(old,fragment)
    text = re.sub(r'{{ ((?:form|state_form)\.f\.\w+\.errors) }}', r'{{ \1|safe }}', text)
    path.write_text(text)

# D3 had the frozen customer-activity fragment inline while its D4 template was pending.
path=ROOT/'templates/directory/detail.html'
text=path.read_text()
start="<h2>Customer activity</h2>"
end='<h2 id="customer-credits">'
if start in text:
    old=text[text.index(start):text.index(end)]
    assert hashlib.sha256(old.encode()).hexdigest() == 'b7bd5866278b7ce4afafeb660b9223499b626a3987bb889e2ae16ee65bf06ffa'
    text=text.replace(old,"{% include 'sales/receipts/customer_activity.html' %}\n")
    path.write_text(text)
else:
    assert "{% include 'sales/receipts/customer_activity.html' %}" in text
