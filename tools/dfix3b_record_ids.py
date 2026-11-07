"""Exercise the record-ID branch independently of the action validator."""
from pathlib import Path
from copy import deepcopy
import json
root=Path(__file__).resolve().parents[1]
p=root/'crates/fb-screendiff/screens.json'
s=json.loads(p.read_text())
base=s['d7b-screen-invalid-shown']
for i,value in enumerate(['abc','','-1','1000000000000000001']):
    item=deepcopy(base)
    item['post'].update(action='save',reconciliation=value,revision='abc',shown='bad')
    s[f'dfix3b-screen-record-id-{i}']=item
p.write_text(json.dumps(s,indent=2,ensure_ascii=False)+'\n')
p=root/'crates/fb-app/tests/d7b_flow.rs'
t=p.read_text()
old='''    let path = begin(&mut f);
    let html = screens::render(&mut f.app, &path).unwrap().html;
    let report_path='''
new='''    let path = begin(&mut f);
    let before=fb_db::test_support::snapshot(f.app.open_mut().unwrap().book.connection()).unwrap();
    for reconciliation in ["abc","","-1","1000000000000000001"] {
        let token=f.app.tokens.mint(&path);
        let result=f.app.run(Command::SubmitForm {path:path.clone(),token,fields:fields(json!({"action":"save","reconciliation":reconciliation,"revision":"abc","shown":"bad"}))}).unwrap();
        let Outcome::Invalid {html}=result else {panic!("malformed record ID accepted")};
        assert!(html.contains("Choose Save, Leave or Reconcile now."));
        assert_eq!(before,fb_db::test_support::snapshot(f.app.open_mut().unwrap().book.connection()).unwrap());
    }
    let html = screens::render(&mut f.app, &path).unwrap().html;
    let report_path='''
assert t.count(old)==1
p.write_text(t.replace(old,new))
print(len(s),'screens')
