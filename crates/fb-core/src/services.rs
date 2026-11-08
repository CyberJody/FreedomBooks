//! Port of `ledger/services.py`: companies, accounts, money, entry numbers, drafts, posting, reversal.
//!
//! Each public function is one command: it opens `BEGIN IMMEDIATE` through `Book::write` (the Python
//! `@transaction.atomic` plus the company `FOR NO KEY UPDATE` mutex), and the `*_tx` functions are the
//! Python `_private` helpers that nest inside it (`_reverse_journal` calls `_save_draft` and
//! `_post_journal` in the same transaction). Names and checks follow the Python code in order, so the
//! first refusal a caller sees is the same one.
//!
//! L2a supplies source ownership and control-account hooks. The banking layer still adds
//! cleared-bank-line reversal checks when that module is translated.

use fb_db::Query;
use crate::error::{FbError, FbResult};
use crate::model::*;
use crate::py::{self, BLANK, max_length_message, py_len, py_strip};
use crate::store::{self, User};
use fb_db::{Book, Writer};
use fb_db::rusqlite::params;
use fb_ids::{AccountId, CompanyId, JournalId, UserId};
use fb_money::{AmountInput, MAX_TOTAL_CENTS, Money};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

pub const MANUAL_MAX_LINES: usize = 50;
pub const ENTRY_NUMBER_MAX: usize = 20;
pub const NAME_MAX: usize = 120;
pub const MEMO_MAX: usize = 240;
pub const LINE_KEYS: [&str; 5] = ["account", "debit", "credit", "description", "name"];

// ------------------------------------------------------------------ access and audit

pub fn authenticated(conn: &(impl Query + ?Sized), actor: UserId) -> FbResult<User> {
    match store::user(conn, actor)? {
        Some(user) if user.is_active => Ok(user),
        _ => Err(FbError::denied("An identified active operator is required.")),
    }
}

pub fn access(conn: &(impl Query + ?Sized), actor: UserId, company: CompanyId) -> FbResult<User> {
    let user = authenticated(conn, actor)?;
    if !store::is_member(conn, company, actor)? {
        return Err(FbError::denied("Company access denied."));
    }
    Ok(user)
}

/// `audit()`: one append-only event; `target` is `<model>:<id>`.
pub fn audit(conn: &Writer<'_>, actor: &User, company: CompanyId, action: &str, target: String, detail: Value) -> FbResult<()> {
    let event_id = fb_ids::AuditEventId::new().to_db();
    conn.execute(
        "INSERT INTO ledger_auditevent (id, company_id, actor_id, actor_name, occurred_at, action, target, detail)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            event_id,
            company.to_db(),
            actor.id.to_db(),
            actor.username,
            py::now_utc(),
            action,
            target,
            serde_json::to_string(&detail).expect("json")
        ],
    )?;
    crate::agent_provenance::record(conn, actor, company, &event_id)?;
    Ok(())
}

// ------------------------------------------------------------------ companies

fn currency_errors(currency: &str) -> Vec<String> {
    if currency.is_empty() {
        return vec![BLANK.into()];
    }
    // Django runs the field's own validators (the RegexValidator) before the MaxLengthValidator CharField appends.
    let mut errors = Vec::new();
    // RegexValidator(r"^[A-Z]{3}$") uses re.search: `$` also matches before one trailing newline.
    let core = currency.strip_suffix('\n').unwrap_or(currency);
    if !(core.len() == 3 && core.bytes().all(|b| b.is_ascii_uppercase())) {
        errors.push("Use a three-letter uppercase currency code.".into());
    }
    if py_len(currency) > 3 {
        errors.push(max_length_message(3, currency));
    }
    errors
}

fn company_full_clean(company: &Company) -> FbResult<()> {
    let mut errors = Vec::new();
    if company.name.is_empty() {
        errors.push(BLANK.to_string());
    } else if py_len(&company.name) > 120 {
        errors.push(max_length_message(120, &company.name));
    }
    errors.extend(currency_errors(&company.currency));
    if !(0..=BILL_REMINDER_MAX).contains(&company.bill_reminder_days) {
        errors.push(format!("Ensure this value is less than or equal to {BILL_REMINDER_MAX}."));
    }
    if errors.is_empty() { Ok(()) } else { Err(FbError::Validation(errors)) }
}

pub fn create_company(book: &mut Book, actor: UserId, name: &str, currency: &str, open_first_run: bool) -> FbResult<Company> {
    create_company_with_chart(book, actor, name, currency, open_first_run, crate::starter_chart::Choice::Empty)
}

/// Create the company and its optional starter accounts as one audited transaction.
pub fn create_company_with_chart(book: &mut Book, actor: UserId, name: &str, currency: &str,
    open_first_run: bool, chart: crate::starter_chart::Choice) -> FbResult<Company> {
    py::clean_free_text(name)?;
    book.write(|tx| {
        let user = authenticated(tx, actor)?;
        let company = Company {
            id: CompanyId::new(),
            name: py_strip(name).to_string(),
            currency: currency.to_uppercase(),
            first_run_open: open_first_run,
            bill_reminder_days: BILL_REMINDER_DEFAULT,
        };
        company_full_clean(&company)?;
        store::insert_company(tx, &company)?;
        store::add_member(tx, company.id, actor)?;
        audit(tx, &user, company.id, "company.created", format!("company:{}", company.id), json!({"name": company.name, "currency": company.currency}))?;
        if chart == crate::starter_chart::Choice::Standard {
            crate::starter_chart::create_tx(tx, actor, company.id)?;
        }
        Ok(company)
    })
}

