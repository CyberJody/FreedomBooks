//! Trusted desktop automation, absent from renderer IPC. Runs only with write authority and never
//! in a restored backup. Audit uses the authenticated operator plus explicit automation provenance.
use crate::{App, AppResult, identity::Permission};
use fb_core::{CompanyId, payables::recurring};

pub fn desktop_tick(app: &mut App) -> AppResult<()> {
    if app.backups.viewing_restored().is_some() || app.authorize(None, Permission::Write).is_err() {
        return Ok(());
    }
    let today = (app.today)().to_string();
    let o = app.open_mut()?;
    let actor = o.owner;
    let companies:Vec<String>=o.book.read(|q| -> fb_core::FbResult<_> {
        let mut st=q.prepare("SELECT DISTINCT t.company_id FROM payables_recurring t JOIN ledger_company_members m ON m.company_id=t.company_id WHERE m.user_id=? AND t.active=1 AND t.deleted=0")?;
        Ok(st.query_map([actor.to_db()],|r|r.get(0))?.collect::<Result<_,_>>()?)
    })?;
    for c in companies {
        let Some(c) = CompanyId::parse(&c) else {
            continue;
        };
        app.authorize(Some(c), Permission::Write)?;
        let o = app.open_mut()?;
        let result = recurring::generate_due(&mut o.book, actor, c, &today)?;
        if let Some(bills) = result["created"].as_array() {
            let closed = bills.iter().filter(|b| b["posting_error"].as_str().is_some_and(fb_core::periods::is_closed_message)).count();
            if closed > 0 {
                let posted = bills.iter().filter(|b| b["state"] == "OPEN").count();
                app.messages.push(format!("Recurring bills: {posted} posted, {closed} saved as drafts due to the closing date."));
            }
        }
        for error in result["errors"]
            .as_array()
            .into_iter()
            .flatten()
            .chain(result["warnings"].as_array().into_iter().flatten())
        {
            let message = format!(
                "Recurring bill “{}”: {}",
                error["name"].as_str().unwrap_or(""),
                error["message"].as_str().unwrap_or("")
            );
            if app
                .recurring_notices
                .insert(format!("{}:{}:{}", c, error["template_id"], message))
            {
                app.messages.push(message);
            }
        }
    }
    Ok(())
}
