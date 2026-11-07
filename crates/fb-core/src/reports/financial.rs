//! Port of `ledger/financial.py`: Profit & loss, Balance sheet and General ledger, accrual or cash basis.
//!
//! READ ONLY: one `Book::read` per report, so every statement sees the same snapshot (Python runs the view in
//! one REPEATABLE READ, READ ONLY transaction). Every figure is recomputed from POSTED journal lines plus, for
//! the cash basis, the settlement rows that already exist (receipt / bill-payment allocations, credit
//! applications, refunds, deposit applications and their reversals). Money is integer cents; the cash-basis
//! shares use exact integer fractions and Python's cent rule (see `share`).
//!
//! Cash basis (decision N19-D2), exactly as Python:
//! - Journals of INV, BILL and CM documents (originals AND reversals) are left out entirely.
//! - Their non-control lines are recognised when the document is settled, in proportion to the cumulative
//!   settled amount: receipt (RCP) and bill-payment (BPAY) allocations on the payment journal date, credit
//!   applications on the application date (both the invoice and the memo), credit refunds (RFD) on the refund
//!   journal date and customer-deposit applications (CDA) on the application journal date. Every reversal,
//!   unapplication or void releases the same amount negatively on its own date.
//! - On the settlement journals the A/R or A/P control line is replaced by those shares.
//! - Posted lines on receivables / payables control accounts from any other journal stay as recorded and are
//!   counted in `unconverted`.
//!
//! Python row ids order events and break cent-rule ties. Rust ids are UUIDv7 text whose byte order is
//! creation order, which is what the serial primary keys gave; journal ordering uses the journal number (the
//! J-number sequence that mirrors the Python journal pk).

use crate::error::{FbError, FbResult};
use crate::model::{Account, DetailType, Kind, Side};
use crate::services::access;
use crate::store;
use chrono::{Datelike, NaiveDate};
use fb_db::rusqlite::params;
use fb_db::{Book, Query};
use fb_ids::{AccountId, CompanyId, UserId};
use fb_money::Money;
use std::collections::{BTreeMap, BTreeSet, HashMap};

pub const ACCRUAL: &str = "ACCRUAL";
pub const CASH: &str = "CASH";
/// `BASES`: (value, label) in Python's order.
pub const BASES: [(&str, &str); 2] = [(ACCRUAL, "Accrual"), (CASH, "Cash")];
pub const DEFERRED_KINDS: [&str; 3] = ["INV", "BILL", "CM"];
/// P2-2: a deposit application settles its invoice.
pub const SETTLEMENT_KINDS: [&str; 4] = ["RCP", "BPAY", "RFD", "CDA"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Basis {
    Accrual,
    Cash,
}

impl Basis {
    pub fn as_str(self) -> &'static str {
        match self {
            Basis::Accrual => ACCRUAL,
            Basis::Cash => CASH,
        }
    }
    /// `BASIS_LABELS`.
    pub fn label(self) -> &'static str {
        match self {
            Basis::Accrual => "Accrual",
            Basis::Cash => "Cash",
        }
    }
    pub fn parse(text: &str) -> Option<Basis> {
        match text {
            ACCRUAL => Some(Basis::Accrual),
            CASH => Some(Basis::Cash),
            _ => None,
        }
    }
}

/// Profit & loss sections in report order: (key, heading, total label, sign). sign +1 = credit-positive.
pub const PL_SECTIONS: [(&str, &str, &str, i8); 5] = [
    ("income", "Income", "Total income", 1),
    ("cogs", "Cost of goods sold", "Total cost of goods sold", -1),
    ("expense", "Expense", "Total expense", -1),
    ("other_income", "Other income", "Total other income", 1),
    ("other_expense", "Other expense", "Total other expense", -1),
];

/// `PL_SECTION_OF` / `pl_section`: the P&L section key of an account type (None for balance sheet types).
pub fn pl_section(detail: DetailType) -> Option<&'static str> {
    match detail {
        DetailType::Income => Some("income"),
        DetailType::CostOfGoodsSold => Some("cogs"),
        DetailType::Expense => Some("expense"),
        DetailType::OtherIncome => Some("other_income"),
        DetailType::OtherExpense => Some("other_expense"),
        _ => None,
    }
}