pub fn edit_company(book: &mut Book, actor: UserId, company_id: CompanyId, name: &str, currency: &str) -> FbResult<Company> {
    book.write(|tx| edit_company_tx(tx, actor, company_id, name, currency))
}

pub(crate) fn edit_company_tx(tx: &Writer<'_>, actor: UserId, company_id: CompanyId, name: &str, currency: &str) -> FbResult<Company> {
        let user = access(tx, actor, company_id)?;
        let mut company = store::get_company(tx, company_id)?;
        let currency = currency.to_uppercase();
        // LAYER 2: also refuse once commercial document identities exist.
        let has_journals: bool =
            tx.query_row("SELECT EXISTS (SELECT 1 FROM ledger_journal WHERE company_id = ?1)", [company.id.to_db()], |r| r.get(0))?;
        let has_documents: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM documents_documentidentity WHERE company_id=?1)",[company.id.to_db()],|r|r.get(0))?;
        if currency != company.currency && (has_journals || has_documents) {
            return Err(FbError::validation("Currency cannot change after journals or commercial document identities have been created."));
        }
        let before = json!({"name": company.name, "currency": company.currency});
        py::clean_changed_free_text(name, Some(&company.name))?;
        company.name = py_strip(name).to_string();
        company.currency = currency;
        company_full_clean(&company)?;
        store::update_company(tx, &company)?;
        audit(tx, &user, company.id, "company.updated", format!("company:{}", company.id),
              json!({"before": before, "after": {"name": company.name, "currency": company.currency}}))?;
        Ok(company)
}

/// P2-3 "Remind me about bills N days before they're due" (0-90). Returns (company, changed).
pub fn set_bill_reminder_days(book: &mut Book, actor: UserId, company_id: CompanyId, days: i64, expected: Option<i64>) -> FbResult<(Company, bool)> {
    book.write(|tx| {
        let user = access(tx, actor, company_id)?;
        if !(user.is_staff || user.is_superuser) {
            return Err(FbError::denied("Only a staff operator can change reminder settings."));
        }
        if !(0..=BILL_REMINDER_MAX).contains(&days) {
            return Err(FbError::validation(format!("Enter a whole number of days from 0 to {BILL_REMINDER_MAX}.")));
        }
        let mut company = store::get_company(tx, company_id)?;
        if company.bill_reminder_days == days {
            return Ok((company, false));
        }
        if expected != Some(company.bill_reminder_days) {
            return Err(FbError::validation("Reminder settings were changed by someone else. Review the current value and try again."));
        }
        let before = company.bill_reminder_days;
        company.bill_reminder_days = days;
        store::update_company(tx, &company)?;
        audit(tx, &user, company.id, "company.reminders_updated", format!("company:{}", company.id),
              json!({"before": {"bill_reminder_days": before}, "after": {"bill_reminder_days": days}}))?;
        Ok((company, true))
    })
}

// ------------------------------------------------------------------ accounts

/// Arguments of `save_account`. `kind` / `detail_type` are text as the Python service takes them
/// (None or "" = not given); `is_contra` / `description` None keep the stored values.
#[derive(Clone, Debug, Default)]
pub struct AccountInput {
    pub code: String,
    pub name: String,
    pub kind: Option<String>,
    pub detail_type: Option<String>,
    pub is_contra: Option<bool>,
    pub description: Option<String>,
    pub active: bool,
    pub postable: bool,
    /// Edit this account; None creates one.
    pub account: Option<AccountId>,
}

impl AccountInput {
    pub fn new(code: &str, name: &str) -> AccountInput {
        AccountInput { code: code.into(), name: name.into(), active: true, postable: true, ..Default::default() }
    }
    pub fn kind(mut self, kind: &str) -> Self {
        self.kind = Some(kind.into());
        self
    }
    pub fn detail(mut self, detail: &str) -> Self {
        self.detail_type = Some(detail.into());
        self
    }
    pub fn contra(mut self, contra: bool) -> Self {
        self.is_contra = Some(contra);
        self
    }
    pub fn description(mut self, description: &str) -> Self {
        self.description = Some(description.into());
        self
    }
    pub fn edit(mut self, account: AccountId) -> Self {
        self.account = Some(account);
        self
    }
    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }
    pub fn postable(mut self, postable: bool) -> Self {
        self.postable = postable;
        self
    }
}

fn account_fields(a: &AccountDraft) -> Value {
    json!({"code": a.code, "name": a.name, "kind": a.kind, "detail_type": a.detail_type, "is_contra": a.is_contra,
           "description": a.description, "active": a.active, "postable": a.postable})
}

/// An account as the Python model instance holds it before `full_clean` (text fields, possibly invalid).
struct AccountDraft {
    code: String,
    name: String,
    kind: String,
    detail_type: String,
    is_contra: bool,
    description: String,
    active: bool,
    postable: bool,
}

