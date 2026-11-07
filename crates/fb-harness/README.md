# fb-harness: the frozen Python answer key

Run the same scenario through `wt/reference` (tag `python-reference-20261030`, read-only) and fb-core on a
fresh SQLCipher book. The normalized books must agree to the cent, including refused commands.

    crates/fb-harness/run_harness.sh
    FB_HARNESS_TEMPLATE=fbv2_l2a_ref FB_HARNESS_DB=fbv2_l2a_harness \
      crates/fb-harness/run_harness.sh crates/fb-harness/scenarios/l2a-*.json

`py/env.sh` isolates keys, attachments and backups outside the reference tree and disables bytecode.
The Python executable defaults to `wt/f2/.venv/bin/python`. Database names are restricted to `fbv2_l1_*`, `fbv2_int*`, the L4 module prefixes (`fbv2_l4b_*`, `fbv2_l4t_*`, `fbv2_l4a_*`, `fbv2_l4r_*`),
`fbv2_l2a_*`, `fbv2_l3s_*`, `fbv2_l3p_*` or `fbv2_core_*`. Each concurrent job
must use its own database pair. The driver recreates its migrated template when
missing, rebuilds the per-scenario database, and leaves the final scenario's database for inspection.

Failures include money, status, lines, register rows and order, trial-balance cells, exact report
company/date sets, duplicate natural keys, full audit payloads (actor, snapshotted actor name, target
and normalized detail), step identity and outcome (ok/refused and exception kind), and any order
inversion outside the punctuation-tie rule below. Error wording remains a warning. Final acceptance
requires **0 failures and 0 warnings**; accepted **collation ties** have a separate summary counter.

Every step expects success by default. Checked-in scenarios state it explicitly
with `"expect_error": false`, or use `"expect_error": true` plus `expect_kind`
for an intended refusal. Optional `expect_message` also pins exact wording on
each implementation independently. A violated expectation on either
side is a failure, including identical errors on both sides and intermediate
checkpoints. A comparison cannot pass by agreeing on an unintended refusal.
The driver passes the scenario as the comparer's third input; `compare_scenario`
rechecks each side's outcome inventory, operations and intended results against
that input even if a dump omits its `unexpected` flags. These are the same fields
and comparison entry point used on the reports branch. Missing-reference negatives
must document their deliberate `intent`; empty-message refusals are rejected by
the corpus audit. See [SCENARIO-AUDIT.md](SCENARIO-AUDIT.md).

After a complete harness run, regenerate the per-scenario outcome audit with:

    python3 crates/fb-harness/py/audit_scenarios.py target/harness audit.json audit.md

`py/gen_int_l3_scenario.py` recreates the full 108-step combined lifecycle, including
cash-sale voids at steps 75/97/98. `py/gen_prepayments_scenarios.py` executes the frozen
Python services while generating 500 seeded workflow commands: 50 complete
lifecycles with successful applications, unapplies, refunds and voids. It adds
records to its live pools only after success and aborts on an unexpected outcome.
Use a fresh migrated generation DB, separate from any running harness:

    createdb -T fbv2_core_scref fbv2_core_scgen
    FBV2_DB=fbv2_core_scgen . crates/fb-harness/py/env.sh
    "$FB_PY" crates/fb-harness/py/gen_prepayments_scenarios.py
    python3 crates/fb-harness/py/gen_int_l3_scenario.py
    dropdb fbv2_core_scgen

INT-L4: `py/gen_int_l4_scenario.py` builds `int-l4-combined.json` (the 108-step layer-3 story plus
attachments, an AI receipt saved as a bill, reminders and two backup/verify/restore cycles; 137 checkpoints
with reports on both bases). `py/gen_reports_scenarios.py` copies main's pinned module corpora and refuses
any unpinned step. Layer-4 generators that do not pin their own outcomes (`gen_assistant_scenarios.py`,
`gen_backups_scenarios.py`, `scripts/l4_attachments_oracle.py`) are followed by a harness run and
`python3 py/pin_from_outputs.py <out dir> <scenario.json ...>`, which pins frozen Python's observed outcome
(kind and exact message) only where both runtimes agree and no existing pin contradicts it.
`"$FB_PY" py/check_unicode_categories.py` re-verifies the shared C*/Zl/Zp table against the frozen
interpreter. The driver now rebuilds its template on every run (`FB_HARNESS_REUSE_TEMPLATE=1` reuses it).

