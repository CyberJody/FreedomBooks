# Credits review fixes

Migration `0061_credits_review_fixes` appends to the existing registry. Earlier
migration bytes remain unchanged. This branch stops before merge.

1. New memo SQL requires the configured receivables and collected-tax roles,
   following the sales `0016` creation-time eligibility pattern. Reopening and
   voiding validate frozen mappings without applying today's configuration.
   Tests cover wrong receivables, wrong collected tax, unchanged books after
   refusal, later setup changes, reopening and voiding historical documents.
2. The pure `fb_credits_fields` callback validates the complete nested
   customer/company/account/item/tax snapshots and their owning IDs. Existing
   insert guards call it; a new seal guard and open-time scan repeat it. Core
   stored reads return validation errors. Harness JSON parsing returns an invalid
   dump on error, and normalization leaves malformed objects visible without
   panicking. Tests include the reviewer regressions, nested field mutations,
   seal checks, and external corruption with the exact schema restored.
3. Credits checks the exact layouts accepted by Python `date.fromisoformat`,
   then reuses `py::model_date` for conversion. This deliberately excludes the
   additional nonpadded/Unicode/newline DateField regex fallback. The reviewer's
   scenario includes all six malformed inputs (five formerly mismatched), and
   unit tests also cover valid calendar/basic/week layouts and date boundaries.
4. The comparer validates canonical dates, bounded money and percentages,
   enums, snapshot IDs and contiguous positions. It recalculates memo totals,
   application allocations and balances, and requires complete posted identity,
   journal and audit coverage. Audit evidence also detects omitted released
   applications that no longer affect balances. Unused identities allocated by
   the L2a numbering API remain valid. Identical-corruption tests cover both.
5. Open-time integrity detects duplicate live refund numbers, including eligible
   numeric CHECK bill payments. Voids and payment reversals release their number.
   Tests cover refund/refund and refund/payment corruption with exact guard
   restoration, check/open refusal, and subsequent release.
6. Missing receivables setup, missing collected-tax setup and an invalid refund
   bank use the frozen Python wording. The copied wording scenario now compares
   without warnings; no additional wording exception was introduced.

All eight independent reviewer regressions were copied into the source tree and
failed against `91df2f9` before production edits. An additional failing-first
comparer test reproduced omission of a released application's entire history.
Command logs and differential dumps are under
`runs/2026-10-30/l3b-credits-fix1/` in the FreedomBooks project root.

The integration replacement point for shared document-number normalization is
`credits::refunds::number_text` in `refunds.rs`. Its Unicode character predicate
is `fb_db::credit_check_number_char_allowed`; insert validation and the comparer
use that same predicate. Checks code is untouched. Existing dynamic number-holder
hooks for checks and prepayments still require verification on the combined
integration branch, as documented in `REQUESTS.md`.

The credits prior-schema fixture now identifies the named `0060_credits` registry
boundary rather than subtracting one migration. A second populated-copy test
covers `0060` to `0061`, preserving every row and key. The known payables
populated-copy schema-growth fixture remains the integration lead's fix.
