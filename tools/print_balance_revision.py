"""Read the live root revision alongside the existing balance query; no core behavior changes."""
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
p=ROOT/'crates/fb-app/src/screens/printing.rs';text=p.read_text()
old='''            if live["number"] == revision_number && live["state"] == "OPEN" {'''
new='''            let live_revision = scoped_rows(app,c,"SELECT revision FROM sales_invoice WHERE company_id=? AND id=?",&[&c.to_db(),&s(&root,"id")])?
                .pop().ok_or_else(||AppError::NotFound("Invoice".into()))?["revision"].as_i64().unwrap();
            if live_revision == revision_number && live["state"] == "OPEN" {'''
assert text.count(old)==1;text=text.replace(old,new)
old='''doc["revision_total"] = json!(live["number"].as_i64().unwrap_or(revision_number).max(revision_number));'''
assert text.count(old)==1;text=text.replace(old,'''doc["revision_total"] = json!(live_revision.max(revision_number));''');p.write_text(text)
print('PRINT: live invoice balance revision guard reads the root revision explicitly')
