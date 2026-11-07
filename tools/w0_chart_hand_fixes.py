"""Apply the starter-chart choice to the desktop-only first-run template."""
from pathlib import Path

root = Path(__file__).resolve().parents[1]
path = root / 'crates/fb-templates/templates/desktop/gate.html'
old = '<p><label for="id_lock_password">App-lock password'
choice = ('<p><label for="id_starter_chart">Chart of accounts:</label> '
          '<select name="starter_chart" id="id_starter_chart" required>'
          '<option value="standard"{% if starter_chart != "empty" %} selected{% endif %}>Standard small-business chart (recommended)</option>'
          '<option value="empty"{% if starter_chart == "empty" %} selected{% endif %}>Start with an empty chart (for accountants)</option>'
          '</select></p>')
text = path.read_text()
assert text.count(old) == 1
assert choice not in text
path.write_text(text.replace(old, choice + old))
