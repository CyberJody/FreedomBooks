"""FIX3c: record Python 3.13 `uuid.UUID(str)` results for py::py_uuid's parity test.

Each vector is [input, str(UUID(input)) or null when Python raises ValueError]. Deterministic (seeded).
Run with the reference Python: python3 tools/dfix3c_uuid_vectors.py
"""
from pathlib import Path
import json
import random
import sys
import uuid

assert sys.version_info[:2] == (3, 13), sys.version
root = Path(__file__).resolve().parents[1]
out = root / 'crates/fb-core/tests/data/py_uuid_vectors.json'


def parse(text):
    try:
        return str(uuid.UUID(text))
    except ValueError:
        return None


Z = '0' * 32
D = '00000000000000000000000000df3003'
curated = [
    # Review 2 DIFF probes (r2-undo-12/13/14, r2-begin-4, r2-finish-0).
    '0x000000000000000000000000000df3', '+0000000000000000000000000000df3', '0000_000000000000000000000000df3',
    '0x00000000000000000000000000d7b0', '0x000000000000000000000000000000',
    # Already accepted forms.
    D, '00000000-0000-0000-0000-000000df3003', '{00000000-0000-0000-0000-000000df3003}',
    'urn:uuid:00000000-0000-0000-0000-000000df3003', '6D1F3A52-0D1E-4C6B-9A57-1E5B3F0C2A10',
    # Accepted int(x, 16) grammar.
    '0X000000000000000000000000000df3', '+0x0000000000000000000000000000f', '0x_00000000000000000000000000df3',
    ' ' + D[1:], D[1:] + ' ', ' ' + D[2:] + ' ', '\t' + D[1:], D[1:] + '\n', '\x0b' + D[1:], '\x0c' + D[1:], '\r' + D[1:],
    ' ' + D[1:], '　' + D[1:], '\u0085' + D[1:], D[1:] + ' ',
    '٣' + D[1:], '３' + D[1:], '0x٣' + D[3:], '٠x' + D[2:],
    '0_0' + D[3:], '0_0_0_0_0_0_0_0_0_0_0_0_0_0_0_0d', ' +0x_' + D[6:], '--' + D + '--', '{{' + D + '}}',
    'urn:urn:' + D, 'uuid:' + D, 'u-r-n-:' + D,
    # Python refuses these near-misses.
    '0000__00000000000000000000000df3', '0000000000000000000000000000df3_', '_0000000000000000000000000000df3',
    '0x' + ' ' * 30, ' ' * 30 + '0x', '0x__000000000000000000000000df3', '+_000000000000000000000000000df3',
    '0_x00000000000000000000000000df3', '00x00000000000000000000000000df3', '+-' + D[2:], '++' + D[2:], '+ ' + D[2:],
    '\x1c' + D[1:], '\x1f' + D[1:], '\x00' + D[1:], D[1:] + '\x00', '᠎' + D[1:], '​' + D[1:],
    'Ａ' + D[1:], 'g' + D[1:], ' ' * 32, '_' * 32, '+' + ' ' * 31, '0x', '', D[1:], D + '0', ' ' + D,
    '{' + D[1:], D[:16] + ' ' + D[17:], D[:16] + '_' + D[17:] + '_', '\x7f' + D[1:], 'Z' * 32,
]
random.seed(20261103)
pieces = (list('0123456789abcdefABCDEF') * 3 + ['x', 'X', '+', '-', '_', '__', ' ', '\t', '\n', '\x0b', '\x0c', '\x1c',
          '{', '}', 'urn:', 'uuid:', '٣', '３', '٠', ' ', '　', ' ', 'Ａ', 'g', '\x00'])
mutations = ['0x', '0X', '+', '+0x', ' ', '  ', '\t', '_', '__', '٣', ' ', '-', '{', '}', 'x', '0x_']
generated = []
for _ in range(3000):
    base = list(format(random.getrandbits(128), '032x'))
    for _ in range(random.randint(1, 4)):
        piece = random.choice(mutations)
        at = random.randrange(len(base) + 1)
        width = random.choice([0, len(piece)])
        base[at:at + width] = list(piece)
    generated.append(''.join(base))
for _ in range(2000):  # same-length replacements, so most reach int(x, 16)
    base = list(format(random.getrandbits(128), '032x'))
    for _ in range(random.randint(1, 3)):
        piece = random.choice(mutations[:11])
        at = random.randrange(len(base) - len(piece) + 1)
        base[at:at + len(piece)] = list(piece)
    generated.append(''.join(base))
for _ in range(1000):
    generated.append(''.join(random.choice(pieces) for _ in range(random.randint(28, 38))))
seen, vectors = set(), []
for text in curated + generated:
    if text not in seen:
        seen.add(text)
        vectors.append([text, parse(text)])
out.write_text(json.dumps(vectors, ensure_ascii=True, indent=0) + '\n')
accepted = sum(1 for _, v in vectors if v is not None)
print(f'{len(vectors)} vectors, {accepted} accepted, {len(vectors) - accepted} refused -> {out.relative_to(root)}')
