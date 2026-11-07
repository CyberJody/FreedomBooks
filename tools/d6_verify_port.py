"""Check all 18 D6 templates against the frozen source and asserted hand adaptations."""
from d6_hand_fixes import ROOT, FILES, source, port, hand
assert len(FILES)==18 and len(set(FILES))==18
for name in FILES:
    expected=hand(port.port(source(name).read_text()))
    actual=(ROOT/'crates/fb-templates/templates'/(name+'.html')).read_text()
    assert actual==expected,name
assert "{% include 'payables/bills/vendor_activity.html' %}" in (ROOT/'crates/fb-templates/templates/directory/detail.html').read_text()
print('D6 port verification: exactly 18 templates reproduce byte-for-byte')
