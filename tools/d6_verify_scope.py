"""Prove sibling-wave shared inventories retain every inherited entry unchanged."""
from pathlib import Path
import json
import re
import subprocess
BASE='779adb1'
def original(name):
    return subprocess.check_output(['git','show',BASE+':'+name],text=True)
file='crates/fb-screendiff/screens.json'
old=json.loads(original(file));new=json.loads(Path(file).read_text())
assert all(new[key]==value for key,value in old.items())
def commands(s):
    return re.search(r'const COMMANDS = new Set\(\[(.*?)\]\);',s,re.S)[1]
file='electron/main/guards.cjs'
a=re.findall(r"'([^']+)'",commands(original(file)));b=re.findall(r"'([^']+)'",commands(Path(file).read_text()))
assert b[:len(a)]==a and len(set(b))==len(b)
file='electron/scripts/smoke.cjs';s=Path(file).read_text()
s=s.replace("    await require('./smoke-d6.cjs')({ page, company, ids, shot, pass, heading });\n\n",'')
s=s.replace('    // D6: schema creation and encrypted-book I/O can queue behind concurrent wave gates.\n    page.setDefaultNavigationTimeout(180000);\n    page.setDefaultTimeout(180000);\n','')
assert s==original(file)
print(f'D6 scope: {len(old)} inherited screens unchanged; {len(new)-len(old)} appended states; {len(b)-len(a)} commands appended; smoke additions only')
