//! Frozen banking register and reconciliation HTTP boundaries. Money and writes stay in fb-core.
use super::d2::{clean_date, clean_text, draw, error, token};
use super::d6::{ctx, s, scoped_rows};
use super::*;
use crate::{Command, commands::BeginReconciliationValues};
use chrono::{Datelike, Duration, Months, NaiveDate};
use fb_core::{
    banking::{self as reconcile, self},
    py,
};
type Errors = HashMap<String, Vec<String>>;
const FILTERS: &[&str] = &[
    "preset", "start", "end", "state", "payee", "number", "amount",
];
const PRESETS: &[(&str, &str)] = &[
    ("", "All dates"),
    ("this_month", "This month"),
    ("last_month", "Last month"),
    ("this_quarter", "This quarter"),
    ("this_year", "This year"),
    ("year_to_date", "Year to date"),
    ("custom", "Custom"),
];
const STATES: &[(&str, &str)] = &[
    ("", "Any"),
    ("U", "Not cleared"),
    ("C", "Cleared"),
    ("R", "Reconciled"),
    ("open", "Not reconciled"),
];
const RELOAD: &str = "Reload the reconciliation and try again.";

pub(crate) fn is_d7b_command(c: &Command) -> bool {
    matches!(
        c,
        Command::ExportBankRegister { .. }
            | Command::ExportReconciliation { .. }
            | Command::MarkBankLine { .. }
            | Command::BeginReconciliation { .. }
            | Command::SaveReconciliationTicks { .. }
            | Command::FinishReconciliation { .. }
            | Command::UndoReconciliation { .. }
    )
}
pub(crate) fn run_d7b_command(app: &mut App, cmd: Command) -> AppResult<Outcome> {
    match &cmd {
        Command::ExportBankRegister {
            company_id,
            account_id,
            filters,
        } => {
            return export(
                app,
                "register",
                company_id,
                account_id,
                filters,
                "bank-register.csv",
            );
        }
        Command::ExportReconciliation {
            company_id,
            reconciliation_id,
            filters,
        } => {
            return export(
                app,
                "reconcile:report",
                company_id,
                reconciliation_id,
                filters,
                "reconciliation.csv",
            );
        }
        _ => {}
    }
    let open = app.open_mut()?;
    let actor = open.owner;
    let data = match cmd {
        Command::MarkBankLine {
            company_id,
            account_id,
            line_id,
            state,
            expected_revision,
        } => banking::set_cleared(
            &mut open.book,
            actor,
            crate::session::company(&company_id)?,
            &line_id,
            &state,
            expected_revision,
            Some(&account_id),
        )?,
        Command::BeginReconciliation {
            company_id,
            account_id,
            values,
        } => reconcile::begin(
            &mut open.book,
            actor,
            crate::session::company(&company_id)?,
            &account_id,
            &serde_json::to_value(values).unwrap(),
        )?,
        Command::SaveReconciliationTicks {
            company_id,
            account_id,
            reconciliation_id,
            expected_revision,
            shown,
            ticked,
            hide_later,
        } => reconcile::save_ticks(
            &mut open.book,
            actor,
            crate::session::company(&company_id)?,
            &reconciliation_id,
            &account_id,
            &json!({"expected_revision":expected_revision,"shown":shown,"ticked":ticked,"hide_later":hide_later}),
        )?,
        Command::FinishReconciliation {
            company_id,
            account_id,
            reconciliation_id,
            expected_revision,
            completion_key,
        } => reconcile::reconcile_now(
            &mut open.book,
            actor,
            crate::session::company(&company_id)?,
            &reconciliation_id,
            &account_id,
            Some(expected_revision),
            &json!(completion_key),
        )?,
        Command::UndoReconciliation {
            company_id,
            account_id,
            reconciliation_id,
            expected_revision,
            reason,
            undo_key,
        } => reconcile::undo_last(
            &mut open.book,
            actor,
            crate::session::company(&company_id)?,
            &reconciliation_id,
            &account_id,
            expected_revision,
            &json!(reason),
            &json!(undo_key),
        )?,
        _ => unreachable!(),
    };
    Ok(Outcome::Done { data })
}
fn export(
    app: &mut App,
    name: &str,
    c: &str,
    id: &str,
    filters: Posted<'_>,
    filename: &str,
) -> AppResult<Outcome> {
    let mut filters = filters
        .iter()
        .filter(|(k, _)| k != "output")
        .cloned()
        .collect::<Vec<_>>();
    filters.push(("output".into(), "csv".into()));
    let target = format!("{}?{}", url(name, &[c, id]), urlencode(&filters));
    let rendered = super::render(app, &target)?;
    if rendered.status != 200 {
        return Err(AppError::Invalid(
            "Correct the export filters and try again.".into(),
        ));
    }
    Ok(Outcome::Export {
        filename: filename.into(),
        contents: rendered.html,
    })
}
pub(super) fn is_route(name: &str) -> bool {
    name.starts_with("registers:") || name.starts_with("reconcile:")
}
fn account(app: &mut App, c: CompanyId, aid: &str, register: bool) -> AppResult<Account> {
    let a = AccountId::parse(aid).ok_or_else(|| AppError::NotFound("account".into()))?;
    let o = app.open_mut()?;
    let actor = o.owner;
    Ok(o.book.read(|q| {
        fb_core::services::access(q, actor, c)?;
        let a = fb_core::store::account(q, a)?
            .filter(|a| a.company_id == c)
            .ok_or_else(|| FbError::missing("Account"))?;
        if register && !banking::is_register_account(q, c, aid)? {
            return Err(FbError::missing("Account"));
        }
        Ok::<_, FbError>(a)
    })?)
}
fn recs(app: &mut App, c: CompanyId, aid: &str) -> AppResult<Vec<Value>> {
    let raw = scoped_rows(
        app,
        c,
        "SELECT id FROM banking_reconciliation WHERE company_id=? AND account_id=? ORDER BY id DESC",
        &[&c.to_db(), &aid],
    )?;
    let mut rows = Vec::new();
    for row in raw {
        let mut r = app
            .open_mut()?
            .book
            .read(|q| reconcile::reconciliation(q, c, s(&row, "id"), aid))?;
        r["pk"] = r["id"].clone();
        for (key, action) in [
            ("completed_at", "reconciliation.completed"),
            ("undone_at", "reconciliation.undone"),
        ] {
            let stamp=scoped_rows(app,c,"SELECT occurred_at FROM ledger_auditevent WHERE company_id=? AND target='reconciliation:'||? AND action=? ORDER BY id LIMIT 1",&[&c.to_db(),&s(&row,"id"),&action])?.pop().unwrap_or(Value::Null);
            r[key] = stamp["occurred_at"].clone();
        }
        for key in ["started_by", "completed_by", "undone_by"] {
            if r[key].is_string() {
                r[key] = json!({"get_username":r[key],"username":r[key]});
            }
        }
        rows.push(r);
    }
    Ok(rows)
}
fn current_last(rows: &[Value]) -> (Value, Value) {
    (
        rows.iter()
            .find(|r| r["status"] == "OPEN")
            .cloned()
            .unwrap_or(Value::Null),
        rows.iter()
            .find(|r| r["status"] == "DONE")
            .cloned()
            .unwrap_or(Value::Null),
    )
}
fn set_page(vm: &mut Value, rows: &[Value], size: usize, p: Posted<'_>) {
    let requested = get(p, "page")
        .and_then(python_integer)
        .map(|n| n.to_string());
    vm["page"] = serde_json::to_value(Page::of(rows, size, requested.as_deref())).unwrap();
    vm["query_string"] = json!("");
}
fn merge(vm: &mut Value, data: Value) {
    for (k, v) in data.as_object().expect("view data") {
        vm[k] = v.clone();
    }
}
fn invalid(app: &mut App, r: &Route, errors: Vec<String>) -> AppResult<Outcome> {
    let mut vm = ctx(app, r)?;
    vm["errors"] = json!(errors);
    Ok(Outcome::Invalid {
        html: draw(app, "ledger/error.html", &vm)?,
    })
}
fn messages(e: AppError) -> Result<Vec<String>, AppError> {
    match e {
        AppError::Core(FbError::Validation(e)) => Ok(e),
        e => Err(e),
    }
}
fn redirect(location: String) -> AppResult<Outcome> {
    Ok(Outcome::Redirect { location })
}
fn rec_url(name: &str, c: CompanyId, aid: &str) -> String {
    url(name, &[&c.to_db(), aid])
}

