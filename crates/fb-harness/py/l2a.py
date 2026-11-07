"""L2a differential adapter. Calls the frozen app; never changes its source."""
from journal_aliases import journal_names
from decimal import Decimal
from django.db import transaction
from directory import services as directory
from directory.models import Customer, Vendor, Item
from documents import services as documents, calculations
from documents.models import DocumentIdentity, DocumentSequence, DocumentAccount, AccountingSetup
from salestax import services as tax
from salestax.models import TaxItem, SalesTaxSettings
from ledger.models import Account, Journal
MODELS = dict(customer=Customer, vendor=Vendor, item=Item)



def apply_l2a(r, s, actor):
    op = s['op']
    if not op.startswith('l2_'):
        return False
    r.l2_types = getattr(r, 'l2_types', {})
    c = r.company(s['company'])
    v = dict(s.get('values', {}))
    for k, value in list(v.items()):
        if (k.endswith('_account') or k in documents.ROLES) and value is not None:
            v[k] = Account.objects.get(pk=r.refs['account'][value])
        elif k in ('agency_id', 'item_id', 'tax_item_id', 'default_item_id') and value is not None:
            v[k] = r.l2_refs.get(value, value)
        elif k == 'member_ids':
            v[k] = [r.l2_refs.get(x, x) for x in value]
        elif k == 'banks':
            v[k] = [Account.objects.get(pk=r.refs['account'][x]) for x in value]
    if op == 'l2_post_source':
        with transaction.atomic():
            obj = documents._post_source(actor, c, r.l2_refs[s['source']], date=s['date'], memo=s['memo'], lines=r.lines(s['lines']))
        r.refs['journal'][s['ref']] = obj.pk
    elif op == 'l2_reverse_source':
        with transaction.atomic():
            obj = documents._reverse_source(actor, c, r.l2_refs[s['source']], date=s['date'], reason=s['reason'])
        r.refs['journal'][s['ref']] = obj.pk
    elif op == 'l2_reminders':
        from reminders.queries import build
        from datetime import date
        built = build(c, today=date.fromisoformat(s['today']), days=s.get('days'))
        result = dict(today=built.today.isoformat(), days=built.days, through=built.through.isoformat())
        for family in ('bills', 'invoices'):
            group = getattr(built, family)
            doc_refs = r.sales_refs['invoice'] if family == 'invoices' else {k: pk for k, pk in r.l2_refs.items() if r.l2_types.get(k) == 'bill'}
            rows = []
            for row in group.rows:
                reference = min(k for k, pk in doc_refs.items() if pk == row.document_id)
                rows.append(dict(document_id=reference, number=row.number, party=row.party, due_date=row.due_date.isoformat(),
                                 days=row.days, balance=str(row.balance), overdue=row.overdue, when=row.when))
            result[family] = dict(count=group.count, total=str(group.total), overdue_count=group.overdue_count,
                                  overdue_total=str(group.overdue_total), capped=group.capped, rows=rows)
        r.l2_results[s['result']] = result
    elif op == 'l2_save':
        obj = directory.save_record(actor, c, MODELS[s['model']], v, record_id=r.l2_refs[s['record']] if 'record' in s else None)
        if 'ref' in s:
            r.l2_refs[s['ref']] = obj.pk
            r.l2_types[s['ref']] = s['model']
    elif op == 'l2_active':
        directory.set_active(actor, c, MODELS[s['model']], r.l2_refs[s['record']], active=s['active'])
    elif op in ('l2_rate', 'l2_group'):
        obj = (tax.save_rate if op == 'l2_rate' else tax.save_group)(actor, c, **v)
        if 'ref' in s:
            r.l2_refs[s['ref']] = obj.pk
            r.l2_types[s['ref']] = 'tax'
            if op == 'l2_rate' and v.get('new_agency_name'):
                alias = 'agency-for:' + s['ref']
                r.l2_refs[alias] = obj.agency_id
                r.l2_types[alias] = 'vendor'
    elif op == 'l2_tax_active':
        tax.set_item_active(actor, c, r.l2_refs[s['record']], active=s['active'], expected_revision=s.get('expected_revision'))
    elif op == 'l2_settings':
        _, offer = tax.save_settings(actor, c, **v)
        if 'result' in s:
            r.l2_results[s['result']] = offer
    elif op == 'l2_bulk':
        preview = tax.bulk_preview(c)
        tax.finish_bulk_step(actor, c, customers=s['customers'], items=s['items'], digest='stale' if s.get('stale') else preview['digest'])
    elif op == 'l2_identity':
        with transaction.atomic():
            obj = documents.create_identity(actor, c, kind=s['kind'], request_key=s['request_key'])
        r.l2_refs[s['ref']] = obj.pk
        r.l2_types[s['ref']] = 'identity'
    elif op == 'l2_first_run_preview':
        from documents.first_run import propose, checklist
        proposal = propose(c)
        proposal.pop('digest')
        proposal.pop('rows')
        account_refs = {pk: key for key, pk in r.refs['account'].items()}
        for role, value in list(proposal['chosen'].items()):
            proposal['chosen'][role] = [account_refs[pk] for pk in value] if role == 'banks' else account_refs[value]
        proposal['checklist'] = checklist(c)
        r.l2_results[s['result']] = proposal
    elif op == 'l2_first_run_apply':
        from documents.first_run import propose, apply_proposal
        proposal = propose(c)
        apply_proposal(actor, c, digest='stale' if s.get('stale') else proposal['digest'], expected_revision=proposal['revision'])
    elif op == 'l2_first_run_dismiss':
        from documents.first_run import dismiss_first_run
        dismiss_first_run(actor, c)
    elif op == 'l2_setup':
        documents.save_setup(actor, c, **v, expected_revision=s.get('expected_revision'))
    elif op == 'l2_snapshot':
        with transaction.atomic():
            documents.snapshot_source_accounts(actor, c, r.l2_refs[s['source']], v)
    elif op == 'l2_totals':
        if s.get('resolve'):
            with transaction.atomic():
                v = tax.document_tax(c, **v)
        v.pop('tax_item_id', None)
        v['for_posting'] = s.get('for_posting', False)
        result = calculations.totals(s['lines'], **v)
        r.l2_results[s['result']] = result
    else:
        raise ValueError(op)
    return True


