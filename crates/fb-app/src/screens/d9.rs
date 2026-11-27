//! D9: literal port of the frozen `bankfeeds.views` / `bankfeeds.forms` HTTP boundary. Every rule lives in
//! `fb_core::bankfeeds`; network actions (connect, sync, forget) go only through `crate::plugins`.
//! Python's integer transaction pk is the book-wide `ordinal`, so row field names (`kind_<n>`) are the
//! same numbers Django prints. Reads with no core query yet are read-only screen queries here.
use super::d2::{checked, draw, error, put, token};
use super::*;
use fb_core::bankfeeds as bf;
use fb_core::{documents, py, store};
use fb_db::Query;
use fb_db::rusqlite::{OptionalExtension, params};

pub(super) mod assistant;
pub(super) mod attachments;
mod review;
pub(crate) mod rule_import;
use review::{review_fragments, review_page, run_review_post};
const ASSISTANT: [&str; 8] = ["assistant:page", "assistant:openai_start", "assistant:notice", "assistant:model", "assistant:models_refresh", "assistant:disconnect", "assistant:key", "assistant:read_attachments"];
type Errors = HashMap<String, Vec<String>>;
const REQUIRED: &str = "This field is required.";
const CHOICE: &str = "Select a valid choice. That choice is not one of the available choices.";
const NULL: &str = "Null characters are not allowed.";
const CREATE_URL: &str = "https://bridge.simplefin.org/simplefin/create";

pub(super) fn is_route(name: &str) -> bool {
    name.starts_with("bankfeeds:") || ASSISTANT.contains(&name) || ["attachments:remove", "attachments:add_document", "attachments:add_journal"].contains(&name)
}

fn base(app: &mut App, r: &Route, path: &str) -> AppResult<Value> {
    let mut vm = super::d6::ctx(app, r)?;
    put(&mut vm, "csrf_token", token(app, path));
    put(&mut vm, "secrets_ok", app.os_store_usable);
    put(&mut vm, "keyring_deferred", app.connection_keys == crate::ConnectionKeys::Deferred);
    put(&mut vm, "builtin", app.builtin());
    put(&mut vm, "keyring_required", crate::net::BANK_KEYRING_REQUIRED);
    Ok(vm)
}
fn actor(app: &mut App) -> AppResult<UserId> {
    Ok(app.open_mut()?.owner)
}
fn messages(e: FbError) -> AppResult<String> {
    match e {
        FbError::Validation(m) => Ok(m.join("; ")),
        other => Err(other.into()),
    }
}

/// Python `str(Decimal)` of whole cents (`-12.50`).
pub(crate) fn cents(v: i64) -> String {
    let sign = if v < 0 { "-" } else { "" };
    let a = v.unsigned_abs();
    format!("{sign}{}.{:02}", a / 100, a % 100)
}
/// Django's `date` filter on an aware datetime (`TIME_ZONE = 'UTC'`).
fn stamp(v: Option<&str>, format: &str) -> String {
    v.and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()).map(|d| d.with_timezone(&chrono::Utc).format(format).to_string()).unwrap_or_default()
}

/// Django CharField(strip=True): required, max length, then the null-character validator.
fn char_field(p: Posted<'_>, name: &str, max: usize, required: bool, e: &mut Errors) -> String {
    let value = py::py_strip(get(p, name).unwrap_or("")).to_owned();
    if value.is_empty() {
        if required {
            error(e, name, REQUIRED);
        }
        return value;
    }
    if py::py_len(&value) > max {
        error(e, name, py::max_length_message(max, &value));
    }
    if value.contains('\0') {
        error(e, name, NULL);
    }
    value
}
/// ModelChoiceField over `choices`: '' is empty; the pk lookup tolerates surrounding whitespace.
fn model_choice(p: Posted<'_>, name: &str, choices: &[(String, String)], required: bool, e: &mut Errors) -> Option<String> {
    let raw = get(p, name).unwrap_or("");
    if raw.is_empty() {
        if required {
            error(e, name, REQUIRED);
        }
        return None;
    }
    let key = py::py_strip(raw);
    if !key.is_empty() && choices.iter().any(|(v, _)| v == key) {
        Some(key.to_owned())
    } else {
        error(e, name, CHOICE);
        None
    }
}
fn bind_errors(fields: Vec<Field>, e: &Errors) -> Vec<Field> {
    fields.into_iter().map(|f| { let k = f.html_name(); f.errors(e.get(&k).cloned().unwrap_or_default()) }).collect()
}
fn account_label(a: &fb_core::model::Account) -> String {
    format!("{} · {}", a.code, a.name)
}

// --- list / sync -----------------------------------------------------------------------------------