/// Decorate the unchanged core rows with Django model attributes and local document URLs.
fn decorate_rows(app: &mut App, c: CompanyId, rows: &mut [Value]) -> AppResult<()> {
    let sources = super::d8::sources(app, c)?;
    let feed = scoped_rows(
        app,
        c,
        "SELECT t.journal_line_id FROM bankfeeds_transaction t JOIN bankfeeds_account a ON a.id=t.feed_account_id JOIN bankfeeds_connection b ON b.id=a.connection_id WHERE b.company_id=? AND t.journal_line_id IS NOT NULL",
        &[&c.to_db()],
    )?;
    let feed = feed
        .iter()
        .map(|r| s(r, "journal_line_id"))
        .collect::<std::collections::HashSet<_>>();
    let reversals = scoped_rows(
        app,
        c,
        "SELECT id,date FROM ledger_journal WHERE company_id=? AND reversal_of_id IS NOT NULL",
        &[&c.to_db()],
    )?;
    for row in rows {
        row["line"] = json!({"pk":row["id"],"journal":{"date":row["date"]}});
        let ju = rec_url("journal", c, s(row, "journal_id"));
        row["journal_url"] = json!(ju);
        row["url"] = json!(ju);
        if let Some(source) = row["source_id"].as_str().and_then(|id| sources.get(id)) {
            row["url"] = json!(source.url);
            row["source"] = serde_json::to_value(source).unwrap();
        } else {
            row["source"] = Value::Null;
        }
        row["voided_by"] = reversals
            .iter()
            .find(|r| r["id"] == row["voided_by_id"])
            .cloned()
            .unwrap_or(Value::Null);
        row["feed"] = json!(feed.contains(s(row, "id")));
    }
    Ok(())
}

