# Combined L3 integration

Integration inputs: sales `24fb522` and payables `efd6363`, both based on
`core-int` `efd81c1`. The `core-l3` branch combines the two modules; the lead
integrates the subsequent L2a-FIX4 work separately.

## Migration chain

`PRAGMA user_version` counts applied migrations, not the filename number.
This integration has 17 migrations, in this order:

1. `0001_ledger`
2. `0002_review_hardening`
3. `0003_callback_and_collision_hardening`
4. `0004_directory_documents_tax`
5. `0005_source_journal_hooks`
6. `0006_l2a_hardening`
7. `0010_sales`
8. `0011_sales_references`
9. `0012_sales_undeposited`
10. `0013_sales_owned_seals`
11. `0014_sales_hardening`
12. `0020_payables`
13. `0021_payables_hardening`
14. `0022_payables_source_hardening`
15. `0023_payables_parent_unique_keys`
16. `0024_payables_line_calculation`
17. `0025_payables_core_hardening`

No migration SQL was rewritten for integration. Both families of scalar functions
register before migrations and callback authorization, including the scratch
connection used for schema hashes. Open/check runs both the payables violation
query and the sales verifier. Both modules retain their guards on shared objects.

Known-prefix books upgrade through the same schema-hash gate. The sales-only
version-7 fixture upgrades through the entire chain. A new populated version-16
copy contains both modules and upgrades through the payables table rebuild to 17,
preserving all tables, UUIDs, journal/audit history, seals, reports and balances.
Its copied trigger definitions are restored before opening and validating.
The sales prefix fixture removes only empty later-module tables absent from that
historical schema and refuses to discard populated tables.

The former standalone payables branch's version 12 is a different migration
lineage, not a prefix of this chain. Those synthetic unmerged books must be
recreated; unknown schema hashes are not accepted. Prior branch reports describing
sales version 11 or payables version 12 refer to their standalone checkpoints.

## Combined gate

Run all 32 scenarios (15 core/L2a, 9 sales, 7 payables, 1 combined):

```sh
FB_HARNESS_TEMPLATE=fbv2_core_l3ref FB_HARNESS_DB=fbv2_core_l3 \
  crates/fb-harness/run_harness.sh
```

The shell environment, driver and Python runner accept `fbv2_l1_`, `fbv2_l2a_`,
`fbv2_l3s_`, `fbv2_l3p_` and `fbv2_core_` prefixes. Use a dedicated database pair
and `FB_HARNESS_VAR` for each concurrent job.

`core-l3-combined.json` posts two invoices (10.00 and 20.00), one bill (12.00),
a bank customer payment (25.00 split across both invoices), and a bill payment
(7.00) in one synthetic company. Three as-of trial balances and the complete
strict dumps are compared with the frozen Python services. Final bank is 18.00,
receivables 5.00, payables 5.00, income 30.00 and expense 12.00; trial balance
debits and credits are each 35.00.

`core_l3_tests.rs` additionally checks the populated copy upgrade, exact report
preservation, both UDF families in Reader/Writer callbacks before and after
reopen, continued authorizer refusal of transaction/PRAGMA changes, and open-time
refusal of simultaneous sales/payables corruption with unchanged schema hashes.
The comparer requires both complete dump shapes and retains both modules' typed
ID/audit normalization and uniqueness checks.

Required gates: workspace tests; clippy on fb-core, fb-db, fb-harness and fb-money
with all targets and `-D warnings`; all 32 harness scenarios MATCH with zero
failures/warnings and only the existing punctuation collation ties.
