"""Bank-feed differential adapter. Calls the frozen bankfeeds services; the reference tree is never modified.

The network half (setup-token claim, /accounts fetch, Account Set parsing) belongs to the simplefin plugin in
v2, so the scenario hands both sides an already parsed pull. Python receives it through the real `connect`
and `sync` with only the provider calls replaced (`client.decode_setup_token`, `claim_access_url`,
`fetch_accounts`, `parse_account_set`), or through `_store` directly (`bankfeeds_store`).

Natural keys: a feed account is `<connection ref>/<conn_key>/<remote id>`, a transaction is
`<feed ref>/<remote id>`, rules and connections use the scenario's `ref`. A pulled account or transaction
may carry its own `ref` (FIX5: ids that differ only in URL userinfo); it is then named
`<connection ref>/~/<ref>` or `<feed>/<ref>`, and the runner records which raw id each name stands for
(provenance), so both runners name rows by the raw identity and no name carries a credential. Python's integer pk and v2's
ordinal both derive `EFT-<n>` and the uuid5 accept request keys; `finalize` rewrites those to labels made
of natural keys (see README "Bank feeds").
"""
import re
from contextlib import ExitStack
from datetime import datetime, timedelta, timezone
from decimal import Decimal
from unittest import mock
from uuid import uuid5
from zoneinfo import ZoneInfo

from django.conf import settings
from django.urls import resolve

from bankfeeds import client, services as feeds
from bankfeeds.models import BankFeedAccount, BankFeedConnection, BankFeedRule, BankFeedTransaction
from ledger.models import Journal, JournalLine

MISSING_PK = 999999999
TEST_FERNET_KEY = 'hZ0w0u3j8yA8b0Pq7cX2r0jLQm6iV5n2kT4sW9eYb1E='   # synthetic, harness-only


def state(r):
    if not hasattr(r, 'bf'):
        # generated_checks / suggested: per-record provenance for finalize (see bankfeeds.rs `State`)
        r.bf = dict(connections={}, rules={}, results={}, tx_names={}, feed_names={}, docs={}, generated_checks=set(), suggested=set(),
                    feed_raw={}, tx_raw={})
        settings.FB_SECRETS_KEYS = (TEST_FERNET_KEY,)


def cents(value):
    return None if value is None else int(Decimal(value) * 100)


def epoch_of(day):
    """Noon in the display zone, so `_epoch_to_date` gives back the scenario's local date."""
    if day is None:
        return None
    y, m, d = map(int, day.split('-'))
    return int(datetime(y, m, d, 12, tzinfo=ZoneInfo(settings.DISPLAY_TIME_ZONE)).timestamp())


def instant(text):
    return datetime.fromisoformat(text.replace('Z', '+00:00'))


def parsed(pull):
    accounts = []
    for a in pull.get('accounts', []):
        accounts.append(dict(conn_key=a['conn_key'], remote_id=a['remote_id'], name=a['name'], org_name=a.get('org_name', ''),
                             currency=a['currency'], balance=None if a.get('balance') is None else Decimal(a['balance']),
                             available=None if a.get('available') is None else Decimal(a['available']), balance_epoch=a.get('balance_epoch'),
                             skipped=list(a.get('skipped', [])),
                             transactions=[dict(remote_id=t['remote_id'], posted_epoch=epoch_of(t.get('posted_on')), transacted_epoch=epoch_of(t.get('transacted_on')),
                                                amount=Decimal(t['amount']), description=t.get('description', ''), pending=t.get('pending', False))
                                           for t in a.get('transactions', [])]))
    return dict(accounts=accounts, skipped=list(pull.get('skipped', [])),
                errors=[dict(code='', message=m, conn_id='', account_id='') for m in pull.get('errors', [])])


def feed_name(conn, a):
    """A pulled account's natural key, or `<conn>/~/<ref>` when the scenario names it (see module doc)."""
    return f"{conn}/~/{a['ref']}" if a.get('ref') else f"{conn}/{a['conn_key']}/{a['remote_id']}"


def tx_name(feed, t):
    return f"{feed}/{t['ref']}" if t.get('ref') else f"{feed}/{t['remote_id']}"


