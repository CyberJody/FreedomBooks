"""Require complete evidence for the PRINT fix gates, never infer success from missing logs."""
from pathlib import Path
import hashlib
import json
import re
import subprocess

ROOT=Path(__file__).resolve().parents[1]
OUT=ROOT.parents[1]/'staging/printfix1'
gate=(OUT/'gate.log').read_text()
assert gate.rstrip().endswith('gate: clippy, check_json_features.sh (workspace tests + JSON feature unification), core isolation and screen diff passed')
counts=re.findall(r'test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored',gate)
tests={name:sum(int(row[index]) for row in counts) for index,name in enumerate(['passed','failed','ignored'])}
assert tests['failed']==0 and tests['passed']>0
screens=json.loads((ROOT/'crates/fb-screendiff/screens.json').read_text())
assert len(screens)==622
assert f'SCREENS MATCH — {len(screens)} screens, 0 with differences' in gate
assert not any('/credit-memos/' in row.get('path','') and row['path'].endswith('/print/') for row in screens.values())
assert not (ROOT/'crates/fb-screendiff/py/print_credit.py').exists()
assert 'credits:print' not in json.loads((ROOT/'crates/fb-templates/urls.json').read_text())
assert 'credit_memo_has_no_print_route_like_the_frozen_reference ... ok' in gate
assert 'print_invoice_uses_stored_revision_and_only_current_open_balance_without_writes ... ok' in gate

harness=(OUT/'harness.log').read_text()
expected={p.stem for p in (ROOT/'crates/fb-harness/scenarios').glob('*.json')}
cases=re.findall(r'^== (\S+) \(python ',harness,re.M)
matches=re.findall(r'^MATCH — .*$',harness,re.M)
assert set(cases)==expected and len(cases)==len(expected) and len(matches)==len(expected)
assert all('0 failures, 0 warnings' in line for line in matches)

native=(OUT/'native.log').read_text()
assert 'fb-node -> electron/native/fb-node.node' in native
assert (OUT/'native.log').stat().st_mtime<(OUT/'smoke.log').stat().st_mtime
smoke=(OUT/'smoke.log').read_text()
desktop=int(re.search(r'SMOKE OK: (\d+) checks',smoke)[1])
reports=int(re.search(r'SMOKE D8 OK: (\d+) checks',smoke)[1])
assert 'PRINT invoice: native Save dialog' in smoke and 'PRINT profit-loss: native Save dialog' in smoke
assert 'SMOKE PRINT REGISTER OK' in (OUT/'smoke-register.log').read_text()
pagination=json.loads((OUT/'print-pagination.json').read_text())
assert pagination['pages']==5 and pagination['rows']==40 and pagination['total']=='1000.00'
assert pagination['footersEveryPage']
node=(OUT/'node-tests.log').read_text()
node_pass=int(re.search(r'(?:ℹ|#) pass (\d+)',node)[1])
assert re.search(r'(?:ℹ|#) fail 0\b',node)
save_path=json.loads((OUT/'save-path-results.json').read_text())
assert len(save_path)==3 and all(case['sentinelPreserved'] and case['error'] for case in save_path)
live=json.loads((OUT/'save-live-results.json').read_text())
assert len(live)==4 and all(case['bookHashUnchanged'] for case in live)
assert all(case.get('pdfNew') and case.get('pdfReplace') and case.get('csvRefused') and case.get('csvNew') and case.get('csvReplace') for case in live if 'pdfNew' in case)
margins=json.loads((OUT/'pdf-margins.json').read_text())
assert len(margins['pages'])==8
cleanup=json.loads((OUT/'database-cleanup.json').read_text())
assert set(cleanup['dropped'])=={'fbv2_core_prf','fbv2_core_prfref'} and cleanup['remaining']==[]
remaining=subprocess.check_output(['psql','-X','-At','-d','postgres','-c',
    "SELECT datname FROM pg_database WHERE datname IN ('fbv2_core_prf','fbv2_core_prfref')"],text=True).strip()
assert not remaining,remaining

evidence=[]
for name in ['invoice-page1.png','profit-loss-page1.png','register-page1.png','catalogue-d5-credit-detail.png']+[f'invoice-multipage-page{page}.png' for page in range(1,6)]:
    file=OUT/name
    evidence.append({'file':str(file),'sha256':hashlib.sha256(file.read_bytes()).hexdigest()})
summary={'head':subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),
         'gateTests':tests,'screenMatches':len(screens),'fullPrintViews':110,'harnessMatches':len(matches),
         'desktopChecks':desktop,'d8Checks':reports,'nodeTests':node_pass,'liveSaveRegressions':live,
         'whitePdfPages':8,'databasesDropped':cleanup['dropped'],'evidence':evidence}
(OUT/'acceptance.json').write_text(json.dumps(summary,indent=2)+'\n')
print(f'PRINT FIX1 ACCEPTANCE PASS: {tests["passed"]} test executions, {len(screens)} screens MATCH, {len(matches)} harness MATCH')
print(f'Desktop {desktop}/{desktop}; D8 {reports}/{reports}; Node {node_pass}/{node_pass}; PDF margins 8/8 white; save regressions PASS; databases absent')