/// One account type inside a balance-sheet group: (type, heading, total label, drill group).
pub type BsType = (DetailType, Option<&'static str>, Option<&'static str>, Option<&'static str>);
/// (group key, heading, total label, drill group, types).
pub type BsGroup = (&'static str, Option<&'static str>, Option<&'static str>, &'static str, &'static [BsType]);

/// `BS_LAYOUT`: (section key, heading, total label, kind, groups). A group with ONE type shows that type's
/// accounts directly; a group of several types nests one sub-heading per type.
pub const BS_LAYOUT: [(&str, &str, &str, Kind, &[BsGroup]); 3] = [
    (
        "assets",
        "Assets",
        "Total assets",
        Kind::Asset,
        &[
            (
                "current_assets",
                Some("Current assets"),
                Some("Total current assets"),
                "current_assets",
                &[
                    (DetailType::Bank, Some("Bank accounts"), Some("Total bank accounts"), Some("bank")),
                    (DetailType::Receivable, Some("Accounts receivable"), Some("Total accounts receivable"), Some("receivable")),
                    (DetailType::OtherCurrentAsset, Some("Other current assets"), Some("Total other current assets"), Some("other_current_assets")),
                ],
            ),
            ("fixed_assets", Some("Fixed assets"), Some("Total fixed assets"), "fixed_assets", &[(DetailType::FixedAsset, None, None, None)]),
            ("other_assets", Some("Other assets"), Some("Total other assets"), "other_assets", &[(DetailType::OtherAsset, None, None, None)]),
        ],
    ),
    (
        "liabilities",
        "Liabilities",
        "Total liabilities",
        Kind::Liability,
        &[
            (
                "current_liabilities",
                Some("Current liabilities"),
                Some("Total current liabilities"),
                "current_liabilities",
                &[
                    (DetailType::Payable, Some("Accounts payable"), Some("Total accounts payable"), Some("payable")),
                    (DetailType::CreditCard, Some("Credit cards"), Some("Total credit cards"), Some("credit_cards")),
                    (
                        DetailType::OtherCurrentLiability,
                        Some("Other current liabilities"),
                        Some("Total other current liabilities"),
                        Some("other_current_liabilities"),
                    ),
                ],
            ),
            (
                "long_term_liabilities",
                Some("Long-term liabilities"),
                Some("Total long-term liabilities"),
                "long_term_liabilities",
                &[(DetailType::LongTermLiability, None, None, None)],
            ),
        ],
    ),
    ("equity", "Equity", "Total equity", Kind::Equity, &[("equity", None, None, "equity", &[(DetailType::Equity, None, None, None)])]),
];

/// `GL_GROUPS`: (value, label) in Python's order.
pub const GL_GROUPS: [(&str, &str); 24] = [
    ("", "All accounts"),
    ("income", "Income accounts"),
    ("expense", "Expense accounts"),
    ("pl", "Income and expense accounts"),
    ("asset", "Asset accounts"),
    ("liability", "Liability accounts"),
    ("equity", "Equity accounts"),
    ("le", "Liability and equity accounts"),
    ("current_assets", "Current assets"),
    ("bank", "Bank accounts"),
    ("receivable", "Accounts receivable"),
    ("other_current_assets", "Other current assets"),
    ("fixed_assets", "Fixed assets"),
    ("other_assets", "Other assets"),
    ("current_liabilities", "Current liabilities"),
    ("payable", "Accounts payable"),
    ("credit_cards", "Credit cards"),
    ("other_current_liabilities", "Other current liabilities"),
    ("long_term_liabilities", "Long-term liabilities"),
    ("operating_income", "Income type only"),
    ("cogs", "Cost of goods sold"),
    ("operating_expense", "Expense type only"),
    ("other_income", "Other income"),
    ("other_expense", "Other expense"),
];

/// `GROUP_TYPES`: the account types of a general-ledger group (None for an unknown group; "" = all).
pub fn group_types(group: &str) -> Option<Vec<DetailType>> {
    use DetailType::*;
    let of_kind = |kinds: &[Kind]| DetailType::ALL.into_iter().filter(|d| kinds.contains(&d.kind())).collect::<Vec<_>>();
    Some(match group {
        "" => DetailType::ALL.to_vec(),
        "income" => of_kind(&[Kind::Income]),
        "expense" => of_kind(&[Kind::Expense]),
        "pl" => {
            let mut v = of_kind(&[Kind::Income]);
            v.extend(of_kind(&[Kind::Expense]));
            v
        }
        "asset" => of_kind(&[Kind::Asset]),
        "liability" => of_kind(&[Kind::Liability]),
        "equity" => of_kind(&[Kind::Equity]),
        "le" => {
            let mut v = of_kind(&[Kind::Liability]);
            v.extend(of_kind(&[Kind::Equity]));
            v
        }
        "current_assets" => vec![Bank, Receivable, OtherCurrentAsset],
        "bank" => vec![Bank],
        "receivable" => vec![Receivable],
        "other_current_assets" => vec![OtherCurrentAsset],
        "fixed_assets" => vec![FixedAsset],
        "other_assets" => vec![OtherAsset],
        "current_liabilities" => vec![Payable, CreditCard, OtherCurrentLiability],
        "payable" => vec![Payable],
        "credit_cards" => vec![CreditCard],
        "other_current_liabilities" => vec![OtherCurrentLiability],
        "long_term_liabilities" => vec![LongTermLiability],
        "operating_income" => vec![Income],
        "cogs" => vec![CostOfGoodsSold],
        "operating_expense" => vec![Expense],
        "other_income" => vec![OtherIncome],
        "other_expense" => vec![OtherExpense],
        _ => return None,
    })
}

// Cash-basis recognition ----------------------------------------------------------------------------------

/// Overflow while computing a share: amounts are bounded by the schema, so this is corrupt evidence.
fn overflow() -> FbError {
    FbError::validation("Report amounts exceed supported totals.")
}

/// `share`: recognised per-account amounts for cumulative settled amount `settled` of a document with
/// per-account nets `nets` (account order) and control amount `total` (> 0).
///
/// Cent rule: s_a * C / T floored to the cent in the direction of the document total, with the leftover cents
/// given one each to the largest fractional remainders (ties: lower account id). A fully settled document
/// recognises exactly its posted lines.
pub fn share<K: Ord + Clone>(nets: &[(K, Money)], total: Money, settled: Money) -> FbResult<BTreeMap<K, Money>> {
    if total.is_zero() || settled.is_zero() {
        return Ok(BTreeMap::new());
    }
    if settled == total {
        return Ok(nets.iter().map(|(k, v)| (k.clone(), *v)).collect());
    }
    let sum: i128 = nets.iter().try_fold(0i128, |acc, (_, n)| acc.checked_add(n.cents())).ok_or_else(overflow)?;
    let sigma: i128 = if sum >= 0 { 1 } else { -1 };
    // Fraction(settled, total) in cents; Python normalizes the sign into the numerator.
    let (mut num, mut den) = (settled.cents(), total.cents());
    if den < 0 {
        num = num.checked_neg().ok_or_else(overflow)?;
        den = den.checked_neg().ok_or_else(overflow)?;
    }
    // exact = net * sigma * num / den; floor and remainder with the same positive denominator.
    let mut parts: Vec<(K, i128, i128)> = Vec::with_capacity(nets.len());
    let mut floors: i128 = 0;
    for (account, net) in nets {
        let scaled = net.cents().checked_mul(sigma).and_then(|v| v.checked_mul(num)).ok_or_else(overflow)?;
        let floor = scaled.div_euclid(den);
        let rem = scaled.rem_euclid(den);
        floors = floors.checked_add(floor).ok_or_else(overflow)?;
        parts.push((account.clone(), floor, rem));
    }
    let leftover = settled.cents().checked_sub(floors).ok_or_else(overflow)?;
    let mut order: Vec<usize> = (0..parts.len()).collect();
    // sorted(parts, key=(-remainder, account)); a stable sort keeps Python's tie behaviour.
    order.sort_by(|&a, &b| parts[b].2.cmp(&parts[a].2).then_with(|| parts[a].0.cmp(&parts[b].0)));
    let take = usize::try_from(leftover.max(0)).unwrap_or(usize::MAX);
    for &index in order.iter().take(take) {
        parts[index].1 = parts[index].1.checked_add(1).ok_or_else(overflow)?;
    }
    let mut found = BTreeMap::new();
    for (account, cents, _) in parts {
        found.insert(account, Money::from_cents(sigma.checked_mul(cents).ok_or_else(overflow)?));
    }
    Ok(found)
}

/// `_diff`: the keys whose amount changed, with the change.
fn diff<K: Ord + Clone>(after: &BTreeMap<K, Money>, before: &BTreeMap<K, Money>) -> BTreeMap<K, Money> {
    let keys: BTreeSet<&K> = after.keys().chain(before.keys()).collect();
    keys.into_iter()
        .filter_map(|k| {
            let (a, b) = (after.get(k).copied().unwrap_or(Money::ZERO), before.get(k).copied().unwrap_or(Money::ZERO));
            (a != b).then(|| (k.clone(), a - b))
        })
        .collect()
}

/// One settlement event (Python `Event`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    /// The settled document identity (INV / BILL / CM).
    pub doc: String,
    pub date: NaiveDate,
    /// + settles, - releases.
    pub amount: Money,
    /// (date, family, settlement row id).
    pub order: (NaiveDate, u8, String),
    /// Identity shown in the general ledger (receipt / payment / credit memo / refund / deposit application).
    pub source: Option<String>,
    /// Settlement journal (None for credit applications) and its number (the Python journal pk order).
    pub journal: Option<(String, i64)>,
    /// Control account of the settled document.
    pub control: AccountId,
    pub note: &'static str,
}

/// A settled document's recognisable lines (Python `Document`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Document {
    pub identity: String,
    pub number: String,
    /// (account, signed net debit-credit), account order.
    pub nets: Vec<(AccountId, Money)>,
    /// Control amount T.
    pub total: Money,
}