/// `Account.full_clean()`: field errors (code, name, kind, detail_type, description), then `clean()`
/// (type/kind agreement, contra on a subledger type), then the unique code constraint.
fn account_full_clean(tx: &(impl Query + ?Sized), company: CompanyId, existing: Option<AccountId>, a: &mut AccountDraft) -> FbResult<()> {
    let mut errors: Vec<String> = Vec::new();
    let mut code_error = false;
    if a.code.is_empty() {
        errors.push(BLANK.into());
        code_error = true;
    } else if py_len(&a.code) > 20 {
        errors.push(max_length_message(20, &a.code));
        code_error = true;
    }
    if a.name.is_empty() {
        errors.push(BLANK.into());
    } else if py_len(&a.name) > 120 {
        errors.push(max_length_message(120, &a.name));
    }
    let kind = Kind::parse(&a.kind);
    if a.kind.is_empty() {
        errors.push(BLANK.into());
    } else if kind.is_none() {
        errors.push(format!("Value '{}' is not a valid choice.", a.kind));
    }
    if a.detail_type.is_empty() && let Some(k) = kind {
        a.detail_type = k.default_detail().as_str().into(); // _fill_detail_type
    }
    let detail = DetailType::parse(&a.detail_type);
    if !a.detail_type.is_empty() && detail.is_none() {
        errors.push(format!("Value '{}' is not a valid choice.", a.detail_type));
    }
    if py_len(&a.description) > DESCRIPTION_MAX {
        errors.push(max_length_message(DESCRIPTION_MAX, &a.description));
    }
    let mut contra_error = false;
    if let Some(d) = detail {
        if Some(d.kind()) != kind && kind.is_some() {
            errors.push("This type does not belong to the account's kind.".into());
        }
        if a.is_contra && d.refuses_contra() {
            errors.push(format!("A {} account cannot be a contra account.", d.label().to_lowercase()));
            contra_error = true;
        }
    }
    let _ = contra_error;
    if !code_error && store::account_code_taken(tx, company, &a.code, existing)? {
        errors.push("Account with this Company and Code already exists.".into());
    }
    if errors.is_empty() { Ok(()) } else { Err(FbError::Validation(errors)) }
}

/// Create or edit an account (FX2 type rules; decision FX2-D4 type-change rule).
pub fn save_account(book: &mut Book, actor: UserId, company_id: CompanyId, input: AccountInput) -> FbResult<Account> {
    let result = book.write(|tx| save_account_tx(tx, actor, company_id, &input));
    match result {
        // An account_code_per_company UNIQUE failure after the checks (a race the Python app maps the same way).
        Err(FbError::Integrity { constraint: Some(c), .. }) if c == "unique:ledger_account.company_id, ledger_account.code" => {
            Err(FbError::validation("An account with this code already exists in this company."))
        }
        other => other,
    }
}

pub(crate) fn save_account_tx(tx: &Writer<'_>, actor: UserId, company_id: CompanyId, input: &AccountInput) -> FbResult<Account> {
    let user = access(tx, actor, company_id)?;
    let existing = match input.account {
        Some(account_id) => {
            let account = store::account(tx, account_id)?.ok_or_else(|| FbError::missing("Account"))?;
            if account.company_id != company_id {
                return Err(FbError::validation("Account belongs to another company."));
            }
            Some(account)
        }
        None => None,
    };
    let given = |v: &Option<String>| v.as_deref().filter(|s| !s.is_empty()).map(str::to_string);
    let mut kind = given(&input.kind);
    let detail_type: String;
    if let Some(dt) = given(&input.detail_type) {
        let Some(detail) = DetailType::parse(&dt) else {
            return Err(FbError::validation("Choose an account type from the list."));
        };
        if let Some(k) = &kind && detail.kind().as_str() != k {
            return Err(FbError::validation("The account type does not belong to that kind."));
        }
        kind = Some(detail.kind().as_str().into());
        detail_type = dt;
    } else if let Some(k) = &kind {
        detail_type = match &existing {
            Some(a) if a.kind.as_str() == k => a.detail_type.as_str().into(),
            _ => Kind::parse(k).map(|k| k.default_detail().as_str().to_string()).unwrap_or_default(),
        };
    } else {
        return Err(FbError::validation("Choose an account type."));
    }
    let kind = kind.unwrap();
    if let Some(a) = &existing {
        let has_snapshot: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM documents_documentaccount WHERE account_id=?1)",[a.id.to_db()],|r|r.get(0))?;
        if a.kind.as_str() != kind && (store::account_has_lines(tx, a.id)? || has_snapshot) {
            // LAYER 2: or captured in a source-document snapshot (documentaccount_set).
            return Err(FbError::validation(format!(
                "An account used in journals or source document snapshots cannot change its type to another kind; choose another {} type.",
                a.kind.label().to_lowercase()
            )));
        }
    }
    let before = match &existing {
        Some(a) => json!({"code": a.code, "name": a.name, "kind": a.kind.as_str(), "detail_type": a.detail_type.as_str(), "is_contra": a.is_contra,
                          "description": a.description, "active": a.active, "postable": a.postable}),
        None => json!({"code": "", "name": "", "kind": "", "detail_type": "", "is_contra": false, "description": "", "active": true, "postable": true}),
    };
    let mut draft = AccountDraft {
        code: { py::clean_changed_free_text(&input.code, existing.as_ref().map(|a| a.code.as_str()))?; py_strip(&input.code).to_string() },
        name: { py::clean_changed_free_text(&input.name, existing.as_ref().map(|a| a.name.as_str()))?; py_strip(&input.name).to_string() },
        kind,
        detail_type,
        is_contra: input.is_contra.unwrap_or_else(|| existing.as_ref().map(|a| a.is_contra).unwrap_or(false)),
        description: match &input.description {
            Some(d) => { py::clean_changed_free_text(d, existing.as_ref().map(|a| a.description.as_str()))?; py_strip(d).to_string() },
            None => existing.as_ref().map(|a| a.description.clone()).unwrap_or_default(),
        },
        active: input.active,
        postable: input.postable,
    };
    account_full_clean(tx, company_id, existing.as_ref().map(|a| a.id), &mut draft)?;
    let account = Account {
        id: existing.as_ref().map(|a| a.id).unwrap_or_default(),
        company_id,
        code: draft.code.clone(),
        name: draft.name.clone(),
        kind: Kind::parse(&draft.kind).expect("cleaned"),
        detail_type: DetailType::parse(&draft.detail_type).expect("cleaned"),
        is_contra: draft.is_contra,
        description: draft.description.clone(),
        active: draft.active,
        postable: draft.postable,
    };
    if existing.is_some() {
        store::update_account(tx, &account)?;
    } else {
        store::insert_account(tx, &account)?;
    }
    audit(tx, &user, company_id, "account.saved", format!("account:{}", account.id), json!({"before": before, "after": account_fields(&draft)}))?;
    Ok(account)
}

