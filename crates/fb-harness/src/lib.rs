//! fb-harness: the differential "answer key" harness for layer 1.
//!
//! A scenario (JSON) lists users and steps in the ledger command vocabulary. `py/run_scenario.py` applies
//! it through the frozen Python services; `run_scenario` here applies it to a fresh SQLCipher book through
//! fb-core. Both dump the same normalized JSON (natural keys = the scenario's refs, money in cents), and
//! `compare` reports every difference. Failures: any money, status, line, register-order or outcome
//! (ok vs refused, and the exception kind) difference. Warnings: error wording.
//! Order inversions pass only under the shared ICU4X punctuation-tie predicate; see README.md.

mod journal_aliases;
mod backups;
mod assistant;
mod assistant_shape;
mod dump_shape;
mod checkpoints;
mod l2a;
mod audit;
mod sales;
mod payables;
mod payables_shape;
mod checks;
mod checks_shape;
mod prepayments;
mod prepayments_shape;
mod credits;
mod credits_shape;
mod attachments;
mod attachments_shape;
mod banking;
mod banking_shape;
mod banking_validation;
mod financial_reports;
#[cfg(test)]
mod financial_reports_tests;
mod bankfeeds;
mod fake_credentials;
mod bankfeeds_shape;
#[cfg(test)]
mod int_l3b_tests;

use fb_core::services::{self, AccountInput, DraftInput};
use fb_core::{AccountId, Book, CompanyId, FbError, JournalId, UserId, reports, store, users};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

pub struct Runner<'a> {
    backups: backups::State,
    scenario: &'a Value,
    book: backups::SelectedBook,
    users: HashMap<String, UserId>,
    companies: HashMap<String, CompanyId>,
    accounts: HashMap<String, AccountId>,
    journals: HashMap<String, JournalId>,
    company_actor: HashMap<String, UserId>,
    company_order: Vec<String>,
    outcomes: Vec<Value>,
    checkpoints: Vec<Value>,
    l2_refs: HashMap<String,String>,
    l2_results: Map<String,Value>,
    sales_reviews: HashMap<String,fb_core::sales::Review>,
    payables_reviews: HashMap<String,fb_core::payables::Review>,
    checks_numbers: Map<String,Value>,
    checks_reviews: HashMap<String,fb_core::checks::Review>,
    transfers_reviews: HashMap<String,fb_core::transfers::Review>,
    pp_reviews: HashMap<String,fb_core::prepayments::Review>,
    credits_reviews: HashMap<String,fb_core::credits::Review>,
    banking_reviews: HashMap<String,fb_core::banking::Review>,
    banking_results: Map<String,Value>,
    banking_screens: Map<String,Value>,
    banking_reports: Map<String,Value>,
    attachment_store: fb_core::attachments::storage::Store,
    /// The book's master key: receipts are wrapped under it, so backups must use the same key.
    key: [u8; 32],
    _attachment_dir: tempfile::TempDir,
    attachment_results: Map<String,Value>,
    attachment_queries: Vec<Value>,
    assistant_results: Map<String,Value>,
    bf: bankfeeds::State,
}

fn s<'v>(step: &'v Value, key: &str) -> &'v str {
    step[key].as_str().unwrap_or_else(|| panic!("step needs {key}: {step}"))
}

/// A step that names a company / account / journal whose creating step was refused.
pub enum StepError {
    Fb(FbError),
    MissingRef(String),
}

impl From<FbError> for StepError {
    fn from(e: FbError) -> Self {
        StepError::Fb(e)
    }
}

fn find<T: Copy>(table: &HashMap<String, T>, r: &str) -> Result<T, StepError> {
    table.get(r).copied().ok_or_else(|| StepError::MissingRef(format!("unknown ref {r}")))
}

fn opt_s(step: &Value, key: &str) -> Option<String> {
    step.get(key).and_then(|v| v.as_str()).map(str::to_string)
}

