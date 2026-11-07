"""Extend the existing differential adapters with calls to the frozen first-run services."""
from pathlib import Path
import json


def replace(path, old, new):
    p=Path(path);t=p.read_text();assert old in t,(path,old);p.write_text(t.replace(old,new))


replace('crates/fb-harness/src/l2a.rs','''            "l2_setup" => {''','''            "l2_first_run_preview" => {
                let mut proposal=self.book.read(|tx|fb_core::first_run::propose(tx,c))?;
                proposal.as_object_mut().unwrap().remove("digest");
                proposal.as_object_mut().unwrap().remove("rows");
                for role in ["receivables","payables","undeposited","collected_tax"] {
                    if let Some(raw)=proposal["chosen"][role].as_str() {
                        let symbolic=self.accounts.iter().find(|(_,id)|id.to_db()==raw).map(|(key,_)|key.clone()).unwrap();
                        proposal["chosen"][role]=json!(symbolic);
                    }
                }
                if let Some(banks)=proposal["chosen"]["banks"].as_array_mut() {
                    for bank in banks {let raw=bank.as_str().unwrap();*bank=json!(self.accounts.iter().find(|(_,id)|id.to_db()==raw).map(|(key,_)|key.clone()).unwrap());}
                }
                proposal["checklist"]=self.book.read(|tx|fb_core::first_run::checklist(tx,c))?;
                self.l2_results.insert(s(step,"result").into(),proposal);
            }
            "l2_first_run_apply" => {
                let proposal=self.book.read(|tx|fb_core::first_run::propose(tx,c))?;
                fb_core::first_run::apply_proposal(&mut self.book,actor,c,if step["stale"]==true{"stale"}else{proposal["digest"].as_str().unwrap()},proposal["revision"].as_i64())?;
            }
            "l2_first_run_dismiss" => { fb_core::first_run::dismiss_first_run(&mut self.book,actor,c)?; }
            "l2_setup" => {''')
replace('crates/fb-harness/py/l2a.py',"    elif op == 'l2_setup':",'''    elif op == 'l2_first_run_preview':
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
    elif op == 'l2_setup':''')
replace('crates/fb-harness/src/audit.rs','''    if event["action"] == "salestax.bulk_step" {''','''    if event["action"] == "setup.filled" {
        for role in ["receivables","payables","undeposited","collected_tax"] {field(&mut event["detail"]["filled"],role,"account",ids);}
        array(&mut event["detail"]["filled"],"banks","account",ids);
    }
    if event["action"] == "salestax.bulk_step" {''')
replace('crates/fb-harness/py/l2a.py',"    if event['action'] == 'salestax.bulk_step':",'''    if event['action'] == 'setup.filled':
        for role in ('receivables', 'payables', 'undeposited', 'collected_tax'):
            field(event['detail']['filled'], role, 'account')
        array(event['detail']['filled'], 'banks', 'account')
    if event['action'] == 'salestax.bulk_step':''')

steps=[{'op':'create_company','ref':'c','name':'D2 first run','open_first_run':True},
       {'op':'l2_first_run_preview','result':'empty','company':'c'},
       {'op':'l2_first_run_apply','company':'c','expect_error':True,'expect_kind':'ValidationError','expect_message':'Nothing on the chart is clear enough to fill. Set the accounts in Accounting setup.'}]
for ref,code,name,detail in [('bank','1000','Checking','BANK'),('ar','1100','Accounts receivable','RECEIVABLE'),('uf','1200','Undeposited funds','OTHER_CURRENT_ASSET'),('ap','2000','Accounts payable','PAYABLE'),('tax','2200','Sales and use tax payable','OTHER_CURRENT_LIABILITY'),('payroll','2300','Payroll tax payable','OTHER_CURRENT_LIABILITY'),('income','4000','Sales','INCOME'),('expense','5000','Supplies','EXPENSE')]:
    steps.append(dict(op='save_account',company='c',ref=ref,code=code,name=name,detail_type=detail))
steps.extend([
    dict(op='l2_first_run_preview',company='c',result='clear'),
    dict(op='l2_first_run_apply',company='c',stale=True,expect_error=True,expect_kind='ValidationError',expect_message='The chart or Accounting setup changed. Review the fill again before saving.'),
    dict(op='l2_first_run_apply',company='c'),
    dict(op='l2_first_run_preview',company='c',result='filled'),
    dict(op='save_account',company='c',ref='ar2',code='1101',name='Other receivables',detail_type='RECEIVABLE'),
    dict(op='l2_first_run_preview',company='c',result='ambiguous'),
    dict(op='l2_first_run_apply',company='c'),
    dict(op='l2_first_run_dismiss',company='c'),
    dict(op='l2_first_run_dismiss',company='c'),
    dict(op='l2_first_run_preview',company='c',result='done'),
])
for s in steps:s.setdefault('expect_error',False)
Path('crates/fb-harness/scenarios/d2-first-run.json').write_text(json.dumps(dict(name='d2-first-run',users=[dict(username='owner',is_staff=True)],as_of=['2026-10-31'],steps=steps),indent=2)+'\n')
