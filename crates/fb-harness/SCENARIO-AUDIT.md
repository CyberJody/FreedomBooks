# Main scenario outcome audit

Base: `05945e5`. Every step is pinned to success or a named refusal. Counts below were checked
independently against both Python and Rust outputs, including the scenario manifest and any message pins.
The JSON audit additionally inventories every refused operation, kind, message and count.

Changes: the 77 accidental empty-message `DoesNotExist` refusals in the prepayment random corpus
were removed by regenerating 50 randomized complete lifecycles (500 commands plus 13 setup steps).
Each has a successful application, unapply, refund, refund void and deposit void, plus a genuine
blocked deposit void. Generation executes the frozen Python services and changes live pools only
after success; an unintended outcome aborts generation instead of being learned as an expectation.

The combined fixture already used `sales_void_sale` at steps 75/97/98 on this base. Its generator
was stale (53 steps); it now regenerates all 108 corrected steps, including deposit/reconciliation
blocks, backdate refusals and successful releases. No financial-core behavior changed.

The two retained `DoesNotExist` cases are deliberate, name existing records, have nonempty messages
and carry explicit `intent` notes: `edge-multi-company` step 37 posts the existing c2 journal via c1;
`sales-cash-sales` step 15 sends an existing cash sale to the customer-payment reversal API.
Neither is an unsuccessful creation followed by a dependent operation. No other missing-reference
or empty-message refusal remains. No real Python/Rust mismatch was found.

