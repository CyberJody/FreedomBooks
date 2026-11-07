"""DESKTOP-BACKUPS: asserted hand changes to the mechanical port of backups/page.html.

    python3 crates/fb-templates/tools/port_template.py <reference>/backups/templates/backups/page.html \
        crates/fb-templates/templates/backups/page.html
    python3 crates/fb-templates/tools/desktop_backups_hand_fixes.py

Parity parts (heading, messages, status grid, create form, file table) stay Django's. Two parts are
deliberate desktop divergences (DESKTOP-WAVES.md "Deliberate divergences", NORMALIZATION.md):
the notes after the create form (server passphrase / web-request size limit do not exist on a
desktop) and the restore help (a server command) replaced by the desktop restore-as-new-book flow.
"""
import hashlib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PATH = ROOT / "templates" / "backups" / "page.html"
MECHANICAL_SHA256 = "20d7df60f2475294d0ce78d396eb0cbf6c1151a713c0b7db2dc56a549e1e289b"

CREATE_NOTES_OLD = (
    "{% if encrypting %}<p class=\"muted\">Encrypted with the server's backup passphrase.</p>{% else %}"
    "<p class=\"backup-warning\">Not encrypted: keep downloaded copies somewhere safe.</p>{% endif %}"
    "<p class=\"muted\">The button works for databases up to {{ web_limit|filesizeformat }}, so the backup finishes "
    "within the web server's time limit for one page; bigger ones use <code>manage.py fb_backup</code> on the server.</p>"
    "{% else %}<p class=\"errorlist\">This database is larger than the browser limit ({{ web_limit|filesizeformat }}): "
    "a backup this size could run past the web server's time limit for one page. Run <code>manage.py fb_backup</code> "
    "on the server.</p>{% endif %}</div>"
)
CREATE_NOTES_NEW = (
    "{% endif %}<form method=\"post\" action=\"{{ url('backups:page', company.pk) }}\">{{ folder_token|safe }}"
    "<input type=\"hidden\" name=\"action\" value=\"folder\"><button type=\"submit\" class=\"secondary\">Choose folder…</button></form>"
    "<p class=\"muted\">Encrypted with the key that opens these books. Each backup is checked right after it is written.</p></div>"
)
HELP_OLD = (
    "<details class=\"backup-help\"><summary>Restoring a backup</summary><p>Restoring is done on the server, into a new "
    "database; the current books are never overwritten.</p><pre><code>python manage.py fb_restore FILE --target-db NEW_NAME"
    "</code></pre><p>Only restore backups you trust. A plain (unencrypted) backup is not authenticated: anyone who could "
    "change the file could change the books.</p><p class=\"muted\">Not in a backup: server settings and keys (.env, "
    "FB_SECRETS_KEY). Back those up separately.</p></details>"
)
HELP_NEW = (
    "<section class=\"narrow backup-restore\"><h2>Restore a backup</h2>"
    "<p>Restoring makes a new, separate book from a backup file. The open book is never changed or replaced.</p>"
    "<form method=\"post\" action=\"{{ url('backups:page', company.pk) }}\" class=\"backup-restore-form\">{{ restore_token|safe }}"
    "<input type=\"hidden\" name=\"action\" value=\"restore\">"
    "<p><label for=\"id_new_book\">Name for the restored book</label><input type=\"text\" name=\"new_book\" id=\"id_new_book\" "
    "maxlength=\"60\" required value=\"{{ new_book }}\"></p><button type=\"submit\">Restore a backup as a new book…</button></form>"
    "{% if restored %}<form method=\"post\" action=\"{{ url('backups:page', company.pk) }}\" "
    "class=\"backup-restored\">{{ open_token|safe }}<input type=\"hidden\" name=\"action\" value=\"open\"><p class=\"notice\" role=\"status\">The restored book "
    "<strong>{{ restored }}</strong> is ready. Your open book was not changed.</p><button type=\"submit\">Open restored book</button></form>{% endif %}"
    "<p class=\"muted\">Only restore backups you trust. A backup opens only with the key that opens these books. "
    "Not in a backup: the app-lock password and the key itself.</p></section>"
)
PAIRS = [
    ("{{ csrf_token }}", "{{ csrf_token|safe }}"),
    # Messages are (level, text) values; Django prints the message object itself.
    # BACKUPS-POLISH: a refused restore adds a small details line under the plain sentence (desktop only;
    # Django's messages never carry one, so the rendered page is unchanged whenever it is absent).
    ("{{ message }}", "{{ message.text }}{% if message.detail %}<small class=\"message-detail\">{{ message.detail }}</small>{% endif %}"),
    (CREATE_NOTES_OLD, CREATE_NOTES_NEW),
    (HELP_OLD, HELP_NEW),
]

text = PATH.read_text()
digest = hashlib.sha256(text.encode()).hexdigest()
if digest != MECHANICAL_SHA256:
    raise SystemExit(f"mechanical source changed: {digest}")
for old, new in PAIRS:
    assert text.count(old) == 1, old
    text = text.replace(old, new)
PATH.write_text(text)
print("backups/page.html: %d hand changes" % len(PAIRS))
