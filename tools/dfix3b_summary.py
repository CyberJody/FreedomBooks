"""Report bounded aggregate FIX3b gate evidence without test payloads."""
from pathlib import Path
import json
import re
root=Path(__file__).resolve().parents[1]
out=root.parents[1]/'staging/dfix3b'
summary={}
for name in ['gate','harness','native','smoke','smoke-secfix1','leak','journal-proof']:
    p=out/(name+'.log')
    if not p.exists(): continue
    t=p.read_text(errors='replace')
    counts=[tuple(map(int,m)) for m in re.findall(r'^test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored;',t,re.M)]
    summary[name]={
        'passed':sum(x[0] for x in counts),'failed':sum(x[1] for x in counts),'ignored':sum(x[2] for x in counts),'suites':len(counts),
        'matches':len(re.findall(r'^MATCH\b',t,re.M)),
        'mismatches':len(re.findall(r'^(?:MISMATCH|DIFF)\b',t,re.M)),
        'failures':re.findall(r'^test (.+) \.\.\. FAILED$',t,re.M),
        'final':re.findall(r'^(?:SCREENS (?:MATCH|DIFFER).*|gate:.*|SMOKE(?: D8)? OK:.*|R11 PASS:.*|FIX3b genuine-source.*|PASS \d+ .*|database left.*)$',t,re.M)[-4:],
    }
    print(name,json.dumps(summary[name]))
(out/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
