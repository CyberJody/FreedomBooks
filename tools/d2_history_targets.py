"""Complete the frozen history target inventory with reconciliation links (the screen stays D7b)."""
from pathlib import Path
import json


def replace(path,old,new):
    p=Path(path);t=p.read_text();assert old in t,(path,old);p.write_text(t.replace(old,new))


replace('crates/fb-app/src/screens/d2/audit.rs','''        let spec = match model {''','''        if model == "reconciliation" {
            let label=tx.query_row("SELECT a.code || ' · statement ' || r.statement_date FROM banking_reconciliation r JOIN ledger_account a ON a.id=r.account_id AND a.company_id=r.company_id WHERE r.id=?1 AND r.company_id=?2",params![raw,c.to_db()],|r|r.get::<_,String>(0)).optional()?;
            if let Some(label)=label{return Ok((label,Some(url("reconcile:report",&[&c.to_db(),raw]))));}
        }
        let spec = match model {''')
# The file contains two match inventories; insert the special lookup once, before the first one.
p=Path('crates/fb-app/src/screens/d2/audit.rs');t=p.read_text();marker='''        if model == "reconciliation" {''';first=t.index(marker);second=t.find(marker,first+1)
if second!=-1:
    end=t.index('        let spec = match model {',second);t=t[:second]+t[end:];p.write_text(t)
replace('crates/fb-screendiff/src/normalize.rs','''        "receipt_id" => "receipt",''','''        "receipt_id" => "receipt",
        "reconciliation_id" => "reconciliation",''')
replace('crates/fb-screendiff/py/render_django.py','''    ids['user:' + user.username] = str(user.pk)''','''    from banking.models import Reconciliation
    for reconciliation in Reconciliation.objects.filter(company=company).select_related('account'):
        ids[f'reconciliation:{reconciliation.account.code}:{reconciliation.statement_date}'] = str(reconciliation.pk)
    ids['user:' + user.username] = str(user.pk)''')
replace('crates/fb-screendiff/src/main.rs','''            ids.insert("proposal_digest:current".into(),''','''            let mut reconciliations=tx.prepare("SELECT a.code || ':' || r.statement_date,r.id FROM banking_reconciliation r JOIN ledger_account a ON a.id=r.account_id WHERE r.company_id=?1")?;
            for row in reconciliations.query_map([&company],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))? {let (key,id)=row?;ids.insert(format!("reconciliation:{key}"),id);}
            ids.insert("proposal_digest:current".into(),''')
p=Path('crates/fb-screendiff/scenarios/screens-d2.json');s=json.loads(p.read_text());s['steps'].append(dict(op='banking_begin',company='c',account='bank',ref='history-reconciliation',values={'request_key':'00000000-0000-0000-0000-0000000000de','statement_date':'2026-10-31','ending_balance':'0.00','hide_later':True},expect_error=False));p.write_text(json.dumps(s,indent=2)+'\n')
replace('crates/fb-screendiff/NORMALIZATION.md','''Company, account, invoice, journal and receipt IDs map by model.''','''Company, account, invoice, journal, receipt and reconciliation IDs map by model.''')
