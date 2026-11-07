"""Generate the L4 `reports-*` harness scenarios from existing module scenarios plus a financial_reports spec.

    python3 crates/fb-harness/py/gen_reports_scenarios.py

- reports-combined.json: the 108-checkpoint layer-3 combined company; every checkpoint runs every report on
  both bases (fiscal-year boundaries, empty periods, single days, from-the-beginning ranges).
- reports-<module>-random.json: each module's 500-step randomized run, reports on the final books. Since
  MAIN-SCENARIOS the module corpora pin every step (success, or kind + message) and the prepayment corpus is
  generated from successful lifecycles only, so they are copied unchanged; this script refuses an unpinned step.
- reports-forms.json: the report controls (presets on boundary days, malformed input, bounds).
"""
import json
import os
import re

HERE = os.path.dirname(os.path.abspath(__file__))
SCENARIOS = os.path.join(HERE, '..', 'scenarios')

PL = [[None, '2026-10-31'], ['2026-01-01', '2026-10-31'], ['2026-10-01', '2026-10-09'], ['2026-10-10', '2026-10-21'],
      ['2026-10-22', '2026-11-30'], ['2026-10-15', '2026-10-15'], ['2025-01-01', '2025-12-31'], ['2027-01-01', '2027-12-31']]
BS = ['1900-01-02', '2025-12-31', '2026-01-01', '2026-10-09', '2026-10-21', '2026-10-31', '2026-12-31', '2027-01-01']
# The first ledger is the comparer's recognition evidence: unfiltered, from the beginning, past every other report.
GL = [[None, '2099-12-31', '', None], [None, '2026-12-31', '', None], ['2026-10-10', '2026-10-21', '', None],
      ['2026-10-01', '2026-10-31', 'pl', None], ['2027-01-01', '2027-01-31', 'le', None]]
RANDOM_PL = [[None, '2099-12-31'], ['2026-01-01', '2026-12-31'], ['2026-09-01', '2026-09-30'], ['2026-10-01', '2026-10-10'],
             ['2026-10-11', '2026-10-20'], ['2026-10-21', '2026-10-31'], ['2026-11-01', '2026-12-31'], ['2025-01-01', '2025-12-31']]
RANDOM_BS = ['2025-12-31', '2026-09-30', '2026-10-10', '2026-10-20', '2026-10-31', '2026-12-31', '2027-01-01', '2099-12-31']
RANDOM_GL = [[None, '2099-12-31', '', None], ['2026-10-01', '2026-10-31', '', None], ['2026-10-11', '2026-10-20', 'pl', None],
             ['2027-01-01', '2027-01-31', 'asset', None]]


def load(name):
    with open(os.path.join(SCENARIOS, name)) as handle:
        return json.load(handle)


def save(name, scenario):
    with open(os.path.join(SCENARIOS, name), 'w') as handle:
        json.dump(scenario, handle, indent=1, ensure_ascii=False)
        handle.write('\n')


def date_span(scenario):
    found = sorted({v for step in scenario['steps'] for v in step.values() if isinstance(v, str) and re.fullmatch(r'\d{4}-\d\d-\d\d', v)})
    return (found[0], found[-1]) if found else (None, None)


def require_pins(name, scenario):
    """Main's rule: every step pins success, or a refusal with its kind and exact message."""
    for index, step in enumerate(scenario['steps']):
        pinned = step.get('expect_error') is False and 'expect_kind' not in step or (
            step.get('expect_error') is True and step.get('expect_kind') and step.get('expect_message'))
        if not pinned:
            raise SystemExit(f'{name} step {index} is not pinned; fix the module corpus first')


def combined():
    scenario = load('int-l3-combined.json')
    require_pins('int-l3-combined.json', scenario)
    gl = GL + [['2026-10-01', '2026-10-31', '', 'income'], ['2026-10-01', '2026-10-31', 'receivable', 'ar']]
    scenario['financial_reports'] = dict(profit_loss=PL, balance_sheet=BS, general_ledger=gl)
    save('reports-combined.json', scenario)
    print('reports-combined.json', len(scenario['steps']), date_span(scenario))


def randoms():
    for module in ('sales', 'credits', 'payables', 'prepayments', 'checks', 'banking'):
        scenario = load(f'{module}-random-500.json')
        require_pins(f'{module}-random-500.json', scenario)
        scenario['financial_reports'] = dict(profit_loss=RANDOM_PL, balance_sheet=RANDOM_BS, general_ledger=RANDOM_GL)
        save(f'reports-{module}-random.json', scenario)
        print(f'reports-{module}-random.json', len(scenario['steps']), date_span(scenario))


