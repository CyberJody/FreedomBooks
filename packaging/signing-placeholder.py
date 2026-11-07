#!/usr/bin/env python3
"""Fail closed if future signing credentials are set; never print their values."""
import argparse
import os

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('platform', choices=['linux', 'win32'])
args = parser.parse_args()
names = ['FB_WINDOWS_SIGNING_CERT_BASE64', 'FB_WINDOWS_SIGNING_PASSWORD'] if args.platform == 'win32' else ['FB_LINUX_GPG_PRIVATE_KEY', 'FB_LINUX_GPG_PASSPHRASE']
if any(os.environ.get(name) for name in names):
    raise SystemExit('Signing credentials configured but signing is a placeholder; implement the SIGNING.md handler before release')
print(f'UNSIGNED ALPHA {args.platform}: no signing credentials configured')