def record_provenance(r, s):
    """Runner-recorded provenance (identical in bankfeeds.rs `bf_provenance`): which raw ids each name stands for."""
    for a in (s.get('pull') or {}).get('accounts') or []:
        feed = feed_name(s['connection'], a)
        r.bf['feed_raw'][feed] = (s['connection'], a['conn_key'], a['remote_id'])
        for t in a.get('transactions') or []:
            r.bf['tx_raw'][tx_name(feed, t)] = (feed, t['remote_id'])


def connection_pk(r, name):
    return r.bf['connections'].get(name, MISSING_PK)


def feed_pk(r, name):
    conn, key, remote = r.bf['feed_raw'].get(name) or name.split('/', 2)
    row = BankFeedAccount.objects.filter(connection_id=connection_pk(r, conn), conn_key=key, remote_id=remote).first()
    return row.pk if row else MISSING_PK


def tx_pk(r, name):
    feed, remote = r.bf['tx_raw'].get(name) or name.rsplit('/', 1)
    row = BankFeedTransaction.objects.filter(feed_account_id=feed_pk(r, feed), remote_id=remote).first()
    return row.pk if row else MISSING_PK


def line_pk(r, name):
    if name is None or not isinstance(name, str):
        return name
    j, p = name.rsplit(':', 1)
    row = JournalLine.objects.filter(journal_id=r.refs['journal'].get(j, MISSING_PK), position=int(p)).first()
    return row.pk if row else MISSING_PK


def resolve_values(r, v):
    out = dict(v)
    for k in ('account_id', 'transfer_account_id', 'ledger_account_id'):
        if isinstance(out.get(k), str):
            out[k] = r.refs['account'].get(out[k], MISSING_PK)
    if isinstance(out.get('vendor_id'), str):
        out['vendor_id'] = r.l2_refs.get(out['vendor_id'], MISSING_PK)
    if 'journal_line_id' in out:
        out['journal_line_id'] = line_pk(r, out['journal_line_id'])
    if isinstance(out.get('receipt_ids'), list):
        out['receipt_ids'] = [r.sales_refs['receipt'].get(x, MISSING_PK) if isinstance(x, str) else x for x in out['receipt_ids']]
    return out


def remember(r):
    """Natural keys of every row seen so far, so audit rows of a later-withdrawn pending item still map."""
    names = {pk: name for name, pk in r.bf['connections'].items()}
    feeds_by_raw = {raw: name for name, raw in r.bf['feed_raw'].items()}
    txs_by_raw = {raw: name for name, raw in r.bf['tx_raw'].items()}
    for f in BankFeedAccount.objects.all():
        conn = names.get(f.connection_id, "?")
        r.bf['feed_names'][f.pk] = feeds_by_raw.get((conn, f.conn_key, f.remote_id), f'{conn}/{f.conn_key}/{f.remote_id}')
    for t in BankFeedTransaction.objects.all():
        feed = r.bf["feed_names"][t.feed_account_id]
        r.bf['tx_names'][t.pk] = txs_by_raw.get((feed, t.remote_id), f'{feed}/{t.remote_id}')


def url_pk(url):
    match = resolve(url)
    return match.view_name, next(v for k, v in match.kwargs.items() if k != 'company_id')


def track_document(r, name, item):
    """Name the document an accept created, through the owning module's own tracking."""
    if item.journal_id is None or item.accepted_as == 'MATCH':
        return                                    # a match names an existing entry; nothing new to track
    name = name or f'bf:{r.bf["tx_names"].get(item.pk, item.pk)}:{item.return_count}'
    view, pk = url_pk(item.reference_url)
    if (view, pk) in r.bf['docs']:
        return                                    # a retry hands back the item; its document is named already
    r.bf['docs'][(view, pk)] = name
    r.l2_types = getattr(r, 'l2_types', {})
    if item.accepted_as == 'CHECK':
        from checks.models import Check
        from checks_adapter import track
        track(r, name, Check.objects.get(pk=pk), True)
    elif item.accepted_as == 'TRANSFER':
        from transfers.models import Transfer
        from checks_adapter import track
        track(r, name, Transfer.objects.get(pk=pk), False)
    elif item.accepted_as == 'DEPOSIT':
        from banking.models import Deposit
        from banking_adapter import state as banking_state, track
        banking_state(r)
        track(r, name, Deposit.objects.get(pk=pk))
    else:
        r.refs['journal'].setdefault(name, item.journal_id)