// ------------------------------------------------------------------ money and entry numbers

/// `money(value)` for a JSON value the way a Python dict value would arrive.
pub fn money_json(value: &Value) -> FbResult<Money> {
    fb_money::money(&amount_input(value)).map_err(|e| FbError::validation(e.to_string()))
}

pub fn amount_input(value: &Value) -> AmountInput {
    match value {
        Value::String(s) => AmountInput::Text(s.clone()),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                AmountInput::Int(i as i128)
            } else if let Some(u) = n.as_u64() {
                AmountInput::Int(u as i128)
            } else {
                AmountInput::Float(n.as_f64().unwrap_or(f64::NAN))
            }
        }
        Value::Bool(b) => AmountInput::Bool(*b),
        _ => AmountInput::Other,
    }
}

/// Exact trimmed entry number (ASCII whitespace only), 0-20 code points, no C* and no Zl / Zp.
pub fn entry_number_text(value: Option<&str>) -> FbResult<String> {
    let Some(value) = value else { return Ok(String::new()) };
    let value = value.trim_matches(|c| matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c' | '\x0b'));
    if py_len(value) > ENTRY_NUMBER_MAX {
        return Err(FbError::validation(format!("Enter an entry number up to {ENTRY_NUMBER_MAX} characters.")));
    }
    if value.chars().any(py::is_control_or_format) {
        return Err(FbError::validation("An entry number cannot contain control or formatting characters."));
    }
    Ok(value.to_string())
}

fn entry_number_taken(tx: &(impl Query + ?Sized), company: CompanyId, number: &str) -> FbResult<bool> {
    Ok(tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM ledger_journal WHERE company_id = ?1 AND entry_number = ?2)",
        params![company.to_db(), number],
        |r| r.get(0),
    )?)
}

fn all_digit(s: &str) -> bool {
    (1..=ENTRY_NUMBER_MAX).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit())
}

fn all_digit_numbers(tx: &(impl Query + ?Sized), company: CompanyId) -> FbResult<Vec<u128>> {
    let mut stmt = tx.prepare("SELECT entry_number FROM ledger_journal WHERE company_id = ?1 AND entry_number <> ''")?;
    let numbers = stmt
        .query_map([company.to_db()], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|s| all_digit(s))
        .map(|s| s.parse::<u128>().expect("<= 20 digits fits u128"))
        .collect();
    Ok(numbers)
}

/// Highest all-digit entry number of any length 1..20, plus one, then the next free text that still
/// fits; if the successor does not fit, the smallest missing positive integer that does.
fn next_free_all_digit(tx: &(impl Query + ?Sized), company: CompanyId) -> FbResult<String> {
    let numbers = all_digit_numbers(tx, company)?;
    let start: u128 = numbers.iter().max().map(|h| h + 1).unwrap_or(1);
    for offset in 0..64 {
        let text = (start + offset).to_string();
        if text.len() > ENTRY_NUMBER_MAX {
            break;
        }
        if !entry_number_taken(tx, company, &text)? {
            return Ok(text);
        }
    }
    let taken: std::collections::HashSet<u128> = numbers.into_iter().collect();
    let none_left = || FbError::validation("No free entry number is left. Enter one yourself.");
    let mut number: u128 = 1;
    while taken.contains(&number) {
        number += 1;
        if number.to_string().len() > ENTRY_NUMBER_MAX {
            return Err(none_left());
        }
    }
    let text = number.to_string();
    if entry_number_taken(tx, company, &text)? {
        return Err(none_left());
    }
    Ok(text)
}

/// Reference rule: the last manual entry's trailing digits + 1, width kept (0099 -> 0100); if that has
/// no digits, is taken or too long, the all-digit fallback. A form default only.
pub fn next_entry_number_tx(tx: &(impl Query + ?Sized), company: CompanyId) -> FbResult<String> {
    // LAYER 2: AND source_id IS NULL.
    let last: Option<String> = tx
        .query_row(
            "SELECT entry_number FROM ledger_journal WHERE company_id = ?1 AND source_id IS NULL AND reversal_of_id IS NULL AND entry_number <> ''
             ORDER BY id DESC LIMIT 1",
            [company.to_db()],
            |r| r.get(0),
        )
        .ok();
    if let Some(last) = last {
        let digits_len = last.bytes().rev().take_while(|b| b.is_ascii_digit()).count();
        if digits_len > 0 {
            let (prefix, digits) = last.split_at(last.len() - digits_len);
            let trimmed = digits.trim_start_matches('0');
            // int(digits) + 1, zero-filled to the original width (Python ints are unbounded).
            let next = increment_decimal(if trimmed.is_empty() { "0" } else { trimmed });
            let candidate = format!("{prefix}{next:0>width$}", width = digits_len);
            if py_len(&candidate) <= ENTRY_NUMBER_MAX && !entry_number_taken(tx, company, &candidate)? {
                return Ok(candidate);
            }
        }
    }
    next_free_all_digit(tx, company)
}

