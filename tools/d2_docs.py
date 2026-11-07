"""Record D2's exact screen-diff normalization contract and desktop verification recipe."""
from pathlib import Path

Path('crates/fb-screendiff/NORMALIZATION.md').write_text('''# Screen diff normalizations (ARCHITECTURE 9.4)

`run_screendiff.sh` applies `scenarios/screens-d2.json` through frozen Python services and fb-core,
then renders every state in `screens.json`. GETs, invalid POSTs, the read-only fill preview, the audit
CSV and the print-toolbar-note include are covered. Both sides assert the expected response status;
a POST that unexpectedly saves/redirects fails. D1's nine screens remain in the same inventory.
Use the dedicated `fbv2_int_d2_ref` / `fbv2_int_d2_screens` databases. No reference source is changed.

Anything not listed below compares exactly. In particular, the known control-account refusal difference
in `D2-CORE-GAPS.md` is **not normalized** and remains a failing screen state.

| Rule | Normalization | Reason and boundary |
|---|---|---|
| N1 (D1) | Collapse HTML whitespace runs and whitespace between tags. | Django widget templates insert formatting whitespace. CSV bypasses this rule. |
| N2 (D1) | Replace the CSRF / single-use command hidden input by `{form-token}`. | The desktop token replaces CSRF and is bound to the actual form action path. |
| N3 (D1) | Normalize a generated UUID only in `name="request_key" value="…"`. | Creation idempotency keys are fresh per render. Invalid typed keys remain exact. |
| N4a (D1, extended D2) | Map IDs only at matched Django route argument positions. | Company, account, invoice, journal and receipt IDs map by model. `record_id` on `/lists/<kind>/` maps only through that kind (customer/vendor/item). Query `customer=` and audit-filter `user=` likewise map through their own models. |
| N4b (D1, extended D2) | Map select option values by field name, plus the known identity's expected `#…` suffix. | Customer, item, income-account, journal-line account, setup roles, banks and audit users each have a declared model. Python's label suffix is its pk. The approved desktop suffix is the per-company ordinal in stable UUIDv7 creation order, **including inactive rows**. `label:<model>:<natural-key>` records the expected label on each side; a wrong ordinal remains a difference. UUIDs remain opaque option values and never appear in visible labels. |
| N4c (D1) | Parse/canonicalize `invoice-catalog` and `customer-tax` JSON, mapping known item/customer keys and `income_account` IDs. | Key order is immaterial; every other JSON field/value stays exact. |
| N5 (D2) | Normalize generated audit timestamps only in the audit history When cell, the entry-history UTC cell, the posted-at sentence, or the **first column** of an audit CSV row. | Each allowed value is read from that side's actual audit/journal rows. Arbitrary dates, transaction dates, other columns and prose are untouched. Whole-second collisions are expected, so timestamp placeholders are column-specific; event order, operators, actions, targets, reasons and record links still compare exactly. |
| N6 (D2) | Parse Python's repr only inside the entry-history `View change` detail cell, canonicalize object key order, and map `before/after.lines[].account_id` and root `reversal_id` by their models. | PostgreSQL JSONB/Python repr and UUID-backed Rust detail have different key/ID representations. The parser preserves all values, list order, missing/extra fields and free-text strings. A memo that resembles `account_id` is never rewritten. Other `<pre>` elements are untouched. |
| N7 (D2) | Normalize only the current recomputed proposal's digest in `name="digest" value="…"`. | The digest includes backend-specific company/account IDs. Each renderer reads the actual proposal from its own database. The proposal's chosen accounts, reasons, order, revision and empty state still compare exactly. |

`ids.json` comes from each side's database, never from the compared page. It uses natural keys by model.
Same-name customers get a per-company creation ordinal in the symbolic key so duplicate rows cannot
overwrite each other. A second company's fixtures deliberately consume global Python pks before the
primary chart/customers/items, proving label ordinals are per company rather than raw global pks.

CSV preserves every byte except the allowed generated first-column timestamp: BOM, CRLF, quoting,
formula guards, empty-cell apostrophes, column labels and row order all compare exactly. The desktop's
native Save dialog replaces a browser download; no new IPC method or renderer network access is added.

Normalizations are tested against wrong-model IDs, wrong ordinals, altered free text, duplicate repr
keys, trailing repr input and ordinary preformatted text. They do not change runtime templates/data.
''')

p=Path('electron/README.md')
text=p.read_text().replace('# FreedomBooks desktop shell (D1)','# FreedomBooks desktop shell (D1 + D2)')
text+='''
## D2

The permanent identity is `app.freedombooks.FreedomBooks` (product name FreedomBooks). The normal data
folder is `<appData>/app.freedombooks.FreedomBooks` on every OS; `FB_DESKTOP_USER_DATA` remains an explicit
test override. Windows sets its AppUserModelId; Linux sets the desktop-file name and WM_CLASS.

Company/account forms, setup/checklist/fill, journals, audit history/CSV, trial balance and Report a
problem use the ported Django screens. POSTs honor each form's action and use typed commands. Audit CSV
uses a native Save dialog through the existing `books.run` method. The frozen empty journal attachment
panel keeps its URLs; actual attachment operations/populated rows belong to D9. PDF printing is later.

Regenerate the fourteen D2 templates with:

    PYTHONDONTWRITEBYTECODE=1 python3 crates/fb-templates/tools/port_d2.py

The extended smoke exercises real account/setup forms, journal Add line, invalid input retention,
editing/posting/reversal, CSV save, the other D2 screens, and D1's hardened-window/invoice checks.
Set `FB_SMOKE_SHOTS` to the project's `staging/d2` directory. It also captures the shared print-note
include from `target/screendiff/rust/print-toolbar-note.html`, so run the screen diff first.

If Xvfb is absent, extract Debian's `xvfb` package into this worktree's ignored `electron/.tooling/xvfb`
and prepend its `usr/bin` directory to PATH. No sudo or machine-wide install is needed.

The known existing core refusal mismatch is documented in `../D2-CORE-GAPS.md`; the screen gate keeps
that case visible instead of changing the core or suppressing its missing sentence.
'''
p.write_text(text)
