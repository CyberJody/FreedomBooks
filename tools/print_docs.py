from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
p=ROOT/'crates/fb-screendiff/NORMALIZATION.md'
if not p.exists():p=ROOT/'NORMALIZATION.md'
assert p.exists()
p.write_text(p.read_text()+'''

## PRINT

- The desktop adds one hidden `fb_command_token` inside the exact `print-now` button. Remove only
  that input in that button with its frozen label; no other button attributes or document text are
  rewritten. A must-DIFF test preserves altered labels and token-like user text elsewhere.
- Generated print instants use the same `clock.json` local-noon instant as the entry/reminder date
  defaults. Print-generation instants, document dates/numbers, balances, taxes, totals and footers
  are compared exactly. Stored completion/undo instants use the scoped rule below.
''')
print('PRINT: documented the single capability normalization and missing reference credit-print URL')
