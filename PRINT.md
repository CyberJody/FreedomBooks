# Desktop printing

The customer document, customer deposit receipt, bank register, reconciliation report and financial report
templates are mechanical ports of the frozen Python reference. The print inventory includes historical,
draft and void document states, bank and card registers, reconciliation detail/summary/undone states,
and the reports' date/basis/account/group matrices. GL printing uses every matching row rather than
the screen's page. Existing 2,000-line banking export limits remain enforced.

From a document's **Print / Save as PDF** link, the app opens the print view in the same window.
Its button offers the native **Print…** and **Save as PDF…** actions. The File menu provides the same
actions and opens the current printable document/report automatically. PDF output uses a native Save
dialog, Electron's main-process `webContents.printToPDF`, Letter sizing and the template's page rules.
Physical printing uses main-process `webContents.print` with the native printer dialog.

No renderer API or command type was added. The button sends `SubmitForm` with a single-use capability
bound to the exact displayed path and query. The main process independently checks the open book,
the displayed local print view and its successful render; it rechecks lock and navigation after the Save
dialog and after PDF generation, before writing. The strict CSP is unchanged. The isolated preload
applies the template-generated `@page` footer through the CSSOM, just as the existing page icon does.

The frozen Python app has no credit-memo print URL, so the desktop offers none either (lead decision,
2026-11-02). The credit memo screen stays exactly as Django renders it; the File print actions refuse
it. The invented route, context adapter and three synthetic print cases have been removed. Credit-memo
printing is a **post-launch candidate**, to be designed only after approval. The print inventory now
has 110 full print cases and 622 total screen comparisons.

No accounting service or migration changed. All print amounts come from stored snapshots or the
existing financial/banking queries. Only current open invoice revisions show their current settled/due
amounts. Presentation generation time is injectable alongside the existing date hook, and the screen
comparison freezes both using `clock.json`; the app keeps its local entry-date behavior.

Reproduce from this worktree with `python3 tools/print_gates.py <name>`: `screens`, `rediff`, `gate`,
`harness`, `native`, `node-tests`, `smoke`. `native` runs `npm run build:native` and must precede smoke.
These commands run in the foreground and retain logs/artifacts under the project's `staging/print/`.
They use the isolated `fbv2_core_prref` / `fbv2_core_pr` databases, which must be dropped after checks.
`bash tools/print_native_smoke.sh` enforces the native-build-before-smoke order. After the gate passes,
`bash tools/print_post_gate.sh` runs the two ambient-clock comparisons, the full harness, both database
drops with an absence check, and the final evidence inventory in sequence.
The real smoke prints its posted invoice and P&L, extracts the PDF text, verifies their identifiers and
totals, and renders page 1 to PNG using Poppler. It also exercises cancellation, capability renewal,
native physical-print dispatch and every screen fixture through navigation with the real CSP.


Inherited reference defect: sales-receipt prints' back link is labelled `Back to sales_receipt` and
uses the invoice-detail route with the receipt ID. PRINT retains it to meet the required Django
parity. The File print actions work independently of that link. A follow-up can correct the link and
its wording on both sides after the lead authorizes the reference change.


## Visual verification

Inspected the first and bottom 1440×1000 captures of all 113 full print cases (the pre-existing toolbar
fragment is counted separately), using the 26 labelled contact sheets in `staging/print/review/`.
Also inspected the invoice screen, credit memo, dense register and undone reconciliation at full size.
All columns and totals remain visible. Register/reconciliation text wraps tightly in the same screen
sheet width as Django; no styles were redesigned to conceal that inherited layout.

Inspected all five pages of the 40-line invoice PDF: rows remain whole, table headings repeat,
the last-page subtotal/tax/total stay together, and each page has its document/revision/footer/page count.
Also inspected page 1 of the invoice and P&L PDFs, and the landscape register PDF. The register's
ten columns fit on landscape Letter; long names, memo and number wrap within their cells. Its PDF
test checks cell glyph order with raw text extraction as well as retaining spatial text for review.

Native evidence: invoice `INV-000001`, total/balance 180.00; P&L net income 180.00; five-page draft
`INV-000003`, 40 lines and total 1,000.00; landscape register `PRINT-REGISTER-1`, ending balance 1234.56.
Each PDF opens with Poppler and has a page-1 PNG under `staging/print/`; all five long-invoice pages
are rendered. No physical printer was used: the smoke verifies main-process native-print dispatch
with its printer callback stub, while `printToPDF` runs for real.

The UTC-labelled invoice balance date follows the UTC print instant, independently of the local entry
date hook. A regression reproduced the previous-local-day/following-UTC-day case before the correction,
then passed with the UTC date while proving that the entry-date hook remains on the previous local day.

## Original PRINT acceptance (before fix 1)

The final standard gate exited 0: 1495 test executions passed, none failed. The two
ignored executions are the existing intentionally ignored glibc oracle diagnostic, once per feature
configuration. Clippy, workspace tests, alternate JSON features, core isolation and all 625 screen
comparisons passed. All 128 full differential-harness scenarios MATCH with zero failures and warnings.

The two ambient local clocks (23:30 and 00:30) each MATCH all 625 screens; all 1,250 normalized
Python/Rust files are byte-identical between runs. The native build preceded the real Electron smoke:
45 desktop checks plus 10 report checks passed, with a separate landscape-register PDF smoke passing.
All 13 Node tests and the focused print regression run passed.

Final evidence includes 113 full print views and their bottom captures at 1440×1000, eight PDF page
PNGs, the PDF/text files and labelled review contact sheets. The validated machine-readable result is
`staging/print/acceptance.json`; `staging/print/evidence.json` includes dimensions and SHA-256 hashes.
Full foreground logs are retained there as `gate.log`, `harness.log`, `clock.log`, `native.log`,
`smoke.log`, `smoke-register.log`, `node-tests.log` and `cleanup.log`.