pub(super) fn render(
    app: &mut App,
    r: &Route,
    p: Posted<'_>,
    path: &str,
) -> AppResult<(String, u16)> {
    let c = arg_company(r)?;
    if r.name == "registers:cleared" {
        let aid = r
            .args
            .get("account_id")
            .ok_or_else(|| AppError::NotFound("account".into()))?;
        account(app, c, aid, false)?;
        return Ok((String::new(), 405));
    }
    if r.name == "registers:list" || r.name == "reconcile:list" {
        let chooser = r.name == "reconcile:list";
        let mut vm = ctx(app, r)?;
        let o = app.open_mut()?;
        let mut accounts = banking::register_accounts(&mut o.book, o.owner, c)?;
        if chooser {
            accounts.retain(|a| a["active"] == true);
        } else {
            accounts.sort_by_key(|a| a["active"] != true);
        }
        let mut rows = vec![];
        for a in accounts {
            let a = account(app, c, s(&a, "id"), true)?;
            let aid = a.id.to_db();
            let mut v = serde_json::to_value(account_vm(&a)).unwrap();
            if chooser {
                let (current, last) = current_last(&recs(app, c, &aid)?);
                v["open_rec"] = current;
                v["last_rec"] = last;
            } else {
                let o = app.open_mut()?;
                let result = banking::build(&mut o.book, o.owner, c, &aid, &json!({}), 1, 1)?;
                v["balance"] = result["closing"].clone();
                v["cleared_balance"] = result["totals"]["cleared"].clone();
            }
            rows.push(v);
        }
        vm["request"] = json!({"GET":{"left":get(p,"left").unwrap_or("")}});
        set_page(&mut vm, &rows, 50, p);
        return Ok((
            draw(
                app,
                if chooser {
                    "banking/reconcile/chooser.html"
                } else {
                    "banking/registers/list.html"
                },
                &vm,
            )?,
            200,
        ));
    }
    if r.name == "reconcile:report" {
        return report(app, r, p);
    }
    let aid = r
        .args
        .get("account_id")
        .ok_or_else(|| AppError::NotFound("account".into()))?;
    let a = account(app, c, aid, true)?;
    let (current, last) = current_last(&recs(app, c, aid)?);
    match r.name.as_str() {
        "reconcile:begin" => Ok((begin_page(app, r, path, None, vec![])?, 200)),
        "reconcile:screen" => {
            if current.is_null() {
                return Ok((rec_url("reconcile:begin", c, aid), 302));
            }
            Ok((
                screen_page(app, r, path, &current, vec![], get(p, "saved") == Some("1"))?,
                200,
            ))
        }
        "reconcile:history" => {
            let mut vm = ctx(app, r)?;
            vm["account"] = serde_json::to_value(account_vm(&a)).unwrap();
            vm["words"] = reconcile::words(&a);
            vm["last"] = last;
            vm["current"] = current;
            vm["request"] = json!({"GET":{"undone":get(p,"undone").unwrap_or("")}});
            let rows = recs(app, c, aid)?
                .into_iter()
                .filter(|r| r["status"] != "OPEN")
                .collect::<Vec<_>>();
            set_page(&mut vm, &rows, 25, p);
            Ok((draw(app, "banking/reconcile/history.html", &vm)?, 200))
        }
        "reconcile:undo" => Ok((undo_page(app, r, path, None, vec![])?, 200)),
        _ => Err(AppError::NotFound(path.into())),
    }
}
fn charge_accounts(app: &mut App, c: CompanyId, a: &Account, kind: &str) -> AppResult<Vec<Value>> {
    let all = app
        .open_mut()?
        .book
        .read(|q| fb_core::store::accounts(q, c))?;
    Ok(all
        .iter()
        .filter(|b| b.active && b.postable && b.kind.as_str() == kind && a.id != b.id)
        .map(|b| serde_json::to_value(account_vm(b)).unwrap())
        .collect())
}
fn begin_page(
    app: &mut App,
    r: &Route,
    path: &str,
    p: Option<Posted<'_>>,
    errors: Vec<String>,
) -> AppResult<String> {
    let c = arg_company(r)?;
    let aid = &r.args["account_id"];
    let a = account(app, c, aid, true)?;
    let (current, last) = current_last(&recs(app, c, aid)?);
    let expenses = charge_accounts(app, c, &a, "EXPENSE")?;
    let incomes = charge_accounts(app, c, &a, "INCOME")?;
    let default = |accounts: &[Value], words: &[&str]| {
        accounts
            .iter()
            .find(|a| {
                words
                    .iter()
                    .any(|w| s(a, "name").to_lowercase().contains(w))
            })
            .map(|a| s(a, "pk").to_owned())
            .unwrap_or_default()
    };
    let mut data = json!({"statement_date":if current.is_null(){(app.today)().to_string()}else{s(&current,"statement_date").into()},"ending_balance":if current.is_null(){""}else{s(&current,"ending_balance")},"service_charge":"","interest":"","service_account":default(&expenses,&["service charge","bank fee","bank charge","finance charge","interest expense"]),"interest_account":default(&incomes,&["interest"]),"request_key":uuid::Uuid::new_v4().to_string(),"revision":current["revision"].as_i64().map(|v|v.to_string()).unwrap_or_default()});
    if let Some(p) = p {
        for name in [
            "statement_date",
            "ending_balance",
            "service_charge",
            "service_account",
            "interest",
            "interest_account",
            "request_key",
            "revision",
        ] {
            data[name] = json!(py::py_truncate(get(p, name).unwrap_or(""), 40));
        }
    }
    let mut vm = ctx(app, r)?;
    merge(
        &mut vm,
        json!({"account":account_vm(&a),"words":reconcile::words(&a),"beginning":last["ending_balance"].as_str().unwrap_or("0.00"),"current":current,"last":last,"data":data,"expenses":expenses,"incomes":incomes,"errors":errors}),
    );
    vm["csrf_token"] = json!(token(app, path));
    draw(app, "banking/reconcile/begin.html", &vm)
}
fn screen_page(
    app: &mut App,
    r: &Route,
    path: &str,
    rec: &Value,
    errors: Vec<String>,
    saved: bool,
) -> AppResult<String> {
    let c = arg_company(r)?;
    let aid = &r.args["account_id"];
    let a = account(app, c, aid, true)?;
    let o = app.open_mut()?;
    let mut data = reconcile::screen(&mut o.book, o.owner, c, s(rec, "id"), aid)?;
    let shown = data["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| format!("{}:{}", s(r, "id"), r["revision"]))
        .collect::<Vec<_>>()
        .join(",");
    for side in ["left", "right"] {
        decorate_rows(app, c, data[side].as_array_mut().unwrap())?;
    }
    data["rec"]["pk"] = data["rec"]["id"].clone();
    let mut vm = ctx(app, r)?;
    merge(&mut vm, data);
    merge(
        &mut vm,
        json!({"account":account_vm(&a),"shown":shown,"errors":errors,"notice":if saved{Some("Ticks saved.")}else{None},"completion_key":uuid::Uuid::new_v4().to_string()}),
    );
    vm["csrf_token"] = json!(token(app, path));
    draw(app, "banking/reconcile/screen.html", &vm)
}
fn undo_page(
    app: &mut App,
    r: &Route,
    path: &str,
    p: Option<Posted<'_>>,
    errors: Vec<String>,
) -> AppResult<String> {
    let c = arg_company(r)?;
    let aid = &r.args["account_id"];
    let a = account(app, c, aid, true)?;
    let (_, last) = current_last(&recs(app, c, aid)?);
    let mut data = json!({"reason":"","undo_key":uuid::Uuid::new_v4().to_string(),"revision":last["revision"].as_i64().map(|n|n.to_string()).unwrap_or_default(),"reconciliation":s(&last,"id")});
    if let Some(p) = p {
        for (key, max) in [
            ("reason", 600),
            ("undo_key", 40),
            ("revision", 12),
            ("reconciliation", 100),
        ] {
            data[key] = json!(py::py_truncate(get(p, key).unwrap_or(""), max));
        }
    }
    let mut vm = ctx(app, r)?;
    merge(
        &mut vm,
        json!({"account":account_vm(&a),"words":reconcile::words(&a),"last":last,"data":data,"errors":errors}),
    );
    vm["csrf_token"] = json!(token(app, path));
    draw(app, "banking/reconcile/undo.html", &vm)
}
fn report(app: &mut App, r: &Route, p: Posted<'_>) -> AppResult<(String, u16)> {
    let c = arg_company(r)?;
    let key = &r.args["reconciliation_id"];
    let row = scoped_rows(
        app,
        c,
        "SELECT account_id FROM banking_reconciliation WHERE company_id=? AND id=?",
        &[&c.to_db(), &key],
    )?
    .pop()
    .ok_or_else(|| AppError::NotFound("reconciliation".into()))?;
    let aid = s(&row, "account_id");
    let a = account(app, c, aid, false)?;
    let rec = recs(app, c, aid)?
        .into_iter()
        .find(|r| r["id"] == *key)
        .ok_or_else(|| AppError::NotFound("reconciliation".into()))?;
    if rec["status"] == "OPEN" {
        return Ok((rec_url("reconcile:screen", c, aid), 302));
    }
    let view = get(p, "view").unwrap_or("");
    let output = get(p, "output").unwrap_or("");
    if !["", "detail", "summary"].contains(&view) || !["", "print", "csv"].contains(&output) {
        let mut vm = ctx(app, r)?;
        vm["errors"] = json!(["Choose the detail or summary view, and screen, print or CSV."]);
        return Ok((draw(app, "ledger/error.html", &vm)?, 400));
    }
    let o = app.open_mut()?;
    let mut data = reconcile::report(&mut o.book, o.owner, c, key, aid)?;
    for side in data["sides"].as_array_mut().unwrap() {
        for group in side["groups"].as_array_mut().unwrap() {
            decorate_rows(app, c, group["rows"].as_array_mut().unwrap())?;
        }
    }
    let mut vm = ctx(app, r)?;
    merge(&mut vm, data);
    merge(
        &mut vm,
        json!({"rec":rec,"account":account_vm(&a),"view":if view.is_empty(){"detail"}else{view},"done":get(p,"done")==Some("1")}),
    );
    if !output.is_empty() && rec["cleared_count"].as_u64().unwrap_or(0) > 2000 {
        vm["errors"] = json!([format!(
            "This reconciliation has {} lines. Print or export at most 2000; use the summary view.",
            rec["cleared_count"]
        )]);
        return Ok((draw(app, "ledger/error.html", &vm)?, 400));
    }
    if output == "csv" {
        return Ok((report_csv(&vm), 200));
    }
    if output == "print" {
        let path = rec_url("reconcile:report",c,key);
        vm["screen_url"] = json!(format!("{path}?view={}",s(&vm,"view")));
        vm["generated"] = json!((app.now)().to_rfc3339());
        let target = format!("{path}?{}",urlencode(p));
        return super::printing::draw(app,"banking/reconcile/report_print.html",&mut vm,&target,"internal");
    }
    vm["export_token"] = json!(token(app, &rec_url("reconcile:report", c, key)));
    Ok((draw(app, "banking/reconcile/report.html", &vm)?, 200))
}

fn revision(raw: &str) -> Result<i64, Vec<String>> {
    python_integer(raw)
        .filter(|n| (0..=1_000_000_000).contains(n))
        .ok_or_else(|| vec!["Reload the page and try again.".into()])
}
fn python_integer(raw: &str) -> Option<i64> {
    match py::py_int(raw)? {
        py::PyInt::Small(n) => Some(n),
        py::PyInt::Large { negative } => Some(if negative { i64::MIN } else { i64::MAX }),
    }
}
fn shown(raw: &str) -> Result<BTreeMap<String, i64>, Vec<String>> {
    let raw = py::py_strip(raw);
    let mut result = BTreeMap::new();
    if raw.is_empty() {
        return Ok(result);
    }
    // Python permits 30 characters per pair for 18-digit PKs. UUIDs add
    // 18 characters; count Unicode scalars (as Python len does), not bytes.
    if raw.chars().count() > 48 * 900 {
        return Err(vec![RELOAD.into()]);
    }
    let pairs = raw.split(',').collect::<Vec<_>>();
    if pairs.len() > 900 {
        return Err(vec![RELOAD.into()]);
    }
    for p in pairs {
        let (id, v) = p.split_once(':').ok_or_else(|| vec![RELOAD.into()])?;
        if fb_ids::parse_uuid(id).is_none_or(|u| u.to_string() != id)
            || v.is_empty()
            || v.chars().count() > 9
            || !v.chars().all(|c| fb_money::unicode_decimal(c).is_some())
        {
            return Err(vec![RELOAD.into()]);
        }
        result.insert(
            id.into(),
            forms::python_integer(v).ok_or_else(|| vec![RELOAD.into()])?,
        );
    }
    Ok(result)
}

pub(super) fn submit(app: &mut App, r: &Route, path: &str, p: Posted<'_>) -> AppResult<Outcome> {
    let c = arg_company(r)?;
    let o = app.open_mut()?;
    let actor = o.owner;
    o.book.read(|q| fb_core::services::access(q, actor, c))?;
    // CSV links submit a generated, path-bound token through the narrowed bridge.
    // Route arguments select the company/document; submitted filters cannot change them.
    if r.name == "register" || r.name == "reconcile:report" {
        let (id, filename) = if r.name == "register" {
            (&r.args["account_id"], "bank-register.csv")
        } else {
            (&r.args["reconciliation_id"], "reconciliation.csv")
        };
        return export(app, &r.name, &c.to_db(), id, p, filename);
    }
    if !matches!(
        r.name.as_str(),
        "registers:cleared" | "reconcile:begin" | "reconcile:screen" | "reconcile:undo"
    ) {
        return Ok(Outcome::Page {
            html: String::new(),
            status: 405,
        });
    }
    let aid = &r.args["account_id"];
    account(app, c, aid, r.name != "registers:cleared")?;
    match r.name.as_str() {
        "registers:cleared" => {
            let line = get(p, "line").unwrap_or("");
            if fb_ids::parse_uuid(line).is_none() {
                return Err(AppError::NotFound("line".into()));
            }
            let cmd = Command::MarkBankLine {
                company_id: c.to_db(),
                account_id: aid.clone(),
                line_id: line.into(),
                state: get(p, "state").unwrap_or("").into(),
                expected_revision: get(p, "revision").and_then(python_integer),
            };
            if let Err(e) = app.run(cmd) {
                return invalid(app, r, messages(e)?);
            }
            let back = py::py_truncate(get(p, "back").unwrap_or(""), 2000);
            let back = register_query(&parse_query(&back), true);
            redirect(format!(
                "{}{}#line-{line}",
                rec_url("register", c, aid),
                if back.is_empty() {
                    String::new()
                } else {
                    format!("?{back}")
                }
            ))
        }
        "reconcile:begin" => {
            let (values, errors) = clean_begin(p);
            let errors = if errors.is_empty() {
                match app.run(Command::BeginReconciliation {
                    company_id: c.to_db(),
                    account_id: aid.clone(),
                    values,
                }) {
                    Ok(_) => return redirect(rec_url("reconcile:screen", c, aid)),
                    Err(e) => messages(e)?,
                }
            } else {
                errors
            };
            Ok(Outcome::Invalid {
                html: begin_page(app, r, path, Some(p), errors)?,
            })
        }
        "reconcile:screen" => submit_screen(app, r, path, p),
        "reconcile:undo" => {
            let key = get(p, "reconciliation").unwrap_or("");
            if fb_ids::parse_uuid(key).is_none() {
                return Err(AppError::NotFound("reconciliation".into()));
            }
            let cmd = Command::UndoReconciliation {
                company_id: c.to_db(),
                account_id: aid.clone(),
                reconciliation_id: key.into(),
                expected_revision: get(p, "revision")
                    .and_then(|raw| forms::python_integer(&py::py_truncate(raw, 12))),
                reason: get(p, "reason").unwrap_or("").into(),
                undo_key: py::py_truncate(get(p, "undo_key").unwrap_or(""), 40),
            };
            match app.run(cmd) {
                Ok(_) => redirect(format!(
                    "{}?undone={key}",
                    rec_url("reconcile:history", c, aid)
                )),
                Err(e) => Ok(Outcome::Invalid {
                    html: undo_page(app, r, path, Some(p), messages(e)?)?,
                }),
            }
        }
        _ => unreachable!(),
    }
}
fn clean_begin(p: Posted<'_>) -> (BeginReconciliationValues, Vec<String>) {
    let mut e = Errors::new();
    let date = clean_date(p, "statement_date", true, &mut e).unwrap_or_default();
    if !date.is_empty() && (date.as_str() < "1900-01-02" || date.as_str() > "9999-12-30") {
        error(
            &mut e,
            "statement_date",
            "Use a statement date between 1900-01-02 and 9999-12-30.",
        );
    }
    let ending = clean_text(p, "ending_balance", 24, true, true, &mut e);
    let charge = clean_text(p, "service_charge", 24, false, true, &mut e);
    let service = clean_text(
        p,
        "service_account",
        if get(p, "service_account").is_some_and(|v| fb_ids::parse_uuid(py::py_strip(v)).is_some())
        {
            36
        } else {
            20
        },
        false,
        true,
        &mut e,
    );
    let interest = clean_text(p, "interest", 24, false, true, &mut e);
    let income = clean_text(
        p,
        "interest_account",
        if get(p, "interest_account").is_some_and(|v| fb_ids::parse_uuid(py::py_strip(v)).is_some())
        {
            36
        } else {
            20
        },
        false,
        true,
        &mut e,
    );
    let key = clean_text(p, "request_key", 40, true, true, &mut e);
    let rev = clean_text(p, "revision", 12, false, true, &mut e);
    let mut errors = vec![];
    for (field, label) in [
        ("statement_date", "Statement date"),
        ("ending_balance", "Ending balance"),
        ("service_charge", "Service charge"),
        ("service_account", "Account"),
        ("interest", "Interest earned"),
        ("interest_account", "Account"),
        ("request_key", "None"),
        ("revision", "None"),
    ] {
        for msg in e.get(field).into_iter().flatten() {
            errors.push(format!("{label}: {msg}"));
        }
    }
    (
        BeginReconciliationValues {
            statement_date: date,
            ending_balance: ending,
            service_charge: charge,
            service_account_id: (!service.is_empty()).then_some(service),
            interest,
            interest_account_id: (!income.is_empty()).then_some(income),
            request_key: key,
            expected_revision: (!rev.is_empty()).then_some(rev),
        },
        errors,
    )
}
fn submit_screen(app: &mut App, r: &Route, path: &str, p: Posted<'_>) -> AppResult<Outcome> {
    let c = arg_company(r)?;
    let aid = &r.args["account_id"];
    let (current, _) = current_last(&recs(app, c, aid)?);
    let action = get(p, "action").unwrap_or("");
    let key = get(p, "reconciliation").unwrap_or("");
    if !["save", "leave", "reconcile"].contains(&action) || fb_ids::parse_uuid(key).is_none() {
        if current.is_null() {
            return redirect(rec_url("reconcile:begin", c, aid));
        }
        return Ok(Outcome::Invalid {
            html: screen_page(
                app,
                r,
                path,
                &current,
                vec!["Choose Save, Leave or Reconcile now.".into()],
                false,
            )?,
        });
    }
    let target = recs(app, c, aid)?
        .into_iter()
        .find(|r| r["id"] == key)
        .ok_or_else(|| AppError::NotFound("reconciliation".into()))?;
    let attempt = (|| -> Result<String, AppError> {
        let shown = shown(get(p, "shown").unwrap_or("")).map_err(FbError::Validation)?;
        let ticked = p
            .iter()
            .filter(|(k, _)| k == "tick")
            .map(|(_, v)| v.clone())
            .collect::<Vec<_>>();
        if ticked.iter().any(|v| fb_ids::parse_uuid(v).is_none()) {
            return Err(FbError::Validation(vec![RELOAD.into()]).into());
        }
        let rev = revision(get(p, "revision").unwrap_or("")).map_err(FbError::Validation)?;
        // Python save_ticks validates membership before raising NotOpen. Keep
        // that validation on completed retries as well as on open statements.
        if ticked.iter().any(|id| !shown.contains_key(id)) {
            return Err(FbError::Validation(vec![RELOAD.into()]).into());
        }
        let hide = (get(p, "options") == Some("1")).then_some(get(p, "hide_later") == Some("1"));
        let saved = if target["status"] == "OPEN" {
            match app.run(Command::SaveReconciliationTicks {
                company_id: c.to_db(),
                account_id: aid.clone(),
                reconciliation_id: key.into(),
                expected_revision: rev,
                shown,
                ticked,
                hide_later: hide,
            })? {
                Outcome::Done { data } => data,
                _ => unreachable!(),
            }
        } else {
            if action != "reconcile" {
                return Err(FbError::validation("This statement is no longer in progress (it was reconciled or undone). Open Banking → Reconcile.").into());
            }
            target.clone()
        };
        if action == "reconcile" {
            // save_ticks increments only when ticks/options changed. A completed retry keeps the posted revision.
            let next = rev
                + i64::from(target["status"] == "OPEN" && saved["revision"] != target["revision"]);
            app.run(Command::FinishReconciliation {
                company_id: c.to_db(),
                account_id: aid.clone(),
                reconciliation_id: key.into(),
                expected_revision: next,
                completion_key: get(p, "completion_key").unwrap_or("").into(),
            })?;
            return Ok(format!("{}?done=1", rec_url("reconcile:report", c, key)));
        }
        Ok(if action == "leave" {
            format!("{}?left={aid}", url("reconcile:list", &[&c.to_db()]))
        } else {
            format!("{}?saved=1", rec_url("reconcile:screen", c, aid))
        })
    })();
    match attempt {
        Ok(location) => redirect(location),
        Err(e) => {
            let errors = messages(e)?;
            let (current, _) = current_last(&recs(app, c, aid)?);
            if current.is_null() {
                return invalid(app, r, errors);
            }
            Ok(Outcome::Invalid {
                html: screen_page(app, r, path, &current, errors, false)?,
            })
        }
    }
}

fn register_query(p: Posted<'_>, page: bool) -> String {
    let mut pairs = vec![];
    for key in FILTERS.iter().copied().chain(page.then_some("page")) {
        let v = get(p, key).unwrap_or("");
        if !v.is_empty() && py::py_len(v) <= 120 {
            pairs.push((key.into(), v.into()));
        }
    }
    urlencode(&pairs)
}
fn choice(p: Posted<'_>, key: &str, choices: &[(&str, &str)], e: &mut Errors) -> String {
    let v = get(p, key).unwrap_or("");
    if !choices.iter().any(|(x, _)| *x == v) {
        error(
            e,
            key,
            format!("Select a valid choice. {v} is not one of the available choices."),
        );
    }
    v.into()
}
fn filter_amount(raw: &str) -> Result<String, &'static str> {
    let raw = raw.replace(',', "");
    let raw = raw.trim_start_matches('$');
    if raw.is_empty() {
        return Ok(String::new());
    }
    let Some(parsed) = fb_money::parse_python_decimal(raw) else {
        return Err("Enter an amount such as 125.00.");
    };
    let invalid = "Enter a positive amount with at most two decimal places.";
    let fb_money::Parsed::Finite(d) = parsed else {
        return Err(invalid);
    };
    let scale = d.exponent + (d.digits.len() - d.digits.trim_end_matches('0').len()) as i128;
    if d.negative || d.is_zero() || scale < -2 {
        return Err(invalid);
    }
    let n = d
        .cents_half_up()
        .filter(|n| *n <= fb_money::MAX_TOTAL_CENTS)
        .ok_or(invalid)?;
    Ok(cash(n))
}
fn filter_data(p: Posted<'_>, today: NaiveDate) -> (Value, Vec<String>) {
    let mut e = Errors::new();
    let preset = choice(p, "preset", PRESETS, &mut e);
    let start = clean_date(p, "start", false, &mut e);
    let end = clean_date(p, "end", false, &mut e);
    for (key, d) in [("start", &start), ("end", &end)] {
        if d.as_ref()
            .is_some_and(|d| d.as_str() < "1900-01-02" || d.as_str() > "9999-12-30")
        {
            error(&mut e, key, "Use a date between 1900-01-02 and 9999-12-30.");
        }
    }
    let state = choice(p, "state", STATES, &mut e);
    let payee = clean_text(p, "payee", 120, false, true, &mut e);
    let number = clean_text(p, "number", 30, false, true, &mut e);
    let amount = clean_text(p, "amount", 20, false, true, &mut e);
    let amount = if !e.contains_key("amount") {
        match filter_amount(&amount) {
            Ok(n) => n,
            Err(msg) => {
                error(&mut e, "amount", msg);
                amount
            }
        }
    } else {
        amount
    };
    let output = choice(
        p,
        "output",
        &[("", "Screen"), ("print", "Print"), ("csv", "CSV")],
        &mut e,
    );
    let mut data = json!({"preset":preset,"start":start.unwrap_or_default(),"end":end.unwrap_or_default(),"state":state,"payee":payee,"number":number,"amount":amount,"output":output});
    if e.is_empty() {
        if preset.is_empty() && (!s(&data, "start").is_empty() || !s(&data, "end").is_empty()) {
            data["preset"] = json!("custom");
        }
        if !preset.is_empty() && preset != "custom" {
            let year = NaiveDate::from_ymd_opt(today.year(), 1, 1).unwrap();
            let month = NaiveDate::from_ymd_opt(today.year(), today.month(), 1).unwrap();
            let last =
                |a: NaiveDate, n| a.checked_add_months(Months::new(n)).unwrap() - Duration::days(1);
            let (a, b) = match preset.as_str() {
                "this_month" => (month, last(month, 1)),
                "last_month" => {
                    let b = month - Duration::days(1);
                    (NaiveDate::from_ymd_opt(b.year(), b.month(), 1).unwrap(), b)
                }
                "this_quarter" => {
                    let a =
                        NaiveDate::from_ymd_opt(today.year(), 3 * ((today.month() - 1) / 3) + 1, 1)
                            .unwrap();
                    (a, last(a, 3))
                }
                "this_year" => (year, NaiveDate::from_ymd_opt(today.year(), 12, 31).unwrap()),
                _ => (year, today),
            };
            data["start"] = json!(a.to_string());
            data["end"] = json!(b.to_string());
        } else if !s(&data, "start").is_empty()
            && !s(&data, "end").is_empty()
            && s(&data, "start") > s(&data, "end")
        {
            error(
                &mut e,
                "__all__",
                "The \"From\" date must not be after the \"Through\" date.",
            );
        }
    }
    let mut errors = vec![];
    for k in FILTERS.iter().copied().chain(["output", "__all__"]) {
        errors.extend(e.get(k).cloned().unwrap_or_default());
    }
    if !errors.is_empty() {
        for key in FILTERS.iter().copied().chain(["output"]) {
            data[key] = json!(get(p, key).unwrap_or(""));
        }
    }
    (data, errors)
}
fn aggregate_amount(v: &Value) -> AppResult<i128> {
    let invalid = || AppError::Invalid("Invalid register balance.".into());
    let Some(fb_money::Parsed::Finite(d)) =
        fb_money::parse_python_decimal(v.as_str().ok_or_else(invalid)?)
    else {
        return Err(invalid());
    };
    let n = d.cents_half_up().ok_or_else(invalid)?;
    if d.negative {
        n.checked_neg().ok_or_else(invalid)
    } else {
        Ok(n)
    }
}
pub(super) fn bank_register(
    app: &mut App,
    r: &Route,
    p: Posted<'_>,
    _path: &str,
) -> AppResult<(String, u16)> {
    let c = arg_company(r)?;
    let aid = &r.args["account_id"];
    let a = account(app, c, aid, true)?;
    let today = (app.today)();
    let (data, errors) = filter_data(p, today);
    let mut vm = ctx(app, r)?;
    let card = a.section_balance() == fb_core::model::Side::Credit;
    merge(
        &mut vm,
        json!({"account":account_vm(&a),"presets":PRESETS,"states":STATES,"controls":data,"errors":errors,"labels":if card{json!({"card":true})}else{json!({"card":false})}}),
    );
    if !errors.is_empty() {
        vm["invalid"] = json!(true);
        return Ok((draw(app, "banking/registers/register.html", &vm)?, 400));
    }
    let requested = get(p, "page")
        .map(|s| {
            python_integer(s)
                .map(|n| {
                    if (1..=1_000_000_000).contains(&n) {
                        n
                    } else {
                        0
                    }
                })
                .unwrap_or(1)
        })
        .unwrap_or(1);
    let o = app.open_mut()?;
    let mut f = data.clone();
    for key in ["start", "end", "amount"] {
        if s(&f, key).is_empty() {
            f[key] = Value::Null;
        }
    }
    let output = s(&data, "output");
    let export = !output.is_empty();
    let mut result = banking::build_on(
        &mut o.book,
        o.owner,
        c,
        aid,
        &f,
        if export { 1 } else { requested },
        if export { 2000 } else { 50 },
        &today.to_string(),
    )?;
    decorate_rows(app, c, result["rows"].as_array_mut().unwrap())?;
    let total = result["total"].as_u64().unwrap() as usize;
    if export && total > 2000 {
        vm["invalid"] = json!(true);
        vm["errors"] = json!([format!(
            "These filters match {total} lines. Print or export at most 2000 at a time: narrow the dates and try again."
        )]);
        return Ok((draw(app, "banking/registers/register.html", &vm)?, 400));
    }
    if output == "csv" {
        return Ok((register_csv(&vm, &result, &data), 200));
    }
    if output == "print" {
        for k in ["rows","totals","labels","opening","closing","total"] { vm[k]=result[k].clone(); }
        vm["start"]=data["start"].clone();vm["end"]=data["end"].clone();
        let filters=filter_text(&data);
        vm["filtered"]=json!(!filters.is_empty());vm["filter_text"]=json!(filters);
        vm["generated"]=json!((app.now)().to_rfc3339());
        let query=register_query(&FILTERS.iter().map(|k|(k.to_string(),s(&data,k).into())).collect::<Vec<_>>(),false);
        vm["screen_url"]=json!(if query.is_empty(){_path.to_owned()}else{format!("{_path}?{query}")});
        let target=format!("{_path}?{}",urlencode(p));
        return super::printing::draw(app,"banking/registers/print.html",&mut vm,&target,"internal");
    }
    let number = result["page"].as_u64().unwrap() as usize;
    let last = total.div_ceil(50).max(1);
    vm["page"] = json!({"number":number,"paginator":{"count":total,"num_pages":last},"has_previous":number>1,"has_next":number<last,"previous_page_number":number.saturating_sub(1),"next_page_number":number+1});
    let filtered = ["state", "payee", "number", "amount"]
        .iter()
        .any(|k| !s(&data, k).is_empty());
    let forward = if number > 1 && !filtered {
        result["rows"]
            .as_array()
            .unwrap()
            .first()
            .map(|row| -> AppResult<String> {
                let balance = aggregate_amount(&row["balance"])?;
                let debit = aggregate_amount(&row["debit"])?;
                let credit = aggregate_amount(&row["credit"])?;
                let overflow = || AppError::Invalid("Register balance overflow.".into());
                let movement = debit
                    .checked_sub(credit)
                    .and_then(|n| n.checked_mul(if card { -1 } else { 1 }))
                    .ok_or_else(overflow)?;
                Ok(cash(balance.checked_sub(movement).ok_or_else(overflow)?))
            })
            .transpose()?
    } else {
        None
    };
    for k in ["rows", "totals", "labels", "opening", "closing"] {
        vm[k] = result[k].clone();
    }
    merge(
        &mut vm,
        json!({"start":s(&data,"start"),"end":s(&data,"end"),"forward":forward,"filtered":filtered,"query_string":register_query(&FILTERS.iter().map(|k|(k.to_string(),s(&data,k).into())).collect::<Vec<_>>(),false),"back_query":register_query(p,true)}),
    );
    vm["export_token"] = json!(token(app, &rec_url("register", c, aid)));
    vm["csrf_token"] = json!(token(app, &rec_url("registers:cleared", c, aid)));
    Ok((draw(app, "banking/registers/register.html", &vm)?, 200))
}

