"""D9: mechanical template port of the bank-feed screens, with every hand adaptation asserted here.

    python3 tools/d9_hand_fixes.py

Each pair is (old, new, expected count) and is asserted against the mechanical output, so a change in
the frozen template or in port_template.py fails loudly instead of silently diverging.
"""
from pathlib import Path
import importlib.util

ROOT = Path(__file__).resolve().parents[1]
REF = Path('/mnt/Data/Projects/FreedomBooks/wt/reference')
spec = importlib.util.spec_from_file_location('port', ROOT/'crates/fb-templates/tools/port_template.py')
port = importlib.util.module_from_spec(spec)
spec.loader.exec_module(port)
FILES = ['bankfeeds/'+n for n in ['list', 'connect', 'forget', 'accounts', 'rules', 'rule_form', 'review']]


def source(name):
    return REF/name.split('/')[0]/'templates'/(name+'.html')


def form_fields(form, names):
    pairs = []
    for name in names:
        pairs += [
            ('{{ %s.%s.label_tag }}' % (form, name), '{{ %s.f.%s.label_tag|safe }}' % (form, name)),
            ('{{ %s.%s.errors }}' % (form, name), '{{ %s.f.%s.errors|safe }}' % (form, name)),
            ('{{ %s.%s }}' % (form, name), '{{ %s.f.%s.widget|safe }}' % (form, name)),
        ]
    return pairs


