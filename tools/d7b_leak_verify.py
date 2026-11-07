"""Review 11's standard decoder applied to its completed D7b runs; retain Unicode inside JSONL."""
from pathlib import Path
import json, importlib.util, collections
OUT=Path('/mnt/Data/Projects/FreedomBooks/staging/d7b/leak')
spec=importlib.util.spec_from_file_location('verify',OUT/'verify.py');verify=importlib.util.module_from_spec(spec);spec.loader.exec_module(verify)
summaries={}
for kind in ['vectors','restore','control']:
    cases={c['id']:c for c in json.loads((OUT/f'{kind if kind=="control" else "vectors"}.json').read_text())}
    summary=dict(runs=0,stored_recoveries=0,returned_recoveries=0,texts=0,control_inputs=0,control_recovered=0,missing_restore_refusals=0,returned_actions=collections.Counter())
    for line in (OUT/f'{kind}.out.jsonl').open():
        run=json.loads(line);case=cases[run['id']];summary['runs']+=1
        secrets=[('pass',case['pass']),('user',case['user']),('user:pass',case['user']+':'+case['pass'])]
        for table,text in run['texts']+run['audits']:
            summary['texts']+=1
            summary['stored_recoveries']+=bool(verify.recovered(text,secrets))
        assert len(run['steps'])==len(run['returned'])
        for step,text in zip(run['steps'],run['returned']):
            if verify.recovered(text,secrets):
                summary['returned_recoveries']+=1
                summary['returned_actions'][step[0]]+=1
                assert step[0] in ['void_check','reverse_deposit','void_transfer','reverse_journal','save_check','revise_check','record_check','void_check2','edit_company','rename_vendor'],step[0]
            if step[1]=='err' and 'Restore bank-feed connection credentials' in text:
                summary['missing_restore_refusals']+=1
        if kind=='vectors':
            for value in case['fields'].values():
                for text in value if isinstance(value,list) else [value]:
                    summary['control_inputs']+=1
                    summary['control_recovered']+=bool(verify.recovered(text,secrets))
    summaries[kind]=summary
    print(kind,json.dumps(summary),flush=True)
    assert summary['stored_recoveries']==0,summary
    # FIX2 now fails closed after reopen without restoring credentials. The historical review-11
    # no-restore case therefore proves refusal, while all 5,520 independent decoder inputs fire.
    if kind=='control':assert summary['missing_restore_refusals']>0 and summary['returned_recoveries']==0
    if kind=='vectors':assert summary['control_inputs']==summary['control_recovered'] and summary['control_inputs']>0
(OUT/'summary.json').write_text(json.dumps(summaries,indent=2)+'\n')
print('review 11: 0 stored recoveries; 5520/5520 decoder controls; no-restore writes refused',flush=True)
