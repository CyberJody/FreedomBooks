"""After mechanically porting the customer print template, apply the business-details extension.

The company edit form is a new desktop template, ledger/company_form.html.
"""
from pathlib import Path
import sys
ROOT=Path(__file__).resolve().parents[1]
path=ROOT/'crates/fb-templates/templates/sales/print/document.html'
checking='--check' in sys.argv
text=(ROOT/'crates/fb-templates/tests/fixtures/company_document_before.html').read_text() if checking else path.read_text()
header='{% if doc.company_logo_url %}<img class="business-logo" src="{{ doc.company_logo_url }}" alt="Company logo">{% endif %}'
contacts="{% for key in ['street1','street2','city','region','postal_code','country','phone','email','website'] %}{% if doc.business[key] %}<p class=\"business-contact\">{{ doc.business[key] }}</p>{% endif %}{% endfor %}{% if doc.business.tax_id %}<p class=\"business-contact\">Tax ID: {{ doc.business.tax_id }}</p>{% endif %}"
pairs=[
    ('<div class="company-block"><h1 class="company-name">{{ doc.company.name }}</h1>', '<div class="company-block">'+header+'<h1 class="company-name">{{ doc.company.name }}</h1>'+contacts),
    ('<div class="message-box"><p class="box-label">Memo</p>', '<div class="message-box">{% if doc.business.payment_instructions %}<div class="payment-instructions"><p class="box-label">Payment instructions</p><p>{{ doc.business.payment_instructions|linebreaksbr }}</p></div>{% endif %}<p class="box-label">Memo</p>'),
]
for old,new in pairs:
    assert text.count(old)==1,old
    text=text.replace(old,new)
if checking:
    assert text==path.read_text(), 'the recorded print transform differs from the checked-in template'
    print('Company document template transform matches.')
else:
    path.write_text(text)
