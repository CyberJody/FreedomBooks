"""The complete comparer must flag a changed submitted undo-key echo."""
from pathlib import Path
import json
import os
import re
import shutil
import subprocess
root=Path(__file__).resolve().parents[1]
out=root.parents[1]/'staging/dfix3'
folder=out/'mutant-undo-echo'
django=folder/'django'
if folder.exists():shutil.rmtree(folder)
shutil.copytree(out/'screens/django',django)
key=json.loads((root/'crates/fb-screendiff/screens.json').read_text())['d7b-undo-invalid']['post']['undo_key']
p=django/'d7b-undo-invalid.html';t=p.read_text()
old=f'name="undo_key" value="{key}"';new='name="undo_key" value="00000000-0000-0000-0000-000000df3bad"'
assert t.count(old)==1;p.write_text(t.replace(old,new))
env=dict(os.environ,TMPDIR=str(out/'tmp'))
log=folder/'run.log'
with log.open('w') as stream:
    result=subprocess.run([str(root/'target/release/fb-screendiff'),str(root/'crates/fb-screendiff/scenarios/screens-d7b.json'),str(django),str(folder)],cwd=root,env=env,stdout=stream,stderr=subprocess.STDOUT)
t=log.read_text();found=re.findall(r'^DIFF ([^\s:]+)',t,re.M)
assert result.returncode==1 and found==['d7b-undo-invalid'],(result.returncode,found)
print(t.splitlines()[-1])
print('MUST-DIFF PASS: changed submitted undo_key caught by full executable; exactly d7b-undo-invalid differs; exit 1')
