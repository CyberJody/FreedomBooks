"""Inventory the genuine 98100e0 schema, then append the table rebuild."""
from pathlib import Path
import re, sqlite3, json, hashlib
root=Path(__file__).resolve().parents[1]
lib=root/'crates/fb-db/src/lib.rs'
source=lib.read_text()
names=re.findall(r'\("([0-9]+_[^"]+)", include_str!\("../sql/[^\"]+"\)\)', source)
assert names[-1]=='0137_bankfeeds_reconnect'
conn=sqlite3.connect(':memory:')
conn.create_collation('fb_text', lambda a,b: (a>b)-(a<b))
sqls=[(name,(root/'crates/fb-db/sql'/f'{name}.sql').read_text()) for name in names]
for fun in set(re.findall(r'\b(fb_[a-z_0-9]+)\s*\(', '\n'.join(sql for _,sql in sqls))):
    conn.create_function(fun,-1,lambda *args: 1,deterministic=True)
for name,sql in sqls:
    conn.executescript(sql)
table='prepayments_customerdeposit'
triggers=conn.execute("SELECT name,sql FROM sqlite_master WHERE type='trigger' AND tbl_name=? ORDER BY name",(table,)).fetchall()
assert len(triggers)==6, triggers
definition=conn.execute("SELECT sql FROM sqlite_master WHERE type='table' AND name=?",(table,)).fetchone()[0]
assert 'estimate_id TEXT CHECK(estimate_id IS NULL)' in definition
new=definition.replace('estimate_id TEXT CHECK(estimate_id IS NULL)', 'estimate_id TEXT REFERENCES sales_estimate(id)')
sql='''-- SEC-FIX1: estimate-linked deposits; preserve the full 98100e0 trigger inventory.
-- No shipped migration is edited. Legacy views/triggers keep referring to the final table name.
PRAGMA defer_foreign_keys=ON;
CREATE TEMP TABLE secfix1_deposit_copy AS SELECT * FROM prepayments_customerdeposit;
DROP TABLE prepayments_customerdeposit;
'''+new+';\nINSERT INTO prepayments_customerdeposit SELECT * FROM secfix1_deposit_copy;\nDROP TABLE secfix1_deposit_copy;\n'
sql+='\n'.join(code+';' for _,code in triggers)
sql+='''
CREATE TRIGGER pp_customerdeposit_estimate_scope BEFORE INSERT ON prepayments_customerdeposit
WHEN NEW.estimate_id IS NOT NULL
BEGIN
 SELECT RAISE(ABORT,'customer_deposit_estimate_scope: Choose an estimate from this company for this customer.')
 WHERE NOT EXISTS(SELECT 1 FROM sales_estimate e
 JOIN sales_estimaterevision v ON v.estimate_id=e.id AND v.number=e.revision
 JOIN sales_receipt r ON r.id=NEW.receipt_id
 WHERE e.id=NEW.estimate_id AND e.company_id=NEW.company_id
 AND v.customer_id=r.customer_id AND v.state NOT IN('DECLINED','CANCELLED'));
END;
'''
(root/'crates/fb-db/sql/0140_deposit_estimates.sql').write_text(sql)
(root/'crates/fb-db/tests/secfix1_98100e0_triggers.json').write_text(json.dumps(dict(triggers),indent=2)+'\n')
(root/'crates/fb-db/tests/secfix1_98100e0_migrations.json').write_text(json.dumps([{'name':name,'sha256':hashlib.sha256(sql.encode()).hexdigest()} for name,sql in sqls],indent=2)+'\n')
source=source.replace('    ("0137_bankfeeds_reconnect", include_str!("../sql/0137_bankfeeds_reconnect.sql")),', '    ("0137_bankfeeds_reconnect", include_str!("../sql/0137_bankfeeds_reconnect.sql")),\n    ("0140_deposit_estimates", include_str!("../sql/0140_deposit_estimates.sql")),')
source=source.replace('pub const REQUIRED_TRIGGERS: &[&str] = &[', 'pub const REQUIRED_TRIGGERS: &[&str] = &[\n    "pp_customerdeposit_estimate_scope",')
lib.write_text(source)
p=root/'crates/fb-db/src/prepayments_integrity.sql'
t=p.read_text()
new_integrity='''UNION ALL SELECT 'deposit estimate scope '||d.id FROM prepayments_customerdeposit d
 WHERE d.estimate_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM sales_estimate e WHERE e.id=d.estimate_id AND e.company_id=d.company_id)
'''
t=t.replace('UNION ALL SELECT problem FROM prepayments_evidence_violations',new_integrity+'UNION ALL SELECT problem FROM prepayments_evidence_violations')
p.write_text(t)
print('0140 restores',len(triggers),'existing triggers and adds one estimate scope trigger; base migrations',len(names))
