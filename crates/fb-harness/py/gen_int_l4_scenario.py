"""Generate int-l4-combined.json: one company story through every layer-4 module (INT-L4).

    python3 crates/fb-harness/py/gen_int_l4_scenario.py

Base: main's 108-step int-l3-combined (checks, deposits, transfers, journals, sales, payables, credits,
prepayments, banking), every step captured as a checkpoint. Appended layer-4 steps:

- attachments on the layer-3 bill (add, idempotent retry, second file, soft removal, download) and reminders;
- an AI receipt reading into a draft bill: consent/connection, parse, begin/finish read, initial form, review
  queue, human Save (DRAFT bill linked to the reading), post, and the receipt attached to the saved bill;
- backup -> verify -> restore of the populated book (attachment files authenticated by the attachment
  module's verifier on the Rust side), the occupied-target refusal, then an explicit live-only posting;
  live bank reports change by -456 cents while restored reports remain at -100 cents; the second
  backup/restore is taken from the selected restored book and preserves its original balances;
- financial reports (profit and loss, balance sheet, general ledger) on both bases at every checkpoint.

Bank-feed accept steps are added at INT-L4b. Every step pins its outcome: success, or the one intended
refusal with its kind and frozen Python's exact message (checked by py/pin_from_outputs.py after a run).
"""
import json
import os

HERE = os.path.dirname(os.path.abspath(__file__))
SCENARIOS = os.path.join(HERE, '..', 'scenarios')
PDF = '255044462d312e340a25e2e3cfd30a312030206f626a3c3c2f547970652f436174616c6f673e3e656e646f626a0a747261696c65723c3c2f526f6f742031203020523e3e0a2525454f460a'
PNG = '89504e470d0a1a0a0000000d4948445200000001000000010802000000907753de0000000c49444154789c6338b1a50200043c01f5201698a50000000049454e44ae426082'
RECEIPT_PDF = PDF  # same bytes as the bill scan: stored once (per-company dedupe), owned by two rows


def load(name):
    with open(os.path.join(SCENARIOS, name)) as handle:
        return json.load(handle)


def key(n):
    return f'00000000-0000-0000-0000-0000000f{n:04x}'


def layer4():
    c = 'c'
    ok = lambda step: dict(step, expect_error=False)  # noqa: E731
    steps = [
        ok(dict(op='attachment_add', company=c, ref='bill-scan', name='scan.pdf', hex=PDF, request_key=key(1), document='bill:source')),
        ok(dict(op='attachment_add', company=c, ref='bill-scan', name='scan.pdf', hex=PDF, request_key=key(1), document='bill:source')),
        ok(dict(op='attachment_add', company=c, ref='bill-photo', name='photo.png', hex=PNG, request_key=key(2), document='bill:source')),
        ok(dict(op='attachment_remove', company=c, attachment='bill-photo', reason='Wrong photo')),
        ok(dict(op='attachment_download', company=c, attachment='bill-scan', ref='bill-scan-download')),
        ok(dict(op='reminder_build', company=c, today='2026-11-10', ref='reminders')),
        ok(dict(op='reminder_count', company=c, today='2026-11-10', ref='reminder-count')),
        ok(dict(op='add_member', company=c, user='member')),
        ok(dict(op='l2_save', company=c, record='vendor', model='vendor', values=dict(default_expense_account='expense', default_terms='30'))),
        ok(dict(op='assistant_connect', company=c, state=dict(mode='api_key', key_last4='demo', account_email='', registration=True,
                                                              model_choices=[dict(slug='demo-model', display_name='Demo')]))),
        ok(dict(op='assistant_switch', company=c, enabled=True)),
        ok(dict(op='assistant_parse', company=c, ref='ai-fields', raw=dict(
            vendor_name='Supplier', vendor_address=[], document_number='Receipt 77', document_date='2026-10-25', due_date='',
            currency='USD', subtotal='6', tax='0', total='6',
            lines=[dict(description='Widgets', quantity='3', unit_price='2.00', amount='6.00')]))),
        ok(dict(op='assistant_begin', company=c, ref='ai-reading', request_key=key(3), actor='member',
                file=dict(source_file_id=key(4), original_name='synthetic-receipt.png', content_type='image/png', size=100, sha256='b' * 64))),
        ok(dict(op='assistant_finish', company=c, reading='ai-reading', result='ai-fields', actor='member')),
        ok(dict(op='assistant_initial', company=c, reading='ai-reading', ref='ai-initial', actor='member')),
        ok(dict(op='assistant_queue', company=c, ref='ai-queue-before', actor='member')),
        ok(dict(op='assistant_save', company=c, reading='ai-reading', ref='ai-bill', actor='member', values=dict(
            vendor_id='vendor', date='2026-10-25', due_date='2026-11-24', reference='Receipt 77', memo='', request_key=key(5),
            lines=[dict(description='Widgets', quantity='3', rate='2.00', expense_account_id='expense', ai_line=1)]))),
        ok(dict(op='assistant_queue', company=c, ref='ai-queue-after', actor='member')),
        ok(dict(op='payables_post', company=c, bill='ai-bill', expected_revision=1)),
        ok(dict(op='attachment_add', company=c, ref='ai-receipt', name='receipt.pdf', hex=RECEIPT_PDF, request_key=key(6), document='ai-bill:source')),
        ok(dict(op='backups_snapshot', ref='snapshot-1')),
        ok(dict(op='backups_verify', backup='snapshot-1')),
        ok(dict(op='backups_restore', backup='snapshot-1')),
        dict(op='backups_refuse_existing', backup='snapshot-1', expect_error=True, expect_kind='ValidationError',
             expect_message='Restore only goes into a NEW target.'),
        ok(dict(op='backups_live_post', company=c, account='bank',
                draft=dict(op='save_draft', company=c, ref='live-only', date='2026-10-31', memo='Live only after restore',
                           lines=[dict(account='expense', debit='4.56', credit='0'), dict(account='bank', debit='0', credit='4.56')]),
                post=dict(op='post_journal', company=c, journal='live-only'),
                expected=dict(kind='isolation', live_before=-100, live_after=-556, restored=-100))),
        ok(dict(op='backups_snapshot', ref='snapshot-2')),
        ok(dict(op='backups_verify', backup='snapshot-2')),
        ok(dict(op='backups_restore', backup='snapshot-2')),
    ]
    return steps


def main():
    scenario = load('int-l3-combined.json')
    reports = load('reports-combined.json')['financial_reports']
    scenario['users'] = scenario['users'] + [dict(username='member', is_staff=False)]
    scenario['steps'] = scenario['steps'] + layer4()
    scenario['as_of'] = sorted(set(scenario['as_of']) | {'2026-11-30'})
    scenario['financial_reports'] = reports
    with open(os.path.join(SCENARIOS, 'int-l4-combined.json'), 'w') as handle:
        json.dump(scenario, handle, indent=1, ensure_ascii=False)
        handle.write('\n')
    print('int-l4-combined.json', len(scenario['steps']), 'steps / checkpoints')


if __name__ == '__main__':
    main()
