#!/usr/bin/env python3
"""Extract and verify the final artifacts without installing/running the Windows app."""
import argparse
import hashlib
import os
from pathlib import Path
import subprocess
import tempfile
import zipfile

pkg = Path(__file__).resolve().parent
root = pkg.parent
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('artifacts', type=Path, nargs='+')
parser.add_argument('--release', action='store_true', help='Require the project LICENSE reserved for publication')
parser.add_argument('--incomplete-windows', action='store_true', help='Explicitly inspect a non-runnable unpacked Windows developer shell')
args = parser.parse_args()
if args.release and not (root / 'LICENSE').is_file():
    raise SystemExit('Release blocked: project LICENSE with approved Electron exception is missing (publication step)')

for artifact in args.artifacts:
    artifact = artifact.resolve()
    platform = 'win32' if artifact.suffix == '.exe' or artifact.is_dir() else 'linux'
    reference = root / f'electron/out/FreedomBooks-{platform}-x64'
    upstream = subprocess.check_output(['node', str(pkg / 'upstream-electron.cjs'), platform], text=True).strip().splitlines()[-1]
    with zipfile.ZipFile(upstream) as archive:
        originals = {name: archive.read(name) for name in ['LICENSE', 'LICENSES.chromium.html']}
    with tempfile.TemporaryDirectory(prefix='fb-artifact-', dir=os.environ.get('TMPDIR')) as temp:
        destination = Path(temp)
        incomplete = artifact.is_dir() and args.incomplete_windows and not args.release
        if artifact.is_dir():
            assert incomplete, 'Unpacked incomplete Windows shell requires --incomplete-windows and is never a release artifact'
            assert (artifact / 'INCOMPLETE-WINDOWS-NATIVE.txt').is_file()
            tree = artifact
        elif artifact.suffix == '.deb':
            subprocess.run(['dpkg-deb', '-x', str(artifact), temp], check=True)
            tree = destination / 'opt/FreedomBooks'
            desktop = destination / 'usr/share/applications/app.freedombooks.FreedomBooks.desktop'
            text = desktop.read_text()
            assert 'StartupWMClass=app.freedombooks.FreedomBooks' in text
            assert 'Terminal=false' in text
            assert 'Exec=/opt/FreedomBooks/freedombooks' in text
        elif artifact.suffix == '.AppImage':
            with (destination / 'extract.log').open('w') as log:
                subprocess.run([str(artifact), '--appimage-extract'], cwd=temp, stdout=log, check=True)
            tree = destination / 'squashfs-root'
            assert '--no-sandbox' not in (tree / 'AppRun').read_text()
            assert '--no-sandbox' not in (tree / 'app.freedombooks.FreedomBooks.desktop').read_text()
        elif artifact.suffix == '.exe':
            subprocess.run(['7z', 'x', '-y', f'-o{temp}', str(artifact)], stdout=subprocess.DEVNULL, check=True)
            payloads = list(destination.rglob('app-64.7z'))
            assert len(payloads) == 1, f'Expected NSIS x64 payload: {payloads}'
            tree = destination / 'installed'
            subprocess.run(['7z', 'x', '-y', f'-o{tree}', str(payloads[0])], stdout=subprocess.DEVNULL, check=True)
        else:
            raise SystemExit(f'Unknown artifact: {artifact}')
        for name, original in originals.items():
            actual = tree / 'licenses/electron' / name
            assert actual.read_bytes() == original, f'Electron notice modified or absent: {actual}'
        assert (tree / 'THIRD-PARTY-NOTICES.md').read_bytes() == (root / 'THIRD-PARTY-NOTICES.md').read_bytes()
        if (root / 'LICENSE').is_file():
            assert (tree / 'licenses/freedombooks/LICENSE').read_bytes() == (root / 'LICENSE').read_bytes()
        subprocess.run(['node', str(pkg / 'verify-tree.cjs'), str(tree), str(reference), platform, *(['incomplete-shell'] if incomplete else [])], check=True)
        print(f'ARTIFACT LICENSES OK: {artifact.name}; Electron notices byte-identical; THIRD-PARTY-NOTICES present; project LICENSE ' + ('present' if (root / 'LICENSE').is_file() else 'awaits publication'), flush=True)
        if not artifact.is_dir():
            with artifact.open('rb') as source:
                print(f'SHA256 {hashlib.file_digest(source, "sha256").hexdigest()}  {artifact.name}', flush=True)
