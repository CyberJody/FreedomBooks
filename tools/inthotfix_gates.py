"""Run INT-HOTFIX gates with dedicated evidence, databases and temp files (FIX3's runner, renamed resources).

Evidence goes to staging/int-hotfix. gate.sh's screen diff writes to staging/int-hotfix/screens, which the
smokes reuse (FB_SCREENDIFF_OUT + a copy of rust/ minus *.norm.html as FB_SMOKE_FIXTURE_DIR).
Harness DBs: fbv2_core_ihf_ref / fbv2_core_ihf. Screen-diff DBs need an approved prefix (fbv2_int_dfix1_*).
"""
from pathlib import Path
import os
import subprocess
import sys

root = Path(__file__).resolve().parents[1]
source = (root / 'tools/dfix3_gates.py').read_text()
pairs = [
    ("staging/dfix3'", "staging/int-hotfix'"),
    ("/'dfix3'", "/'int-hotfix'"),
    ("'fbv2_core_df3ref'", "'fbv2_core_ihf_ref'"),
    ("'fbv2_core_df3'", "'fbv2_core_ihf'"),
    ("'fbv2_int_dfix1_df3ref'", "'fbv2_int_dfix1_ihf_ref'"),
    ("'fbv2_int_dfix1_df3screens'", "'fbv2_int_dfix1_ihf_screens'"),
    ("assert 'SCREENS MATCH' in (OUT/'screens.log').read_text()",
     "assert any((OUT/f).exists() and 'SCREENS MATCH' in (OUT/f).read_text() for f in ('gate.log','screens.log'))"),
    ("'native':['node','scripts/build-native.cjs']", "'native':['npm','run','build:native']"),
    ("shutil.copytree(OUT/'screens/rust',fixture)",
     "shutil.copytree(OUT/'screens/rust',fixture,ignore=shutil.ignore_patterns('*.norm.html'))"),
    ("'node-tests':['node','--test',", "'npm-test':['npm','test'],\n    'node-tests':['node','--test',"),
    ("('smoke','smoke-secfix1','native','node-tests')", "('smoke','smoke-secfix1','native','node-tests','npm-test')"),
    ('ROOT=Path(__file__).resolve().parents[1]', 'ROOT=Path(' + repr(str(root)) + ')'),
    # Smokes and npm test: isolated HOME/XDG (never the user's FreedomBooks folders), no inherited DISPLAY,
    # and a short TMPDIR (Chromium aborts on a long singleton socket path).
    ("env.pop('FB_FIX6_CASES',None)\n",
     "env.pop('FB_FIX6_CASES',None)\n"
     "if name in ('smoke','smoke-secfix1','npm-test'):\n"
     "    iso=OUT/'iso'\n"
     "    for sub in ('home','config','data','cache','runtime'):(iso/sub).mkdir(parents=True,exist_ok=True)\n"
     "    os.chmod(iso/'runtime',0o700)\n"
     "    (OUT/'t').mkdir(exist_ok=True)\n"
     "    env.update(TMPDIR=str(OUT/'t'),HOME=str(iso/'home'),XDG_CONFIG_HOME=str(iso/'config'),XDG_DATA_HOME=str(iso/'data'),\n"
     "        XDG_CACHE_HOME=str(iso/'cache'),XDG_RUNTIME_DIR=str(iso/'runtime'))\n"
     "    env.pop('DISPLAY',None)\n"),
]
for old, new in pairs:
    assert source.count(old) == 1, old
    source = source.replace(old, new)
runner = root / 'target' / f'inthotfix_gate_runner-{sys.argv[1]}-{os.getpid()}.py'
runner.parent.mkdir(exist_ok=True)
runner.write_text(source)
sys.exit(subprocess.run(['python3', str(runner), *sys.argv[1:]], env=os.environ).returncode)
