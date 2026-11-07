"""Update exact upgrade inventories for the explicitly required 0140 schema change."""
from pathlib import Path
root=Path(__file__).resolve().parents[1]
path=root/'crates/fb-db/src/desktop_upgrade_tests.rs'
text=path.read_text()
old='''        "0136_bankfeeds_identity", "0137_bankfeeds_reconnect",
'''
assert text.count(old)==1
text=text.replace(old,old+'        "0140_deposit_estimates",\n')
old='''    let prior_schema=schema_hashes(&conn).unwrap();
'''
assert text.count(old)==1
text=text.replace(old,old+'''    let prior_deposit_sql:String=conn.query_row("SELECT sql FROM sqlite_master WHERE type='table' AND name='prepayments_customerdeposit'",[],|r|r.get(0)).unwrap();
''')
old='''    let head_schema=schema_hashes(&upgraded.conn).unwrap();
'''
assert text.count(old)==1
text=text.replace(old,old+'''    // 0140 changes exactly the formerly NULL-only estimate column into an FK.
    // Keep every other table/trigger hash and every prior row checked below.
    let deposit_sql:String=upgraded.conn.query_row("SELECT sql FROM sqlite_master WHERE type='table' AND name='prepayments_customerdeposit'",[],|r|r.get(0)).unwrap();
    assert!(prior_deposit_sql.contains("estimate_id TEXT CHECK(estimate_id IS NULL)"));
    assert_eq!(deposit_sql,prior_deposit_sql.replace("estimate_id TEXT CHECK(estimate_id IS NULL)","estimate_id TEXT REFERENCES sales_estimate(id)"));
''')
old='''if !["view:fb_json_invalid","view:fb_value_invalid"].contains(&object.as_str())'''
new='''if !["view:fb_json_invalid","view:fb_value_invalid","table:prepayments_customerdeposit"].contains(&object.as_str())'''
assert text.count(old)==1
path.write_text(text.replace(old,new))

path=root/'crates/fb-db/src/reviewed_inventory_tests.rs'
text=path.read_text()
for old,new in [('assert_eq!(installed.len(), 925);','assert_eq!(installed.len(), 926);'),('assert_eq!(required.len(), 564);','assert_eq!(required.len(), 565);'),('count(), 757);','count(), 758);')]:
    assert text.count(old)==1
    text=text.replace(old,new)
old='''    assert!(required.contains("fb_sales_receipt_snapshot"));'''
assert text.count(old)==1
text=text.replace(old,'''    // 0140 retains all reviewed triggers and adds exactly this required guard.
    assert!(installed.contains("pp_customerdeposit_estimate_scope"));
    assert!(required.contains("pp_customerdeposit_estimate_scope"));
'''+old)
path.write_text(text)
