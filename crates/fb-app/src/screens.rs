//! Screens: route a path (the same paths Django serves) to a view model and render it with fb-templates.
//! Each builder is a port of the Django view that served that path; reads go through fb-core services
//! where they exist, and through read-only screen queries here where the core has no query yet.
mod d2;
mod d3;
mod d4;
pub(crate) mod d7a;
pub(crate) mod d7b;
pub(crate) use d7b::{is_d7b_command, run_d7b_command};
pub(crate) use d7a::{is_d7a_command, run_d7a_command};
pub(crate) mod d5;
pub(crate) use d5::{is_d5_command, run_d5_command};
pub(crate) mod d6;
mod d8;
pub mod backups;
mod printing;
mod protection;
mod identity;
mod d9;
pub(crate) use d8::export_report;
pub(crate) use d6::{is_d6_command, run_d6_command};
use crate::commands::{InvoiceLineValues, InvoiceValues, Outcome};
use crate::session::{App, AppError, AppResult, save_invoice};
pub(crate) use d2::export_history;
/// SIM-FIX: the history action labels the frozen v1 table lacked (the screen diff maps them back).
pub use d2::SIM_FIX_ACTION_LABELS;
pub(crate) use d3::{is_d3_command, run_d3_command};
pub(crate) use d4::{is_d4_command, run_d4_command};
use fb_core::model::{Account, DetailType, Kind};
use fb_core::{AccountId, CompanyId, FbError, UserId};
use fb_db::Book;
use fb_templates::forms::{self, Field, Widget};
use fb_templates::nav::{self, Current};
use fb_templates::vm::*;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};

pub struct Rendered {
    /// HTML for a page; for a 3xx response, the local Location path. The bridge shape stays fixed.
    pub html: String,
    pub status: u16,
    /// The resolved route name, for the shell (window title, tests).
    pub route: String,
}

/// A path matched against the frozen app's route table.
pub struct Route {
    pub name: String,
    pub args: BTreeMap<String, String>,
}

/// Match `path` (no query string) against `urls.json`. `<int:...>` parts carry our UUID ids here.
pub fn resolve(path: &str) -> Option<Route> {
    let want: Vec<&str> = path.split('/').collect();
    for (name, pattern) in fb_templates::django::route_table() {
        let have: Vec<&str> = pattern.split('/').collect();
        if have.len() != want.len() {
            continue;
        }
        let mut args = BTreeMap::new();
        let ok = have.iter().zip(&want).all(|(p, w)| {
            if let (Some(inner), true) = (p.strip_prefix('<').and_then(|p| p.strip_suffix('>')), !w.is_empty()) {
                let (conv, arg) = inner.split_once(':').unwrap_or(("str", inner));
                // Django's `int` converter carries a row id; ours are canonical UUIDs, so only those match
                // (otherwise `/invoices/new/` would resolve as an invoice detail).
                if conv == "int"
                    && if arg == "number" {
                        !w.bytes().all(|b| b.is_ascii_digit())
                    } else {
                        fb_ids::parse_uuid(w).is_none_or(|u| u.to_string() != *w)
                    }
                {
                    return false;
                }
                args.insert(arg.to_owned(), (*w).to_owned());
                true
            } else {
                p == w
            }
        });
        if ok {
            return Some(Route { name: name.clone(), args });
        }
    }
    None
}

/// Parse `a=1&b=x%20y` (form-urlencoded) into ordered pairs.
pub fn parse_query(query: &str) -> Vec<(String, String)> {
    fn decode(s: &str) -> String {
        let bytes = s.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'+' => out.push(b' '),
                b'%' if i + 2 < bytes.len() => {
                    match u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("zz"), 16) {
                        Ok(b) => {
                            out.push(b);
                            i += 2;
                        }
                        Err(_) => out.push(b'%'),
                    }
                }
                b => out.push(b),
            }
            i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    }
    query.split('&').filter(|p| !p.is_empty()).map(|p| match p.split_once('=') {
        Some((k, v)) => (decode(k), decode(v)),
        None => (decode(p), String::new()),
    }).collect()
}

/// `urlencode` as Django's QueryDict/urllib do it (spaces as `+`).
pub fn urlencode(pairs: &[(String, String)]) -> String {
    fn enc(s: &str) -> String {
        let mut out = String::new();
        for b in s.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' => {
                    out.push(b as char)
                }
                b' ' => out.push('+'),
                b => out.push_str(&format!("%{b:02X}")),
            }
        }
        out
    }
    pairs.iter().map(|(k, v)| format!("{}={}", enc(k), enc(v))).collect::<Vec<_>>().join("&")
}

fn get<'a>(params: &'a [(String, String)], key: &str) -> Option<&'a str> {
    params.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

fn arg_company(route: &Route) -> AppResult<CompanyId> {
    route.args.get("company_id").and_then(|s| CompanyId::parse(s)).ok_or_else(|| AppError::NotFound("company".into()))
}

