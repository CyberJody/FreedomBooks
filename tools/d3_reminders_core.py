"""Adopt the existing 1:1 reminder-query translation from the integration branch.

It is absent on D2. This adds a service and its four frozen-Python test ports,
without changing any existing core behavior or adding a migration. Integration
can retain its identical copy when the two branches meet.
"""
from pathlib import Path
import subprocess

for name in ['crates/fb-core/src/reminders.rs', 'crates/fb-core/tests/reminders.rs']:
    text=subprocess.check_output(['git','show','int-l4:'+name],text=True)
    Path(name).write_text(text)
p=Path('crates/fb-core/src/lib.rs');s=p.read_text();assert 'pub mod reminders;' not in s;p.write_text(s.replace('pub mod reports;','pub mod reports;\npub mod reminders;'))
