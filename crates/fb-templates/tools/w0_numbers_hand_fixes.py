"""Add numbering preferences after the mechanically ported company form."""
from pathlib import Path

path = Path(__file__).resolve().parents[1] / 'templates/ledger/form.html'
old = '</section>{% endblock %}'
new = '</section>{% if numbering %}{% include "ledger/numbering.html" %}{% endif %}{% endblock %}'
text = path.read_text()
assert text.count(old) == 1
path.write_text(text.replace(old, new))
