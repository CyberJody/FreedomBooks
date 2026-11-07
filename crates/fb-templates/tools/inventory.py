"""Inventory of the frozen app's templates for DESKTOP-WAVES.md.

    python3 crates/fb-templates/tools/inventory.py [reference dir] > target/template-inventory.tsv

One row per .html file: module, path, bytes, POST forms, formsets, inline <script> blocks, inline style
attributes, includes/extends, and whether D1 ported it.
"""
import os
import re
import sys

REF = sys.argv[1] if len(sys.argv) > 1 else "/mnt/Data/Projects/FreedomBooks/wt/reference"
PORTED = {"ledger/base.html", "ledger/nav.html", "ledger/pager.html", "ledger/company_home.html", "ledger/accounts.html",
          "ledger/register.html", "sales/invoices/list.html", "sales/invoices/form.html", "sales/invoices/line_fields.html"}

rows = []
for dirpath, dirs, files in os.walk(REF):
    dirs[:] = [d for d in dirs if d not in (".venv", "node_modules", ".git")]
    for name in files:
        if not name.endswith(".html"):
            continue
        full = os.path.join(dirpath, name)
        rel = os.path.relpath(full, REF)
        text = open(full, encoding="utf-8").read()
        tpl = rel.split("/templates/", 1)[1] if "/templates/" in rel else rel
        module = rel.split("/", 1)[0]
        rows.append((
            module, tpl, len(text),
            len(re.findall(r'<form[^>]*method="post"', text, re.I)),
            len(re.findall(r"formset", text)),
            len(re.findall(r"<script(?![^>]*\bsrc=)(?![^>]*application/json)[^>]*>", text)),
            len(re.findall(r'\sstyle="', text)),
            ",".join(sorted(set(re.findall(r"{%\s*(?:include|extends)\s+'([^']+)'", text)))),
            "yes" if tpl in PORTED else "",
        ))
print("module\ttemplate\tbytes\tpost_forms\tformset_refs\tinline_scripts\tstyle_attrs\tincludes\tported_d1")
for r in sorted(rows):
    print("\t".join(str(x) for x in r))
print(f"# {len(rows)} templates, {sum(r[5] for r in rows)} inline scripts, {sum(1 for r in rows if r[8])} ported", file=sys.stderr)
