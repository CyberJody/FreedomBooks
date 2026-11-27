//! Dated customer subledger statement on the same evidence as A/R aging.

use super::receivables::ReceivableBalance;
use super::{check_range, check_rows, financial::date_of, receivables};
use crate::error::{FbError, FbResult};
use crate::services::access;
use chrono::NaiveDate;
use fb_db::rusqlite::{OptionalExtension, params};
use fb_db::{Book, Query};
use fb_ids::{CompanyId, UserId};
use fb_money::Money;
use std::collections::{BTreeMap, HashMap};

#[derive(Debug)]
pub struct CustomerChoice {
    pub id: String,
    pub name: String,
    pub active: bool,
}

pub const CUSTOMER_PICKER_LIMIT: usize = 100;

#[derive(Debug)]
pub struct CustomerPicker {
    pub choices: Vec<CustomerChoice>,
    pub truncated: bool,
}

fn choice(row: &fb_db::rusqlite::Row<'_>) -> fb_db::rusqlite::Result<CustomerChoice> {
    Ok(CustomerChoice {
        id: row.get(0)?,
        name: row.get(1)?,
        active: row.get(2)?,
    })
}

fn customer_choice(
    tx: &impl Query,
    company: CompanyId,
    customer: &str,
) -> FbResult<CustomerChoice> {
    tx.query_row(
        "SELECT id,display_name,active FROM directory_customer WHERE company_id=?1 AND id=?2",
        params![company.to_db(), customer],
        choice,
    )
    .optional()?
    .ok_or_else(|| FbError::missing("Customer"))
}

