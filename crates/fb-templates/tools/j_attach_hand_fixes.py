"""J-ATTACH: mechanical port of the frozen attachments panel for the journal page, hand fixes asserted.

    python3 crates/fb-templates/tools/j_attach_hand_fixes.py

The detail pages inline the panel; the journal page renders this one with the D9 panel's view model.
"""
from pathlib import Path
import hashlib
import importlib.util

ROOT = Path(__file__).resolve().parents[1]
REF = Path('/mnt/Data/Projects/FreedomBooks/wt/reference')
SOURCE_SHA256 = '2d0bdca042c99553b7aa1f49e6fa3b59d798fb9b8c0458c51f0454979d2edd42'
spec = importlib.util.spec_from_file_location('port', ROOT/'tools/port_template.py')
port = importlib.util.module_from_spec(spec)
spec.loader.exec_module(port)

TOKEN = '<input type="hidden" name="csrfmiddlewaretoken" value="{{ csrf_token }}">'
PAIRS = [
    # Each form posts to its own path, so each carries its own command token input.
    (TOKEN + '<label>Reason', '{{ file.remove_token|safe }}<label>Reason', 1),
    (TOKEN, '{{ csrf_token|safe }}', 1),
]

if __name__ == '__main__':
    source = (REF/'attachments/templates/attachments/panel.html').read_text()
    assert hashlib.sha256(source.encode()).hexdigest() == SOURCE_SHA256
    text = port.port(source)
    for old, new, count in PAIRS:
        assert text.count(old) == count, (old, text.count(old))
        text = text.replace(old, new)
    dest = ROOT/'templates/attachments/panel.html'
    dest.parent.mkdir(parents=True, exist_ok=True)
    dest.write_text(text)
    print('J-ATTACH: ported attachments/panel.html with %d asserted hand fixes' % len(PAIRS))