Both generators accept `FB_SCENARIO_OUT` to regenerate into a separate existing
directory for comparison. Refusal contracts are authored, not inferred from the
implementation's observed results.

Review hardening (L1-FIX1): generated reversal prefixes alone have their J-number normalized; ordinary
memos and reversal reasons remain exact. Identity fields in audit snapshots are mapped to scenario refs;
only generated event IDs/timestamps are omitted. Row order is never allowed to erase duplicate rows.
`py/probe_money.py` and `py/probe_dates.py` generate the checked-in oracle vectors by calling the frozen
Python implementations directly. `py/test_check_sample_tb.py` covers Decimal cents and posted/company/
as-of filtering, optionally against real temporary PostgreSQL tables with `FBV2_SAMPLE_TEST_DB=fbv2_l1_*`.

For the L2 guard/transaction API contract, see `../fb-db/GUARD-PATTERNS.md`.
For the combined sales/payables migration chain, schema version and 32-scenario
gate, see [CORE-L3.md](CORE-L3.md).

## Scenario vocabulary

- L1: users; `create_company`, `add_member`, `edit_company`, `save_account`, `save_draft`, `post_journal`,
  `reverse_journal`; trial balances at `as_of` dates. Account/journal refs are scenario natural keys.
- L2a (`src/l2a.rs`, `py/l2a.py`): `l2_save` / `l2_active` (customer/vendor/item), `l2_rate`, `l2_group`,
  `l2_tax_active`, `l2_settings`, `l2_bulk`, `l2_identity`, `l2_setup`, `l2_snapshot`, `l2_totals`,
  `l2_post_source`, `l2_reverse_source`. Master-data service arguments go under `values`; create steps
  supply a `ref`. The adapters resolve account/contact/tax references before calling the real services.
- Python integer keys overlap between tables. Its adapter records each reference's entity type rather
  than guessing from the integer. UUIDv7 Rust row ids are normalized to those same reference names.
- `l2_totals` optionally resolves a picked rate/group, then invokes the document calculation routine.
  Its full returned result, including each frozen member share, is compared, not just its total.
- `l2a-random-500.json`: 500 seeded random directory/tax commands plus 13 fixed setup commands.
- `l2a-source-lifecycle.json`: the generic INV identity used by Python FoundationTests, not a complete
  invoice. Owning invoice/bill/check/etc. lifecycle ports must add their own completeness seals,
  late-reference guards, allocations and posting evidence; this scenario does not claim to test them.

`py/gen_scenarios.py` generates the L1 edge and 1,000-entry cases. `py/gen_l2a_scenarios.py` generates the
L2a directory, numbering, rounding, randomized, locale/case and rate-edit-oddity cases. The source
lifecycle fixture is checked in separately. `py/probe_l2a.py` executes the frozen calculation and label
functions, Django email validation and live PostgreSQL casing to generate Rust test vectors.
`py/gen_label_tables.py` pins Python 3.13's digit/titlecase label behaviour. Regenerate these only with
the named reference environment, not with a different installed Python version.

## Comparison and collation

`fb_text` uses embedded ICU4X 2.4.0 compiled data on every OS. See
[`COLLATION.md`](../fb-db/COLLATION.md) for the exact options, the six measured counterexamples,
portable uniqueness casing and Linux oracle tests. Chart, trial-balance, directory, tax-item and
snapshot-role order differences pass only when **every inverted pair** compares EQUAL under the
shared ICU4X Tertiary / Shifted punctuation-tie predicate. Key sets, duplicate detection and complete
row values are checked first; register order and group-member position remain exact. Accepted
inversions increment `collation ties`; other order changes are MISMATCH, never warnings.

