#!/usr/bin/env python3
"""Launch the actual AppImage on this host under Xvfb and a new synthetic HOME."""
import argparse
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import importlib.util

pkg = Path(__file__).resolve().parent
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('artifact', type=Path)
parser.add_argument('--extract-and-run', action='store_true', help='Use the runtime fallback on hosts without /dev/fuse')
args = parser.parse_args()
shots = pkg / '.work/appimage-smoke'
shots.mkdir(parents=True, exist_ok=True)
temp = tempfile.mkdtemp(prefix='fb-appimage-', dir=os.environ.get('TMPDIR'))
try:
    home = Path(temp)
    env = dict(os.environ, HOME=temp, GIO_USE_VFS='local', GTK_USE_PORTAL='0', XDG_CONFIG_HOME=str(home / '.config'), XDG_DATA_HOME=str(home / '.local/share'),
               XDG_CACHE_HOME=str(home / '.cache'), XDG_RUNTIME_DIR=str(home / 'runtime'))
    Path(env['XDG_RUNTIME_DIR']).mkdir(mode=0o700)
    # Do not borrow the user's desktop bus, credentials, or keyring.
    env.pop('DBUS_SESSION_BUS_ADDRESS', None)
    env.pop('SESSION_MANAGER', None)
    tools = pkg / '.work/tools/usr'
    if not shutil.which('xvfb-run'):
        env['PATH'] = str(tools / 'bin') + os.pathsep + env['PATH']
        env['LD_LIBRARY_PATH'] = str(tools / 'lib/x86_64-linux-gnu')
    if args.extract_and_run:
        env['APPIMAGE_EXTRACT_AND_RUN'] = '1'
    subprocess.run(['dbus-run-session', '--', 'xvfb-run', '-a', '-s', '-screen 0 1600x1100x24',
                    'node', str(pkg / 'smoke-installed.cjs'), str(args.artifact.resolve()), str(shots), 'APPIMAGE'],
                   env=env, check=True)
finally:
    spec = importlib.util.spec_from_file_location('cleanup_home', pkg / 'cleanup-home.py')
    cleanup = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(cleanup)
    cleanup.cleanup_home(Path(temp))
print('APPIMAGE CHECK COMPLETE: 6/6; cleanup complete or test-owned portal mount safely skipped')
