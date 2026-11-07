"""FIX3c: hidden-key fixtures for Python's `uuid.UUID` grammar (review 2 of FIX3, finding 1).

The first five are the reviewer's DIFF probes (r2-undo-12/13/14, r2-begin-4, r2-finish-0); the rest are
near-misses Python refuses, whitespace Python's `int()` strips, and the Django-form strip on transfers.
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


undo, begin, finish, transfer = ('dfix3b-undo-review-0', 'dfix3b-begin-valid-revision-0', 'dfix3b-save-stale-noop',
                                 'd6-transfer-same-bank-invalid')
new = {
    # Reviewer's DIFF cases: Python accepts each key, so the later refusal shows.
    'dfix3c-key-undo-0x': clone(undo, undo_key='0x000000000000000000000000000df3', revision='abc', reason=''),
    'dfix3c-key-undo-plus': clone(undo, undo_key='+0000000000000000000000000000df3', revision='abc', reason=''),
    'dfix3c-key-undo-underscore': clone(undo, undo_key='0000_000000000000000000000000df3', revision='abc', reason=''),
    'dfix3c-key-begin-0x': clone(begin, request_key='0x00000000000000000000000000d7b0'),
    'dfix3c-key-finish-0x': clone(finish, completion_key='0x000000000000000000000000000000'),
    # More accepted forms: `0x_`, surrounding whitespace, a Unicode digit.
    'dfix3c-key-undo-0x-underscore': clone(undo, undo_key='0x_00000000000000000000000000df3', revision='abc', reason=''),
    'dfix3c-key-undo-spaces': clone(undo, undo_key=' ' + '0' * 27 + 'df3 ', revision='abc', reason=''),
    'dfix3c-key-finish-space': clone(finish, completion_key=' ' + '0' * 31),
    'dfix3c-key-begin-plus-0x': clone(begin, request_key='+0x0000000000000000000000000d7b0'),
    'dfix3c-key-finish-arabic-digit': clone(finish, completion_key='٠' * 2 + '0' * 30),
    # Near-misses Python refuses: missing key.
    'dfix3c-key-undo-double-underscore': clone(undo, undo_key='0000__00000000000000000000000df3', revision='abc', reason=''),
    'dfix3c-key-undo-trailing-underscore': clone(undo, undo_key='0000000000000000000000000000df3_', revision='abc', reason=''),
    'dfix3c-key-undo-0x-alone': clone(undo, undo_key='0x', revision='abc', reason=''),
    'dfix3c-key-undo-0x-spaces': clone(undo, undo_key='0x' + ' ' * 30, revision='abc', reason=''),
    'dfix3c-key-begin-0x-double-underscore': clone(begin, request_key='0x__000000000000000000000000d7b0'),
    'dfix3c-key-finish-plus-underscore': clone(finish, completion_key='+_' + '0' * 30),
    # Django forms.UUIDField strips before uuid.UUID: a padded canonical key is accepted (same-bank error),
    # a space plus 31 digits is refused even though bare uuid.UUID would accept it.
    'dfix3c-key-transfer-padded': clone(transfer, request_key=' 00000000-0000-0000-0000-000000000699 '),
    'dfix3c-key-transfer-space-31': clone(transfer, request_key=' 0000000000000000000000000000699'),
    'dfix3c-key-transfer-blank': clone(transfer, request_key='   '),
}
assert not set(new) & set(s)
for name, item in new.items():
    key = item['post'].get('undo_key', item['post'].get('request_key', item['post'].get('completion_key')))
    assert key is not None and len(key) <= 40, name
    assert len(key) == 32 or name.endswith(('-alone', '-padded', '-blank')), name
s.update(new)
p.write_text(json.dumps(s, indent=2, ensure_ascii=False) + '\n')
print(f'{len(new)} added, {len(s)} total fixtures')