fn increment_decimal(digits: &str) -> String {
    let mut bytes: Vec<u8> = digits.bytes().collect();
    let mut i = bytes.len();
    loop {
        if i == 0 {
            bytes.insert(0, b'1');
            break;
        }
        i -= 1;
        if bytes[i] == b'9' {
            bytes[i] = b'0';
        } else {
            bytes[i] += 1;
            break;
        }
    }
    String::from_utf8(bytes).unwrap()
}

pub fn next_entry_number(book: &mut Book, company: CompanyId) -> FbResult<String> {
    book.read(|tx| next_entry_number_tx(tx, company))
}

fn check_revision(actual: i64, expected: Option<i64>) -> FbResult<()> {
    match expected {
        None => Ok(()),
        Some(e) if e >= 0 && e == actual => Ok(()),
        Some(_) => Err(FbError::validation("This journal entry changed since you opened it. Reload it and review before saving or posting.")),
    }
}

fn short_text(value: Option<&Value>, limit: usize, label: &str) -> FbResult<String> {
    match value {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(s)) if py_len(s) <= limit => Ok(s.clone()),
        _ => Err(FbError::validation(format!("Enter a {label} up to {limit} characters."))),
    }
}

/// One normalized journal line (`_normalize_lines` row).
#[derive(Clone, Debug, PartialEq)]
pub struct LineRow {
    pub account: AccountId,
    pub debit: Money,
    pub credit: Money,
    pub description: String,
    pub name: String,
}

fn normalize_lines(lines: &[Value], manual: bool) -> FbResult<Vec<LineRow>> {
    if manual && lines.len() > MANUAL_MAX_LINES {
        return Err(FbError::validation(format!("A journal entry has at most {MANUAL_MAX_LINES} lines.")));
    }
    let mut rows = Vec::with_capacity(lines.len());
    for values in lines {
        let Value::Object(map) = values else {
            return Err(FbError::validation("Unexpected journal line fields."));
        };
        if map.keys().any(|k| !LINE_KEYS.contains(&k.as_str())) {
            return Err(FbError::validation("Unexpected journal line fields."));
        }
        let account = match map.get("account") {
            Some(Value::String(s)) => AccountId::parse(s),
            _ => None,
        }
        .ok_or_else(|| FbError::validation("Choose an active posting account in this company."))?;
        let zero = Value::String("0".into());
        let debit = money_json(map.get("debit").unwrap_or(&zero))?;
        let credit = money_json(map.get("credit").unwrap_or(&zero))?;
        let description = short_text(map.get("description"), 160, "line description")?;
        let name = short_text(map.get("name"), NAME_MAX, "name")?;
        rows.push(LineRow { account, debit, credit, description, name });
    }
    Ok(rows)
}

