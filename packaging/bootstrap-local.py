#!/usr/bin/env python3
"""Optional worker-only tools, extracted inside this worktree; never sudo/install."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tarfile
import urllib.request

root = Path(__file__).resolve().parent
work = root / '.work'
work.mkdir(exist_ok=True)
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('tool', choices=['node', 'linux', 'windows', 'llvm'])
args = parser.parse_args()
if args.tool == 'node':
    index = json.load(urllib.request.urlopen('https://nodejs.org/dist/index.json'))
    version = next(item['version'] for item in index if item['version'].startswith('v24.'))
    filename = f'node-{version}-linux-x64.tar.xz'
    url = f'https://nodejs.org/dist/{version}/'
    checksums = urllib.request.urlopen(url + 'SHASUMS256.txt').read().decode()
    digest = next(line.split()[0] for line in checksums.splitlines() if line.endswith('  ' + filename))
    archive = work / filename
    if not archive.exists():
        urllib.request.urlretrieve(url + filename, archive)
    if hashlib.sha256(archive.read_bytes()).hexdigest() != digest:
        raise SystemExit('Node download checksum mismatch')
    with tarfile.open(archive) as tar:
        tar.extractall(work, filter='data')
    link = work / 'node'
    if not link.exists():
        link.symlink_to(f'node-{version}-linux-x64', target_is_directory=True)
    print(f'NODE VERIFIED {version}: {link / "bin"}')
else:
    packages = ['xvfb', 'xserver-common', 'libxfont2', 'xdotool', 'libxdo3'] if args.tool == 'linux' else [
        'wine64', 'libwine', 'gcc-mingw-w64-x86-64-posix', 'g++-mingw-w64-x86-64-posix',
        'gcc-mingw-w64-x86-64-posix-runtime', 'binutils-mingw-w64-x86-64',
        'mingw-w64-x86-64-dev', 'mingw-w64-common', 'gcc-mingw-w64-base',
        'libz-mingw-w64', 'libcapi20-3t64', 'libgettextpo0', 'libosmesa6', 'libpcap0.8t64',
    ]
    if args.tool == 'llvm':
        packages = ['clang-19', 'lld-19', 'llvm-19', 'llvm-19-runtime', 'llvm-19-linker-tools',
                    'libllvm19', 'libclang-cpp19', 'libclang1-19', 'libclang-common-19-dev']
    downloads = work / 'downloads'
    downloads.mkdir(exist_ok=True)
    # apt-get download verifies repository checksums and only writes the .deb files here.
    subprocess.run(['apt-get', 'download', *packages], cwd=downloads, check=True)
    prefix = work / 'tools'
    prefix.mkdir(exist_ok=True)
    for archive in sorted(downloads.glob('*.deb')):
        package = subprocess.check_output(['dpkg-deb', '-f', str(archive), 'Package'], text=True).strip()
        if package in packages:
            subprocess.run(['dpkg-deb', '-x', str(archive), str(prefix)], check=True)
    print(f'LOCAL TOOLS {prefix}')
