"""Run every screen at 23:30 and 00:30 New York using a user-local libfaketime.

Run after run_screendiff.sh, with its isolated reference DB/environment sourced.
FB_FAKETIME_LIBRARY names libfaketime.so.1; no host clock or app code is changed.
"""
from pathlib import Path
import hashlib
import json
import os
import subprocess

ROOT = Path(__file__).resolve().parents[1]
HERE = ROOT / 'crates/fb-screendiff'
OUT = Path(os.environ.get('FB_CLOCK_PROOF_OUT', ROOT / 'target/clock-proof'))
LIBRARY = Path(os.environ['FB_FAKETIME_LIBRARY']).resolve()
assert LIBRARY.is_file()
assert os.environ['FB_DB_NAME'].startswith('fbv2_int_') or os.environ['FB_DB_NAME'] == 'fbv2_core_pr'
scenario = HERE / 'scenarios/screens-d7b.json'
company = json.loads(scenario.read_text())['steps'][0]['name']
digests = []
for label, instant in [('2330', '2026-11-01 23:30:00'), ('0030', '2026-11-02 00:30:00')]:
    folder = OUT / label
    django = folder / 'django'
    django.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ, LD_PRELOAD=str(LIBRARY), TZ='America/New_York',
               FAKETIME='@' + instant, FAKETIME_DONT_FAKE_MONOTONIC='1')
    # Confirm the runner really sees the requested ambient clock before freezing it.
    clock_probe = folder / 'ambient.py'
    clock_probe.write_text('from datetime import datetime\nprint(datetime.now().isoformat(), flush=True)\n')
    actual = subprocess.check_output([os.environ['FB_PY'], str(clock_probe)], env=env, text=True).strip()
    assert actual.startswith(instant.replace(' ', 'T')[:16]), actual
    print('AMBIENT LOCAL CLOCK', actual, flush=True)
    commands = [
        [os.environ['FB_PY'], str(HERE / 'py/render_django.py'), company, str(django)],
        [str(ROOT / 'target/release/fb-screendiff'), str(scenario), str(django), str(folder)],
    ]
    for index, command in enumerate(commands):
        log = folder / ('django.log' if index == 0 else 'compare.log')
        with log.open('w') as stream:
            code = subprocess.run(command, cwd=ROOT, env=env, stdout=stream, stderr=subprocess.STDOUT).returncode
        assert code == 0, f'{label}: {log}'
    print((folder / 'compare.log').read_text().splitlines()[-1], flush=True)
    normalized = {}
    for path in sorted((folder / 'rust').glob('*.norm.html')):
        normalized[path.name] = hashlib.sha256(path.read_bytes()).hexdigest()
    digests.append(normalized)
assert digests[0] == digests[1], 'normalized screen evidence changed across local midnight'
(OUT / 'normalized-hashes.json').write_text(json.dumps(digests[0], indent=2) + '\n')
print(f'CLOCK PROOF PASS: 23:30 and 00:30 local; {len(digests[0])} normalized files byte-identical', flush=True)
