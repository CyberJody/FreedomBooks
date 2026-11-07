"""Actual frozen Python accrual reports, normalized only at the export boundary."""
from datetime import date
from ledger import financial

def reports(runner):
    out = {}
    refs = {pk: ref for ref, pk in runner.refs['account'].items()}
    cents = lambda value: int(value * 100)
    def group(section):
        return dict(rows=[[refs[row['account'].pk], cents(row['amount'])] for row in section['rows']], total=cents(section['total']))
    for ref in runner.refs['company']:
        company = runner.company(ref)
        pl = financial.profit_and_loss(company, basis=financial.ACCRUAL, start=date(2026,1,1), end=date(2026,10,31))
        bs = financial.balance_sheet(company, basis=financial.ACCRUAL, as_of=date(2026,10,31))
        out[ref] = dict(basis=financial.ACCRUAL, start='2026-01-01', end='2026-10-31',
            profit_loss=dict(sections={key:group(value) for key,value in pl['sections'].items()}, **{key:cents(pl[key]) for key in ['gross_profit','net_ordinary_income','net_other_income','net_income']}),
            balance_sheet=dict(assets=group(bs['assets']), liabilities=group(bs['liabilities']), equity=group(bs['equity']), balanced=bs['balanced'], **{key:cents(bs[key]) for key in ['retained_earnings','net_income','total_equity','total_liabilities_equity']}))
    return out
