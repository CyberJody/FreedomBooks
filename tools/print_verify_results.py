"""Summarize real PRINT logs; final mode refuses incomplete or failed acceptance evidence."""
from pathlib import Path
import json
import re
import subprocess
import sys
ROOT=Path(__file__).resolve().parents[1]
OUT=ROOT.parents[1]/'staging/print'
expected={p.stem for p in (ROOT/'crates/fb-harness/scenarios').glob('*.json')}
harness=(OUT/'harness.log').read_text() if (OUT/'harness.log').exists() else ''
cases=re.findall(r'^== (\S+) \(python ',harness,re.M)
matches=re.findall(r'^MATCH — .*$',harness,re.M)
summary={'harness_expected':len(expected),'harness_completed':len(matches),'harness_fail_lines':len(re.findall(r'^FAIL',harness,re.M))}
if '--progress' in sys.argv:
    print(json.dumps(summary,indent=2))
    raise SystemExit(0)
assert set(cases)==expected and len(cases)==len(expected) and len(matches)==len(expected),summary
assert all('0 failures, 0 warnings' in line for line in matches), 'inspect harness results'
assert not summary['harness_fail_lines']
gate=(OUT/'gate.log').read_text()
assert gate.rstrip().endswith('gate: clippy, check_json_features.sh (workspace tests + JSON feature unification), core isolation and screen diff passed')
counts=re.findall(r'test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored',gate)
summary['gate_test_executions']={key:sum(int(row[index]) for row in counts) for index,key in enumerate(['passed','failed','ignored'])}
assert summary['gate_test_executions']['failed']==0
screens=json.loads((ROOT/'crates/fb-screendiff/screens.json').read_text())
summary['screens']=len(screens)
summary['full_print_views']=sum(k.startswith('print-') and 'template' not in value for k,value in screens.items())
assert f'SCREENS MATCH — {len(screens)} screens, 0 with differences' in gate
clock=(OUT/'clock.log').read_text()
assert clock.count(f'SCREENS MATCH — {len(screens)} screens, 0 with differences')==2
hashes=json.loads((OUT/'clock-proof/normalized-hashes.json').read_text())
assert len(hashes)==len(screens)*2
assert f'CLOCK PROOF PASS: 23:30 and 00:30 local; {len(hashes)} normalized files byte-identical' in clock
summary['clock_identical_normalized_files']=len(hashes)
smoke=(OUT/'smoke.log').read_text()
summary['desktop_smoke_checks']=int(re.search(r'SMOKE OK: (\d+) checks',smoke)[1])
summary['report_smoke_checks']=int(re.search(r'SMOKE D8 OK: (\d+) checks',smoke)[1])
assert 'PRINT invoice: native Save dialog' in smoke and 'PRINT profit-loss: native Save dialog' in smoke
assert 'SMOKE PRINT REGISTER OK' in (OUT/'smoke-register.log').read_text()
pagination=json.loads((OUT/'print-pagination.json').read_text())
assert pagination['rows']==40 and pagination['pages']==5 and pagination['total']=='1000.00' and pagination['footersEveryPage']
summary['multi_page_invoice']=pagination
node=(OUT/'node-tests.log').read_text()
summary['node_tests']=int(re.search(r'(?:ℹ|#) pass (\d+)',node)[1])
assert re.search(r'(?:ℹ|#) fail 0\b',node)
assert 'test print_invoice_uses_stored_revision_and_only_current_open_balance_without_writes ... ok' in (OUT/'printing-tests.log').read_text()
assert 'UTC-labelled balance date must follow the print clock' in (OUT/'printing-tests-utc-before.log').read_text()
cleanup=json.loads((OUT/'database-cleanup.json').read_text())
assert set(cleanup['dropped'])=={'fbv2_core_pr','fbv2_core_prref'} and cleanup['remaining']==[]
summary['databases_dropped']=cleanup['dropped']
inventory=json.loads((OUT/'evidence.json').read_text())
assert inventory['printViews']==summary['full_print_views'] and inventory['screenshots']==summary['full_print_views']
assert inventory['pdfPageRenders']==8
summary['print_screenshots']=inventory['screenshots']
summary['print_bottom_screenshots']=inventory['bottomScreenshots']
summary['pdf_page_pngs']=inventory['pdfPageRenders']
summary['branch']=subprocess.check_output(['git','branch','--show-current'],cwd=ROOT,text=True).strip()
summary['verified_head']=subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip()
assert summary['branch']=='print'
(OUT/'acceptance.json').write_text(json.dumps(summary,indent=2)+'\n')
print('PRINT ACCEPTANCE PASS')
print(json.dumps(summary,indent=2))