/// Render the screen at `target` (`/c/<id>/accounts/?sort=code`).
pub fn render(app: &mut App, target: &str) -> AppResult<Rendered> {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let params = parse_query(query);
    if identity::handles(path) { return identity::render(app, path, &[], None); }
    if app.is_open() && app.identity_location().is_some() { return Ok(Rendered { html: crate::identity::SIGN_IN.into(), status: 303, route: "sign-in".into() }); }
    app.authorize_path(target, crate::identity::Permission::Read)?;
    if ![crate::protection::RECOVERY, crate::protection::KIT].contains(&path) { app.recovery_authorized = false; }
    if protection::handles(path) { return protection::render(app, path, &[]); }
    let route = resolve(path).ok_or_else(|| AppError::NotFound(path.to_owned()))?;
    if !plugin_allowed(app, &route.name) {
        return Err(AppError::NotFound(path.to_owned()));
    }
    if let Some(company) = route.args.get("company_id").and_then(|s| CompanyId::parse(s)) {
        app.ai_select_company(company)?;
    }
    let name = route.name.clone();
    let (html, status) = match name.as_str() {
        "home" | "company" | "company_new" | "company_edit" | "account_new" | "account_edit"
        | "journal" | "journal_new" | "journal_edit" | "history" | "trial_balance"
        | "report_problem" | "setup_start" | "documents:setup" => {
            d2::render(app, &route, &params, path)?
        }
        _ if printing::is_document(&name) => printing::document(app, &route, target)?,
        _ if d4::is_route(&name) => d4::render(app, &route, &params, path)?,
        _ if d7a::is_route(&name) => d7a::render(app, &route, &params, path)?,
        _ if d7b::is_route(&name) => d7b::render(app, &route, &params, path)?,
        _ if d5::is_route(&name) => d5::render(app, &route, &params, path)?,
        _ if d6::is_route(&name) => d6::render(app, &route, &params, path)?,
        _ if d3::is_route(&name) => d3::render(app, &route, &params, path)?,
        _ if d8::is_route(&name) => d8::render(app, &route, &params, path)?,
        _ if backups::is_route(&name) => backups::render(app, &route, &params, path)?,
        _ if d9::is_route(&name) => d9::render(app, &route, &params, path)?,
        "company_home" => (home(app, &route)?, 200),
        "accounts" => (accounts(app, &route, &params)?, 200),
        "register" => register(app, &route, &params, path)?,
        "invoices:list" => invoice_list(app, &route, &params)?,

        _ => (not_ported(app, &route, path)?, 200),
    };
    let html = if status == 200 && html.contains("id=\"print-now\"") { printing::capability(app, html, target) } else { html };
    Ok(Rendered { html, status, route: name })
}

fn shell(app: &mut App, company: Option<CompanyId>, current: &Current<'_>) -> AppResult<Shell> {
    let today = (app.today)();
    // DESKTOP-BACKUPS: a restored book opened beside the main one says so on every page.
    // Built-in-key and auto-open books cannot lock, so they return to the main book by reopening the app.
    let back = if app.can_lock() { "Lock (File › Lock)" } else { "Quit and reopen FreedomBooks" };
    let mut banner = match (app.upgrade_notice.take(), app.backups.viewing_restored()) {
        (Some(notice), _) => notice,
        (None, Some(name)) => format!("Restored book “{name}” is open. {back} to return to your main book."),
        (None, None) => String::new(),
    };
    let role = app.authenticated_session()?.role;
    if role == crate::identity::Role::Readonly { banner.push_str(" Read-only access: view and print. Ask an administrator for permission to save changes."); }
    let plugins = app.plugins;
    let bankfeeds = app.bankfeeds_enabled();
    let open = app.open_mut()?;
    let (owner, staff, username) = (open.owner, open.staff, open.owner_name.clone());
    let company_vm = match company {
        Some(c) => {
            let row = open.book.read(|tx| {
                fb_core::services::access(tx, owner, c)?;
                fb_core::store::company(tx, c)
            })?;
            let row = row.ok_or_else(|| AppError::NotFound("company".into()))?;
            Some(CompanyVm { pk: c.to_db(), name: row.name, currency: row.currency })
        }
        None => None,
    };
    let overdue = match company {
        Some(c) => overdue_count(&mut open.book, owner, c, today)?,
        None => 0,
    };
    let pk = company_vm.as_ref().map(|c| c.pk.clone());
    let mut nav = nav::build(pk.as_deref(), current, staff, overdue).map_err(AppError::Render)?;
    hide_plugin_nav(&mut nav, bankfeeds, plugins);
    Ok(Shell {
        plugins,
        bankfeeds,
        company: company_vm,
        user: UserVm { username, is_authenticated: true },
        nav,
        fb_local_preview: false,
        fb_banner: banner,
    })
}

/// Routes of the plugin surfaces hidden in release builds (attachments and backups are core).
pub fn plugin_route(name: &str) -> bool {
    name.starts_with("bankfeeds:") || name.starts_with("assistant:")
}

/// Nav entries of the bank-feed and AI plugin surfaces.
pub const PLUGIN_NAV: [&str; 3] = ["bank_review", "bank_feeds", "ai_assistant"];

fn plugin_allowed(app: &App, name: &str) -> bool {
    if name.starts_with("bankfeeds:") { app.bankfeeds_enabled() }
    else if name.starts_with("assistant:") { app.plugins }
    else { true }
}

fn hide_plugin_nav(nav: &mut nav::Nav, bankfeeds: bool, ai: bool) {
    for group in &mut nav.groups {
        group.items.retain(|item| match item.key.as_str() {
            "bank_review" | "bank_feeds" => bankfeeds,
            "ai_assistant" => ai,
            _ => true,
        });
    }
    nav.groups.retain(|group| !group.items.is_empty());
    if nav.current.as_ref().is_some_and(|c| match c.key.as_str() {
        "bank_review" | "bank_feeds" => !bankfeeds,
        "ai_assistant" => !ai,
        _ => false,
    }) {
        nav.current = None;
    }
}

/// `reminders.queries.overdue_count`: overdue invoices + overdue bills for the nav badge.
fn overdue_count(book: &mut Book, actor: UserId, c: CompanyId, today: chrono::NaiveDate) -> AppResult<i64> {
    let count=fb_core::reminders::overdue_count(book,actor,c,&today.to_string())?;
    i64::try_from(count).map_err(|_|AppError::Invalid("Reminder count overflow.".into()))
}

fn money(v: &Value) -> i128 {
    v.as_str().and_then(|s| fb_core::services::money_json(&json!(s)).ok()).map(|m| m.cents()).unwrap_or(0)
}

fn cash(cents: i128) -> String {
    fb_money::Money::from_cents(cents).to_string()
}

fn open_q(r: &Value) -> bool {
    matches!(r["payment_state"].as_str(), Some("OPEN" | "PARTIAL")) && money(&r["outstanding"]) > 0
}

fn invoice_overdue(r: &Value, today: &str) -> bool {
    r["state"] == "OPEN" && open_q(r) && r["due_date"].as_str().is_some_and(|d| d < today)
}