fn creation_digest(date: &str, memo: &str, entry_number: &str, adjusting: bool, lines: &[LineRow]) -> String {
    let lines: Vec<Value> = lines
        .iter()
        .map(|r| json!({"account": r.account.to_string(), "debit": r.debit.to_string(), "credit": r.credit.to_string(),
                        "description": r.description, "name": r.name}))
        .collect();
    let intent = json!({"date": date, "memo": memo, "entry_number": entry_number, "adjusting": adjusting, "lines": lines});
    let text = py::py_json_dumps_sorted(&intent);
    Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// Manual journals may not touch current or historically frozen subledger control accounts.
fn refuse_controls(tx: &(impl Query + ?Sized), company: CompanyId, accounts: &[AccountId]) -> FbResult<()> {
    // Python sorts and deduplicates the account hits, then reports every control in that order.
    let mut accounts = accounts.to_vec();
    accounts.sort();
    accounts.dedup();
    let mut errors = Vec::new();
    for id in accounts {
        if let Some(role) = crate::documents::control_role(tx,company,id)? {
            let account=store::account(tx,id)?.ok_or_else(||FbError::missing("Account"))?;
            errors.push(format!("{} Journal entries cannot use control accounts.", crate::documents::control_message(&account,&role)));
        }
    }
    if errors.is_empty() { Ok(()) } else { Err(FbError::Validation(errors)) }
}

pub fn journal_snapshot(tx: &(impl Query + ?Sized), journal: &Journal) -> FbResult<Value> {
    let mut lines = Vec::new();
    for line in store::lines(tx, journal.id)? {
        let code: String = tx.query_row("SELECT code FROM ledger_account WHERE id = ?1", [line.account_id.to_db()], |r| r.get(0))?;
        lines.push(json!({"position": line.position, "account_id": line.account_id.to_string(), "account_code": code,
                          "debit": line.debit.to_string(), "credit": line.credit.to_string(), "description": line.description, "name": line.name}));
    }
    Ok(json!({"date": journal.date, "memo": journal.memo, "entry_number": journal.entry_number, "adjusting": journal.adjusting,
              "revision": journal.revision, "lines": lines}))
}

// ------------------------------------------------------------------ drafts

/// Arguments of `save_draft` (keyword arguments of the Python service).
#[derive(Clone, Debug, Default)]
pub struct DraftInput {
    pub date: String,
    pub memo: String,
    /// Journal lines as the Python dicts: `{"account": "<uuid>", "debit": "1.00", "credit": ..., "description": ..., "name": ...}`.
    pub lines: Vec<Value>,
    pub journal_id: Option<JournalId>,
    pub entry_number: Option<String>,
    pub adjusting: Option<bool>,
    pub request_key: Option<String>,
    pub expected_revision: Option<i64>,
}

impl DraftInput {
    pub fn new(date: &str, memo: &str, lines: Vec<Value>) -> DraftInput {
        DraftInput { date: date.into(), memo: memo.into(), lines, ..Default::default() }
    }
}

/// A line dict builder for Rust callers: `line(account).debit("10")`.
pub fn line(account: AccountId) -> LineBuilder {
    LineBuilder(Map::from_iter([("account".to_string(), Value::String(account.to_string()))]))
}

pub struct LineBuilder(Map<String, Value>);

impl LineBuilder {
    pub fn debit(mut self, amount: &str) -> Self {
        self.0.insert("debit".into(), Value::String(amount.into()));
        self
    }
    pub fn credit(mut self, amount: &str) -> Self {
        self.0.insert("credit".into(), Value::String(amount.into()));
        self
    }
    pub fn description(mut self, text: &str) -> Self {
        self.0.insert("description".into(), Value::String(text.into()));
        self
    }
    pub fn name(mut self, text: &str) -> Self {
        self.0.insert("name".into(), Value::String(text.into()));
        self
    }
    pub fn set(mut self, key: &str, value: Value) -> Self {
        self.0.insert(key.into(), value);
        self
    }
    pub fn build(self) -> Value {
        Value::Object(self.0)
    }
}

pub fn save_draft(book: &mut Book, actor: UserId, company: CompanyId, input: DraftInput) -> FbResult<Journal> {
    book.write(|tx| save_draft_tx(tx, actor, company, &input, None))
}

/// `_save_draft`. `reversal_of` is the internal argument `_reverse_journal` uses.
/// A raw/autocommit connection cannot call a multi-row write helper:
/// ```compile_fail
/// fn invalid(conn: &fb_db::rusqlite::Connection, actor: fb_core::UserId, company: fb_core::CompanyId, input: &fb_core::services::DraftInput) {
///     fb_core::services::save_draft_tx(conn, actor, company, input, None);
/// }
/// ```
pub fn save_draft_tx(tx: &Writer<'_>, actor: UserId, company: CompanyId, input: &DraftInput, reversal_of: Option<&Journal>) -> FbResult<Journal> {
    save_draft_source_tx(tx,actor,company,input,reversal_of,None)
}

/// Internal source boundary; only an owning document lifecycle may supply `source`.
pub(crate) fn save_draft_source_tx(tx: &Writer<'_>, actor: UserId, company: CompanyId, input: &DraftInput, reversal_of: Option<&Journal>, source: Option<&str>) -> FbResult<Journal> {
    let user = access(tx, actor, company)?;
    let manual = reversal_of.is_none() && source.is_none();
    if manual {
        // Existing journal/snapshot loads below own their errors, after line cleaning.
        // Only a successful provenance read can exempt an unchanged field.
        let previous_result = input.journal_id.map(|id| store::journal_in(tx, id, company)).transpose();
        let previous = previous_result.as_ref().ok().and_then(|v| v.as_ref());
        py::clean_changed_free_text(&input.memo, previous.map(|j| j.memo.as_str()))?;
        let old_lines_result = previous.map(|j| store::lines(tx, j.id)).transpose();
        let old_lines = old_lines_result.as_ref().ok().and_then(|v| v.as_ref()).map(Vec::as_slice).unwrap_or(&[]);
        // The frozen journal form supplies no stable line identity. Deleting
        // or reordering rows must not turn retained legacy text into new input.
        // Exempt only the same field's exact text from this journal's lines.
        for row in &input.lines {
            for key in ["description", "name"] {
                if let Some(raw) = row[key].as_str() {
                    let retained = old_lines.iter().any(|line| match key {
                        "description" => line.description == raw,
                        "name" => line.name == raw,
                        _ => unreachable!(),
                    });
                    py::clean_changed_free_text(raw, retained.then_some(raw))?;
                }
            }
        }
    }
    let rows = normalize_lines(&input.lines, manual)?;
    let number = if manual { entry_number_text(input.entry_number.as_deref())? } else { String::new() };
    let mut key: Option<String> = None;
    let mut digest = String::new();
    if manual && input.journal_id.is_none() && let Some(raw) = &input.request_key {
        let parsed = py::py_uuid(raw).ok_or_else(|| FbError::validation("Supply a stable UUID request key."))?;
        let date = py::form_date(&input.date).map_err(FbError::validation)?;
        digest = creation_digest(&date.to_string(), &input.memo, &number, input.adjusting.unwrap_or(false), &rows);
        let existing = tx
            .query_row(
                &format!("SELECT {} FROM ledger_journal WHERE company_id = ?1 AND request_key = ?2", store::JOURNAL_COLS),
                params![company.to_db(), parsed.0],
                store::journal_row,
            )
            .ok();
        if let Some(existing) = existing {
            if existing.creation_digest != digest {
                return Err(FbError::validation("This retry key belongs to a different journal entry. Start a new entry instead."));
            }
            return Ok(existing); // exact retry: the same journal, whatever happened to it since
        }
        key = Some(parsed.0);
    }
    let mut journal = match input.journal_id {
        Some(id) => store::journal_in(tx, id, company)?,
        None => Journal {
            id: JournalId::new(),
            number: 0,
            company_id: company,
            date: String::new(),
            memo: String::new(),
            status: Status::Draft,
            created_by_id: actor,
            created_at: String::new(),
            posted_by_id: None,
            posted_at: None,
            reversal_of_id: reversal_of.map(|r| r.id),
            source_id: source.map(str::to_string),
            entry_number: String::new(),
            adjusting: false,
            request_key: None,
            creation_digest: String::new(),
            revision: 1,
        },
    };
    let stored = input.journal_id.is_some();
    source_access(tx,&journal,source)?;
    if journal.status != Status::Draft {
        return Err(FbError::validation("Posted journals cannot be edited."));
    }
    let before = if stored { Some(journal_snapshot(tx, &journal)?) } else { None };
    if stored {
        if manual && journal.reversal_of_id.is_some() {
            return Err(FbError::validation("Reversal journals are created by the reversal command only."));
        }
        check_revision(journal.revision, input.expected_revision)?;
        journal.revision = journal.revision.checked_add(1).filter(|r| *r < i64::MAX)
            .ok_or_else(|| FbError::validation("Journal revision limit reached."))?;
    }
    let raw_date = input.date.clone();
    journal.memo = input.memo.clone();
    if manual {
        let number = if !number.is_empty() {
            number
        } else if !journal.entry_number.is_empty() {
            journal.entry_number.clone()
        } else {
            next_entry_number_tx(tx, company)?
        };
        let clash: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM ledger_journal WHERE company_id = ?1 AND entry_number = ?2 AND id <> ?3)",
            params![company.to_db(), number, journal.id.to_db()],
            |r| r.get(0),
        )?;
        if clash {
            return Err(FbError::validation(format!("Entry number {number} is already used by another journal entry. Choose another number.")));
        }
        journal.entry_number = number;
        if let Some(adjusting) = input.adjusting {
            journal.adjusting = adjusting;
        }
        if let Some(k) = key {
            journal.request_key = Some(k);
            journal.creation_digest = digest;
        }
        refuse_controls(tx, company, &rows.iter().map(|r| r.account).collect::<Vec<_>>())?;
    } else {
        journal.entry_number = String::new();
        journal.adjusting = reversal_of.map(|r| r.adjusting).unwrap_or(false);
    }
    // journal.full_clean(): date, then memo.
    let mut errors = Vec::new();
    match py::model_date(&raw_date) {
        Ok(d) => journal.date = d.to_string(),
        Err(e) => errors.push(e),
    }
    if journal.memo.is_empty() {
        errors.push(BLANK.into());
    } else if py_len(&journal.memo) > MEMO_MAX {
        errors.push(max_length_message(MEMO_MAX, &journal.memo));
    }
    if !errors.is_empty() {
        return Err(FbError::Validation(errors));
    }
    if stored {
        store::update_journal(tx, &journal)?;
    } else {
        journal.number = store::next_journal_number(tx)?;
        journal.created_at = py::now_utc();
        store::insert_journal(tx, &journal)?;
    }
    store::delete_lines(tx, journal.id)?;
    for (index, values) in rows.iter().enumerate() {
        let eligible = matches!(store::account(tx, values.account)?, Some(a) if a.company_id == company && a.active && a.postable);
        if !eligible {
            return Err(FbError::validation("Choose an active posting account in this company."));
        }
        // line.full_clean(): the one_positive_side constraint.
        let one_side = (values.debit > Money::ZERO && values.credit.is_zero()) || (values.credit > Money::ZERO && values.debit.is_zero());
        if !one_side {
            return Err(FbError::validation("Constraint “one_positive_side” is violated."));
        }
        store::insert_line(
            tx,
            &JournalLine {
                id: store::new_line_id(),
                journal_id: journal.id,
                account_id: values.account,
                position: index as i64 + 1,
                debit: values.debit,
                credit: values.credit,
                description: values.description.clone(),
                name: values.name.clone(),
            },
        )?;
    }
    let after = journal_snapshot(tx, &journal)?;
    audit(tx, &user, company, "journal.drafted", format!("journal:{}", journal.id), json!({"before": before, "after": after}))?;
    Ok(journal)
}

