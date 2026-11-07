# Screen diff normalizations (ARCHITECTURE 9.4)

SEC-FIX1's intended free-text input difference is documented in
[`../../NORMALIZATION.md`](../../NORMALIZATION.md). Valid scenario inputs use TAB
instead of LF/CR on both runners; dedicated `fb-app/tests/secfix1.rs` fixtures
assert the refusal and field-error render independently of v1. This adds no
output normalization and changes no screen-inventory entry.

Both presentation runners read `clock.json`: November 1, 2026 at noon in
America/New_York. Python patches every entry-view `date.today()`, the report clock,
and both aware clock providers (`timezone.now()` and `payables.calendar._utcnow`);
Rust supplies the corresponding local date to
`App.today`. Entry defaults and reminder calculations share that instant. Process TZ
and the desktop's production clock are unchanged. Dates compare literally.

`run_screendiff.sh` applies `scenarios/screens-dfix2.json` through frozen Python services and fb-core,
then renders every state in `screens.json`. GETs, invalid POSTs, the read-only fill preview, the audit
CSV and the print-toolbar-note include are covered. Both sides assert the expected response status;
a POST that unexpectedly saves/redirects fails. D1's nine screens remain in the same inventory.
Use the dedicated `fbv2_int_d5_ref` / `fbv2_int_d5_screens` databases. No reference source is changed.

Anything not listed below compares exactly. In particular, the known control-account refusal difference
in `D2-CORE-GAPS.md` is **not normalized** and remains a failing screen state.

| Rule | Normalization | Reason and boundary |
|---|---|---|
| N1 (D1) | Collapse HTML whitespace runs and whitespace between tags. | Django widget templates insert formatting whitespace. CSV bypasses this rule. |
| N2 (D1) | Replace the CSRF / single-use command hidden input by `{form-token}`. | The desktop token replaces CSRF and is bound to the actual form action path. |
| N3 (D1) | Normalize a generated UUID only in `name="request_key" value="…"`. | Creation idempotency keys are fresh per render. Invalid typed keys remain exact. |
| N4a (D1, extended D2) | Map IDs only at matched Django route argument positions. | Company, account, invoice, journal, receipt and reconciliation IDs map by model. `record_id` on `/lists/<kind>/` maps only through that kind (customer/vendor/item). Query `customer=` and audit-filter `user=` likewise map through their own models. |
| N4b (D1, extended D2) | Map select option values by field name, plus the known identity's expected `#…` suffix. | Customer, item, income-account, journal-line account, setup roles, banks and audit users each have a declared model. Python's label suffix is its pk. The approved desktop suffix is the per-company ordinal in stable UUIDv7 creation order, **including inactive rows**. `label:<model>:<natural-key>` records the expected label on each side; a wrong ordinal remains a difference. UUIDs remain opaque option values and never appear in visible labels. |
| N4c (D1) | Parse/canonicalize `invoice-catalog` and `customer-tax` JSON, mapping known item/customer keys and `income_account` IDs. | Key order is immaterial; every other JSON field/value stays exact. |
| N5 (D2) | Normalize generated audit timestamps only in the audit history When cell, the entry-history UTC cell, the posted-at sentence, or the **first column** of an audit CSV row. | Each allowed value is read from that side's actual audit/journal rows. Arbitrary dates, transaction dates, other columns and prose are untouched. Whole-second collisions are expected, so timestamp placeholders are column-specific; event order, operators, actions, targets, reasons and record links still compare exactly. |
| N6 (D2) | Parse Python's repr only inside the entry-history `View change` detail cell, canonicalize object key order, and map `before/after.lines[].account_id` and root `reversal_id` by their models. | PostgreSQL JSONB/Python repr and UUID-backed Rust detail have different key/ID representations. The parser preserves all values, list order, missing/extra fields and free-text strings. A memo that resembles `account_id` is never rewritten. Other `<pre>` elements are untouched. |
| N7 (D2) | Normalize only the current recomputed proposal's digest in `name="digest" value="…"`. | The digest includes backend-specific company/account IDs. Each renderer reads the actual proposal from its own database. The proposal's chosen accounts, reasons, order, revision and empty state still compare exactly. |

`ids.json` comes from each side's database, never from the compared page. It uses natural keys by model.
Same-name customers get a per-company creation ordinal in the symbolic key so duplicate rows cannot
overwrite each other. A second company's fixtures deliberately consume global Python pks before the
primary chart/customers/items, proving label ordinals are per company rather than raw global pks.

## D3 additions