fn account_vm(a: &Account) -> AccountVm {
    AccountVm {
        pk: a.id.to_db(),
        code: a.code.clone(),
        name: a.name.clone(),
        label: a.label(),
        get_detail_type_display: a.detail_type.label().into(),
        is_contra: a.is_contra,
        description: a.description.clone(),
        normal_balance: a.normal_balance().as_str().into(),
        section_balance: a.section_balance().as_str().into(),
        active: a.active,
        postable: a.postable,
    }
}

const HOME_BANK_LIMIT: usize = 6;

/// `ledger.views.company_home`.
fn home(app: &mut App, route: &Route) -> AppResult<String> {
    let c = arg_company(route)?;
    let shell = shell(app, Some(c), &Current { route: "company_home", kind: None })?;
    let open = app.open_mut()?;
    let banks: Vec<Account> = open.book.read(|tx| -> Result<Vec<Account>, FbError> {
        let mut st = tx.prepare(
            "SELECT a.id FROM documents_designatedbank d JOIN documents_accountingsetup s ON s.id=d.setup_id JOIN ledger_account a ON a.id=d.account_id
             WHERE s.company_id=?1 AND a.company_id=?1 ORDER BY a.code COLLATE BINARY, a.id LIMIT ?2",
        )?;
        let ids = st.query_map(fb_db::rusqlite::params![c.to_db(), (HOME_BANK_LIMIT + 1) as i64], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
        let mut out = Vec::new();
        for id in ids {
            if let Some(a) = AccountId::parse(&id).map(|a| fb_core::store::account(tx, a)).transpose()?.flatten() {
                out.push(a);
            }
        }
        Ok(out)
    })?;
    let vm = HomeVm { shell, more_banks: banks.len() > HOME_BANK_LIMIT, banks: banks.iter().take(HOME_BANK_LIMIT).map(account_vm).collect() };
    Ok(app.screens.render("ledger/company_home.html", &vm)?)
}

/// `ledger.views.accounts`: FX2 chart order (type, code, creation), or by code.
fn accounts(app: &mut App, route: &Route, params: &[(String, String)]) -> AppResult<String> {
    let c = arg_company(route)?;
    let shell = shell(app, Some(c), &Current { route: "accounts", kind: None })?;
    let by_code = get(params, "sort") == Some("code");
    let open = app.open_mut()?;
    let owner = open.owner;
    let mut rows = open.book.read(|tx| {
        fb_core::services::access(tx, owner, c)?;
        fb_core::store::accounts(tx, c)
    })?;
    if by_code {
        rows.sort_by(|a, b| a.code.cmp(&b.code).then(a.id.to_db().cmp(&b.id.to_db())));
    } else {
        rows.sort_by(|a, b| {
            a.type_order()
                .cmp(&b.type_order())
                .then(a.code.cmp(&b.code))
                .then(a.id.to_db().cmp(&b.id.to_db()))
        });
    }
    let vms: Vec<AccountVm> = rows.iter().map(account_vm).collect();
    let page = Page::of(&vms, 100, get(params, "page"));
    let vm = AccountsVm { shell, accounts: page.object_list.clone(), page, by_code, query_string: if by_code { "sort=code".into() } else { String::new() } };
    Ok(app.screens.render("ledger/accounts.html", &vm)?)
}

/// Provenance of register rows (`ledger.provenance` resolvers for the sales kinds ported so far).
fn sources(book: &mut Book, c: CompanyId) -> AppResult<HashMap<String, (Option<SourceVm>, String)>> {
    let cpk = c.to_db();
    Ok(book.read(|tx| -> Result<_, FbError> {
        let mut out = HashMap::new();
        let mut st = tx.prepare(
            "SELECT j.id, i.kind, i.number,
                    (SELECT inv.id FROM sales_invoice inv WHERE inv.identity_id=i.id),
                    (SELECT json_extract(r.data,'$.customer_snapshot.display_name') FROM sales_invoice inv JOIN sales_invoicerevision r ON r.invoice_id=inv.id AND r.number=inv.revision WHERE inv.identity_id=i.id),
                    (SELECT rc.id FROM sales_receipt rc WHERE rc.identity_id=i.id),
                    (SELECT json_extract(rc.data,'$.customer_snapshot.display_name') FROM sales_receipt rc WHERE rc.identity_id=i.id),
                    (SELECT rc.kind FROM sales_receipt rc WHERE rc.identity_id=i.id)
             FROM ledger_journal j JOIN documents_documentidentity i ON i.id=j.source_id WHERE j.company_id=?1",
        )?;
        let rows = st.query_map([&cpk], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, Option<String>>(7)?,
            ))
        })?;
        for row in rows {
            let (journal, kind, number, inv, inv_party, rcp, rcp_party, rcp_kind) = row?;
            let source = match (kind.as_str(), inv, rcp, rcp_kind.as_deref()) {
                ("INV", Some(id), _, _) => Some(SourceVm { label: "Invoice".into(), number: number.clone(), party: inv_party.unwrap_or_default(), url: url("invoices:detail", &[&cpk, &id]) }),
                ("RCP", _, Some(id), Some("RCP")) => Some(SourceVm { label: "Receipt".into(), number: number.clone(), party: rcp_party.unwrap_or_default(), url: url("receipts:detail", &[&cpk, &id]) }),
                ("SR", _, Some(id), Some("SR")) => Some(SourceVm { label: "Sales receipt".into(), number: number.clone(), party: rcp_party.unwrap_or_default(), url: url("sales_receipts:detail", &[&cpk, &id]) }),
                _ => None,
            };
            out.insert(journal, (source, number));
        }
        out.extend(d5::register_sources(tx,c)?);
        Ok(out)
    })?)
}

fn url(name: &str, args: &[&str]) -> String {
    fb_templates::django::reverse(name, &args.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>()).unwrap_or_default()
}

