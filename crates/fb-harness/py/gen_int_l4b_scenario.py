"""Combined bank feeds -> checks/deposits -> reports -> backup/restore story.

Keeps all of main's integration checkpoints, with explicit live/restored balance pins.
The SelectedBook test host restores synthetic masks on each open; production wiring is wave 4.
"""
import copy
import json
from pathlib import Path

SCENARIOS = Path(__file__).resolve().parent.parent / 'scenarios'

def main():
    scenario = json.loads((SCENARIOS / 'int-l4-combined.json').read_text())
    source = copy.deepcopy(next(s for s in scenario['steps'] if s['op'] == 'sales_review_sale'))
    values = source['values']
    values.update(request_key='00000000-0000-7000-8000-00000000b401', date='2026-10-29', destination='UNDEPOSITED')
    values['lines'][0].update(rate='12.34', description='Feed deposit cash sale')
    values['lines'] = values['lines'][:1]
    source.update(review='feed-sale')
    recorded = dict(source, op='sales_record_sale', ref='feed-sale')
    steps = [source, recorded,
        dict(op='bankfeeds_sync', company='c', connection='feed-connection', now='2026-10-30T12:00:00Z', force=True,
             pull=dict(accounts=[dict(conn_key='integration', remote_id='checking', name='Integration Checking', org_name='Synthetic Bank', currency='USD',
                 transactions=[dict(remote_id='check-1', posted_on='2026-10-29', amount='-12.34', description='Feed stationery check'),
                               dict(remote_id='deposit-1', posted_on='2026-10-29', amount='12.34', description='Feed cash sale deposit')])])),
        dict(op='bankfeeds_map', company='c', feed='feed-connection/integration/checking', account='bank'),
        dict(op='bankfeeds_accept', company='c', tx='feed-connection/integration/checking/check-1', ref='feed-check',
             values=dict(kind='CHECK', account_id='expense', check_number='FEED-INT-1', payee_name='Synthetic stationery')),
        dict(op='bankfeeds_accept', company='c', tx='feed-connection/integration/checking/deposit-1', ref='feed-deposit',
             values=dict(kind='DEPOSIT', receipt_ids=['feed-sale'])),
        dict(op='bankfeeds_items', company='c', result='feed-items'),
    ]
    for step in steps:
        step['expect_error'] = False
    index = next(i for i, step in enumerate(scenario['steps']) if step['op'] == 'backups_snapshot')
    scenario['steps'][index:index] = steps
    # All main modules write with the central mask active, including the early chart/setup and
    # later sales, payables, transfers, estimates, assistant and attachment services.
    scenario['steps'].insert(1, dict(op='bankfeeds_connect', company='c', ref='feed-connection', test_mask=True, expect_error=False))
    # Equal check and deposit leave the original -100/-556 live/restore pins valid.
    scenario['strict_outcomes'] = True
    scenario['capture_steps'] = True
    (SCENARIOS / 'int-l4b-combined.json').write_text(json.dumps(scenario, indent=1) + '\n')
    print('int-l4b combined:', len(scenario['steps']), 'pinned checkpoints')

if __name__ == '__main__':
    main()
