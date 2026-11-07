#!/usr/bin/env python3
"""Acceptance requires one successful execution of every required step on this candidate."""
from pathlib import Path
import re
import sys

REQUIRED = {'gate.log':'GATE_RC', 'harness.log':'HARNESS_RC', 'smoke.log':'SMOKE_RC',
            'smoke-legacy.log':'SMOKE_LEGACY_RC'}

def passed(directory):
    for file, tag in REQUIRED.items():
        path=Path(directory)/file
        if not path.is_file(): return False
        values=re.findall(r'^'+tag+r'=(\d+)$',path.read_text(errors='replace'),re.M)
        if values != ['0']: return False
    return True

if __name__=='__main__':
    ok=passed(sys.argv[1])
    print('acceptance: all required steps passed' if ok else 'acceptance: FAILED or incomplete; inspect logs')
    raise SystemExit(0 if ok else 1)
