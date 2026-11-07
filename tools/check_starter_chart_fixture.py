"""Keep the reference screen fixture aligned with the chart shipped in the app."""
import json
from pathlib import Path


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        assert key not in result, f'duplicate JSON key: {key}'
        result[key] = value
    return result


root = Path(__file__).resolve().parents[1]


def read(path):
    return json.loads((root / path).read_text(), object_pairs_hook=unique_object)


chart = read('crates/fb-core/data/standard_chart.json')
scenario = read('crates/fb-screendiff/scenarios/screens-d9.json')
rows = [s for s in scenario['steps'] if s['op'] == 'save_account' and s['company'] == 'w0standard']
assert [{key: row[key] for key in account} for row, account in zip(rows, chart, strict=True)] == chart
assert all(not s.get('expect_error', False) for s in rows)
assert not any(s['op'] == 'save_account' and s['company'] == 'w0empty' for s in scenario['steps'])
screens = read('crates/fb-screendiff/screens.json')
expected = {
    'w0-standard-chart': '/c/{company:W0 standard books}/accounts/',
    'w0-standard-chart-page-fallback': '/c/{company:W0 standard books}/accounts/?page=2',
    'w0-standard-chart-by-code': '/c/{company:W0 standard books}/accounts/?sort=code',
    'w0-empty-chart': '/c/{company:W0 empty books}/accounts/',
    'w0-standard-home': '/c/{company:W0 standard books}/home/',
    'w0-empty-home': '/c/{company:W0 empty books}/home/',
}
assert all(screens[name]['path'] == path for name, path in expected.items())
print(f'starter chart fixtures: {len(chart)} accounts; {len(expected)} screen states verified')
