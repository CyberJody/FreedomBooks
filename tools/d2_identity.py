"""Asserted D2 permanent identity, form action and short ordinal edits."""
from pathlib import Path
import json


def replace(path, old, new):
    p = Path(path)
    text = p.read_text()
    assert old in text, (path, old)
    p.write_text(text.replace(old, new))


replace('electron/main/main.cjs', "app.enableSandbox();", "app.enableSandbox();\nrequire('./identity.cjs').configure(app);")
replace('electron/main/keys.cjs', "return app.getPath('userData');", "return require('./identity.cjs').dataDir(app);")
p=Path('electron/package.json')
v=json.loads(p.read_text());v['productName']='FreedomBooks';v['build']={'appId':'app.freedombooks.FreedomBooks','productName':'FreedomBooks'}
p.write_text(json.dumps(v,indent=2)+'\n')
replace('electron/preload/preload.cjs', "path: window.location.pathname, token,", "path: new URL(form.getAttribute('action') || window.location.pathname, window.location.href).pathname, token,")
replace('crates/fb-app/src/screens.rs', 'use crate::commands::{InvoiceLineValues', 'mod d2;\nuse crate::commands::{InvoiceLineValues')
replace('crates/fb-app/src/screens.rs', '            item_choices.push((id.clone(), format!("{} · #{}", it["code"].as_str().unwrap_or_default(), id)));', '''            item_choices.push((id.clone(), format!("{} · #{}", it["code"].as_str().unwrap_or_default(), item_ordinals[&id])));''')
replace('crates/fb-app/src/screens.rs', '        let items = fb_core::directory::list(tx, c, fb_core::directory::Model::Item)?;', '''        let item_ordinals = d2::ordinals(tx, c, "directory_item")?;
        let account_ordinals = d2::ordinals(tx, c, "ledger_account")?;
        let items = fb_core::directory::list(tx, c, fb_core::directory::Model::Item)?;''')
replace('crates/fb-app/src/screens.rs', 'a.code, a.name, a.id.to_db(), if unavailable', 'a.code, a.name, account_ordinals[&a.id.to_db()], if unavailable')
replace('crates/fb-app/src/screens.rs', '        "company_home" =>', '''        "home" | "company" | "company_new" | "company_edit" | "account_new" | "account_edit" |
        "journal" | "journal_new" | "journal_edit" | "history" | "trial_balance" | "report_problem" |
        "setup_start" | "documents:setup" => d2::render(app, &route, &params, path)?,
        "company_home" =>''')
replace('crates/fb-app/src/screens.rs', '        "invoices:new" => submit_invoice(app, &route, path, fields),', '''        "invoices:new" => submit_invoice(app, &route, path, fields),
        _ if d2::is_form(&route.name) => d2::submit(app, &route, path, fields),''')