The default scenario is now `scenarios/screens-d3.json`, using dedicated `fbv2_int_d3_ref` /
`fbv2_int_d3_screens` databases. It retains every D1/D2 state. D3 adds directory list/detail/form/status
states, field and service errors, stale account defaults, tax settings/rates/groups/bulk/members,
tax-ON/OFF invoice pickers and reminders (including stale-setting conflicts and populated balances).
Every scenario call must succeed independently on Python and Rust before rendering starts.

| Rule | Normalization | Reason and boundary |
|---|---|---|
| N4a extension | Route positions for tax `item_id`, bill, bill payment, check, credit memo/refund and customer deposit IDs; query `vendor=`. | Each maps only through its own model; customer deposits use the receipt root identity. The query key and route argument determine the model. |
| N4b extension | `expense_account` / `default_expense_account`, `agency`, `default_item` selects; `T<id>` options only inside the sales `tax_mode` select. | Accounts, vendors and tax items have separate maps. `NONE`, `KEEP` and the blank tax choice remain literal. |
| N4c extension | Keys in the `tax-rates` JSON data block map through tax items. | Rates and all values remain exact; only object order is canonicalized. |
| N5 extension | UTC timestamps in directory's recent-change `<li>… UTC · actor · action</li>` entries. | Allowed timestamps are read from actual audit rows for every fixture company. All other text, event order, actions and actors remain exact. |
| N8 | `data-record-id` on directory table rows, with the model derived from that row's detail URL; `data-tax-item` on tax rows. | Only identity attributes in those known markup contexts change. |
| N9 | Tax-item values on `<input type="checkbox" name="members">`. | No other checkbox, label, field name, selection state or value is rewritten. |
| N10 | The current bulk-preview digest, only in `name="digest" value="…"`, and only when equal to that side's service-computed digest. | The digest includes backend-specific IDs. Stale/invalid typed digests stay exact, as do the displayed customer/item inventories and their order. |

Duplicate party fallback labels map only at typed directory detail anchors or model selects, using the
known record's expected pk/ordinal suffix. A wrong ordinal remains a difference. Additional companies'
natural keys are company-qualified, so their records and ordinals cannot overwrite another company's.
Regression checks cover wrong contexts, wrong models and wrong ordinals. The inherited D2 core
refusal wording difference is still not normalized.

CSV preserves every byte except the allowed generated first-column timestamp: BOM, CRLF, quoting,
formula guards, empty-cell apostrophes, column labels and row order all compare exactly. The desktop's
native Save dialog replaces a browser download; no new IPC method or renderer network access is added.

Normalizations are tested against wrong-model IDs, wrong ordinals, altered free text, duplicate repr
keys, trailing repr input and ordinary preformatted text. They do not change runtime templates/data.

## D4 additions

The default scenario is `scenarios/screens-d4.json`, using dedicated `fbv2_int_d4_ref` /
`fbv2_int_d4_screens` databases. Its 160 states retain D1–D3 and cover all 20 D4 templates,
invalid forms, successful bound review forms, revision evidence, conversion, reversals, voids,
allocations, credit/deposit history and receipts included in a bank deposit.

| Rule | Normalization | Reason and boundary |
|---|---|---|
| N4a extension | Estimate, sales receipt, root document and bank deposit route arguments. | The declared route argument determines the model. Revision numbers remain literal. |
| N4b/c extension | Receipt invoice/bank choices and estimate/sales-receipt item catalogs. | Known values and expected ordinal suffixes map through their own models. Amounts, rates, tax modes, ordering and selected state remain exact. |
| N5 extension | Sales event times only in actor/time table cells, revision evidence headings and sales-receipt event rows. | Allowed values come from actual sales and audit rows. Document dates and arbitrary prose remain exact. |
| N11 | Successful invoice-post and receipt-review hidden `review_token` capabilities. | Python signs the review while the desktop holds an opaque capability. The form command token is single-use. Only those review page contexts normalize generated capabilities; invalid bound values and other fields stay exact. |
| N12 | The known journal's expected pk/ordinal in the sales-receipt void event summary. | The visible journal number uses the approved company ordinal. The identity, date and free-text reason still compare exactly; a wrong ordinal is not normalized. |

Additional-company journals and root document identities have company-qualified natural keys.
The inherited control-account refusal difference remains visible and is not normalized.

The financial harness separately maps a conversion's internally generated invoice request key,
including that exact identity's `document.identified` audit event. The key is generated independently
inside each backend's conversion service. Caller-provided request keys remain exact. The estimate
manifest derives expected roots, complete revision/line cardinalities, states and conversions from
successful requests, including a zero-amount line, so equal omissions cannot silently match.
# D6 additions