/// `ledger.views.register` (the plain ledger register; bank and card registers are the banking wave).
fn register(app: &mut App, route: &Route, params: &[(String, String)], path: &str) -> AppResult<(String, u16)> {
    let c = arg_company(route)?;
    let account_id = route.args.get("account_id").and_then(|s| AccountId::parse(s)).ok_or_else(|| AppError::NotFound("account".into()))?;
    let shell_vm = shell(app, Some(c), &Current { route: "register", kind: None })?;
    let open = app.open_mut()?;
    let owner = open.owner;
    let account = open.book.read(|tx| fb_core::store::account(tx, account_id))?.filter(|a| a.company_id == c).ok_or_else(|| AppError::NotFound("account".into()))?;
    let designated: bool = open
        .book
        .read(|tx| {
            tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM documents_designatedbank d JOIN documents_accountingsetup s ON s.id=d.setup_id WHERE s.company_id=?1 AND d.account_id=?2)",
                [c.to_db(), account_id.to_db()],
                |r| r.get(0),
            )
        })
        .map_err(|e| AppError::Invalid(e.to_string()))?;
    if matches!(account.detail_type, DetailType::Bank | DetailType::CreditCard) || designated {
        return d7b::bank_register(app, route, params, path);
    }
    let start = get(params, "start");
    let end = get(params, "end");
    let result = match fb_core::reports::register(&mut open.book, owner, c, account_id, start, end) {
        Ok(r) => r,
        Err(FbError::Validation(e)) => return Ok((failure(app, shell_vm, &e)?, 400)),
        Err(e) => return Err(e.into()),
    };
    let mut sources = sources(&mut open.book, c)?;
    let complete_sources = d8::sources(app, c)?;
    let open = app.open_mut()?;
    let journal_sources = open.book.read(|tx| {
        tx.prepare("SELECT id,source_id FROM ledger_journal WHERE company_id=?1 AND source_id IS NOT NULL")?
            .query_map([c.to_db()], |r| Ok((r.get::<_,String>(0)?, r.get::<_,String>(1)?)))?
            .collect::<Result<Vec<_>,_>>()
    })?;
    for (journal, identity) in journal_sources {
        if let Some((source, _)) = sources.get_mut(&journal)
            && let Some(meta) = complete_sources.get(&identity)
        {
            *source = Some(meta.clone());
        }
    }
    let mut journals: HashMap<String, fb_core::model::Journal> = HashMap::new();
    for row in &result.rows {
        let key = row.journal_id.to_db();
        if !journals.contains_key(&key)
            && let Some(j) = open.book.read(|tx| fb_core::store::journal(tx, row.journal_id))?
        {
            journals.insert(key, j);
        }
    }
    let rows: Vec<RegisterRowVm> = result
        .rows
        .iter()
        .map(|row| {
            let jid = row.journal_id.to_db();
            let journal = journals.get(&jid);
            let (source, number) = sources.get(&jid).cloned().unwrap_or((None, String::new()));
            let source_id = journal.and_then(|j| j.source_id.clone());
            RegisterRowVm {
                line: LineVm {
                    journal_id: jid,
                    journal: JournalRefVm {
                        label: format!("J{:06}", row.journal_number),
                        date: row.date.clone(),
                        memo: row.memo.clone(),
                        source: source_id.as_ref().map(|_| SourceRefVm { number }),
                        source_id,
                    },
                    debit: row.line.debit.to_string(),
                    credit: row.line.credit.to_string(),
                },
                source,
                reversal: row.reversal,
                balance: row.balance.to_string(),
            }
        })
        .collect();
    let page = Page::of(&rows, 25, get(params, "page"));
    let query: Vec<(String, String)> = params.iter().filter(|(k, _)| k != "page").cloned().collect();
    let vm = RegisterVm {
        shell: shell_vm,
        account: account_vm(&account),
        start: result.start.clone(),
        end: result.end.clone(),
        opening: result.opening.to_string(),
        closing: result.closing.to_string(),
        rows: page.object_list.clone(),
        page,
        query_string: urlencode(&query),
    };
    Ok((app.screens.render("ledger/register.html", &vm)?, 200))
}

fn failure(app: &mut App, shell: Shell, errors: &[String]) -> AppResult<String> {
    let mut vm=serde_json::to_value(shell).map_err(|e|AppError::Invalid(e.to_string()))?;
    vm["errors"]=json!(errors);
    Ok(app.screens.render("ledger/error.html",&vm)?)
}

fn not_ported(app: &mut App, route: &Route, path: &str) -> AppResult<String> {
    #[derive(serde::Serialize)]
    struct Vm {
        #[serde(flatten)]
        shell: Shell,
        title: String,
        path: String,
    }
    let company = route.args.get("company_id").and_then(|s| CompanyId::parse(s));
    let shell = shell(app, company, &Current { route: &route.name, kind: route.args.get("kind").map(String::as_str) })?;
    Ok(app.screens.render("desktop/not_ported.html", &Vm { shell, title: "Not available yet".into(), path: path.into() })?)
}

fn short_contact_label(label: String, id: &str, ordinal: usize) -> String {
    let suffix=format!("(#{id})");
    match label.strip_suffix(&suffix) {Some(prefix)=>format!("{prefix}(#{ordinal})"),None=>label}
}

fn customers(book: &mut Book, c: CompanyId) -> AppResult<Vec<(String, String)>> {
    Ok(book.read(|tx| -> Result<_, FbError> {
        let all = fb_core::directory::list(tx, c, fb_core::directory::Model::Customer)?;
        let ordinals=d2::ordinals(tx,c,"directory_customer")?;
        let dups = fb_core::party_display::duplicate_active_display_names(tx, c, fb_core::directory::Model::Customer)?;
        Ok(all
            .iter()
            .filter(|r| r["active"] == true)
            .map(|r| {
                (r["id"].as_str().unwrap_or_default().to_owned(), {
                    let id = r["id"].as_str().unwrap_or_default();
                    short_contact_label(
                        fb_core::party_display::contact_list_label(r, &dups, None, ""),
                        id,
                        ordinals[id],
                    )
                })
            })
            .collect())
    })?)
}

