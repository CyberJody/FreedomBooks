//! L4 financial reports: fb-core's P&L, balance sheet and general ledger on both bases, plus the report-control
//! forms, in the same normalized shape as `py/reports_adapter.py`. Each side echoes the scenario's request manifest.
//! The comparer validates both sides on their own first (identical incomplete or corrupted dumps fail). The
//! scenario file, never a dump, fixes the expected reports and companies, and the echo must equal it. Every row,
//! total, ledger row (with its order and display cells), CSV row and unconverted count is recomputed from the same
//! dump's posted journals: the cash basis drops the lines its settlement evidence excludes and adds the recognised
//! shares rebuilt from the settlement records with the cent rule. Form results are typed. Then exact equality.
use super::*;
use chrono::NaiveDate;
use fb_core::reports::financial::{self, Basis};
use fb_core::model::DetailType;
use fb_core::reports::params::{self, RangeInput};
use std::collections::{BTreeMap, BTreeSet};

const BASES: [Basis; 2] = [Basis::Accrual, Basis::Cash];

fn day(text: &Value) -> Option<NaiveDate> {
    text.as_str().filter(|s| !s.is_empty()).map(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").expect("spec date"))
}

fn cents(m: Money) -> i64 {
    i64::try_from(m.cents()).expect("report amount within i64")
}

use fb_core::Money;

fn period_key(start: &Value, end: &Value) -> String {
    format!("{}..{}", start.as_str().unwrap_or(""), end.as_str().unwrap_or(""))
}

impl Runner<'_> {
    pub(super) fn dump_reports(&mut self) -> Option<Value> {
        let spec = self.scenario.get("financial_reports")?.clone();
        let arefs: HashMap<AccountId, String> = self.accounts.iter().map(|(r, id)| (*id, r.clone())).collect();
        let aref = |id: &AccountId| arefs.get(id).cloned().unwrap_or_else(|| "?".into());
        let mut companies = Map::new();
        let mut ordered: Vec<(CompanyId, String)> = self.companies.iter().map(|(r, id)| (*id, r.clone())).collect();
        ordered.sort();
        let empty = vec![];
        for (company, cref) in ordered {
            let actor = self.company_actor[&cref];
            let (ranks, numbers) = self
                .book
                .read(|tx| -> fb_core::FbResult<(HashMap<String, usize>, HashMap<String, String>)> {
                    let mut stmt = tx.prepare("SELECT id FROM ledger_journal WHERE company_id = ?1 ORDER BY number")?;
                    let ranks = stmt.query_map([company.to_db()], |r| r.get::<_, String>(0))?.enumerate().map(|(i, id)| id.map(|id| (id, i))).collect::<Result<_, _>>()?;
                    let mut stmt = tx.prepare("SELECT id, number FROM documents_documentidentity WHERE company_id = ?1")?;
                    let numbers = stmt.query_map([company.to_db()], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
                    Ok((ranks, numbers))
                })
                .unwrap();
            let mut per_basis = Map::new();
            for basis in BASES {
                let (mut pl_out, mut bs_out, mut gl_out) = (Map::new(), Map::new(), Map::new());
                for period in spec["profit_loss"].as_array().unwrap_or(&empty) {
                    let report = financial::profit_and_loss(&mut self.book, actor, company, basis, day(&period[0]), day(&period[1]).unwrap()).unwrap();
                    pl_out.insert(period_key(&period[0], &period[1]), pl_dump(&report, &aref));
                }
                for as_of in spec["balance_sheet"].as_array().unwrap_or(&empty) {
                    let report = financial::balance_sheet(&mut self.book, actor, company, basis, day(as_of).unwrap()).unwrap();
                    bs_out.insert(as_of.as_str().unwrap().to_string(), bs_dump(&report, &aref));
                }
                for item in spec["general_ledger"].as_array().unwrap_or(&empty) {
                    let group = item[2].as_str().unwrap_or("");
                    let account_ref = item[3].as_str();
                    let key = format!("{}|{}|{}", period_key(&item[0], &item[1]), group, account_ref.unwrap_or(""));
                    let account = match account_ref {
                        Some(r) => match self.accounts.get(r) {
                            Some(id) => Some(*id),
                            None => {
                                gl_out.insert(key, Value::Null);
                                continue;
                            }
                        },
                        None => None,
                    };
                    match financial::general_ledger(&mut self.book, actor, company, basis, day(&item[0]), day(&item[1]).unwrap(), account, group) {
                        Ok(report) => gl_out.insert(key, gl_dump(&report, &aref, &ranks, &numbers)),
                        // Another company's account (Python: Http404 before the report runs).
                        Err(FbError::DoesNotExist(_)) => gl_out.insert(key, Value::Null),
                        Err(e) => panic!("general ledger: {e}"),
                    };
                }
                per_basis.insert(basis.as_str().into(), json!({"profit_loss": pl_out, "balance_sheet": bs_out, "general_ledger": gl_out}));
            }
            companies.insert(cref, Value::Object(per_basis));
        }
        let forms: Vec<Value> = spec["forms"].as_array().unwrap_or(&empty).iter().map(form_dump).collect();
        // The manifest: exactly the reports the scenario requested, echoed so the comparer can require all of them.
        let requests = json!({"profit_loss": spec.get("profit_loss").cloned().unwrap_or(json!([])),
                              "balance_sheet": spec.get("balance_sheet").cloned().unwrap_or(json!([])),
                              "general_ledger": spec.get("general_ledger").cloned().unwrap_or(json!([])),
                              "forms": spec.get("forms").cloned().unwrap_or(json!([]))});
        Some(json!({"companies": companies, "forms": forms, "requests": requests}))
    }
}

fn rows_of(items: &[financial::AmountRow], aref: &dyn Fn(&AccountId) -> String) -> Value {
    json!(items.iter().map(|r| json!([aref(&r.account.id), cents(r.amount)])).collect::<Vec<_>>())
}

fn csv_dump(rows: &[params::DisplayRow], aref: &dyn Fn(&AccountId) -> String) -> Value {
    let rows: Vec<params::DisplayRow> = rows.iter().filter(|r| r.kind != params::RowKind::Spacer).cloned().collect();
    let csv = params::csv_rows(&rows);
    let amounts: Vec<&params::DisplayRow> = rows.iter().filter(|r| !matches!(r.kind, params::RowKind::Heading | params::RowKind::Spacer)).collect();
    json!(csv
        .into_iter()
        .zip(amounts)
        .map(|(c, row)| json!([c[0], c[1], c[2], c[3], c[4], row.account.as_ref().map(|a| aref(&a.id)).unwrap_or_default(), row.amount.map(cents)]))
        .collect::<Vec<_>>())
}

fn pl_dump(report: &financial::ProfitAndLoss, aref: &dyn Fn(&AccountId) -> String) -> Value {
    let mut sections = Map::new();
    for s in &report.sections {
        sections.insert(s.key.into(), json!({"rows": rows_of(&s.rows, aref), "total": cents(s.total)}));
    }
    json!({"sections": sections, "gross_profit": cents(report.gross_profit), "net_ordinary_income": cents(report.net_ordinary_income),
           "net_other_income": cents(report.net_other_income), "net_income": cents(report.net_income), "unconverted": report.unconverted,
           "show_cogs": report.show_cogs, "show_other": report.show_other, "csv": csv_dump(&params::pl_rows(report), aref)})
}

fn bs_dump(report: &financial::BalanceSheet, aref: &dyn Fn(&AccountId) -> String) -> Value {
    let section = |s: &financial::BsSection| {
        json!({"rows": rows_of(&s.rows, aref), "total": cents(s.total), "groups": s.groups.iter().map(|g| json!({
            "key": g.key, "nested": g.nested, "total": cents(g.total),
            "types": g.types.iter().map(|t| json!({"type": t.detail_type.as_str(), "rows": rows_of(&t.rows, aref), "total": cents(t.total)})).collect::<Vec<_>>()
        })).collect::<Vec<_>>()})
    };
    json!({"assets": section(&report.assets), "liabilities": section(&report.liabilities), "equity": section(&report.equity),
           "fiscal_year_start": report.fiscal_year_start.to_string(), "retained_earnings": cents(report.retained_earnings),
           "net_income": cents(report.net_income), "total_equity": cents(report.total_equity),
           "total_liabilities_equity": cents(report.total_liabilities_equity), "balanced": report.balanced,
           "unconverted": report.unconverted, "csv": csv_dump(&params::bs_rows(report), aref)})
}

fn gl_dump(report: &financial::GeneralLedger, aref: &dyn Fn(&AccountId) -> String, ranks: &HashMap<String, usize>, numbers: &HashMap<String, String>) -> Value {
    let sections: Vec<Value> = report
        .sections
        .iter()
        .map(|s| {
            let rows: Vec<Value> = s
                .rows
                .iter()
                .map(|r| {
                    json!([r.date.to_string(), cents(r.debit), cents(r.credit), r.journal.as_ref().map(|j| format!("j{}", ranks[&j.0])),
                           r.source_id.as_ref().and_then(|id| numbers.get(id)), r.reversal, r.entry_number, r.memo, r.name, r.synthetic, cents(r.balance)])
                })
                .collect();
            json!({"account": aref(&s.account.id), "opening": cents(s.opening), "debit": cents(s.debit), "credit": cents(s.credit),
                   "closing": cents(s.closing), "rows": rows})
        })
        .collect();
    json!({"sections": sections, "debit": cents(report.debit), "credit": cents(report.credit), "unconverted": report.unconverted})
}

fn form_dump(case: &Value) -> Value {
    let today = day(&case["today"]).expect("form today");
    let data = &case["data"];
    let get = |key: &str| data.get(key).and_then(Value::as_str);
    let input = RangeInput { basis: get("basis"), preset: get("preset"), start: get("start"), end: get("end") };
    let refused = |e: FbError| json!({"ok": false, "errors": e.messages()});
    let date = |d: Option<NaiveDate>| d.map(|d| d.to_string());
    match case["form"].as_str() {
        Some("range") => match params::range(&input, today) {
            Ok(r) => json!({"ok": true, "basis": r.basis.as_str(), "preset": r.preset, "start": date(r.start), "end": r.end.to_string()}),
            Err(e) => refused(e),
        },
        Some("ledger") => match params::ledger_range(&input, get("account"), get("group"), today) {
            Ok(l) => json!({"ok": true, "basis": l.range.basis.as_str(), "preset": l.range.preset, "start": date(l.range.start),
                            "end": l.range.end.to_string(), "group": l.group}),
            Err(e) => refused(e),
        },
        Some("as_of") => match params::as_of(get("basis"), get("as_of"), today) {
            Ok((basis, as_of)) => json!({"ok": true, "basis": basis.as_str(), "as_of": as_of.to_string()}),
            Err(e) => refused(e),
        },
        other => panic!("unknown report form {other:?}"),
    }
}

// Validation and comparison ------------------------------------------------------------------------------

type Check = Result<(), String>;

fn keys_exact(v: &Value, want: &[&str], at: &str) -> Check {
    let got: BTreeSet<&str> = v.as_object().ok_or(format!("{at}: not an object"))?.keys().map(String::as_str).collect();
    let want: BTreeSet<&str> = want.iter().copied().collect();
    if got != want {
        return Err(format!("{at}: keys {got:?}, expected {want:?}"));
    }
    Ok(())
}

fn int(v: &Value, at: &str) -> Result<i64, String> {
    v.as_i64().ok_or(format!("{at}: not an integer"))
}

fn count(v: &Value, at: &str) -> Result<i64, String> {
    int(v, at).and_then(|n| if n >= 0 { Ok(n) } else { Err(format!("{at}: negative count")) })
}

fn arr<'v>(v: &'v Value, at: &str) -> Result<&'v Vec<Value>, String> {
    v.as_array().ok_or(format!("{at}: not an array"))
}