def forms():
    t = '2026-10-29'
    cases = [
        ('range', {}, t), ('range', {'preset': 'last_month'}, t), ('range', {'preset': 'this_quarter'}, t), ('range', {'preset': 'this_year'}, t),
        ('range', {'preset': 'year_to_date'}, t), ('range', {'preset': 'custom', 'start': '2026-10-01', 'end': '2026-10-31'}, t),
        ('range', {'basis': 'CASH', 'preset': 'this_month'}, '2028-02-29'), ('range', {'preset': 'last_month'}, '2028-02-29'),
        ('range', {'preset': 'this_quarter'}, '2028-02-29'), ('range', {'preset': 'last_month'}, '2027-01-01'),
        ('range', {'preset': 'year_to_date'}, '2027-01-01'), ('range', {'preset': 'this_month'}, '2026-12-31'),
        ('range', {'preset': 'this_quarter'}, '2026-12-31'), ('range', {'preset': 'this_year'}, '2026-12-31'),
        ('range', {'preset': 'this_quarter'}, '2026-03-31'), ('range', {'preset': 'this_quarter'}, '2026-07-01'),
        ('range', {'start': 'notadate', 'preset': 'custom'}, t), ('range', {'preset': 'custom', 'end': '2026-13-45'}, t),
        ('range', {'preset': 'custom', 'start': '2026-12-01', 'end': '2026-01-01'}, t), ('range', {'preset': 'bogus'}, t),
        ('range', {'basis': 'ACCRUALX'}, t), ('range', {'preset': 'custom', 'end': '0001-01-01'}, t),
        ('range', {'preset': 'custom', 'start': '', 'end': ''}, t), ('range', {'start': '2026-10-01'}, t), ('range', {'end': '2026-10-31'}, t),
        ('range', {'preset': 'this_month', 'start': '2020-01-01', 'end': '2020-01-31'}, t), ('range', {'end': '10/31/2026'}, t),
        ('range', {'end': ' 2026-10-31 '}, t), ('range', {'end': 'Oct 31 2026'}, t), ('range', {'start': '1900-01-01', 'end': '2026-01-01'}, t),
        ('range', {'start': '1900-01-02', 'end': '9999-12-30'}, t), ('range', {'end': '9999-12-31'}, t),
        ('range', {'basis': 'X', 'preset': 'Y', 'start': 'bad', 'end': 'bad'}, t), ('range', {'start': '2026-10-31', 'end': '2026-10-31'}, t),
        ('range', {'start': '  ', 'end': '2026-10-31'}, t), ('range', {'basis': 'cash'}, t), ('range', {'preset': 'custom'}, t),
        ('ledger', {'group': 'nope'}, t), ('ledger', {'group': 'income', 'preset': 'custom', 'start': '0001-01-01', 'end': '2026-01-01'}, t),
        ('ledger', {'account': 'abc'}, t), ('ledger', {'group': 'receivable'}, t), ('ledger', {'group': 'le', 'preset': 'last_month'}, t),
        ('ledger', {'group': 'bogus', 'account': 'x1', 'preset': 'nope'}, t), ('ledger', {}, t),
        ('as_of', {}, t), ('as_of', {'as_of': 'garbage'}, t), ('as_of', {'as_of': '0001-01-01'}, t), ('as_of', {'as_of': '99999-01-01'}, t),
        ('as_of', {'as_of': '2026-10-31', 'basis': 'CASH'}, t), ('as_of', {'as_of': '', 'basis': ''}, t), ('as_of', {'basis': 'NONE', 'as_of': 'x'}, t),
        # Django: every field is cleaned before RangeForm.clean, which runs only when all fields are valid.
        ('ledger', {'preset': 'custom', 'group': 'bogus'}, t), ('ledger', {'start': '2026-12-01', 'end': '2026-01-01', 'account': 'bad'}, t),
        ('ledger', {'preset': 'custom', 'account': 'x12'}, t), ('ledger', {'start': '2026-12-01', 'end': '2026-01-01', 'group': 'pl'}, t),
        # Digits: Python 3.13 knows Arabic-Indic and Extended Arabic-Indic, not the Unicode 16 Garay digits.
        ('range', {'end': '\U00010d42\U00010d40\U00010d42\U00010d46-10-31'}, t), ('range', {'end': '\u0662\u0660\u0662\u0666-10-31'}, t),
        ('as_of', {'as_of': '10/31/\U00010d42\U00010d40\U00010d42\U00010d46'}, t), ('as_of', {'as_of': 'Oct 31 \u06f2\u06f0\u06f2\u06f6'}, t),
    ]
    scenario = dict(users=[dict(username='reports', is_staff=True)],
                    steps=[dict(op='create_company', ref='c', name='Report controls', currency='USD', expect_error=False)],
                    financial_reports=dict(profit_loss=[[None, '2026-10-31']], balance_sheet=['2026-10-31'], general_ledger=[[None, '2026-10-31', '', None]],
                                           forms=[dict(form=form, data=data, today=today) for form, data, today in cases]))
    save('reports-forms.json', scenario)
    print('reports-forms.json', len(cases), 'form cases')


if __name__ == '__main__':
    import sys
    for part in sys.argv[1:] or ['combined', 'randoms', 'forms']:
        globals()[part]()
