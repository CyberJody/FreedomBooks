"""Differential vectors for bank-feed text handling, taken from the frozen Python entry points
(`bankfeeds.client.redact`, `client.sanitize`, `services.default_check_number`, and `rule_for`'s
`str.lower()` containment). The inputs mix credential URLs, Basic headers and CHECK numbers with the
characters where Python 3.13's `re` and `str` disagree with Rust's Unicode tables (combining marks,
joiners, connector punctuation, case-fold specials, U+001C..U+001F, newer letters).

Usage (no database is touched):  FBV2_DB=fbv2_l4f_x1 . crates/fb-harness/py/env.sh
    $FB_PY crates/fb-harness/py/gen_bankfeeds_text_vectors.py
Writes crates/fb-core/src/bankfeeds/testdata/python-{redact,check,lower}.json.
"""
import json
import random
from pathlib import Path
from types import SimpleNamespace

import django

django.setup()
from bankfeeds import client, services  # noqa: E402

rng = random.Random(20261031)
ODD = ['́', '‍', '‌', '‿', '⁀', '·', 'ͅ', 'Ᲊ', 'ᲊ', 'İ', 'ı',
       'ſ', 'K', 'Σ', 'σ', '²', '١', '٢', 'ß', 'ẞ', 'Ⅰ', 'Ⓐ',
       '／', '？', '＃', '＠', '：', '℀', '℁', '⁇', '﹕', '﹖',
       '\x00', '\x1c', '\x1d', '\x1e', '\x1f', '\x7f', '\x85', '\xa0', ' ', '　', '᠎', '​',
       '_', '-', '.', '~', '+', '/', '=', '#', '?', '@', ':', '[', ']', '%', '<', '>', '"', "'", ' ', '\t', '\n',
       'a', 'Z', '0', '9', 'x', 'k', 'K', 'S', 's', 'i', 'I', '\U0001d7ce', '\U00016ac0', '๐', '꩐',
       '̀', '⃝', '­', 'ʰ', 'ᴬ', 'ⱼ', 'և', 'ﬀ']
PIECES = ['https://', 'http://', 'HTTPS://', 'HttP://', 'httpſ://', 'u:p@', 'user:pa@ss@', 'review-user:review-pass@',
          'host', 'h.example', '[::1]', '[bad]', '[v1.x]', '[127.0.0.1]', '[fe80::1%eth0]', '[::ffff:1.2.3.4]',
          '[1:2:3:4:5:6:7:8]', '[1::2::3]', '[x', 'x]', ':443', '/path', '?q=1', '#frag', '@', 'basic ',
          'Basic ', 'BASIC\t', 'bası', 'basİc ', 'Authorization: Basic ', 'authorıᴢation=basic ', 'AUTHORIZATION =  basic\t',
          'dXNlcjpwYXNz', 'ABCDEFGHIJ', 'ABCDEFGH1J', 'abcdefgh==', 'abcdefg=', 'QUJDREVGR0g+', 'ſſſſſſſſ1',
          'KKKKKKKK1', 'CHECK', 'check', 'CHEcK', 'CHECK ', 'CHECK #', 'CHECK#', 'check  # ', '123', '00123',
          '1234567890123456789', '123456789012345678', '١٢٣', '𝟎𝟏', '<b>', '</b>', '<x y="1">', 'EFT', ' ']


def sample(n):
    parts = []
    for _ in range(n):
        parts.append(rng.choice(PIECES) if rng.random() < 0.6 else rng.choice(ODD))
    return ''.join(parts)


inputs = ['́https://review-user:review-pass@bridge.example/x', 'https://u:p@[bad]/x', 'https://u:p@h／x',
          'https://u:p@h\t/path', 'basic ABCDEFGH1J', 'https://u:p@host/x', 'xhttps://u:p@host/x',
          'https://u:p@h℀x/', 'https://[::1]@h/', 'https://u:p@[fe80::1%25eth0]/']
inputs += [sample(rng.randint(1, 9)) for _ in range(6000)]
redact = [[s, client.redact(s), client.sanitize(s, 120)] for s in inputs]

check_inputs = ['CHECK 123́', 'CHECK 123‍', 'CHECK 123‿', '́CHECK 123', 'CHECK\u001c123',
                'CHECK ١٢٣', 'CHECK 123', 'check #00123', 'CHECK 7', 'xCHECK 9', '_CHECK 9']
check_inputs += [sample(rng.randint(1, 6)) for _ in range(6000)]
check = [[s, services.default_check_number(SimpleNamespace(description=s, pk=42))] for s in check_inputs]

lower_inputs = [chr(c) for c in range(0x110000) if not 0xD800 <= c <= 0xDFFF and chr(c).lower() != chr(c)]
lower_inputs += ['Σ', 'AΣ', 'AΣB', 'ÁΣ', 'AΣ́', 'AΣ́B', 'ͅΣ',
                 'A­Σ', 'ᲉΣ', 'AΣᲉ', 'İ', 'ΑΣ ΣΑ', 'ΣΣ']
lower_inputs += [sample(rng.randint(1, 5)) + rng.choice(['Σ', '']) + sample(rng.randint(0, 3)) for _ in range(3000)]
lower = [[s, s.lower()] for s in lower_inputs]

out = Path(__file__).resolve().parents[2] / 'fb-core' / 'src' / 'bankfeeds' / 'testdata'
for name, data in [('redact', redact), ('check', check), ('lower', lower)]:
    (out / f'python-{name}.json').write_text(json.dumps(data, ensure_ascii=True))
print('redact', len(redact), 'check', len(check), 'lower', len(lower),
      'unredacted-credentials', sum('u:p@' in r[1] for r in redact))
