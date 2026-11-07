"""Prove the D5 port is exactly its 22 frozen sources plus recorded hand fixes."""
from d5_hand_fixes import ROOT,FILES,source,port,hand
assert len(FILES)==22 and len(set(FILES))==22
for name in FILES:
    expected=hand(port.port(source(name).read_text()))
    actual=(ROOT/'crates/fb-templates/templates'/(name+'.html')).read_text()
    assert expected==actual,name
for fragment,host in [('credits/customer_activity','directory/detail'),('prepayments/customer_deposits','directory/detail'),('credits/invoice_credits','sales/invoices/detail'),('prepayments/invoice_deposits','sales/invoices/detail')]:
    assert "{% include '"+fragment+".html' %}" in (ROOT/'crates/fb-templates/templates'/(host+'.html')).read_text()
print('D5 port verification: exactly 22 templates reproduce byte-for-byte; all four fragments are included')
