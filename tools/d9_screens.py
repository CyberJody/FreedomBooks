"""Add the D9 bank-feed screens (GETs and every form's invalid POSTs) to crates/fb-screendiff/screens.json.

    python3 tools/d9_screens.py

Idempotent: replaces every `d9-` entry. POSTs here all fail form cleaning or a service check before any
write or network call (the screen diff refuses a mutating POST). Transaction numbers are Python's pks,
which equal fb-core's book-wide ordinals in this scenario (pull order: c1=1 ... pend=10, cc1=11, u1=12).
"""
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PATH = ROOT/'crates/fb-screendiff/screens.json'
C = '/c/{company:D9 feed books}/bank-feeds/'
ACCT = '{account:D9 feed books:%s}'

SCREENS = {
    'd9-bankfeeds-list': {'path': C},
    'd9-bankfeeds-connect': {'path': C + 'connect/'},
    'd9-bankfeeds-connect-blank': {'path': C + 'connect/', 'post': {'label': '', 'setup_token': ''}, 'status': 400},
    'd9-bankfeeds-connect-limits': {'path': C + 'connect/', 'post': {'label': 'L' * 121, 'setup_token': '   '}, 'status': 400},
    'd9-bankfeeds-connect-token-long': {'path': C + 'connect/', 'post': {'label': '  Padded  ', 'setup_token': 't' * 4097}, 'status': 400},
    'd9-bankfeeds-accounts': {'path': C + '{bfconnection:Synthetic feed}/accounts/'},
    'd9-bankfeeds-accounts-forgotten': {'path': C + '{bfconnection:Old feed}/accounts/'},
    'd9-bankfeeds-accounts-invalid': {'path': C + '{bfconnection:Synthetic feed}/accounts/', 'status': 400,
                                      'post': {'map_{bfaccount:chk}': 'not-an-account', 'map_{bfaccount:cc}': ACCT % '2100', 'map_{bfaccount:unm}': ' '}},
    'd9-bankfeeds-accounts-not-mappable': {'path': C + '{bfconnection:Synthetic feed}/accounts/', 'status': 400,
                                            'post': {'map_{bfaccount:chk}': ACCT % '6000', 'map_{bfaccount:cc}': ACCT % '2100', 'map_{bfaccount:unm}': ''}},
    'd9-bankfeeds-forget': {'path': C + '{bfconnection:Synthetic feed}/forget/'},
    'd9-bankfeeds-rules': {'path': C + 'rules/'},
    'd9-bankfeeds-rules-no-action': {'path': C + 'rules/', 'post': {}, 'status': 400},
    'd9-bankfeeds-rules-bad-rule': {'path': C + 'rules/', 'post': {'move_up': 'abc'}, 'status': 400},
    'd9-bankfeeds-rule-new': {'path': C + 'rules/new/'},
    'd9-bankfeeds-rule-new-from': {'path': C + 'rules/new/?from=7'},
    'd9-bankfeeds-rule-edit': {'path': C + 'rules/{bfrule:Office supplies}/edit/'},
    'd9-bankfeeds-rule-edit-inactive': {'path': C + 'rules/{bfrule:Interest}/edit/'},
    'd9-bankfeeds-rule-blank': {'path': C + 'rules/new/', 'post': {'name': '', 'match_text': '  ', 'sign': '', 'amount_min': '', 'amount_max': '', 'account': '', 'payee_name': ''}, 'status': 400},
    'd9-bankfeeds-rule-invalid': {'path': C + 'rules/new/', 'status': 400,
                                  'post': {'name': 'N' * 121, 'match_text': 'x', 'sign': ' IN', 'amount_min': '1' * 33, 'amount_max': '5', 'account': '999999', 'payee_name': 'P' * 121, 'active': 'false'}},
    'd9-bankfeeds-rule-min-over-max': {'path': C + 'rules/new/', 'status': 400,
                                       'post': {'name': 'Range', 'match_text': 'range', 'sign': 'OUT', 'amount_min': '10.00', 'amount_max': '5.00', 'account': ACCT % '6000', 'payee_name': '', 'active': 'on'}},
    'd9-bankfeeds-rule-control-account': {'path': C + 'rules/{bfrule:Office supplies}/edit/', 'status': 400,
                                          'post': {'name': 'Office supplies', 'match_text': 'office depot', 'sign': 'OUT', 'amount_min': '', 'amount_max': '', 'account': ACCT % '1100', 'payee_name': 'Office Depot', 'active': 'on'}},
    'd9-bankfeeds-review': {'path': C + 'review/'},
    'd9-bankfeeds-review-all': {'path': C + 'review/?status=ALL'},
    'd9-bankfeeds-review-ignored': {'path': C + 'review/?status=IGNORED'},
    'd9-bankfeeds-review-accepted': {'path': C + 'review/?status=ACCEPTED'},
    'd9-bankfeeds-review-search': {'path': C + 'review/?q=office+depot'},
    'd9-bankfeeds-review-bad-filter': {'path': C + 'review/?status=BOGUS&q=' + 'q' * 121, 'status': 400},
    'd9-bankfeeds-review-empty-post': {'path': C + 'review/', 'post': {}, 'status': 400},
    'd9-bankfeeds-review-accept-pending': {'path': C + 'review/', 'status': 400,
                                           'post': {'accept_one': '10', 'kind_10': 'JOURNAL', 'account_10': ACCT % '6100', 'payee_10': '  Bank  ', 'number_10': 'EFT-10'}},
    'd9-bankfeeds-review-accept-unmapped': {'path': C + 'review/', 'status': 400,
                                            'post': {'action': 'accept', 'sel': '12', 'kind_12': 'JOURNAL', 'account_12': ACCT % '6100'}},
    # Django's loopback case: the browser reached the install as 127.0.0.1 (the desktop always is).
    'd9-assistant-page-off': {'path': '/c/{company}/ai/', 'host': '127.0.0.1:8000'},
    'd9-assistant-page-reading-on': {'path': '/c/{company:D9 feed books}/ai/', 'host': '127.0.0.1:8000'},
    # The attachments panel on a draft bill: one file, one removed (D9 owns the panel's rows).
    'd9-attachments-bill-panel': {'path': '/c/{company:D9 feed books}/bills/{bill:D9 feed books:BILL-000001}/'},
    # J-ATTACH: the manual journal page's panel (one file) and the hand-entered journal it belongs to.
    'd9-attachments-journal-panel': {'path': '/c/{company:D9 feed books}/journals/{journal:D9 feed books:J000328}/'},
    'd9-assistant-bill-read-available': {'path': '/c/{company:D9 AI books}/bills/{bill:D9 AI books:BILL-000001}/'},
    'd9-assistant-new-bill-from-receipt': {'path': '/c/{company:D9 AI books}/bills/new/'},
    'd9-backups-page': {'path': '/c/{company:D9 feed books}/backups/'},
    'd9-backups-page-unknown-created': {'path': '/c/{company:D9 feed books}/backups/?created=freedombooks-missing-20261101T120000Z.fbbackup'},
}

if __name__ == '__main__':
    screens = json.loads(PATH.read_text())
    screens = {k: v for k, v in screens.items() if not k.startswith('d9-')}
    # render_django.py fetches in file order with one logged-in session: the Backups page prints
    # Django's pending messages, so it goes first, before any earlier screen's POST leaves one queued.
    first = {k: v for k, v in SCREENS.items() if k.startswith(('d9-backups-', 'd9-assistant-'))}
    screens = {**first, **screens, **{k: v for k, v in SCREENS.items() if k not in first}}
    PATH.write_text(json.dumps(screens, indent=2, ensure_ascii=False) + '\n')
    print('D9: %d screens in screens.json (%d D9)' % (len(screens), len(SCREENS)))
