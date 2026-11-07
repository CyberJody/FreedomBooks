#!/usr/bin/env python3
"""Build the ABI-stable fb-node module for a release target, without global installs."""
import argparse
import os
from pathlib import Path
import shutil
import subprocess

root = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('platform', choices=['linux', 'win32'])
args = parser.parse_args()
env = dict(os.environ)
env.setdefault('CARGO_BUILD_JOBS', '4')
command = ['cargo', 'build', '--locked', '--release', '-p', 'fb-node']
if args.platform == 'linux':
    source = root / 'target/release/libfb_node.so'
    destination = root / 'electron/native/fb-node.node'
elif os.name == 'nt':
    env['RUSTFLAGS'] = env.get('RUSTFLAGS', '') + ' -C target-feature=+crt-static'
    source = root / 'target/release/fb_node.dll'
    destination = root / 'electron/native/win32-x64/fb-node.node'
else:
    # cargo-xwin + clang-cl use the same MSVC ABI as the Windows CI runner.
    # All optional bootstrap tools and SDK downloads stay inside this worktree.
    tools = root / 'packaging/.work/tools/usr'
    clang_cl = tools / 'lib/llvm-19/bin/clang-cl'
    if not clang_cl.exists() and (clang_cl.parent / 'clang').exists():
        clang_cl.symlink_to('clang')
    env['PATH'] = os.pathsep.join([str(root / 'packaging/.work/cargo-tools/bin'),
        str(tools / 'lib/llvm-19/bin'), str(tools / 'bin'), env['PATH']])
    env['LD_LIBRARY_PATH'] = str(tools / 'lib/x86_64-linux-gnu') + os.pathsep + env.get('LD_LIBRARY_PATH', '')
    env['XWIN_CACHE_DIR'] = str(root / 'packaging/.work/xwin')
    env['XWIN_ARCH'] = 'x86_64'
    env['CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS'] = '-C target-feature=+crt-static'
    if not shutil.which('cargo-xwin', path=env['PATH']) or not shutil.which('clang-cl', path=env['PATH']):
        raise SystemExit('Need cargo-xwin and clang-cl (see packaging/README.md), or build on Windows CI')
    command = ['cargo', 'xwin', 'build', '--locked', '--release', '-p', 'fb-node', '--target', 'x86_64-pc-windows-msvc']
    source = root / 'target/x86_64-pc-windows-msvc/release/fb_node.dll'
    destination = root / 'electron/native/win32-x64/fb-node.node'
subprocess.run(command, cwd=root, env=env, check=True)
destination.parent.mkdir(parents=True, exist_ok=True)
shutil.copy2(source, destination)
print(f'NATIVE BUILT {args.platform}/x64 napi8 {destination}')
