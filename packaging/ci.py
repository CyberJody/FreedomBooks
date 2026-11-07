#!/usr/bin/env python3
"""Portable native + installer + final-artifact CI driver; never publishes."""
import argparse
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import sys

pkg = Path(__file__).resolve().parent
root = pkg.parent
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('platform', choices=['linux', 'win32'])
args = parser.parse_args()
tag = os.environ.get('RELEASE_TAG', '')
if not tag.startswith('v'):
    raise SystemExit('CI builds require a v* release tag')
if not (root / 'LICENSE').is_file():
    raise SystemExit('Release blocked: add the approved project LICENSE at publication')
env = dict(os.environ, FB_RELEASE_VERSION=tag[1:], CSC_IDENTITY_AUTO_DISCOVERY='false')
def run(command):
    subprocess.run(command, cwd=root, env=env, check=True)
run([sys.executable, str(pkg / 'build-native.py'), args.platform])
run(['node', str(pkg / 'installers.cjs'), args.platform])
patterns = ['*.AppImage', '*.deb'] if args.platform == 'linux' else ['FreedomBooks-Setup-*.exe']
artifacts = [file for pattern in patterns for file in (pkg / 'dist').glob(pattern)]
if len(artifacts) != len(patterns):
    raise SystemExit(f'Expected one artifact per format, found {artifacts}')
run([sys.executable, str(pkg / 'verify-artifacts.py'), '--release', *map(str, artifacts)])
if args.platform == 'linux':
    run([sys.executable, str(pkg / 'test-deb.py'), str(next(p for p in artifacts if p.suffix == '.deb'))])
    run([sys.executable, str(pkg / 'test-appimage.py'), '--extract-and-run', str(next(p for p in artifacts if p.suffix == '.AppImage'))])
# Signing secrets belong only to the separate workflow placeholder step.
for artifact in artifacts:
    with artifact.open('rb') as source:
        digest = hashlib.file_digest(source, 'sha256').hexdigest()
    artifact.with_name(artifact.name + '.sha256').write_text(f'{digest}  {artifact.name}\n')
print(f'RELEASE BUILD OK {args.platform}: {len(artifacts)} unsigned artifacts verified')