- Transfer route arguments map only as `transfer` ids. Attachment document arguments map as
  document identities, collected from the actual source identities in each fixture database.
  Journal links in the D6 fixture company use that company's journal map.
- The vendor, bill, superseded-bill, bank, transfer-from and transfer-to select fields map their
  own models. Item and contact ordinal labels keep the existing per-company creation-order rule.
- `bill-catalog` keys map items, with only its `expense_account` value mapped as an account.
  `vendor-defaults` keys map vendors, with only its `expense_account` mapped as an account.
  `check-next-numbers` keys map banks; the suggested check numbers are compared literally.
- Query-only navigation links (`?year=…&month=…&vendor=…`) normalize the same named vendor
  query argument as absolute local links.
- Only the hidden `review_token` field is normalized as a generated posting capability.
  Django signs a digest; the desktop keeps the opaque Rust review in the unlocked session and
  returns a random capability. The subsequent service checks the reviewed intent and state.
- Creation timestamps are collected from the actual bill revisions, payments, payment reversals,
  transfers and transfer voids. Only those known values are normalized inside their immutable
  history tables and the historical revision evidence paragraph. Dates, amounts, descriptions,
  reasons and timestamps typed into ordinary text are compared literally.

## Desktop FIX1 integration

The default is now `scenarios/screens-dfix1.json`, with isolated
`fbv2_int_dfix1_ref` / `fbv2_int_dfix1_screens` databases. It combines the D4 and D6
extensions of D3 and includes same-date invoices revised out of creation order.
Both renderers submit every catalogue `post` object as an actual form POST/typed
`SubmitForm`; invalid fields, management forms, error ordering and Django 5.2's
`aria-invalid` / `aria-describedby` attributes compare literally.

N4 option labels replace only the generated suffix: ` · #<known id/ordinal>` for
accounts/items, or ` (#<known id/ordinal>)` for duplicate contact names whose
identity is explicitly qualified in the fixture. The suffix must end the label,
apart from the known inactive/archived/unavailable annotation. A `#` inside the
user's name is never searched or replaced. Tests require DIFF for `Sales #1`
versus `Sales #<UUID>`, `Typed #7` versus `Typed #3`, parenthesized free text,
changed names before a valid generated suffix and an incorrect ordinal.

No new normalization hides an invalid form error, choice value, amount, date,
line order, label or layout change. Shared sales/payables JSON catalogues map
only their declared `income_account` / `expense_account` fields, preserving all
other properties literally.

## D7a additions

The default scenario is `scenarios/screens-d7a.json`. It preserves all FIX1 steps and
appends an isolated `D7a synthetic books` company. Its PostgreSQL pair is
`fbv2_int_dfix1_d7a_ref` / `fbv2_int_dfix1_d7a_screens`, under the approved harness
prefix, with reference storage in this worktree's `target/d7a-reference-var`.

- Deposit IDs in other companies use company-qualified natural keys. Check revision
  numbers and check numbers are literal; current-revision ordering is compared against
  same-date drafts revised out of root-creation order, including vendor activity.
- Receipt identities map only in the deposit form's `receipts` checkbox value,
  `receipt-<id>` control ID and its matching label's `for` attribute. Selected state,
  row order, amounts and ordinary text containing `receipt-…` remain literal.
- `item-catalog` keys map items and only its `account` property maps an account.
  `vendor-catalog` keys map vendors and only its `expense_account` maps an account.
  `next-numbers` keys map banks; the suggested numbers remain literal. These are the
  frozen check helper's actual JSON element IDs.
- Check reference snapshot dictionaries are parsed only inside the frozen
  `Line N (expense/item) · catalog: … · account: …` and `Payee snapshot: …` paragraphs.
  Dictionary key order is canonicalized; only a snapshot's typed top-level `id` and
  the item's `expense_account_id` / `income_account_id` are mapped. Names, descriptions,
  addresses, costs and other values remain exact. A vendor ID can never normalize as
  an account ID, and tests require DIFF for changed user `#…` text.
- Check revision and deposit history timestamps come from actual immutable revisions
  or the corresponding deposit audit events. The saved-check revision evidence
  paragraph maps only known `document_stamp` values; ordinary prose stays literal.
  The inherited actor/time-cell and review-capability rules also apply to these forms.
  Hidden review capabilities normalize only when a successful posting-preview context
  is present. An invalid bound `review_token` is caller text and stays literal.
- The bank picker's generated ordinal may precede its frozen
  `(unavailable — replace)` annotation. It still maps only the known option identity's
  generated suffix, preserving user text and refusing incorrect ordinals.

