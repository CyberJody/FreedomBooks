"""Populate the new optional FK in the existing exhaustive parent-retention test."""
from pathlib import Path

path=Path(__file__).resolve().parents[1]/'crates/fb-core/src/prepayments/guard_tests.rs'
text=path.read_text()
old='''fn fix1_every_foreign_parent_has_immediate_retention_and_permanent_keys() {
    let mut f = Fixture::new();
    fix1_history(&mut f);
    let q = f.book.connection();'''
new='''fn fix1_every_foreign_parent_has_immediate_retention_and_permanent_keys() {
    let mut f = Fixture::new();
    fix1_history(&mut f);
    // 0140 adds an optional estimate FK. Populate it so the same exhaustive
    // retention/re-key attacks exercise this parent alongside every older FK.
    let estimate = sales::save_estimate(&mut f.book, f.actor, f.c, &json!({
        "request_key": new_id(), "customer_id": f.customer, "date": "2026-10-01",
        "tax_mode": "NONE", "lines": [{"description": "Work", "quantity": "1", "rate": "100", "taxable": false}]
    })).unwrap();
    let mut input = f.input();
    input["estimate_id"] = estimate["id"].clone();
    let review = review_deposit(&mut f.book, f.actor, f.c, &input).unwrap();
    record_deposit(&mut f.book, f.actor, f.c, Some(&review), &input).unwrap();
    let q = f.book.connection();'''
assert text.count(old)==1
text=text.replace(old,new)
old='''&format!("SELECT {column} FROM {table} LIMIT 1")'''
new='''&format!("SELECT {column} FROM {table} WHERE {column} IS NOT NULL LIMIT 1")'''
assert text.count(old)==1
path.write_text(text.replace(old,new))
