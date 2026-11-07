"""Slice undo revision at the same boundary as Django and append its screen case."""
from pathlib import Path
import json
root=Path(__file__).resolve().parents[1]
p=root/'crates/fb-app/src/screens/d7b.rs';t=p.read_text()
old='''                expected_revision: get(p, "revision").and_then(forms::python_integer),'''
new='''                expected_revision: get(p, "revision")
                    .and_then(|raw| forms::python_integer(&py::py_truncate(raw, 12))),'''
assert t.count(old)==1;p.write_text(t.replace(old,new))
p=root/'crates/fb-screendiff/scenarios/screens-d7b.json';s=json.loads(p.read_text())
s['steps'] += [
    dict(op='save_account',company='c',expect_error=False,ref='dfix3-empty-bank',code='1071',name='Empty statement bank',detail_type='BANK'),
    dict(op='banking_begin',company='c',expect_error=False,ref='dfix3-revision-two',account='dfix3-empty-bank',values=dict(request_key='00000000-0000-0000-0000-000000df3001',statement_date='2026-10-31',ending_balance='0.00')),
    dict(op='banking_finish',company='c',expect_error=False,account='dfix3-empty-bank',reconciliation='dfix3-revision-two',expected_revision=1,completion_key='00000000-0000-0000-0000-000000df3002'),
]
p.write_text(json.dumps(s,indent=2)+'\n')
p=root/'crates/fb-screendiff/screens.json';s=json.loads(p.read_text())
s['dfix3-undo-truncated-revision'] = dict(path='/c/{company}/reconcile/{account:1071}/undo/',status=400,post=dict(reconciliation='{reconciliation:1071:2026-10-31}',revision='0000000000002',undo_key='00000000-0000-0000-0000-000000df3003',reason='Synthetic correction'))
p.write_text(json.dumps(s,indent=2)+'\n')
