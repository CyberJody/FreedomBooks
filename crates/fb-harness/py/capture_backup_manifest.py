"""Capture a small accounting-manifest vector from the frozen-Python harness.

Run backups-lifecycle.json first, then pass its *.python.json dump to this file.
The vector intentionally excludes engine-specific tables/schema, which each
native verifier independently checks in full before returning this result.
"""
import json
from pathlib import Path
import sys

dump = json.loads(Path(sys.argv[1]).read_text())
row = dump['backups']['7']
assert row['kind'] == 'snapshot'
assert row['company_count'] == 1 and row['account_count'] == 3
assert row['companies'][0]['name'] == 'Synthetic backup company'
assert row['companies'][0]['trial_balance'] == {'debit': '123.45', 'credit': '123.45'}
assert row['companies'][0]['journals'] == {'posted': 1, 'draft': 1}
out = Path(__file__).resolve().parents[1] / 'tests/data/backups-manifest.json'
out.parent.mkdir(exist_ok=True)
out.write_text(json.dumps(row, indent=2, ensure_ascii=False) + '\n')
print('Captured Python raw-SQL manifest vector:', out)
