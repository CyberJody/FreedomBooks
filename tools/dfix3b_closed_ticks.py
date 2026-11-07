"""Cover tick membership before completed-statement retry/status/key handling."""
from pathlib import Path
from copy import deepcopy
import json
import sys
root=Path(__file__).resolve().parents[1]
if sys.argv[1]=='tests':
    p=root/'crates/fb-app/tests/d7b_flow.rs'
    t=p.read_text()
    old='''            fields: retry,
'''
    assert t.count(old)==1
    t=t.replace(old,'''            fields: retry.clone(),
''')
    old='''    assert!(matches!(result, Outcome::Redirect { .. }));
    let undo = format!'''
    new='''    assert!(matches!(result, Outcome::Redirect { .. }));
    // Even a completed retry validates membership before NotOpen/completion.
    let mut malformed=retry;
    malformed.iter_mut().find(|(k,_)|k=="shown").unwrap().1=String::new();
    let before=fb_db::test_support::snapshot(f.app.open_mut().unwrap().book.connection()).unwrap();
    let token=f.app.tokens.mint(&path);
    let result=f.app.run(Command::SubmitForm {path:path.clone(),token,fields:malformed}).unwrap();
    let Outcome::Invalid {html}=result else {panic!("completed retry bypassed tick validation")};
    assert!(html.contains("Reload the reconciliation and try again."));
    assert_eq!(before,fb_db::test_support::snapshot(f.app.open_mut().unwrap().book.connection()).unwrap());
    let undo = format!'''
    assert t.count(old)==1
    p.write_text(t.replace(old,new))
    p=root/'crates/fb-screendiff/screens.json'
    s=json.loads(p.read_text())
    base=dict(path='/c/{company}/reconcile/{account:1071}/',status=400,post=dict(action='save',reconciliation='{reconciliation:1071:2026-10-31}',revision='2',shown='',tick='{line:first:1070}',completion_key='00000000-0000-0000-0000-000000df3002'))
    for action in ['save','reconcile']:
        item=deepcopy(base);item['post']['action']=action
        s['dfix3b-closed-'+action+'-membership']=item
    item=deepcopy(base);item['post'].update(action='reconcile',completion_key='abc')
    s['dfix3b-closed-membership-before-key']=item
    item=deepcopy(base);item['post'].update(action='reconcile',revision='abc',completion_key='abc')
    s['dfix3b-closed-revision-before-membership']=item
    p.write_text(json.dumps(s,indent=2,ensure_ascii=False)+'\n')
    print(len(s),'screens')
else:
    p=root/'crates/fb-app/src/screens/d7b.rs'
    t=p.read_text()
    old='''        let rev = revision(get(p, "revision").unwrap_or("")).map_err(FbError::Validation)?;
        let hide ='''
    new='''        let rev = revision(get(p, "revision").unwrap_or("")).map_err(FbError::Validation)?;
        // Python save_ticks validates membership before raising NotOpen. Keep
        // that validation on completed retries as well as on open statements.
        if ticked.iter().any(|id| !shown.contains_key(id)) {
            return Err(FbError::Validation(vec![RELOAD.into()]).into());
        }
        let hide ='''
    assert t.count(old)==1
    p.write_text(t.replace(old,new))
