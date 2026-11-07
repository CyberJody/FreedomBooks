"""Hand fixes applied after port_template.py for the D1 screens (kept as a script so they're reviewable)."""
import re
FIX = {
    "templates/ledger/base.html": [("{% app_nav as nav %}", "")],
    "templates/ledger/register.html": [
        ("<h1>{{ account }}</h1>", "<h1>{{ account.label }}</h1>"),
        ("{{ row.line.journal }}</a>", "{{ row.line.journal.label }}</a>"),
    ],
    "templates/sales/invoices/form.html": [
        ("{{ csrf_token }}", "{{ csrf_token|safe }}"),
        ("{{ form.non_field_errors }}{{ form.request_key }}{{ form.expected_revision }}{{ form.request_key.errors }}{{ form.expected_revision.errors }}",
         "{{ form.non_field_errors|safe }}{{ form.f.request_key.widget|safe }}{{ form.f.expected_revision.widget|safe }}{{ form.f.request_key.errors|safe }}{{ form.f.expected_revision.errors|safe }}"),
        ("{% if form.errors or formset.total_error_count %}", "{% if form.has_errors or formset.total_error_count %}"),
        ("{{ field.label_tag }} {{ field }}{{ field.errors }}", "{{ field.label_tag|safe }} {{ field.widget|safe }}{{ field.errors|safe }}"),
        ("{{ formset.management_form }}{{ formset.non_form_errors }}", "{{ formset.management_form|safe }}{{ formset.non_form_errors|safe }}"),
        ("{% for line in formset %}", "{% for line in formset.forms %}"),
    ],
    "templates/sales/invoices/line_fields.html": [
        ("{{ field }}{{ field.errors }}{% endfor %}{{ line.non_field_errors }}", "{{ field.widget|safe }}{{ field.errors|safe }}{% endfor %}{{ line.non_field_errors|safe }}"),
        ("{{ field.label_tag }}{{ field }}{{ field.errors }}", "{{ field.label_tag|safe }}{{ field.widget|safe }}{{ field.errors|safe }}"),
    ],
    "templates/sales/invoices/list.html": [
        ("{{ filter_form.as_p }}", "{{ filter_form.as_p|safe }}"),
        ("{% for rev in page %}", "{% for rev in page.object_list %}"),
        ("{{ rev.invoice.identity.number }}", "{{ rev.number }}"),
        ("{{ rev.company_snapshot.currency }}", "{{ rev.currency }}"),
        ("{{ customer_labels|party_label(rev.customer_id)|default(rev.customer_snapshot.display_name) }}", "{{ rev.customer_label }}"),
    ],
}
for path, pairs in FIX.items():
    text = open(path, encoding="utf-8").read()
    for old, new in pairs:
        assert old in text, (path, old)
        text = text.replace(old, new)
    open(path, "w", encoding="utf-8").write(text)
print("hand fixes applied")
