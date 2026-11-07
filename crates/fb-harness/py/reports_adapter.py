"""L4 financial reports: the frozen ledger.financial reports and the report_views data logic, normalized only at
the export boundary (accounts -> scenario refs, journals -> creation rank, source identities -> document numbers,
money -> integer cents). A scenario opts in with a top-level "financial_reports" spec:

    {"profit_loss": [[start or null, end], ...], "balance_sheet": [as_of, ...],
     "general_ledger": [[start or null, end, group, account ref or null], ...],
     "forms": [{"form": "range"|"ledger"|"as_of", "data": {...}, "today": "YYYY-MM-DD"}, ...]}

Every company of the scenario runs every report on both bases. The spec is echoed as `requests`: the comparer's
manifest of exactly which reports must exist.
"""
from datetime import date
from decimal import Decimal

from documents.models import DocumentIdentity
from ledger import financial, report_views
from ledger.models import Account, Journal

BASES = (financial.ACCRUAL, financial.CASH)


def cents(value):
    return int((Decimal(value) * 100).to_integral_value())


def day(text):
    return date.fromisoformat(text) if text else None


def period_key(start, end):
    return f"{start or ''}..{end}"


def ledger_key(start, end, group, account):
    return f"{start or ''}..{end}|{group}|{account or ''}"


def dump_reports(r):
    spec = r.scenario.get('financial_reports')
    if spec is None:
        return None
    arefs = {pk: ref for ref, pk in r.refs['account'].items()}
    aref = lambda pk: arefs.get(pk, '?')
    out = {}
    for ref, company_pk in r.refs['company'].items():
        company = r.company(ref)
        ranks = {pk: index for index, pk in enumerate(Journal.objects.filter(company=company).order_by('pk').values_list('pk', flat=True))}
        numbers = dict(DocumentIdentity.objects.filter(company=company).values_list('pk', 'number'))
        per_basis = {}
        for basis in BASES:
            pl_out, bs_out, gl_out = {}, {}, {}
            for start, end in spec.get('profit_loss', []):
                report = financial.profit_and_loss(company, basis=basis, start=day(start), end=day(end))
                pl_out[period_key(start, end)] = pl_dump(report, aref)
            for as_of in spec.get('balance_sheet', []):
                report = financial.balance_sheet(company, basis=basis, as_of=day(as_of))
                bs_out[as_of] = bs_dump(report, aref)
            for start, end, group, account in spec.get('general_ledger', []):
                chosen = None
                if account:
                    chosen = Account.objects.filter(company=company, pk=r.refs['account'].get(account)).first()
                    if chosen is None:
                        gl_out[ledger_key(start, end, group, account)] = None
                        continue
                report = financial.general_ledger(company, basis=basis, start=day(start), end=day(end), account=chosen, group=group)
                gl_out[ledger_key(start, end, group, account)] = gl_dump(report, aref, ranks, numbers)
            per_basis[basis] = dict(profit_loss=pl_out, balance_sheet=bs_out, general_ledger=gl_out)
        out[ref] = per_basis
    requests = dict(profit_loss=spec.get('profit_loss', []), balance_sheet=spec.get('balance_sheet', []),
                    general_ledger=spec.get('general_ledger', []), forms=spec.get('forms', []))
    return dict(companies=out, forms=[form_dump(case) for case in spec.get('forms', [])], requests=requests)


def rows_of(items, aref):
    return [[aref(row['account'].pk), cents(row['amount'])] for row in items]


def csv_dump(rows, aref):
    found = []
    for row in rows:
        if row['kind'] in ('heading', 'spacer'):
            continue
        account = row.get('account')
        found.append([row['kind'] if row['kind'] != 'grand' else 'total', row['section'], row.get('group', ''), account.code if account else '',
                      account.name if account else row['label'], aref(account.pk) if account else '', cents(row['amount'])])
    return found


def pl_dump(report, aref):
    sections = {key: dict(rows=rows_of(part['rows'], aref), total=cents(part['total'])) for key, part in report['sections'].items()}
    drill = dict(basis=report['basis'], preset='custom', start=report['start'], end=report['end'])
    rows = report_views.pl_rows(_Company, report, drill)
    return dict(sections=sections, gross_profit=cents(report['gross_profit']), net_ordinary_income=cents(report['net_ordinary_income']),
                net_other_income=cents(report['net_other_income']), net_income=cents(report['net_income']), unconverted=report['unconverted'],
                show_cogs=report['show_cogs'], show_other=report['show_other'], csv=csv_dump(rows, aref))


def bs_dump(report, aref):
    def section(part):
        return dict(rows=rows_of(part['rows'], aref), total=cents(part['total']),
                    groups=[dict(key=g['key'], nested=g['nested'], total=cents(g['total']),
                                 types=[dict(type=t['type'], rows=rows_of(t['rows'], aref), total=cents(t['total'])) for t in g['types']])
                            for g in part['groups']])
    rows = report_views.bs_rows(_Company, report, report['basis'], report['as_of'])
    return dict(assets=section(report['assets']), liabilities=section(report['liabilities']), equity=section(report['equity']),
                fiscal_year_start=report['fiscal_year_start'].isoformat(), retained_earnings=cents(report['retained_earnings']),
                net_income=cents(report['net_income']), total_equity=cents(report['total_equity']),
                total_liabilities_equity=cents(report['total_liabilities_equity']), balanced=report['balanced'],
                unconverted=report['unconverted'], csv=csv_dump([row for row in rows if row['kind'] != 'spacer'], aref))


def gl_dump(report, aref, ranks, numbers):
    sections = []
    for section in report['sections']:
        rows = [[row['date'].isoformat(), cents(row['debit']), cents(row['credit']),
                 f"j{ranks[row['journal_id']]}" if row['journal_id'] else None, numbers.get(row['source_id']) if row['source_id'] else None,
                 row['reversal'], row['entry_number'], row['memo'], row['name'], row['synthetic'], cents(row['balance'])] for row in section['rows']]
        sections.append(dict(account=aref(section['account'].pk), opening=cents(section['opening']), debit=cents(section['debit']),
                             credit=cents(section['credit']), closing=cents(section['closing']), rows=rows))
    return dict(sections=sections, debit=cents(report['debit']), credit=cents(report['credit']), unconverted=report['unconverted'])


class _Company:
    """Display rows build drill-down URLs from company.pk; the URL text is not compared."""
    pk = 0


def form_dump(case):
    """The report controls through the real view forms, with today() pinned."""
    forms = dict(range=report_views.RangeForm, ledger=report_views.LedgerForm, as_of=report_views.AsOfForm)
    original = report_views.today
    report_views.today = lambda: day(case['today'])
    try:
        form = forms[case['form']](case['data'])
        form.company = None
        if not form.is_valid():
            return dict(ok=False, errors=[message for messages in form.errors.values() for message in messages])
        data = form.cleaned_data
        found = dict(ok=True, basis=data['basis'])
        for key in ('preset', 'start', 'end', 'as_of', 'group'):
            if key in data:
                value = data[key]
                found[key] = value.isoformat() if hasattr(value, 'isoformat') else value
        return found
    except (OverflowError, ValueError) as exc:
        return dict(ok=False, errors=[str(exc)])
    finally:
        report_views.today = original