const INVOICE_STATES: &[(&str, &str)] = &[
    ("", "All states"),
    ("OUTSTANDING", "Unpaid balance"),
    ("OVERDUE", "Overdue"),
    ("PAID_RECENT", "Paid in last 30 days"),
    ("DRAFT", "Draft"),
    ("OPEN", "Open"),
    ("PARTIAL", "Partial"),
    ("PAID", "Paid"),
    ("VOID", "Voided"),
];

fn valid_date(text: &str) -> bool {
    fb_core::py::form_date(text).is_ok()
}

/// Latest settlement date of each paid invoice (non-reversed receipts, non-unapplied credit applications).
fn paid_on(book: &mut Book, c: CompanyId) -> AppResult<HashMap<String, String>> {
    book
        .read(|tx| -> fb_db::rusqlite::Result<HashMap<String, String>> {
            let mut out: HashMap<String, String> = HashMap::new();
            let mut st = tx.prepare(
                "SELECT a.invoice_id, max(r.date) FROM sales_receiptallocation a JOIN sales_receipt r ON r.id=a.receipt_id
                 WHERE r.company_id=?1 AND NOT EXISTS(SELECT 1 FROM sales_receiptreversal v WHERE v.receipt_id=r.id) GROUP BY a.invoice_id
                 UNION ALL
                 SELECT l.invoice_id, max(p.date) FROM credits_creditapplicationline l JOIN credits_creditapplication p ON p.id=l.application_id
                 WHERE p.company_id=?1 AND NOT EXISTS(SELECT 1 FROM credits_creditunapplication u WHERE u.application_id=p.id) GROUP BY l.invoice_id",
            )?;
            for row in st.query_map([c.to_db()], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
                let (id, date) = row?;
                let e = out.entry(id).or_default();
                if date > *e {
                    *e = date;
                }
            }
            Ok(out)
        })
        .map_err(|e| AppError::Invalid(e.to_string()))
}

/// `sales.invoice_views.listing` with `invoice_listing` (filters, tracker totals, links).
fn invoice_list(app: &mut App, route: &Route, params: &[(String, String)]) -> AppResult<(String, u16)> {
    let c = arg_company(route)?;
    let today = (app.today)();
    let shell_vm = shell(app, Some(c), &Current { route: "invoices:list", kind: None })?;
    let open = app.open_mut()?;
    let owner = open.owner;
    let customer_choices = d4::customer_choices(&mut open.book, c, true, None, false)?;
    let q = get(params, "q").unwrap_or("").trim().to_owned();
    let state = get(params, "state").unwrap_or("").to_owned();
    let customer = get(params, "customer").unwrap_or("").to_owned();
    let start = get(params, "start").unwrap_or("").trim().to_owned();
    let end = get(params, "end").unwrap_or("").trim().to_owned();
    let mut errs: HashMap<&str, Vec<String>> = HashMap::new();
    if q.chars().count() > 120 {
        errs.entry("q").or_default().push(format!("Ensure this value has at most 120 characters (it has {}).", q.chars().count()));
    }
    if !INVOICE_STATES.iter().any(|(v, _)| *v == state) {
        errs.entry("state").or_default().push(format!("Select a valid choice. {state} is not one of the available choices."));
    }
    if !customer.is_empty() && !customer_choices.iter().any(|(id, _)| *id == customer) {
        errs.entry("customer").or_default().push("Select a valid choice. That choice is not one of the available choices.".into());
    }
    for (k, v) in [("start", &start), ("end", &end)] {
        if !v.is_empty() && !valid_date(v) {
            errs.entry(k).or_default().push("Enter a valid date.".into());
        }
    }
    let valid = errs.is_empty();
    let mut choices = vec![(String::new(), "Choose…".to_owned())];
    choices.extend(customer_choices.iter().cloned());
    let take = |k: &str| errs.get(k).cloned().unwrap_or_default();
    let fields = vec![
        Field::new("q", "Search number / customer / memo", Widget::Text).attr("maxlength", "120").value(get(params, "q").unwrap_or("")).errors(take("q")),
        Field::new("state", "State", Widget::Select { choices: INVOICE_STATES.iter().map(|(a, b)| ((*a).into(), (*b).into())).collect() }).value(state.clone()).errors(take("state")),
        Field::new("customer", "Customer", Widget::Select { choices }).value(customer.clone()).errors(take("customer")),
        Field::new("start", "Start", Widget::Date).value_opt(get(params, "start").map(str::to_owned)).errors(take("start")),
        Field::new("end", "End", Widget::Date).value_opt(get(params, "end").map(str::to_owned)).errors(take("end")),
    ];
    let filter_form = forms::bind(&fields, &[]);
    let (rows, totals, links) = if valid {
        let all = fb_core::sales::invoice_balances(&mut open.book, owner, c)?;
        let meta = invoice_meta(&mut open.book, c)?;
        let labels = open.book.read(|tx| -> Result<Value,FbError> {
            let mut labels=fb_core::party_display::contact_label_map(tx,c,fb_core::directory::Model::Customer,None)?;
            let ordinals=d2::ordinals(tx,c,"directory_customer")?;
            for (id,label) in labels.as_object_mut().unwrap() {
                if let Some(text)=label.as_str(){*label=json!(short_contact_label(text.to_owned(),id,ordinals[id]));}
            }
            Ok(labels)
        })?;
        let ql = q.to_lowercase();
        let scoped: Vec<&Value> = all
            .iter()
            .filter(|r| {
                let id = r["invoice_id"].as_str().unwrap_or_default();
                let (number, party, memo) = meta.get(id).map(|m| (m.0.as_str(), m.1.as_str(), m.2.as_str())).unwrap_or(("", "", ""));
                (ql.is_empty() || number.to_lowercase().contains(&ql) || party.to_lowercase().contains(&ql) || memo.to_lowercase().contains(&ql))
                    && (customer.is_empty() || r["customer_id"] == customer.as_str())
                    && (start.is_empty() || r["date"].as_str().is_some_and(|d| d >= start.as_str()))
                    && (end.is_empty() || r["date"].as_str().is_some_and(|d| d <= end.as_str()))
            })
            .collect();
        let paid = paid_on(&mut open.book, c)?;
        let cutoff = (today - chrono::Duration::days(30)).to_string();
        let today_s = today.to_string();
        let paid_recent = |r: &Value| {
            r["payment_state"] == "PAID"
                && paid
                    .get(r["invoice_id"].as_str().unwrap_or_default())
                    .is_some_and(|d| *d >= cutoff)
        };
        let currency = shell_vm
            .company
            .as_ref()
            .map(|c| c.currency.clone())
            .unwrap_or_default();
        let totals = TrackerTotalsVm {
            open_money: cash(scoped.iter().filter(|r| open_q(r)).map(|r| money(&r["outstanding"])).sum()),
            overdue_money: cash(scoped.iter().filter(|r| invoice_overdue(r, &today_s)).map(|r| money(&r["outstanding"])).sum()),
            paid_recent_money: cash(scoped.iter().filter(|r| paid_recent(r)).map(|r| money(&r["total"])).sum()),
            draft_count: scoped.iter().filter(|r| r["payment_state"] == "DRAFT").count() as i64,
            currency: currency.clone(),
        };
        let mut base: Vec<(String, String)> = vec![];
        for (k, v) in [("q", &q), ("customer", &customer), ("start", &start), ("end", &end)] {
            if !v.is_empty() {
                base.push((k.into(), v.clone()));
            }
        }
        let link = |s: &str| {
            let mut p = base.clone();
            p.push(("state".into(), s.into()));
            urlencode(&p)
        };
        let links = TrackerLinksVm { open: link("OUTSTANDING"), overdue: link("OVERDUE"), paid_recent: link("PAID_RECENT"), drafts: link("DRAFT") };
        let mut filtered: Vec<&Value> = scoped
            .into_iter()
            .filter(|r| match state.as_str() {
                "" => true,
                "OUTSTANDING" => open_q(r),
                "OVERDUE" => invoice_overdue(r, &today_s),
                "PAID_RECENT" => paid_recent(r),
                s => r["payment_state"] == s,
            })
            .collect();
        filtered.sort_by(|a, b| {
            b["date"]
                .as_str()
                .cmp(&a["date"].as_str())
                .then(meta_ordinal(&meta, b).cmp(&meta_ordinal(&meta, a)))
        });
        let rows: Vec<InvoiceRowVm> = filtered
            .iter()
            .map(|r| {
                let id = r["invoice_id"].as_str().unwrap_or_default().to_owned();
                let m = meta.get(&id).cloned().unwrap_or_default();
                let label = labels[r["customer_id"].as_str().unwrap_or_default()].as_str().map(str::to_owned).filter(|s| !s.is_empty()).unwrap_or(m.1.clone());
                InvoiceRowVm {
                    number: m.0,
                    customer_label: label,
                    date: r["date"].as_str().unwrap_or_default().into(),
                    due_date: r["due_date"].as_str().unwrap_or_default().into(),
                    payment_state: r["payment_state"].as_str().unwrap_or_default().into(),
                    total: r["total"].as_str().unwrap_or_default().into(),
                    currency: m.3,
                    outstanding: r["outstanding"].as_str().map(str::to_owned),
                    invoice_id: id,
                }
            })
            .collect();
        (rows, Some(totals), Some(links))
    } else {
        (vec![], None, None)
    };
    let page = Page::of(&rows, 25, get(params, "page"));
    let query: Vec<(String, String)> = params.iter().filter(|(k, _)| k != "page").cloned().collect();
    let vm = InvoiceListVm { shell: shell_vm, filter_form, page, query_string: urlencode(&query), tracker_totals: totals, tracker_links: links };
    Ok((app.screens.render("sales/invoices/list.html", &vm)?, if valid { 200 } else { 400 }))
}