## Strict audit normalization

Both adapters retain the full ordered event list with exact action, actor, actor-name, target and
detail. Generated event IDs/timestamps are omitted from the dump. Generated L1 journal display
prefix normalization is unchanged. L2a adds only these typed identifier mappings:

- Targets: customer/vendor/item, taxitem and documentidentity map their actual keys to scenario refs
  (the lexicographically first alias for retries). accountingsetup and salestaxsettings are singletons
  mapped to their company ref. A row with no scenario alias uses its entity type and creation rank;
  the mapping is built from actual rows, and unknown references remain visibly unknown.
- Directory `before`/`after`: `id`, `company_id`, `income_account`, `expense_account`,
  `default_expense_account`.
- Tax-item `before`/`after`: `id`, `agency_id`, and each `member_ids` element.
- Settings `before`/`after`: `default_item_id`. Setup snapshots: the five account roles and `banks`.
- `salestax.bulk_step`: `customers_marked_taxable` and `items_marked_taxable` elements.
- `document.accounts`: each `detail.accounts[].id` is an account ID.

Python keys overlap across tables, so mappings always carry an entity type. These are key mappings,
not substitutions in free text: names, descriptions, notes, reasons, amounts, revisions, currency,
request keys, business numbers and all other fields remain exact. Comparer mutation tests change
every audit payload leaf in a real L2a scenario and require a failure. Neither adapter nor comparer
reduces audits to action counts.

Every driver step also records per-company `audit_counts` in its outcome, even without
`capture_steps`. Successful attachment downloads and reminder queries retain their original
inputs there. These records are captured by the driver separately from the attachment adapter's
generated `queries` and `results`. The comparer binds every query selector and cutoff to its
indexed driver outcome, checks audit counts against each checkpoint's history, then rebuilds
reminder membership, totals and nav counts from audited source revisions and settlements up to
that boundary. Downloads must match the selected attachment and its audit fingerprint. Retry
aliases are normalized when dumping both the query and its driver input. Older saved dumps
without driver evidence must be regenerated; missing evidence is an error.

The Rust suite covers primary/alternate-key INSERT and UPDATE OR REPLACE (including the displaced
row), delete/reinsert, recursive triggers OFF, WITHOUT ROWID aliases, immediate parent eligibility,
versioned schema-definition hashes and full rollback after late audit failures across all 17 L2a
write paths. Member/bank link sets intentionally remain editable; immutable identities, counters,
snapshots, settings and archived directory records cannot be deleted. HTTP/form/UI assertions
belong to later presentation work; no Electron, IPC, plugins or preview server is part of this harness.

For the L1 sample-book cross-check, `py/export_sample.py` exports the seeded Python ledger and
`py/check_sample_tb.py` compares its trial balance to the Rust run. It does not translate all owning
sales/payables/banking document services; those remain subsequent layers.

## Bank feeds (`bankfeeds-*`, DB prefix `fbv2_l4f_`)

`py/bankfeeds_adapter.py` and `src/bankfeeds.rs`; scenarios from `py/gen_bankfeeds_scenarios.py`.
The provider half (token claim, `/accounts`, Account Set parsing) is the simplefin plugin's, so a
scenario hands both sides an already parsed pull (local dates, Decimal money text). Python runs the
real `connect` and `sync` with only `client.decode_setup_token`, `claim_access_url`, `fetch_accounts`
and `parse_account_set` replaced (dates become noon in the display zone so `_epoch_to_date` returns
them), or `_store` directly (`bankfeeds_store`). Rust runs `connect`, then `begin_sync` + `store_pull`,
or `record_sync_failure` with the plugin's redacted message (`fail`).

Ops: `bankfeeds_connect` / `_forget` / `_sync` / `_store` / `_map` / `_mappable` / `_rule_accounts` /
`_save_rule` / `_move_rule` / `_rule_active` / `_suggest` (also records `default_check_number`) /
`_accept` / `_ignore` / `_return` / `_items` (the review list in screen order: Python's `views.review`
queryset ordered `-posted_on, -pk`, undated rows first), plus the harness-only `bankfeeds_undo`, which voids or reverses the
document an ACCEPTED item became through its owning module (check void, transfer void, deposit
reversal, journal reversal) and refuses with the same adapter message on both sides otherwise.

