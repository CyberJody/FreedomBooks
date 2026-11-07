"""Read-only progress for the long foreground gates."""
from pathlib import Path
import json
import re
ROOT=Path(__file__).resolve().parents[1]
OUT=ROOT.parents[1]/'staging/printfix1'
gate=(OUT/'gate.log').read_text()
harness=(OUT/'harness.log').read_text()
tests=re.findall(r'test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored',gate)
current=gate.rsplit('\n     Running ',1)[-1]
print(json.dumps({'gatePassedExecutionsSoFar':sum(int(row[0]) for row in tests),
                  'individualTestsPassedSoFar':len(re.findall(r'^test .* \.\.\. ok$',gate,re.M)),
                  'gateFailedExecutionsSoFar':sum(int(row[1]) for row in tests),
                  'gateTail':gate.splitlines()[-2:],
                  'longRunningUnfinished':[name for name in re.findall(r'^test (.*?) has been running for over 60 seconds$',current,re.M) if not re.search(r'^test '+re.escape(name)+r' \.\.\. (?:ok|FAILED)$',current,re.M)],
                  'harnessMatches':len(re.findall(r'^MATCH —',harness,re.M)),
                  'harnessFailures':len(re.findall(r'^FAIL',harness,re.M))},indent=2))