Both assigned databases (`fbv2_core_prref`, `fbv2_core_pr`) were dropped and their absence verified.
The clock-proof clone was dropped on exit. The desktop preview entry is `fb://app/`; no HTTP preview
server is retained. The branch remains separate for lead review and merge.


## PRINT fix 1: protected destinations

PDF and every CSV export share `electron/main/save-file.cjs`. The native Save dialog confirms
regular-file replacement; the main process refuses the entire active application data directory,
`$XDG_DATA_HOME/FreedomBooks` (default `~/.local/share/FreedomBooks`), the open book and its sidecars
and keys, and `.fbbook*` filenames anywhere. It checks chosen and canonical paths, resolves parent
aliases, and refuses all leaf symlinks, dangling links and non-files. The refusal reads:
“Choose a different file name. That location is used by FreedomBooks.”

Output goes to a private 0600 random temporary file in the selected canonical directory, opened
exclusively with `wx`, fsynced, then renamed over the destination. Linux pins the directory by fd
through `/proc/self/fd`; other platforms check its identity before rename. Renaming never follows
a destination symlink. Failure before rename preserves the old destination and removes our temp.
Internal key persistence is not a user-selected output path. There is no desktop backup Save-dialog
handler on this branch; any future one must use this same helper. Chromium downloads remain blocked.

The reviewer's `save-path-test.cjs` and `save-live.cjs` are retained as the adapted
`electron/scripts/save-path-regression.cjs` and `save-live-regression.cjs`: refusal and an unchanged
synthetic book SHA-256 are required. The latter also checks normal and replacement PDF/CSV output.
`electron/tests/save-file.test.cjs` uses real filesystem entries plus injected leaf/parent races,
temp collisions and write/fsync/rename failures. The review's direct-book probe failed before the
fix (`exact-book must refuse`); protected-path regressions now pass.

Fix verification uses `python3 tools/print_gates.py <gate|harness|native|smoke|smoke-register|node-tests|save-path|save-live|cleanup>`.
Its dedicated databases are `fbv2_core_prf` / `fbv2_core_prfref`; artifacts are in `staging/printfix1/`.
The earlier acceptance section above records the original PRINT run, before this fix.


PDF margins use a white Electron window canvas (`backgroundColor: '#ffffff'`), matching Django's
browser output. Ported HTML/CSS and their on-screen tan theme are unchanged. `printBackground:true`
still preserves the document's intentional shaded cells and other template backgrounds.


The destination guard uses `fs.realpathSync.native` before lexical path cleanup. A regression proved
that `alias/../book-key.sealed` must follow the parent symlink before resolving `..`; Node's ordinary
JavaScript realpath implementation can collapse `..` first. Both that form and ordinary parent aliases
now refuse protected locations, without writing to an unintended lexical destination. The regression
failed first, then passed with all 25 Node tests (12 dedicated save-policy tests).


## PRINT fix 1 acceptance — 2026-11-02

The standard `scripts/gate.sh` exited 0: 1496 test executions passed,
0 failed, and 2 existing intentionally ignored oracle executions.
Clippy, workspace tests, alternate JSON features, core isolation and all 622
screen comparisons passed. The inventory has 110 full print cases; the three
invented credit-print cases are gone. No new normalization or accounting service/migration was added.

The full differential harness MATCHed all 128 scenarios with zero failures
and warnings. The final native rebuild preceded the desktop smoke (45/45)
and D8 smoke (10/10); the separate landscape-register smoke also passed.
All 25 Node tests passed, including 12 real-filesystem save-policy tests.
The retained reviewer probes refuse the open book directly and through `invoice.pdf`, with the
encrypted book SHA-256 unchanged. Normal and replacement PDF/CSV saves pass in real Electron.

Re-rendered and inspected the eight PDF page PNGs (invoice, P&L, five-page invoice and landscape
register). All eight have white margins (255,255,255), against the review's tan baseline (247,243,236).
The inherited DRAFT placement, register wrapping/number formatting and footer wording remain as
Django renders them. The review's PDF safety audit still finds no paths, keys, scripts or attachments.
The credit-memo detail capture has no print link and MATCHes Django.

The assigned `fbv2_core_prf` / `fbv2_core_prfref` databases were dropped after verification, with live
absence checked. Logs, PDFs, PNGs, save-regression evidence and the strict `acceptance.json` are in
`staging/printfix1/`. Preview entry: `fb://app/`; all smoke windows are closed and no HTTP preview is
retained. Physical printing was dispatch-tested with a callback stub. The branch stops before merge.


## Business details

Edit company now stores the seller's address, phone, email, website, optional tax ID,
logo and payment instructions inside the book. The current details appear on invoice,
estimate and sales-receipt print/PDF headers; payment instructions appear beside the
totals. Empty fields produce the previous markup exactly. Existing documents also use
the current contact details; their accounting snapshots and amounts remain intact.

Logos must decode as PNG or JPEG, at most 128 KB and 1200 × 1200 pixels. They are
re-encoded without metadata, retained inside the book and served through the local
authenticated image route. No logo URL or network access is used. Payment instructions
are plain text, up to 2000 characters. Use the removal checkbox to clear a saved logo.

Run `python3 tools/w0_docs_checks.py targeted`, `gate`, `native`, `npm`, then `smoke`
from the worktree. The real smoke produces filled/empty invoice and estimate PDFs,
page PNGs, settings captures and `w0-docs-evidence.json` under `staging/w0-docs/smoke`.
The genuine prior-template comparison and prior-schema migration tests are separate
from the deliberate company-edit/audit-picker screen-diff adapters in NORMALIZATION.md.
