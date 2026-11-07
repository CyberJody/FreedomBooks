# Sales review fixes at fdd2fa6

Migration `0016_sales_review_fixes` is appended **after the existing integrated
migration registry**, including 0020–0026. The names are module-owned; the applied
version is the registry count. Inserting 0016 into the old prefix would change
the schema hashes for existing integrated books. Earlier migration bytes remain
unchanged.

## Receipt destinations

Both RCP and SR insertions require a currently designated bank or the configured
undeposited account, with the same company, kind and posting flags as the service.
These are creation-time checks. Reopen and reversal validate frozen evidence;
they do not reapply today's mutable setup to old receipts.

The independent reviewer regression and `review_hardening` test both destinations
for both kinds, immediate SQL refusal, unchanged books on refusal, successful
recording, setup changes, reopening and subsequent reversal.

## Snapshot boundaries

Pure registered Rust validators require complete party/company/item/account/tax
snapshot shapes and consistent snapshot IDs. SQL guards also check JSON object
boundaries and relational company identities at insertion. Sealing and open-time
scans recheck the same evidence. Stored JSON accesses in the sales service return
validation errors, and harness dump errors produce a structurally invalid dump
that the comparer refuses.

Tests include the reviewer's null-account-snapshot reproducer, partial and scalar
objects, mismatched IDs, wrong item rate types, seal checks with insertion checks
deliberately bypassed in dev-only fixtures, and check/reopen after external data
corruption with the exact guards restored. Public save is checked for controlled
failure rather than unwinding.

## Retained line origins

New draft lines store nullable `origin_position` and `origin_revision_id` in their
immutable JSON. A non-null origin must resolve to a line of the immediately prior
revision of this same invoice. Origins cannot repeat within a draft revision.
Archived items and inactive income accounts use this provenance for retention,
independently of the new position. Posting and voiding copy the complete line
evidence unchanged; open-time checks verify those copies too.

Legacy lines without origin fields remain valid. New retained lines acquire
explicit provenance through the service. The public differential dump omits the
two internal fields. The copied reviewer scenarios exercise archived-item and
inactive-account reorders against Python; direct SQL tests cover absent, forged
and duplicate origins.

## Strict comparison

The balance invoice IDs must equal the invoice refs. Every sales monetary field,
including nested item prices and tax shares, must use canonical nonnegative
two-place ASCII syntax within 0.00–99999999999999.99. Percentages retain their
separate representation. Identical malformed dumps fail, with explicit tests
for missing/unknown balances, malformed syntax, negative and overflowing values,
and inclusive valid bounds.

## Upgrade and remaining comparer notes

The populated sales upgrade test runs from both `0010_sales` and the exact
`0026_payables_core_adoption` registry prefix. It uses legacy line JSON without
origin fields, compares all old rows and keys, verifies balances and reopens.

Payables source was inspected and left unchanged. Its comparer already requires
balance keys to equal bill IDs. It validates monetary report fields as strings,
without decimal syntax or bounds, and stored amounts as signed integers without
domain bounds. Read-only identical-corruption probes confirmed MATCH for invalid
allocated/tracker/calendar strings, overflowing outstanding and a negative stored
revision total; missing balances correctly failed.

Four general rules were added to `fb-db/GUARD-PATTERNS.md` under “Module review
lessons”. Full command evidence is in the job's `runs/2026-10-30/l3s-fix1-*.log`
files. This branch stops before merge.
