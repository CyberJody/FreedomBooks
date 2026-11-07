"""Independent standard-decoder check of FIX9's final SQL TEXT and persisted rows.

Usage: python3 tools/r9_verify.py DIRECTORY [--expect-leaks]
DIRECTORY is FB_R9_FUZZ_OUT from the Rust tests. It contains vectors.json and/or persisted.json.
The before texts are positive controls only; the masked texts and stored rows must not leak.
"""
import base64
import html
import itertools
import json
import re
import sys
from collections import Counter
from pathlib import Path
from urllib.parse import unquote


def squeeze(text):
    return ''.join(c for c in text if not c.isspace() and c not in '\u200b\u200c\u200d\u2060\ufeff\u00ad')


def strings(value):
    if isinstance(value, str):
        yield value
        try:
            parsed = json.loads(value)
        except (ValueError, TypeError):
            return
        if parsed != value:
            yield from strings(parsed)
    elif isinstance(value, list):
        for item in value:
            yield from strings(item)
    elif isinstance(value, dict):
        for key, item in value.items():
            yield key
            yield from strings(item)


def readings(text, layer):
    if layer == 'raw':
        return [text]
    if layer == 'pct':
        return [unquote(text)]
    if layer in ('html', 'hex'):
        return [html.unescape(text)]
    if layer == 'json':
        result = []
        for left in range(min(len(text) + 1, 7)):
            for right in range(min(len(text) + 1, 7)):
                try:
                    result.append(json.loads('"' + text[left:len(text)-right] + '"'))
                except ValueError:
                    pass
        return result
    if layer in ('b64', 'b64url'):
        result = []
        for chunk in re.findall('[A-Za-z0-9+/_=-]+', text):
            for start in range(min(len(chunk), 4)):
                part = chunk[start:].rstrip('=')
                if len(part) % 4 == 1:
                    part = part[:-1]
                try:
                    result.append(base64.urlsafe_b64decode(part + '=' * (-len(part) % 4)).decode(errors='replace'))
                except ValueError:
                    pass
        return result
    raise ValueError(layer)


def leaks(text, secret, layers):
    want = squeeze(secret)
    texts = list(strings(text))
    if any(want in squeeze(t) for t in texts):
        return True
    # Decode each JSON leaf as well as the serialized text. The storage serializer adds one layer
    # beyond the provider's three supported layers, which must not consume its encoding budget.
    for raw in texts:
        if len(raw) < len(want):
            continue
        if not any(layer in ('b64', 'b64url') for layer in layers) and not any(c in raw for c in ('%', '&', '\\')):
            continue
        decoded = set([raw] + raw.split('***'))
        for layer in reversed(layers):
            decoded = {out for piece in decoded for out in readings(piece, layer)}
            if any(want in squeeze(t) for t in decoded):
                return True
        # Independent HTML/percent compositions also catch a missing/misreported vector layer.
        for chain in itertools.product(('html', 'pct'), repeat=3):
            value = raw
            for layer in chain:
                value = readings(value, layer)[0]
            if want in squeeze(value):
                return True
    return False


def shape(value):
    if isinstance(value, dict):
        return {k: shape(v) for k, v in value.items()}
    if isinstance(value, list):
        return [shape(v) for v in value]
    if isinstance(value, str):
        return 'string'
    return (type(value).__name__, value)


root = Path(sys.argv[1])
stats = Counter()
misses = []
path = root / 'vectors.json'
if path.exists():
    for index, vector in enumerate(json.loads(path.read_text())):
        secret, layers = vector['secret'], vector['layers']
        assert leaks(vector['before'], secret, layers), f'positive control not visible: {index}'
        assert shape(json.loads(vector['before'])) == shape(json.loads(vector['masked'])), index
        stats['decoder_confirmed_controls'] += 1
        stats['serialized_vectors'] += 1
        if leaks(vector['masked'], secret, layers):
            misses.append(('vector', index))

path = root / 'persisted.json'
if path.exists():
    expected_paths = {'accept', 'connect', 'forget', 'ignore', 'map_account', 'move_rule', 'record_sync_failure', 'return_to_review', 'save_rule', 'set_rule_active', 'stage_bank_transactions', 'store_pull'}
    expected_actions = {'bankfeed.' + suffix for suffix in ('accepted', 'connected', 'forgotten', 'ignored', 'mapped', 'returned', 'rule_created', 'rule_edited', 'rule_moved', 'staged', 'synced')}
    for index, case in enumerate(json.loads(path.read_text())):
        assert set(case['paths']) == expected_paths
        assert set(case['actions']) >= expected_actions
        stats['persisted_scenarios'] += 1
        for before, masked in case['final_texts']:
            stats['final_sql_text_checks'] += 1
            if leaks(masked, case['secret'], case['layers']) or ('join_secret' in case and leaks(masked, case['join_secret'], [])):
                misses.append(('final_sql', index))
            if leaks(before, case['secret'], case['layers']) or ('join_secret' in case and leaks(before, case['join_secret'], [])):
                stats['final_sql_positive_controls'] += 1
        for table, text in case['stored']:
            stats['stored_text_checks'] += 1
            if leaks(text, case['secret'], case['layers']) or ('join_secret' in case and leaks(text, case['join_secret'], [])):
                misses.append(('stored', index, table))
    stats['write_paths_per_scenario'] = len(expected_paths)
    stats['audit_actions_per_scenario'] = len(expected_actions)
    if 'join_secret' in case:
        assert stats['final_sql_positive_controls'] >= stats['persisted_scenarios'], 'final-only cross-field controls were not observed'

assert stats, 'no test output files found'
result = dict(stats, leaks=len(misses), first_leaks=misses[:10])
(root / 'verification.json').write_text(json.dumps(result, indent=2) + '\n')
print(json.dumps(result, sort_keys=True))
if '--expect-leaks' in sys.argv:
    assert misses, 'disabled final mask did not trip the independent decoder check'
else:
    assert not misses, 'final serialized text leaked known credentials'
