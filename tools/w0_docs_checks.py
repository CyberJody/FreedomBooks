"""Run W0-DOCS checks with all outputs confined to the worktree's disk."""
from pathlib import Path
import os
import subprocess
import sys
import struct
import zlib

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / 'staging/w0-docs'
OUT.mkdir(parents=True, exist_ok=True)
BUILD = Path('/mnt/APPS/fb-job-tmp/w0-docs')
BUILD.mkdir(parents=True, exist_ok=True)
(BUILD/'target').mkdir(exist_ok=True)
(BUILD/'tmp').mkdir(exist_ok=True)
if not (ROOT/'target').exists():
    (ROOT/'target').symlink_to(BUILD/'target', target_is_directory=True)
if not (ROOT/'electron/node_modules').exists():
    (ROOT/'electron/node_modules').symlink_to(ROOT.parent/'d1-desktop/electron/node_modules', target_is_directory=True)
env = dict(os.environ)
env['CARGO_HOME'] = env.get('CARGO_HOME') or str(Path.home()/'.cargo')
env['RUSTUP_HOME'] = env.get('RUSTUP_HOME') or str(Path.home()/'.rustup')
for name in ('tmp', 'home', 'xdg-cache', 'xdg-data', 'xdg-config'):
    (OUT/name).mkdir(exist_ok=True)
env.update(TMPDIR=str(BUILD/'tmp'), CARGO_TARGET_DIR=str(ROOT/'target'),
    RUSTC_WRAPPER='', SCCACHE_DISABLE='1',
    CARGO_INCREMENTAL='0', CARGO_PROFILE_DEV_DEBUG='0', CARGO_PROFILE_TEST_DEBUG='0',
    CARGO_PROFILE_TEST_OPT_LEVEL='1', CARGO_PROFILE_TEST_DEBUG_ASSERTIONS='true', CARGO_PROFILE_TEST_OVERFLOW_CHECKS='true',
    CARGO_BUILD_JOBS='4', RUST_TEST_THREADS='4',
    npm_config_cache=str(OUT/'npm-cache'), npm_config_update_notifier='false',
    XDG_CACHE_HOME=str(OUT/'xdg-cache'), XDG_DATA_HOME=str(OUT/'xdg-data'),
    XDG_CONFIG_HOME=str(OUT/'xdg-config'),
    FB_SCREENDIFF_TEMPLATE='fbv2_int_dfix1_w0docs_ref',
    FB_SCREENDIFF_DB='fbv2_int_dfix1_w0docs_screens',
    FB_SCREENDIFF_OUT=str(OUT/'screens'), FB_HARNESS_VAR=str(OUT/'reference-var'),
    FB_SMOKE_SHOTS=str(OUT/'smoke'), FB_BACKUPS_SMOKE_DIR=str(OUT/'backups-smoke'),
    FB_GATE_BASE='d72b0a6')
node = ROOT.parent/'d1-desktop/electron/.tooling/node-v24.22.2-linux-x64/bin'
xv = ROOT.parent/'d1-desktop/electron/.tooling/xvfb/usr'
env['PATH'] = f'{node}:{xv}/bin:' + env['PATH']
env['LD_LIBRARY_PATH'] = f'{xv}/lib/x86_64-linux-gnu:{xv}/lib:' + env.get('LD_LIBRARY_PATH','')
commands = {
    'format': ['rustfmt','--edition','2024',
        'crates/fb-core/src/company_documents.rs','crates/fb-core/tests/company_documents.rs',
        'crates/fb-db/src/company_documents_upgrade_tests.rs','crates/fb-app/tests/company_documents.rs',
        'crates/fb-templates/src/company_documents_tests.rs','crates/fb-screendiff/src/normalize/company_documents.rs'],
    'check': ['cargo','check','--offline','-p','fb-app','-p','fb-node'],
    'targeted': ['cargo','test','--offline','-p','fb-core','-p','fb-app','-p','fb-db','-p','fb-templates','-p','fb-screendiff','company_document'],
    'upgrade': ['cargo','test','--offline','-p','fb-app','--test','reconnect_needed','frozen_0140_book_with_an_active_connection_opens_with_reconnect_needed'],
    'upgrade-core': ['cargo','test','--offline','-p','fb-core','--lib','populated_98100e0_upgrade_preserves_every_cell_trigger_and_schema_hash'],
    'gate': ['bash','scripts/gate.sh'],
    'screens': ['bash','crates/fb-screendiff/run_screendiff.sh'],
    'native': ['npm','run','build:native'],
    'npm': ['npm','test'],
    'smoke': ['xvfb-run','-a','-s','-screen 0 1440x1000x24','node','scripts/smoke-w0-docs.cjs'],
    'backups': ['xvfb-run','-a','-s','-screen 0 1440x1000x24','node','scripts/smoke-backups.cjs'],
}
name = sys.argv[1]
cwd = ROOT/'electron' if name in ('native','npm','smoke','backups') else ROOT
if name in ('smoke','backups'):
    env['HOME'] = str(OUT/'home')
    env['FB_SMOKE_LAUNCH_TIMEOUT_MS'] = '240000'
    (OUT/'smoke').mkdir(exist_ok=True)
    pixels=b''.join(b'\0'+b''.join(bytes((79,43,70) if x<35 else (225,210,184)) for x in range(120)) for y in range(48))
    def chunk(kind,data):
        return struct.pack('>I',len(data))+kind+data+struct.pack('>I',zlib.crc32(kind+data))
    png=b'\x89PNG\r\n\x1a\n'+chunk(b'IHDR',struct.pack('>IIBBBBB',120,48,8,2,0,0,0))+chunk(b'IDAT',zlib.compress(pixels))+chunk(b'IEND',b'')
    (OUT/'smoke/sample-logo.png').write_bytes(png)
log = OUT/(name+'.log')
print('START', name, flush=True)
with log.open('w') as output:
    result = subprocess.run(commands[name], cwd=cwd, env=env, stdout=output, stderr=subprocess.STDOUT)
print(f'{name}: exit {result.returncode}; log {log}', flush=True)
print('\n'.join(log.read_text().splitlines()[-65:]), flush=True)
sys.exit(result.returncode)