/// Per invoice: (number, customer snapshot name, memo, currency, identity ordinal).
type Meta = HashMap<String, (String, String, String, String, i64)>;

fn meta_ordinal(meta: &Meta, r: &Value) -> i64 {
    meta.get(r["invoice_id"].as_str().unwrap_or_default()).map(|m| m.4).unwrap_or(0)
}

fn invoice_meta(book: &mut Book, c: CompanyId) -> AppResult<Meta> {
    book.read(|tx| -> fb_db::rusqlite::Result<Meta> {
        let mut st = tx.prepare(
            "SELECT inv.id, i.number, coalesce(json_extract(r.data,'$.customer_snapshot.display_name'),''), coalesce(json_extract(r.data,'$.memo'),''),
                    coalesce(json_extract(r.data,'$.company_snapshot.currency'), i.currency), i.ordinal
             FROM sales_invoice inv JOIN documents_documentidentity i ON i.id=inv.identity_id
             JOIN sales_invoicerevision r ON r.invoice_id=inv.id AND r.number=inv.revision WHERE inv.company_id=?1",
        )?;
        st.query_map([c.to_db()], |r| Ok((r.get::<_, String>(0)?, (r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))))?.collect()
    })
    .map_err(|e| AppError::Invalid(e.to_string()))
}

/// What the invoice form was given (a POST's fields), or nothing for a fresh GET.
type Posted<'a> = &'a [(String, String)];

struct InvoiceChoices {
    customers: Vec<(String, String)>,
    items: Vec<(String, String)>,
    accounts: Vec<(String, String)>,
    tax: Vec<(String, String)>,
    tax_initial: String,
    catalog: Value,
    customer_tax: Value,
}