// ------------------------------------------------------------------ posting

pub fn post_journal(book: &mut Book, actor: UserId, company: CompanyId, journal_id: JournalId, expected_revision: Option<i64>) -> FbResult<Journal> {
    book.write(|tx| post_journal_tx(tx, actor, company, journal_id, expected_revision))
}

/// `_post_journal`: the Rust posting checks (the triggers repeat them as the backstop).
/// ```compile_fail
/// fn invalid(conn: &fb_db::rusqlite::Connection, actor: fb_core::UserId, company: fb_core::CompanyId, journal: fb_core::JournalId) {
///     fb_core::services::post_journal_tx(conn, actor, company, journal, None);
/// }
/// ```
pub fn post_journal_tx(tx: &Writer<'_>, actor: UserId, company: CompanyId, journal_id: JournalId, expected_revision: Option<i64>) -> FbResult<Journal> {
    post_journal_source_tx(tx,actor,company,journal_id,expected_revision,None)
}

pub(crate) fn post_journal_source_tx(tx: &Writer<'_>, actor: UserId, company: CompanyId, journal_id: JournalId, expected_revision: Option<i64>, source: Option<&str>) -> FbResult<Journal> {
    let user = access(tx, actor, company)?;
    let mut journal = store::journal_in(tx, journal_id, company)?;
    source_access(tx,&journal,source)?;
    if journal.status != Status::Draft {
        return Err(FbError::validation("Only drafts can be posted."));
    }
    let manual = journal.reversal_of_id.is_none() && journal.source_id.is_none();
    if manual {
        check_revision(journal.revision, expected_revision)?;
    }
    let lines = store::lines(tx, journal.id)?;
    for line in &lines {
        let ok = matches!(store::account(tx, line.account_id)?, Some(a) if a.company_id == company && a.active && a.postable);
        if !ok {
            return Err(FbError::validation("Every line must use an active posting account in this company."));
        }
    }
    let debit: Money = lines.iter().map(|l| l.debit).sum();
    let credit: Money = lines.iter().map(|l| l.credit).sum();
    if lines.len() < 2 || debit.is_zero() || debit != credit {
        return Err(FbError::validation(format!(
            "Posting needs at least two nonzero lines and equal debits and credits (debits {debit}, credits {credit})."
        )));
    }
    if manual {
        if lines.len() > MANUAL_MAX_LINES {
            return Err(FbError::validation(format!("A journal entry has at most {MANUAL_MAX_LINES} lines.")));
        }
        if debit.cents() > MAX_TOTAL_CENTS {
            return Err(FbError::validation("Journal entry totals cannot exceed 99999999999999.99."));
        }
        refuse_controls(tx, company, &lines.iter().map(|l| l.account_id).collect::<Vec<_>>())?;
        if journal.entry_number.is_empty() {
            journal.entry_number = next_entry_number_tx(tx, company)?; // a draft saved before entry numbers existed
        }
    }
    journal.status = Status::Posted;
    journal.posted_by_id = Some(actor);
    journal.posted_at = Some(py::now_utc());
    match store::update_journal(tx, &journal) {
        Err(FbError::Integrity { constraint: Some(c), .. }) if c == "unique:ledger_journal.company_id, ledger_journal.entry_number" => {
            return Err(FbError::validation(format!(
                "Entry number {} is already used by another journal entry. Choose another number.",
                journal.entry_number
            )));
        }
        other => other?,
    }
    audit(tx, &user, company, "journal.posted", format!("journal:{}", journal.id),
          json!({"debit": debit.to_string(), "credit": credit.to_string(), "date": journal.date, "memo": journal.memo,
                 "entry_number": journal.entry_number, "adjusting": journal.adjusting}))?;
    Ok(journal)
}