def stringify(v):
    if isinstance(v, Decimal):
        return str(v)
    if isinstance(v, dict):
        return {k: stringify(x) for k, x in v.items()}
    if isinstance(v, list):
        return [stringify(x) for x in v]
    return v


def dump_l2a(r):
    out = dict(directory=[], tax=[], settings={}, identities=[], sequences=[], setup={}, snapshots=[], results={}, source_journals=[])
    def ref(pk, kind):
        return min((k for k, value in r.l2_refs.items() if value == pk and getattr(r, 'l2_types', {}).get(k) == kind), default='?')
    def aref(pk):
        return next((k for k, value in r.refs['account'].items() if value == pk), '?')
    def norm(v):
        if isinstance(v, list):
            return [norm(x) for x in v]
        if isinstance(v, dict):
            return {k: ref(x, 'vendor' if k == 'agency_id' else 'tax') if k in ('id', 'agency_id') and x is not None else norm(x) for k, x in v.items()}
        return stringify(v)
    for cref, pk in r.refs['company'].items():
        c = r.company(cref)
        for name, model in MODELS.items():
            for obj in model.objects.filter(company=c):
                row = directory.snapshot(obj)
                row['id'] = ref(obj.pk, name)
                if row['id'] == '?':
                    row['id'] = 'vendor:' + obj.display_name
                row['company_id'] = cref
                row['model'] = name
                for f in ('income_account', 'expense_account', 'default_expense_account'):
                    if row.get(f) is not None:
                        row[f] = aref(row[f])
                out['directory'].append(row)
        for obj in TaxItem.objects.filter(company=c):
            row = tax.item_snapshot(obj)
            row['rate'] = str(obj.rate) if obj.rate is not None else None
            row['agency_id'] = ref(obj.agency_id, 'vendor') if obj.agency_id else None
            row['id'] = ref(obj.pk, 'tax')
            row['company'] = cref
            if 'member_ids' in row:
                row['member_ids'] = [ref(x, 'tax') for x in row['member_ids']]
            out['tax'].append(row)
        settings = tax._settings_snapshot(tax.settings_for(c))
        if settings['default_item_id']:
            settings['default_item_id'] = ref(settings['default_item_id'], 'tax')
        out['settings'][cref] = settings
        setup = documents.setup_snapshot(AccountingSetup.objects.filter(company=c).first())
        for role in documents.ROLES:
            if setup[role] is not None:
                setup[role] = aref(setup[role])
        setup['banks'] = [aref(x) for x in setup['banks']]
        out['setup'][cref] = setup
        for obj in DocumentIdentity.objects.filter(company=c).order_by('pk'):
            out['identities'].append(dict(id=ref(obj.pk, 'identity'), company_id=cref, kind=obj.kind, ordinal=obj.ordinal, number=obj.number, request_key=str(obj.request_key), currency=obj.currency))
            for a in obj.account_references.all():
                out['snapshots'].append(dict(source=ref(obj.pk, 'identity'), role=a.role, account_id=aref(a.account_id), account_code=a.account_code, account_name=a.account_name, account_kind=a.account_kind))
        out['sequences'].extend([cref, k, n] for k, n in DocumentSequence.objects.filter(company=c).order_by('kind').values_list('kind', 'last_number'))
    journal_refs = journal_names(r)
    out['source_journals'] = [[ref(j.source_id, 'identity'), journal_refs.get(j.pk, '?')] for j in Journal.objects.exclude(source_id=None).order_by('pk')]
    out['results'] = {k: norm(v) for k, v in r.l2_results.items()}
    return stringify(out)


