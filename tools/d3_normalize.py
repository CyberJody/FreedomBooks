"""Context-bounded D3 screen identities, with no global substitutions."""
from pathlib import Path
import json

def edit(name,old,new):
    p=Path(name);s=p.read_text();assert old in s,(name,old);p.write_text(s.replace(old,new))

p=Path('crates/fb-screendiff/screens.json');s=json.loads(p.read_text())
for name,row in s.items():
    if name.startswith('tax-list'):row['path']=row['path'].replace('/sales-tax/items/','/sales-tax/rates/')
    if name.startswith('tax-rate-status'):row['path']=row['path'].replace('/sales-tax/items/','/sales-tax/')
    row['path']=row['path'].replace('/sales-tax/first-enable/','/sales-tax/turn-on/')
p.write_text(json.dumps(s,indent=2)+'\n')
edit('crates/fb-screendiff/src/normalize.rs','mod literal;','mod literal;\nmod d3;')
edit('crates/fb-screendiff/src/normalize.rs','        "receipt_id" => "receipt",','        "receipt_id" => "receipt",\n        "item_id" => "taxitem",')
edit('crates/fb-screendiff/src/normalize.rs','        "customer" => "customer",','        "customer" => "customer",\n        "agency" => "vendor",\n        "default_item" => "taxitem",')
edit('crates/fb-screendiff/src/normalize.rs','"income_account" | "account"','"expense_account" | "default_expense_account" | "income_account" | "account"')
edit('crates/fb-screendiff/src/normalize.rs','pub fn normalize(html: &str, ids: &IdMap) -> String {','pub fn normalize(html: &str, ids: &IdMap) -> String {\n    let html=d3::normalize(html,ids);\n    let html=html.as_str();')
edit('crates/fb-screendiff/src/normalize.rs','        "customer-tax" => "customer",','        "customer-tax" => "customer",\n        "tax-rates" => "taxitem",')
# Do not let the proposal normalizer absorb bulk hashes; each is checked against its own source.
edit('crates/fb-screendiff/src/main.rs','if model=="customer" {rows.sort_by','if model=="customer" || model=="vendor" {rows.sort_by')
edit('crates/fb-screendiff/src/main.rs','if model=="customer"&&counts[&k]>1','if (model=="customer"||model=="vendor")&&counts[&k]>1')
edit('crates/fb-screendiff/src/main.rs','if model=="customer" {ids.insert','if model=="customer" || model=="vendor" {ids.insert')
