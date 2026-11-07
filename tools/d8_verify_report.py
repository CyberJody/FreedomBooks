"""Independent rendered-evidence assertions for paging, revisions and CSV safety."""
from pathlib import Path
from html.parser import HTMLParser
import csv
import io
import json
import os

ROOT=Path(__file__).resolve().parents[1]
OUT=Path(os.environ.get('FB_SCREENDIFF_OUT',ROOT/'target/screendiff'))
class Rows(HTMLParser):
    def __init__(self):
        super().__init__();self.rows=[];self.current=None
    def handle_starttag(self,tag,attrs):
        if tag=='tr':
            self.current=[dict(attrs).get('class',''),[]]
    def handle_data(self,data):
        if self.current is not None:self.current[1].append(data)
    def handle_endtag(self,tag):
        if tag=='tr' and self.current is not None:
            self.rows.append((self.current[0],' '.join(self.current[1]).split()));self.current=None
def rows(side,name):
    p=Rows();p.feed((OUT/side/(name+'.html')).read_text());return [(kind,' '.join(text)) for kind,text in p.rows]

screens=json.loads((ROOT/'crates/fb-screendiff/screens.json').read_text())
for name in screens:
    a=OUT/'rust'/(name+'.django.norm.html');b=OUT/'rust'/(name+'.norm.html')
    assert a.read_bytes()==b.read_bytes(),name
for side in ['django','rust']:
    second=rows(side,'d8-gl-page-second')
    assert any(kind=='gl-opening' and text=='Balance forward 100.00' for kind,text in second),second[:3]
    assert any(kind=='fin-total' and 'Total Paging evidence' in text for kind,text in second)
    first=rows(side,'d8-gl-page-first')
    assert any(kind=='gl-continued' and 'Continued on the next page'==text for kind,text in first)
    bank=[text for kind,text in rows(side,'d8-gl-group-bank') if kind.startswith('gl-line')]
    older=next(i for i,text in enumerate(bank) if 'Revised older journal' in text)
    newer=next(i for i,text in enumerate(bank) if 'd8-revision-new' in text)
    assert older<newer,(older,newer)
    assert '3.00' in bank[older] and '2.00' in bank[newer]
    bs=list(csv.reader(io.StringIO((OUT/side/'d8-bs-accrual-csv.html').read_text())))
    assert any("'=Fixed \"assets\", tools" in row for row in bs),bs
    assert any(any(cell.startswith('-') for cell in row[-1:]) for row in bs if row and row[0] in ('account','total'))
print(f'D8 report evidence: {len(screens)} normalized states equal; both sides carry 100.00 forward, order revised journals by creation, and preserve CSV formula guards and signed amounts')


class DateInputs(HTMLParser):
    def __init__(self):
        super().__init__(); self.values=[]
    def handle_starttag(self, tag, attrs):
        attrs=dict(attrs)
        if tag=='input' and attrs.get('name')=='date':
            self.values.append(attrs.get('value'))

defaults=('invoice-new','journal-new','invoice-new-tax-off','estimate-new','estimate-convert',
          'invoice-void','receipt-new','receipt-customer','receipt-reverse','sales-receipt-new',
          'sales-receipt-void','d6-bill-form','d6-bill-void','d6-bill-payment-form',
          'd6-bill-payment-reverse','d6-transfer-form','d6-transfer-void')
for side in ('django','rust'):
    for name in defaults:
        inputs=DateInputs();inputs.feed((OUT/side/(name+'.html')).read_text())
        assert inputs.values==['2026-11-01'],(side,name,inputs.values)
print('D8 clock evidence: all 17 inherited default-date forms receive the same literal date on both sides')