def undo(r, c, actor, s):
    """Harness op: void / reverse the document an accepted item became, through its own module."""
    from django.core.exceptions import ValidationError
    item = feeds._item(c, tx_pk(r, s['tx']))
    if item.status != 'ACCEPTED':
        raise ValidationError('Nothing to undo: this item is not accepted.')
    view, pk = url_pk(item.reference_url)
    name = r.bf['docs'].get((view, pk), '?')
    if item.accepted_as == 'CHECK':
        from checks import services as checks
        from checks.models import Check
        from checks_adapter import track
        track(r, name, checks.void_check(actor, c, pk, expected_revision=Check.objects.get(pk=pk).revision, date=s['date'], reason=s['reason']), True)
    elif item.accepted_as == 'TRANSFER':
        from transfers import services as transfers
        v = transfers.void_transfer(actor, c, pk, expected_revision=1, date=s['date'], reason=s['reason'])
        r.refs['journal'].setdefault(name + ':reversal', v.journal_id)
    elif item.accepted_as == 'DEPOSIT':
        from banking import services as deposits
        v = deposits.reverse_deposit(actor, c, pk, expected_revision=1, date=s['date'], reason=s['reason'])
        r.refs['journal'].setdefault(name + ':reversed', v.journal_id)
    else:
        from ledger import services as ledger
        v = ledger.reverse_journal(actor, c, item.journal_id, date=s['date'], reason=s['reason'])
        r.refs['journal'].setdefault(name + ':undo', v.pk)


def manifest(step):
    """The scenario's own record of what a bank-feed step names (identical in both runners): its result
    name, the connection or rule it creates, and the feed accounts / transactions a pull lists. The comparer
    proves the dumped inventories against these, so identical omissions in both dumps still fail."""
    op = step['op']
    if op == 'save_account':
        # the account a step creates (an edit names `account` instead), for query-list completeness
        return dict(company=step.get('company'), created=step.get('ref') if 'account' not in step else None)
    if not op.startswith('bankfeeds_'):
        return None
    out = dict(company=step.get('company'), result=step.get('result'))
    if op == 'bankfeeds_connect':
        out['connection'] = step.get('ref')
    if op == 'bankfeeds_save_rule' and 'rule' not in step:
        out['rule'] = step.get('ref')
    if op in ('bankfeeds_sync', 'bankfeeds_store'):
        accounts = (step.get('pull') or {}).get('accounts') or []
        out['feeds'] = [feed_name(step['connection'], a) for a in accounts]
        out['transactions'] = [tx_name(feed_name(step['connection'], a), t) for a in accounts for t in a.get('transactions') or []]
        out['pending'] = [tx_name(feed_name(step['connection'], a), t) for a in accounts for t in a.get('transactions') or []
                          if t.get('pending') is True]
        # FIX5: an aliased transaction's id as this runner's `bankfeed.synced` audit writes it (the withdrawal
        # replay cannot read it from the name); only scenarios with aliases carry the key.
        aliases = {tx_name(feed_name(step['connection'], a), t): client.sanitize(t['remote_id'], 200)
                   for a in accounts for t in a.get('transactions') or [] if t.get('ref')}
        if aliases:
            out['aliases'] = aliases
    return out


def evidence(r, c, transactions):
    """The raw rows a query list is drawn from, read straight from the tables (not through the service), so
    the comparer can recompute the list's members and order: every account of the company (ref, code, kind,
    active, postable, control role) or every transaction of the company (name, posted date, rank of its pk).
    Sorted by name so both runners record the same value (see bankfeeds.rs `bf_evidence`)."""
    if transactions:
        rows = BankFeedTransaction.objects.filter(feed_account__connection__company=c).order_by('pk')
        out = [dict(tx=r.bf['tx_names'].get(t.pk, '?'), posted_on=str(t.posted_on) if t.posted_on else None, seq=i)
               for i, t in enumerate(rows, 1)]
        return dict(transactions=sorted(out, key=lambda x: x['tx']))
    from ledger.models import Account
    controls = feeds.control_accounts(c)
    out = [dict(account=r_account(r, a.pk), code=a.code, kind=a.kind, active=a.active, postable=a.postable, control=a.pk in controls)
           for a in Account.objects.filter(company=c)]
    return dict(accounts=sorted(out, key=lambda x: x['account']))