fn text<'v>(v: &'v Value, at: &str) -> Result<&'v str, String> {
    v.as_str().ok_or(format!("{at}: not a string"))
}

fn checked_sum(values: impl IntoIterator<Item = i64>, at: &str) -> Result<i64, String> {
    values.into_iter().try_fold(0i64, |a, b| a.checked_add(b)).ok_or(format!("{at}: total overflows"))
}

/// A canonical `YYYY-MM-DD` date; never sliced or coerced.
fn canonical_date(v: &Value, at: &str) -> Result<NaiveDate, String> {
    let raw = text(v, at)?;
    let date = NaiveDate::parse_from_str(raw, "%Y-%m-%d").map_err(|_| format!("{at}: not a date"))?;
    if date.format("%Y-%m-%d").to_string() != raw {
        return Err(format!("{at}: not a canonical date"));
    }
    Ok(date)
}

/// A canonical requested or cleaned report date, inside the report bounds (1900-01-02..9999-12-30).
fn date_text(v: &Value, at: &str) -> Result<NaiveDate, String> {
    let date = canonical_date(v, at)?;
    if !(params::min_date()..=params::max_date()).contains(&date) {
        return Err(format!("{at}: outside the report date bounds"));
    }
    Ok(date)
}

fn optional_date(v: &Value, at: &str) -> Result<Option<NaiveDate>, String> {
    if v.is_null() { Ok(None) } else { date_text(v, at).map(Some) }
}

fn period_text(start: Option<NaiveDate>, end: NaiveDate) -> String {
    format!("{}..{end}", start.map(|s| s.to_string()).unwrap_or_default())
}

// The expected-report manifest ------------------------------------------------------------------------------

/// What the scenario file (never the dumps) fixes for one dump: the requested reports, the companies its
/// `create_company` steps made by then, and the account creation steps that fix the accounts' creation order (the
/// cent rule's tie-break).
pub(super) struct Expected<'s> {
    /// The scenario's `financial_reports` spec; None when it requests none.
    pub spec: Option<&'s Value>,
    pub companies: BTreeSet<String>,
    /// (step, account ref) of every `save_account` creation the scenario does not pin as refused.
    pub creations: Vec<(usize, String)>,
}

impl Expected<'_> {
    /// The accounts' creation order on one side: only the creations that side's own outcomes record as
    /// successful take a rank (a refused attempt creates no account, so a later retry of its ref is ranked
    /// where it actually happened).
    pub(super) fn creation_order(&self, outcomes: &[Value]) -> HashMap<String, usize> {
        let mut order = HashMap::new();
        for (step, r) in &self.creations {
            if outcomes.get(*step).is_some_and(|o| o["step"] == json!(step) && o["ok"] == true) {
                let next = order.len();
                order.entry(r.clone()).or_insert(next);
            }
        }
        order
    }
}

/// The expectation for the dump taken after step `through` (None: the final dump).
pub(super) fn expected(scenario: &Value, through: Option<usize>) -> Expected<'_> {
    let steps = scenario["steps"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    let upto = through.map_or(steps.len(), |i| (i + 1).min(steps.len()));
    let mut companies = BTreeSet::new();
    let mut creations = Vec::new();
    for (i, step) in steps[..upto].iter().enumerate() {
        let refused = step.get("expect_error").and_then(Value::as_bool) == Some(true);
        let Some(r) = step.get("ref").and_then(Value::as_str) else { continue };
        match step["op"].as_str() {
            Some("create_company") if !refused => {
                companies.insert(r.to_string());
            }
            Some("save_account") if !refused && step.get("account").is_none() => creations.push((i, r.to_string())),
            _ => {}
        }
    }
    Expected { spec: scenario.get("financial_reports"), companies, creations }
}

/// The spec as both sides echo it (`requests`): the four families, absent ones empty, nothing else.
fn echo(spec: &Value) -> Result<Value, String> {
    let object = spec.as_object().ok_or("scenario financial_reports: not an object")?;
    if let Some(extra) = object.keys().find(|k| !matches!(k.as_str(), "profit_loss" | "balance_sheet" | "general_ledger" | "forms")) {
        return Err(format!("scenario financial_reports: unknown key {extra}"));
    }
    Ok(json!({"profit_loss": spec.get("profit_loss").cloned().unwrap_or(json!([])),
              "balance_sheet": spec.get("balance_sheet").cloned().unwrap_or(json!([])),
              "general_ledger": spec.get("general_ledger").cloned().unwrap_or(json!([])),
              "forms": spec.get("forms").cloned().unwrap_or(json!([]))}))
}

/// One requested general ledger.
struct LedgerRequest {
    key: String,
    start: Option<NaiveDate>,
    end: NaiveDate,
    group: String,
    account: Option<String>,
}

/// The scenario's `financial_reports` spec: exactly which reports must exist.
struct Manifest {
    pl: Vec<(String, Option<NaiveDate>, NaiveDate)>,
    bs: Vec<(String, NaiveDate)>,
    gl: Vec<LedgerRequest>,
    forms: Vec<String>,
}

fn manifest(v: &Value) -> Result<Manifest, String> {
    let at = "requests";
    keys_exact(v, &["profit_loss", "balance_sheet", "general_ledger", "forms"], at)?;
    let mut seen = BTreeSet::new();
    let mut unique = |family: &str, key: &str| -> Check {
        if seen.insert(format!("{family} {key}")) { Ok(()) } else { Err(format!("{at}: duplicate {family} request {key}")) }
    };
    let range = |item: &Value, len: usize, at: &str| -> Result<(Option<NaiveDate>, NaiveDate), String> {
        let cells = arr(item, at)?;
        if cells.len() != len {
            return Err(format!("{at}: expected {len} cells"));
        }
        let (start, end) = (optional_date(&cells[0], at)?, date_text(&cells[1], at)?);
        if start.is_some_and(|s| s > end) {
            return Err(format!("{at}: start after end"));
        }
        Ok((start, end))
    };
    let mut pl = Vec::new();
    for (i, item) in arr(&v["profit_loss"], at)?.iter().enumerate() {
        let (start, end) = range(item, 2, &format!("{at} profit_loss[{i}]"))?;
        let key = period_text(start, end);
        unique("profit_loss", &key)?;
        pl.push((key, start, end));
    }
    let mut bs = Vec::new();
    for (i, item) in arr(&v["balance_sheet"], at)?.iter().enumerate() {
        let date = date_text(item, &format!("{at} balance_sheet[{i}]"))?;
        unique("balance_sheet", &date.to_string())?;
        bs.push((date.to_string(), date));
    }
    let mut gl = Vec::new();
    for (i, item) in arr(&v["general_ledger"], at)?.iter().enumerate() {
        let iat = format!("{at} general_ledger[{i}]");
        let (start, end) = range(item, 4, &iat)?;
        let group = text(&item[2], &iat)?.to_string();
        if financial::group_types(&group).is_none() {
            return Err(format!("{iat}: unknown account group"));
        }
        let account = match &item[3] {
            Value::Null => None,
            Value::String(r) if !r.is_empty() => Some(r.clone()),
            _ => return Err(format!("{iat}: account must be null or a reference")),
        };
        let key = format!("{}|{group}|{}", period_text(start, end), account.as_deref().unwrap_or(""));
        unique("general_ledger", &key)?;
        gl.push(LedgerRequest { key, start, end, group, account });
    }
    let mut forms = Vec::new();
    for (i, item) in arr(&v["forms"], at)?.iter().enumerate() {
        let fat = format!("{at} forms[{i}]");
        keys_exact(item, &["form", "data", "today"], &fat)?;
        let kind = text(&item["form"], &fat)?;
        if !matches!(kind, "range" | "ledger" | "as_of") {
            return Err(format!("{fat}: unknown form"));
        }
        if !item["data"].as_object().ok_or(format!("{fat}: data not an object"))?.values().all(Value::is_string) {
            return Err(format!("{fat}: data values must be strings"));
        }
        date_text(&item["today"], &fat)?;
        forms.push(kind.to_string());
    }
    if pl.is_empty() || bs.is_empty() || gl.is_empty() {
        return Err(format!("{at}: every report family needs at least one request"));
    }
    Ok(Manifest { pl, bs, gl, forms })
}

