"""Verify the entire D8 template inventory, mechanically reproduced from the frozen reference."""
from d8_hand_fixes import ROOT, FILES, source, port, hand
assert len(FILES)==8 and len(set(FILES))==8
for name in FILES:
    assert (ROOT/'crates/fb-templates/templates'/(name+'.html')).read_text()==hand(port.port(source(name).read_text()),name),name
print('D8 port verification: exactly eight templates reproduce byte-for-byte')