impl<'a> Runner<'a> {
    pub fn new(scenario: &'a Value, book_path: &Path) -> Runner<'a> {
        let mut key = [0u8; 32];
        getrandom::fill(&mut key).expect("random key");
        Self::with_key(scenario, book_path, &key)
    }

    pub fn with_key(scenario: &'a Value, book_path: &Path, key: &[u8; 32]) -> Runner<'a> {
        let book = backups::SelectedBook::create(book_path, key).expect("create book");
        let attachment_dir = tempfile::Builder::new().prefix("fbv2-attachments-").tempdir().expect("private attachment fixture");
        let attachment_store = fb_core::attachments::storage::Store::for_book(&attachment_dir.path().join("book.fbbook"), key).expect("attachment store");
        Runner {
            scenario,
            book,
            backups: backups::State::new(book_path),
            users: HashMap::new(),
            companies: HashMap::new(),
            accounts: HashMap::new(),
            journals: HashMap::new(),
            company_actor: HashMap::new(),
            company_order: Vec::new(),
            outcomes: Vec::new(),
            checkpoints: Vec::new(),
            l2_refs: HashMap::new(),
            l2_results: Map::new(),
            sales_reviews: HashMap::new(),
            payables_reviews: HashMap::new(),
            checks_numbers: Map::new(),
            checks_reviews: HashMap::new(),
            transfers_reviews: HashMap::new(),
            pp_reviews: HashMap::new(),
            credits_reviews: HashMap::new(),
            banking_reviews: HashMap::new(),
            banking_results: Map::new(),
            banking_screens: Map::new(),
            banking_reports: Map::new(),
            attachment_store,
            key: *key,
            _attachment_dir: attachment_dir,
            attachment_results: Map::new(),
            attachment_queries: Vec::new(),
            assistant_results: Map::new(),
            bf: bankfeeds::State::default(),
        }
    }

    fn actor(&self, step: &Value) -> UserId {
        let default = self.scenario["users"][0]["username"].as_str().unwrap();
        let name = step.get("actor").and_then(|v| v.as_str()).unwrap_or(default);
        if name == "<anonymous>" { UserId::new() } else { self.users[name] }
    }

    fn lines(&self, lines: &Value) -> Vec<Value> {
        lines
            .as_array()
            .unwrap()
            .iter()
            .map(|line| {
                let mut line = line.clone();
                if let Some(Value::String(r)) = line.get("account").cloned() {
                    // An unknown ref stands for an account that does not exist (Python passes a missing pk).
                    let id = self.accounts.get(&r).copied().unwrap_or_else(AccountId::new);
                    line["account"] = Value::String(id.to_string());
                }
                line
            })
            .collect()
    }

    pub fn run(&mut self) {
        for user in self.scenario["users"].as_array().unwrap() {
            let name = user["username"].as_str().unwrap();
            let id = users::create_user(&mut self.book, name, user.get("is_staff").and_then(|v| v.as_bool()).unwrap_or(false)).unwrap();
            self.users.insert(name.to_string(), id);
        }
        let steps = self.scenario["steps"].as_array().unwrap().clone();
        for (index, step) in steps.iter().enumerate() {
            let mut outcome = Map::new();
            outcome.insert("step".into(), json!(index));
            outcome.insert("op".into(), step["op"].clone());
            match self.apply(step) {
                Ok(()) => {
                    outcome.insert("ok".into(), json!(true));
                    if ["assistant_parse","assistant_initial","assistant_queue"].contains(&s(step,"op")){outcome.insert("result_ref".into(),step["ref"].clone());}
                }
                Err(StepError::Fb(e)) => {
                    outcome.insert("ok".into(), json!(false));
                    outcome.insert("kind".into(), json!(e.kind()));
                    outcome.insert("message".into(), json!(e.message()));
                }
                Err(StepError::MissingRef(m)) => {
                    outcome.insert("ok".into(), json!(false));
                    outcome.insert("kind".into(), json!("MissingRef"));
                    outcome.insert("message".into(), json!(m));
                }
            }
            let ok = outcome["ok"].as_bool().unwrap();
            let expect_error = step.get("expect_error").and_then(Value::as_bool).unwrap_or(false);
            if ok == expect_error
                || step.get("expect_kind").is_some_and(|v| outcome.get("kind") != Some(v))
                || (!s(step, "op").starts_with("bankfeeds_") && step.get("expect_message").is_some_and(|v| outcome.get("message") != Some(v)))
            {
                outcome.insert("unexpected".into(), json!(true));
            }
            self.record_step_evidence(step, &mut outcome);
            // A refusal's pinned reason travels with the outcome; the comparer holds each side to it
            // (after v2's redaction of the reference's bank-feed text, which a runner cannot apply).
            for pin in ["expect_kind", "expect_message"] {
                if let Some(v) = step.get(pin) {
                    outcome.insert(pin.into(), v.clone());
                }
            }
            if let Some(manifest) = bankfeeds::manifest(step) {
                outcome.insert("manifest".into(), manifest);
            }
            if let Some(evidence) = self.bf.evidence.take().filter(|_| ok) {
                outcome.insert("evidence".into(), evidence);
            }
            self.outcomes.push(Value::Object(outcome));
            if self.scenario["capture_steps"]==true {
                let dump=self.dump_current();let financial=checkpoints::financial(&dump).expect("financial checkpoint within integer totals");
                self.checkpoints.push(json!({"step":index,"dump":dump,"financial":financial}));
            }
        }
    }

    fn apply(&mut self, step: &Value) -> Result<(), StepError> {
        let actor = self.actor(step);
        if self.apply_attachments(step,actor)? {return Ok(());}
        if self.apply_assistant(step,actor)? {return Ok(());}
        if self.apply_checks(step,actor)? {return Ok(());}
        if self.apply_prepayments(step,actor)? {return Ok(());}
        if self.apply_credits(step,actor)? {return Ok(());}
        if self.apply_backups(step)? {return Ok(());}
        if self.apply_bankfeeds(step,actor)? {return Ok(());}
        if self.apply_banking(step,actor)? {return Ok(());}
        if self.apply_sales(step,actor)? {return Ok(());}
        if self.apply_l2a(step,actor)? {return Ok(());}
        if self.apply_payables(step,actor)? {return Ok(());}
        let company = |r: &Runner, step: &Value| find(&r.companies, s(step, "company"));
        match s(step, "op") {
            "create_company" => {
                let c = services::create_company(
                    &mut self.book,
                    actor,
                    s(step, "name"),
                    step.get("currency").and_then(|v| v.as_str()).unwrap_or("USD"),
                    step.get("open_first_run").and_then(|v| v.as_bool()).unwrap_or(false),
                )?;
                let r = s(step, "ref").to_string();
                self.companies.insert(r.clone(), c.id);
                self.company_actor.entry(r.clone()).or_insert(actor);
                self.company_order.push(r);
            }
            "add_member" => {
                let c = company(self, step)?;
                users::add_member(&mut self.book, c, self.users[s(step, "user")])?;
            }
            "edit_company" => {
                let c = company(self, step)?;
                services::edit_company(&mut self.book, actor, c, s(step, "name"), s(step, "currency"))?;
            }
            "save_account" => {
                let c = company(self, step)?;
                let editing = opt_s(step, "account");
                let input = AccountInput {
                    code: s(step, "code").into(),
                    name: s(step, "name").into(),
                    kind: opt_s(step, "kind"),
                    detail_type: opt_s(step, "detail_type"),
                    is_contra: step.get("is_contra").and_then(|v| v.as_bool()),
                    description: opt_s(step, "description"),
                    active: step.get("active").and_then(|v| v.as_bool()).unwrap_or(true),
                    postable: step.get("postable").and_then(|v| v.as_bool()).unwrap_or(true),
                    account: editing.as_ref().map(|r| find(&self.accounts, r)).transpose()?,
                };
                let saved = services::save_account(&mut self.book, actor, c, input)?;
                if let (Some(r), None) = (opt_s(step, "ref"), editing) {
                    self.accounts.insert(r, saved.id);
                }
            }
            "save_draft" => {
                let c = company(self, step)?;
                let editing = opt_s(step, "journal");
                let input = DraftInput {
                    date: s(step, "date").into(),
                    memo: s(step, "memo").into(),
                    lines: self.lines(&step["lines"]),
                    journal_id: editing.as_ref().map(|r| find(&self.journals, r)).transpose()?,
                    entry_number: opt_s(step, "entry_number"),
                    adjusting: step.get("adjusting").and_then(|v| v.as_bool()),
                    request_key: opt_s(step, "request_key"),
                    expected_revision: step.get("expected_revision").and_then(|v| v.as_i64()),
                };
                let journal = services::save_draft(&mut self.book, actor, c, input)?;
                if let (Some(r), None) = (opt_s(step, "ref"), editing) {
                    self.journals.entry(r).or_insert(journal.id);
                }
            }
            "post_journal" => {
                let c = company(self, step)?;
                let j = find(&self.journals, s(step, "journal"))?;
                services::post_journal(&mut self.book, actor, c, j, step.get("expected_revision").and_then(|v| v.as_i64()))?;
            }
            "reverse_journal" => {
                let c = company(self, step)?;
                let j = find(&self.journals, s(step, "journal"))?;
                let reversal = services::reverse_journal(&mut self.book, actor, c, j, s(step, "date"), s(step, "reason"))?;
                self.journals.insert(s(step, "ref").into(), reversal.id);
            }
            other => panic!("unknown op {other}"),
        }
        Ok(())
    }

    /// The normalized dump; the same shape `py/run_scenario.py` writes.
    pub fn dump(&mut self) -> Value {
        let current=self.dump_current();
        if self.scenario["capture_steps"]==true {json!({"checkpoints":self.checkpoints,"final":current})} else {current}
    }
    fn dump_current(&mut self) -> Value {
        let raw = self.dump_modules();
        self.bankfeeds_finalize(raw)
    }
    fn dump_modules(&mut self) -> Value {
        let company_ref: HashMap<CompanyId, String> = self.companies.iter().map(|(r, id)| (*id, r.clone())).collect();
        let account_ref: HashMap<AccountId, String> = self.accounts.iter().map(|(r, id)| (*id, r.clone())).collect();
        let journal_ref = journal_aliases::journal_aliases(&self.journals);
        let aref = |id: &AccountId| account_ref.get(id).cloned().unwrap_or_else(|| "?".into());
        let jref = |id: &JournalId| journal_ref.get(id).cloned().unwrap_or_else(|| "?".into());
        let mut companies = Vec::new();
        let mut accounts = Vec::new();
        let mut registers = Map::new();
        let mut trial_balances = Map::new();
        let mut audit = Map::new();
        let l2_audit_ids = self.l2_audit_ids();
        let mut l2_audit_ids = self.sales_audit_ids(l2_audit_ids);
        self.extend_payables_audit_ids(&mut l2_audit_ids);
        self.extend_checks_audit_ids(&mut l2_audit_ids);
        let checks = self.dump_checks();
        self.extend_pp_audit_ids(&mut l2_audit_ids);
        let prepayments = self.dump_prepayments();
        self.extend_credits_audit_ids(&mut l2_audit_ids);
        let credits = self.dump_credits();
        self.extend_banking_audit(&mut l2_audit_ids);
        self.extend_bankfeeds_audit(&mut l2_audit_ids);
        let bankfeeds = self.dump_bankfeeds();
        let banking = self.dump_banking();
        self.extend_assistant_audit(&mut l2_audit_ids);
        let assistant = self.dump_assistant();
        let payables = self.dump_payables();
        let attachments = self.dump_attachments();
        let outcomes = self.dump_step_evidence();
        // Companies in creation order (Python: order_by("pk")); UUIDv7 ids sort the same way.
        let mut ordered: Vec<(CompanyId, String)> = self.companies.iter().map(|(r, id)| (*id, r.clone())).collect();
        ordered.sort();
        for (company_id, r) in &ordered {
            let (company, mut members, company_accounts, events) = self.book.read(|conn| -> fb_core::FbResult<_> {
            let company = store::get_company(conn, *company_id)?;
            let members: Vec<String> = conn
                .prepare("SELECT u.username FROM ledger_company_members m JOIN fb_user u ON u.id = m.user_id WHERE m.company_id = ?1")
                .unwrap()
                .query_map([company_id.to_db()], |row| row.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            let company_accounts = store::accounts(conn, *company_id)?;
            let mut stmt=conn.prepare("SELECT e.action,u.username,e.actor_name,e.target,e.detail FROM ledger_auditevent e JOIN fb_user u ON u.id=e.actor_id WHERE e.company_id=?1 ORDER BY e.id")?;
            let events: Vec<Value> = stmt.query_map([company_id.to_db()], |row| {
                let detail:String=row.get(4)?;
                Ok(json!({"action":row.get::<_,String>(0)?,"actor":row.get::<_,String>(1)?,"actor_name":row.get::<_,String>(2)?,"target":row.get::<_,String>(3)?,"detail":fb_db::parse_unique_json_as::<Value>(&detail).expect("valid JSON detail")}))
            })?.collect::<Result<_,_>>()?;
            Ok((company,members,company_accounts,events))
            }).unwrap();
            members.sort();
            companies.push(json!({"ref": r, "name": company.name, "currency": company.currency, "first_run_open": company.first_run_open,
                                  "bill_reminder_days": company.bill_reminder_days, "members": members}));
            let actor = self.company_actor[r];
            for account in company_accounts {
                accounts.push(json!({"ref": aref(&account.id), "company": r, "code": account.code, "name": account.name,
                                     "kind": account.kind.as_str(), "detail_type": account.detail_type.as_str(), "is_contra": account.is_contra,
                                     "description": account.description, "active": account.active, "postable": account.postable}));
                let register = reports::register(&mut self.book, actor, *company_id, account.id, None, None).unwrap();
                let rows: Vec<Value> = register.rows.iter().map(|row| json!([jref(&row.journal_id), row.line.position, row.balance.cents() as i64])).collect();
                registers
                    .insert(aref(&account.id), json!({"opening": register.opening.cents() as i64, "closing": register.closing.cents() as i64, "rows": rows}));
            }
            let dates: Vec<String> = match self.scenario.get("as_of").and_then(|v| v.as_array()) {
                Some(d) if !d.is_empty() => d.iter().map(|v| v.as_str().unwrap().to_string()).collect(),
                _ => vec!["2099-12-31".into()],
            };
            let mut per_date = Map::new();
            for as_of in dates {
                let tb = reports::trial_balance(&mut self.book, actor, *company_id, &as_of).unwrap();
                let rows: Vec<Value> = tb.rows.iter().map(|row| json!([aref(&row.account.id), row.debit.cents() as i64, row.credit.cents() as i64])).collect();
                per_date.insert(as_of, json!({"rows": rows, "debit": tb.debit.cents() as i64, "credit": tb.credit.cents() as i64}));
            }
            trial_balances.insert(r.clone(), Value::Object(per_date));
            let refs: HashMap<String, String> = self
                .companies
                .iter()
                .map(|(r, id)| (format!("company:{id}"), format!("company:{r}")))
                .chain(self.accounts.iter().map(|(r, id)| (format!("account:{id}"), format!("account:{r}"))))
                .chain(journal_ref.iter().map(|(id, r)| (format!("journal:{id}"), format!("journal:{r}"))))
                .collect();
            let account_ids: HashMap<String, String> = self.accounts.iter().map(|(r, id)| (id.to_string(), r.clone())).collect();
            let journal_ids = self.journal_names();
            let events: Vec<Value> = events
                .into_iter()
                .map(|mut event| {
                    let is_l2 = bankfeeds::normalize_audit(&mut event, &l2_audit_ids) || assistant::normalize_audit(&mut event, &l2_audit_ids) || self.normalize_attachment_audit(&mut event, &l2_audit_ids) || banking::normalize_audit(&mut event, &l2_audit_ids) || credits::normalize_audit(&mut event, &l2_audit_ids) || prepayments::normalize_audit(&mut event, &l2_audit_ids) || checks::normalize_audit(&mut event, &l2_audit_ids) || sales::audit::normalize(&mut event, &l2_audit_ids) || payables::normalize_audit(&mut event, &l2_audit_ids) || audit::normalize(&mut event, &l2_audit_ids);
                    let target = event["target"].as_str().unwrap();
                    if !is_l2 { event["target"] = json!(refs.get(target).cloned().unwrap_or_else(|| format!("<unknown:{target}>"))); }
                    normalize_detail_ids(&mut event["detail"], &account_ids, &journal_ids);
                    event
                })
                .collect();
            audit.insert(r.clone(), json!(events));
        }
        let l2a = self.dump_l2a();
        let l2a = self.sales_normalize_l2(l2a);
        let l2a = self.payables_l2(l2a);
        let l2a = self.credits_l2(l2a);
        let l2a = self.banking_l2(l2a);
        let sales = self.dump_sales();
        let mut output = self.book
            .read(|conn| -> fb_core::FbResult<Value> {
                let mut journals = Vec::new();
                let mut display = Map::new();
                let username = |id: UserId| -> String { store::user(conn, id).unwrap().map(|u| u.username).unwrap_or_default() };
                let mut stmt = conn.prepare(&format!("SELECT {} FROM ledger_journal ORDER BY id", store::JOURNAL_COLS)).unwrap();
                let all: Vec<fb_core::model::Journal> = stmt.query_map([], store::journal_row).unwrap().collect::<Result<_, _>>().unwrap();
                for journal in all {
                    let Some(cref) = company_ref.get(&journal.company_id) else { continue };
                    let r = jref(&journal.id);
                    display.insert(r.clone(), json!(journal.display()));
                    let lines: Vec<Value> = store::lines(conn, journal.id)
                        .unwrap()
                        .into_iter()
                        .map(|l| {
                            json!({"position": l.position, "account": aref(&l.account_id), "debit": l.debit.cents() as i64, "credit": l.credit.cents() as i64,
                                "description": l.description, "name": l.name})
                        })
                        .collect();
                    journals.push(json!({
                "ref": r, "company": cref, "date": journal.date, "memo": journal.memo, "status": journal.status.as_str(),
                "entry_number": journal.entry_number, "adjusting": journal.adjusting, "revision": journal.revision,
                "reversal_of": journal.reversal_of_id.map(|o| jref(&o)), "request_key": journal.request_key,
                "created_by": username(journal.created_by_id), "posted_by": journal.posted_by_id.map(username), "lines": lines}));
                }
                Ok(json!({"outcomes": outcomes, "companies": companies, "accounts": accounts, "journals": journals, "display": display,
               "registers": registers, "trial_balances": trial_balances, "audit": audit, "l2a": l2a, "sales": sales, "payables": payables, "credits": credits, "banking": banking, "bankfeeds": bankfeeds, "assistant": assistant, "checks": checks, "prepayments": prepayments, "attachments": attachments}))
            })
            .unwrap();
        if !self.backups.results.is_empty() { output["backups"] = json!(self.backups.results); }
        if let Some(identity) = self.backup_identity() { output["book_identity"] = identity; }
        if let Some(financial_reports) = self.dump_reports() {
            output["financial_reports"] = financial_reports;
        }
        output
    }

}

fn normalize_detail_ids(value: &mut Value, accounts: &HashMap<String, String>, journals: &HashMap<String, String>) {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                let refs = match key.as_str() {
                    "account_id" => Some(accounts),
                    "reversal_id" => Some(journals),
                    _ => None,
                };
                if let Some(refs) = refs {
                    let id = value.as_str().map(str::to_string).unwrap_or_else(|| value.to_string());
                    *value = json!(refs.get(&id).cloned().unwrap_or_else(|| format!("<unknown:{id}>")));
                } else {
                    normalize_detail_ids(value, accounts, journals);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                normalize_detail_ids(value, accounts, journals);
            }
        }
        _ => {}
    }
}
// ------------------------------------------------------------------ comparer

#[derive(Default, Debug)]
pub struct Report {
    pub failures: Vec<String>,
    pub warnings: Vec<String>,
    pub checked: BTreeMap<&'static str, usize>,
    pub collation_ties: usize,
}

impl Report {
    pub fn ok(&self) -> bool {
        self.failures.is_empty()
    }
}

/// Only the generated prefix of a linked reversal may contain a side-specific display number.
/// User-entered reasons, ordinary memos and every suffix byte remain significant.
fn normalize_memo(memo: &str, journal: &Value, display: &Map<String, Value>) -> String {
    if let Some(original) = journal["reversal_of"].as_str()
        && let Some(shown) = display.get(original).and_then(Value::as_str)
    {
        let prefix = format!("Reversal of {shown}: ");
        if let Some(reason) = memo.strip_prefix(&prefix) {
            return format!("Reversal of J<{original}>: {reason}");
        }
    }
    memo.into()
}

fn reject_duplicates(report: &mut Report, label: &str, rows: &[Value], key: impl Fn(&Value) -> String) {
    let mut seen = std::collections::BTreeSet::new();
    for row in rows {
        let key = key(row);
        if !seen.insert(key.clone()) {
            report.failures.push(format!("duplicate key in {label}: {key}"));
        }
    }
}
fn same_keys(report: &mut Report, label: &str, p: &Map<String, Value>, r: &Map<String, Value>) {
    if p.keys().collect::<std::collections::BTreeSet<_>>() != r.keys().collect::<std::collections::BTreeSet<_>>() {
        report.failures.push(format!("{label} key sets differ: python {:?} rust {:?}", p.keys().collect::<Vec<_>>(), r.keys().collect::<Vec<_>>()));
    }
}

fn normalized_audit(side: &Value) -> Value {
    let mut audit = side["audit"].clone();
    let display = side["display"].as_object().unwrap();
    for events in audit.as_object_mut().unwrap().values_mut() {
        if let Some(events) = events.as_array_mut() {
            for event in events {
                let journal = event["target"]
                    .as_str()
                    .and_then(|s| s.strip_prefix("journal:"))
                    .and_then(|r| side["journals"].as_array().unwrap().iter().find(|j| j["ref"] == r));
                if let Some(journal) = journal {
                    // These are the only memo paths written by L1 services.
                    for pointer in ["/detail/memo", "/detail/before/memo", "/detail/after/memo"] {
                        if let Some(value) = event.pointer_mut(pointer)
                            && let Some(memo) = value.as_str()
                        {
                            *value = json!(normalize_memo(memo, journal, display));
                        }
                    }
                }
            }
        }
    }
    audit
}

/// Canonical id-independent order inside audit details, on every audit list of a dump or its checkpoints.
fn canonical_audit(dump: &mut Value) {
    fn events(audit: Option<&mut Value>) {
        for list in audit.and_then(Value::as_object_mut).into_iter().flat_map(|m| m.values_mut()) {
            for event in list.as_array_mut().into_iter().flatten() {
                audit::canonical_document_accounts(event);
            }
        }
    }
    events(dump.get_mut("audit"));
    events(dump.get_mut("final").and_then(|f| f.get_mut("audit")));
    for checkpoint in dump.get_mut("checkpoints").and_then(Value::as_array_mut).into_iter().flatten() {
        events(checkpoint.get_mut("dump").and_then(|d| d.get_mut("audit")));
    }
}

/// Compare two harness outputs of a scenario that requests no financial reports (any report fails: only the
/// scenario file can say which reports must exist).
pub fn compare(py: &Value, rs: &Value) -> Report {
    compare_scenario(None, py, rs)
}

/// Compare two harness outputs of `scenario`. L4 financial reports (`financial_reports`, at the top level or
/// inside each checkpoint dump) are validated against the scenario's request manifest and the same dump's
/// evidence, compared on their own, then removed before the book comparison.
pub fn compare_scenario(scenario: Option<&Value>, py: &Value, rs: &Value) -> Report {
    // Apply the reviewed, bank-feed-only Python credential divergence before main's independent
    // scenario-pin checks as well. Rust stays untouched and is still checked for persisted userinfo.
    let (mut py, mut rs) = (bankfeeds_shape::redacted_reference(py), rs.clone());
    for side in [&mut py, &mut rs] {
        canonical_audit(side);
    }
    let mut report = Report::default();
    let outcomes = |v: &Value| v.get("final").unwrap_or(v)["outcomes"].as_array().cloned().unwrap_or_default();
    let (py_outcomes, rs_outcomes) = (outcomes(&py), outcomes(&rs));
    let outcomes = (py_outcomes.as_slice(), rs_outcomes.as_slice());
    fn take(v: &mut Value) -> Option<Value> {
        v.as_object_mut().and_then(|o| o.remove("financial_reports"))
    }
    if py.get("checkpoints").is_some() || rs.get("checkpoints").is_some() {
        let count = py["checkpoints"].as_array().map(Vec::len).unwrap_or(0).max(rs["checkpoints"].as_array().map(Vec::len).unwrap_or(0));
        for index in 0..count {
            let at = |v: &mut Value| v.get_mut("checkpoints").and_then(|c| c.get_mut(index)).and_then(|c| c.get_mut("dump")).and_then(take);
            let (p, r) = (at(&mut py), at(&mut rs));
            let expected = scenario.map(|s| financial_reports::expected(s, Some(index)));
            financial_reports::compare(expected.as_ref(), p.as_ref(), r.as_ref(), (&py["checkpoints"][index]["dump"], &rs["checkpoints"][index]["dump"]), outcomes, &format!("checkpoint {index}"), &mut report);
        }
        let fin = |v: &mut Value| v.get_mut("final").and_then(take);
        let (p, r) = (fin(&mut py), fin(&mut rs));
        let expected = scenario.map(|s| financial_reports::expected(s, None));
        financial_reports::compare(expected.as_ref(), p.as_ref(), r.as_ref(), (&py["final"], &rs["final"]), outcomes, "final", &mut report);
        if py.get("checkpoints").is_none() || rs.get("checkpoints").is_none() || !py["checkpoints"].is_array() || !rs["checkpoints"].is_array() {
            report.failures.push("checkpoint envelope on one side only".into());
            return report;
        }
    } else {
        let (p, r) = (take(&mut py), take(&mut rs));
        let expected = scenario.map(|s| financial_reports::expected(s, None));
        financial_reports::compare(expected.as_ref(), p.as_ref(), r.as_ref(), (&py, &rs), outcomes, "final", &mut report);
    }
    // Each side must reach the outcome the scenario intends (success, or the named refusal), independently: two
    // identical wrong outcomes are not parity.
    for (side, label) in [(&py, "python"), (&rs, "rust")] {
        let outcomes = side.get("final").unwrap_or(side)["outcomes"].as_array().cloned().unwrap_or_default();
        for o in outcomes.iter().filter(|o| o.get("unexpected").is_some()) {
            report.failures.push(format!("{label} step {} {}: not the outcome the scenario intends ({})", o["step"], o["op"], brief(o)));
        }
        if let Some(scenario) = scenario {
            report.failures.extend(backups::intended(scenario, side).into_iter().map(|f| format!("{label} {f}")));
            report.failures.extend(intended(scenario, side).into_iter().map(|f| format!("{label} {f}")));
            report.failures.extend(sales::estimates_manifest::check(scenario, side).into_iter().map(|f| format!("{label} {f}")));
        }
    }
    let books = compare_books(&py, &rs);
    report.failures.extend(books.failures);
    report.warnings.extend(books.warnings);
    report.collation_ties += books.collation_ties;
    for (k, v) in books.checked {
        *report.checked.entry(k).or_default() += v;
    }
    report
}

/// The dump against its scenario file, not its own flags: one outcome per step in order (op and intended success
/// or refusal kind recomputed from `expect_error` / `expect_kind`), and one checkpoint per step exactly when the
/// scenario captures steps.
fn intended(scenario: &Value, side: &Value) -> Vec<String> {
    let steps = scenario["steps"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    let mut failures = Vec::new();
    let outcomes = side.get("final").unwrap_or(side)["outcomes"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    if outcomes.len() != steps.len() {
        failures.push(format!("outcomes: {} for the scenario's {} steps", outcomes.len(), steps.len()));
    }
    for (i, (o, step)) in outcomes.iter().zip(steps).enumerate() {
        let ok = o["ok"].as_bool();
        let refused_as = |kind: &str| o["kind"].as_str() == Some(kind);
        let wrong = o["step"] != json!(i)
            || o["op"] != step["op"]
            || ok.is_none()
            || ok == Some(step.get("expect_error").and_then(Value::as_bool).unwrap_or(false))
            || step.get("expect_kind").and_then(Value::as_str).is_some_and(|k| !refused_as(k))
            || step.get("expect_message").is_some_and(|m| o.get("message") != Some(m));
        if wrong {
            failures.push(format!("step {i} {}: the outcome is not the one the scenario intends ({})", step["op"], brief(o)));
        }
    }
    for step in steps.iter().filter(|s|["l2_first_run_preview","l2_reminders"].iter().any(|op|s["op"]==*op) && s["expect_error"]!=true) {
        let key=step["result"].as_str().unwrap_or("");
        let actual=&side.get("final").unwrap_or(side)["l2a"]["results"][key];
        if step.get("expect_result").is_none() || actual!=&step["expect_result"] {
            failures.push(format!("query result {key}: differs from the scenario's authored expectation"));
        }
    }
    let captured = scenario["capture_steps"] == true;
    match side.get("checkpoints").and_then(Value::as_array) {
        Some(checkpoints) if captured => {
            if checkpoints.len() != steps.len() || checkpoints.iter().enumerate().any(|(i, c)| c["step"] != json!(i)) {
                failures.push(format!("checkpoints: {} for the scenario's {} steps, or out of step order", checkpoints.len(), steps.len()));
            }
        }
        None if !captured => {}
        _ => failures.push("checkpoints: the envelope disagrees with the scenario's capture_steps".into()),
    }
    failures
}

fn compare_books(py: &Value, rs: &Value) -> Report {
    if py.get("checkpoints").is_some() || rs.get("checkpoints").is_some() {return checkpoints::compare(py,rs);}
    let mut report = Report::default();
    for (side, label) in [(py, "python"), (rs, "rust")] {
        if let Err(problem) = dump_shape::validate(side) {
            report.failures.push(format!("{label} malformed dump: {problem}"));
        }
    }
    if !report.ok() { return report; }
    // Bank-feed credentials: Rust must never keep URL userinfo; Python's known leaks are redacted the v2 way.
    for at in bankfeeds_shape::kept_credentials(rs) {
        report.failures.push(format!("rust {at}: URL credentials persisted"));
    }
    let reference = bankfeeds_shape::redacted_reference(py);
    let py = &reference;
    for (side, label) in [(py, "python"), (rs, "rust")] {
        for collection in ["companies", "accounts", "journals"] {
            reject_duplicates(&mut report, &format!("{label} {collection}"), side[collection].as_array().unwrap(), |row| row["ref"].to_string());
        }
        for journal in side["journals"].as_array().unwrap() {
            reject_duplicates(&mut report, &format!("{label} journal {} lines", journal["ref"]), journal["lines"].as_array().unwrap(), |row| {
                row["position"].to_string()
            });
        }
        reject_duplicates(&mut report, &format!("{label} outcomes"), side["outcomes"].as_array().unwrap(), |row| row["step"].to_string());
        for (account, register) in side["registers"].as_object().unwrap() {
            reject_duplicates(&mut report, &format!("{label} register {account}"), register["rows"].as_array().unwrap(), |row| {
                json!([row[0], row[1]]).to_string()
            });
        }
    }
    // Validate the Rust contract independently, even when Python has the same
    // wrong order. Oracle punctuation exceptions apply only after this check.
    check_rust_order(&mut report, "chart", rs["accounts"].as_array().unwrap(), |row| {
        (row["company"].to_string(), row["code"].as_str().unwrap().to_string())
    });
    for (company, dates) in rs["trial_balances"].as_object().unwrap() {
        for (date, report_rows) in dates.as_object().unwrap() {
            check_rust_order(&mut report, &format!("trial balance {company} {date}"), report_rows["rows"].as_array().unwrap(), |row| {
                let account = rs["accounts"].as_array().unwrap().iter().find(|a| a["ref"] == row[0]);
                (company.clone(), account.and_then(|a| a["code"].as_str()).unwrap_or("").to_string())
            });
        }
    }
    for field in ["directory", "tax", "snapshots"] {
        check_rust_order(&mut report, &format!("l2a {field}"), rs["l2a"][field].as_array().unwrap(), |row| {
            match field {
                "directory" => (json!([row["company_id"],row["model"]]).to_string(),row[if row["model"]=="item" {"code"} else {"display_name"}].as_str().unwrap().to_string()),
                "tax" => (row["company"].to_string(),row["name"].as_str().unwrap().to_string()),
                _ => (row["source"].to_string(),row["role"].as_str().unwrap().to_string()),
            }
        });
    }
    // Outcomes: ok/refused and the exception kind must match; wording is a warning.
    let (po, ro) = (py["outcomes"].as_array().unwrap(), rs["outcomes"].as_array().unwrap());
    if po.len() != ro.len() {
        report.failures.push(format!("outcome count: python {} rust {}", po.len(), ro.len()));
    }
    for (p, r) in po.iter().zip(ro) {
        *report.checked.entry("outcomes").or_default() += 1;
        if p["step"] != r["step"] || p["op"] != r["op"] || p["ok"] != r["ok"] || p.get("kind") != r.get("kind") {
            report.failures.push(format!("step {} {}: python {} / rust {}", p["step"], p["op"], brief(p), brief(r)));
        } else if p.get("message") != r.get("message") {
            report.warnings.push(format!("step {} {} wording: python {:?} / rust {:?}", p["step"], p["op"], p["message"], r["message"]));
        }
        // Each side is held to the scenario's expectation on its own: two identical wrong outcomes fail.
        for (side, label) in [(p, "python"), (r, "rust")] {
            if side["unexpected"] == true {
                report.failures.push(format!("step {} {}: {label} violated the scenario expectation ({})", side["step"], side["op"], brief(side)));
            }
            // A pinned refusal must be refused for that reason: a right operation refused for another
            // reason (an outsider's PermissionDenied for an intended sync failure) fails on each side.
            for pin in ["kind", "message"] {
                if let Some(want) = side.get(format!("expect_{pin}").as_str())
                    && side.get(pin) != Some(want)
                {
                    report.failures.push(format!("step {} {}: {label} refused for another reason than the scenario pins ({pin} {want}; got {})", side["step"], side["op"], brief(side)));
                }
            }
        }
        if p.get("expect_kind") != r.get("expect_kind") || p.get("expect_message") != r.get("expect_message") {
            report.failures.push(format!("step {} {}: the runners recorded different refusal pins", p["step"], p["op"]));
        }
        if p.get("manifest") != r.get("manifest") {
            report.failures.push(format!("step {} {}: the runners recorded different scenario manifests", p["step"], p["op"]));
        }
        if p.get("evidence") != r.get("evidence") {
            report.failures.push(format!("step {} {}: the runners read different rows behind a query list", p["step"], p["op"]));
        }
    }
    backups::compare(&mut report, py, rs);
    // Compare driver evidence as well as generated results; error wording remains a warning.
    for (p, r) in po.iter().zip(ro) {
        same(&mut report, "step audit counts", &p["audit_counts"], &r["audit_counts"]);
        same(&mut report, "step query inputs", &p["query"], &r["query"]);
    }
    same(&mut report, "attachments", &py["attachments"], &rs["attachments"]);
    same(&mut report, "companies", &py["companies"], &rs["companies"]);
    same(&mut report, "accounts", &sorted_by_ref(&py["accounts"]), &sorted_by_ref(&rs["accounts"]));
    if py["accounts"] != rs["accounts"] && sorted_by_ref(&py["accounts"]) == sorted_by_ref(&rs["accounts"]) {
        check_order(&mut report, "chart", py["accounts"].as_array().unwrap(), rs["accounts"].as_array().unwrap(),
            |row| json!([row["company"],row["ref"]]).to_string(),
            |row| (row["company"].to_string(), row["code"].as_str().unwrap().to_string()));
    }
    // Journals in creation order, memos normalized.
    let norm = |side: &Value| -> Vec<Value> {
        let display = side["display"].as_object().unwrap();
        side["journals"]
            .as_array()
            .unwrap()
            .iter()
            .map(|j| {
                let mut j = j.clone();
                j["memo"] = json!(normalize_memo(j["memo"].as_str().unwrap(), &j, display));
                j
            })
            .collect()
    };
    let (pj, rj) = (norm(py), norm(rs));
    if pj.len() != rj.len() {
        report.failures.push(format!("journal count: python {} rust {}", pj.len(), rj.len()));
    }
    for (p, r) in pj.iter().zip(&rj) {
        *report.checked.entry("journals").or_default() += 1;
        *report.checked.entry("journal lines").or_default() += p["lines"].as_array().map(|l| l.len()).unwrap_or(0);
        if p != r {
            report.failures.push(format!("journal {}: {}", p["ref"], diff(p, r)));
        }
    }
    // Registers: every row, in order.
    let (pr, rr) = (py["registers"].as_object().unwrap(), rs["registers"].as_object().unwrap());
    same_keys(&mut report, "registers", pr, rr);
    for (account, p) in pr {
        *report.checked.entry("register rows").or_default() += p["rows"].as_array().unwrap().len();
        match rr.get(account) {
            Some(r) if r == p => {}
            Some(r) => report.failures.push(format!("register {account}: {}", diff(p, r))),
            None => report.failures.push(format!("register {account} missing in rust")),
        }
    }
    if pr.len() != rr.len() {
        report.failures.push(format!("register count: python {} rust {}", pr.len(), rr.len()));
    }
    // Trial balances: exact company/date/key sets and multiplicity precede tie checks.
    let (pt, rt) = (py["trial_balances"].as_object().unwrap(), rs["trial_balances"].as_object().unwrap());
    same_keys(&mut report, "trial balance companies", pt, rt);
    for (company, dates) in pt {
        let Some(rdates) = rt.get(company).and_then(Value::as_object) else {
            report.failures.push(format!("trial balance {company} reports missing or malformed"));
            continue;
        };
        same_keys(&mut report, &format!("trial balance {company} dates"), dates.as_object().unwrap(), rdates);
        for (as_of, p) in dates.as_object().unwrap() {
            let Some(r) = rdates.get(as_of) else {
                continue;
            };
            let label = format!("trial balance {company} {as_of}");
            let (prows, rrows) = (p["rows"].as_array().unwrap(), r["rows"].as_array().unwrap());
            *report.checked.entry("trial balance rows").or_default() += prows.len();
            reject_duplicates(&mut report, &format!("python {label}"), prows, |row| row[0].to_string());
            reject_duplicates(&mut report, &format!("rust {label}"), rrows, |row| row[0].to_string());
            if p["debit"] != r["debit"] || p["credit"] != r["credit"] {
                report.failures.push(format!("{label} totals: python {}/{} rust {}/{}", p["debit"], p["credit"], r["debit"], r["credit"]));
            }
            let sorted = |rows: &[Value]| {
                let mut rows = rows.to_vec();
                rows.sort_by_key(|row| row[0].to_string());
                rows
            };
            if sorted(prows) != sorted(rrows) {
                report.failures.push(format!("{label} rows: {}", diff(&json!(sorted(prows)), &json!(sorted(rrows)))));
            } else if prows != rrows {
                check_order(&mut report, &label, prows, rrows, |row| row[0].to_string(), |row| {
                    let account=py["accounts"].as_array().unwrap().iter().find(|a| a["ref"]==row[0]).unwrap();
                    (company.clone(), account["code"].as_str().unwrap().to_string())
                });
            }
        }
    }
    *report.checked.entry("audit events").or_default() += py["audit"].as_object().unwrap().values().filter_map(Value::as_array).map(Vec::len).sum::<usize>();
    same(&mut report, "audit", &normalized_audit(py), &normalized_audit(rs));
    let mut pl2=py["l2a"].clone(); let mut rl2=rs["l2a"].clone();
    for field in ["directory","tax","snapshots"] {
        let (Some(p),Some(r))=(pl2[field].as_array(),rl2[field].as_array()) else {continue};
        let key=|row: &Value| if field=="snapshots" {json!([row["source"],row["role"]]).to_string()} else {json!([row["company_id"],row["company"],row["model"],row["id"]]).to_string()};
        reject_duplicates(&mut report,&format!("python l2a {field}"),p,key);
        reject_duplicates(&mut report,&format!("rust l2a {field}"),r,key);
        let sorted=|rows: &[Value]| {let mut rows=rows.to_vec();rows.sort_by_key(key);rows};
        if p!=r && sorted(p)==sorted(r) {
            check_order(&mut report,&format!("l2a {field}"),p,r,key,|row| {
                match field {
                    "directory" => (json!([row["company_id"],row["model"]]).to_string(),row[if row["model"]=="item" {"code"} else {"display_name"}].as_str().unwrap().to_string()),
                    "tax" => (row["company"].to_string(),row["name"].as_str().unwrap().to_string()),
                    _ => (row["source"].to_string(),row["role"].as_str().unwrap().to_string()),
                }
            });
            pl2[field]=json!(sorted(p));rl2[field]=json!(sorted(r));
        }
    }
    same(&mut report, "l2a", &pl2, &rl2);
    same(&mut report, "sales", &py["sales"], &rs["sales"]);
    same(&mut report, "payables", &py["payables"], &rs["payables"]);
    same(&mut report, "checks", &py["checks"], &rs["checks"]);
    same(&mut report, "prepayments", &py["prepayments"], &rs["prepayments"]);
    same(&mut report, "credits", &py["credits"], &rs["credits"]);
    same(&mut report, "banking", &py["banking"], &rs["banking"]);
    same(&mut report, "assistant", &py["assistant"], &rs["assistant"]);
    *report.checked.entry("bank-feed rows").or_default() += ["connections", "accounts", "transactions", "rules"].iter().map(|k| py["bankfeeds"][*k].as_array().map_or(0, Vec::len)).sum::<usize>();
    same(&mut report, "bankfeeds", &py["bankfeeds"], &rs["bankfeeds"]);
    report
}
/// Keep creation-order groups, sort each group's text with the product's
/// embedded ICU + UTF-8 comparator, then require the original rows to be identical.
fn check_rust_order(report: &mut Report, label: &str, rows: &[Value], text: impl Fn(&Value) -> (String, String)) {
    let mut groups = HashMap::new();
    for row in rows {
        let next = groups.len();
        groups.entry(text(row).0).or_insert(next);
    }
    let mut sorted = rows.to_vec();
    sorted.sort_by(|a, b| {
        let ((ag, at), (bg, bt)) = (text(a), text(b));
        groups[&ag].cmp(&groups[&bg]).then_with(|| fb_db::collation::compare(&at, &bt))
    });
    if sorted != rows {
        report.failures.push(format!("{label}: Rust order violates ICU/UTF-8 contract"));
    }
}

/// Inspect every inverted pair, not just displaced rows or adjacent neighbours.
/// Call only after exact contents/key sets/multiplicities have been checked.
fn check_order(report: &mut Report, label: &str, p: &[Value], r: &[Value], key: impl Fn(&Value)->String, text: impl Fn(&Value)->(String,String)) {
    let positions: HashMap<String,usize>=r.iter().enumerate().map(|(i,row)|(key(row),i)).collect();
    for (i,a) in p.iter().enumerate() {
        for b in &p[i+1..] {
            if positions[&key(a)] > positions[&key(b)] {
                let ((ag,at),(bg,bt))=(text(a),text(b));
                if ag==bg && fb_db::collation::punctuation_tie(&at,&bt) { report.collation_ties+=1; }
                else { report.failures.push(format!("{label}: non-punctuation order mismatch {at:?} / {bt:?}")); }
            }
        }
    }
}

fn brief(o: &Value) -> String {
    if o["ok"].as_bool() == Some(true) { "ok".into() } else { format!("{} {:?}", o["kind"].as_str().unwrap_or("?"), o["message"].as_str().unwrap_or("")) }
}

fn sorted_by_ref(v: &Value) -> Value {
    let mut items = v.as_array().cloned().unwrap_or_default();
    items.sort_by(|a, b| (a["company"].to_string(), a["ref"].to_string()).cmp(&(b["company"].to_string(), b["ref"].to_string())));
    Value::Array(items)
}

fn same(report: &mut Report, label: &'static str, p: &Value, r: &Value) {
    *report.checked.entry(label).or_default() += p.as_array().map(|a| a.len()).or_else(|| p.as_object().map(|o| o.len())).unwrap_or(1);
    if p != r {
        report.failures.push(format!("{label}: {}", diff(p, r)));
    }
}

/// The first differing path between two JSON values.
pub fn diff(p: &Value, r: &Value) -> String {
    fn walk(p: &Value, r: &Value, path: String) -> Option<String> {
        match (p, r) {
            (Value::Object(a), Value::Object(b)) => {
                for (k, v) in a {
                    match b.get(k) {
                        Some(w) => {
                            if let Some(d) = walk(v, w, format!("{path}.{k}")) {
                                return Some(d);
                            }
                        }
                        None => return Some(format!("{path}.{k}: missing in rust")),
                    }
                }
                b.keys().find(|k| !a.contains_key(*k)).map(|k| format!("{path}.{k}: extra in rust"))
            }
            (Value::Array(a), Value::Array(b)) => {
                for (i, (v, w)) in a.iter().zip(b).enumerate() {
                    if let Some(d) = walk(v, w, format!("{path}[{i}]")) {
                        return Some(d);
                    }
                }
                (a.len() != b.len()).then(|| format!("{path}: length python {} rust {}", a.len(), b.len()))
            }
            _ if p == r => None,
            _ => Some(format!("{path}: python {p} / rust {r}")),
        }
    }
    walk(p, r, String::new()).unwrap_or_else(|| "equal".into())
}

#[cfg(test)]
mod core_l3_tests;

#[cfg(test)]
mod journal_alias_tests;