// Journal and settlement evidence of the same dump -------------------------------------------------------------

/// A company account from the dump's chart.
struct ChartAccount {
    code: String,
    name: String,
    kind: String,
    detail: DetailType,
}

impl ChartAccount {
    /// Section side (Python `Account.section_balance`): +1 debit (asset, expense), -1 credit.
    fn sign(&self) -> i64 {
        if matches!(self.kind.as_str(), "ASSET" | "EXPENSE") { 1 } else { -1 }
    }
}

/// One ledger entry exactly as a general-ledger row shows it (without the running balance): a posted line, or
/// (cash basis) a recognised share.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Entry {
    date: NaiveDate,
    debit: i64,
    credit: i64,
    journal: Option<String>,
    source: Option<String>,
    reversal: bool,
    entry_number: String,
    memo: String,
    name: String,
    synthetic: Option<String>,
}

impl Entry {
    fn net(&self) -> Result<i64, String> {
        self.debit.checked_sub(self.credit).ok_or("entry overflows".into())
    }
}

/// Python's general-ledger sort key: (date, journal pk or 10**15, position or 10**6 + family, row pk); the
/// journal is its creation rank, the settlement row its dump (pk) order.
type RowKey = (NaiveDate, usize, i64, usize);

/// Every company account (chart order: code, then dump order), the per-basis entries per account in the order
/// they are generated, and the dates of the posted lines the cash basis leaves unconverted.
struct Evidence {
    chart: Vec<(String, ChartAccount)>,
    entries: HashMap<String, Vec<(RowKey, Entry)>>,
    unconverted: Vec<NaiveDate>,
}

impl Evidence {
    fn account(&self, r: &str) -> Option<&ChartAccount> {
        self.chart.iter().find(|(k, _)| k == r).map(|(_, a)| a)
    }

    fn net(&self, account: &str, start: Option<NaiveDate>, end: NaiveDate) -> Result<(i64, bool), String> {
        let (mut net, mut synthetic, mut real) = (0i64, 0i64, false);
        for (_, e) in self.entries.get(account).into_iter().flatten().filter(|(_, e)| start.is_none_or(|s| e.date >= s) && e.date <= end) {
            net = net.checked_add(e.net()?).ok_or("net overflows")?;
            if e.synthetic.is_some() {
                synthetic = synthetic.checked_add(e.net()?).ok_or("net overflows")?;
            } else {
                real = true;
            }
        }
        // Python `nets` activity: a posted line, or a non-zero recognised total for the period.
        Ok((net, real || synthetic != 0))
    }

    /// `_unconverted` through `end`; always 0 on the accrual basis (the list is empty there).
    fn unconverted(&self, end: NaiveDate) -> i64 {
        self.unconverted.iter().filter(|d| **d <= end).count() as i64
    }
}

fn list<'v>(v: &'v Value, at: &str) -> Result<&'v [Value], String> {
    if v.is_null() { Ok(&[]) } else { arr(v, at).map(Vec::as_slice) }
}

/// A money cell of the dump in cents: integer cents, or a plain two-place decimal string.
fn money(v: &Value, at: &str) -> Result<i64, String> {
    if let Some(n) = v.as_i64() {
        return Ok(n);
    }
    let raw = text(v, at)?;
    let (negative, digits) = raw.strip_prefix('-').map_or((false, raw), |d| (true, d));
    let (whole, cents) = digits.split_once('.').ok_or(format!("{at}: not a money amount"))?;
    if whole.is_empty() || cents.len() != 2 || !whole.bytes().chain(cents.bytes()).all(|b| b.is_ascii_digit()) {
        return Err(format!("{at}: not a money amount"));
    }
    let value = whole.parse::<i64>().ok().and_then(|w| w.checked_mul(100)).and_then(|w| w.checked_add(cents.parse::<i64>().ok()?));
    let value = value.ok_or(format!("{at}: money amount overflows"))?;
    if negative { value.checked_neg().ok_or(format!("{at}: money amount overflows")) } else { Ok(value) }
}

/// A stored settlement amount (allocation, application line, refund): 0..=MAX_TOTAL cents, as the books store
/// it. Anything else is malformed evidence, never arithmetic input.
fn settlement_amount(v: &Value, at: &str) -> Result<i64, String> {
    let amount = money(v, at)?;
    if !(0..=fb_money::MAX_TOTAL_CENTS).contains(&i128::from(amount)) {
        return Err(format!("{at}: {amount} is not a stored amount (0 to MAX_TOTAL cents)"));
    }
    Ok(amount)
}

/// One journal of the dump.
struct Journal<'v> {
    /// The report's `j<rank>`: creation rank among the company's journals.
    rank: usize,
    date: NaiveDate,
    posted: bool,
    identity: Option<&'v str>,
}

/// One document identity of the dump.
struct Identity<'v> {
    company: &'v str,
    kind: &'v str,
    number: &'v str,
}

/// The dump's journals and identities by reference.
struct Index<'v> {
    journals: HashMap<&'v str, Journal<'v>>,
    identities: HashMap<&'v str, Identity<'v>>,
}

impl<'v> Index<'v> {
    fn new(dump: &'v Value) -> Result<Index<'v>, String> {
        let mut identities = HashMap::new();
        for identity in list(&dump["l2a"]["identities"], "identities")? {
            let id = text(&identity["id"], "identity")?;
            let found = Identity { company: text(&identity["company_id"], "identity company")?, kind: text(&identity["kind"], "identity kind")?, number: text(&identity["number"], "identity number")? };
            if identities.insert(id, found).is_some() {
                return Err(format!("identities: duplicate {id}"));
            }
        }
        let mut identity_of: HashMap<&str, &str> = HashMap::new();
        for pair in list(&dump["l2a"]["source_journals"], "source journals")? {
            let (identity, journal) = (text(&pair[0], "source identity")?, text(&pair[1], "source journal")?);
            if !identities.contains_key(identity) || identity_of.insert(journal, identity).is_some_and(|other| other != identity) {
                return Err(format!("source journals: {journal} has no single known source"));
            }
        }
        let mut journals = HashMap::new();
        let mut ranks: HashMap<&str, usize> = HashMap::new();
        for j in list(&dump["journals"], "journals")? {
            let r = text(&j["ref"], "journal ref")?;
            let company = text(&j["company"], "journal company")?;
            let rank = ranks.entry(company).or_default();
            let identity = identity_of.get(r).copied();
            if identity.is_some_and(|i| identities[i].company != company) {
                return Err(format!("journal {r}: its source belongs to another company"));
            }
            let journal = Journal { rank: *rank, date: canonical_date(&j["date"], "journal date")?, posted: j["status"] == "POSTED", identity };
            *rank += 1;
            if journals.insert(r, journal).is_some() {
                return Err(format!("journals: duplicate {r}"));
            }
        }
        Ok(Index { journals, identities })
    }

    fn journal(&self, r: &Value, at: &str) -> Result<&Journal<'v>, String> {
        let r = text(r, at)?;
        self.journals.get(r).ok_or(format!("{at}: unknown journal {r}"))
    }

    /// A settlement journal that counts: posted (Python filters `status = POSTED`).
    fn posted(&self, r: &Value, at: &str) -> Result<Option<&Journal<'v>>, String> {
        if r.is_null() {
            return Ok(None);
        }
        let j = self.journal(r, at)?;
        Ok(j.posted.then_some(j))
    }

    fn number(&self, identity: &str) -> Option<String> {
        self.identities.get(identity).map(|i| i.number.to_string())
    }
}

/// One settlement event (Python `Event`), rebuilt from the dump's settlement records.
struct Event<'v> {
    doc: &'v str,
    date: NaiveDate,
    amount: i64,
    /// (date, family, settlement row in dump order): Python's (date, family, row pk).
    order: (NaiveDate, u8, usize),
    source: Option<&'v str>,
    journal: Option<&'v str>,
    control: &'v str,
    note: &'static str,
}

