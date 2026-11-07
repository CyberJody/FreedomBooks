"""Print aggregate progress only, without replaying synthetic sensitive test payloads."""
from pathlib import Path
import json
import re
import sys
root=Path(__file__).resolve().parents[1]
out=root.parents[1]/'staging/dfix3'
for name in (sys.argv[1:] or ['gate','screens','harness','native','smoke','smoke-secfix1','package','node-tests']):
    p=out/(name+'.log')
    if not p.exists():continue
    text=p.read_text(errors='replace')
    totals=[tuple(map(int,m)) for m in re.findall(r'^test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored;',text,re.M)]
    finished=set(re.findall(r'^test (.+) \.\.\. (?:ok|FAILED)$',text,re.M))
    pending=[n for n in re.findall(r'^test (.+) has been running for over 60 seconds$',text,re.M) if n not in finished]
    print(name,json.dumps(dict(passed=sum(v[0] for v in totals),individual_passes=len(re.findall(r'^test .+ \.\.\. ok$',text,re.M)),failed=sum(v[1] for v in totals),ignored=sum(v[2] for v in totals),matches=len(re.findall(r'^MATCH\b',text,re.M)),differences=len(re.findall(r'^DIFF\b',text,re.M)),long_running=pending,failures=re.findall(r'^test ([^\n]+) \.\.\. FAILED$',text,re.M),last_lines=text.splitlines()[-3:])))
# Show only command names, PIDs and current state for our own worktree's processes.
for p in Path('/proc').iterdir():
    if not p.name.isdigit():continue
    try:
        if (p/'cwd').resolve()!=root:continue
        name=(p/'comm').read_text().strip()
        if name in ['cargo','fb-screendiff','fb-harness','rustc','bash']:
            state=(p/'status').read_text().split('State:',1)[1].splitlines()[0].strip()
            print('process',p.name,name,state)
    except (OSError,ValueError):pass