Natural keys: a feed account is `<connection>/<conn_key>/<remote id>`, a transaction
`<feed>/<remote id>`; connections and rules use the step's `ref`. A pulled account or transaction with its
own `ref` (ids that differ only in URL userinfo, FIX5) is named `<connection>/~/<ref>` or `<feed>/<ref>`;
each runner records which raw ids a name stands for at the pull (provenance) and finds rows by them
(Python by the raw id it stores, v2 by the raw id's identity key), so no name carries a credential. Accepted documents are named by the
step's `ref` (else `bf:<transaction>:<return count>`) through the owning module's tracking, so their
check, transfer, deposit and journal rows dump under the same names. `reference_url` compares in v2's
route form `<route>:<document name>` (Python's Django URL is resolved to its view name).

**Derived-key pass.** Python derives `EFT-<pk>` and the uuid5 accept request keys from its integer
transaction pk; v2 uses the never-reused `ordinal`, and request-key intents embed integer versus UUID
account/vendor ids. After the whole dump is built, each side rewrites generated fields only (FIX1: the
earlier pass rewrote every string and key, so a company named `Ordinary EFT-1` matched `Ordinary EFT#T2`):

- `EFT-<n>` (`n` a known pk/ordinal) to `EFT#T<rank>`, the transaction's rank in sorted natural-key
  order (short enough to stay a valid 20-character check number), by **record provenance** (FIX3; FIX2
  went by value, so an ordinary `checks_save` writing `EFT-1` was rewritten when a bank-feed check had
  generated `EFT-1`). Each runner records a (check document, number) pair for every check revision a
  CHECK accept with a blank number creates (`default_check_number`; recorded before the error is
  raised, so a draft left by a failed accept counts too), and the result key of every suggestion
  step. Only these are rewritten: a check row's `number_key`, a check revision's `check_number`, a
  `check:<document>` audit event's `detail.check_number` of a recorded pair, and a suggestion result's
  `check_number`. Every other entry point writes literal values that stay exact even when they look
  generated: the ordinary check service (`checks_save` and friends), an accept with an explicit number,
  and the deposit, transfer and journal paths, whose numbers (`DEP-`, `TRF-`, `JE`) come from shared
  sequences and are never rewritten;
- a whole `request_key` value it can rebuild with its own formula to `bankfeed:<KIND>:<transaction>:<return
  count>:<intent>`, where the intent's ids are replaced by scenario refs. Candidates come from this
  side's own documents: every check revision (first line's account, vendor, payee, number; vendor
  checks also with an empty payee) and every journal line (account, name); deposits and transfers
  have an empty intent.

A key the core derived differently stays a raw uuid and mismatches the other side's label. Both rows
derived from an ordinal and rows derived from a pk therefore compare by natural key, which also hides
the documented Postgres sequence gaps.

**Strict comparer** (`src/bankfeeds_shape.rs`): exact field shapes; natural-key syntax; dates; the
decision shape (status / kind / journal / line / reference / decider); a decided line exists in the
named posted journal of the same company with the item's direction and amount; mappings and rule
accounts belong to the company; rule positions are unique, at most 200 per company. Completeness
against the audit trail: one `connected` event per connection, `forgotten` iff inactive, `sync_count`
and last summary/ok equal the `synced` events, each connection's row count equals the sum of `new`
minus `pending_dropped`, each mapping equals the last `mapped` event, each transaction's status and
return count replay from its accepted/ignored/returned events (the last acceptance's kind, journal,
reference, amount, date and description must agree), each rule equals its created/edited events, and
every bank-feed audit target is a dumped row unless it is a withdrawn pending item last returned to
review; `bankfeed.staged` (bridge staging) adds its `new` rows to its connection's count. All stored
counts are parsed as checked non-negative integers (a malformed count fails, never panics).

**Scenario manifest and outcomes (FIX1).** Every bank-feed scenario sets `strict_outcomes`: a step without
`expect_error` must succeed, `expect_kind` / `expect_message` pin a refusal, and a violated expectation on
either side is a failure (two identical wrong outcomes no longer MATCH). **Refusal pins (FIX3):** every
refusal in every bank-feed scenario pins both `expect_kind` and `expect_message`
(`pin_bankfeeds_refusals.py` writes `py/bankfeeds-refusals.expect.json` from a harness run; the generator
refuses an unpinned refusal; `fix3_every_bank_feed_refusal_pins_kind_and_message`). The runners copy a
step's pins into its outcome, and their `unexpected` flag now means success versus refusal only. The
comparer holds each side to its pins: the right operation refused for another reason fails on the side
that did it (an outsider's `PermissionDenied` where the scenario intends a sync failure), even when both
sides do it identically. The Python side is checked after the v2 redaction of its bank-feed text: where the
frozen `redact` keeps URL userinfo (two credential-failure steps of `bankfeeds-pulls`), the pin is the v2
message and Python's message matches it once redacted. Each runner records, in its own
outcome for every bank-feed step, a `manifest` built from the scenario step alone (company, result name,
created connection / rule, the feed accounts and transactions a pull lists); the two manifests must be
equal. Against the successful steps' manifests the comparer requires: the dumped connections, rules and
feed accounts are exactly those created/listed, every transaction was listed by some pull, the result set
is exactly the named results, and each result has its op's typed shape (account lists of the company's
refs; review lists of unique transaction keys; sync results whose counts add up to the pull's listed
accounts/transactions, skipped reasons and provider errors; suggestions whose kind follows their
matches / receipts / rule). Rule positions must equal a replay of the created/moved audit events.
`bankfeeds-random-500` pins every step's Python outcome (kind and message for refusals) from
`py/bankfeeds-random-500.expect.json`, written by `py/pin_bankfeeds_expectations.py` from a Python run of
the unpinned scenario. `tests/strict_bankfeeds.rs` corrupts both sides identically (every leaf, every
table, 28 semantic cases, the reviewer's inventory/type/priority/count cases) and requires a failure.

**Query-list members and order (FIX2).** A list result alone can be emptied identically on both sides, so
each runner also records, in the outcome of a successful `bankfeeds_mappable`, `bankfeeds_rule_accounts`
or `bankfeeds_items` step, `evidence`: the raw rows the list is drawn from, read from the tables rather
than through the service (every account of the company with code, kind, active, postable and control role;
every transaction of the company with its posted date and the rank of its pk/ordinal). The two runners'
evidence must be equal. The comparer recomputes the exact list and order from it (services' filters and
`code` order; the review screen's `-posted_on` (undated first), `-pk`), and bounds the evidence by the same
dump and the scenario manifests: listed accounts are accounts of the company and include every account a
successful `save_account` created earlier (`save_account` steps carry a `{company, created}` manifest
wherever bank-feed steps run). Listed transactions are exactly the company's inventory at that step,
replayed over time (FIX3; FIX2 only bounded them from below and exempted every row ever listed as
pending, so a single member could be dropped from a list and its evidence). The k-th successful pull
(sync/store manifest: feeds, transactions, and the `pending` ones) pairs with the company's k-th
`bankfeed.synced` event with `ok: true`. Each pull adds what it lists and withdraws the rows its event
names in `pending_dropped_ids`. Those ids are remote ids, ordered feed by feed in the pull's order and by
first insertion within a feed, so the comparer aligns them with each feed's candidates: rows last listed
as pending, of the event's connection, not listed again. A row withdrawn in every alignment is gone, in
none is present. Only a row withdrawn in some alignments, or any candidate of a pull whose ids do not
cover `pending_dropped` (the audit keeps 50), may go either way. Every bank-feed scenario replays with
no such row (`bankfeeds_shape::tests::fix3_review_list_inventory_replays_exactly_on_every_scenario`).
