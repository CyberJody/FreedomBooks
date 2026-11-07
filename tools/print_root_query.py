"""Use the established sales queries for roots, whose revisioned storage has no JSON data column."""
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
p=ROOT/'crates/fb-app/src/screens/printing.rs';text=p.read_text()
old='''        let root = raw(
            app,
            c,
            &format!("SELECT data FROM sales_{model} WHERE company_id=? AND id=?"),
            &[&c.to_db(), &id],
        )?;'''
new='''        let root = {
            let open = app.open_mut()?;
            let actor = open.owner;
            open.book.read(|tx| {
                fb_core::services::access(tx,actor,c)?;
                if estimate { fb_core::sales::estimate(tx,c,id) } else { fb_core::sales::invoice(tx,c,id) }
            })?
        };'''
assert text.count(old)==1,(old,text[text.find('let id = &r.args[key]'):text.find('let id = &r.args[key]')+300])
p.write_text(text.replace(old,new))
print('PRINT: document roots use existing validated core queries')
