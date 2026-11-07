

## SEC-FIX1: intended security difference from v1

New free text (names, memos, descriptions, addresses and related text fields) refuses
Unicode Cc/Cf except TAB, including bidi controls U+202A–202E/U+2066–2069 and
zero-width U+200B–200D/U+2060/U+FEFF. Validation runs before whitespace stripping.
The plain refusal is “Control and formatting characters are not allowed.” This
deliberately hardens v1, which accepted them; existing stored text is not rewritten.
Dedicated SEC-FIX1 screen fixtures assert the Rust refusal independently of v1;
the ordinary differential corpus uses TAB instead of LF/CR in its valid free-text
inputs on both runners. LF/CR refusal remains covered by dedicated security tests.
No output or comparer normalization is added; all parity checks stay exact.
CSV text uses the same cell encoder for audit and reports; leading TAB/CR is
prefixed with an apostrophe and quoted. This is a deliberate export hardening.
## J-PAN: card numbers in payment free text

The reference, memo and order reference of payment documents refuse a Luhn-valid 13–19 digit card
number, including digit groups joined by four or more spaces or dashes (v1 accepts it). Refusals are
Rust-only tests; the differential scenario
`j-pan-card-near-misses.json` keeps near-miss text exact on both sides. Stored card numbers are masked
(all but the last four digits) in audit history and CSV exports, which v1 echoes; no fixture holds one,
so no output normalization is added. See DESKTOP-WAVES.md, "Card numbers in payment free text".
# FIX3 input/copy boundary (lead decision)

The Cc/Cf refusal (TAB allowed) applies to newly supplied or changed human text.
Unchanged fields identified by their stored record and line origin, generated source journals,
reversals, voids, snapshot carryover, restore and upgrade retain historical text. Pure calculation
and model-integrity checks do not reapply an input policy to stored evidence.
`tools/dfix3_upgrade.py` builds actual `98100e0` and `cc40505` sources and exercises legacy
`Legacy\nline` records through these paths, comparing all old row hashes and fresh/upgraded schema hashes.

Bank-provider text is cleaned, never refused for these characters: ingress removes the same frozen
Cc/Cf set (TAB allowed) before known-secret masking, redaction, length checks and storage. Raw provider
identifiers still determine their HMAC identity keys. A nonzero removal count is recorded as
`removed_characters` and in the sync summary, without recording the removed text. Clean-provider audit
events keep their existing shape. The partial-stage and failure paths record counts too.



## W0-DOCS: company details on customer documents (approved 2026-11-05)

The company-edit GET and invalid-POST screens intentionally gain address, contact,
optional tax ID, plain-text payment instructions and a local logo upload/removal.
`normalize/company_documents.rs` extends only the Django reference form with the
independently specified empty fields and exact upload hint; the desktop HTML is
never stripped. All existing fields, errors, labels and limits still compare.
Filled settings and print views are deliberate new output, checked by the
W0-DOCS Electron flow and retained screenshots/PDFs.

Invoices, estimates and the shared sales-receipt print template use current company
business details, including on documents created before the settings were filled.
Immutable accounting snapshots and their amounts, company name and currency stay
intact. New details are absent when empty: a regression renders the genuine prior
template and the new one byte-for-byte across twelve kind/status combinations;
the ordinary full screen diff covers the existing print inventory with empty settings.
No statements exist on the base, so no statement feature is added.

Logo admission is based on decoded PNG/JPEG bytes, at most 128 KB and 1200 × 1200
pixels. It re-encodes pixels and rejects a larger result, dropping metadata and
trailing payloads. The image stays inside the book and is served through an
authenticated, no-store `fb://app/c/<company>/document-logo/` response. No CSP or
renderer command authorization changes. Payment instructions allow 2000 characters,
with escaped text and line breaks; tax ID is free text and has no label when blank.

Migration `0180_company_documents` adds one optional table; missing rows mean all
fields are empty. It does not rewrite company rows or document snapshots. The upgrade
test starts from the exact 0161 schema and compares every prior table and schema object.

The audit action picker also gains “Company document details changed”. The reference adapter inserts only that exact option after “Company created”; the frozen label inventory and every other audit control remain unchanged.
