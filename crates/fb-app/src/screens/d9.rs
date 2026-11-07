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
const ASSISTANT: [&str; 8] = ["assistant:page", "assistant:openai_start", "assistant:notice", "assistant:model", "assistant:models_refresh", "assistant:disconnect", "assistant:key", "assistant:read_attachments"];
type Errors = HashMap<String, Vec<String>>;
const REQUIRED: &str = "This field is required.";
const CHOICE: &str = "Select a valid choice. That choice is not one of the available choices.";
const NULL: &str = "Null characters are not allowed.";
const CREATE_URL: &str = "https://bridge.simplefin.org/simplefin/create";
const PER_PAGE: usize = 50;
const MAX_BULK: usize = 100;

pub(super) fn is_route(name: &str) -> bool {
    name.starts_with("bankfeeds:") || ASSISTANT.contains(&name) || ["attachments:remove", "attachments:add_document", "attachments:add_journal"].contains(&name)
}

fn base(app: &mut App, r: &Route, path: &str) -> AppResult<Value> {
    let mut vm = super::d6::ctx(app, r)?;
    put(&mut vm, "csrf_token", token(app, path));
    put(&mut vm, "secrets_ok", app.os_store_usable);
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
    for conn in &mut connections {
        let sync = format!("/c/{}/bank-feeds/{}/sync/", c.to_db(), conn["pk"].as_str().unwrap_or(""));
        conn["sync_token"] = json!(token(app, &sync));
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
    let fields = vec![
        Field::new("label", "Connection name", Widget::Text).attr("maxlength", "120").required().value_opt(label),
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
            (SELECT count(*) FROM bankfeeds_transaction t WHERE t.feed_account_id=f.id AND t.status='NEW')
         FROM bankfeeds_account f WHERE f.connection_id=?1 ORDER BY f.org_name COLLATE fb_text,f.name COLLATE fb_text,f.first_seen_at,f.id",
    )?;
    Ok(st
        .query_map([connection_id], |r| {
            let (name, org): (String, String) = (r.get(1)?, r.get(2)?);
            let at: Option<String> = r.get(6)?;
            Ok(json!({"pk": r.get::<_, String>(0)?, "name": name, "org_name": org, "label": if org.is_empty() { name.clone() } else { format!("{org} · {name}") },
                "remote_id": r.get::<_, String>(3)?, "currency": r.get::<_, String>(4)?, "balance": r.get::<_, Option<i64>>(5)?.map(cents),
                "balance_at": at, "balance_at_display": stamp(at.as_deref(), "%Y-%m-%d"), "ledger_account_id": r.get::<_, Option<String>>(7)?, "new_count": r.get::<_, i64>(8)?}))
        })?
        .collect::<Result<Vec<_>, _>>()?)
}
fn accounts_page(app: &mut App, r: &Route, path: &str, posted: Option<Posted<'_>>, e: &Errors, errors: &[String]) -> AppResult<String> {
    let c = arg_company(r)?;
    let conn = connection(app, r)?;
    let owner = actor(app)?;
    let mut vm = base(app, r, path)?;
    let mappable = bf::mappable_accounts(&mut app.open_mut()?.book, owner, c)?;
    let conn_id = conn["pk"].as_str().unwrap_or("").to_owned();
    let rows = app.open_mut()?.book.read(|tx| feeds(tx, &conn_id))?;
    let mut choices = vec![(String::new(), "— not mapped —".to_owned())];
    choices.extend(mappable.iter().map(|a| (a.id.to_db(), account_label(a))));
    choices.push(("__new__".into(), "Add new account…".into()));
    let mut fields = vec![];
    let mut pairs = vec![];
    for feed in &rows {
        let name = format!("map_{}", feed["pk"].as_str().unwrap_or(""));
        let value = match posted { Some(p) => get(p, &name).map(str::to_owned), None => feed["ledger_account_id"].as_str().map(str::to_owned) };
        let f = Field::new(&name, feed["label"].as_str().unwrap_or(""), Widget::Select { choices: choices.clone() }).value_opt(value);
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
        pairs.push(json!([feed, forms::BoundField::from(&f), forms::BoundField::from(&new_name), forms::BoundField::from(&new_type), creating]));
        fields.push(f);
    }
    let mut conn = conn;
    conn["sync_token"] = json!(token(app, &format!("/c/{}/bank-feeds/{conn_id}/sync/", c.to_db())));
    put(&mut vm, "connection", conn);
    put(&mut vm, "rows", pairs);
    put(&mut vm, "form", forms::bind(&fields, &[]));
    put(&mut vm, "errors", errors);
    let currency = vm["company"]["currency"].clone();
    put(&mut vm, "company_currency", currency);
    draw(app, "bankfeeds/accounts.html", &vm)
}
fn submit_accounts(app: &mut App, r: &Route, path: &str, p: Posted<'_>) -> AppResult<Outcome> {
    let c = arg_company(r)?;
    let conn = connection(app, r)?;
    let owner = actor(app)?;
    let mappable = bf::mappable_accounts(&mut app.open_mut()?.book, owner, c)?;
    let conn_id = conn["pk"].as_str().unwrap_or("").to_owned();
    let rows = app.open_mut()?.book.read(|tx| feeds(tx, &conn_id))?;
    let choices: Vec<(String, String)> = mappable.iter().map(|a| (a.id.to_db(), account_label(a))).collect();
    let mut e = Errors::new();
    let mut wanted = vec![];
    for feed in &rows {
        let feed_id = feed["pk"].as_str().unwrap_or("");
        let name = format!("map_{feed_id}");
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
            let id = model_choice(p, &name, &choices, false, &mut e);
            bf::MappingTarget::Existing(id.as_deref().and_then(AccountId::parse))
        };
        wanted.push((feed_id.to_owned(), target));
    }
    let mut errors = vec![];
    if e.is_empty() {
        match bf::save_mappings(&mut app.open_mut()?.book, owner, c, &conn_id, &wanted, &bf::SecretSet::none()) {
            Ok(()) => return Ok(Outcome::Redirect { location: format!("/c/{}/bank-feeds/", c.to_db()) }),
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
    let rules = bf::all_rules(&mut app.open_mut()?.book, owner, c)?;
    let rows = app.open_mut()?.book.read(|tx| -> Result<Vec<Value>, FbError> {
        rules
            .iter()
            .map(|rule| {
                let a = AccountId::parse(&rule.account_id).map(|id| store::account(tx, id)).transpose()?.flatten();
                Ok(json!({"pk": rule.id, "name": rule.name, "match_text": rule.match_text, "sign_display": sign_display(&rule.sign),
                    "amount_min": rule.amount_min.map(cents), "amount_max": rule.amount_max.map(cents),
                    "account": {"code": a.as_ref().map(|a| a.code.clone()), "name": a.as_ref().map(|a| a.name.clone())},
                    "payee_name": rule.payee_name, "active": rule.active}))
            })
            .collect()
    })?;
    put(&mut vm, "rules", rows);
    put(&mut vm, "error", error_text);
    draw(app, "bankfeeds/rules.html", &vm)
}
fn submit_rules(app: &mut App, r: &Route, path: &str, p: Posted<'_>) -> AppResult<Outcome> {
    let c = arg_company(r)?;
    let owner = actor(app)?;
    let present = |k: &str| get(p, k).filter(|v| !v.is_empty());
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
        initial.insert("payee_name", Some(rule.payee_name.clone()));
        active_initial = rule.active;
    } else if let Some(from) = get(params, "from").filter(|v| !v.is_empty()) {
        let ordinal = int(from).unwrap_or(0);
        let source = app.open_mut()?.book.read(|tx| transaction_by_ordinal(tx, c, ordinal))?;
        if let Some(item) = source {
            let d = &item.description;
            initial.insert("name", Some(if d.is_empty() { "New rule".into() } else { py::py_truncate(d, 60) }));
            initial.insert("match_text", Some(py::py_truncate(d, 200)));
            initial.insert("sign", Some(if item.money_in() { "IN" } else { "OUT" }.into()));
            initial.insert("payee_name", Some(py::py_truncate(d, 120)));
        }
    }
    let value = |name: &str| -> Option<String> {
        match posted {
            Some(p) => get(p, name).map(str::to_owned),
            None => initial.get(name).cloned().flatten().or_else(|| (name == "sign").then(|| "ANY".to_owned())),
        }
    };
    let mut account_choices = vec![(String::new(), "---------".to_owned())];
    account_choices.extend(choices);
    let active = match posted { Some(p) => checked(p, "active"), None => active_initial };
    let fields = vec![
        Field::new("name", "Rule name", Widget::Text).attr("maxlength", "120").required().value_opt(value("name")),
        Field::new("match_text", "Bank description contains", Widget::Text).attr("maxlength", "200").required().value_opt(value("match_text")),
        Field::new("sign", "Applies to", Widget::Select { choices: vec![("ANY".into(), "Money in or out".into()), ("IN".into(), "Money in".into()), ("OUT".into(), "Money out".into())] }).required().value_opt(value("sign")),
        Field::new("amount_min", "Minimum amount", Widget::Text).attr("inputmode", "decimal").attr("maxlength", "32").value_opt(value("amount_min")),
        Field::new("amount_max", "Maximum amount", Widget::Text).attr("inputmode", "decimal").attr("maxlength", "32").value_opt(value("amount_max")),
        Field::new("account", "Category (account)", Widget::Select { choices: account_choices }).required().value_opt(value("account")),
        Field::new("payee_name", "Payee name (optional)", Widget::Text).attr("maxlength", "120").value_opt(value("payee_name")),
        Field::new("active", "Active", Widget::Checkbox { checked: active }),
    ];
    let mut vm = base(app, r, path)?;
    put(&mut vm, "form", forms::bind(&bind_errors(fields, e), &e.get("__all__").cloned().unwrap_or_default()));
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
    let payee_name = char_field(p, "payee_name", 120, false, &mut e);
    let active = checked(p, "active");
    if e.is_empty() {
        let owner = actor(app)?;
        let values = json!({"rule_id": rule.as_ref().map(|x| x.id.clone()), "name": name, "match_text": match_text, "sign": sign,
            "amount_min": if amount_min.is_empty() { Value::Null } else { json!(amount_min) },
            "amount_max": if amount_max.is_empty() { Value::Null } else { json!(amount_max) },
            "account_id": account, "payee_name": payee_name, "active": active});
        match bf::save_rule(&mut app.open_mut()?.book, owner, c, &values, &bf::SecretSet::none()) {
            Ok(_) => return Ok(Outcome::Redirect { location: format!("/c/{}/bank-feeds/rules/", c.to_db()) }),
            Err(err) => error(&mut e, "__all__", messages(err)?),
        }
    }
    Ok(Outcome::Invalid { html: rule_page(app, r, path, &[], Some(p), &e)? })
}

// --- review ----------------------------------------------------------------------------------------

/// `views._int`: a positive integer below 10**12, else None.
fn int(v: &str) -> Option<i64> {
    forms::python_integer(v).filter(|n| *n > 0 && *n < 1_000_000_000_000)
}
fn transaction_by_ordinal(tx: &(impl Query + ?Sized), c: CompanyId, ordinal: i64) -> Result<Option<bf::Item>, FbError> {
    let id: Option<String> = tx
        .query_row(
            "SELECT t.id FROM bankfeeds_transaction t JOIN bankfeeds_account f ON f.id=t.feed_account_id JOIN bankfeeds_connection c ON c.id=f.connection_id WHERE t.ordinal=?1 AND c.company_id=?2",
            params![ordinal, c.to_db()],
            |r| r.get(0),
        )
        .optional()?;
    id.map(|id| bf::item(tx, c, &id)).transpose()
}
/// The transaction id a posted pk names, or a value no transaction has (the service then refuses it
/// with its own "this company" message, as Python's does for an unknown pk).
fn target_id(app: &mut App, c: CompanyId, ordinal: i64) -> AppResult<String> {
    Ok(app.open_mut()?.book.read(|tx| transaction_by_ordinal(tx, c, ordinal))?.map(|i| i.id).unwrap_or_default())
}
fn status_display(s: &str) -> &'static str {
    match s {
        "NEW" => "To review",
        "ACCEPTED" => "Accepted",
        "MATCHED" => "Matched",
        _ => "Ignored",
    }
}
fn kind_display(s: &str) -> &'static str {
    match s {
        "CHECK" => "Check",
        "DEPOSIT" => "Deposit",
        "TRANSFER" => "Transfer",
        "JOURNAL" => "Journal entry",
        "MATCH" => "Existing entry",
        _ => "",
    }
}
/// fb-core's `reference_url` route name (`checks:detail:<id>`) as the URL Django stored.
fn reference_url(c: CompanyId, stored: &str) -> String {
    let Some((route, id)) = stored.rsplit_once(':') else { return String::new() };
    fb_templates::django::reverse(route, &[c.to_db(), id.to_owned()]).unwrap_or_default()
}
fn designated(tx: &(impl Query + ?Sized), c: CompanyId) -> Result<Vec<String>, FbError> {
    let setup = documents::setup_snapshot(tx, c)?;
    Ok(setup["banks"].as_array().map(|b| b.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_default())
}

struct Posted1 {
    kind: Option<String>,
    account_id: Option<String>,
    payee: String,
    number: String,
    error: String,
}

#[derive(Default)]
struct ReviewPost {
    posted: HashMap<i64, Posted1>,
    results: Vec<Value>,
}

fn accept_from_post(app: &mut App, c: CompanyId, pk: i64, p: Posted<'_>, vendors: &HashMap<String, String>) -> AppResult<Result<bf::Item, FbError>> {
    let owner = actor(app)?;
    let id = target_id(app, c, pk)?;
    let field = |k: &str| get(p, &format!("{k}_{pk}")).unwrap_or("");
    let posted_id = |k: &str| int(field(k)).map(|_| field(k).to_owned()).or_else(|| {
        // Rust ids are UUIDs where Python's are integers; `_int` refuses anything else.
        let v = py::py_strip(field(k));
        fb_ids::parse_uuid(v).filter(|u| u.to_string() == v).map(|_| v.to_owned())
    });
    let payee = py::py_truncate(py::py_strip(field("payee")), 120);
    let vendor_id = if payee.is_empty() { None } else { vendors.get(&bf::text::py_lower(&payee)).cloned() };
    let input = json!({"kind": get(p, &format!("kind_{pk}")).unwrap_or(""), "account_id": posted_id("account"), "payee_name": payee,
        "vendor_id": vendor_id, "check_number": py::py_truncate(py::py_strip(field("number")), 20), "journal_line_id": posted_id("line"),
        "receipt_ids": posted_id("receipt").into_iter().collect::<Vec<_>>(), "transfer_account_id": posted_id("to")});
    Ok(bf::accept(&mut app.open_mut()?.book, owner, c, &id, &input, &bf::SecretSet::none()))
}

fn run_review_post(app: &mut App, c: CompanyId, p: Posted<'_>) -> AppResult<ReviewPost> {
    let owner = actor(app)?;
    let mut out = ReviewPost::default();
    let vendors: HashMap<String, String> = app.open_mut()?.book.read(|tx| -> Result<_, FbError> {
        let mut st = tx.prepare("SELECT display_name,id FROM directory_vendor WHERE company_id=?1 AND active=1 ORDER BY id")?;
        let rows = st.query_map([c.to_db()], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows.into_iter().map(|(n, id)| (bf::text::py_lower(&n), id)).collect())
    })?;
    let truthy = |k: &str| get(p, k).filter(|v| !v.is_empty());
    let mut targets: Vec<(&str, Option<i64>)> = vec![];
    if let Some(v) = truthy("accept_one") {
        targets.push(("accept", int(v)));
    } else if let Some(v) = truthy("ignore_one") {
        targets.push(("ignore", int(v)));
    } else if let Some(v) = truthy("return_one") {
        targets.push(("return", int(v)));
    } else if let Some(action) = get(p, "action").filter(|a| ["accept", "ignore"].contains(a)) {
        let mut found = vec![];
        for (_, v) in p.iter().filter(|(k, _)| k == "sel").take(MAX_BULK + 1) {
            if let Some(n) = int(v)
                && !found.contains(&n)
            {
                found.push(n);
            }
        }
        targets = found.into_iter().map(|n| (if action == "accept" { "accept" } else { "ignore" }, Some(n))).collect();
    }
    if targets.len() > MAX_BULK {
        out.results.push(json!({"ok": false, "text": format!("Choose at most {MAX_BULK} rows at a time.")}));
        targets.clear();
    }
    if targets.is_empty() && out.results.is_empty() {
        out.results.push(json!({"ok": false, "text": "Tick the rows to accept or ignore, or use a row button."}));
    }
    for (action, pk) in targets {
        let Some(pk) = pk else { continue };
        let result = match action {
            "accept" => accept_from_post(app, c, pk, p, &vendors)?.map(|i| (i, "accept")),
            "ignore" => {
                let id = target_id(app, c, pk)?;
                bf::ignore(&mut app.open_mut()?.book, owner, c, &id, &bf::SecretSet::none()).map(|i| (i, "ignore"))
            }
            _ => {
                let id = target_id(app, c, pk)?;
                bf::return_to_review(&mut app.open_mut()?.book, owner, c, &id, &bf::SecretSet::none()).map(|i| (i, "return"))
            }
        };
        match result {
            Ok((item, what)) => {
                let head = format!("{} {} {}", item.date().unwrap_or("None"), py::py_truncate(&item.description, 60), cents(item.amount));
                out.results.push(match what {
                    "accept" => json!({"ok": true, "text": format!("{head}: recorded as {}", item.reference), "url": reference_url(c, &item.reference_url)}),
                    "ignore" => json!({"ok": true, "text": format!("{head}: ignored")}),
                    _ => json!({"ok": true, "text": format!("{head}: back in review")}),
                });
            }
            Err(e) => {
                let message = messages(e)?;
                let field = |k: &str| get(p, &format!("{k}_{pk}"));
                out.posted.insert(pk, Posted1 {
                    kind: field("kind").map(str::to_owned),
                    account_id: field("account").map(py::py_strip).filter(|v| !v.is_empty()).map(str::to_owned),
                    payee: field("payee").unwrap_or("").to_owned(),
                    number: field("number").unwrap_or("").to_owned(),
                    error: message.clone(),
                });
                out.results.push(json!({"ok": false, "text": message, "pk": pk}));
            }
        }
    }
    Ok(out)
}

struct Filter {
    status: String,
    account: Option<String>,
    q: String,
}

fn review_page(app: &mut App, r: &Route, path: &str, full_path: &str, params: Posted<'_>, post: Option<ReviewPost>) -> AppResult<(String, u16)> {
    let c = arg_company(r)?;
    let owner = actor(app)?;
    let mut status = 200;
    let post = post.unwrap_or_default();
    if post.results.iter().any(|x| x["ok"] == false) {
        status = 400;
    }
    // ReviewFilterForm(request.GET): always bound.
    let feed_choices: Vec<(String, String)> = app.open_mut()?.book.read(|tx| -> Result<_, FbError> {
        let mut st = tx.prepare("SELECT f.id,f.org_name,f.name,c.label FROM bankfeeds_account f JOIN bankfeeds_connection c ON c.id=f.connection_id WHERE c.company_id=?1 ORDER BY f.org_name COLLATE fb_text,f.name COLLATE fb_text,f.first_seen_at,f.id")?;
        let rows = st
            .query_map([c.to_db()], |r| {
                let (org, name, label): (String, String, String) = (r.get(1)?, r.get(2)?, r.get(3)?);
                Ok((r.get::<_, String>(0)?, format!("{} ({label})", if org.is_empty() { name } else { format!("{org} · {name}") })))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })?;
    let status_choices: Vec<(String, String)> = [("", "To review"), ("ACCEPTED", "Accepted"), ("MATCHED", "Matched"), ("IGNORED", "Ignored"), ("ALL", "Everything")]
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
    let mut e = Errors::new();
    let raw_status = get(params, "status").unwrap_or("").to_owned();
    if !raw_status.is_empty() && !status_choices.iter().any(|(v, _)| *v == raw_status) {
        error(&mut e, "status", format!("Select a valid choice. {raw_status} is not one of the available choices."));
    }
    let account = model_choice(params, "account", &feed_choices, false, &mut e);
    let q = char_field(params, "q", 120, false, &mut e);
    let valid = e.is_empty();
    let filter = valid.then(|| Filter { status: raw_status.clone(), account: account.clone(), q: q.clone() });
    if !valid {
        status = 400;
    }
    let mut account_choices = vec![(String::new(), "All accounts".to_owned())];
    account_choices.extend(feed_choices);
    let fields = vec![
        Field::new("status", "Show", Widget::Select { choices: status_choices }).value_opt(get(params, "status").map(str::to_owned)),
        Field::new("account", "Bank account", Widget::Select { choices: account_choices }).value_opt(get(params, "account").map(str::to_owned)),
        Field::new("q", "Description contains", Widget::Text).attr("maxlength", "120").value_opt(get(params, "q").map(str::to_owned)),
    ];
    let filter_form = forms::bind(&bind_errors(fields, &e), &[]);

    let rules = bf::active_rules(&mut app.open_mut()?.book, owner, c)?;
    let categories = bf::rule_accounts(&mut app.open_mut()?.book, owner, c)?;
    let mut vm = base(app, r, path)?;
    let (rows, page, unmapped, pending, vendor_names) = app.open_mut()?.book.read(|tx| -> Result<_, FbError> {
        let mut items: Vec<bf::Item> = vec![];
        if let Some(f) = &filter {
            let mut st = tx.prepare("SELECT t.id,t.feed_account_id,t.status,t.description FROM bankfeeds_transaction t JOIN bankfeeds_account f ON f.id=t.feed_account_id JOIN bankfeeds_connection c ON c.id=f.connection_id WHERE c.company_id=?1 ORDER BY t.posted_on IS NOT NULL,t.posted_on DESC,t.ordinal DESC")?;
            let ids = st.query_map([c.to_db()], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?)))?.collect::<Result<Vec<_>, _>>()?;
            let wanted = if f.status.is_empty() { "NEW" } else { f.status.as_str() };
            let needle = bf::text::py_lower(py::py_strip(&f.q));
            for (id, feed, st, description) in ids {
                if wanted != "ALL" && st != wanted {
                    continue;
                }
                if f.account.as_ref().is_some_and(|a| *a != feed) {
                    continue;
                }
                if !f.q.is_empty() && !bf::text::py_lower(&description).contains(&needle) {
                    continue;
                }
                items.push(bf::item(tx, c, &id)?);
            }
        }
        // Paginator(PER_PAGE).get_page(request.GET.get('page')).
        let count = items.len();
        let pages = count.div_ceil(PER_PAGE).max(1);
        let number = match get(params, "page").and_then(forms::python_integer) {
            Some(n) if n >= 1 => (n as usize).min(pages),
            Some(_) => pages,
            None => 1,
        };
        let shown: Vec<bf::Item> = items.into_iter().skip((number - 1) * PER_PAGE).take(PER_PAGE).collect();
        let banks = designated(tx, c)?;
        let bank_accounts: Vec<fb_core::model::Account> = store::accounts(tx, c)?.into_iter().filter(|a| banks.contains(&a.id.to_db())).collect();
        let mut rows = vec![];
        for item in shown {
            let feed = bf::feed(tx, &item.feed_account_id)?.ok_or_else(|| FbError::validation("Choose a downloaded transaction of this company."))?;
            let ledger = feed.ledger_account_id.as_deref().and_then(AccountId::parse).map(|id| store::account(tx, id)).transpose()?.flatten();
            let mut iv = json!({"pk": item.ordinal, "status": item.status, "status_display": status_display(&item.status), "pending": item.pending,
                "date": item.date().unwrap_or(""), "description": item.description, "amount": cents(item.amount), "money_in": item.money_in(),
                "reference": item.reference, "reference_url": reference_url(c, &item.reference_url), "accepted_as": item.accepted_as,
                "accepted_as_display": kind_display(&item.accepted_as),
                "feed_account": {"name": feed.name, "ledger_account": ledger.as_ref().map(|a| json!({"code": a.code, "name": a.name}))}});
            if item.status != "NEW" {
                let reversed = match &item.journal_id {
                    Some(j) => tx.query_row("SELECT EXISTS(SELECT 1 FROM ledger_journal WHERE reversal_of_id=?1)", [j], |r| r.get::<_, bool>(0))?,
                    None => false,
                };
                rows.push(json!({"item": iv, "suggestion": null, "reversed": reversed}));
                continue;
            }
            let s = bf::suggest_for(tx, c, &item, Some(&rules))?;
            let is_designated = feed.ledger_account_id.as_ref().is_some_and(|a| banks.contains(a));
            let other: Vec<&fb_core::model::Account> = bank_accounts.iter().filter(|b| Some(b.id.to_db()) != feed.ledger_account_id && b.active && b.postable).collect();
            let mut options: Vec<(&str, &str)> = vec![];
            if !s.matches.is_empty() {
                options.push(("MATCH", "Match existing entry"));
            }
            if !s.receipts.is_empty() {
                options.push(("DEPOSIT", "Deposit receipt"));
            }
            if item.money_in() {
                options.push(("JOURNAL", if is_designated { "Deposit (journal)" } else { "Payment (journal)" }));
            } else {
                if is_designated {
                    options.push(("CHECK", "Check or debit"));
                }
                options.push(("JOURNAL", if !is_designated { "Charge (journal)" } else { "Journal entry" }));
            }
            if is_designated && !other.is_empty() {
                options.push(("TRANSFER", "Transfer"));
            }
            let default_kind = if !item.money_in() && is_designated { "CHECK" } else { "JOURNAL" };
            let preferred = if s.kind.is_empty() { default_kind } else { s.kind.as_str() };
            options.sort_by_key(|o| o.0 != preferred);
            let posted = post.posted.get(&item.ordinal);
            let kind = posted.and_then(|x| x.kind.clone()).filter(|k| !k.is_empty()).unwrap_or_else(|| options.first().map(|o| o.0.to_owned()).unwrap_or_default());
            let account_id = posted.and_then(|x| x.account_id.clone()).or_else(|| s.account_id.clone());
            let payee = posted.map(|x| x.payee.clone()).unwrap_or_else(|| s.payee_name.clone());
            let number = posted.map(|x| x.number.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| if item.money_in() { String::new() } else { bf::default_check_number(&item) });
            iv["pk"] = json!(item.ordinal);
            rows.push(json!({
                "item": iv,
                "suggestion": {
                    "matches": s.matches.iter().map(|m| json!({"label": m.label, "date": m.date, "line": {"pk": m.line_id}})).collect::<Vec<_>>(),
                    "receipts": s.receipts.iter().map(|x| json!({"label": x.label, "date": x.date, "receipt": {"pk": x.receipt_id}})).collect::<Vec<_>>(),
                    "rule": s.rule.as_ref().map(|x| json!({"name": x.name})),
                },
                "options": options,
                "kind": kind,
                "account_id": account_id,
                "payee": payee,
                "number": number,
                "categories": categories.iter().filter(|a| Some(a.id.to_db()) != feed.ledger_account_id).map(|a| json!({"pk": a.id.to_db(), "code": a.code, "name": a.name})).collect::<Vec<_>>(),
                "other_banks": other.iter().map(|a| json!({"pk": a.id.to_db(), "code": a.code, "name": a.name})).collect::<Vec<_>>(),
                "error": posted.map(|x| x.error.clone()),
            }));
        }
        let page = json!({"number": number, "has_previous": number > 1, "has_next": number < pages, "previous_page_number": number.saturating_sub(1),
            "next_page_number": number + 1, "paginator": {"num_pages": pages, "count": count}});
        let unmapped: i64 = tx.query_row("SELECT count(DISTINCT f.id) FROM bankfeeds_account f JOIN bankfeeds_connection c ON c.id=f.connection_id JOIN bankfeeds_transaction t ON t.feed_account_id=f.id WHERE c.company_id=?1 AND f.active=1 AND f.ledger_account_id IS NULL AND t.status='NEW'", [c.to_db()], |r| r.get(0))?;
        let (_, pending, _) = overview(tx, c)?;
        let mut st = tx.prepare("SELECT display_name FROM directory_vendor WHERE company_id=?1 AND active=1 ORDER BY display_name COLLATE fb_text,id LIMIT 500")?;
        let vendor_names = st.query_map([c.to_db()], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
        Ok((rows, page, unmapped, pending, vendor_names))
    })?;
    let query: Vec<(String, String)> = params.iter().filter(|(k, _)| k != "page").cloned().collect();
    put(&mut vm, "filter_form", filter_form);
    put(&mut vm, "page", page);
    put(&mut vm, "rows", rows);
    put(&mut vm, "results", post.results);
    put(&mut vm, "query_string", urlencode(&query));
    put(&mut vm, "vendor_names", vendor_names);
    put(&mut vm, "showing", if valid && !raw_status.is_empty() { raw_status } else { "NEW".into() });
    put(&mut vm, "unmapped", unmapped);
    put(&mut vm, "pending", pending);
    put(&mut vm, "full_path", full_path);
    Ok((draw(app, "bankfeeds/review.html", &vm)?, status))
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
        "bankfeeds:accounts" => submit_accounts(app, r, path, p),
        "bankfeeds:forget" => {
            let conn = connection(app, r)?;
            crate::plugins::bankfeed_forget(app, c, conn["pk"].as_str().unwrap_or(""))?;
            Ok(Outcome::Redirect { location: format!("/c/{}/bank-feeds/", c.to_db()) })
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
            let (html, status) = review_page(app, r, path, path, &[], Some(post))?;
            Ok(Outcome::Page { html, status })
        }
        // The list is GET only.
        _ => {
            let _ = base(app, r, path)?;
            Ok(Outcome::Page { html: String::new(), status: 405 })
        }
    }
}