// ------------------------------------------------------------------ reversal

pub fn reverse_journal(book: &mut Book, actor: UserId, company: CompanyId, journal_id: JournalId, date: &str, reason: &str) -> FbResult<Journal> {
    book.write(|tx| reverse_journal_tx(tx, actor, company, journal_id, date, reason))
}

/// `_reverse_journal`: a linked, posted mirror of the original (voids of documents come through here too).
/// ```compile_fail
/// fn invalid(conn: &fb_db::rusqlite::Connection, actor: fb_core::UserId, company: fb_core::CompanyId, journal: fb_core::JournalId) {
///     fb_core::services::reverse_journal_tx(conn, actor, company, journal, "2026-01-01", "reason");
/// }
/// ```
pub fn reverse_journal_tx(tx: &Writer<'_>, actor: UserId, company: CompanyId, journal_id: JournalId, date: &str, reason: &str) -> FbResult<Journal> {
    reverse_journal_source_tx(tx,actor,company,journal_id,date,reason,None)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn reverse_journal_source_tx(tx: &Writer<'_>, actor: UserId, company: CompanyId, journal_id: JournalId, date: &str, reason: &str, source: Option<&str>) -> FbResult<Journal> {
    let user = access(tx, actor, company)?;
    let original = store::journal_in(tx, journal_id, company)?;
    source_access(tx,&original,source)?;
    crate::banking::refuse_cleared_reversal(tx, company, &original.id.to_db())?;
    let reversal_date = py::form_date(date).map_err(FbError::validation)?;
    let already: bool =
        tx.query_row("SELECT EXISTS (SELECT 1 FROM ledger_journal WHERE reversal_of_id = ?1)", [original.id.to_db()], |r| r.get(0))?;
    if original.status != Status::Posted || original.reversal_of_id.is_some() || already {
        return Err(FbError::validation("Only an unreversed original posted journal can be reversed."));
    }
    py::clean_free_text(reason)?;
    let reason = py_strip(reason);
    if reversal_date.to_string() < original.date || reason.is_empty() {
        return Err(FbError::validation("Supply a reason and a reversal date on or after the original date."));
    }
    // Bound only the display summary, never the canonical reason audited below.
    let summary = py::py_truncate(&format!("Reversal of {}: {}", original.display(), reason), MEMO_MAX);
    let lines: Vec<Value> = store::lines(tx, original.id)?
        .into_iter()
        .map(|l| {
            json!({"account": l.account_id.to_string(), "debit": l.credit.to_string(), "credit": l.debit.to_string(),
                   "description": l.description, "name": l.name})
        })
        .collect();
    let draft = DraftInput::new(&reversal_date.to_string(), &summary, lines);
    let reversal = save_draft_source_tx(tx, actor, company, &draft, Some(&original),source)?;
    let reversal = post_journal_source_tx(tx, actor, company, reversal.id, None,source)?;
    audit(tx, &user, company, "journal.reversed", format!("journal:{}", original.id),
          json!({"reversal_id": reversal.id.to_string(), "reason": reason, "date": reversal_date.to_string()}))?;
    Ok(reversal)
}

fn source_access(tx: &(impl Query + ?Sized),journal:&Journal,source:Option<&str>)->FbResult<()> {
    if journal.source_id.as_deref()!=source {return Err(FbError::validation("Use the owning source document lifecycle, not generic journal actions."));}
    if let Some(id)=source {crate::documents::identity(tx,journal.company_id,id).map_err(|_|FbError::validation("Source document belongs to another company."))?;}
    Ok(())
}
