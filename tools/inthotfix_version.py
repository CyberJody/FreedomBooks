"""INT-HOTFIX: desktop package version 0.1.0-d3 (electron/package.json and its lock file root)."""
from pathlib import Path
import json

root = Path(__file__).resolve().parents[1]
OLD, NEW = '0.1.0-d1', '0.1.0-d3'

pkg = root / 'electron/package.json'
text = pkg.read_text()
assert text.count(f'"version": "{OLD}"') == 1
pkg.write_text(text.replace(f'"version": "{OLD}"', f'"version": "{NEW}"', 1))

lock = root / 'electron/package-lock.json'
data = json.loads(lock.read_text())
assert data['version'] == OLD and data['packages']['']['version'] == OLD
text = lock.read_text()
head, sep, rest = text.partition('"packages"')
assert head.count(f'"version": "{OLD}"') == 1
rest_head, sep2, rest_tail = rest.partition('"dependencies"')
assert rest_head.count(f'"version": "{OLD}"') == 1
lock.write_text(head.replace(OLD, NEW, 1) + sep + rest_head.replace(f'"version": "{OLD}"', f'"version": "{NEW}"', 1)
                + sep2 + rest_tail)
data = json.loads(lock.read_text())
assert data['version'] == NEW and data['packages']['']['version'] == NEW
assert json.loads(pkg.read_text())['version'] == NEW
print('version', NEW)