Invalid POST errors and field attributes are never normalized. Model-choice identifiers
accept surrounding whitespace as Django's integer model PK lookup does; plain choices
and whole-receipt multiple-choice selections compare their original strings. UUID fields
strip surrounding whitespace, while the view's retry lookup retains its own UUID grammar.
## D5 credits and customer deposits

- Generated invoice IDs in credit-application field `name`, `id`, `for` and `aria-describedby` attributes map through the invoice model map. Ordinary prose and text-input values are unchanged; the must-DIFF regression keeps user-typed `amount_42` distinct from `amount_43`.
- Credit application route IDs map by memo number and application creation ordinal. Deposit application and refund IDs map by their document numbers, each under its own model. Estimate and deposit select values and `estimate` query arguments use their corresponding model maps.
- Credit/deposit history event timestamps normalize only in the generated timestamp cell immediately before a `credit_memo.*` or `customer_deposit.*` action, and only when present in that side's audit evidence. The generated application ordinal in an unapply history summary maps by its application identity; the reason text is unchanged.
- D4 actor/time normalization is restricted to its estimate/invoice revision and receipt history tables; D4/D6 revision evidence uses its owning module's page styles. A bill and a sale can share a generated second on one backend but differ on the other. The collision regression proves that this never selects a different normalization, and that literal timestamps remain distinct.
## D8 reports

The default scenario is `scenarios/screens-d8.json`. It appends report evidence to every FIX1 step,
using isolated approved-prefix databases `fbv2_int_dfix1_d8_ref` and `fbv2_int_dfix1_d8_screens`.
The two presentation runners inject November 1, 2026 into the presentation clock for every state,
including inherited form defaults and navigation reminder badges. The Python runner supplies
`date.today()` on the nine entry-view modules and `timezone.now()`; the Rust runner injects
`App.today`. Scenario services and stored timestamps are untouched. This prevents the reference
UTC date and native local date drifting across midnight during long gates. Preset and default
dates still compare literally, including the October/November boundary; no date is normalized.

- The `account=` query argument maps only through account identities on the general-ledger route or
  a query-only pager link. This covers report drill-downs, export links and GL paging. Unknown/invalid
  values and `account` parameters on other named routes remain literal.
- Only `<tbody class="gl-account" id="gl-account-…">` maps its account anchor through that same model.
  The account label, account type, amounts, opening/balance-forward state and line order stay exact.
- Report CSVs are identified by their complete seven- or thirteen-column header. Only the declared
  `Account ID` column maps account identities. Only the GL `line` row's `Link` column maps a known
  local route, removing the test client's `http://testserver` or desktop's `fb://app` origin. A native
  desktop has no HTTP origin. Field spans preserve the original CSV bytes, including quoting, CRLF,
  formula escaping, multiline text and every amount; the parser never reserializes the file.

Must-DIFF regressions cover wrong accounts, wrong markup contexts, user-typed hash text, changed
amounts and changed CSV quoting. No report control, field error, source label/number/party, date,
balance or posted line is normalized. The three controls forms are GET-only: invalid queries render
the literal Django 400 pages, and actual POSTs compare the empty 405 response on all three routes.
Cross-company account refusal is asserted by an application boundary test, without normalizing a
Django DEBUG error page.

FIX2 combines all D5, D7a and D8 scenarios and screens. The local-noon clock above supersedes the historical per-wave clock/default-scenario descriptions.

D7b adds the register/reconciliation screens and keeps that same frozen local-noon clock. The
banking view modules' imported `today` functions receive it explicitly. No entered date or balance
is normalized.

- Journal-line identities map only in a register/reconciliation row's `line-` anchor, the `line` or
  `tick` input, and each identity component of the `shown` identity/revision list. Revisions remain
  exact. The hidden `reconciliation` input maps only through reconciliation identities.
- The Begin form's service/interest account selects map account option values, including Django's
  id-before-name markup order. Unknown identities and other element contexts remain literal.
- Generated completion and undo keys normalize only in their named hidden inputs. Completion/undo
  timestamps normalize only in reconciliation history/report markup, using actual reconciliation
  events as provenance. User text, statement dates, actors and reasons remain exact.
- Register and reconciliation CSVs require their complete declared headers. Only the Link column
  maps known local document routes and the test-client/desktop origins. Only the `Reconciled at`
  metadata value and the timestamp prefix of `Undone` metadata map exact known event timestamps.
  Every other CSV byte (including signs, quoting, CRLF, filter text and numeric cells) stays exact.

