"""INT-HOTFIX: journal hidden-key fixtures (review 3 of FIX3, finding 1).

Django's `forms.UUIDField` strips, then tests required: a whitespace-only `request_key` on a new journal is
"This field is required.", the same as FIX3c's `dfix3c-key-transfer-blank`.
"""
from copy import deepcopy
from pathlib import Path
import json

root = Path(__file__).resolve().parents[1]
p = root / 'crates/fb-screendiff/screens.json'
s = json.loads(p.read_text())


def clone(base, **post):
    item = deepcopy(s[base])
    item['post'].update(post)
    return item


new = {
    'dfix3c-key-journal-blank-spaces': clone('journal-new-invalid', request_key='   '),
    'dfix3c-key-journal-blank-nbsp': clone('journal-new-invalid', request_key=' '),
}
assert not set(new) & set(s)
s.update(new)
p.write_text(json.dumps(s, indent=2, ensure_ascii=False) + '\n')
print(f'{len(new)} added, {len(s)} total fixtures')