/// `_events` from the dump: receipt (RCP) and bill-payment allocations, credit-application lines (both the
/// invoice and the memo), credit refunds and deposit applications, each with its reversal, unapplication or
/// void releasing the same amount on its own date. Settlement rows are dumped in creation (pk) order.
fn settlement_events<'v>(dump: &'v Value, index: &Index<'v>) -> Result<Vec<Event<'v>>, String> {
    let mut events = Vec::new();
    let mut seq = 0usize;
    let mut invoices: HashMap<&str, &str> = HashMap::new();
    for invoice in list(&dump["sales"]["invoices"], "invoices")? {
        let r = text(&invoice["ref"], "invoice ref")?;
        if let Some(journal) = list(&invoice["history"], "invoice history")?.iter().find_map(|h| h["journal_id"].as_str()) {
            let identity = index.journals.get(journal).and_then(|j| j.identity).ok_or(format!("invoice {r}: its journal has no source"))?;
            invoices.insert(r, identity);
        }
    }
    let invoice = |v: &Value, at: &str| -> Result<&'v str, String> {
        let r = text(v, at)?;
        invoices.get(r).copied().ok_or(format!("{at}: invoice {r} was never posted"))
    };
    // The settling event, then its release (same amount, negative) on the undo date when there is one.
    let settle = |events: &mut Vec<Event<'v>>, event: Event<'v>, undo: Option<(Option<&'v str>, NaiveDate)>, note: &'static str| -> Check {
        if let Some((journal, date)) = undo {
            let order = (date, event.order.1 + 1, event.order.2);
            let amount = event.amount.checked_neg().ok_or(format!("{note}: released amount overflows"))?;
            events.push(Event { doc: event.doc, date, amount, order, source: event.source, journal, control: event.control, note });
        }
        events.push(event);
        Ok(())
    };
    // A posted undo journal: (reference, date).
    let undo_journal = |v: &'v Value, at: &str| -> Result<Option<(Option<&'v str>, NaiveDate)>, String> {
        Ok(index.posted(v, at)?.map(|j| (v.as_str(), j.date)))
    };
    // Customer receipts (kind RCP) allocated to invoices.
    for receipt in list(&dump["sales"]["receipts"], "receipts")? {
        if receipt["kind"] != "RCP" {
            continue;
        }
        let allocations = list(&receipt["allocations"], "allocations")?;
        let amounts = allocations.iter().map(|a| settlement_amount(&a["amount"], "allocation amount")).collect::<Result<Vec<_>, _>>()?;
        let Some(j) = index.posted(&receipt["journal_id"], "receipt journal")? else { continue };
        let reversed = undo_journal(&receipt["reversal"]["journal_id"], "receipt reversal journal")?;
        for (allocation, amount) in allocations.iter().zip(amounts) {
            seq += 1;
            let event = Event {
                doc: invoice(&allocation["invoice_id"], "receipt allocation")?,
                date: j.date,
                amount,
                order: (j.date, 0, seq),
                source: j.identity,
                journal: receipt["journal_id"].as_str(),
                control: text(&allocation["receivables_id"], "allocation receivables")?,
                note: "receipt",
            };
            settle(&mut events, event, reversed, "receipt reversal")?;
        }
    }
    let p = &dump["payables"];
    let bills: HashMap<&str, &str> = list(&p["bill"], "bills")?.iter().filter_map(|b| Some((b["id"].as_str()?, b["identity_id"].as_str()?))).collect();
    let payments: HashMap<&str, &Value> = list(&p["billpayment"], "bill payments")?.iter().filter_map(|b| Some((b["id"].as_str()?, b))).collect();
    let payment_undo: HashMap<&str, &Value> = list(&p["paymentreversal"], "payment reversals")?.iter().filter_map(|r| Some((r["payment_id"].as_str()?, &r["journal_id"]))).collect();
    for allocation in list(&p["paymentallocation"], "payment allocations")? {
        seq += 1;
        let pid = text(&allocation["payment_id"], "payment allocation")?;
        let amount = settlement_amount(&allocation["amount"], "payment amount")?;
        let payment = payments.get(pid).ok_or(format!("payment allocation: unknown payment {pid}"))?;
        let Some(j) = index.posted(&payment["journal_id"], "bill payment journal")? else { continue };
        let bill = text(&allocation["bill_id"], "payment allocation bill")?;
        let reversed = match payment_undo.get(pid) {
            Some(r) => undo_journal(r, "payment reversal journal")?,
            None => None,
        };
        let event = Event {
            doc: bills.get(bill).copied().ok_or(format!("payment allocation: unknown bill {bill}"))?,
            date: j.date,
            amount,
            order: (j.date, 2, seq),
            source: payment["identity_id"].as_str(),
            journal: payment["journal_id"].as_str(),
            control: text(&allocation["payables_id"], "allocation payables")?,
            note: "bill payment",
        };
        settle(&mut events, event, reversed, "bill payment reversal")?;
    }
    let c = &dump["credits"];
    let memos: HashMap<&str, &Value> = list(&c["creditmemo"], "credit memos")?.iter().filter_map(|m| Some((m["id"].as_str()?, m))).collect();
    let memo = |v: &Value, at: &str| -> Result<(&'v str, &'v str), String> {
        let r = text(v, at)?;
        let m = memos.get(r).ok_or(format!("{at}: unknown credit memo {r}"))?;
        Ok((text(&m["identity_id"], "credit memo identity")?, text(&m["receivables_id"], "credit memo receivables")?))
    };
    let applications: HashMap<&str, &Value> = list(&c["creditapplication"], "credit applications")?.iter().filter_map(|a| Some((a["id"].as_str()?, a))).collect();
    let unapplied: HashMap<&str, &Value> = list(&c["creditunapplication"], "credit unapplications")?.iter().filter_map(|u| Some((u["application_id"].as_str()?, &u["date"]))).collect();
    // Credit applications settle both the invoice and the memo; no journal, no status filter.
    for line in list(&c["creditapplicationline"], "credit application lines")? {
        seq += 1;
        let aid = text(&line["application_id"], "credit application line")?;
        let application = applications.get(aid).ok_or(format!("credit application line: unknown application {aid}"))?;
        let date = canonical_date(&application["date"], "credit application date")?;
        let undone = unapplied.get(aid).map(|d| canonical_date(d, "credit unapplication date")).transpose()?.map(|d| (None, d));
        let (memo_identity, memo_control) = memo(&application["credit_memo_id"], "credit application")?;
        let amount = settlement_amount(&line["amount"], "credit application amount")?;
        for (doc, control) in [(invoice(&line["invoice_id"], "credit application line")?, text(&line["receivables_id"], "credit line receivables")?), (memo_identity, memo_control)] {
            let event = Event { doc, date, amount, order: (date, 4, seq), source: Some(memo_identity), journal: None, control, note: "credit applied" };
            settle(&mut events, event, undone, "credit unapplied")?;
        }
    }
    let voids: HashMap<&str, &Value> = list(&c["creditrefundvoid"], "refund voids")?.iter().filter_map(|v| Some((v["refund_id"].as_str()?, &v["journal_id"]))).collect();
    for refund in list(&c["creditrefund"], "refunds")? {
        seq += 1;
        let amount = settlement_amount(&refund["amount"], "refund amount")?;
        let Some(j) = index.posted(&refund["journal_id"], "refund journal")? else { continue };
        let voided = match refund["id"].as_str().and_then(|r| voids.get(r)) {
            Some(v) => undo_journal(v, "refund void journal")?,
            None => None,
        };
        let event = Event {
            doc: memo(&refund["credit_memo_id"], "refund")?.0,
            date: j.date,
            amount,
            order: (j.date, 6, seq),
            source: refund["identity_id"].as_str(),
            journal: refund["journal_id"].as_str(),
            control: text(&refund["receivables_id"], "refund receivables")?,
            note: "refund",
        };
        settle(&mut events, event, voided, "refund void")?;
    }
    // P2-2: a customer deposit application settles its invoice on the application journal date.
    for application in list(&dump["prepayments"]["applications"], "deposit applications")? {
        seq += 1;
        let amount = settlement_amount(&application["amount"], "deposit application amount")?;
        let Some(j) = index.posted(&application["journal_id"], "deposit application journal")? else { continue };
        let unapplied = undo_journal(&application["reversal"]["journal_id"], "deposit unapplication journal")?;
        let event = Event {
            doc: invoice(&application["invoice_id"], "deposit application")?,
            date: j.date,
            amount,
            order: (j.date, 8, seq),
            source: j.identity,
            journal: application["journal_id"].as_str(),
            control: text(&application["receivables_id"], "deposit receivables")?,
            note: "deposit applied",
        };
        settle(&mut events, event, unapplied, "deposit unapplied")?;
    }
    // Python's list.sort is stable: the invoice and memo events of one credit line keep their order.
    events.sort_by_key(|e| e.order);
    Ok(events)
}

/// A settled document: per-account nets keyed by (creation rank, account) and the control amount T.
type Settled = (Vec<((usize, String), Money)>, Money);

/// `_diff`: the keys whose amount changed, with the change (checked: shares come from dump amounts).
fn share_diff<K: Ord + Clone>(after: &BTreeMap<K, Money>, before: &BTreeMap<K, Money>) -> Result<Vec<(K, i64)>, String> {
    let keys: BTreeSet<&K> = after.keys().chain(before.keys()).collect();
    let mut changed = Vec::new();
    for k in keys {
        let (a, b) = (after.get(k).copied().unwrap_or(Money::ZERO), before.get(k).copied().unwrap_or(Money::ZERO));
        if a != b {
            let change = a.cents().checked_sub(b.cents()).and_then(|c| i64::try_from(c).ok()).ok_or("recognised share overflows")?;
            changed.push((k.clone(), change));
        }
    }
    Ok(changed)
}