pub fn customers(
    book: &mut Book,
    actor: UserId,
    company: CompanyId,
    search: &str,
    selected: &str,
) -> FbResult<CustomerPicker> {
    book.read(|tx| {
        access(tx, actor, company)?;
        let mut stmt = tx.prepare(
            "SELECT id,display_name,active FROM directory_customer WHERE company_id=?1
               AND instr(fb_lower(display_name),fb_lower(?2))>0
             ORDER BY active DESC,display_name COLLATE fb_text,id LIMIT ?3",
        )?;
        let mut choices = stmt
            .query_map(
                params![
                    company.to_db(),
                    search.trim(),
                    CUSTOMER_PICKER_LIMIT as i64 + 1
                ],
                choice,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        let truncated = choices.len() > CUSTOMER_PICKER_LIMIT;
        choices.truncate(CUSTOMER_PICKER_LIMIT);
        if !selected.is_empty() && !choices.iter().any(|c| c.id == selected) {
            choices.push(customer_choice(tx, company, selected)?);
        }
        Ok(CustomerPicker { choices, truncated })
    })
}

#[derive(Debug)]
pub struct StatementLine {
    pub date: NaiveDate,
    pub activity: &'static str,
    pub number: String,
    pub journal: Option<String>,
    pub ar_delta: Money,
    pub credit_allocation: Money,
    pub ar_balance: Money,
}

#[derive(Debug)]
pub struct CustomerStatement {
    pub customer: String,
    pub opening_ar: Money,
    pub closing_ar: Money,
    pub opening_receivables: ReceivableBalance,
    pub closing_receivables: ReceivableBalance,
    pub lines: Vec<StatementLine>,
    pub opening_deposits: Money,
    pub closing_deposits: Money,
}

pub fn customer_statement(
    book: &mut Book,
    actor: UserId,
    company: CompanyId,
    customer: &str,
    start: NaiveDate,
    end: NaiveDate,
) -> FbResult<CustomerStatement> {
    book.read(|tx| {
        access(tx, actor, company)?;
        check_range(start, end)?;
        let name = customer_choice(tx, company, customer)?.name;
        let (docs, events) = receivables::load(tx, company, end, Some(customer))?;
        let mut opening_ar = Money::ZERO;
        let mut dated = BTreeMap::new();
        let mut add = |key, line: StatementLine| {
            if line.date < start {
                opening_ar += line.ar_delta;
            } else {
                dated.insert(key, line);
            }
            check_rows(dated.len())
        };
        for doc in &docs {
            let (original, reversed) = if doc.credit {
                ("Credit memo", "Credit memo void")
            } else {
                ("Invoice", "Invoice void")
            };
            add(
                (doc.date, 0, doc.id.clone()),
                line(
                    doc.date,
                    original,
                    doc.number.clone(),
                    Some(doc.journal.clone()),
                    doc.principal,
                    Money::ZERO,
                ),
            )?;
            if let Some((date, journal)) = &doc.reversal {
                add(
                    (*date, 1, doc.id.clone()),
                    line(
                        *date,
                        reversed,
                        doc.number.clone(),
                        Some(journal.clone()),
                        -doc.principal,
                        Money::ZERO,
                    ),
                )?;
            }
        }

        let by_identity: HashMap<_, _> = docs
            .iter()
            .map(|doc| (doc.identity.as_str(), doc))
            .collect();
        let mut allocations = BTreeMap::<_, StatementLine>::new();
        let mut opening_settled = HashMap::<_, Money>::new();
        for event in &events {
            let Some(doc) = by_identity.get(event.doc.as_str()) else {
                continue;
            };
            let delta = if doc.credit {
                event.amount
            } else {
                -event.amount
            };
            if event.date < start {
                opening_ar += delta;
                *opening_settled.entry(event.doc.as_str()).or_default() += event.amount;
                continue;
            }
            if event.journal.is_none() {
                // One application produces two events. Merge them into a single dated row
                // showing the amount allocated and its zero effect on customer A/R.
                let allocation = allocations.entry(event.order.clone()).or_insert_with(|| {
                    line(
                        event.date,
                        event.note,
                        String::new(),
                        None,
                        Money::ZERO,
                        event.amount,
                    )
                });
                allocation.ar_delta += delta;
                if !doc.credit {
                    allocation.number = event
                        .source
                        .as_deref()
                        .and_then(|identity| by_identity.get(identity))
                        .map_or_else(
                            || doc.number.clone(),
                            |credit| format!("{} · {}", doc.number, credit.number),
                        );
                }
                check_rows(allocations.len())?;
                continue;
            }
            let (journal, number) = event.journal.as_ref().expect("posted settlement");
            let number = format!("J{number:06} · {}", doc.number);
            dated.insert(
                (event.date, event.order.1 + 2, event.order.2.clone()),
                line(
                    event.date,
                    event.note,
                    number,
                    Some(journal.clone()),
                    delta,
                    Money::ZERO,
                ),
            );
            check_rows(dated.len())?;
        }
        for ((date, family, id), line) in allocations {
            dated.insert((date, family + 2, id), line);
        }
        check_rows(dated.len())?;

        let mut closing_ar = opening_ar;
        let lines = dated
            .into_values()
            .map(|mut line| {
                closing_ar += line.ar_delta;
                line.ar_balance = closing_ar;
                line
            })
            .collect();
        let settled = receivables::settlements(&events);
        let opening_date = start.pred_opt().expect("validated report start");
        let mut opening_receivables = ReceivableBalance::default();
        let mut closing_receivables = ReceivableBalance::default();
        for d in &docs {
            let identity = d.identity.as_str();
            if d.date < start {
                opening_receivables.add(
                    d.outstanding(
                        opening_date,
                        opening_settled.get(identity).copied().unwrap_or_default(),
                    ),
                    d.credit,
                );
            }
            closing_receivables.add(
                d.outstanding(end, settled.get(identity).copied().unwrap_or_default()),
                d.credit,
            );
        }
        if opening_ar != opening_receivables.net() || closing_ar != closing_receivables.net() {
            return Err(FbError::validation(
                "Customer statement activity does not reconcile to A/R aging.",
            ));
        }
        let (opening_deposits, closing_deposits) =
            deposit_balances(tx, company, customer, start, end)?;
        Ok(CustomerStatement {
            customer: name,
            opening_ar,
            closing_ar,
            opening_receivables,
            closing_receivables,
            lines,
            opening_deposits,
            closing_deposits,
        })
    })
}

fn line(
    date: NaiveDate,
    activity: &'static str,
    number: String,
    journal: Option<String>,
    ar_delta: Money,
    credit_allocation: Money,
) -> StatementLine {
    StatementLine {
        date,
        activity,
        number,
        journal,
        ar_delta,
        credit_allocation,
        ar_balance: Money::ZERO,
    }
}

fn deposit_balances(
    tx: &(impl Query + ?Sized),
    company: CompanyId,
    customer: &str,
    start: NaiveDate,
    end: NaiveDate,
) -> FbResult<(Money, Money)> {
    let mut stmt = tx.prepare(
        "WITH deposits AS (
           SELECT r.id,r.journal_id,d.liability_id
           FROM prepayments_customerdeposit d JOIN sales_receipt r ON r.id=d.receipt_id
           WHERE d.company_id=?1 AND r.company_id=?1 AND r.customer_id=?2
         ), movements(journal_id,liability_id) AS (
           SELECT journal_id,liability_id FROM deposits
           UNION ALL SELECT z.journal_id,d.liability_id FROM deposits d
             JOIN sales_receiptreversal z ON z.receipt_id=d.id
           UNION ALL SELECT a.journal_id,a.liability_id FROM deposits d
             JOIN prepayments_depositapplication a ON a.deposit_id=d.id AND a.company_id=?1
           UNION ALL SELECT z.journal_id,a.liability_id FROM deposits d
             JOIN prepayments_depositapplication a ON a.deposit_id=d.id AND a.company_id=?1
             JOIN prepayments_depositunapplication z ON z.application_id=a.id
           UNION ALL SELECT f.journal_id,f.liability_id FROM deposits d
             JOIN prepayments_depositrefund f ON f.deposit_id=d.id AND f.company_id=?1
           UNION ALL SELECT z.journal_id,f.liability_id FROM deposits d
             JOIN prepayments_depositrefund f ON f.deposit_id=d.id AND f.company_id=?1
             JOIN prepayments_depositrefundvoid z ON z.refund_id=f.id
         )
         SELECT j.date,l.credit,l.debit FROM movements m
         JOIN ledger_journal j ON j.id=m.journal_id AND j.company_id=?1 AND j.status='POSTED'
         JOIN ledger_journalline l ON l.journal_id=j.id AND l.account_id=m.liability_id
         WHERE j.date<=?3",
    )?;
    let mut rows = stmt.query(params![company.to_db(), customer, end.to_string()])?;
    let (mut opening, mut closing) = (Money::ZERO, Money::ZERO);
    while let Some(row) = rows.next()? {
        let delta = Money::from_db(row.get(1)?) - Money::from_db(row.get(2)?);
        closing += delta;
        if date_of(&row.get::<_, String>(0)?)? < start {
            opening += delta;
        }
    }
    Ok((opening, closing))
}