| Scenario | Steps | Success | Intended refusal | DoesNotExist (intentional) | Empty message | Audit |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| banking-charges.json | 20 | 20 | 0 | 0 | 0 | PASS |
| banking-clearing-locks.json | 27 | 24 | 3 | 0 | 0 | PASS |
| banking-credit-card.json | 27 | 26 | 1 | 0 | 0 | PASS |
| banking-deposits.json | 24 | 21 | 3 | 0 | 0 | PASS |
| banking-filter-amount-array.json | 17 | 17 | 0 | 0 | 0 | PASS |
| banking-filter-amount-object.json | 17 | 17 | 0 | 0 | 0 | PASS |
| banking-filter-number-nul.json | 17 | 16 | 1 | 0 | 0 | PASS |
| banking-filter-payee-nul.json | 17 | 16 | 1 | 0 | 0 | PASS |
| banking-filter-start-array.json | 17 | 17 | 0 | 0 | 0 | PASS |
| banking-filter-start-object.json | 17 | 17 | 0 | 0 | 0 | PASS |
| banking-later-lines.json | 25 | 25 | 0 | 0 | 0 | PASS |
| banking-payable-register.json | 24 | 22 | 2 | 0 | 0 | PASS |
| banking-payments-original-funds.json | 27 | 25 | 2 | 0 | 0 | PASS |
| banking-random-500.json | 544 | 514 | 30 | 0 | 0 | PASS |
| banking-reclassified-designation.json | 18 | 18 | 0 | 0 | 0 | PASS |
| banking-reconcile.json | 35 | 33 | 2 | 0 | 0 | PASS |
| banking-review-register-filters.json | 41 | 38 | 3 | 0 | 0 | PASS |
| banking-review-reversal-number.json | 28 | 25 | 3 | 0 | 0 | PASS |
| banking-review-screen-large.json | 20 | 20 | 0 | 0 | 0 | PASS |
| checks-edit-refusals.json | 27 | 14 | 13 | 0 | 0 | PASS |
| checks-lifecycle.json | 19 | 18 | 1 | 0 | 0 | PASS |
| checks-numbering.json | 19 | 18 | 1 | 0 | 0 | PASS |
| checks-payables-collisions.json | 22 | 20 | 2 | 0 | 0 | PASS |
| checks-random-500.json | 509 | 447 | 62 | 0 | 0 | PASS |
| core-l3-combined.json | 26 | 26 | 0 | 0 | 0 | PASS |
| core-l3-hardening.json | 41 | 29 | 12 | 0 | 0 | PASS |
| credits-catalog-review-oddity.json | 18 | 18 | 0 | 0 | 0 | PASS |
| credits-lifecycle.json | 38 | 35 | 3 | 0 | 0 | PASS |
| credits-maximum-text.json | 18 | 18 | 0 | 0 | 0 | PASS |
| credits-multi-invoice.json | 28 | 28 | 0 | 0 | 0 | PASS |
| credits-number-text.json | 32 | 23 | 9 | 0 | 0 | PASS |
| credits-random-500.json | 521 | 300 | 221 | 0 | 0 | PASS |
| credits-receipt-settlement.json | 29 | 27 | 2 | 0 | 0 | PASS |
| credits-refusals.json | 33 | 20 | 13 | 0 | 0 | PASS |
| credits-retry-aliases.json | 25 | 25 | 0 | 0 | 0 | PASS |
| credits-review-invalid-dates.json | 21 | 15 | 6 | 0 | 0 | PASS |
| credits-review-validation-wording.json | 23 | 19 | 4 | 0 | 0 | PASS |
| credits-reviews-original-ar.json | 36 | 33 | 3 | 0 | 0 | PASS |
| credits-tax-group-retry.json | 23 | 23 | 0 | 0 | 0 | PASS |
| edge-contra.json | 42 | 31 | 11 | 0 | 0 | PASS |
| edge-entry-numbers.json | 59 | 43 | 16 | 0 | 0 | PASS |
| edge-half-cent.json | 50 | 37 | 13 | 0 | 0 | PASS |
| edge-multi-company.json | 60 | 46 | 14 | 1 | 0 | PASS |
| edge-reversal.json | 29 | 21 | 8 | 0 | 0 | PASS |
| edge-void.json | 18 | 18 | 0 | 0 | 0 | PASS |
| int-l3-combined.json | 108 | 87 | 21 | 0 | 0 | PASS |
| int-l3-unused-designated-reclassification.json | 4 | 4 | 0 | 0 | 0 | PASS |
| l2a-collation.json | 68 | 58 | 10 | 0 | 0 | PASS |
| l2a-directory.json | 30 | 16 | 14 | 0 | 0 | PASS |
| l2a-numbering.json | 43 | 39 | 4 | 0 | 0 | PASS |
| l2a-random-500.json | 513 | 513 | 0 | 0 | 0 | PASS |
| l2a-rate-edit-oddity.json | 15 | 14 | 1 | 0 | 0 | PASS |
| l2a-source-lifecycle.json | 19 | 15 | 4 | 0 | 0 | PASS |
| l2a-tax-rounding.json | 27 | 21 | 6 | 0 | 0 | PASS |
| payables-boundaries.json | 66 | 43 | 23 | 0 | 0 | PASS |
| payables-edit-refusals.json | 24 | 15 | 9 | 0 | 0 | PASS |
| payables-fifty-allocations.json | 110 | 110 | 0 | 0 | 0 | PASS |
| payables-filtered-tracker.json | 34 | 34 | 0 | 0 | 0 | PASS |
| payables-lifecycle.json | 21 | 19 | 2 | 0 | 0 | PASS |
| payables-payments.json | 23 | 21 | 2 | 0 | 0 | PASS |
| payables-random-500.json | 508 | 203 | 305 | 0 | 0 | PASS |
| payables-receipt-core.json | 14 | 14 | 0 | 0 | 0 | PASS |
| prepayments-1200-less-500.json | 21 | 21 | 0 | 0 | 0 | PASS |
| prepayments-date-parity.json | 67 | 39 | 28 | 0 | 0 | PASS |
| prepayments-lifecycle-retries.json | 30 | 28 | 2 | 0 | 0 | PASS |
| prepayments-random-500.json | 513 | 463 | 50 | 0 | 0 | PASS |
| prepayments-refund-number-release.json | 18 | 17 | 1 | 0 | 0 | PASS |
| prepayments-refusals.json | 35 | 19 | 16 | 0 | 0 | PASS |
| prepayments-scope-dates.json | 19 | 15 | 4 | 0 | 0 | PASS |
| prepayments-stale-and-frozen-accounts.json | 24 | 22 | 2 | 0 | 0 | PASS |
| random-1000.json | 2366 | 2277 | 89 | 0 | 0 | PASS |
| sales-cash-sales.json | 21 | 20 | 1 | 1 | 0 | PASS |
| sales-catalog-review-oddity.json | 15 | 15 | 0 | 0 | 0 | PASS |
| sales-edits-reviews.json | 21 | 19 | 2 | 0 | 0 | PASS |
| sales-invoice-payments.json | 29 | 27 | 2 | 0 | 0 | PASS |
| sales-maximum-text.json | 17 | 17 | 0 | 0 | 0 | PASS |
| sales-original-ar-split.json | 22 | 22 | 0 | 0 | 0 | PASS |
| sales-random-500.json | 512 | 424 | 88 | 0 | 0 | PASS |
| sales-refusals.json | 27 | 15 | 12 | 0 | 0 | PASS |
| sales-review-reorder-archived-item.json | 15 | 15 | 0 | 0 | 0 | PASS |
| sales-review-reorder-inactive-account.json | 15 | 15 | 0 | 0 | 0 | PASS |
| sales-tax-group-retry.json | 20 | 20 | 0 | 0 | 0 | PASS |
| sample-company.json | 96 | 96 | 0 | 0 | 0 | PASS |
| transfers-lifecycle.json | 22 | 16 | 6 | 0 | 0 | PASS |

84 scenarios; totals: {'steps': 8280, 'successes': 7111, 'refusals': 1169, 'missing': 2, 'empty': 0}; 0 audit problems.