/// The company's chart and its entries on one basis, from the dump alone. Posted lines are every POSTED journal
/// line (cash: without the lines `cash_excluded` leaves out). Cash recognised shares are rebuilt from the
/// settlement records and the settled documents' original journals with the cent rule, never read from a
/// report. Journal references are the report's `j<rank>`.
fn evidence(dump: &Value, company: &str, cash: bool, order: &HashMap<String, usize>) -> Result<Evidence, String> {
    let mut chart = Vec::new();
    for a in arr(&dump["accounts"], "accounts")?.iter().filter(|a| a["company"] == company) {
        let detail = DetailType::parse(text(&a["detail_type"], "account type")?).ok_or("account type")?;
        let account = ChartAccount {
            code: text(&a["code"], "account code")?.to_string(),
            name: text(&a["name"], "account name")?.to_string(),
            kind: text(&a["kind"], "account kind")?.to_string(),
            detail,
        };
        chart.push((text(&a["ref"], "account ref")?.to_string(), account));
    }
    chart.sort_by(|a, b| a.1.code.cmp(&b.1.code));
    let index = Index::new(dump)?;
    let in_company = |doc: &str| index.identities.get(doc).is_some_and(|i| i.company == company);
    let mut events = Vec::new();
    if cash {
        for e in settlement_events(dump, &index)? {
            if !index.identities.contains_key(e.doc) {
                return Err(format!("settlement of unknown document {}", e.doc));
            }
            if in_company(e.doc) {
                events.push(e);
            }
        }
    }
    // Lines on a settlement identity's settled control accounts (receipt, bill payment, refund, deposit
    // application); every line of a deferred document's journal.
    let mut settles: HashMap<&str, BTreeSet<&str>> = HashMap::new();
    for e in events.iter().filter(|e| e.journal.is_some()) {
        if let Some(source) = e.source {
            settles.entry(source).or_default().insert(e.control);
        }
    }
    let journals: Vec<&Value> = list(&dump["journals"], "journals")?.iter().filter(|j| j["company"] == company).collect();
    let excluded = |identity: Option<&str>, account: &str| -> bool {
        let Some(identity) = identity else { return false };
        match index.identities.get(identity).map(|i| i.kind) {
            Some("INV" | "BILL" | "CM") => true,
            Some("RCP" | "BPAY" | "RFD" | "CDA") => settles.get(identity).is_some_and(|c| c.contains(account)),
            _ => false,
        }
    };
    // `_unconverted`: control accounts with a receivables / payables role (setup or document roles).
    let mut controls: BTreeSet<&str> = BTreeSet::new();
    let setup = &dump["l2a"]["setup"][company];
    for role in ["receivables", "payables"] {
        if let Some(account) = setup[role].as_str() {
            controls.insert(account);
        }
    }
    for snapshot in list(&dump["l2a"]["snapshots"], "document accounts")? {
        let role = text(&snapshot["role"], "document account role")?;
        let base = role.split(':').next().unwrap_or_default();
        if matches!(base, "receivables" | "payables") && snapshot["source"].as_str().is_some_and(in_company) {
            controls.insert(text(&snapshot["account_id"], "document account")?);
        }
    }
    let mut entries: HashMap<String, Vec<(RowKey, Entry)>> = HashMap::new();
    let mut unconverted = Vec::new();
    for j in &journals {
        let r = text(&j["ref"], "journal ref")?;
        let journal = &index.journals[r];
        if !journal.posted {
            continue;
        }
        let source = journal.identity.and_then(|i| index.number(i));
        let kind = journal.identity.map(|i| index.identities[i].kind);
        for line in arr(&j["lines"], "journal lines")? {
            let account = text(&line["account"], "line account")?;
            if cash && excluded(journal.identity, account) {
                continue;
            }
            if cash && controls.contains(account) && !kind.is_some_and(|k| matches!(k, "INV" | "BILL" | "CM" | "RCP" | "BPAY" | "RFD" | "CDA")) {
                unconverted.push(journal.date);
            }
            let description = text(&line["description"], "line description")?;
            let entry = Entry {
                date: journal.date,
                debit: int(&line["debit"], "debit")?,
                credit: int(&line["credit"], "credit")?,
                journal: Some(format!("j{}", journal.rank)),
                source: source.clone(),
                reversal: !j["reversal_of"].is_null(),
                entry_number: text(&j["entry_number"], "entry number")?.to_string(),
                memo: if description.is_empty() { text(&j["memo"], "journal memo")?.to_string() } else { description.to_string() },
                name: text(&line["name"], "line name")?.to_string(),
                synthetic: None,
            };
            let key = (journal.date, journal.rank, int(&line["position"], "line position")?, 0);
            entries.entry(account.to_string()).or_default().push((key, entry));
        }
    }
    // `_documents`: the non-control lines of each settled document's ORIGINAL posted journals.
    let mut controls_of: HashMap<&str, &str> = HashMap::new();
    for e in &events {
        controls_of.entry(e.doc).or_insert(e.control);
    }
    let rank = |account: &str| -> (usize, String) { (order.get(account).copied().unwrap_or(usize::MAX), account.to_string()) };
    let mut nets: HashMap<&str, BTreeMap<(usize, String), i64>> = HashMap::new();
    for j in &journals {
        let journal = &index.journals[text(&j["ref"], "journal ref")?];
        let Some(doc) = journal.identity else { continue };
        if !journal.posted || !j["reversal_of"].is_null() || !matches!(index.identities[doc].kind, "INV" | "BILL" | "CM") {
            continue;
        }
        let Some(control) = controls_of.get(doc) else { continue };
        for line in arr(&j["lines"], "journal lines")? {
            let account = text(&line["account"], "line account")?;
            if account != *control {
                let net = int(&line["debit"], "debit")?.checked_sub(int(&line["credit"], "credit")?).ok_or("line overflows")?;
                let slot = nets.entry(doc).or_default().entry(rank(account)).or_default();
                *slot = slot.checked_add(net).ok_or("document overflows")?;
            }
        }
    }
    let mut documents: HashMap<&str, Settled> = HashMap::new();
    for (doc, by_account) in nets {
        let lines: Vec<((usize, String), Money)> = by_account.into_iter().filter(|(_, n)| *n != 0).map(|(k, n)| (k, Money::from_db(n))).collect();
        let sum = lines.iter().try_fold(0i128, |s, (_, n)| s.checked_add(n.cents())).and_then(i128::checked_abs).ok_or("document overflows")?;
        documents.insert(doc, (lines, Money::from_cents(sum)));
    }
    // `recognition_rows`: per event, the change of the document's cumulative share.
    let mut settled: HashMap<&str, i64> = HashMap::new();
    let failed = |e: FbError| format!("recognised share: {e}");
    for e in &events {
        let slot = settled.entry(e.doc).or_default();
        let before = *slot;
        *slot = slot.checked_add(e.amount).ok_or("settled amount overflows")?;
        let Some((lines, total)) = documents.get(e.doc) else { continue };
        let after = financial::share(lines, *total, Money::from_db(*slot)).map_err(failed)?;
        let prior = financial::share(lines, *total, Money::from_db(before)).map_err(failed)?;
        for ((_, account), amount) in share_diff(&after, &prior)? {
            let entry = Entry {
                date: e.date,
                debit: amount.max(0),
                credit: amount.checked_neg().ok_or("recognised share overflows")?.max(0),
                journal: e.journal.map(|r| format!("j{}", index.journals[r].rank)),
                source: e.source.and_then(|s| index.number(s)),
                reversal: e.amount < 0,
                entry_number: String::new(),
                memo: format!("Cash basis: share of {} ({})", index.identities[e.doc].number, e.note),
                name: String::new(),
                synthetic: Some(e.note.to_string()),
            };
            let key = (e.date, e.journal.map_or(usize::MAX, |r| index.journals[r].rank), 1_000_000 + i64::from(e.order.1), e.order.2);
            entries.entry(account).or_default().push((key, entry));
        }
    }
    Ok(Evidence { chart, entries, unconverted })
}

// Per-report validation --------------------------------------------------------------------------------------

/// `[account, amount]` rows: references into the chart, no duplicates.
fn amount_rows(v: &Value, ev: &Evidence, at: &str) -> Result<Vec<(String, i64)>, String> {
    let mut seen = BTreeSet::new();
    let mut rows = Vec::new();
    for (i, row) in arr(v, at)?.iter().enumerate() {
        let pair = arr(row, &format!("{at}[{i}]"))?;
        if pair.len() != 2 {
            return Err(format!("{at}[{i}]: expected [account, amount]"));
        }
        let account = text(&pair[0], &format!("{at}[{i}] account"))?;
        if ev.account(account).is_none() {
            return Err(format!("{at}: {account} is not an account of this company"));
        }
        if !seen.insert(account) {
            return Err(format!("{at}: duplicate account {account}"));
        }
        rows.push((account.to_string(), int(&pair[1], &format!("{at}[{i}] amount"))?));
    }
    Ok(rows)
}

fn total_of(rows: &[(String, i64)], at: &str) -> Result<i64, String> {
    checked_sum(rows.iter().map(|r| r.1), at)
}

/// The CSV export model (`_csv_rows` of `pl_rows` / `bs_rows`): `[row type, section, group, code, name,
/// account, amount]`, headings and spacers left out, a grand row written as "total".
struct Csv<'e> {
    ev: &'e Evidence,
    rows: Vec<Value>,
}

impl Csv<'_> {
    fn accounts(&mut self, section: &str, group: &str, rows: &[(String, i64)]) {
        for (r, amount) in rows {
            let a = self.ev.account(r).expect("validated account");
            self.rows.push(json!(["account", section, group, a.code, a.name, r, amount]));
        }
    }

    fn total(&mut self, kind: &str, section: &str, group: &str, label: &str, amount: i64) {
        self.rows.push(json!([kind, section, group, "", label, "", amount]));
    }

    /// The whole export, row for row: every account, subtotal, computed and grand row in layout order.
    fn check(self, v: &Value, at: &str) -> Check {
        let got = arr(v, at)?;
        for (i, row) in got.iter().enumerate() {
            let cells = arr(row, &format!("{at}[{i}]"))?;
            if cells.len() != 7 || !cells[..6].iter().all(Value::is_string) || !cells[6].is_i64() {
                return Err(format!("{at}[{i}]: malformed CSV row"));
            }
        }
        if got.len() != self.rows.len() {
            return Err(format!("{at}: {} rows, the report's sections give {}", got.len(), self.rows.len()));
        }
        if let Some(i) = got.iter().zip(&self.rows).position(|(g, w)| g != w) {
            return Err(format!("{at}[{i}]: {} is not the report's row {}", got[i], self.rows[i]));
        }
        Ok(())
    }
}

