"""Differential reminder query scenario calls the actual frozen Python query builder."""
from pathlib import Path
import json
def edit(name,old,new):
    p=Path(name);s=p.read_text();assert old in s,(name,old);p.write_text(s.replace(old,new))

edit('crates/fb-harness/src/l2a.rs','            "l2_save" => {','''            "l2_reminders" => {
                let mut built=fb_core::reminders::build(&mut self.book,actor,c,s(step,"today"),step["days"].as_i64())?;
                for family in ["bills","invoices"] {
                    for row in built[family]["rows"].as_array_mut().unwrap() {
                        let raw=row["document_id"].as_str().unwrap();
                        row["document_id"]=json!(self.l2_refs.iter().filter(|(_,v)|v.as_str()==raw).map(|(k,_)|k.clone()).min().expect("reminder document has a driver ref"));
                    }
                }
                self.l2_results.insert(s(step,"result").into(),built);
            }
            "l2_save" => {''')
edit('crates/fb-harness/py/l2a.py', "    elif op == 'l2_save':", '''    elif op == 'l2_reminders':
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
    elif op == 'l2_save':''')
edit('crates/fb-harness/src/lib.rs','for step in steps.iter().filter(|s|s["op"]=="l2_first_run_preview" && s["expect_error"]!=true)', 'for step in steps.iter().filter(|s|["l2_first_run_preview","l2_reminders"].iter().any(|op|s["op"]==*op) && s["expect_error"]!=true)')
edit('crates/fb-harness/src/lib.rs','first-run result {key}: differs', 'query result {key}: differs')
s=json.loads(Path('crates/fb-screendiff/scenarios/screens-d3.json').read_text());s['name']='d3-reminders'
s['steps'] += [
 dict(op='save_account',company='c',ref='ap',code='2000',name='Payables',detail_type='PAYABLE',expect_error=False),
 dict(op='l2_setup',company='c',expected_revision=1,values=dict(receivables='ar',undeposited='uf',payables='ap',collected_tax='tax',banks=['bank']),expect_error=False),
]
for i,due in enumerate(['2026-10-30','2026-11-01','2026-11-11','2026-11-12']):
    s['steps'] += [dict(op='payables_save',company='c',ref='bill'+str(i),values=dict(vendor_id='vendor',date='2026-10-01',due_date=due,request_key=f'00000000-0000-0000-0000-{230+i:012x}',lines=[dict(description='Supplies',quantity='1',rate='30.50',expense_account_id='expense')]),expect_error=False),dict(op='payables_post',company='c',bill='bill'+str(i),expected_revision=1,expect_error=False)]
empty=dict(count=0,total='0.00',overdue_count=0,overdue_total='0.00',capped=False,rows=[])
bills=dict(count=3,total='91.50',overdue_count=1,overdue_total='30.50',capped=False,rows=[dict(document_id='bill'+str(i),number=f'BILL-{i+1:06}',party='Pond Supply',due_date=due,days=days,balance='30.50',overdue=days<0,when=when) for i,(due,days,when) in enumerate([('2026-10-30',-2,'2 days overdue'),('2026-11-01',0,'Due today'),('2026-11-11',10,'Due in 10 days')])])
# The original invoice's due date is Oct 31; the live receipt leaves 141.25 owing.
invoices=dict(count=1,total='141.25',overdue_count=1,overdue_total='141.25',capped=False,rows=[dict(document_id='inv1',number='INV-000001',party='Maple & Birch Café',due_date='2026-10-31',days=-1,balance='141.25',overdue=True,when='1 day overdue')])
for result,company,days in [('default','c',10),('today','c',0),('other','other',10)]:
    from datetime import date,timedelta
    expected=dict(today='2026-11-01',days=days,through=str(date(2026,11,1)+timedelta(days=days)),bills=bills if company=='c' else empty,invoices=invoices if company=='c' else empty)
    if days==0:expected['bills']={**bills,'count':2,'total':'61.00','rows':bills['rows'][:2]}
    s['steps'].append(dict(op='l2_reminders',company=company,today='2026-11-01',days=days,result=result,expect_error=False,expect_result=expected))
Path('crates/fb-harness/scenarios/d3-reminders.json').write_text(json.dumps(s,indent=2,ensure_ascii=False)+'\n')