def apply_bankfeeds(r, s, actor):
    op = s['op']
    if not op.startswith('bankfeeds_'):
        return False
    state(r)
    if 'result' in s:
        r.bf['suggested'].discard(s['result'])
    if op in ('bankfeeds_sync', 'bankfeeds_store'):
        record_provenance(r, s)
    try:
        _apply(r, s, actor, op)
    finally:
        remember(r)
    return True


def _apply(r, s, actor, op):
    c = r.company(s['company'])
    if op == 'bankfeeds_connect':
        host = s.get('host', 'bridge.example.test')
        with mock.patch.object(client, 'decode_setup_token', return_value=f'https://{host}/claim/synthetic'), \
             mock.patch.object(client, 'claim_access_url', return_value=f'https://user:synthetic@{host}/simplefin'):
            conn = feeds.connect(actor, c, setup_token='synthetic-token', label=s.get('label', feeds.DEFAULT_LABEL))
        r.bf['connections'].setdefault(s['ref'], conn.pk)
    elif op == 'bankfeeds_forget':
        feeds.forget(actor, c, connection_pk(r, s['connection']))
    elif op == 'bankfeeds_sync':
        now = instant(s['now'])
        with ExitStack() as stack:
            if 'fail' in s:
                stack.enter_context(mock.patch.object(client, 'fetch_accounts', side_effect=client.SimpleFinError(s['fail'])))
            else:
                stack.enter_context(mock.patch.object(client, 'fetch_accounts', return_value={}))
                stack.enter_context(mock.patch.object(client, 'parse_account_set', return_value=parsed(s.get('pull', {}))))
            result = feeds.sync(actor, c, connection_pk(r, s['connection']), now=now, force=s.get('force', False), scheduled=s.get('scheduled', False),
                                min_interval=timedelta(seconds=s.get('min_interval', int(feeds.SCHEDULED_MIN_INTERVAL.total_seconds()))))
        if 'result' in s:
            r.bf['results'][s['result']] = sync_result(result)
    elif op == 'bankfeeds_store':
        start = s.get('window_start')
        result = feeds._store(actor, c, connection_pk(r, s['connection']), parsed(s.get('pull', {})), instant(s['now']),
                              scheduled=s.get('scheduled', False), window_start=None if start is None else instant(start))
        if 'result' in s:
            r.bf['results'][s['result']] = sync_result(result)
    elif op == 'bankfeeds_map':
        account = s.get('account')
        feeds.map_account(actor, c, feed_pk(r, s['feed']), ledger_account_id=r.refs['account'].get(account, MISSING_PK) if isinstance(account, str) else account)
    elif op == 'bankfeeds_mappable':
        from ledger.services import access
        access(actor, c)
        r.bf['results'][s['result']] = dict(accounts=[r_account(r, a.pk) for a in feeds.mappable_accounts(c)])
        r.bf['evidence'] = evidence(r, c, False)
    elif op == 'bankfeeds_rule_accounts':
        from ledger.services import access
        access(actor, c)
        r.bf['results'][s['result']] = dict(accounts=[r_account(r, a.pk) for a in feeds.rule_accounts(c)])
        r.bf['evidence'] = evidence(r, c, False)
    elif op == 'bankfeeds_save_rule':
        v = resolve_values(r, s.get('values', {}))
        rule_id = r.bf['rules'].get(s['rule'], MISSING_PK) if 'rule' in s else None
        rule = feeds.save_rule(actor, c, rule_id=rule_id, **v)
        if 'ref' in s:
            r.bf['rules'].setdefault(s['ref'], rule.pk)
    elif op == 'bankfeeds_move_rule':
        feeds.move_rule(actor, c, r.bf['rules'].get(s['rule'], MISSING_PK), direction=s['direction'])
    elif op == 'bankfeeds_rule_active':
        feeds.set_rule_active(actor, c, r.bf['rules'].get(s['rule'], MISSING_PK), active=s['active'])
    elif op == 'bankfeeds_suggest':
        from ledger.services import access
        access(actor, c)
        item = feeds._item(c, tx_pk(r, s['tx']))
        got = feeds.suggest(c, item)
        rules = {pk: name for name, pk in r.bf['rules'].items()}
        r.bf['results'][s['result']] = dict(
            kind=got['kind'], matches=[dict(line=line_name(r, m['line']), journal=r_journal(r, m['journal'].pk), label=m['label'], date=str(m['date'])) for m in got['matches']],
            receipts=[dict(receipt=r_receipt(r, x['receipt'].pk), label=x['label'], date=str(x['date'])) for x in got['receipts']],
            rule=rules.get(got['rule'].pk, '?') if got['rule'] else None, account_id=r_account(r, got['account_id']) if got['account_id'] else None,
            payee_name=got['payee_name'], check_number=feeds.default_check_number(item))
        r.bf['suggested'].add(s['result'])
    elif op == 'bankfeeds_accept':
        v = resolve_values(r, s.get('values', {}))
        # `_text(check_number, 20) or default_check_number(item)`: a blank number is generated. The
        # revisions this step creates with that number are the generated ones, whatever happens next.
        from checks.models import CheckRevision
        number = v.get('check_number')
        generated = None
        if not (isinstance(number, str) and number.strip()) and v.get('kind') == 'CHECK':
            item = BankFeedTransaction.objects.filter(pk=tx_pk(r, s['tx']), feed_account__connection__company=c).first()
            if item is not None:
                generated = feeds.default_check_number(item)
        before = set(CheckRevision.objects.values_list('pk', flat=True)) if generated is not None else set()
        try:
            item = feeds.accept(actor, c, tx_pk(r, s['tx']), **v)
        finally:
            if generated is not None:
                for pk, doc, n in CheckRevision.objects.values_list('pk', 'document_id', 'check_number'):
                    if pk not in before and n == generated:
                        r.bf['generated_checks'].add((doc, n))
        track_document(r, s.get('ref'), item)
    elif op == 'bankfeeds_undo':
        undo(r, c, actor, s)
    elif op == 'bankfeeds_ignore':
        feeds.ignore(actor, c, tx_pk(r, s['tx']))
    elif op == 'bankfeeds_return':
        feeds.return_to_review(actor, c, tx_pk(r, s['tx']))
    elif op == 'bankfeeds_items':
        # the review screen's list: views.review's queryset and order, without its optional filters
        from ledger.services import access
        access(actor, c)
        remember(r)
        rows = BankFeedTransaction.objects.filter(feed_account__connection__company=c).order_by('-posted_on', '-pk')
        r.bf['results'][s['result']] = dict(items=[r.bf['tx_names'][t.pk] for t in rows])
        r.bf['evidence'] = evidence(r, c, True)
    else:
        raise ValueError(op)


