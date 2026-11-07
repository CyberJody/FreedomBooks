"""Verify the mechanical template recipe and append-only shared inventories."""
from pathlib import Path
import importlib.util,json,subprocess,re
ROOT=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('hand',ROOT/'tools/d7a_hand_fixes.py')
hand=importlib.util.module_from_spec(spec);spec.loader.exec_module(hand)
assert len(hand.FILES)==14
for name in hand.FILES:
    expected=hand.hand(hand.port.port(hand.source(name).read_text()))
    actual=(ROOT/'crates/fb-templates/templates'/(name+'.html')).read_text()
    assert actual==expected,name
base=json.loads(subprocess.check_output(['git','show','b0b216a:crates/fb-screendiff/screens.json'],cwd=ROOT))
current=json.loads((ROOT/'crates/fb-screendiff/screens.json').read_text())
assert all(current[k]==v for k,v in base.items())
assert list(current)[:len(base)]==list(base)
old_guards=subprocess.check_output(['git','show','b0b216a:electron/main/guards.cjs'],cwd=ROOT,text=True)
new_guards=(ROOT/'electron/main/guards.cjs').read_text()
def commands(text):
    body=re.search(r'const COMMANDS = new Set\(\[(.*?)\]\);',text,re.S).group(1)
    return re.findall(r'''["']([^"']+)["']''',body)
assert commands(new_guards)[:len(commands(old_guards))]==commands(old_guards)
old_smoke=subprocess.check_output(['git','show','b0b216a:electron/scripts/smoke.cjs'],cwd=ROOT,text=True)
new_smoke=(ROOT/'electron/scripts/smoke.cjs').read_text()
addition="    await require('./smoke-d7a.cjs')({ page, company, ids, invoiceId, shot, pass, heading });\n"
assert new_smoke.replace(addition,'')==old_smoke
for name in ['crates/fb-templates/templates/ledger/company_home.html','electron/preload/preload.cjs','electron/main/main.cjs','crates/fb-node/src/lib.rs']:
    original=subprocess.check_output(['git','show','b0b216a:'+name],cwd=ROOT)
    assert (ROOT/name).read_bytes()==original,name
print(f'PASS: exactly 14 mechanically generated D7a templates; {len(base)} inherited screens unchanged; {len(current)-len(base)} D7a screens appended; Home, bridge and security entry points unchanged')