fn invoice_choices(book: &mut Book, c: CompanyId) -> AppResult<InvoiceChoices> {
    let customers = customers(book, c)?;
    book.read(|tx| -> Result<InvoiceChoices, FbError> {
        let item_ordinals = d2::ordinals(tx, c, "directory_item")?;
        let account_ordinals = d2::ordinals(tx, c, "ledger_account")?;
        let items = fb_core::directory::list(tx, c, fb_core::directory::Model::Item)?;
        let mut item_choices = vec![];
        let mut catalog = serde_json::Map::new();
        for it in items.iter().filter(|i| i["active"] == true) {
            let id = it["id"].as_str().unwrap_or_default().to_owned();
            item_choices.push((id.clone(), format!("{} · #{}", it["code"].as_str().unwrap_or_default(), item_ordinals[&id])));
            catalog.insert(
                id,
                json!({"description": it["description"], "rate": it["sale_rate"].as_str().map(str::to_owned).unwrap_or_else(|| "None".into()),
                       "taxable": it["taxable"], "income_account": it["income_account_id"].as_str().or(it["income_account"].as_str()).unwrap_or("")}),
            );
        }
        let mut accounts = fb_core::store::accounts(tx, c)?;
        accounts.sort_by(|a, b| a.code.cmp(&b.code).then(a.id.to_db().cmp(&b.id.to_db())));
        let account_choices = accounts
            .iter()
            .map(|a| {
                let unavailable = !a.active || !a.postable || a.kind != Kind::Income;
                (a.id.to_db(), format!("{} · {} · #{}{}", a.code, a.name, account_ordinals[&a.id.to_db()], if unavailable { " (unavailable — replace before posting)" } else { "" }))
            })
            .collect();
        let settings = fb_core::salestax::settings_for(tx, c)?;
        let tax = if settings["charge_tax"] == true {
            let mut t = vec![(String::new(), "Choose sales tax".into()), ("NONE".to_owned(), "No tax".to_owned())];
            for item in fb_core::salestax::list(tx, c)? {
                if item["active"] == true {
                    t.push((format!("T{}", item["id"].as_str().unwrap_or_default()), fb_core::salestax::item_label(tx, c, &item)?));
                }
            }
            t
        } else {
            vec![("NONE".to_owned(), "No tax (sales tax is turned off)".to_owned())]
        };
        Ok(InvoiceChoices {
            customers,
            items: item_choices,
            accounts: account_choices,
            tax_initial: if settings["charge_tax"]==true {settings["default_item_id"].as_str().filter(|id|tax.iter().any(|(v,_)|v==&format!("T{id}"))).map(|id|format!("T{id}")).unwrap_or_default()}else{"NONE".into()},
            tax,
            catalog: Value::Object(catalog),
            customer_tax: fb_core::directory::customer_tax_hints(tx, c)?,
        })
    })
    .map_err(AppError::from)
}

fn line_fields(
    prefix: &str,
    ch: &InvoiceChoices,
    data: Option<&HashMap<String, String>>,
    errors: &HashMap<String, Vec<String>>,
    full_choices: bool,
) -> Vec<Field> {
    let v = |name: &str| data.and_then(|d| d.get(&format!("{prefix}-{name}")).cloned());
    let checked = |name: &str| v(name).is_some_and(|s| !s.is_empty() && s != "false" && s != "0");
    let e = |name: &str| {
        errors
            .get(&format!("{prefix}-{name}"))
            .cloned()
            .unwrap_or_default()
    };
    let with_blank = |list: &[(String, String)]| {
        let mut out = vec![(String::new(), "Choose…".to_owned())];
        out.extend(list.iter().cloned());
        out
    };
    let item_choices = if full_choices {
        with_blank(&ch.items)
    } else {
        vec![]
    };
    let account_choices = if full_choices {
        with_blank(&ch.accounts)
    } else {
        vec![]
    };
    vec![
        Field::new("origin_position", "Origin position", Widget::Hidden)
            .prefix(prefix)
            .value_opt(v("origin_position"))
            .errors(e("origin_position")),
        Field::new(
            "item",
            "Item",
            Widget::Select {
                choices: item_choices,
            },
        )
        .prefix(prefix)
        .value(v("item").unwrap_or_default())
        .errors(e("item")),
        Field::new(
            "description",
            "Description",
            Widget::Textarea { cols: 40, rows: 2 },
        )
        .prefix(prefix)
        .attr("maxlength", "1000")
        .value_opt(v("description"))
        .errors(e("description")),
        Field::new("quantity", "Quantity", Widget::Text)
            .prefix(prefix)
            .attr("maxlength", "40")
            .value_opt(v("quantity"))
            .errors(e("quantity")),
        Field::new("rate", "Rate", Widget::Text)
            .prefix(prefix)
            .attr("maxlength", "40")
            .value_opt(v("rate"))
            .errors(e("rate")),
        Field::new(
            "taxable",
            "Taxable",
            Widget::Checkbox {
                checked: checked("taxable"),
            },
        )
        .prefix(prefix)
        .errors(e("taxable")),
        Field::new("tax_auto", "Tax auto", Widget::Hidden)
            .prefix(prefix)
            .value(if data.is_some() {
                v("tax_auto").unwrap_or_default()
            } else {
                "1".into()
            })
            .errors(e("tax_auto")),
        Field::new(
            "refresh_item",
            "Update item details from the item list",
            Widget::Checkbox {
                checked: checked("refresh_item"),
            },
        )
        .prefix(prefix),
        Field::new(
            "keep_archived",
            "Keep archived item details on this line",
            Widget::Checkbox {
                checked: checked("keep_archived"),
            },
        )
        .prefix(prefix),
        Field::new(
            "income_account",
            "Income account (required to post)",
            Widget::Select {
                choices: account_choices,
            },
        )
        .prefix(prefix)
        .value(v("income_account").unwrap_or_default())
        .errors(e("income_account")),
        Field::new(
            "DELETE",
            "Delete",
            Widget::Checkbox {
                checked: checked("DELETE"),
            },
        )
        .prefix(prefix),
    ]
}

/// First run (`mode` = "setup") or the app lock ("unlock"). Works while the book is closed.
pub fn render_gate(app: &App, mode: &str, needs_password: bool, password_reason: &str, errors: &[String]) -> AppResult<String> {
    render_gate_with_chart(app, mode, needs_password, password_reason, errors, "standard")
}

