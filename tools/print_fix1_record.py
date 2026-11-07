"""Record verified fix acceptance in PRINT.md only after the strict checker has passed."""
from pathlib import Path
import json
ROOT=Path(__file__).resolve().parents[1]
OUT=ROOT.parents[1]/'staging/printfix1'
summary=json.loads((OUT/'acceptance.json').read_text())
assert summary['gateTests']['failed']==0
assert summary['screenMatches']==622 and summary['harnessMatches']==128
p=ROOT/'PRINT.md'
text=p.read_text()
old='## Final acceptance\n'
assert text.count(old)==1
text=text.replace(old,'## Original PRINT acceptance (before fix 1)\n')
text+=f'''

## PRINT fix 1 acceptance — 2026-11-02

The standard `scripts/gate.sh` exited 0: {summary['gateTests']['passed']} test executions passed,
0 failed, and {summary['gateTests']['ignored']} existing intentionally ignored oracle executions.
Clippy, workspace tests, alternate JSON features, core isolation and all {summary['screenMatches']}
screen comparisons passed. The inventory has {summary['fullPrintViews']} full print cases; the three
invented credit-print cases are gone. No new normalization or accounting service/migration was added.

The full differential harness MATCHed all {summary['harnessMatches']} scenarios with zero failures
and warnings. The final native rebuild preceded the desktop smoke ({summary['desktopChecks']}/{summary['desktopChecks']})
and D8 smoke ({summary['d8Checks']}/{summary['d8Checks']}); the separate landscape-register smoke also passed.
All {summary['nodeTests']} Node tests passed, including 12 real-filesystem save-policy tests.
The retained reviewer probes refuse the open book directly and through `invoice.pdf`, with the
encrypted book SHA-256 unchanged. Normal and replacement PDF/CSV saves pass in real Electron.

Re-rendered and inspected the eight PDF page PNGs (invoice, P&L, five-page invoice and landscape
register). All eight have white margins (255,255,255), against the review's tan baseline (247,243,236).
The inherited DRAFT placement, register wrapping/number formatting and footer wording remain as
Django renders them. The review's PDF safety audit still finds no paths, keys, scripts or attachments.
The credit-memo detail capture has no print link and MATCHes Django.

The assigned `fbv2_core_prf` / `fbv2_core_prfref` databases were dropped after verification, with live
absence checked. Logs, PDFs, PNGs, save-regression evidence and the strict `acceptance.json` are in
`staging/printfix1/`. Preview entry: `fb://app/`; all smoke windows are closed and no HTTP preview is
retained. Physical printing was dispatch-tested with a callback stub. The branch stops before merge.
'''
p.write_text(text)
print('PRINT.md now records the completed PRINT fix gates and current evidence')
