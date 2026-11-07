"""One-off edit: the Rust screen-diff id block covers both D9 companies (feed books, AI books)."""
from pathlib import Path

path = Path(__file__).resolve().parents[1]/'crates/fb-screendiff/src/main.rs'
text = path.read_text()
old_head = '''            // D9 bank feeds: the same symbolic names as render_django.py writes for "D9 feed books".
            if let Some(d9)=tx.query_row("SELECT id FROM ledger_company WHERE name='D9 feed books'",[],|r|r.get::<_,String>(0)).optional()? {'''
new_head = '''            // D9: the same symbolic names as render_django.py writes for its two companies.
            for name in ["D9 feed books","D9 AI books"] {
            if let Some(d9)=tx.query_row("SELECT id FROM ledger_company WHERE name=?1",[name],|r|r.get::<_,String>(0)).optional()? {'''
assert text.count(old_head) == 1
text = text.replace(old_head, new_head)
for model in ['account', 'check', 'transfer', 'receipt', 'bill']:
    old = '("%s:D9 feed books",' % model
    assert text.count(old) == 1, model
    text = text.replace(old, '(&*format!("%s:{name}"),' % model)
old_loop = '''                    for row in tx.prepare(sql)?.query_map([&d9],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))? {let (key,id)=row?;ids.insert(format!("{model}:{key}"),id);}
                }'''
assert text.count(old_loop) == 1
old_tail = '''                    if let Ok(t)=chrono::DateTime::parse_from_rfc3339(&raw){let text=t.with_timezone(&chrono::Utc).format("%Y-%m-%d %H:%M").to_string();ids.insert(format!("attachment_stamp:{text}"),text);}
                }
            }'''
assert text.count(old_tail) == 1
text = text.replace(old_tail, old_tail + '\n            }')
path.write_text(text)
print('D9: Rust ids cover both D9 companies')