/// A section's `[account, amount]` rows and total.
type Section = (Vec<(String, i64)>, i64);

const PL_KEYS: [&str; 5] = ["income", "cogs", "expense", "other_income", "other_expense"];

fn validate_pl(v: &Value, ev: &Evidence, start: Option<NaiveDate>, end: NaiveDate, cash: bool, at: &str) -> Check {
    keys_exact(v, &["sections", "gross_profit", "net_ordinary_income", "net_other_income", "net_income", "unconverted", "show_cogs", "show_other", "csv"], at)?;
    let sections = &v["sections"];
    keys_exact(sections, &PL_KEYS, &format!("{at} sections"))?;
    let mut totals = HashMap::new();
    let mut parts = HashMap::new();
    for key in PL_KEYS {
        let s = &sections[key];
        let sat = format!("{at} {key}");
        keys_exact(s, &["rows", "total"], &sat)?;
        let rows = amount_rows(&s["rows"], ev, &sat)?;
        // The complete inventory: every active account of this section, recomputed from the entries.
        let mut want = Vec::new();
        for (r, _) in ev.chart.iter().filter(|(_, a)| financial::pl_section(a.detail) == Some(key)) {
            let (net, active) = ev.net(r, start, end)?;
            if active {
                let sign = if matches!(key, "income" | "other_income") { -1 } else { 1 };
                want.push((r.clone(), net.checked_mul(sign).ok_or("overflow")?));
            }
        }
        if rows != want {
            return Err(format!("{sat}: rows disagree with the ledger entries"));
        }
        let sum = total_of(&rows, &sat)?;
        if sum != int(&s["total"], &sat)? {
            return Err(format!("{sat}: rows do not add to the total"));
        }
        totals.insert(key, sum);
        parts.insert(key, rows);
    }
    let sub = |a: i64, b: i64| a.checked_sub(b).ok_or(format!("{at}: totals overflow"));
    let gross = sub(totals["income"], totals["cogs"])?;
    let ordinary = sub(gross, totals["expense"])?;
    let other = sub(totals["other_income"], totals["other_expense"])?;
    let net = ordinary.checked_add(other).ok_or(format!("{at}: totals overflow"))?;
    for (key, want) in [("gross_profit", gross), ("net_ordinary_income", ordinary), ("net_other_income", other), ("net_income", net)] {
        if int(&v[key], at)? != want {
            return Err(format!("{at}: {key} disagrees with the sections"));
        }
    }
    if count(&v["unconverted"], &format!("{at} unconverted"))? != if cash { ev.unconverted(end) } else { 0 } {
        return Err(format!("{at}: unconverted disagrees with the posted control-account lines"));
    }
    let (show_cogs, show_other) = (!parts["cogs"].is_empty(), !parts["other_income"].is_empty() || !parts["other_expense"].is_empty());
    if v["show_cogs"].as_bool() != Some(show_cogs) || v["show_other"].as_bool() != Some(show_other) {
        return Err(format!("{at}: show flags disagree with the sections"));
    }
    let mut csv = Csv { ev, rows: Vec::new() };
    let section = |csv: &mut Csv, key: &str| {
        let &(_, heading, total_label, _) = financial::PL_SECTIONS.iter().find(|s| s.0 == key).expect("P&L section");
        csv.accounts(heading, heading, &parts[key]);
        csv.total("total", heading, heading, total_label, totals[key]);
    };
    section(&mut csv, "income");
    if show_cogs {
        section(&mut csv, "cogs");
    }
    csv.total("total", "", "", "Gross profit", gross);
    section(&mut csv, "expense");
    csv.total("total", "", "", "Net ordinary income", ordinary);
    if show_other {
        for key in ["other_income", "other_expense"] {
            if !parts[key].is_empty() {
                section(&mut csv, key);
            }
        }
        csv.total("total", "", "", "Net other income", other);
    }
    csv.total("total", "", "", "Net income", net);
    csv.check(&v["csv"], &format!("{at} csv"))
}

/// The balance-sheet groups `BS_LAYOUT` builds from the section rows.
fn bs_groups(kind: &str, rows: &[(String, i64)], ev: &Evidence) -> Result<Value, String> {
    let mut groups = Vec::new();
    for (_, _, _, layout_kind, layout) in financial::BS_LAYOUT {
        if layout_kind.as_str() != kind {
            continue;
        }
        for &(key, _, _, _, types) in layout {
            let mut parts = Vec::new();
            let mut group_total = 0i64;
            for &(detail, _, _, _) in types {
                let part: Vec<(String, i64)> = rows.iter().filter(|(r, _)| ev.account(r).is_some_and(|a| a.detail == detail)).cloned().collect();
                if part.is_empty() {
                    continue;
                }
                let total = total_of(&part, "group")?;
                group_total = group_total.checked_add(total).ok_or("overflow")?;
                parts.push(json!({"type": detail.as_str(), "rows": part.iter().map(|(r, a)| json!([r, a])).collect::<Vec<_>>(), "total": total}));
            }
            if !parts.is_empty() {
                groups.push(json!({"key": key, "nested": types.len() > 1, "total": group_total, "types": parts}));
            }
        }
    }
    Ok(json!(groups))
}

/// The balance-sheet CSV: per section its groups (a heading-less group lists its accounts directly; a nested
/// group one sub-total per type), the equity section's computed rows, the section total, then the grand total.
fn bs_csv(csv: &mut Csv, sections: &HashMap<&str, Section>, retained: i64, year: i64, equity: i64, le: i64) -> Check {
    for (key, heading, total_label, _, layout) in financial::BS_LAYOUT {
        let (rows, total) = &sections[key];
        for &(_, part_heading, part_total, _, types) in layout {
            let parts: Vec<(&financial::BsType, Vec<(String, i64)>)> = types
                .iter()
                .map(|t| (t, rows.iter().filter(|(r, _)| csv.ev.account(r).is_some_and(|a| a.detail == t.0)).cloned().collect::<Vec<_>>()))
                .filter(|(_, p)| !p.is_empty())
                .collect();
            if parts.is_empty() {
                continue;
            }
            let Some(part_heading) = part_heading else {
                for (_, p) in &parts {
                    csv.accounts(heading, heading, p);
                }
                continue;
            };
            let mut group_total = 0i64;
            for ((_, type_heading, type_total, _), p) in &parts {
                let sum = total_of(p, "group")?;
                group_total = group_total.checked_add(sum).ok_or("group total overflows")?;
                if types.len() > 1 {
                    let type_heading = type_heading.unwrap_or("");
                    csv.accounts(heading, type_heading, p);
                    csv.total("total", heading, type_heading, type_total.unwrap_or(""), sum);
                } else {
                    csv.accounts(heading, part_heading, p);
                }
            }
            csv.total("total", heading, part_heading, part_total.unwrap_or(""), group_total);
        }
        if key == "equity" {
            csv.total("computed", heading, heading, "Retained earnings", retained);
            csv.total("computed", heading, heading, "Net income", year);
        }
        csv.total("total", heading, "", total_label, if key == "equity" { equity } else { *total });
    }
    csv.total("total", "", "", "Total liabilities and equity", le);
    Ok(())
}

fn validate_bs(v: &Value, ev: &Evidence, as_of: NaiveDate, cash: bool, at: &str) -> Check {
    keys_exact(
        v,
        &["assets", "liabilities", "equity", "fiscal_year_start", "retained_earnings", "net_income", "total_equity", "total_liabilities_equity", "balanced", "unconverted", "csv"],
        at,
    )?;
    let mut sections = HashMap::new();
    for (key, kind) in [("assets", "ASSET"), ("liabilities", "LIABILITY"), ("equity", "EQUITY")] {
        let s = &v[key];
        let sat = format!("{at} {key}");
        keys_exact(s, &["rows", "total", "groups"], &sat)?;
        let rows = amount_rows(&s["rows"], ev, &sat)?;
        let mut want = Vec::new();
        for (r, _) in ev.chart.iter().filter(|(_, a)| a.kind == kind) {
            let (net, _) = ev.net(r, None, as_of)?;
            let amount = if kind == "ASSET" { net } else { net.checked_neg().ok_or("overflow")? };
            if amount != 0 {
                want.push((r.clone(), amount));
            }
        }
        if rows != want {
            return Err(format!("{sat}: rows disagree with the ledger entries"));
        }
        let sum = total_of(&rows, &sat)?;
        if sum != int(&s["total"], &sat)? {
            return Err(format!("{sat}: rows do not add to the total"));
        }
        if s["groups"] != bs_groups(kind, &rows, ev)? {
            return Err(format!("{sat}: groups are not the layout of the rows"));
        }
        sections.insert(key, (rows, sum));
    }
    let fy = financial::fiscal_year_start(as_of);
    if canonical_date(&v["fiscal_year_start"], at)? != fy {
        return Err(format!("{at}: fiscal year start disagrees"));
    }
    // Retained earnings: every P&L account before the fiscal year; net income: the fiscal year to date.
    let (mut retained, mut year) = (0i64, 0i64);
    for (r, _) in ev.chart.iter().filter(|(_, a)| financial::pl_section(a.detail).is_some()) {
        if let Some(prior) = fy.pred_opt() {
            retained = retained.checked_sub(ev.net(r, None, prior)?.0).ok_or("overflow")?;
        }
        year = year.checked_sub(ev.net(r, Some(fy), as_of)?.0).ok_or("overflow")?;
    }
    if int(&v["retained_earnings"], at)? != retained || int(&v["net_income"], at)? != year {
        return Err(format!("{at}: retained earnings / net income disagree with the ledger entries"));
    }
    let equity = checked_sum([sections["equity"].1, retained, year], at)?;
    if equity != int(&v["total_equity"], at)? {
        return Err(format!("{at}: total equity disagrees"));
    }
    let le = checked_sum([sections["liabilities"].1, equity], at)?;
    if le != int(&v["total_liabilities_equity"], at)? {
        return Err(format!("{at}: total liabilities and equity disagrees"));
    }
    if v["balanced"].as_bool() != Some(le == sections["assets"].1) || le != sections["assets"].1 {
        return Err(format!("{at}: not balanced"));
    }
    if count(&v["unconverted"], &format!("{at} unconverted"))? != if cash { ev.unconverted(as_of) } else { 0 } {
        return Err(format!("{at}: unconverted disagrees with the posted control-account lines"));
    }
    let mut csv = Csv { ev, rows: Vec::new() };
    bs_csv(&mut csv, &sections, retained, year, equity, le)?;
    csv.check(&v["csv"], &format!("{at} csv"))
}

