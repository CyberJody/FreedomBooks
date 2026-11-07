# Banking translation

Frozen reference: `wt/reference/banking/`, based on `fdd2fa6`.
Migrations: `0050_banking`, `0051_banking_integrity`, `0052_banking_completion`,
`0053_banking_actors`.
All monetary accumulation and reconciliation equations run in checked Rust cents;
SQL only selects, groups, compares and invokes pure registered predicates.

The port includes whole-receipt deposits for both customer payments and sales
receipts, original Undeposited Funds account grouping, review binding and exact
retries, reversal/release history, the bank/card register, cleared status and
reversal locks, manual reconciliation, statement modification and ticks,
service charges/interest, completion at exactly zero difference, report grouping
and Undo last. The equation `0.00 + 6,200.27 - 266.85 = 5,933.42` is covered by both
Rust and differential tests, with a one-cent refusal alongside it.

Writes own a `Book::write`/`Writer::atomic` boundary. The composable clearing
writer also owns its audit inside an atomic; caught nested errors cannot commit
partial state. Nine protected tables have permanent primary keys, all-key
replacement guards, immediate parent checks, retention guards and WITHOUT ROWID.
Deposit and reconciliation completion use deferred seal obligations. The root
transition precedes its status marks internally; the seal proves every mark
before commit. This replaces PostgreSQL's transaction-local reconciliation flag.
Schema definitions and relationship checks run on open. No UI/server/plugin or
provider integration is included.

Completion and undo also retain immutable user-ID references in a protected
actor table. Usernames remain editable and read through those references. The
populated upgrade recovers actor IDs from the original append-only audit before
falling back to the legacy name; a rename test proves identity survives upgrade
and a later undo, including a statement with no bank lines.

Python behaviors retained:

- Deposit retries return the original row even after reversal. A released receipt
  cannot be redeposited or reversed before its latest release date.
- Deposits use each receipt's original destination, including after setup changes;
  they never post income or receivables a second time.
- Cleared and reconciled journals cannot be reversed. Undo returns R to C and
  retains both membership evidence and posted service/interest entries.
- Beginning balance comes from the last DONE reconciliation by creation identity.
  Hiding later lines controls which cleared items completion counts.
- Exact clearing retries ignore a stale revision when the requested state is
  already present. Tick no-ops likewise do not advance the reconciliation.
- A repeated Modify request key returns the current reconciliation, following
  Python's audit-key retry behavior. Option-only tick changes have no separate
  reconciliation audit event in Python; that oddity is retained.
- Bank balances read on the debit side; card balances read on the credit side.
  Filters hide register rows without changing balances. Deposit party labels are
  the single customer or “N receipts”; payee search checks the member customers.

Safety differences required by the Rust guard contract:

- Missing parents, replacement victims, hidden rowids and fresh-key rekey/reinsert
  cannot exploit deferred FKs. The matrix covers both recursive-trigger settings
  and INSTEAD OF view propagation.
- A raw reconciliation cannot omit cleared items whose net is zero. The failing
  regression first demonstrated that omission; 0052 now checks the complete
  counted set, beyond Python's SQL totals-only check.
- Checked revision/amount arithmetic returns errors on exhaustion. Malformed JSON
  booleans and floating-point money are not coerced into revisions or amounts.
  Exact Python decimal text, including Unicode digits/underscores/exponents, is
  parsed without rounding away fractional cents.

The harness adds ten `banking-*` scenarios, including 500 seeded random commands
plus setup/finalization, full audit payloads, banking row evidence, register
results, reconciliation screens and reports. Its strict validator rejects
duplicate rows, malformed identical dumps and missing/extra requested reports.
Generated account IDs in `undeposited:<id>` role names map to natural references;
only those role groups are canonically sorted, preserving source creation order.

Regenerate scenarios from the worktree:

```
python3 crates/fb-harness/py/gen_banking_scenarios.py
python3 crates/fb-harness/py/extend_banking_scenarios.py
```

Run the isolated differential gate:

```
FB_HARNESS_TEMPLATE=fbv2_l3b_banking_ref \
FB_HARNESS_DB=fbv2_l3b_banking_test \
FB_HARNESS_VAR="$PWD/target/banking-var" \
crates/fb-harness/run_harness.sh
```

See `REQUESTS.md` for the untouched payables fixture blocker and shared service
hook requests, plus the L2 open-check mismatch on an unused reclassified
designated account. The lead must integrate those, rerun the unfiltered workspace
suite, independently review this branch and merge it. This worker stops before
merge. Preview URL and screenshots are not applicable to this core-only job.