def sync_result(v):
    out = {k: v[k] for k in ('summary', 'reasons', 'accounts', 'new', 'updated', 'posted', 'unchanged', 'skipped', 'errors', 'pending_dropped')}
    out['provider_errors'] = [e['message'] for e in v['provider_errors']]
    return out


def r_account(r, pk):
    return {v: k for k, v in r.refs['account'].items()}.get(pk, '?')


def r_journal(r, pk):
    return min((k for k, v in r.refs['journal'].items() if v == pk), default='?')


def r_receipt(r, pk):
    return min((k for k, v in getattr(r, 'sales_refs', {}).get('receipt', {}).items() if v == pk), default='?')


def line_name(r, line):
    return f'{r_journal(r, line.journal_id)}:{line.position}'


def reference_url(r, url):
    """Django URL -> the v2 route name form `<name>:<natural key>` (platform change, README)."""
    if not url:
        return ''
    view, pk = url_pk(url)
    if view == 'journal':
        return f'journal:{r_journal(r, pk)}'
    return f'{view}:{r.bf["docs"].get((view, pk), "?")}'


def dump_bankfeeds(r):
    state(r)
    remember(r)
    conn_names = {pk: name for name, pk in r.bf['connections'].items()}
    rule_names = {pk: name for name, pk in r.bf['rules'].items()}
    company = {pk: name for name, pk in r.refs['company'].items()}
    stamp = lambda d: None if d is None else int(d.timestamp())
    out = dict(connections=[], accounts=[], transactions=[], rules=[], results=r.bf['results'])
    for c in BankFeedConnection.objects.all():
        out['connections'].append(dict(id=conn_names.get(c.pk, '?'), company_id=company.get(c.company_id, '?'), label=c.label, host=c.host, active=c.active,
                                       forgotten=c.forgotten_at is not None, created_by=c.created_by.username, last_sync_at=stamp(c.last_sync_at),
                                       last_sync_ok=c.last_sync_ok, last_sync_summary=c.last_sync_summary, sync_count=c.sync_count))
    for f in BankFeedAccount.objects.all():
        out['accounts'].append(dict(id=r.bf['feed_names'][f.pk], connection_id=conn_names.get(f.connection_id, '?'), conn_key=f.conn_key, remote_id=f.remote_id,
                                    name=f.name, org_name=f.org_name, currency=f.currency, balance=cents(f.balance), available_balance=cents(f.available_balance),
                                    balance_at=stamp(f.balance_at), ledger_account_id=r_account(r, f.ledger_account_id) if f.ledger_account_id else None,
                                    mapped=f.mapped_at is not None, last_seen_at=stamp(f.last_seen_at), active=f.active))
    for t in BankFeedTransaction.objects.select_related('journal_line', 'decided_by'):
        out['transactions'].append(dict(id=r.bf['tx_names'][t.pk], feed_account_id=r.bf['feed_names'][t.feed_account_id], remote_id=t.remote_id,
                                        posted_on=str(t.posted_on) if t.posted_on else None, transacted_on=str(t.transacted_on) if t.transacted_on else None,
                                        amount=cents(t.amount), description=t.description, pending=t.pending, status=t.status, accepted_as=t.accepted_as,
                                        journal_id=r_journal(r, t.journal_id) if t.journal_id else None,
                                        journal_line_id=line_name(r, t.journal_line) if t.journal_line_id else None,
                                        reference=t.reference, reference_url=reference_url(r, t.reference_url), decided=t.decided_at is not None,
                                        decided_by=t.decided_by.username if t.decided_by_id else None, return_count=t.return_count))
    for x in BankFeedRule.objects.all():
        out['rules'].append(dict(id=rule_names.get(x.pk, '?'), company_id=company.get(x.company_id, '?'), position=x.position, name=x.name, match_text=x.match_text,
                                 sign=x.sign, amount_min=cents(x.amount_min), amount_max=cents(x.amount_max), account_id=r_account(r, x.account_id),
                                 payee_name=x.payee_name, active=x.active))
    for key in ('connections', 'accounts', 'transactions', 'rules'):
        out[key].sort(key=lambda v: v['id'])
    return out