def audit_maps(r):
    """Map only generated row keys, retaining entity types (Python PKs overlap)."""
    ids = dict(company={str(pk): ref for ref, pk in r.refs['company'].items()},
               account={str(pk): ref for ref, pk in r.refs['account'].items()})
    for kind, model, reftype in [('customer', Customer, 'customer'), ('vendor', Vendor, 'vendor'),
                                  ('item', Item, 'item'), ('taxitem', TaxItem, 'tax'),
                                  ('documentidentity', DocumentIdentity, 'identity'),
                                  ('accountingsetup', AccountingSetup, None),
                                  ('salestaxsettings', SalesTaxSettings, None)]:
        ids[kind] = {}
        for rank, obj in enumerate(model.objects.order_by('pk')):
            if reftype is None:
                mapped = ids['company'][str(obj.company_id)]
            else:
                mapped = min((ref for ref, pk in r.l2_refs.items() if pk == obj.pk and getattr(r, 'l2_types', {}).get(ref) == reftype), default=f'unreferenced-{kind}:{rank}')
            ids[kind][str(obj.pk)] = mapped
    return ids


def normalize_audit(event, ids):
    """Only the documented typed target and snapshot ID paths are normalized."""
    kind, raw = event['target'].split(':', 1)
    if kind not in ('customer', 'vendor', 'item', 'taxitem', 'documentidentity', 'accountingsetup', 'salestaxsettings'):
        return False
    def mapped(value, entity):
        return None if value is None else ids.get(entity, {}).get(str(value), f'<unknown:{entity}:{value}>')
    def field(snapshot, key, entity):
        if key in snapshot:
            snapshot[key] = mapped(snapshot[key], entity)
    def array(snapshot, key, entity):
        if key in snapshot:
            snapshot[key] = [mapped(value, entity) for value in snapshot[key]]
    event['target'] = f"{kind}:{ids[kind].get(raw, f'<unknown:{raw}>')}"
    for key in ('before', 'after'):
        snapshot = event['detail'].get(key)
        if not isinstance(snapshot, dict):
            continue
        if kind in ('customer', 'vendor', 'item'):
            field(snapshot, 'id', kind)
            field(snapshot, 'company_id', 'company')
            for f in ('income_account', 'expense_account', 'default_expense_account'):
                field(snapshot, f, 'account')
        elif kind == 'taxitem':
            field(snapshot, 'id', 'taxitem')
            field(snapshot, 'agency_id', 'vendor')
            array(snapshot, 'member_ids', 'taxitem')
        elif kind == 'salestaxsettings':
            field(snapshot, 'default_item_id', 'taxitem')
        elif kind == 'accountingsetup':
            for role in documents.ROLES:
                field(snapshot, role, 'account')
            array(snapshot, 'banks', 'account')
    if event['action'] == 'setup.filled':
        for role in ('receivables', 'payables', 'undeposited', 'collected_tax'):
            field(event['detail']['filled'], role, 'account')
        array(event['detail']['filled'], 'banks', 'account')
    if event['action'] == 'salestax.bulk_step':
        array(event['detail'], 'customers_marked_taxable', 'customer')
        array(event['detail'], 'items_marked_taxable', 'item')
    if event['action'] == 'document.accounts':
        for account in event['detail']['accounts']:
            field(account, 'id', 'account')
    return True