COMMON = [
    ('{{ csrf_token }}', '{{ csrf_token|safe }}'),
    ('{{ form.non_field_errors }}', '{{ form.non_field_errors|safe }}'),
]
PAIRS = {
    'list': [
        # Django's `is False` tells a failed sync from a never-run one (None); the view model says so.
        ('{% if conn.last_sync_ok is false %}', '{% if conn.last_sync_failed %}', 1),
        # Dates are formatted in the view model exactly as Django's `date` filter prints them (UTC).
        ("{{ conn.last_sync_at|date('Y-m-d H:i') }}", '{{ conn.last_sync_display }}', 1),
        # Each form carries a command token bound to its own action path (the sync URL), not the page's.
        ('{{ csrf_token }}', '{{ conn.sync_token|safe }}', 1),
        # Desktop-only state (Rust books only, never in a Django fixture): a connection whose saved
        # sign-in could not be read offers Forget and Connect again, never Sync.
        ('{% if conn.connected %}{% if conn.last_sync_failed %}', '{% if conn.connected and conn.needs_reconnect %}Reconnect needed{% elif conn.connected %}{% if conn.last_sync_failed %}', 1),
        ('<td class="actions">{% if conn.connected %}', '<td class="actions">{% if conn.connected and conn.needs_reconnect %}<a href="{{ url(\'bankfeeds:forget\', company.pk, conn.pk) }}">Forget</a> <a href="{{ url(\'bankfeeds:connect\', company.pk) }}">Connect again</a>{% elif conn.connected %}', 1),
        # Desktop messages (app.messages) appear only when there are any; Django's list has none.
        ('{% if notice %}', '{% for message in messages %}<p class="notice">{{ message }}</p>{% endfor %}{% if notice %}', 1),
    ],
    'connect': COMMON + form_fields('form', ['label', 'setup_token']),
    'forget': [('{{ csrf_token }}', '{{ csrf_token|safe }}', 1)],
    'accounts': [
        ("{{ connection.last_sync_at|date('Y-m-d H:i') }}", '{{ connection.last_sync_display }}', 1),
        ("{{ feed.balance_at|date('Y-m-d') }}", '{{ feed.balance_at_display }}', 1),
        ('<label for="{{ field.id_for_label }}">{{ feed }}</label>', '<label for="{{ field.id_for_label }}">{{ feed.label }}</label>', 1),
        ('{{ field }}{{ field.errors }}', '{{ field.widget|safe }}{{ field.errors|safe }}', 1),
        ("{{ url('bankfeeds:sync', company.pk, connection.pk) }}\">{{ csrf_token }}", "{{ url('bankfeeds:sync', company.pk, connection.pk) }}\">{{ connection.sync_token|safe }}", 1),
        # Desktop-only (never in a Django fixture): a connection whose saved sign-in could not be read never offers Sync.
        ('<div class="actions">{% if connection.connected %}<form method="post" action=', '<div class="actions">{% if connection.connected and not connection.needs_reconnect %}<form method="post" action=', 1),
        ('{{ csrf_token }}', '{{ csrf_token|safe }}', 1),
        ('{{ form.non_field_errors }}', '{{ form.non_field_errors|safe }}', 1),
    ],
    'rules': [
        ('{{ csrf_token }}', '{{ csrf_token|safe }}', 1),
        ('{{ rule.get_sign_display }}', '{{ rule.sign_display }}', 1),
    ],
    'rule_form': COMMON + form_fields('form', ['name', 'match_text', 'sign', 'account', 'amount_min', 'amount_max', 'payee_name']) + [
        ('<label for="{{ form.active.id_for_label }}">{{ form.active }} {{ form.active.label }}</label>{{ form.active.errors }}',
         '<label for="{{ form.f.active.id_for_label }}">{{ form.f.active.widget|safe }} {{ form.f.active.label }}</label>{{ form.f.active.errors|safe }}', 1),
    ],
    'review': [
        ('{% for field in filter_form %}', '{% for field in filter_form.fields %}', 1),
        ('{{ field }}{{ field.errors }}', '{{ field.widget|safe }}{{ field.errors|safe }}', 1),
        ('{{ filter_form.non_field_errors }}', '{{ filter_form.non_field_errors|safe }}', 1),
        ('{% if filter_form.errors %}', '{% if filter_form.has_errors %}', 1),
        ('{{ request.get_full_path }}', '{{ full_path }}', 1),
        ('{{ csrf_token }}', '{{ csrf_token|safe }}', 1),
        ("{{ item.date|date('Y-m-d') }}", '{{ item.date }}', 1),
        ("{{ row.suggestion.matches[0].date|date('Y-m-d') }}", '{{ row.suggestion.matches[0].date }}', 1),
        ("{{ m.date|date('Y-m-d') }}", '{{ m.date }}', 1),
        ("{{ r.date|date('Y-m-d') }}", '{{ r.date }}', 1),
        ('{{ item.get_status_display }}', '{{ item.status_display }}', 1),
        ('{{ item.get_accepted_as_display|lower }}', '{{ item.accepted_as_display|lower }}', 1),
    ],
    # Dates (UTC, as Django's `date` filter prints them) and Django's `filesizeformat` text come
    # pre-formatted in the view model; messages carry their level tag.
    'page': [
        ('{{ csrf_token }}', '{{ csrf_token|safe }}', 1),
        ('{{ message }}', '{{ message.text }}', 1),
        ("{{ last.created|date('Y-m-d H:i') }}", '{{ last.created_min }}', 1),
        ('{{ last.size|filesizeformat }}', '{{ last.size_text }}', 1),
        ("{{ last_verified.verified_at|date('Y-m-d H:i') }}", '{{ last_verified.verified_min }}', 1),
        ("{{ last_verified.created|date('Y-m-d H:i') }}", '{{ last_verified.created_min }}', 1),
        ('{{ total_bytes|filesizeformat }}', '{{ total_text }}', 1),
        ('{{ db_bytes|filesizeformat }}', '{{ db_text }}', 1),
        ('{{ web_limit|filesizeformat }}', '{{ web_limit_text }}', 2),
        ("{{ row.created|date('Y-m-d H:i:s') }}", '{{ row.created_sec }}', 1),
        ('{{ row.size|filesizeformat }}', '{{ row.size_text }}', 1),
        ("{{ row.verified_at|date('Y-m-d H:i:s') }}", '{{ row.verified_sec }}', 1),
    ],
}
FILES += ['backups/page']