def extend_bankfeeds_audit(r, ids):
    state(r)
    remember(r)
    ids['bankfeedconnection'] = {str(pk): name for name, pk in r.bf['connections'].items()}
    ids['bankfeedaccount'] = {str(pk): name for pk, name in r.bf['feed_names'].items()}
    ids['bankfeedtransaction'] = {str(pk): name for pk, name in r.bf['tx_names'].items()}
    ids['bankfeedrule'] = {str(pk): name for name, pk in r.bf['rules'].items()}
    ids['bf_account'] = {str(pk): name for name, pk in r.refs['account'].items()}
    ids['bf_journal'] = {str(pk): r_journal(r, pk) for pk in r.refs['journal'].values()}


def normalize_bankfeeds_audit(v, ids):
    kind, key = v['target'].split(':', 1)
    if kind not in ('bankfeedconnection', 'bankfeedaccount', 'bankfeedtransaction', 'bankfeedrule'):
        return False
    mapped = lambda x, k: None if x is None else ids[k].get(str(x), f'<unknown:{k}:{x}>')
    v['target'] = f'{kind}:{mapped(key, kind)}'
    d = v['detail']
    for field in ('before', 'after'):
        if field in d:
            d[field] = mapped(d[field], 'bf_account')
    if 'journal_id' in d:
        d['journal_id'] = mapped(d['journal_id'], 'bf_journal')
    if isinstance(d.get('previous'), dict):
        d['previous']['journal_id'] = mapped(d['previous']['journal_id'], 'bf_journal')
    return True


# --- the final pass: Python-integer-derived keys --------------------------------------------------------------

