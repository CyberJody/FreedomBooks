#!/usr/bin/env python3
"""Prove final-artifact verification refuses missing/modified shipped notices.

Requires the explicit unpacked Windows developer fallback. Hardlinks avoid
copying the runtime; each changed notice is unlinked before mutation, so the
original build remains untouched. Actual .deb/AppImage coverage is supplied by
verify-artifacts.py on those complete final artifacts.
"""
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

pkg = Path(__file__).resolve().parent
source = pkg.parent / 'electron/out/FreedomBooks-win32-x64'
if not (source / 'INCOMPLETE-WINDOWS-NATIVE.txt').is_file():
    raise SystemExit('Build windows/prepare-shell.cjs first for the corruption-test fixture')
with tempfile.TemporaryDirectory(prefix='notice-rejection-', dir=pkg / '.work') as temp:
    tree = Path(temp) / 'shell'
    shutil.copytree(source, tree, copy_function=os.link)
    cases = [('licenses/electron/LICENSE', b'modified notice\n'),
             ('licenses/electron/LICENSES.chromium.html', None),
             ('THIRD-PARTY-NOTICES.md', None)]
    for number, (name, replacement) in enumerate(cases, start=1):
        target = tree / name
        target.unlink()
        if replacement is not None:
            target.write_bytes(replacement)
        result = subprocess.run([sys.executable, str(pkg / 'verify-artifacts.py'), '--incomplete-windows', str(tree)],
                                capture_output=True, text=True)
        assert result.returncode != 0, f'Verifier accepted corrupted notice {name}'
        assert Path(name).name in result.stderr, result.stderr
        print(f'PASS {number}: final-artifact verifier refuses altered/missing {name}', flush=True)
        target.unlink(missing_ok=True)
        os.link(source / name, target)
print('LICENSE REJECTION OK: 3 corruption checks')
