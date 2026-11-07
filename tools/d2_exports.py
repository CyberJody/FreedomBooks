"""D2 audit CSV export via the existing typed command bridge and a native Save dialog."""
from pathlib import Path


def replace(path,old,new):
    p=Path(path);t=p.read_text();assert old in t,(path,old);p.write_text(t.replace(old,new))


replace('crates/fb-app/src/commands.rs','''    SaveCompany {''','''    ExportHistory { company_id: Id, filters: Vec<(String,String)> },
    SaveCompany {''')
replace('crates/fb-app/src/commands.rs','''    Page { html: String, status: u16 },''','''    Page { html: String, status: u16 },
    Export { filename: String, contents: String },''')
replace('crates/fb-app/src/session.rs','''        let open = self.open_mut()?;
        let actor = open.owner;
        let book = &mut open.book;
        let data = match command {''','''        if let Command::ExportHistory{company_id,filters}=command {
            return screens::export_history(self,company(&company_id)?,&filters);
        }
        let open = self.open_mut()?;
        let actor = open.owner;
        let book = &mut open.book;
        let data = match command {''')
replace('crates/fb-app/src/session.rs','''            Command::SubmitForm { .. } => unreachable!("handled above"),''','''            Command::SubmitForm { .. } | Command::ExportHistory{..} => unreachable!("handled above"),''')
replace('crates/fb-app/src/screens.rs','''mod d2;''','''mod d2;
pub(crate) use d2::export_history;''')
replace('crates/fb-app/src/screens/d2.rs','''use audit::{history,journal_page};''','''use audit::{history,journal_page};
pub(crate) use audit::export_history;''')
replace('crates/fb-app/src/screens/d2/audit.rs','''    let mut rows=events(app,c)?;rows.reverse();''','''    let mut rows=events(app,c)?;rows.reverse();
    let receipt_kind=get(params,"doc").filter(|d|["receipt","sales_receipt"].contains(d)).map(|d|if d=="receipt"{"RCP"}else{"SR"});
    let receipt_targets=if let Some(kind)=receipt_kind {
        app.open_mut()?.book.read(|tx|->Result<std::collections::HashSet<String>,FbError>{
            let mut st=tx.prepare("SELECT id FROM sales_receipt WHERE company_id=?1 AND kind=?2")?;
            Ok(st.query_map(params![c.to_db(),kind],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?.into_iter().map(|id|format!("receipt:{id}")).collect())
        })?
    }else{Default::default()};''')
replace('crates/fb-app/src/screens/d2/audit.rs','''prefix.is_none_or(|p|e["target"].as_str().unwrap().starts_with(p))&&''','''prefix.is_none_or(|p|e["target"].as_str().unwrap().starts_with(p))&&
        (receipt_kind.is_none()||receipt_targets.contains(e["target"].as_str().unwrap()))&&''')
replace('crates/fb-app/src/screens/d2/audit.rs','''    let page=Page::of(&rows,25,get(params,"page"));''','''    if valid && get(params,"format")==Some("csv") {return Ok((history_csv(&rows),200));}
    let page=Page::of(&rows,25,get(params,"page"));''')
replace('electron/main/guards.cjs',"new Set(['SaveCompany',", "new Set(['ExportHistory', 'SaveCompany',")
replace('electron/main/main.cjs',"  'books.run': (payload) => {", "  'books.run': async (payload) => {")
replace('electron/main/main.cjs',"    const outcome = JSON.parse(native.run(JSON.stringify(command)));", """    const outcome = JSON.parse(native.run(JSON.stringify(command)));
    if (outcome.outcome === 'export') {
      const selected = await dialog.showSaveDialog(win, { defaultPath: outcome.filename, filters: [{ name: 'CSV', extensions: ['csv'] }] });
      if (selected.canceled || !selected.filePath) return { outcome: 'done', data: { saved: false } };
      fs.writeFileSync(selected.filePath, outcome.contents, { encoding: 'utf8', mode: 0o600 });
      return { outcome: 'done', data: { saved: true } };
    }""")
replace('electron/preload/preload.cjs',"document.addEventListener('submit',", """// CSV links retain Django's URLs; a desktop Save dialog replaces the browser download.
document.addEventListener('click', async (event) => {
  const anchor = event.target.closest && event.target.closest('a[href]');
  if (!anchor || event.defaultPrevented || event.button !== 0) return;
  const url = new URL(anchor.href, window.location.href);
  const match = /^\\/c\\/([0-9a-f-]{36})\\/history\\/$/.exec(url.pathname);
  if (url.origin !== window.location.origin || !match || url.searchParams.get('format') !== 'csv') return;
  event.preventDefault();
  try {
    await call('books.run', { type: 'ExportHistory', company_id: match[1], filters: Array.from(url.searchParams.entries()) });
  } catch (e) { window.alert(String(e.message || e)); }
}, true);

document.addEventListener('submit',""")
replace('electron/scripts/smoke.cjs',"  await page.screenshot({ path: file, fullPage: false });", "  await page.evaluate(() => window.scrollTo(0, 0));\n  await page.screenshot({ path: file, fullPage: false });")