def finalize(r, out):
    """Rewrite `EFT-<pk>` and the uuid5 accept request keys to natural-key labels, in generated values only,
    by record provenance (`generated_checks`, `suggested`), never by field name or value: a check row's
    `number_key`, a check revision's `check_number` and a `check:` audit event's `check_number` when that
    document got that number from the service, and a suggestion result's `check_number`. A whole
    `request_key` value this side derived becomes its label. Names, memos, descriptions and every other
    field stay exact.

    The candidate keys are rebuilt from this side's own documents (every check revision's first line,
    vendor, payee and number; every journal line's account and name) with Python's own formula, so a key
    the service derived differently stays a raw uuid and mismatches the other side's label."""
    state(r)
    remember(r)
    from checks.models import CheckRevision
    from checks_adapter import names as check_names
    accounts = {pk: name for name, pk in r.refs['account'].items()}
    vendors = {}
    for name, pk in sorted(r.l2_refs.items()):
        if getattr(r, 'l2_types', {}).get(name) == 'vendor':
            vendors.setdefault(pk, name)
    # EFT-<pk> -> EFT#T<rank of the natural key>: short enough to stay a valid 20-character check number
    rank = {name: i for i, name in enumerate(sorted(set(r.bf['tx_names'].values())), 1)}
    eft = {str(pk): f'EFT#T{rank[name]}' for pk, name in r.bf['tx_names'].items()}
    eft_re = re.compile(r'EFT-([0-9]+)')

    def fix_eft(text):
        m = eft_re.fullmatch(text)
        return eft.get(m.group(1), text) if m else text
    generated = r.bf['generated_checks']
    check_refs = check_names(r)['check']
    generated_refs = {(check_refs[doc], n) for doc, n in generated if doc in check_refs}

    def fix_if(doc, row, field):
        if isinstance(row.get(field), str) and (doc, row[field]) in generated_refs:
            row[field] = fix_eft(row[field])
    for row in (out.get('checks') or {}).get('check') or []:
        fix_if(row.get('id'), row, 'number_key')
    for row in (out.get('checks') or {}).get('checkrevision') or []:
        fix_if(row.get('document_id'), row, 'check_number')
    for events in (out.get('audit') or {}).values():
        for e in events:
            target = e.get('target')
            if isinstance(target, str) and target.startswith('check:') and isinstance(e.get('detail'), dict):
                fix_if(target[len('check:'):], e['detail'], 'check_number')
    results = (out.get('bankfeeds') or {}).get('results') or {}
    for key in r.bf['suggested']:
        if isinstance(results.get(key), dict) and isinstance(results[key].get('check_number'), str):
            results[key]['check_number'] = fix_eft(results[key]['check_number'])
    intents = {'DEPOSIT': [('', '')], 'TRANSFER': [('', '')], 'CHECK': set(), 'JOURNAL': set()}
    for rev in CheckRevision.objects.prefetch_related('lines'):
        first = rev.lines.order_by('position').first()
        category = first.account_id if first else None
        for payee in ([rev.payee_name, ''] if rev.vendor_id else [rev.payee_name]):
            raw = f'{category}:{rev.vendor_id}:{payee}:{rev.check_number}'
            number = fix_eft(rev.check_number) if (rev.document_id, rev.check_number) in generated else rev.check_number
            label = (f'{accounts.get(category, "?") if category else "None"}:{vendors.get(rev.vendor_id, "?") if rev.vendor_id else "None"}'
                     f':{payee}:{number}')
            intents['CHECK'].add((raw, label))
    for line in JournalLine.objects.all():
        intents['JOURNAL'].add((f'{line.account_id}:{line.name}', f'{accounts.get(line.account_id, "?")}:{line.name}'))
    keys = {}
    seen_returns = {}
    for t in BankFeedTransaction.objects.all():
        seen_returns[t.pk] = t.return_count
    for pk, name in r.bf['tx_names'].items():
        for kind, options in intents.items():
            for rc in range(seen_returns.get(pk, 0) + 1):
                for raw, label in options:
                    key = str(uuid5(feeds.REQUEST_NAMESPACE, f'{kind}:{pk}:{rc}:{raw}'))
                    keys[key] = f'bankfeed:{kind}:{name}:{rc}:{label}'

    def walk(v):
        if isinstance(v, dict):
            out = {}
            for k, x in v.items():
                if k == 'request_key' and isinstance(x, str):
                    out[k] = keys.get(x, x)
                else:
                    out[k] = walk(x)
            return out
        if isinstance(v, list):
            return [walk(x) for x in v]
        return v
    return walk(out)
