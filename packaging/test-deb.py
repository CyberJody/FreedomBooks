#!/usr/bin/env python3
"""Test apt's dependency resolution and the installed .deb in a disposable Debian 13 container."""
import argparse
import os
from pathlib import Path
import subprocess

pkg = Path(__file__).resolve().parent
root = pkg.parent
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('artifact', type=Path)
args = parser.parse_args()
artifact = args.artifact.resolve()
if artifact.parent != pkg / 'dist' or artifact.suffix != '.deb':
    raise SystemExit('Expected a .deb in packaging/dist')
result = pkg / '.work/deb-smoke'
result.mkdir(parents=True, exist_ok=True)
command = [os.environ.get('FB_CONTAINER_ENGINE', 'docker'), 'run', '--rm', '--init',
           '--name', f'freedombooks-pkg-installers-{os.getpid()}',
           # Docker's default seccomp blocks Chromium's namespace sandbox. Keep
           # Chromium sandboxed inside this disposable test container instead.
           '--cap-add=SYS_ADMIN', '--security-opt', 'seccomp=unconfined', '--security-opt', 'apparmor=unconfined',
           '--shm-size=512m', '-v', f'{root}:/src:ro', '-v', f'{result}:/results',
           '-e', f'FB_DEB=/src/packaging/dist/{artifact.name}',
           '-e', 'DEBIAN_FRONTEND=noninteractive', 'debian:13-slim',
           'bash', '/src/packaging/linux/test-container.sh']
subprocess.run(command, check=True)