fn ledger_row(row: &Value, at: &str) -> Result<(Entry, i64), String> {
    let cells = arr(row, at)?;
    if cells.len() != 11 {
        return Err(format!("{at}: expected 11 cells"));
    }
    let date = canonical_date(&cells[0], at)?;
    let (debit, credit) = (int(&cells[1], at)?, int(&cells[2], at)?);
    if debit < 0 || credit < 0 || (debit != 0 && credit != 0) {
        return Err(format!("{at}: invalid debit/credit"));
    }
    if !(cells[3].is_null() || cells[3].is_string()) || !(cells[4].is_null() || cells[4].is_string()) || !cells[5].is_boolean() {
        return Err(format!("{at}: malformed journal/source/reversal"));
    }
    if !cells[6].is_string() || !cells[7].is_string() || !cells[8].is_string() || !(cells[9].is_null() || cells[9].is_string()) {
        return Err(format!("{at}: malformed text cells"));
    }
    let owned = |v: &Value| v.as_str().map(str::to_string);
    let entry = Entry {
        date,
        debit,
        credit,
        journal: owned(&cells[3]),
        source: owned(&cells[4]),
        reversal: cells[5] == true,
        entry_number: owned(&cells[6]).unwrap_or_default(),
        memo: owned(&cells[7]).unwrap_or_default(),
        name: owned(&cells[8]).unwrap_or_default(),
        synthetic: owned(&cells[9]),
    };
    Ok((entry, int(&cells[10], at)?))
}

fn validate_gl(v: &Value, ev: &Evidence, request: &LedgerRequest, cash: bool, at: &str) -> Check {
    if v.is_null() {
        // Python: a 404 before the report runs, only for an account that is not this company's.
        return match &request.account {
            Some(r) if ev.account(r).is_none() => Ok(()),
            _ => Err(format!("{at}: missing ledger")),
        };
    }
    if request.account.as_ref().is_some_and(|r| ev.account(r).is_none()) {
        return Err(format!("{at}: a ledger for another company's account"));
    }
    keys_exact(v, &["sections", "debit", "credit", "unconverted"], at)?;
    let types = financial::group_types(&request.group).ok_or(format!("{at}: unknown group"))?;
    let mut want_accounts = Vec::new();
    for (r, a) in &ev.chart {
        if request.account.as_ref().is_some_and(|x| x != r) || !(request.group.is_empty() || types.contains(&a.detail)) {
            continue;
        }
        let opening = match request.start.and_then(|s| s.pred_opt()) {
            Some(before) => ev.net(r, None, before)?.0.checked_mul(a.sign()).ok_or("overflow")?,
            None => 0,
        };
        let mut rows: Vec<(RowKey, Entry)> =
            ev.entries.get(r).into_iter().flatten().filter(|(_, e)| request.start.is_none_or(|s| e.date >= s) && e.date <= request.end).cloned().collect();
        if rows.is_empty() && opening == 0 {
            continue;
        }
        // A stable sort: rows with one key keep their generation order, as in the report.
        rows.sort_by_key(|r| r.0);
        let rows: Vec<Entry> = rows.into_iter().map(|(_, e)| e).collect();
        want_accounts.push((r.as_str(), a.sign(), opening, rows));
    }
    let sections = arr(&v["sections"], at)?;
    if sections.len() != want_accounts.len() {
        return Err(format!("{at}: {} account sections, the ledger entries give {}", sections.len(), want_accounts.len()));
    }
    let (mut debits, mut credits) = (Vec::new(), Vec::new());
    for (si, (s, (account, sign, opening, want))) in sections.iter().zip(&want_accounts).enumerate() {
        let sat = format!("{at} section {si}");
        keys_exact(s, &["account", "opening", "debit", "credit", "closing", "rows"], &sat)?;
        if text(&s["account"], &sat)? != *account {
            return Err(format!("{sat}: account sections disagree with the ledger entries"));
        }
        let mut balance = int(&s["opening"], &sat)?;
        if balance != *opening {
            return Err(format!("{sat}: opening balance disagrees with the ledger entries"));
        }
        let (mut debit, mut credit, mut got) = (Vec::new(), Vec::new(), Vec::new());
        for (ri, row) in arr(&s["rows"], &sat)?.iter().enumerate() {
            let rat = format!("{sat} row {ri}");
            let (entry, next) = ledger_row(row, &rat)?;
            let step = entry.net()?.checked_mul(*sign).ok_or(format!("{rat}: overflow"))?;
            if balance.checked_add(step) != Some(next) {
                return Err(format!("{rat}: running balance does not follow the line"));
            }
            balance = next;
            debit.push(entry.debit);
            credit.push(entry.credit);
            got.push(entry);
        }
        if got != *want {
            let missing = want.iter().find(|w| !got.contains(w));
            let extra = got.iter().find(|g| !want.contains(g));
            if missing.is_none() && extra.is_none() && got.len() == want.len() {
                return Err(format!("{sat}: rows are not in the ledger's (date, journal, position) order"));
            }
            return Err(format!("{sat}: rows disagree with the ledger and settlement evidence (expected {missing:?}, found {extra:?})"));
        }
        if balance != int(&s["closing"], &sat)? {
            return Err(format!("{sat}: closing is not the last running balance"));
        }
        let (sd, sc) = (checked_sum(debit, &sat)?, checked_sum(credit, &sat)?);
        if sd != int(&s["debit"], &sat)? || sc != int(&s["credit"], &sat)? {
            return Err(format!("{sat}: debit/credit totals disagree"));
        }
        debits.push(sd);
        credits.push(sc);
    }
    if checked_sum(debits, at)? != int(&v["debit"], at)? || checked_sum(credits, at)? != int(&v["credit"], at)? {
        return Err(format!("{at}: report totals disagree"));
    }
    if count(&v["unconverted"], &format!("{at} unconverted"))? != if cash { ev.unconverted(request.end) } else { 0 } {
        return Err(format!("{at}: unconverted disagrees with the posted control-account lines"));
    }
    Ok(())
}
fn validate_form(v: &Value, kind: &str, at: &str) -> Check {
    match v.get("ok").and_then(Value::as_bool) {
        Some(false) => {
            keys_exact(v, &["ok", "errors"], at)?;
            let errors = arr(&v["errors"], at)?;
            if errors.is_empty() || errors.iter().any(|e| !e.as_str().is_some_and(|e| !e.is_empty())) {
                return Err(format!("{at}: malformed errors"));
            }
        }
        Some(true) => {
            let basis = text(&v["basis"], at)?;
            if financial::Basis::parse(basis).is_none_or(|b| b.as_str() != basis) {
                return Err(format!("{at}: unknown basis"));
            }
            if kind == "as_of" {
                keys_exact(v, &["ok", "basis", "as_of"], at)?;
                date_text(&v["as_of"], &format!("{at} as_of"))?;
                return Ok(());
            }
            if kind == "range" {
                keys_exact(v, &["ok", "basis", "preset", "start", "end"], at)?;
            } else {
                keys_exact(v, &["ok", "basis", "preset", "start", "end", "group"], at)?;
                if !financial::GL_GROUPS.iter().any(|(g, _)| v["group"] == *g) {
                    return Err(format!("{at}: unknown group"));
                }
            }
            if !params::PRESETS.iter().any(|(p, _)| v["preset"] == *p) {
                return Err(format!("{at}: unknown preset"));
            }
            let (start, end) = (optional_date(&v["start"], &format!("{at} start"))?, date_text(&v["end"], &format!("{at} end"))?);
            if start.is_some_and(|s| s > end) {
                return Err(format!("{at}: start after end"));
            }
        }
        None => return Err(format!("{at}: missing ok")),
    }
    Ok(())
}

