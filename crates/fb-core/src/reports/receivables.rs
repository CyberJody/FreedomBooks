//! Shared posted principal and dated settlements for aging and customer statements.

use super::financial::{Event, date_of, events};
use crate::error::FbResult;
use chrono::NaiveDate;
use fb_db::{Query, rusqlite::params};
use fb_ids::CompanyId;
use fb_money::Money;
use std::collections::HashMap;

pub(super) struct Receivable {
    pub id: String,
    pub identity: String,
    pub number: String,
    pub customer_id: String,
    pub customer: String,
    pub date: NaiveDate,
    pub due: NaiveDate,
    pub principal: Money,
    pub journal: String,
    pub reversal: Option<(NaiveDate, String)>,
    pub credit: bool,
}

impl Receivable {
    pub fn outstanding(&self, as_of: NaiveDate, settled: Money) -> Money {
        let principal = if self
            .reversal
            .as_ref()
            .is_some_and(|(date, _)| *date <= as_of)
        {
            Money::ZERO
        } else {
            self.principal
        };
        if self.credit {
            principal + settled
        } else {
            principal - settled
        }
    }
}

#[derive(Debug, Default)]
pub struct ReceivableBalance {
    pub open_invoices: Money,
    pub unapplied_credits: Money,
}

impl ReceivableBalance {
    pub(super) fn add(&mut self, balance: Money, credit_memo: bool) {
        if Self::is_credit(balance, credit_memo) {
            self.unapplied_credits += balance;
        } else {
            self.open_invoices += balance;
        }
    }

    pub(super) fn is_credit(balance: Money, credit_memo: bool) -> bool {
        // A void dated before a receipt reversal can leave a temporary invoice credit.
        credit_memo || balance < Money::ZERO
    }

    pub fn net(&self) -> Money {
        self.open_invoices + self.unapplied_credits
    }
}

pub(super) fn load(
    tx: &(impl Query + ?Sized),
    company: CompanyId,
    end: NaiveDate,
    customer: Option<&str>,
) -> FbResult<(Vec<Receivable>, Vec<Event>)> {
    let mut stmt = tx.prepare(
        "WITH principal AS (
           SELECT i.id,i.identity_id,r.customer_id,j.date,r.due_date,r.total,j.id journal,
                  z.date reversed,z.id reversal,0 credit
           FROM sales_invoice i
           JOIN sales_invoicerevision r ON r.invoice_id=i.id AND r.state='OPEN'
           JOIN ledger_journal j ON j.id=r.journal_id AND j.company_id=i.company_id AND j.status='POSTED'
           LEFT JOIN sales_invoicerevision v ON v.invoice_id=i.id AND v.state='VOID'
           LEFT JOIN ledger_journal z ON z.id=v.reversal_id AND z.company_id=i.company_id
              AND z.status='POSTED' AND z.date<=?2
           WHERE i.company_id=?1 AND j.date<=?2 AND (?3 IS NULL OR r.customer_id=?3)
           UNION ALL
           SELECT m.id,m.identity_id,m.customer_id,j.date,j.date,-m.total,j.id,z.date,z.id,1
           FROM credits_creditmemo m
           JOIN ledger_journal j ON j.id=m.journal_id AND j.company_id=m.company_id AND j.status='POSTED'
           LEFT JOIN credits_creditmemovoid v ON v.credit_memo_id=m.id
           LEFT JOIN ledger_journal z ON z.id=v.journal_id AND z.company_id=m.company_id
              AND z.status='POSTED' AND z.date<=?2
           WHERE m.company_id=?1 AND j.date<=?2 AND (?3 IS NULL OR m.customer_id=?3)
         )
         SELECT p.id,p.identity_id,d.number,c.id,c.display_name,p.date,p.due_date,p.total,
                p.journal,p.reversed,p.reversal,p.credit
         FROM principal p JOIN documents_documentidentity d ON d.id=p.identity_id AND d.company_id=?1
         JOIN directory_customer c ON c.id=p.customer_id AND c.company_id=?1
         ORDER BY c.display_name COLLATE fb_text,c.id,p.credit,p.due_date,d.number,p.id",
    )?;
    let mut docs = Vec::new();
    let mut rows = stmt.query(params![company.to_db(), end.to_string(), customer])?;
    while let Some(r) = rows.next()? {
        let reversed: Option<String> = r.get(9)?;
        let reversal: Option<String> = r.get(10)?;
        docs.push(Receivable {
            id: r.get(0)?,
            identity: r.get(1)?,
            number: r.get(2)?,
            customer_id: r.get(3)?,
            customer: r.get(4)?,
            date: date_of(&r.get::<_, String>(5)?)?,
            due: date_of(&r.get::<_, String>(6)?)?,
            principal: Money::from_db(r.get(7)?),
            journal: r.get(8)?,
            reversal: reversed
                .zip(reversal)
                .map(|(date, id)| Ok::<_, crate::FbError>((date_of(&date)?, id)))
                .transpose()?,
            credit: r.get(11)?,
        });
    }
    let events = events(tx, company, end)?;
    Ok((docs, events))
}

pub(super) fn settlements(events: &[Event]) -> HashMap<&str, Money> {
    let mut settled = HashMap::new();
    for event in events {
        *settled.entry(event.doc.as_str()).or_default() += event.amount;
    }
    settled
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credit_void_and_settlement_are_effective_independently() {
        let date = "2028-02-29".parse().unwrap();
        let mut memo = Receivable {
            id: String::new(),
            identity: String::new(),
            number: String::new(),
            customer_id: String::new(),
            customer: String::new(),
            date,
            due: date,
            principal: Money::from_db(-4000),
            journal: String::new(),
            reversal: None,
            credit: true,
        };
        let applied = Money::from_db(3500);
        assert_eq!(memo.outstanding(date, applied), Money::from_db(-500));
        memo.reversal = Some((date, String::new()));
        assert_eq!(
            memo.outstanding(date.pred_opt().unwrap(), applied),
            Money::from_db(-500)
        );
        assert_eq!(memo.outstanding(date, applied), applied);
        assert_eq!(memo.outstanding(date, Money::ZERO), Money::ZERO);
    }
}
