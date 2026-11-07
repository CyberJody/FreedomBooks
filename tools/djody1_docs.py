"""DESKTOP-JODY1 item 3 docs: N13 in the screen-diff NORMALIZATION.md, "Deliberate divergences" in DESKTOP-WAVES.md."""
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
AUTHORITY = ('Authority: Jody, via Dev Master, 11/03 ~10:50, from his screenshots: "(2) Report a problem should be '
             'an in-app how-to for submitting a GitHub issue with a working link to CyberJody/FreedomBooks '
             'issues/new/choose; steps, expected/actual, version/OS, redacted screenshot, public-data warning. '
             "Remove 'No report address is set ... person who runs this copy' placeholder; no email reporting.\" "
             'and "(3) Remove unnecessary footer text \'No payroll processing\' — this is copy removal only, not '
             'payroll functionality."')

p = ROOT / 'crates/fb-screendiff/NORMALIZATION.md'
t = p.read_text()
assert 'N13' not in t
p.write_text(t.rstrip('\n') + '''

# DESKTOP-JODY1: deliberate divergences (N13)

''' + AUTHORITY + '''

| Rule | Normalization | Reason and boundary |
|---|---|---|
| N13a | On the **Django side only**, the exact normalized footer suffix ` · Bank feeds are read-only · No payroll processing · <a class="report-problem" href="/help/report/">Report a problem</a></footer></body></html>` becomes the same suffix without ` · No payroll processing`. | Copy removal only. It applies only when the page has one `<footer>` and ends with that suffix; the phrase anywhere else, a second footer, trailing markup or a desktop page that kept the phrase all remain a difference. |
| N13b | On the **Django side only**, the exact normalized old `<main>` of `/help/report/` (with the "No report address is set…" placeholder) becomes the exact approved desktop how-to (`normalize/desktop.rs` `report_new`). | The desktop page is a GitHub-issue how-to with the issue link, the copyable URL, the public-data warning and the version/OS block. The screen diff gives the Rust app fixed host details (`0.0.0-screendiff`, `Screen-diff OS 1`) so the approved text compares exactly. A changed reference page, or any other wording, link, version or OS on the desktop page, remains a difference. |

The Rust side is never rewritten by N13. `normalize/desktop.rs` tests render the real desktop page and the
real reference page (fixture `normalize/fixtures/report-problem.django.html`), assert MATCH, and assert DIFF for
mutated desktop wording, link, version, a kept payroll phrase or placeholder, and for a mutated reference page.
''')

p = ROOT / 'DESKTOP-WAVES.md'
t = p.read_text()
assert '## Deliberate divergences' not in t
p.write_text(t.rstrip('\n') + '''

## Deliberate divergences

Screens that intentionally differ from the Django reference. Each is normalized in the diff by an exact
Django-side rewrite (never a Rust-side one), listed in `crates/fb-screendiff/NORMALIZATION.md`, and tested to
DIFF on any other change.

- **Report a problem + footer (DESKTOP-JODY1, N13).** ''' + AUTHORITY + '''
  - `/help/report/` is a how-to: open the issue page (system browser via the main process,
    `shell.openExternal` behind the exact-URL allowlist `guards.EXTERNAL_URLS`; the renderer never navigates),
    steps, expected, actual, version and OS (from `app.getVersion()` and the host's os-release / system
    version, shown ready to copy), and a screenshot only after covering names, amounts and account numbers,
    under a "GitHub issues are public" warning. The report-address setting (`fb_feedback_url`) and its
    placeholder are gone; there is no email path.
  - The footer no longer says "No payroll processing" on any screen. Payroll behaviour is unchanged.
  - v1 (`app/`) is not changed; this is a desktop-only divergence.
''')
print('ok')
