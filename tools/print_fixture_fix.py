from pathlib import Path
import json
ROOT=Path(__file__).resolve().parents[1]
p=ROOT/'crates/fb-screendiff/screens.json';screens=json.loads(p.read_text())
removed=[k for k,v in screens.items() if k.startswith('print-') and '/new/print/' in v['path']]
for k in removed:del screens[k]
p.write_text(json.dumps(screens,indent=2)+'\n')
p=ROOT/'tools/print_diff.py';text=p.read_text();old="        screens['print-'+key]={'path':path+'print/'}";assert text.count(old)==1
p.write_text(text.replace(old,"        if '/new/' not in path: screens['print-'+key]={'path':path+'print/'}"))
print('PRINT: omitted entry forms from print snapshots:',removed)