fn overview(tx: &(impl Query + ?Sized), c: CompanyId) -> Result<(Vec<Value>, i64, i64), FbError> {
    let mut st = tx.prepare(
        "SELECT c.id,c.label,c.host,c.active,c.last_sync_at,c.last_sync_ok,c.last_sync_summary,c.needs_reconnect,
            (SELECT count(*) FROM bankfeeds_account f WHERE f.connection_id=c.id),
            (SELECT count(*) FROM bankfeeds_account f WHERE f.connection_id=c.id AND f.ledger_account_id IS NOT NULL)
         FROM bankfeeds_connection c WHERE c.company_id=?1 ORDER BY c.created_at,c.id",
    )?;
    let rows = st
        .query_map([c.to_db()], |r| {
            let last: Option<String> = r.get(4)?;
            let ok: Option<bool> = r.get(5)?;
            Ok(json!({"pk": r.get::<_, String>(0)?, "label": r.get::<_, String>(1)?, "host": r.get::<_, String>(2)?,
                "connected": r.get::<_, bool>(3)?, "last_sync_at": last, "last_sync_display": stamp(last.as_deref(), "%Y-%m-%d %H:%M"),
                "last_sync_ok": ok, "last_sync_failed": ok == Some(false), "last_sync_summary": r.get::<_, String>(6)?,
                "needs_reconnect": r.get::<_, bool>(7)?, "account_count": r.get::<_, i64>(8)?, "mapped_count": r.get::<_, i64>(9)?}))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let pending = tx.query_row(
        "SELECT count(*) FROM bankfeeds_transaction t JOIN bankfeeds_account f ON f.id=t.feed_account_id JOIN bankfeeds_connection c ON c.id=f.connection_id WHERE c.company_id=?1 AND t.status='NEW'",
        [c.to_db()],
        |r| r.get(0),
    )?;
    let rules = tx.query_row("SELECT count(*) FROM bankfeeds_rule WHERE company_id=?1", [c.to_db()], |r| r.get(0))?;
    Ok((rows, pending, rules))
}
fn list_page(app: &mut App, r: &Route, path: &str, notice: Option<(String, bool)>) -> AppResult<String> {
    let c = arg_company(r)?;
    let mut vm = base(app, r, path)?;
    let (mut connections, pending, rules) = app.open_mut()?.book.read(|tx| overview(tx, c))?;
    let gaps = app.open_mut()?.book.read(|tx| bf::sync_gaps(tx, c))?;
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
    let clock = app.open_mut()?.book.read(|tx| bf::clock_warning(tx, c, &now))?;
    put(&mut vm, "clock_warning", clock.map(|w| json!({"recorded": w.recorded, "now": w.now})));
    for conn in &mut connections {
        let sync = format!("/c/{}/bank-feeds/{}/sync/", c.to_db(), conn["pk"].as_str().unwrap_or(""));
        conn["sync_token"] = json!(token(app, &sync));
        // Dates a sync could not reach back to, until the user says they compared them with the statement.
        let mine: Vec<Value> = gaps.iter().filter(|g| conn["pk"] == g.connection_id.as_str()).map(|g| json!({"from": g.from, "to": g.to})).collect();
        if !mine.is_empty() {
            conn["gaps"] = json!(mine);
            let accounts = format!("/c/{c}/bank-feeds/{}/accounts/", conn["pk"].as_str().unwrap_or(""));
            conn["gap_token"] = json!(token(app, &accounts));
        }
    }
    put(&mut vm, "connections", connections);
    put(&mut vm, "pending", pending);
    put(&mut vm, "rules", rules);
    put(&mut vm, "messages", std::mem::take(&mut app.messages));
    if let Some((text, bad)) = notice {
        put(&mut vm, "notice", text);
        put(&mut vm, "notice_error", bad);
    }
    draw(app, "bankfeeds/list.html", &vm)
}

fn connection(app: &mut App, r: &Route) -> AppResult<Value> {
    let c = arg_company(r)?;
    let id = r.args.get("connection_id").cloned().unwrap_or_default();
    let owner = actor(app)?;
    let row = app.open_mut()?.book.read(|tx| {
        fb_core::services::access(tx, owner, c)?;
        tx.query_row(
            "SELECT id,label,host,active,last_sync_at,last_sync_ok,last_sync_summary,needs_reconnect FROM bankfeeds_connection WHERE company_id=?1 AND id=?2",
            params![c.to_db(), id],
            |r| {
                let last: Option<String> = r.get(4)?;
                Ok(json!({"pk": r.get::<_, String>(0)?, "label": r.get::<_, String>(1)?, "host": r.get::<_, String>(2)?, "connected": r.get::<_, bool>(3)?,
                    "last_sync_at": last, "last_sync_display": stamp(last.as_deref(), "%Y-%m-%d %H:%M"), "last_sync_ok": r.get::<_, Option<bool>>(5)?,
                    "last_sync_summary": r.get::<_, String>(6)?, "needs_reconnect": r.get::<_, bool>(7)?}))
            },
        )
        .optional()
        .map_err(FbError::from)
    })?;
    row.ok_or_else(|| AppError::NotFound("connection".into()))
}

// --- connect ---------------------------------------------------------------------------------------

fn connect_page(app: &mut App, r: &Route, path: &str, posted: Option<Posted<'_>>, e: &Errors) -> AppResult<String> {
    let mut vm = base(app, r, path)?;
    let label = match posted { Some(p) => get(p, "label").map(str::to_owned), None => Some(bf::DEFAULT_LABEL.to_owned()) };
    // Worker item 38: the suggested name is selected when the box is clicked, so typing replaces it.
    let mut name = Field::new("label", "Connection name", Widget::Text).attr("maxlength", "120");
    if label.as_deref() == Some(bf::DEFAULT_LABEL) {
        name = name.attr("data-select-on-focus", "");
    }
    let fields = vec![
        name.required().value_opt(label),
        // A pasted token is never echoed back into the page.
        Field::new("setup_token", "Setup token", Widget::Textarea { cols: 40, rows: 4 }).attr("autocomplete", "off").attr("spellcheck", "false").required().value_opt(posted.map(|_| String::new())),
    ];
    put(&mut vm, "form", forms::bind(&bind_errors(fields, e), &e.get("__all__").cloned().unwrap_or_default()));
    put(&mut vm, "create_url", CREATE_URL);
    draw(app, "bankfeeds/connect.html", &vm)
}
fn submit_connect(app: &mut App, r: &Route, path: &str, p: Posted<'_>) -> AppResult<Outcome> {
    let c = arg_company(r)?;
    let mut e = Errors::new();
    let label = char_field(p, "label", 120, true, &mut e);
    let setup = char_field(p, "setup_token", 4096, true, &mut e);
    if e.is_empty() {
        match crate::plugins::bankfeed_connect(app, c, &setup, &label)? {
            Ok(id) => return Ok(Outcome::Redirect { location: format!("/c/{}/bank-feeds/{id}/accounts/", c.to_db()) }),
            Err(message) => error(&mut e, "__all__", message),
        }
    }
    Ok(Outcome::Invalid { html: connect_page(app, r, path, Some(p), &e)? })
}

// --- accounts (mapping) ----------------------------------------------------------------------------

fn feeds(tx: &(impl Query + ?Sized), connection_id: &str) -> Result<Vec<Value>, FbError> {
    let mut st = tx.prepare(
        "SELECT f.id,f.name,f.org_name,f.remote_id,f.currency,f.balance,f.balance_at,f.ledger_account_id,
            (SELECT count(*) FROM bankfeeds_transaction t WHERE t.feed_account_id=f.id AND t.status='NEW'),
            (SELECT a.code || ' · ' || a.name FROM ledger_account a WHERE a.id=f.ledger_account_id),
            f.import_state,f.import_from_date,f.active
         FROM bankfeeds_account f WHERE f.connection_id=?1 ORDER BY f.org_name COLLATE fb_text,f.name COLLATE fb_text,f.first_seen_at,f.id",
    )?;
    Ok(st
        .query_map([connection_id], |r| {
            let (name, org): (String, String) = (r.get(1)?, r.get(2)?);
            let at: Option<String> = r.get(6)?;
            Ok(json!({"pk": r.get::<_, String>(0)?, "name": name, "org_name": org, "label": if org.is_empty() { name.clone() } else { format!("{org} · {name}") },
                "remote_id": r.get::<_, String>(3)?, "currency": r.get::<_, String>(4)?, "balance": r.get::<_, Option<i64>>(5)?.map(cents),
                "balance_at": at, "balance_at_display": stamp(at.as_deref(), "%Y-%m-%d"), "ledger_account_id": r.get::<_, Option<String>>(7)?, "new_count": r.get::<_, i64>(8)?, "mapped_label": r.get::<_, Option<String>>(9)?,
                "import_state": r.get::<_, String>(10)?, "import_from_date": r.get::<_, Option<String>>(11)?, "active": r.get::<_, bool>(12)?}))
        })?
        .collect::<Result<Vec<_>, _>>()?)
}
// New selections in the mapping form are limited to bank and card accounts. The core
// mapping service also supports historical asset and liability mappings.
fn mapping_choices(app: &mut App, c: CompanyId) -> AppResult<Vec<(String, String)>> {
    let open = app.open_mut()?;
    let accounts = bf::mappable_accounts(&mut open.book, open.owner, c)?;
    Ok(accounts.iter()
        .filter(|a| matches!(a.detail_type.as_str(), "BANK" | "CREDIT_CARD"))
        .map(|a| (a.id.to_db(), account_label(a)))
        .collect())
}
fn accounts_page(app: &mut App, r: &Route, path: &str, posted: Option<Posted<'_>>, e: &Errors, errors: &[String]) -> AppResult<String> {
    let c = arg_company(r)?;
    let conn = connection(app, r)?;
    let mut vm = base(app, r, path)?;
    let conn_id = conn["pk"].as_str().unwrap_or("").to_owned();
    let rows = app.open_mut()?.book.read(|tx| feeds(tx, &conn_id))?;
    let mut choices = vec![(String::new(), "— not mapped —".to_owned())];
    choices.extend(mapping_choices(app, c)?);
    choices.push(("__new__".into(), "Add new account…".into()));
    let mut fields = vec![];
    let mut pairs = vec![];
    for feed in &rows {
        let name = format!("map_{}", feed["pk"].as_str().unwrap_or(""));
        let value = match posted { Some(p) => get(p, &name).map(str::to_owned), None => feed["ledger_account_id"].as_str().map(str::to_owned) };
        let mut row_choices = choices.clone();
        if let Some(old) = feed["ledger_account_id"].as_str() && !choices.iter().any(|(id, _)| id == old) {
            row_choices.push((old.to_owned(), format!("{} (existing mapping)", feed["mapped_label"].as_str().unwrap_or(""))));
        }
        let f = Field::new(&name, feed["label"].as_str().unwrap_or(""), Widget::Select { choices: row_choices }).value_opt(value);
        let f = f.clone().errors(e.get(&name).cloned().unwrap_or_default());
        let feed_id = feed["pk"].as_str().unwrap_or("");
        let creating = f.value.as_deref() == Some("__new__");
        let name_key = format!("name_{feed_id}");
        let type_key = format!("type_{feed_id}");
        let suggested = feed["name"].as_str().filter(|s| !s.is_empty()).unwrap_or("New bank account");
        let suggested = py::py_truncate(suggested, 120);
        let name = posted.and_then(|p| get(p, &name_key)).unwrap_or(&suggested);
        let kind = posted.and_then(|p| get(p, &type_key)).unwrap_or("");
        let new_name = Field::new(&name_key, "Account name", Widget::Text).attr("maxlength", "120").required().value(name)
            .errors(e.get(&name_key).cloned().unwrap_or_default());
        let new_type = Field::new(&type_key, "Account type", Widget::Select { choices: vec![
            ("".into(), "Choose account type…".into()), ("CHECKING".into(), "Checking".into()),
            ("SAVINGS".into(), "Savings".into()), ("CREDIT_CARD".into(), "Credit card".into())
        ] }).required().value(kind).errors(e.get(&type_key).cloned().unwrap_or_default());
        let import_key = format!("import_from_{feed_id}");
        let import_date = Field::new(&import_key, "Import from", Widget::Date)
            .value_opt(posted.and_then(|p| get(p, &import_key)).or_else(|| feed["import_from_date"].as_str()).map(str::to_owned))
            .errors(e.get(&import_key).cloned().unwrap_or_default());
        let configure = feed["active"] == true && conn["connected"] == true
            && conn["needs_reconnect"] != true && feed["import_state"] != "ACTIVE";
        let expected_mapping = posted.and_then(|p| get(p, &format!("expected_map_{feed_id}"))).or(feed["ledger_account_id"].as_str()).unwrap_or("");
        pairs.push(json!([feed, forms::BoundField::from(&f), forms::BoundField::from(&new_name),
            forms::BoundField::from(&new_type), creating, configure, forms::BoundField::from(&import_date), expected_mapping]));
        fields.push(f);
    }
    let mut conn = conn;
    conn["sync_token"] = json!(token(app, &format!("/c/{}/bank-feeds/{conn_id}/sync/", c.to_db())));
    put(&mut vm, "connection", conn);
    put(&mut vm, "rows", pairs);
    put(&mut vm, "form", forms::bind(&fields, &[]));
    put(&mut vm, "errors", errors);
    put(&mut vm, "messages", std::mem::take(&mut app.messages));
    let currency = vm["company"]["currency"].clone();
    put(&mut vm, "company_currency", currency);
    draw(app, "bankfeeds/accounts.html", &vm)
}
fn submit_accounts(app: &mut App, r: &Route, path: &str, p: Posted<'_>) -> AppResult<Outcome> {
    let c = arg_company(r)?;
    let conn = connection(app, r)?;
    let owner = actor(app)?;
    let choices = mapping_choices(app, c)?;
    let conn_id = conn["pk"].as_str().unwrap_or("").to_owned();
    let rows = app.open_mut()?.book.read(|tx| feeds(tx, &conn_id))?;
    let mut e = Errors::new();
    let mut wanted = vec![];
    let mut import_settings = vec![];
    let mut expected = HashMap::new();
    for feed in &rows {
        let feed_id = feed["pk"].as_str().unwrap_or("");
        let name = format!("map_{feed_id}");
        let import_key = format!("import_from_{feed_id}");
        if let Some(date) = get(p, &import_key) {
            let state = get(p, &format!("import_state_{feed_id}")).unwrap_or("");
            let expected = get(p, &format!("import_expected_{feed_id}")).unwrap_or("");
            let valid = |date: &str| date.is_empty() || fb_db::dates::parse_iso_date(date).as_deref() == Some(date);
            if !valid(date) {
                error(&mut e, &import_key, "Enter a valid date.");
            } else if !["PENDING", "READY"].contains(&state) || !valid(expected) {
                error(&mut e, &import_key, "Reload Bank feed accounts and enter the import start again.");
            } else {
                import_settings.push(bf::ImportSettings {
                    feed_account_id: feed_id.to_owned(), expected_state: state.to_owned(),
                    expected_date: (!expected.is_empty()).then(|| expected.to_owned()),
                    import_from_date: (!date.is_empty()).then(|| date.to_owned()),
                });
            }
        }
        if get(p, &name).is_none() {
            continue;
        }
        let target = if get(p, &name) == Some("__new__") {
            let name_key = format!("name_{feed_id}");
            let type_key = format!("type_{feed_id}");
            let name = char_field(p, &name_key, 120, true, &mut e);
            let kind = match get(p, &type_key) {
                Some("CHECKING") => bf::NewAccountKind::Checking,
                Some("SAVINGS") => bf::NewAccountKind::Savings,
                Some("CREDIT_CARD") => bf::NewAccountKind::CreditCard,
                _ => { error(&mut e, &type_key, "Choose Checking, Savings or Credit card."); bf::NewAccountKind::Checking }
            };
            bf::MappingTarget::New { name, kind }
        } else {
            let id = if let Some(old) = feed["ledger_account_id"].as_str() && get(p, &name) == Some(old) {
                Some(old.to_owned())
            } else {
                model_choice(p, &name, &choices, false, &mut e)
            };
            bf::MappingTarget::Existing(id.as_deref().and_then(AccountId::parse))
        };
        expected.insert(feed_id.to_owned(), get(p, &format!("expected_map_{feed_id}")).unwrap_or("").to_owned());
        wanted.push((feed_id.to_owned(), target));
    }
    let mut errors = vec![];
    if e.is_empty() {
        match bf::save_mapping_form(&mut app.open_mut()?.book, owner, c, &conn_id, &wanted, &import_settings, &expected, &bf::SecretSet::none()) {
            Ok(()) => {
                // A replacement connection's first download is staged before it is mapped: say now how many
                // of its transactions are already in the books, so none is recorded twice.
                app.messages.push(if import_settings.is_empty() { "Bank account mappings saved." } else {
                    "Bank account mappings and import settings saved. Sync now to download transactions."
                }.into());
                let recorded = app.open_mut()?.book.read(|tx| bf::already_recorded_count(tx, c))?;
                if recorded > 0 {
                    app.messages.push(format!(
                        "{recorded} downloaded {} already recorded from an earlier bank connection. Review transactions shows {} as “Already recorded”; link {} to the existing entries instead of recording {} again.",
                        if recorded == 1 { "transaction looks" } else { "transactions look" },
                        if recorded == 1 { "it" } else { "them" },
                        if recorded == 1 { "it" } else { "them" },
                        if recorded == 1 { "it" } else { "them" }
                    ));
                }
                return Ok(Outcome::Redirect { location: format!("/c/{}/bank-feeds/", c.to_db()) });
            }
            Err(err) => errors.push(messages(err)?),
        }
    }
    Ok(Outcome::Invalid { html: accounts_page(app, r, path, Some(p), &e, &errors)? })
}

// --- forget ----------------------------------------------------------------------------------------

fn forget_page(app: &mut App, r: &Route, path: &str) -> AppResult<String> {
    let conn = connection(app, r)?;
    let mut vm = base(app, r, path)?;
    put(&mut vm, "connection", conn);
    draw(app, "bankfeeds/forget.html", &vm)
}

// --- rules -----------------------------------------------------------------------------------------

fn sign_display(sign: &str) -> &'static str {
    match sign {
        "IN" => "Money in",
        "OUT" => "Money out",
        _ => "Money in or out",
    }
}
fn rules_page(app: &mut App, r: &Route, path: &str, error_text: Option<String>) -> AppResult<String> {
    let c = arg_company(r)?;
    let owner = actor(app)?;
    let mut vm = base(app, r, path)?;
    if app.backups.rule_import.as_ref().is_some_and(|p|p.company==c && p.at.elapsed().as_secs()>=1800) {
        app.backups.rule_import=None;app.messages.push("The import preview expired. Choose the file again.".into());
    }
    if app.backups.rule_import.as_ref().is_some_and(|p|p.company==c) {return rule_import::page(app,r,path,error_text);}
    let rules = bf::all_rules(&mut app.open_mut()?.book, owner, c)?;
    let rows = app.open_mut()?.book.read(|tx| -> Result<Vec<Value>, FbError> {
        rules
            .iter()
            .map(|rule| {
                let a = AccountId::parse(&rule.account_id).map(|id| store::account(tx, id)).transpose()?.flatten();
                let id = rule.policy.as_ref().map(|p| p.scope.as_str()).filter(|id| *id != "ALL")
                    .or(rule.feed_account_id.as_deref());
                let scope = match id {
                    Some(id) => bf::feed(tx,id)?.map(|f| f.label()).unwrap_or_else(|| "Unavailable feed".into()),
                    None => "Any feed account".into(),
                };
                Ok(json!({"pk": rule.id, "name": rule.name, "match_text": rule.match_text, "sign_display": sign_display(&rule.sign), "scope_label":scope,
                    "amount_min": rule.amount_min.map(cents), "amount_max": rule.amount_max.map(cents),
                    "account": {"code": a.as_ref().map(|a| a.code.clone()), "name": a.as_ref().map(|a| a.name.clone())},
                    "payee_name": rule.payee_name, "active": rule.active, "policy":rule.policy, "auto_post":rule.policy.as_ref().is_some_and(|p|p.auto_post)}))
            })
            .collect()
    })?;
    put(&mut vm, "rules", rows);
    put(&mut vm, "has_auto_post", rules.iter().any(|r|r.policy.as_ref().is_some_and(|p|p.auto_post)));
    put(&mut vm, "rule_notices", std::mem::take(&mut app.messages));
    put(&mut vm, "error", error_text);
    draw(app, "bankfeeds/rules.html", &vm)
}
fn submit_rules(app: &mut App, r: &Route, path: &str, p: Posted<'_>) -> AppResult<Outcome> {
    if get(p,"import_action").is_some() {return rule_import::submit(app,r,path,p);}
    if let Some(id)=get(p,"test_rule") {return rule_import::test(app,r,path,id);}
    let c = arg_company(r)?;
    let owner = actor(app)?;
    let present = |k: &str| get(p, k).filter(|v| !v.is_empty());
    if let Some(id) = present("auto_post_off") {
        let result = bf::turn_off_imported_auto_post(&mut app.open_mut()?.book, owner, c, (id != "ALL").then_some(id), &bf::SecretSet::none());
        return match result {
            Ok(count) => {
                app.messages.push(format!("Auto-post turned off for {count} imported rules. Active rules still suggest categories; transactions require review."));
                Ok(Outcome::Redirect { location: format!("/c/{}/bank-feeds/rules/", c.to_db()) })
            }
            Err(e) => Ok(Outcome::Invalid { html: rules_page(app, r, path, Some(messages(e)?))? }),
        };
    }
    let none = bf::SecretSet::none();
    let book = &mut app.open_mut()?.book;
    let result = if let Some(v) = present("move_up").or(present("move_down")) {
        let direction = if present("move_up").is_some() { "up" } else { "down" };
        bf::move_rule(book, owner, c, py::py_strip(v), &json!(direction), &none).map(|_| ())
    } else if let Some(v) = present("deactivate").or(present("activate")) {
        bf::set_rule_active(book, owner, c, py::py_strip(v), &json!(present("activate").is_some()), &none).map(|_| ())
    } else {
        Err(FbError::validation("Choose a rule action."))
    };
    match result {
        Ok(()) => Ok(Outcome::Redirect { location: format!("/c/{}/bank-feeds/rules/", c.to_db()) }),
        Err(e) => {
            let text = messages(e)?;
            Ok(Outcome::Invalid { html: rules_page(app, r, path, Some(text))? })
        }
    }
}

// --- rule form -------------------------------------------------------------------------------------

fn rule_at(app: &mut App, r: &Route) -> AppResult<Option<bf::Rule>> {
    let Some(id) = r.args.get("rule_id") else { return Ok(None) };
    let c = arg_company(r)?;
    let owner = actor(app)?;
    let rules = bf::all_rules(&mut app.open_mut()?.book, owner, c)?;
    rules.into_iter().find(|x| &x.id == id).map(Some).ok_or_else(|| AppError::NotFound("rule".into()))
}
fn rule_choices(app: &mut App, r: &Route, rule: Option<&bf::Rule>) -> AppResult<Vec<(String, String)>> {
    let c = arg_company(r)?;
    let owner = actor(app)?;
    let mut accounts = bf::rule_accounts(&mut app.open_mut()?.book, owner, c)?;
    if let Some(rule) = rule
        && !accounts.iter().any(|a| a.id.to_db() == rule.account_id)
        && let Some(id) = AccountId::parse(&rule.account_id)
        && let Some(a) = app.open_mut()?.book.read(|tx| store::account(tx, id))?
    {
        // Keep showing an account that became ineligible; the service decides.
        accounts.push(a);
        accounts.sort_by(|x, y| (x.code.as_str(), x.id.to_db()).cmp(&(y.code.as_str(), y.id.to_db())));
    }
    Ok(accounts.iter().map(|a| (a.id.to_db(), account_label(a))).collect())
}
fn rule_feed_choices(app: &mut App, c: CompanyId, retained: Option<&str>) -> AppResult<Vec<(String, String)>> {
    Ok(app.open_mut()?.book.read(|tx| -> Result<_, FbError> {
        let mut st = tx.prepare(
            "SELECT f.id,f.name,f.org_name,f.active AND c.active FROM bankfeeds_account f
              JOIN bankfeeds_connection c ON c.id=f.connection_id WHERE c.company_id=?1
              AND ((f.active=1 AND c.active=1) OR f.id=?2)
              ORDER BY f.org_name COLLATE fb_text,f.name COLLATE fb_text,f.id",
        )?;
        Ok(st.query_map(params![c.to_db(), retained], |r| {
            let (id, name, org, active): (String, String, String, bool) = (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?);
            let label = if org.is_empty() { name } else { format!("{org} · {name}") };
            Ok((id, if active { label } else { format!("{label} (disconnected)") }))
        })?.collect::<Result<_, _>>()?)
    })?)
}

fn rule_page(app: &mut App, r: &Route, path: &str, params: Posted<'_>, posted: Option<Posted<'_>>, e: &Errors) -> AppResult<String> {
    let c = arg_company(r)?;
    let rule = rule_at(app, r)?;
    let choices = rule_choices(app, r, rule.as_ref())?;
    let mut initial: HashMap<&str, Option<String>> = HashMap::new();
    let mut active_initial = true;
    if let Some(rule) = &rule {
        initial.insert("name", Some(rule.name.clone()));
        initial.insert("match_text", Some(rule.match_text.clone()));
        initial.insert("sign", Some(rule.sign.clone()));
        initial.insert("amount_min", rule.amount_min.map(cents));
        initial.insert("amount_max", rule.amount_max.map(cents));
        initial.insert("account", Some(rule.account_id.clone()));
        initial.insert("feed_account", rule.feed_account_id.clone());
        initial.insert("payee_name", Some(rule.payee_name.clone()));
        active_initial = rule.active;
    } else {
        initial.extend(review::rule_initial(app, c, params, &choices)?);
    }
    let value = |name: &str| -> Option<String> {
        match posted {
            Some(p) => get(p, name).map(str::to_owned),
            None => initial.get(name).cloned().flatten().or_else(|| (name == "sign").then(|| "ANY".to_owned())),
        }
    };
    let mut account_choices = vec![(String::new(), "---------".to_owned())];
    account_choices.extend(choices);
    const DISCONNECTED_SCOPE: &str = "__choose_scope__";
    let mut feed_choices = vec![(String::new(), "Any feed account".to_owned())];
    feed_choices.extend(rule_feed_choices(app, c, rule.as_ref().and_then(|r| r.feed_account_id.as_deref()))?);
    let mut feed_value = value("feed_account");
    if rule.is_none()
        && let Some(selected) = feed_value.as_deref().filter(|id| !id.is_empty())
        && !feed_choices.iter().any(|(id, _)| id == selected)
    {
        feed_choices.insert(
            0,
            (
                DISCONNECTED_SCOPE.into(),
                "Choose a feed scope (source feed is disconnected)".into(),
            ),
        );
        if posted.is_none() {
            feed_value = Some(DISCONNECTED_SCOPE.into());
        }
    }
    let active = match posted { Some(p) => checked(p, "active"), None => active_initial };
    let fields = vec![
        Field::new("name", "Rule name", Widget::Text).attr("maxlength", "120").required().value_opt(value("name")),
        Field::new("match_text", "Bank description contains", Widget::Text).attr("maxlength", "200").required().value_opt(value("match_text")),
        Field::new("feed_account", "Applies to", Widget::Select { choices: feed_choices }).value_opt(feed_value),
        Field::new("sign", "Direction", Widget::Select { choices: vec![("ANY".into(), "Money in or out".into()), ("IN".into(), "Money in".into()), ("OUT".into(), "Money out".into())] }).required().value_opt(value("sign")),
        Field::new("amount_min", "Minimum amount", Widget::Text).attr("inputmode", "decimal").attr("maxlength", "32").value_opt(value("amount_min")),
        Field::new("amount_max", "Maximum amount", Widget::Text).attr("inputmode", "decimal").attr("maxlength", "32").value_opt(value("amount_max")),
        Field::new("account", "Category (account)", Widget::Select { choices: account_choices }).required().value_opt(value("account")),
        Field::new("payee_name", "Payee name (optional)", Widget::Text).attr("maxlength", "120").value_opt(value("payee_name")),
        Field::new("active", "Active", Widget::Checkbox { checked: active }),
    ];
    let mut vm = base(app, r, path)?;
    put(&mut vm, "form", forms::bind(&bind_errors(fields, e), &e.get("__all__").cloned().unwrap_or_default()));
    put(&mut vm, "expected_snapshot", posted.and_then(|p| get(p, "expected_snapshot")).map(str::to_owned).or_else(|| rule.as_ref().map(bf::Rule::edit_snapshot)));
    put(&mut vm, "rule", rule.map(|x| json!({"pk": x.id})));
    draw(app, "bankfeeds/rule_form.html", &vm)
}
fn submit_rule(app: &mut App, r: &Route, path: &str, p: Posted<'_>) -> AppResult<Outcome> {
    let c = arg_company(r)?;
    let rule = rule_at(app, r)?;
    let choices = rule_choices(app, r, rule.as_ref())?;
    let mut e = Errors::new();
    let name = char_field(p, "name", 120, true, &mut e);
    let match_text = char_field(p, "match_text", 200, true, &mut e);
    let sign = get(p, "sign").unwrap_or("").to_owned();
    if sign.is_empty() {
        error(&mut e, "sign", REQUIRED);
    } else if !["ANY", "IN", "OUT"].contains(&sign.as_str()) {
        error(&mut e, "sign", format!("Select a valid choice. {sign} is not one of the available choices."));
    }
    let amount_min = char_field(p, "amount_min", 32, false, &mut e);
    let amount_max = char_field(p, "amount_max", 32, false, &mut e);
    let account = model_choice(p, "account", &choices, true, &mut e);
    let prior_scope = rule.as_ref().and_then(|r| r.feed_account_id.as_deref());
    let feed_choices = rule_feed_choices(app, c, prior_scope)?;
    let feed = match get(p, "feed_account") {
        None if rule.is_some() => prior_scope.map(str::to_owned),
        None | Some("__choose_scope__") => {
            error(&mut e, "feed_account", "Choose Any feed account or an active feed account.");
            None
        }
        Some(_) => model_choice(p, "feed_account", &feed_choices, false, &mut e),
    };
    let payee_name = char_field(p, "payee_name", 120, false, &mut e);
    let active = checked(p, "active");
    if e.is_empty() {
        let owner = actor(app)?;
        let values = json!({"expected_snapshot":rule.as_ref().map(|_|get(p, "expected_snapshot").unwrap_or("")), "rule_id": rule.as_ref().map(|x| x.id.clone()), "name": name, "match_text": match_text, "sign": sign,
            "amount_min": if amount_min.is_empty() { Value::Null } else { json!(amount_min) },
            "amount_max": if amount_max.is_empty() { Value::Null } else { json!(amount_max) },
            "account_id": account, "feed_account_id": feed, "payee_name": payee_name, "active": active});
        match bf::save_rule(&mut app.open_mut()?.book, owner, c, &values, &bf::SecretSet::none()) {
            Ok(_) => {
                app.messages.push("Rule saved.".into());
                return Ok(Outcome::Redirect { location: format!("/c/{}/bank-feeds/rules/", c.to_db()) });
            }
            Err(err) => error(&mut e, "__all__", messages(err)?),
        }
    }
    Ok(Outcome::Invalid { html: rule_page(app, r, path, &[], Some(p), &e)? })
}

// --- dispatch --------------------------------------------------------------------------------------

pub(super) fn render(app: &mut App, r: &Route, p: Posted<'_>, path: &str) -> AppResult<(String, u16)> {
    if r.name.starts_with("attachments:") {
        // POST only (Django answers GET with 405).
        arg_company(r)?;
        return Ok((String::new(), 405));
    }
    if r.name.starts_with("assistant:") {
        return assistant::render(app, r, path);
    }
    Ok(match r.name.as_str() {
        "bankfeeds:list" => (list_page(app, r, path, None)?, 200),
        "bankfeeds:connect" => (connect_page(app, r, path, None, &Errors::new())?, 200),
        "bankfeeds:accounts" => (accounts_page(app, r, path, None, &Errors::new(), &[])?, 200),
        "bankfeeds:forget" => (forget_page(app, r, path)?, 200),
        "bankfeeds:rules" => (rules_page(app, r, path, None)?, 200),
        "bankfeeds:rule_new" | "bankfeeds:rule_edit" => (rule_page(app, r, path, p, None, &Errors::new())?, 200),
        "bankfeeds:review" => {
            let full = if p.is_empty() { path.to_owned() } else { format!("{path}?{}", urlencode(p)) };
            review_page(app, r, path, &full, p, None)?
        }
        // sync is POST only (Django answers GET with 405).
        _ => {
            connection(app, r)?;
            (String::new(), 405)
        }
    })
}

pub(super) fn submit(app: &mut App, r: &Route, path: &str, p: Posted<'_>) -> AppResult<Outcome> {
    if r.name.starts_with("attachments:") {
        return attachments::submit(app, r, p);
    }
    if r.name.starts_with("assistant:") {
        return assistant::submit(app, r, p);
    }
    let c = arg_company(r)?;
    match r.name.as_str() {
        "bankfeeds:connect" => submit_connect(app, r, path, p),
        "bankfeeds:forget" => {
            let conn = connection(app, r)?;
            crate::plugins::bankfeed_forget(app, c, conn["pk"].as_str().unwrap_or(""))?;
            Ok(Outcome::Redirect { location: format!("/c/{}/bank-feeds/", c.to_db()) })
        }
        "bankfeeds:accounts" | "bankfeeds:sync" if get(p, "action") == Some("gap_reviewed") => {
            let conn = connection(app, r)?;
            let owner = actor(app)?;
            bf::review_sync_gap(&mut app.open_mut()?.book, owner, c, conn["pk"].as_str().unwrap_or(""), &bf::SecretSet::none())?;
            Ok(Outcome::Redirect { location: format!("/c/{}/bank-feeds/", c.to_db()) })
        }
        "bankfeeds:accounts" => submit_accounts(app, r, path, p),
        "bankfeeds:sync" if get(p, "action") == Some("discover") => {
            let conn = connection(app, r)?;
            let id = conn["pk"].as_str().unwrap_or("");
            let accounts_path = format!("/c/{c}/bank-feeds/{id}/accounts/");
            match crate::plugins::bankfeed_discover(app, c, id)? {
                Ok(summary) => {
                    app.messages.push(summary);
                    Ok(Outcome::Redirect { location: accounts_path })
                }
                Err(message) => {
                    let route = resolve(&accounts_path).ok_or_else(|| AppError::NotFound("bankfeeds:accounts".into()))?;
                    Ok(Outcome::Invalid { html: accounts_page(app, &route, &accounts_path, None, &Errors::new(), &[message])? })
                }
            }
        }
        "bankfeeds:sync" => {
            let conn = connection(app, r)?;
            let label = conn["label"].as_str().unwrap_or("").to_owned();
            let (notice, status) = match crate::plugins::bankfeed_sync(app, c, conn["pk"].as_str().unwrap_or(""))? {
                Ok(summary) => (format!("{label}: {summary}"), 200),
                Err(message) => (format!("{label}: {message}"), 400),
            };
            let list = resolve(&format!("/c/{}/bank-feeds/", c.to_db())).ok_or_else(|| AppError::NotFound("bankfeeds:list".into()))?;
            let html = list_page(app, &list, path, Some((notice, status != 200)))?;
            Ok(Outcome::Page { html, status })
        }
        "bankfeeds:rules" => submit_rules(app, r, path, p),
        "bankfeeds:rule_new" | "bankfeeds:rule_edit" => submit_rule(app, r, path, p),
        "bankfeeds:review" => {
            let post = run_review_post(app, c, p)?;
            if get(p, "inline") == Some("1") {
                return review_fragments(app, r, path, p, post);
            }
            let filters: Vec<(String, String)> = ["status", "account", "q", "page"].into_iter()
                .filter_map(|key| get(p, &format!("review_{key}")).map(|v| (key.to_owned(), v.to_owned()))).collect();
            let full = format!("{path}?{}", urlencode(&filters));
            let (html, status) = review_page(app, r, path, &full, &filters, Some(post))?;
            Ok(Outcome::Page { html, status })
        }
        // The list is GET only.
        _ => {
            let _ = base(app, r, path)?;
            Ok(Outcome::Page { html: String::new(), status: 405 })
        }
    }
}