# assistant/page: every form posts to its own action, so each carries a token bound to that path.
ACTIONS = [
    ("action=\"{{ url('assistant:notice', company.pk) }}\">", 'notice'),
    ("action=\"{{ url('assistant:model', company.pk) }}\" class=\"ai-model-form\">", 'model'),
    ("action=\"{{ url('assistant:models_refresh', company.pk) }}\" class=\"ai-inline-form\">", 'models_refresh'),
    ("action=\"{{ url('assistant:disconnect', company.pk) }}\">", 'disconnect'),
    ('action="{{ start_url }}">', 'start'),
    ("action=\"{{ url('assistant:key', company.pk) }}\" class=\"ai-key-form\" novalidate>", 'key'),
    ("action=\"{{ url('assistant:read_attachments', company.pk) }}\" class=\"ai-inline-form\">", 'read_attachments'),
]
PAIRS['assistant_page'] = [(a + '{{ csrf_token }}', a + '{{ tokens.%s|safe }}' % name, 1) for a, name in ACTIONS] + [
    ('{{ message }}', '{{ message.text }}', 1),
    ("{{ connection.updated_at|date('Y-m-d H:i') }}", '{{ connection.updated_display }}', 1),
    ("{{ connection.last_used_at|date('Y-m-d H:i') }}", '{{ connection.last_used_display }}', 1),
    ('{{ key_form.api_key.id_for_label }}', '{{ key_form.f.api_key.id_for_label }}', 1),
    ('{{ key_form.api_key.label }}', '{{ key_form.f.api_key.label }}', 1),
    ('{{ key_form.api_key.help_text }}', '{{ key_form.f.api_key.help_text }}', 1),
    ('{{ key_form.api_key }}', '{{ key_form.f.api_key.widget|safe }}', 1),
    # Desktop-only addition (Jody's rule), the one D9 normalization of added text (NORMALIZATION.md).
    ('<h2 id="ai-plan-title">Use your ChatGPT plan</h2>', '<h2 id="ai-plan-title">Use your ChatGPT plan</h2><p class="ai-desktop-note">For powering your personal finance.</p>', 1),
]
FILES += ['assistant/page']


def _key(name):
    return 'assistant_page' if name == 'assistant/page' else name.rsplit('/', 1)[-1]


def hand(text, name):
    short = _key(name)
    for pair in PAIRS[short]:
        old, new = pair[0], pair[1]
        expected = pair[2] if len(pair) > 2 else 1
        assert text.count(old) == expected, (name, old, text.count(old))
        text = text.replace(old, new)
    return text


if __name__ == '__main__':
    for name in FILES:
        dest = ROOT/'crates/fb-templates/templates'/(name+'.html')
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_text(hand(port.port(source(name).read_text()), name))
    print('D9: regenerated exactly %d templates with asserted hand fixes' % len(FILES))
    # Earlier waves inlined the frozen attachments panel into each detail page with one command token
    # (bound to the add URL). Each remove form posts to its own path, so it carries that file's token.
    old = "action=\"{{ url('attachments:remove', company.pk, file.pk) }}\">{{ attachments.csrf_token|safe }}<label>"
    new = "action=\"{{ url('attachments:remove', company.pk, file.pk) }}\">{{ file.remove_token|safe }}<label>"
    patched = 0
    for path in sorted((ROOT/'crates/fb-templates/templates').rglob('*.html')):
        text = path.read_text()
        if 'attachments-panel' not in text or (old not in text and new not in text):
            continue
        assert text.count(old) + text.count(new) == 1, path
        path.write_text(text.replace(old, new))
        patched += 1
    assert patched == 9, patched  # the four sales pages (D4) keep their own pending-attachment token
    print('D9: %d inlined attachment panels carry per-file remove tokens' % patched)
    # The inlined Read with AI section (D6 bill detail): each file's form posts to its own path.
    bill = ROOT/'crates/fb-templates/templates/payables/bills/detail.html'
    old = 'action="{{ row.url }}" class="ai-inline-form">{{ csrf_token|safe }}'
    new = 'action="{{ row.url }}" class="ai-inline-form">{{ row.token|safe }}'
    text = bill.read_text()
    assert text.count(old) + text.count(new) == 1
    bill.write_text(text.replace(old, new))
    print('D9: the bill detail Read with AI forms carry per-file tokens')
    # The inlined receipt_form_panel (D6 bill form): the upload posts to assistant:read_upload.
    form = ROOT/'crates/fb-templates/templates/payables/bills/form.html'
    old = "action=\"{{ url('assistant:read_upload', company.pk) }}\" class=\"ai-upload-form\">{{ csrf_token|safe }}"
    new = "action=\"{{ url('assistant:read_upload', company.pk) }}\" class=\"ai-upload-form\">{{ ai_upload_token|safe }}"
    text = form.read_text()
    assert text.count(old) + text.count(new) == 1
    form.write_text(text.replace(old, new))
    print('D9: the new-bill upload form carries its own token')