/// Strict structure plus the complete report inventory of one side's `financial_reports`, recomputed from the
/// scenario and the same dump: the scenario fixes which companies and reports exist (the echoed `requests` must
/// be exactly its spec); posted journals (cash: minus the lines the settlement evidence excludes, plus the
/// shares rebuilt from the settlement records) fix every row, total, CSV row and unconverted count. The
/// accounts' creation order comes from this side's own successful creation outcomes.
pub(super) fn validate(v: &Value, dump: &Value, outcomes: &[Value], expected: &Expected, spec: &Value) -> Check {
    keys_exact(v, &["companies", "forms", "requests"], "financial_reports")?;
    let requests = echo(spec)?;
    if v["requests"] != requests {
        return Err("requests: not the scenario's financial_reports spec".into());
    }
    let m = manifest(&requests)?;
    let forms = arr(&v["forms"], "forms")?;
    if forms.len() != m.forms.len() {
        return Err(format!("forms: {} results for {} requests", forms.len(), m.forms.len()));
    }
    for (i, (f, kind)) in forms.iter().zip(&m.forms).enumerate() {
        validate_form(f, kind, &format!("form {i}"))?;
    }
    let companies = v["companies"].as_object().ok_or("companies: not an object")?;
    let mut books = BTreeSet::new();
    for c in arr(&dump["companies"], "dump companies")? {
        books.insert(text(&c["ref"], "company ref")?.to_string());
    }
    let reported: BTreeSet<String> = companies.keys().cloned().collect();
    if reported != books || books != expected.companies || books.is_empty() {
        return Err(format!("companies: reports for {reported:?}, the books have {books:?}, the scenario created {:?}", expected.companies));
    }
    let order = expected.creation_order(outcomes);
    let keys = |items: Vec<&String>| -> BTreeSet<String> { items.into_iter().cloned().collect() };
    let (pl_keys, bs_keys, gl_keys) = (keys(m.pl.iter().map(|p| &p.0).collect()), keys(m.bs.iter().map(|b| &b.0).collect()), keys(m.gl.iter().map(|g| &g.key).collect()));
    for (company, bases) in companies {
        keys_exact(bases, &["ACCRUAL", "CASH"], company)?;
        for basis in ["ACCRUAL", "CASH"] {
            let b = &bases[basis];
            let at = format!("{company} {basis}");
            keys_exact(b, &["profit_loss", "balance_sheet", "general_ledger"], &at)?;
            for (family, want) in [("profit_loss", &pl_keys), ("balance_sheet", &bs_keys), ("general_ledger", &gl_keys)] {
                let got: BTreeSet<String> = b[family].as_object().ok_or(format!("{at} {family}: not an object"))?.keys().cloned().collect();
                if got != *want {
                    return Err(format!("{at} {family}: reports {got:?}, the manifest requests {want:?}"));
                }
            }
            let cash = basis == "CASH";
            let ev = evidence(dump, company, cash, &order).map_err(|e| format!("{at} evidence: {e}"))?;
            for request in &m.gl {
                validate_gl(&b["general_ledger"][&request.key], &ev, request, cash, &format!("{at} GL {}", request.key))?;
            }
            for (key, start, end) in &m.pl {
                validate_pl(&b["profit_loss"][key], &ev, *start, *end, cash, &format!("{at} P&L {key}"))?;
            }
            for (key, as_of) in &m.bs {
                validate_bs(&b["balance_sheet"][key], &ev, *as_of, cash, &format!("{at} BS {key}"))?;
            }
        }
    }
    Ok(())
}

/// Validate both sides independently against the scenario (identical incomplete or malformed reports still
/// fail), then compare exactly. Without the scenario (`expected` None) no report can be checked, so any is a
/// failure. `outcomes` are each side's step outcomes.
pub(super) fn compare(expected: Option<&Expected>, py: Option<&Value>, rs: Option<&Value>, dumps: (&Value, &Value), outcomes: (&[Value], &[Value]), label: &str, report: &mut Report) {
    let Some(expected) = expected else {
        if py.is_some() || rs.is_some() {
            report.failures.push(format!("{label}: financial reports need the scenario's request manifest (pass the scenario file)"));
        }
        return;
    };
    let Some(spec) = expected.spec else {
        if py.is_some() || rs.is_some() {
            report.failures.push(format!("{label}: financial reports the scenario does not request"));
        }
        return;
    };
    let (Some(py), Some(rs)) = (py, rs) else {
        for (side, name) in [(py, "python"), (rs, "rust")] {
            if side.is_none() {
                report.failures.push(format!("{label} {name}: the scenario's financial reports are missing"));
            }
        }
        return;
    };
    let mut ok = true;
    for (side, dump, outcomes, name) in [(py, dumps.0, outcomes.0, "python"), (rs, dumps.1, outcomes.1, "rust")] {
        if let Err(e) = validate(side, dump, outcomes, expected, spec) {
            report.failures.push(format!("{label} {name} financial reports: {e}"));
            ok = false;
        }
    }
    if !ok {
        return;
    }
    let count = |v: &Value| -> usize {
        v["companies"].as_object().map(|c| c.values().map(|b| ["ACCRUAL", "CASH"].iter().map(|k| ["profit_loss", "balance_sheet", "general_ledger"].iter().map(|r| b[k][r].as_object().map(Map::len).unwrap_or(0)).sum::<usize>()).sum::<usize>()).sum()).unwrap_or(0)
    };
    *report.checked.entry("financial reports").or_default() += count(py);
    *report.checked.entry("report forms").or_default() += py["forms"].as_array().map(Vec::len).unwrap_or(0);
    if py != rs {
        let pc = &py["companies"];
        let rc = &rs["companies"];
        for (company, bases) in pc.as_object().into_iter().flatten() {
            for basis in ["ACCRUAL", "CASH"] {
                for kind in ["profit_loss", "balance_sheet", "general_ledger"] {
                    for (k, p) in bases[basis][kind].as_object().into_iter().flatten() {
                        let r = &rc[company][basis][kind][k];
                        if p != r {
                            report.failures.push(format!("{label} {company} {basis} {kind} {k}: {}", diff(p, r)));
                        }
                    }
                }
            }
        }
        for (i, (p, r)) in py["forms"].as_array().into_iter().flatten().zip(rs["forms"].as_array().into_iter().flatten()).enumerate() {
            if p != r {
                report.failures.push(format!("{label} form {i}: python {p} / rust {r}"));
            }
        }
        if report.failures.is_empty() {
            report.failures.push(format!("{label}: financial reports differ"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn journal(r: &str, date: &str, status: &str, lines: &[(&str, i64)]) -> Value {
        let lines: Vec<Value> = lines
            .iter()
            .enumerate()
            .map(|(i, (a, n))| json!({"position": i + 1, "account": a, "debit": n.max(&0), "credit": (-n).max(0), "description": "", "name": ""}))
            .collect();
        json!({"ref": r, "company": "c", "date": date, "memo": r, "status": status, "entry_number": "", "reversal_of": null, "lines": lines})
    }

    /// `_unconverted` recounted from the dump: posted lines on receivables / payables control accounts (setup
    /// roles and document roles) from journals that are neither deferred documents nor settlements.
    #[test]
    fn unconverted_is_recounted_from_journals_and_control_roles() {
        let account = |r: &str, kind: &str, detail: &str| json!({"ref": r, "company": "c", "code": r, "name": r, "kind": kind, "detail_type": detail});
        let dump = json!({
            "accounts": [account("bank", "ASSET", "BANK"), account("ar", "ASSET", "RECEIVABLE"), account("ar2", "ASSET", "RECEIVABLE"),
                         account("ap", "LIABILITY", "PAYABLE"), account("uf", "ASSET", "OTHER_CURRENT_ASSET")],
            "journals": [
                journal("manual", "2026-10-01", "POSTED", &[("ar", 100), ("bank", -100)]),
                journal("draft", "2026-10-02", "DRAFT", &[("ar", 100), ("bank", -100)]),
                journal("receipt", "2026-10-03", "POSTED", &[("bank", 100), ("ar", -100)]),
                journal("historic", "2026-10-05", "POSTED", &[("ar2", 100), ("bank", -100)]),
                journal("payable", "2026-10-10", "POSTED", &[("bank", 100), ("ap", -100)]),
                journal("deposit", "2026-10-11", "POSTED", &[("bank", 100), ("uf", -100)]),
                journal("other-company", "2026-10-12", "POSTED", &[])
            ],
            "l2a": {
                "identities": [{"id": "rcp", "company_id": "c", "kind": "RCP", "number": "RCP-1"}, {"id": "inv", "company_id": "c", "kind": "INV", "number": "INV-1"}],
                "source_journals": [["rcp", "receipt"]],
                "setup": {"c": {"receivables": "ar", "payables": "ap", "undeposited": "uf"}},
                "snapshots": [{"role": "receivables:ar2", "account_id": "ar2", "source": "inv"}, {"role": "undeposited", "account_id": "uf", "source": "inv"}]
            }
        });
        let cash = evidence(&dump, "c", true, &HashMap::new()).unwrap();
        let day = |d: &str| NaiveDate::parse_from_str(d, "%Y-%m-%d").unwrap();
        assert_eq!(cash.unconverted(day("2026-10-31")), 3, "manual on ar, historic on ar2, manual on ap");
        assert_eq!(cash.unconverted(day("2026-10-04")), 1);
        assert_eq!(cash.unconverted(day("2026-09-30")), 0);
        assert_eq!(evidence(&dump, "c", false, &HashMap::new()).unwrap().unconverted(day("2026-10-31")), 0);
    }

    /// Shares come from dump amounts: a recognised change outside i64 is a failure, never a panic.
    #[test]
    fn share_changes_outside_i64_are_failures() {
        let huge = BTreeMap::from([("a", Money::from_cents(i128::from(i64::MAX) + 1))]);
        assert!(share_diff(&huge, &BTreeMap::new()).is_err());
        let low = BTreeMap::from([("a", Money::from_cents(i128::from(i64::MIN)))]);
        assert_eq!(share_diff(&low, &BTreeMap::new()), Ok(vec![("a", i64::MIN)]));
        let extreme = BTreeMap::from([("a", Money::from_cents(i128::MIN))]);
        assert!(share_diff(&BTreeMap::from([("a", Money::from_cents(i128::MAX))]), &extreme).is_err());
        // A settlement far above its document (total 1 cent) shares far outside i64: refused, not panicked.
        let after = financial::share(&[("a", Money::from_db(i64::MAX))], Money::from_cents(1), Money::from_db(fb_money::MAX_TOTAL_CENTS as i64)).unwrap();
        assert!(share_diff(&after, &BTreeMap::new()).is_err());
    }
}
