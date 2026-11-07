"""Record completed acceptance from the validated PRINT evidence, without rerunning tests."""
import json
from pathlib import Path

root = Path(__file__).resolve().parents[1]
out = root.parents[1] / 'staging/print'
result = json.loads((out / 'acceptance.json').read_text())
counts = result['gate_test_executions']
assert counts == {'passed': 1495, 'failed': 0, 'ignored': 2}
assert result['harness_expected'] == result['harness_completed'] == 128
document = root / 'PRINT.md'
text = document.read_text()
marker = '## Final acceptance'
assert marker not in text
document.write_text(text + f'''
{marker}

The final standard gate exited 0: {counts['passed']} test executions passed, none failed. The two
ignored executions are the existing intentionally ignored glibc oracle diagnostic, once per feature
configuration. Clippy, workspace tests, alternate JSON features, core isolation and all 625 screen
comparisons passed. All 128 full differential-harness scenarios MATCH with zero failures and warnings.

The two ambient local clocks (23:30 and 00:30) each MATCH all 625 screens; all 1,250 normalized
Python/Rust files are byte-identical between runs. The native build preceded the real Electron smoke:
45 desktop checks plus 10 report checks passed, with a separate landscape-register PDF smoke passing.
All 13 Node tests and the focused print regression run passed.

Final evidence includes 113 full print views and their bottom captures at 1440×1000, eight PDF page
PNGs, the PDF/text files and labelled review contact sheets. The validated machine-readable result is
`staging/print/acceptance.json`; `staging/print/evidence.json` includes dimensions and SHA-256 hashes.
Full foreground logs are retained there as `gate.log`, `harness.log`, `clock.log`, `native.log`,
`smoke.log`, `smoke-register.log`, `node-tests.log` and `cleanup.log`.

Both assigned databases (`fbv2_core_prref`, `fbv2_core_pr`) were dropped and their absence verified.
The clock-proof clone was dropped on exit. The desktop preview entry is `fb://app/`; no HTTP preview
server is retained. The branch remains separate for lead review and merge.
''')
print('Recorded completed PRINT acceptance in PRINT.md')