Must-DIFF tests cover wrong models, changed revisions, changed balances, changed user text and
unknown timestamps. The two pre-existing core validation-wording gaps remain visible as DIFF;
they are not normalized or translated in the UI.
# FIX3: submitted undo keys

Only an undo form fetched by GET generates an `undo_key`. Screen-request metadata enables that
normalization only for reconciliation undo GETs. Every POST echo stays literal, including a valid UUID
on `d7b-undo-invalid` and `dfix3-undo-truncated-revision`. The must-DIFF regression mutates the submitted
key; a separate assertion permits independently generated GET keys to MATCH.

The generated hidden `fb_command_token` inside a D7b CSV anchor is removed in that exact context.
It is desktop bridge metadata for token-bound `SubmitForm`; Django downloads from the unchanged GET
URL. The link URL and visible CSV label remain exact, with must-DIFF checks for both.

# DESKTOP-JODY1: deliberate divergences (N13)

Authority: Jody, via Dev Master, 11/03 ~10:50, from his screenshots: "(2) Report a problem should be an in-app how-to for submitting a GitHub issue with a working link to CyberJody/FreedomBooks issues/new/choose; steps, expected/actual, version/OS, redacted screenshot, public-data warning. Remove 'No report address is set ... person who runs this copy' placeholder; no email reporting." and "(3) Remove unnecessary footer text 'No payroll processing' — this is copy removal only, not payroll functionality."

| Rule | Normalization | Reason and boundary |
|---|---|---|
| N13a | On the **Django side only**, the exact normalized footer suffix ` · Bank feeds are read-only · No payroll processing · <a class="report-problem" href="/help/report/">Report a problem</a></footer></body></html>` becomes the same suffix without ` · No payroll processing`. | Copy removal only. It applies only when the page has one `<footer>` and ends with that suffix; the phrase anywhere else, a second footer, trailing markup or a desktop page that kept the phrase all remain a difference. |
| N13b | On the **Django side only**, the exact normalized old `<main>` of `/help/report/` (with the "No report address is set…" placeholder) becomes the exact approved desktop how-to (`normalize/desktop.rs` `report_new`). | The desktop page is a GitHub-issue how-to with the issue link, the copyable URL, the public-data warning and the version/OS block. The screen diff gives the Rust app fixed host details (`0.0.0-screendiff`, `Screen-diff OS 1`) so the approved text compares exactly. A changed reference page, or any other wording, link, version or OS on the desktop page, remains a difference. |

The Rust side is never rewritten by N13. `normalize/desktop.rs` tests render the real desktop page and the
real reference page (fixture `normalize/fixtures/report-problem.django.html`), assert MATCH, and assert DIFF for
mutated desktop wording, link, version, a kept payroll phrase or placeholder, and for a mutated reference page.

## DESKTOP-BACKUPS: Backups page

`backups` (`/c/{company}/backups/`) renders first in `screens.json`, so Django's session carries no
message from an earlier POST screen into it. Both sides start with an empty, not-yet-created backup
folder (`FB_BACKUP_DIR` set by `render_django.py`; `App.backups.configure` in `fb-screendiff`).
Only on a page with exactly one Backups status grid, four spans normalize (`normalize/backups.rs`):

