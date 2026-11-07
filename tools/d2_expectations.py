"""Author first-run proposal expectations from the scenario's fixed chart, not from either dump."""
from pathlib import Path
import json

p=Path('crates/fb-harness/scenarios/d2-first-run.json');scenario=json.loads(p.read_text())
roles={'receivables':'ar','payables':'ap','undeposited':'uf','collected_tax':'tax','banks':['bank']}
empty_reasons={role:f'No {label} account is in the chart yet.' for role,label in [('receivables','Accounts receivable'),('payables','Accounts payable'),('undeposited','undeposited funds'),('collected_tax','sales tax'),('banks','bank')]}
for step in scenario['steps']:
    if step['op']!='l2_first_run_preview':continue
    name=step['result'];empty=name=='empty';ambiguous=name in ['ambiguous','done'];filled=name in ['filled','ambiguous','done']
    chosen={} if empty else dict(roles)
    if ambiguous:chosen.pop('receivables')
    reasons=empty_reasons if empty else {'receivables':'More than one Accounts receivable account is in the chart. Pick one in Accounting setup.'} if ambiguous else {}
    step['expect_result']=dict(empty=empty,revision=2 if name=='done' else 1 if filled else 0,chosen=chosen,reasons=reasons,
        checklist=dict(chart=not empty,special=filled,tax=False,customer=False,vendor=False,item=False))
p.write_text(json.dumps(scenario,indent=2)+'\n')

p=Path('crates/fb-harness/src/lib.rs');t=p.read_text();old='''    let captured = scenario["capture_steps"] == true;''';assert old in t
t=t.replace(old,'''    for step in steps.iter().filter(|s|s["op"]=="l2_first_run_preview" && s["expect_error"]!=true) {
        let key=step["result"].as_str().unwrap_or("");
        let actual=&side.get("final").unwrap_or(side)["l2a"]["results"][key];
        if step.get("expect_result").is_none() || actual!=&step["expect_result"] {
            failures.push(format!("first-run result {key}: differs from the scenario's authored expectation"));
        }
    }
    let captured = scenario["capture_steps"] == true;''');p.write_text(t)
