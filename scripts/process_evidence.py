#!/usr/bin/env python3
"""Run/check exact-SHA verification evidence. Never turn quick evidence into acceptance.

process_evidence.py run --kind quick|acceptance --output /outside/repo/result.json -- COMMAND...
process_evidence.py check --kind acceptance --output /outside/repo/result.json -- COMMAND...
Only clean, identical SHA/toolchain/environment/test-set evidence is reusable.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
import time

ENV_KEYS = ('RUSTFLAGS', 'RUSTDOCFLAGS', 'CARGO_BUILD_TARGET', 'CARGO_PROFILE_DEV_DEBUG', 'RUST_TEST_THREADS',
            'CARGO_ENCODED_RUSTFLAGS', 'FB_GATE_BASE', 'TZ', 'FB_HARNESS_TEMPLATE', 'FB_HARNESS_DB',
            'FB_HARNESS_VAR', 'FB_SMOKE_SHOTS', 'FB_BACKUPS_SMOKE_DIR', 'NODE_ENV')


def output(argv, cwd):
    return subprocess.run(argv, cwd=cwd, text=True, capture_output=True, check=True).stdout.strip()


def identity(root, command):
    tools = {}
    for tool, args in (('rustc', ['-Vv']), ('cargo', ['-V']), ('node', ['--version']), ('npm', ['--version']), ('python3', ['--version'])):
        try:
            tools[tool] = output([tool, *args], root)
        except (OSError, subprocess.CalledProcessError):
            tools[tool] = 'unavailable'
    # SHA covers tracked tests, fixtures and code. Hash the external integration verifier too.
    h = hashlib.sha256()
    for rel in ('scripts/gate.sh', 'scripts/gate_quick_plan.py', 'scripts/process_evidence.py',
                'crates/fb-db/check_json_features.sh', 'crates/fb-screendiff/run_screendiff.sh',
                'crates/fb-harness/run_harness.sh', 'electron/package.json', 'electron/package-lock.json'):
        p = root/rel
        h.update(rel.encode()); h.update(p.read_bytes() if p.exists() else b'MISSING')
    for arg in command:
        p = Path(arg)
        if p.is_absolute() and p.is_file():
            h.update(str(p).encode()); h.update(p.read_bytes())
    return dict(sha=output(['git','rev-parse','HEAD'], root),
                dirty=output(['git','status','--porcelain','--untracked-files=all'], root),
                toolchain=tools, platform=platform.platform(), testset=h.hexdigest(), command=command,
                environment={key:os.environ.get(key) for key in ENV_KEYS})


def reusable(evidence, current, kind):
    return (evidence.get('kind') == kind and evidence.get('exit_code') == 0
            and not current.get('dirty') and evidence.get('identity') == current)


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('action', choices=('run','check'))
    p.add_argument('--kind', required=True, choices=('quick','acceptance'))
    p.add_argument('--output', required=True)
    p.add_argument('--root', default='.')
    argv = list(sys.argv[1:] if argv is None else argv)
    split = argv.index('--') if '--' in argv else len(argv)
    command = argv[split+1:] if split < len(argv) else []
    args = p.parse_args(argv[:split])
    root = Path(args.root).resolve(); dest = Path(args.output).resolve()
    if dest.is_relative_to(root):
        p.error('evidence belongs outside the source tree')

    if args.action == 'check':
        if not command:
            print('evidence: check requires an independent expected -- COMMAND...', file=sys.stderr)
            return 1
        record = json.loads(dest.read_text())
        current = identity(root, command)
        ok = reusable(record, current, args.kind)
        print('evidence: exact clean identity matches' if ok else 'evidence: stale, dirty, wrong tier or failed')
        return 0 if ok else 1
    if not command:
        p.error('run requires -- COMMAND...')
    before = identity(root, command)
    started = time.time()
    rc = subprocess.run(command, cwd=root).returncode
    after = identity(root, command)
    record = dict(kind=args.kind, identity=before, exit_code=rc,
                  unchanged=before == after, duration_seconds=time.time()-started)
    if before != after:
        record['exit_code'] = rc or 2
    dest.parent.mkdir(parents=True, exist_ok=True)
    temp = dest.with_name(dest.name + f'.{os.getpid()}.tmp')
    temp.write_text(json.dumps(record, indent=2)+'\n'); os.replace(temp, dest)
    print(f'evidence: {dest}; reusable={reusable(record, after, args.kind)}', flush=True)
    return record['exit_code']

if __name__ == '__main__':
    raise SystemExit(main())