- the `<code>` path in the "Backup folder" row → `{backup-folder}` (each side's own folder);
- the "Database size" value → `{book-size}` (PostgreSQL `pg_database_size` vs the SQLCipher book file;
  both add attachment bytes);
- inside `<div class="backup-create">`, everything after the create `<form>` → `{desktop: create notes}`
  (Django: server passphrase / web-request size limit; desktop: book-key encryption note and "Choose folder…");
- Django's `<details class="backup-help">` server-restore help and the desktop's
  `<section class="narrow backup-restore">` restore-as-new-book flow → `{desktop: restore flow}`.

These are the deliberate divergences recorded in `DESKTOP-WAVES.md`. The heading, messages, the other
status rows (last backup, last verified, file count, folder state), the create form and button, and the
file table stay exact. Must-DIFF unit checks change the file count, the button text, the folder state and
the create action.

BACKUPS-POLISH adds no rule. The desktop-only message details line (`<small class="message-detail">`),
the renderer-only busy line and the Encrypted column (now Django's Unreadable logic) are outside the four
spans and stay literal; `only_the_four_declared_spans_normalize` checks that each still DIFFs.


## PRINT

- The desktop adds one hidden `fb_command_token` inside the exact `print-now` button. Remove only
  that input in that button with its frozen label; no other button attributes or document text are
  rewritten. A must-DIFF test preserves altered labels and token-like user text elsewhere.
- Generated print instants use the same `clock.json` local-noon instant as the entry/reminder date
  defaults. Print-generation instants, document dates/numbers, balances, taxes, totals and footers
  are compared exactly. Stored completion/undo instants use the scoped rule below.

- Reconciliation print headers contain the same stored completion/undo instants already normalized
  in reconciliation screen/history. Rewrite only a known `reconciliation_stamp` from that side's
  database, in the completed/undone field of a `rec-paper` report header; preserve dates in reasons,
  other body text and changed/unknown instants. A must-DIFF regression exercises a changed minute.
- The frozen sales-receipt print toolbar incorrectly links `invoices:detail` with the receipt's PK.
  In that exact generated back-link of a SALES RECEIPT print only, map the argument as a receipt,
  preserving the broken route and its label. Numeric Django PK collisions must not map it to an
  unrelated invoice. Must-DIFF tests preserve changed receipt identities and unrelated anchors.

# D9: bank feeds

Scenario `screens-d9.json` = the combined `screens-d7b.json` plus one separate company, "D9 feed books",
built only from the harness's `bankfeeds_*` steps with a pre-parsed pull (no network). Ids map per model:

- Route arguments `connection_id` -> `bfconnection:<label>` and `rule_id` -> `bfrule:<name>` (both runners
  write the same symbolic names for that company only).
- A downloaded account's id only inside the mapping select's own attributes (`name="map_<id>"`,
  `id`/`for="id_map_<id>"`, `aria-describedby` and the error-list id) -> `bfaccount:<remote id>`; the
  review filter's `<select name="account" id="id_account">` holds feed-account ids (not ledger accounts) and
  maps through `bfaccount` only on a page with the `review-filters` form.
- A review row's `account_<n>` / `to_<n>` / `map_<id>` options map as ledger accounts, `line_<n>` options as
  `bfline:<journal memo>:<position>`, `receipt_<n>` options as that company's receipts.
- The rules page's `move_up`/`move_down`/`activate`/`deactivate` button values map as rules only.
- Downloaded-transaction pks need no rewrite: fb-core's book-wide `ordinal` is Python's integer pk, so
  `kind_<n>`, `sel`, `accept_one` and `?from=<n>` compare literally.
- Django's renderer runs with the harness's synthetic `FB_SECRETS_KEYS` so it shows the configured branch;
  the desktop always stores bank access encrypted in the book's credential vault.

All of it is gated on the page's `<div class="bankfeed">`. Must-DIFF tests (`normalize/d9.rs`): a mapping
select naming the wrong account, a rule button carrying an account id, a ledger account in the review
filter, and look-alike text outside those attributes all stay different.

Desktop-only, never in a Django fixture (so never in the diff): the list shows `app.messages` as
`<p class="notice">` lines only when there are any, and a connection with `needs_reconnect` shows
"Reconnect needed" with only Forget and "Connect again" (no Sync); covered by `fb-app/tests/d9_flow.rs`.
Not diffed: sync and forget POSTs (they mutate or need the network), and a connect POST that passes
form cleaning (it calls the plugin).

## D9: backups page

The DESKTOP-BACKUPS page and its four-span rule (above) replace the D9 port. The two D9 cases
(`d9-backups-page`, `d9-backups-page-unknown-created`) render that page for the D9 company.

## D9: AI assistant page

render_django.py fetches the assistant screens with `Host: 127.0.0.1:8000` (screens.json `host`), Django's
loopback case, which is the desktop's only case (the SSH-tunnel branch is server-only). Two rewrites, only on
a page carrying the plan card heading: the desktop-only note
`<p class="ai-desktop-note">For powering your personal finance.</p>` (Jody's rule) is dropped when it is
exactly that element right after `<h2 id="ai-plan-title">`, and the start form's hidden `company` value maps
through the company ids (Django's integer pk vs the book's company id). Must-DIFF tests: a changed note, the
note anywhere else, and the note on another page all stay different. These screens also come first in
screens.json (Django's pending messages, as for Backups).

## D9: attachments panel

The panel's rows are real now (D9 owns attachments). On a page's `attachments-panel` section only, an upload or
removal minute (`· <stamp> UTC`, `removed by <name> <stamp> UTC`) maps to `{attachment_stamp}` when it is the
minute of one of that company's attachment rows on that side (both runners store rows at their own run time).
Attachment ids map through the `attachment_id` route argument (`attachment:<original name>`). Names, types,
sizes, actors and reasons stay exact; must-DIFF tests in `normalize/d9.rs`.

J-ATTACH (`d9-attachments-journal-panel`, a journal of the D9 company): N5's posted-at sentence now reads its allowed
stamps from every company's posted journals on each side (journal numbers are book-wide on both), not only the
scenario's main company. Same sentence, same rule; must-DIFF test
`posted_at_maps_only_a_posted_journal_stamp_in_the_posted_sentence` in `normalize.rs`. The same page's entry
history shows the `attachment.added` detail; N6's typed fields gain `attachment` (the attachment id, through the
D9 `attachment:<original name>` ids). Name, type, size, number and hashes stay exact; must-DIFF test
`an_attachment_event_maps_only_its_own_attachment_id`.

## D9: Read with AI on the bill page

A second company, "D9 AI books", has a synthetic connected ChatGPT account (harness `assistant_connect`, fake
credentials, no network), reading switched on and a draft bill with one attachment, so the bill page's Read
with AI section renders its available state. No new rewrite: its per-file read URL maps through the
`attachment_id` route argument, its `request_key` through the existing generated-key rule.
## SIM-FIX: findings of the v1 virtual-employee run (owner approved 11/03)

Deliberate divergences recorded in `DESKTOP-WAVES.md` ("SIM-FIX") and `TRIAGE.md`; template changes are in
`crates/fb-templates/tools/sim_fix_hand_fixes.py`. Each rule is in `normalize/sim_fix.rs`, with must-DIFF
unit checks. Item 1 (submit buttons) changes only the desktop preload, not rendered HTML, so it has no rule.

- **Item 2, invoice status line.** In `<section class="panel estimate-document">`, the status line
  `INV-nnnnnn · Posted · (Open|Partial|Paid) · CUR` → `INV-nnnnnn · Open · CUR` (Django's posting-state
  word). A different number, currency, any other state (Draft, Voided) or any other payment word still
  diffs, and the same words outside that panel (or on an estimate) stay literal.
  On an INVOICE print (`<p class="doc-title">INVOICE`) the revision line's `Status: Posted · (Open|Partial|Paid)<br>`
  → `Status: Open<br>` the same way; estimate and sales-receipt prints, other states and other revision
  numbers stay literal (`item2_print_status_matches_djangos_open_only_on_an_invoice_print`).
- **Item 3, transfer-list bank filter.** In the `<select name="account">` of
  `<form … class="filters transfer-filters">` only, options whose id is in the `transfer_nonbank` class are
  dropped. Only `py/render_django.py` writes that class: asset accounts that are neither designated banks
  nor used by a recorded transfer (Django offers every asset account; the desktop offers the banks). The
  desktop's ids never name the class, so a non-bank account appearing on the desktop side, a missing bank
  or a changed label still diffs. Other `account` selects are untouched.
- **Item 4, twin-name hint.** Only right after the display-name input (`id="id_display_name"><div
  class="helptext">`), the exact desktop sentence "…lists tell them apart by email, or else by a number."
  becomes Django's "…use town or email to tell them apart in lists.". Any other wording, or the sentence
  after another field, still diffs.
- **Item 5, report presets.** Only inside `<form class="filters report-controls">`: the desktop's
  `data-start="…" data-end="…"` pair on each `#report-preset` option is dropped. When the selected preset is
  not Custom and the page shows both From/To dates, the `start=`/`end=` values in the Print/CSV links
  (`<span class="report-actions">`) and in that page's `<nav class="pager">` links become the page's own
  From/To values. Django keeps stale submitted dates there, and the desktop links carry the resolved ones,
  which both sides show in the boxes. A Custom range, a different preset, basis or shown date, malformed
  option attributes, and pagers on non-report pages still diff. New fixtures `d8-pl-preset-stale-dates` and
  `d8-gl-preset-stale-dates` (`?preset=last_month&start=2026-01-01&end=2026-01-31`) cover the case.
- **Extra, audit history labels.** Only on the audit history page: the 19 desktop-added action labels (exact
  code/label pairs, `fb_app::screens::SIM_FIX_ACTION_LABELS`, checked equal to the copy in `sim_fix.rs`) map
  back to Django's code names in `<td data-label="Action">` cells inside `<table class="audit-history">` and
  drop out of the audit filter's `action` select. Document cells that now resolve sales tax rates or groups,
  the sales tax settings, deposit refunds and backups map back to Django's fallback (`Record (no longer on
  file)`, or `<number> (no longer on file)` for a deposit refund). A different label, a label on another
  code, another link target or number, and the same text outside the history table still diff.

## Release 1: plugins hidden (N14)

Authority: Jody 11/03 ~17:20 (Release 1 = Linux single-user desktop, plugins hidden). The release build
(fb-app without its `plugins` feature) leaves out bank feeds, the AI assistant / Sign in with ChatGPT, the
receipt reader and the bill AI actions. `run_screendiff.sh` builds fb-screendiff the same way;
`FB_SCREENDIFF_PLUGINS=1` builds it with `plugins` and none of N14 applies (every D9 screen compares as
before). In the release run (`normalize/release.rs`):

| Rule | Normalization | Boundary |
|---|---|---|
| N14a | A case whose path resolves to a `bankfeeds:` or `assistant:` route is not compared: the release app must refuse it (GET render or SubmitForm POST answers `NotFound`). Reported as `HIDDEN <case>` and counted on the `HIDDEN —` line. | Any rendered page (even a 404 page) or another refusal is a `DIFF`. |
| N14b | **Django side only**, inside `<nav class="appnav">`: the exact "Bank feeds" and "AI assistant" entries (their href, icon and label markup) are dropped. | Another label, address or the same link outside the nav stays. |
| N14c | **Django side only**: the bill page's `assistant.css` link plus `<section class="ai-receipt" id="read-with-ai">` headed "Read a receipt with AI", and the new-bill page's `<section … id="start-from-receipt">` headed "Start a bill from a receipt", are dropped. | Other headings stay. |
| N14d | **Django side only**, after N13a: the exact single-footer suffix loses ` · Bank feeds are read-only`. | A second footer, other wording or trailing markup stays. |

The Rust side is never rewritten, so a release page that still shows any of these differs. Must-DIFF unit
tests: `normalize/release.rs` (each rule and `a_hidden_screen_that_renders_is_a_difference`) and
`normalize/desktop.rs` `a_release_shell_page_footer_matches_after_n13_and_n14`.


## KEYS-R1 deliberate differences

On the uniquely identified Backups page only, remove the exact legacy
`Set up recovery and portable backups` link and map the adjacent Protection /
Verified heading pair to the old Encrypted / Verified pair. No row value is
normalized. The warning, unsaved note, recovery link, changed link wording and
contexts outside that page must DIFF (normalize/backups.rs tests). Existing
folder/size/create-note/restore-flow rules are unchanged.

Setup, unlock, Book protection, Recovery, Emergency Kit and foreign-backup prompt
are desktop-only divergences with literal invariants in fb-app/tests/protection.rs
and real Electron coverage in smoke:keys. Screenshots mask only the generated
recovery key; PDF/text export tests inspect it without logging it.

`FB_SCREENDIFF_SCREENS` optionally selects a reviewed subset manifest for targeted
checks; the default full gate still uses the entire screens.json inventory.

The core-only assistant scenario has no credential I/O. Before opening its screen
fixture, the runner now supplies synthetic local envelopes for its explicitly
connected AI accounts. This preserves that fixture's connected state while real
portable restores correctly require reconnect. No rendered output is normalized
for this behavior.

## Starter chart at company creation

The desktop first-run gate and new-company form now offer Standard small-business chart
(recommended) and Start with an empty chart (for accountants). The first-run gate has no
Django equivalent. On `/companies/new/` only, `normalize/starter_chart.rs` removes the exact
default selector immediately before Save on the Rust side. The labels, option values,
default selection and placement are pinned; changed content and other routes still differ.
This applies to `company-new` and `company-new-invalid` (the old POST omits the choice).
Company editing and all existing-book screens retain their markup.

New fixtures: `w0-standard-chart`, `w0-standard-chart-page-fallback`, `w0-standard-chart-by-code`,
and `w0-empty-chart`. The frozen reference has no starter-chart service, so screens-d9
creates those accounts with its normal audited account service from the saved chart data.
Core and application tests separately prove both company-creation paths produce that chart
atomically, and opening existing books never seeds accounts. No account-row normalization
is added. Retained earnings stays computed by the report engine.

`w0-standard-home` and `w0-empty-home` also compare Home for the new-company states.
These cover the new account data without changing or normalizing Home markup.

## Document numbering settings

Company settings adds `#document-numbering`, one editable row per shared document kind plus one per designated bank. The frozen Python reference has no numbering preferences. Only that exact section is excluded from the old company-settings comparison; every old field, validation error, document screen and number still compares normally. The section is separately compared byte-for-byte after whitespace, command-token and its bank-account hidden-field normalization against `recorded/numbering-settings.html`, with `RECORDED MATCH numbering-settings` required. Recording uses `FB_RECORD_NUMBERING_SCREEN=1` explicitly. App tests and the desktop numbering smoke check invalid values, collision/stale refusals, the saved next number, subsequent document creation and audit history.