fn csv_row(out: &mut String, cells: impl IntoIterator<Item = String>) {
    csv_numeric(out, cells, &[]);
}
fn csv_numeric(out: &mut String, cells: impl IntoIterator<Item = String>, numeric: &[usize]) {
    let cells = cells
        .into_iter()
        .enumerate()
        .map(|(i, s)| {
            if numeric.contains(&i) {
                s
            } else {
                fb_core::reports::params::csv_text(&fb_core::card_numbers::mask_card_numbers(&s))
            }
        })
        .map(|s| {
            if s.contains([',', '"', '\r', '\n']) {
                format!("\"{}\"", s.replace('"', "\"\""))
            } else {
                s
            }
        })
        .collect::<Vec<_>>();
    out.push_str(&cells.join(","));
    out.push_str("\r\n");
}
fn filter_text(data: &Value) -> String {
    let mut parts = vec![];
    if let Some((_, label)) = STATES
        .iter()
        .find(|(v, _)| !v.is_empty() && *v == s(data, "state"))
    {
        parts.push(label.to_string());
    }
    for key in ["payee", "number", "amount"] {
        if !s(data, key).is_empty() {
            parts.push(format!("{key} \"{}\"", s(data, key)));
        }
    }
    parts.join(", ")
}
fn register_csv(vm: &Value, result: &Value, data: &Value) -> String {
    let mut out = String::new();
    let account = &vm["account"];
    let labels = &result["labels"];
    let totals = &result["totals"];
    let text = filter_text(data);
    let start = if s(data, "start").is_empty() {
        "Beginning"
    } else {
        s(data, "start")
    };
    let end = if s(data, "end").is_empty() {
        "Latest posting"
    } else {
        s(data, "end")
    };
    for (k, v) in [
        ("Report", "Register"),
        ("Company", s(&vm["company"], "name")),
        ("Account", s(account, "label")),
        ("Currency", s(&vm["company"], "currency")),
        ("From", start),
        ("Through", end),
        ("Filters", if text.is_empty() { "None" } else { &text }),
        ("Opening balance", s(result, "opening")),
        ("Ending balance", s(result, "closing")),
        ("Cleared balance", s(totals, "cleared")),
        (s(labels, "left_open"), s(totals, "left_open")),
        (s(labels, "right_open"), s(totals, "right_open")),
        (
            "Note",
            "Alpha: keep your real books in your current software. Posted lines only; balances count every posted line.",
        ),
    ] {
        let number = matches!(k, "Opening balance" | "Ending balance" | "Cleared balance")
            || k == s(labels, "left_open")
            || k == s(labels, "right_open");
        csv_numeric(
            &mut out,
            [k.into(), v.into()],
            if number { &[1] } else { &[] },
        );
    }
    out.push_str("\r\n");
    csv_row(
        &mut out,
        [
            "Date",
            "Number",
            "Type",
            "Payee / name",
            "Account",
            "Memo",
            s(labels, "left"),
            "Cleared",
            s(labels, "right"),
            "Balance",
            "Link",
        ]
        .map(str::to_owned),
    );
    for row in result["rows"].as_array().unwrap() {
        csv_numeric(
            &mut out,
            [
                s(row, "date"),
                s(row, "number"),
                s(row, "type"),
                s(row, "party"),
                s(row, "other"),
                s(row, "memo"),
                s(row, "left"),
                if row["state"] == "U" {
                    ""
                } else {
                    s(row, "state")
                },
                s(row, "right"),
                s(row, "balance"),
                &format!("fb://app{}", s(row, "url")),
            ]
            .map(str::to_owned),
            &[0, 6, 8, 9],
        );
    }
    out
}
fn iso_seconds(raw: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(raw)
        .map(|d| d.format("%Y-%m-%dT%H:%M:%S+00:00").to_string())
        .unwrap_or_default()
}
fn report_csv(vm: &Value) -> String {
    let mut out = String::new();
    let rec = &vm["rec"];
    let sides = vm["sides"].as_array().unwrap();
    let view = s(vm, "view");
    let title = format!("Reconciliation {view}");
    let completed = iso_seconds(s(rec, "completed_at"));
    for (k, v) in [
        ("Report", title.as_str()),
        ("Company", s(&vm["company"], "name")),
        ("Account", s(&vm["account"], "label")),
        ("Currency", s(&vm["company"], "currency")),
        ("Statement date", s(rec, "statement_date")),
        (
            "Status",
            if rec["status"] == "DONE" {
                "Reconciled"
            } else {
                "Undone"
            },
        ),
        ("Beginning balance", s(rec, "beginning_balance")),
        (s(&sides[0], "summary_label"), s(&sides[0], "signed")),
        (s(&sides[1], "summary_label"), s(&sides[1], "signed")),
        ("Cleared balance", s(rec, "ending_balance")),
        ("Statement ending balance", s(rec, "ending_balance")),
        ("Difference", "0.00"),
        ("Reconciled by", s(&rec["completed_by"], "get_username")),
        ("Reconciled at", completed.as_str()),
        (
            "Note",
            "Alpha: keep your real books in your current software.",
        ),
    ] {
        let number = matches!(
            k,
            "Beginning balance" | "Cleared balance" | "Statement ending balance" | "Difference"
        ) || sides.iter().any(|side| s(side, "summary_label") == k);
        csv_numeric(
            &mut out,
            [k.into(), v.into()],
            if number { &[1] } else { &[] },
        );
    }
    if rec["status"] == "UNDONE" {
        csv_row(
            &mut out,
            [
                "Undone".into(),
                format!(
                    "{} by {}: {}",
                    iso_seconds(s(rec, "undone_at")),
                    s(&rec["undone_by"], "get_username"),
                    s(rec, "undo_reason")
                ),
            ],
        );
    }
    out.push_str("\r\n");
    if view == "summary" {
        csv_row(&mut out, ["Section", "Items", "Amount"].map(str::to_owned));
        for side in sides {
            csv_row(
                &mut out,
                [
                    s(side, "label").into(),
                    side["count"].to_string(),
                    s(side, "total").into(),
                ],
            );
        }
        csv_numeric(
            &mut out,
            [
                "Register balance as of statement date".into(),
                "".into(),
                s(&vm["summary"], "register_balance").into(),
            ],
            &[2],
        );
    } else {
        csv_row(
            &mut out,
            [
                "Section",
                "Type",
                "Date",
                "Number",
                "Payee / name",
                "Memo",
                "Amount",
                "Link",
            ]
            .map(str::to_owned),
        );
        for side in sides {
            for group in side["groups"].as_array().unwrap() {
                for row in group["rows"].as_array().unwrap() {
                    csv_row(
                        &mut out,
                        [
                            s(side, "label"),
                            s(row, "type"),
                            s(row, "date"),
                            s(row, "number"),
                            s(row, "party"),
                            s(row, "memo"),
                            s(row, s(side, "key")),
                            &format!("fb://app{}", s(row, "url")),
                        ]
                        .map(str::to_owned),
                    );
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod validator_tests {
    use super::*;

    #[test]
    fn shown_parser_checks_size_count_grammar_and_python_decimal_digits() {
        let id = "00000000-0000-0000-0000-000000000001";
        assert!(shown("").unwrap().is_empty());
        assert_eq!(shown(&format!("{id}:９")).unwrap()[id], 9);
        let pair = format!("{id}:0");
        assert!(shown(&vec![pair.clone(); 900].join(",")).is_ok());
        assert_eq!(shown(&vec![pair; 901].join(",")).unwrap_err(), vec![RELOAD]);
        assert_eq!(shown(&"x".repeat(48 * 900 + 1)).unwrap_err(), vec![RELOAD]);
        for revision in ["999999999", "９９９９９９９９９"] {
            let maximum_pair=format!("{id}:{revision}");
            assert_eq!(shown(&vec![maximum_pair;900].join(",")).unwrap()[id],999999999);
        }
        for value in ["abc", "", "-1", "1000000001", "0000000000", "1.0"] {
            assert_eq!(shown(&format!("{id}:{value}")).unwrap_err(), vec![RELOAD]);
        }
    }
}
