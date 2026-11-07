"""SIM-FIX (owner approved 11/03): asserted hand changes to already-ported templates.

    python3 crates/fb-templates/tools/sim_fix_hand_fixes.py          # apply (idempotent)
    python3 crates/fb-templates/tools/sim_fix_hand_fixes.py --check  # every change is in place

Each pair is a deliberate divergence from the frozen Django template, recorded in DESKTOP-WAVES.md
("SIM-FIX") and normalized only in its span by crates/fb-screendiff/src/normalize/sim_fix.rs.
TRIAGE.md (repo root) has the v1 file:line for each item.
"""
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1] / "templates"

FIXES = {
    # Item 2: a posted invoice's document panel shows its payment state next to the posting state
    # ("Posted · Paid"), where the live payment state is known (the detail page). Review and
    # historical revisions have no live payment state and keep the recorded state word.
    "sales/invoices/document.html": [(
        "<p>{{ invoice.identity.number }} · {{ current.get_state_display }} · {{ current.company_snapshot.currency }}</p>",
        "<p>{{ invoice.identity.number }} · {% if current.state == 'OPEN' and current.payment_state %}Posted · "
        "{{ current.payment_state|title }}{% else %}{{ current.get_state_display }}{% endif %} · {{ current.company_snapshot.currency }}</p>",
    )],
    # Item 5: each report preset option carries the dates it resolves to today (data-start/data-end), so
    # the desktop preload fills From/To when a preset is chosen, before Run. Custom carries none.
    "ledger/reports/_controls.html": [(
        "{% for key, label in presets %}<option value=\"{{ key }}\"{% if controls.preset == key %} selected{% endif %}>",
        "{% for key, label, start, end in presets %}<option value=\"{{ key }}\"{% if end %} data-start=\"{{ start }}\" "
        "data-end=\"{{ end }}\"{% endif %}{% if controls.preset == key %} selected{% endif %}>",
    )],
}


def main(check):
    missing = []
    for rel, pairs in FIXES.items():
        path = ROOT / rel
        text = path.read_text(encoding="utf-8")
        for old, new in pairs:
            if text.count(new) == 1 and old not in text:
                continue
            if check:
                missing.append(f"{rel}: {new[:70]}")
                continue
            assert text.count(old) == 1, f"{rel}: expected exactly one {old[:70]!r}"
            text = text.replace(old, new)
        if not check:
            path.write_text(text, encoding="utf-8")
    if missing:
        sys.exit("SIM-FIX hand fixes missing:\n" + "\n".join(missing))
    print("SIM-FIX hand fixes in place" if check else "SIM-FIX hand fixes applied")


if __name__ == "__main__":
    main("--check" in sys.argv)
