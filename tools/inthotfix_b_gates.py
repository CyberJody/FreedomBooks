"""INT-HOTFIX part B gate runner (int-hotfix + desktop-backups), after staging/desktop-backups/gates.py.

Dedicated DBs: screen diff fbv2_int_dfix1_ihfb_ref / fbv2_int_dfix1_ihfb_screens (approved prefix),
harness fbv2_core_ihfb_ref / fbv2_core_ihfb. All output under staging/int-hotfix/partB/.
Usage: python3 tools/inthotfix_b_gates.py <gate|screens|harness|native|smoke|smoke-secfix1|npm-test|r11>
"""
from pathlib import Path
import os
import shutil
import subprocess
import sys

ROOT = Path('/mnt/Data/Projects/FreedomBooks/wt-v2/int-hotfix')
OUT = Path('/mnt/Data/Projects/FreedomBooks/staging/int-hotfix/partB')
OUT.mkdir(parents=True, exist_ok=True)
XV = ROOT.parent / 'd1-desktop/electron/.tooling/xvfb/usr'
NODE = ROOT.parent / 'd1-desktop/electron/.tooling/node-v24.22.2-linux-x64/bin'
name = sys.argv[1]
env = dict(os.environ)
env['PATH'] = f'{NODE}:{XV}/bin:' + env['PATH']
env['LD_LIBRARY_PATH'] = f'{XV}/lib/x86_64-linux-gnu:{XV}/lib:' + env.get('LD_LIBRARY_PATH', '')
# Short TMPDIR: Chromium aborts on a long singleton socket path.
tmp = OUT / 't'
tmp.mkdir(parents=True, exist_ok=True)
# Isolated HOME/XDG so no smoke can touch ~/.config/FreedomBooks or ~/.local/share/FreedomBooks.
iso = OUT / 'iso'
for sub in ('home', 'config', 'data', 'cache', 'runtime'):
    (iso / sub).mkdir(parents=True, exist_ok=True)
os.chmod(iso / 'runtime', 0o700)
env.update(
    TMPDIR=str(tmp), CARGO_BUILD_JOBS='6', RUST_TEST_THREADS='6',
    FB_SCREENDIFF_TEMPLATE='fbv2_int_dfix1_ihfb_ref', FB_SCREENDIFF_DB='fbv2_int_dfix1_ihfb_screens',
    FB_SCREENDIFF_OUT=str(OUT / 'gate-screens'),
    FB_HARNESS_VAR=str(OUT / ('reference-var-' + name)),
    FB_HARNESS_TEMPLATE='fbv2_core_ihfb_ref', FB_HARNESS_DB='fbv2_core_ihfb',
    FB_HARNESS_OUT=str(OUT / 'harness'), FB_SMOKE_SHOTS=str(OUT / ('screenshots-' + name)),
    FB_BACKUPS_SMOKE_DIR=str(OUT / ('backups-run-' + name)),
    FB_FAKETIME_LIBRARY=str(ROOT.parents[1] / 'staging/dfix2/faketime/usr-root/usr/lib/x86_64-linux-gnu/faketime/libfaketime.so.1'),
    FB_CLOCK_PROOF_OUT=str(OUT / 'clock-proof'),
)
if name in ('smoke', 'smoke-secfix1', 'npm-test'):
    env.update(HOME=str(iso / 'home'), XDG_CONFIG_HOME=str(iso / 'config'), XDG_DATA_HOME=str(iso / 'data'),
               XDG_CACHE_HOME=str(iso / 'cache'), XDG_RUNTIME_DIR=str(iso / 'runtime'))
    env.pop('DISPLAY', None)
    # The backup smoke builds the backup_proof example with cargo; keep the real rustup toolchain reachable.
    real_home = Path(os.environ['HOME'])
    env.setdefault('RUSTUP_HOME', str(real_home / '.rustup'))
    env.setdefault('CARGO_HOME', str(real_home / '.cargo'))
    env['PATH'] = f'{real_home}/.cargo/bin:' + env['PATH']
env.pop('CARGO_TARGET_DIR', None)
env.pop('FB_FIX6_CASES', None)


def r11_command():
    """review 11's leak generator (tools/dfix3_leak.py) with part B's own staging and target dirs."""
    source = (ROOT / 'tools/dfix3_leak.py').read_text()
    pairs = [
        ("root=Path(__file__).resolve().parents[1]", 'root=Path(' + repr(str(ROOT)) + ')'),
        ('staging/dfix3/leak', 'staging/int-hotfix/partB/leak'),
        ('archive.unlink()', "archive.unlink()\nshutil.copy2(root/'Cargo.lock',scratch/'Cargo.lock')"),
    ]
    for old, new in pairs:
        assert source.count(old) == 1, old
        source = source.replace(old, new)
    runner = ROOT / 'target/inthotfix_b_r11_runner.py'
    runner.parent.mkdir(exist_ok=True)
    runner.write_text(source)
    env['CARGO_TARGET_DIR'] = str(ROOT / 'target/inthotfix-b-r11')
    return ['python3', str(runner)]


xvfb = ['xvfb-run', '-a', '-s', '-screen 0 1440x1000x24 -nolisten tcp', 'node']
commands = {
    'gate': ['bash', 'scripts/gate.sh'],
    'screens': ['bash', 'crates/fb-screendiff/run_screendiff.sh'],
    'harness': ['bash', 'crates/fb-harness/run_harness.sh'],
    'smoke': xvfb + ['scripts/smoke.cjs'],
    'smoke-secfix1': xvfb + ['scripts/smoke-secfix1.cjs'],
    'native': ['npm', 'run', 'build:native'],
    'npm-test': ['npm', 'test'],
}
command = r11_command() if name == 'r11' else commands[name]
cwd = ROOT / ('electron' if name in ('smoke', 'smoke-secfix1', 'native', 'npm-test') else '.')
if name in ('smoke', 'smoke-secfix1'):
    # Reuse the gate's screen-diff output (no second Django render).
    src = OUT / 'gate-screens'
    logs = [OUT / 'gate.log', OUT / 'screens.log']
    assert any(p.exists() and 'SCREENS MATCH' in p.read_text() for p in logs), 'finish the screen gate before smoke'
    fixture = OUT / (name + '-fixtures')
    if fixture.exists():
        shutil.rmtree(fixture)
    shutil.copytree(src / 'rust', fixture, ignore=shutil.ignore_patterns('*.norm.html'))
    env['FB_SMOKE_FIXTURE_DIR'] = str(fixture)
    run_dir = OUT / ('backups-run-' + name)
    if run_dir.exists():
        shutil.rmtree(run_dir)
log = OUT / (name + '.log')
print('START', name, 'pid', os.getpid(), flush=True)
with log.open('w') as output:
    result = subprocess.run(command, cwd=cwd, env=env, stdout=output, stderr=subprocess.STDOUT)
print(f'{name}: exit {result.returncode}; log {log}', flush=True)
print('\n'.join(log.read_text().splitlines()[-20:]), flush=True)
(OUT / (name + '.exit')).write_text(str(result.returncode) + '\n')
sys.exit(result.returncode)