fn date_of(text: &str) -> FbResult<NaiveDate> {
    NaiveDate::parse_from_str(text, "%Y-%m-%d").map_err(|_| FbError::validation("Stored date is invalid."))
}

fn account_of(text: &str) -> FbResult<AccountId> {
    AccountId::parse(text).ok_or_else(|| FbError::validation("Stored account id is invalid."))
}

/// `_events`: every settlement event up to `end` (inclusive), sorted by (date, family, row id). Five bounded
/// queries (one per settlement family).
fn events(tx: &(impl Query + ?Sized), company: CompanyId, end: NaiveDate) -> FbResult<Vec<Event>> {
    let company = company.to_db();
    let end_text = end.to_string();
    let mut events = Vec::new();
    // (family, settlement row id, amount, control, doc, source, journal id, journal number, journal date,
    //  undo journal id, undo number, undo date, undo status)
    type Row = (String, i64, String, String, Option<String>, String, i64, String, Option<String>, Option<i64>, Option<String>, Option<String>);
    let collect = |sql: &str| -> FbResult<Vec<Row>> {
        let mut stmt = tx.prepare(sql)?;
        let rows = stmt
            .query_map(params![company, end_text], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                    r.get(9)?,
                    r.get(10)?,
                    r.get(11)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    };
    fn push(events: &mut Vec<Event>, end: NaiveDate, rows: Vec<Row>, family: u8, note: &'static str, undo_note: &'static str) -> FbResult<()> {
        for (pk, amount, control, doc, source, journal, number, date, undo_journal, undo_number, undo_date, undo_status) in rows {
            let control = account_of(&control)?;
            let date = date_of(&date)?;
            let amount = Money::from_db(amount);
            events.push(Event {
                doc: doc.clone(),
                date,
                amount,
                order: (date, family, pk.clone()),
                source: source.clone(),
                journal: Some((journal, number)),
                control,
                note,
            });
            if let (Some(undo), Some(undo_journal), Some(undo_number)) = (undo_date, undo_journal, undo_number) {
                let undo = date_of(&undo)?;
                if undo <= end && undo_status.as_deref() == Some("POSTED") {
                    events.push(Event {
                        doc,
                        date: undo,
                        amount: -amount,
                        order: (undo, family + 1, pk),
                        source,
                        journal: Some((undo_journal, undo_number)),
                        control,
                        note: undo_note,
                    });
                }
            }
        }
        Ok(())
    }
    // Customer receipts (kind RCP) allocated to invoices.
    let receipts = collect(
        "SELECT a.id, a.amount, a.receivables_id, i.identity_id, r.identity_id, j.id, j.number, j.date,
                uj.id, uj.number, uj.date, uj.status
         FROM sales_receiptallocation a JOIN sales_receipt r ON r.id = a.receipt_id JOIN ledger_journal j ON j.id = r.journal_id
         JOIN sales_invoice i ON i.id = a.invoice_id
         LEFT JOIN sales_receiptreversal v ON v.receipt_id = r.id LEFT JOIN ledger_journal uj ON uj.id = v.journal_id
         WHERE r.company_id = ?1 AND r.kind = 'RCP' AND j.status = 'POSTED' AND i.company_id = ?1 AND j.date <= ?2",
    )?;
    push(&mut events, end, receipts, 0, "receipt", "receipt reversal")?;
    let payments = collect(
        "SELECT a.id, a.amount, a.payables_id, b.identity_id, p.identity_id, j.id, j.number, j.date,
                uj.id, uj.number, uj.date, uj.status
         FROM payables_paymentallocation a JOIN payables_billpayment p ON p.id = a.payment_id JOIN ledger_journal j ON j.id = p.journal_id
         JOIN payables_bill b ON b.id = a.bill_id
         LEFT JOIN payables_paymentreversal v ON v.payment_id = p.id LEFT JOIN ledger_journal uj ON uj.id = v.journal_id
         WHERE p.company_id = ?1 AND j.status = 'POSTED' AND b.company_id = ?1 AND j.date <= ?2",
    )?;
    push(&mut events, end, payments, 2, "bill payment", "bill payment reversal")?;
    // Credit applications settle both the invoice and the credit memo; no journal, no status filter.
    {
        let mut stmt = tx.prepare(
            "SELECT l.id, l.amount, l.receivables_id, i.identity_id, a.date, m.identity_id, m.receivables_id, u.date
             FROM credits_creditapplicationline l JOIN credits_creditapplication a ON a.id = l.application_id
             JOIN credits_creditmemo m ON m.id = a.credit_memo_id JOIN sales_invoice i ON i.id = l.invoice_id
             LEFT JOIN credits_creditunapplication u ON u.application_id = a.id
             WHERE a.company_id = ?1 AND m.company_id = ?1 AND i.company_id = ?1 AND a.date <= ?2",
        )?;
        let rows = stmt
            .query_map(params![company, end_text], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, Option<String>>(7)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        for (pk, amount, receivables, invoice, date, memo, memo_receivables, undo) in rows {
            let date = date_of(&date)?;
            let amount = Money::from_db(amount);
            let undo = undo.as_deref().map(date_of).transpose()?;
            for (doc, control) in [(invoice, receivables), (memo.clone(), memo_receivables)] {
                let control = account_of(&control)?;
                events.push(Event {
                    doc: doc.clone(),
                    date,
                    amount,
                    order: (date, 4, pk.clone()),
                    source: Some(memo.clone()),
                    journal: None,
                    control,
                    note: "credit applied",
                });
                if let Some(undo) = undo
                    && undo <= end
                {
                    events.push(Event {
                        doc,
                        date: undo,
                        amount: -amount,
                        order: (undo, 5, pk.clone()),
                        source: Some(memo.clone()),
                        journal: None,
                        control,
                        note: "credit unapplied",
                    });
                }
            }
        }
    }
    let refunds = collect(
        "SELECT f.id, f.amount, f.receivables_id, m.identity_id, f.identity_id, j.id, j.number, j.date,
                uj.id, uj.number, uj.date, uj.status
         FROM credits_creditrefund f JOIN credits_creditmemo m ON m.id = f.credit_memo_id JOIN ledger_journal j ON j.id = f.journal_id
         LEFT JOIN credits_creditrefundvoid v ON v.refund_id = f.id LEFT JOIN ledger_journal uj ON uj.id = v.journal_id
         WHERE f.company_id = ?1 AND m.company_id = ?1 AND j.status = 'POSTED' AND j.date <= ?2",
    )?;
    push(&mut events, end, refunds, 6, "refund", "refund void")?;
    // P2-2: a customer deposit application settles its invoice on the application journal date; its unapply
    // (exact reversal journal) releases the same share on its own date.
    let deposits = collect(
        "SELECT d.id, d.amount, d.receivables_id, i.identity_id, d.identity_id, j.id, j.number, j.date,
                uj.id, uj.number, uj.date, uj.status
         FROM prepayments_depositapplication d JOIN ledger_journal j ON j.id = d.journal_id JOIN sales_invoice i ON i.id = d.invoice_id
         LEFT JOIN prepayments_depositunapplication u ON u.application_id = d.id LEFT JOIN ledger_journal uj ON uj.id = u.journal_id
         WHERE d.company_id = ?1 AND i.company_id = ?1 AND j.status = 'POSTED' AND j.date <= ?2",
    )?;
    push(&mut events, end, deposits, 8, "deposit applied", "deposit unapplied")?;
    // Python's list.sort is stable: equal keys (the invoice and memo events of one credit line) keep their order.
    events.sort_by(|a, b| a.order.cmp(&b.order));
    Ok(events)
}

/// `_documents`: recognisable lines (all non-control lines of every ORIGINAL posted journal) per settled document.
fn documents(tx: &(impl Query + ?Sized), company: CompanyId, events: &[Event]) -> FbResult<HashMap<String, Document>> {
    let mut controls: HashMap<&str, AccountId> = HashMap::new();
    for event in events {
        controls.entry(event.doc.as_str()).or_insert(event.control);
    }
    if controls.is_empty() {
        return Ok(HashMap::new());
    }
    let mut stmt = tx.prepare(
        "SELECT j.source_id, s.number, l.account_id, l.debit, l.credit
         FROM ledger_journalline l JOIN ledger_journal j ON j.id = l.journal_id JOIN ledger_account a ON a.id = l.account_id
         JOIN documents_documentidentity s ON s.id = j.source_id
         WHERE j.company_id = ?1 AND a.company_id = ?1 AND j.status = 'POSTED' AND j.reversal_of_id IS NULL
           AND s.kind IN ('INV', 'BILL', 'CM')",
    )?;
    let rows = stmt
        .query_map([company.to_db()], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?, r.get::<_, i64>(4)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut nets: HashMap<String, BTreeMap<AccountId, Money>> = HashMap::new();
    let mut numbers: HashMap<String, String> = HashMap::new();
    for (doc, number, account, debit, credit) in rows {
        let Some(control) = controls.get(doc.as_str()) else { continue };
        let account = account_of(&account)?;
        numbers.insert(doc.clone(), number);
        if account != *control {
            *nets.entry(doc).or_default().entry(account).or_default() += Money::from_db(debit) - Money::from_db(credit);
        }
    }
    let mut found = HashMap::new();
    for (doc, by_account) in nets {
        let lines: Vec<(AccountId, Money)> = by_account.into_iter().filter(|(_, net)| !net.is_zero()).collect();
        let sum: Money = lines.iter().map(|(_, n)| *n).sum();
        let total = Money::from_cents(sum.cents().abs());
        let number = numbers.remove(&doc).unwrap_or_default();
        found.insert(doc.clone(), Document { identity: doc, number, nets: lines, total });
    }
    Ok(found)
}

/// One posted line as the reports read it.
#[derive(Clone, Debug)]
struct Line {
    id: String,
    account: AccountId,
    debit: Money,
    credit: Money,
    description: String,
    name: String,
    position: i64,
    journal_id: String,
    journal_number: i64,
    date: NaiveDate,
    memo: String,
    entry_number: String,
    source_id: Option<String>,
    reversal: bool,
    /// Left out (and replaced by recognised shares) on the cash basis.
    cash_excluded: bool,
}

/// Every posted line of the company through `end` (`posted_lines` + `_within`), with the cash-basis exclusion
/// (`cash_excluded()`: a deferred document's journal, or the A/R / A/P control line of a settlement journal).
fn posted_lines(tx: &(impl Query + ?Sized), company: CompanyId, end: NaiveDate) -> FbResult<Vec<Line>> {
    let mut stmt = tx.prepare(
        "SELECT l.id, l.account_id, l.debit, l.credit, l.description, l.name, l.position, j.id, j.number, j.date, j.memo,
                j.entry_number, j.source_id, j.reversal_of_id IS NOT NULL,
                (s.kind IN ('INV', 'BILL', 'CM'))
                OR (s.kind = 'RCP' AND EXISTS (SELECT 1 FROM sales_receiptallocation x JOIN sales_receipt r ON r.id = x.receipt_id
                                              WHERE r.identity_id = j.source_id AND x.receivables_id = l.account_id))
                OR (s.kind = 'BPAY' AND EXISTS (SELECT 1 FROM payables_paymentallocation x JOIN payables_billpayment p ON p.id = x.payment_id
                                               WHERE p.identity_id = j.source_id AND x.payables_id = l.account_id))
                OR (s.kind = 'RFD' AND EXISTS (SELECT 1 FROM credits_creditrefund f WHERE f.identity_id = j.source_id AND f.receivables_id = l.account_id))
                OR (s.kind = 'CDA' AND EXISTS (SELECT 1 FROM prepayments_depositapplication d
                                              WHERE d.identity_id = j.source_id AND d.receivables_id = l.account_id))
         FROM ledger_journalline l JOIN ledger_journal j ON j.id = l.journal_id JOIN ledger_account a ON a.id = l.account_id
         LEFT JOIN documents_documentidentity s ON s.id = j.source_id
         WHERE j.company_id = ?1 AND a.company_id = ?1 AND j.status = 'POSTED' AND j.date <= ?2
         ORDER BY j.date, j.number, l.position, l.id",
    )?;
    let rows = stmt
        .query_map(params![company.to_db(), end.to_string()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, i64>(6)?,
                r.get::<_, String>(7)?,
                r.get::<_, i64>(8)?,
                r.get::<_, String>(9)?,
                r.get::<_, String>(10)?,
                r.get::<_, String>(11)?,
                r.get::<_, Option<String>>(12)?,
                r.get::<_, bool>(13)?,
                r.get::<_, Option<bool>>(14)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(id, account, debit, credit, description, name, position, journal_id, journal_number, date, memo, entry_number, source_id, reversal, excluded)| {
            Ok(Line {
                id,
                account: account_of(&account)?,
                debit: Money::from_db(debit),
                credit: Money::from_db(credit),
                description,
                name,
                position,
                journal_id,
                journal_number,
                date: date_of(&date)?,
                memo,
                entry_number,
                source_id,
                reversal,
                cash_excluded: excluded.unwrap_or(false),
            })
        })
        .collect()
}

/// Posted books of one company on one basis, through `end` inclusive (Python `Books`). Built inside one read
/// snapshot; the cash-basis events and settled-document lines are loaded once.
pub struct Books {
    pub basis: Basis,
    pub end: NaiveDate,
    pub events: Vec<Event>,
    pub documents: HashMap<String, Document>,
    pub unconverted: i64,
    lines: Vec<Line>,
}

impl Books {
    pub fn load(tx: &(impl Query + ?Sized), company: CompanyId, basis: Basis, end: NaiveDate) -> FbResult<Books> {
        let lines = posted_lines(tx, company, end)?;
        let (mut events, mut docs, mut unconverted) = (Vec::new(), HashMap::new(), 0);
        if basis == Basis::Cash {
            events = self::events(tx, company, end)?;
            docs = documents(tx, company, &events)?;
            unconverted = Self::count_unconverted(tx, company, end)?;
        }
        Ok(Books { basis, end, events, documents: docs, unconverted, lines })
    }

    /// `_unconverted`: posted lines through `end` on receivables / payables control accounts (the account's
    /// highest-priority control role) from journals that are not deferred documents or settlements.
    fn count_unconverted(tx: &(impl Query + ?Sized), company: CompanyId, end: NaiveDate) -> FbResult<i64> {
        Ok(tx.query_row(
            "WITH roles AS (SELECT account_id, min(priority) priority FROM fb_control_accounts WHERE company_id = ?1 GROUP BY account_id)
             SELECT count(*) FROM ledger_journalline l JOIN ledger_journal j ON j.id = l.journal_id JOIN ledger_account a ON a.id = l.account_id
             JOIN roles c ON c.account_id = l.account_id AND c.priority IN (0, 1)
             LEFT JOIN documents_documentidentity s ON s.id = j.source_id
             WHERE j.company_id = ?1 AND a.company_id = ?1 AND j.status = 'POSTED' AND j.date <= ?2
               AND (s.kind IS NULL OR s.kind NOT IN ('INV', 'BILL', 'CM', 'RCP', 'BPAY', 'RFD', 'CDA'))",
            params![company.to_db(), end.to_string()],
            |r| r.get(0),
        )?)
    }

    fn within(&self, start: Option<NaiveDate>, end: Option<NaiveDate>) -> impl Iterator<Item = &Line> {
        let cash = self.basis == Basis::Cash;
        self.lines
            .iter()
            .filter(move |l| start.is_none_or(|s| l.date >= s) && end.is_none_or(|e| l.date <= e) && !(cash && l.cash_excluded))
    }

    /// `nets`: {account: (net debit-credit, activity count)} for journal dates start..end inclusive.
    pub fn nets(&self, start: Option<NaiveDate>, end: Option<NaiveDate>) -> FbResult<HashMap<AccountId, (Money, i64)>> {
        let mut found: HashMap<AccountId, (Money, i64)> = HashMap::new();
        for line in self.within(start, end) {
            let entry = found.entry(line.account).or_insert((Money::ZERO, 0));
            entry.0 += line.debit - line.credit;
            entry.1 += 1;
        }
        if self.basis == Basis::Cash {
            for (account, amount) in self.recognised(start, end)? {
                let entry = found.entry(account).or_insert((Money::ZERO, 0));
                entry.0 += amount;
                entry.1 += 1;
            }
        }
        Ok(found)
    }

    /// `recognised`: cash-basis recognised amounts per account for events dated start..end.
    pub fn recognised(&self, start: Option<NaiveDate>, end: Option<NaiveDate>) -> FbResult<BTreeMap<AccountId, Money>> {
        let mut before: BTreeMap<&str, Money> = BTreeMap::new();
        let mut through: BTreeMap<&str, Money> = BTreeMap::new();
        for event in &self.events {
            if end.is_some_and(|e| event.date > e) {
                continue;
            }
            let target = if start.is_some_and(|s| event.date < s) { &mut before } else { &mut through };
            *target.entry(event.doc.as_str()).or_default() += event.amount;
        }
        let mut total: BTreeMap<AccountId, Money> = BTreeMap::new();
        let docs: BTreeSet<&str> = before.keys().chain(through.keys()).copied().collect();
        for doc_id in docs {
            let Some(doc) = self.documents.get(doc_id) else { continue };
            let settled_before = before.get(doc_id).copied().unwrap_or(Money::ZERO);
            let after = share(&doc.nets, doc.total, settled_before + through.get(doc_id).copied().unwrap_or(Money::ZERO))?;
            for (account, amount) in diff(&after, &share(&doc.nets, doc.total, settled_before)?) {
                *total.entry(account).or_default() += amount;
            }
        }
        total.retain(|_, v| !v.is_zero());
        Ok(total)
    }

    /// `recognition_rows`: event-level cash-basis rows dated start..end (differences of consecutive shares).
    pub fn recognition_rows(&self, start: Option<NaiveDate>, end: Option<NaiveDate>, accounts: Option<&BTreeSet<AccountId>>) -> FbResult<Vec<Recognition<'_>>> {
        let mut rows = Vec::new();
        let mut settled: HashMap<&str, Money> = HashMap::new();
        for event in &self.events {
            let doc = self.documents.get(&event.doc);
            let entry = settled.entry(event.doc.as_str()).or_default();
            let before = *entry;
            *entry += event.amount;
            let after = *entry;
            let Some(doc) = doc else { continue };
            if end.is_some_and(|e| event.date > e) || start.is_some_and(|s| event.date < s) {
                continue;
            }
            for (account, amount) in diff(&share(&doc.nets, doc.total, after)?, &share(&doc.nets, doc.total, before)?) {
                if accounts.is_some_and(|set| !set.contains(&account)) {
                    continue;
                }
                rows.push(Recognition { date: event.date, account, debit: amount.max(Money::ZERO), credit: (-amount).max(Money::ZERO), event, document: doc });
            }
        }
        Ok(rows)
    }
}

/// One event-level cash-basis row.
#[derive(Clone, Debug)]
pub struct Recognition<'a> {
    pub date: NaiveDate,
    pub account: AccountId,
    pub debit: Money,
    pub credit: Money,
    pub event: &'a Event,
    pub document: &'a Document,
}

// Reports ------------------------------------------------------------------------------------------------

/// Accounts in Python's `sorted(key=(code, pk))`: code by code point, then creation order.
fn sorted_accounts(tx: &(impl Query + ?Sized), company: CompanyId) -> FbResult<Vec<Account>> {
    let mut accounts = store::accounts(tx, company)?;
    accounts.sort_by(|a, b| a.code.cmp(&b.code).then(a.id.cmp(&b.id)));
    Ok(accounts)
}

#[derive(Clone, Debug, PartialEq)]
pub struct AmountRow {
    pub account: Account,
    pub amount: Money,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlSection {
    pub key: &'static str,
    pub heading: &'static str,
    pub total_label: &'static str,
    pub rows: Vec<AmountRow>,
    pub total: Money,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProfitAndLoss {
    pub basis: Basis,
    pub start: Option<NaiveDate>,
    pub end: NaiveDate,
    /// In `PL_SECTIONS` order.
    pub sections: Vec<PlSection>,
    pub gross_profit: Money,
    pub net_ordinary_income: Money,
    pub net_other_income: Money,
    pub net_income: Money,
    pub unconverted: i64,
    pub show_cogs: bool,
    pub show_other: bool,
}

impl ProfitAndLoss {
    pub fn section(&self, key: &str) -> Option<&PlSection> {
        self.sections.iter().find(|s| s.key == key)
    }
}

fn pl_from(books: &Books, accounts: &[Account], start: Option<NaiveDate>, end: NaiveDate) -> FbResult<ProfitAndLoss> {
    let nets = books.nets(start, Some(end))?;
    let mut sections: Vec<PlSection> =
        PL_SECTIONS.iter().map(|&(key, heading, total_label, _)| PlSection { key, heading, total_label, rows: Vec::new(), total: Money::ZERO }).collect();
    for account in accounts {
        let Some(key) = pl_section(account.detail_type) else { continue };
        let Some(&(net, _)) = nets.get(&account.id) else { continue };
        let index = PL_SECTIONS.iter().position(|s| s.0 == key).unwrap_or(0);
        let amount = if PL_SECTIONS[index].3 > 0 { -net } else { net };
        sections[index].rows.push(AmountRow { account: account.clone(), amount });
        sections[index].total += amount;
    }
    let total = |key: &str| sections.iter().find(|s| s.key == key).map(|s| s.total).unwrap_or(Money::ZERO);
    let gross = total("income") - total("cogs");
    let ordinary = gross - total("expense");
    let other = total("other_income") - total("other_expense");
    let show_cogs = !sections[1].rows.is_empty();
    let show_other = !sections[3].rows.is_empty() || !sections[4].rows.is_empty();
    Ok(ProfitAndLoss {
        basis: books.basis,
        start,
        end,
        sections,
        gross_profit: gross,
        net_ordinary_income: ordinary,
        net_other_income: other,
        net_income: ordinary + other,
        unconverted: books.unconverted,
        show_cogs,
        show_other,
    })
}

/// `profit_and_loss`.
pub fn profit_and_loss(book: &mut Book, actor: UserId, company: CompanyId, basis: Basis, start: Option<NaiveDate>, end: NaiveDate) -> FbResult<ProfitAndLoss> {
    book.read(|tx| {
        access(tx, actor, company)?;
        let books = Books::load(tx, company, basis, end)?;
        pl_from(&books, &sorted_accounts(tx, company)?, start, end)
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct BsTypePart {
    pub detail_type: DetailType,
    pub heading: Option<&'static str>,
    pub total_label: Option<&'static str>,
    pub drill: Option<&'static str>,
    pub rows: Vec<AmountRow>,
    pub total: Money,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BsGroupPart {
    pub key: &'static str,
    pub heading: Option<&'static str>,
    pub total_label: Option<&'static str>,
    pub drill: &'static str,
    pub nested: bool,
    pub types: Vec<BsTypePart>,
    pub total: Money,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BsSection {
    pub kind: Kind,
    pub rows: Vec<AmountRow>,
    pub total: Money,
    pub groups: Vec<BsGroupPart>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BalanceSheet {
    pub basis: Basis,
    pub as_of: NaiveDate,
    pub fiscal_year_start: NaiveDate,
    pub assets: BsSection,
    pub liabilities: BsSection,
    pub equity: BsSection,
    pub retained_earnings: Money,
    pub net_income: Money,
    pub total_equity: Money,
    pub total_liabilities_equity: Money,
    pub balanced: bool,
    pub unconverted: i64,
}

/// `fiscal_year_start`: decision N19-D3, the company has no fiscal-year setting, so it is the calendar year.
pub fn fiscal_year_start(on: NaiveDate) -> NaiveDate {
    NaiveDate::from_ymd_opt(on.year(), 1, 1).unwrap_or(on)
}

/// `balance_sheet`.
pub fn balance_sheet(book: &mut Book, actor: UserId, company: CompanyId, basis: Basis, as_of: NaiveDate) -> FbResult<BalanceSheet> {
    book.read(|tx| {
        access(tx, actor, company)?;
        let books = Books::load(tx, company, basis, as_of)?;
        let accounts = sorted_accounts(tx, company)?;
        let fy_start = fiscal_year_start(as_of);
        let everything = books.nets(None, Some(as_of))?;
        let mut sections: Vec<BsSection> =
            [Kind::Asset, Kind::Liability, Kind::Equity].into_iter().map(|kind| BsSection { kind, rows: Vec::new(), total: Money::ZERO, groups: Vec::new() }).collect();
        let mut by_type: HashMap<DetailType, Vec<AmountRow>> = HashMap::new();
        for account in &accounts {
            let Some(index) = sections.iter().position(|s| s.kind == account.kind) else { continue };
            let Some(&(net, _)) = everything.get(&account.id) else { continue };
            // Section side; a contra account reads negative.
            let amount = if account.kind == Kind::Asset { net } else { -net };
            if !amount.is_zero() {
                let row = AmountRow { account: account.clone(), amount };
                sections[index].rows.push(row.clone());
                sections[index].total += amount;
                by_type.entry(account.detail_type).or_default().push(row);
            }
        }
        for (_, _, _, kind, groups) in BS_LAYOUT {
            let mut built = Vec::new();
            for &(key, heading, total_label, drill, types) in groups {
                let parts: Vec<BsTypePart> = types
                    .iter()
                    .map(|&(detail, type_heading, type_total, type_drill)| {
                        let rows = by_type.get(&detail).cloned().unwrap_or_default();
                        let total = rows.iter().map(|r| r.amount).sum();
                        BsTypePart { detail_type: detail, heading: type_heading, total_label: type_total, drill: type_drill, rows, total }
                    })
                    .filter(|p| !p.rows.is_empty())
                    .collect();
                if !parts.is_empty() {
                    let total = parts.iter().map(|p| p.total).sum();
                    built.push(BsGroupPart { key, heading, total_label, drill, nested: types.len() > 1, types: parts, total });
                }
            }
            if let Some(section) = sections.iter_mut().find(|s| s.kind == kind) {
                section.groups = built;
            }
        }
        let prior = books.nets(None, fy_start.pred_opt())?;
        let mut retained = Money::ZERO;
        for account in &accounts {
            if pl_section(account.detail_type).is_some()
                && let Some(&(net, _)) = prior.get(&account.id)
            {
                retained -= net;
            }
        }
        let year = pl_from(&books, &accounts, Some(fy_start), as_of)?;
        let mut sections = sections.into_iter();
        let (assets, liabilities, equity) = match (sections.next(), sections.next(), sections.next()) {
            (Some(a), Some(l), Some(e)) => (a, l, e),
            _ => return Err(FbError::validation("Balance sheet sections are incomplete.")),
        };
        let total_equity = equity.total + retained + year.net_income;
        let total_liabilities_equity = liabilities.total + total_equity;
        Ok(BalanceSheet {
            basis,
            as_of,
            fiscal_year_start: fy_start,
            balanced: total_liabilities_equity == assets.total,
            assets,
            liabilities,
            equity,
            retained_earnings: retained,
            net_income: year.net_income,
            total_equity,
            total_liabilities_equity,
            unconverted: books.unconverted,
        })
    })
}

/// One general-ledger row: a posted line, or (cash basis) a recognised share.
#[derive(Clone, Debug, PartialEq)]
pub struct LedgerRow {
    pub date: NaiveDate,
    pub debit: Money,
    pub credit: Money,
    /// Journal id and number (None for a credit application share).
    pub journal: Option<(String, i64)>,
    pub source_id: Option<String>,
    pub reversal: bool,
    pub entry_number: String,
    pub memo: String,
    pub name: String,
    /// The event note of a cash-basis share (None for a posted line).
    pub synthetic: Option<&'static str>,
    pub balance: Money,
    /// The posted line id (None for a share).
    pub line_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LedgerSection {
    pub account: Account,
    pub opening: Money,
    pub rows: Vec<LedgerRow>,
    pub debit: Money,
    pub credit: Money,
    pub closing: Money,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GeneralLedger {
    pub basis: Basis,
    pub start: Option<NaiveDate>,
    pub end: NaiveDate,
    pub sections: Vec<LedgerSection>,
    pub unconverted: i64,
    pub debit: Money,
    pub credit: Money,
}

/// Python's sort key: (date, journal pk or 10**15, position or 10**6 + family, row pk).
type RowKey = (NaiveDate, i64, i64, String);

/// `general_ledger`: per account, the opening balance, every line dated start..end with a running balance
/// on the account's section side, and the closing balance. `account` must belong to the company.
#[allow(clippy::too_many_arguments)]
pub fn general_ledger(
    book: &mut Book,
    actor: UserId,
    company: CompanyId,
    basis: Basis,
    start: Option<NaiveDate>,
    end: NaiveDate,
    account: Option<AccountId>,
    group: &str,
) -> FbResult<GeneralLedger> {
    let types = group_types(group).ok_or_else(|| FbError::validation(format!("Select a valid choice. {group} is not one of the available choices.")))?;
    book.read(|tx| {
        access(tx, actor, company)?;
        let accounts = sorted_accounts(tx, company)?;
        if let Some(id) = account
            && !accounts.iter().any(|a| a.id == id)
        {
            return Err(FbError::missing("Account"));
        }
        let books = Books::load(tx, company, basis, end)?;
        let chosen: BTreeSet<AccountId> = accounts
            .iter()
            .filter(|a| account.is_none_or(|id| id == a.id) && (group.is_empty() || types.contains(&a.detail_type)))
            .map(|a| a.id)
            .collect();
        let opening = match start {
            Some(s) => books.nets(None, s.pred_opt())?,
            None => HashMap::new(),
        };
        let mut rows: HashMap<AccountId, Vec<(RowKey, LedgerRow)>> = HashMap::new();
        for line in books.within(start, Some(end)).filter(|l| chosen.contains(&l.account)) {
            let key = (line.date, line.journal_number, line.position, line.id.clone());
            let memo = if line.description.is_empty() { line.memo.clone() } else { line.description.clone() };
            rows.entry(line.account).or_default().push((
                key,
                LedgerRow {
                    date: line.date,
                    debit: line.debit,
                    credit: line.credit,
                    journal: Some((line.journal_id.clone(), line.journal_number)),
                    source_id: line.source_id.clone(),
                    reversal: line.reversal,
                    entry_number: line.entry_number.clone(),
                    memo,
                    name: line.name.clone(),
                    synthetic: None,
                    balance: Money::ZERO,
                    line_id: Some(line.id.clone()),
                },
            ));
        }
        if basis == Basis::Cash {
            for item in books.recognition_rows(start, Some(end), Some(&chosen))? {
                let event = item.event;
                let key = (item.date, event.journal.as_ref().map(|j| j.1).unwrap_or(1_000_000_000_000_000), 1_000_000 + i64::from(event.order.1), event.order.2.clone());
                rows.entry(item.account).or_default().push((
                    key,
                    LedgerRow {
                        date: item.date,
                        debit: item.debit,
                        credit: item.credit,
                        journal: event.journal.clone(),
                        source_id: event.source.clone(),
                        reversal: event.amount < Money::ZERO,
                        entry_number: String::new(),
                        memo: format!("Cash basis: share of {} ({})", item.document.number, event.note),
                        name: String::new(),
                        synthetic: Some(event.note),
                        balance: Money::ZERO,
                        line_id: None,
                    },
                ));
            }
        }
        let mut sections = Vec::new();
        let (mut all_debit, mut all_credit) = (Money::ZERO, Money::ZERO);
        for acct in accounts.iter().filter(|a| chosen.contains(&a.id)) {
            let sign: i128 = if acct.section_balance() == Side::Debit { 1 } else { -1 };
            let start_balance = Money::from_cents(sign * opening.get(&acct.id).map(|e| e.0).unwrap_or(Money::ZERO).cents());
            let mut entries = rows.remove(&acct.id).unwrap_or_default();
            if entries.is_empty() && start_balance.is_zero() {
                continue;
            }
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            let (mut balance, mut debit, mut credit) = (start_balance, Money::ZERO, Money::ZERO);
            let mut out = Vec::with_capacity(entries.len());
            for (_, mut entry) in entries {
                balance += Money::from_cents(sign * (entry.debit - entry.credit).cents());
                entry.balance = balance;
                debit += entry.debit;
                credit += entry.credit;
                out.push(entry);
            }
            all_debit += debit;
            all_credit += credit;
            sections.push(LedgerSection { account: acct.clone(), opening: start_balance, rows: out, debit, credit, closing: balance });
        }
        Ok(GeneralLedger { basis, start, end, sections, unconverted: books.unconverted, debit: all_debit, credit: all_credit })
    })
}