pub fn render_gate_with_chart(app: &App, mode: &str, needs_password: bool, password_reason: &str,
    errors: &[String], starter_chart: &str) -> AppResult<String> {
    if !["standard", "empty"].contains(&starter_chart) {
        return Err(AppError::Invalid("Choose a chart of accounts.".into()));
    }
    #[derive(serde::Serialize)]
    struct Vm<'a> {
        #[serde(flatten)]
        gate: GateVm,
        mode: &'a str,
        key_store: &'static str,
        starter_chart: &'a str,
    }
    if mode != "setup" && mode != "unlock" {
        return Err(AppError::Invalid("Unknown gate screen.".into()));
    }
    let shell = Shell {
        nav: nav::build(None, &Current::default(), false, 0).map_err(AppError::Render)?,
        plugins: app.plugins,
        bankfeeds: app.bankfeeds_enabled(),
        ..Shell::default()
    };
    let gate = GateVm { shell, errors: errors.to_vec(), needs_password, password_reason: password_reason.into(), ..GateVm::default() };
    Ok(app.screens.render("desktop/gate.html", &Vm { gate, mode, key_store: key_store(std::env::consts::OS), starter_chart })?)
}

/// Where Electron's safeStorage keeps the sealed key: DPAPI under the Windows sign-in, the macOS
/// Keychain, or the Linux keyring (Secret Service / KWallet).
fn key_store(os: &str) -> &'static str {
    match os {
        "windows" => "protected by your Windows sign-in",
        "macos" => "kept in this Mac's Keychain",
        _ => "kept in this computer's keyring",
    }
}

/// A ported form's submission (the command token was already checked by the session).
pub fn submit(app: &mut App, path: &str, fields: &[(String, String)]) -> AppResult<Outcome> {
    if fields.len() > 2000 || fields.iter().any(|(k, v)| k.len() > 100 || v.len() > 10_000) {
        return Err(AppError::Invalid("Form too large.".into()));
    }
    if identity::handles(path) { return identity::submit(app,path,fields); }
    app.authorize_path(path, if get(fields,"action")==Some("desktop_print") { crate::identity::Permission::Read } else { crate::identity::Permission::Write })?;
    if get(fields, "action") == Some("desktop_print") { return printing::submit(app, path, fields); }
    if protection::handles(path) { return protection::submit(app, path, fields); }
    let route = resolve(path).ok_or_else(|| AppError::NotFound(path.to_owned()))?;
    if !plugin_allowed(app, &route.name) {
        return Err(AppError::NotFound(format!("no form at {path}")));
    }
    match route.name.as_str() {
        _ if d8::is_route(&route.name) => d8::method_not_allowed(app, &route),
        _ if d9::is_route(&route.name) => d9::submit(app, &route, path, fields),
        _ if d4::is_route(&route.name) => d4::submit(app, &route, path, fields),
        _ if d2::is_form(&route.name) => d2::submit(app, &route, path, fields),
        _ if d7a::is_route(&route.name) => d7a::submit(app, &route, path, fields),
        _ if route.name == "register" || d7b::is_route(&route.name) => d7b::submit(app, &route, path, fields),
        _ if d5::is_route(&route.name) => d5::submit(app, &route, path, fields),
        _ if d6::is_route(&route.name) => d6::submit(app, &route, path, fields),
        _ if d3::is_route(&route.name) => d3::submit(app, &route, path, fields),
        _ if backups::is_route(&route.name) => backups::submit(app, &route, fields),
        _ => Err(AppError::NotFound(format!("no form at {path}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_uses_the_django_route_table() {
        let (c, a) = ("0195b4a4-55af-719e-96cb-a2f946b067bf", "0195b4a4-55af-719e-96cb-a2f946b067c0");
        let r = resolve(&format!("/c/{c}/accounts/{a}/register/")).unwrap();
        assert_eq!(r.name, "register");
        assert_eq!(r.args["company_id"], c);
        assert_eq!(r.args["account_id"], a);
        assert_eq!(resolve(&format!("/c/{c}/invoices/new/")).unwrap().name, "invoices:new");
        assert_eq!(resolve(&format!("/c/{c}/invoices/{a}/")).unwrap().name, "invoices:detail");
        assert!(resolve("/c/abc/home/").is_none(), "non-id in an id position");
        assert!(resolve(&format!("/c/{}/home/", c.to_uppercase())).is_none(), "ids are canonical lowercase");
        assert!(resolve("/c//home/").is_none());
        assert!(resolve("/nope/").is_none());
        for kind in ["estimates", "invoices"] {
            assert!(resolve(&format!("/c/{c}/{kind}/{a}/revisions/2/")).is_some());
            for invalid in ["+2", "-2", "٢", "2_0"] {
                assert!(resolve(&format!("/c/{c}/{kind}/{a}/revisions/{invalid}/")).is_none());
            }
        }
    }

    #[test]
    fn setup_names_the_key_store_of_each_os() {
        assert_eq!(key_store("windows"), "protected by your Windows sign-in");
        assert_eq!(key_store("macos"), "kept in this Mac's Keychain");
        assert_eq!(key_store("linux"), "kept in this computer's keyring");
        let html = render_gate(&App::new(), "setup", false, "", &[]).unwrap();
        assert!(html.contains("Anyone who gets a copy of this file can open it."), "{html}");
        assert!(html.contains("Protect these books with my own password"), "{html}");
        assert!(!html.contains("its key is kept"), "built-in keys must not imply OS confidentiality");
        let html = render_gate(&App::new(), "setup", true, "reason", &[]).unwrap();
        assert!(html.contains("recovery key") && html.contains("id=\"own-password-fields\" hidden"), "{html}");
    }

    #[test]
    fn query_strings_round_trip() {
        let p = parse_query("q=a+b%26c&state=&x");
        assert_eq!(p, vec![("q".into(), "a b&c".into()), ("state".into(), "".into()), ("x".into(), "".into())]);
        assert_eq!(urlencode(&[("q".into(), "a b&c".into()), ("state".into(), "OVERDUE".into())]), "q=a+b%26c&state=OVERDUE");
        assert_eq!(parse_query("a=%zz&b=%4"), vec![("a".into(), "%zz".into()), ("b".into(), "%4".into())]);
    }
}
