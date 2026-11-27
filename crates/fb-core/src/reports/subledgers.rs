//! Document aging and vendor payments from the posted company subledgers.

use super::financial::{date_of, events};
use super::receivables;
use crate::error::{FbError, FbResult};
use crate::services::access;
use chrono::NaiveDate;
use fb_db::Book;
use fb_db::rusqlite::params;
use fb_ids::{CompanyId, UserId};
use fb_money::Money;
use std::collections::HashMap;

pub use super::receivables::ReceivableBalance;

pub const AGING_BUCKETS: [&str; 5] = ["Current", "1–30", "31–60", "61–90", "90+"];

fn bucket(days: i64) -> usize {
    match days {
        ..=0 => 0,
        1..=30 => 1,
        31..=60 => 2,
        61..=90 => 3,
        _ => 4,
    }
}

#[derive(Debug, Default)]
pub struct AgingAmounts {
    pub buckets: [Money; 5],
    pub total: Money,
    pub receivables: ReceivableBalance,
}

impl AgingAmounts {
    fn add(&mut self, bucket: usize, doc: &OpenDocument, receivables: bool) {
        if receivables {
            self.receivables.add(doc.balance, doc.credit_memo);
        }
        if !receivables || !ReceivableBalance::is_credit(doc.balance, doc.credit_memo) {
            self.buckets[bucket] += doc.balance;
        }
        self.total += doc.balance;
    }
}

#[derive(Debug)]
pub struct OpenDocument {
    pub id: String,
    pub number: String,
    pub due_date: NaiveDate,
    pub balance: Money,
    pub credit_memo: bool,
}

#[derive(Debug, Default)]
pub struct AgingParty {
    pub id: String,
    pub name: String,
    pub amounts: AgingAmounts,
    pub documents: Vec<OpenDocument>,
}

#[derive(Debug, Default)]
pub struct AgingSummary {
    pub parties: Vec<AgingParty>,
    pub amounts: AgingAmounts,
}

impl AgingSummary {
    fn add(
        &mut self,
        party_id: String,
        name: String,
        bucket: usize,
        document: OpenDocument,
        receivables: bool,
    ) {
        if document.balance.is_zero() {
            return;
        }
        if self.parties.last().is_none_or(|party| party.id != party_id) {
            self.parties.push(AgingParty { id: party_id, name, ..Default::default() });
        }
        let party = self.parties.last_mut().expect("party was added");
        party.amounts.add(bucket, &document, receivables);
        self.amounts.add(bucket, &document, receivables);
        party.documents.push(document);
    }
}

/// A posted invoice contributes its principal until its posted void date (zero from
/// the void date onward). Settlement events (receipts, credit applications, deposits,
/// and their reversals/unapplications) apply on their effective dates, so a void that
/// predates a settlement reversal still leaves the unsettled credit balance visible
/// until the reversal takes effect. The age buckets show open invoices, with
/// unapplied credits reported separately and included in Net A/R.
pub fn ar_aging(
    book: &mut Book,
    actor: UserId,
    company: CompanyId,
    as_of: NaiveDate,
) -> FbResult<AgingSummary> {
    book.read(|tx| {
        access(tx, actor, company)?;
        let (documents, events) = receivables::load(tx, company, as_of, None)?;
        let settled = receivables::settlements(&events);
        let mut report = AgingSummary::default();
        for d in documents {
            let balance = d.outstanding(
                as_of,
                settled
                    .get(d.identity.as_str())
                    .copied()
                    .unwrap_or_default(),
            );
            let days = (as_of - d.due).num_days();
            report.add(
                d.customer_id,
                d.customer,
                bucket(days),
                OpenDocument {
                    id: d.id,
                    number: d.number,
                    due_date: d.due,
                    balance,
                    credit_memo: d.credit,
                },
                true,
            );
        }
        Ok(report)
    })
}

/// Bill payments are the payable settlements supported by the current book format.
/// A posted bill's principal drops to zero on its posted void date while any
/// unreversed settlement residual remains visible until its reversal date.
pub fn ap_aging(
    book: &mut Book,
    actor: UserId,
    company: CompanyId,
    as_of: NaiveDate,
) -> FbResult<AgingSummary> {
    aging(
        book,
        actor,
        company,
        as_of,
        "SELECT b.id,b.identity_id,d.number,c.id,c.display_name,r.due_date,
                CASE WHEN EXISTS (SELECT 1 FROM payables_billrevision v
                  JOIN ledger_journal z ON z.id=v.reversal_id AND z.company_id=b.company_id
                  WHERE v.bill_id=b.id AND v.state='VOID' AND z.status='POSTED' AND z.date<=?2)
                THEN 0 ELSE r.total END
         FROM payables_bill b
         JOIN payables_billrevision r ON r.bill_id=b.id AND r.state='OPEN'
         JOIN documents_documentidentity d ON d.id=b.identity_id AND d.company_id=b.company_id
         JOIN directory_vendor c ON c.id=r.vendor_id AND c.company_id=b.company_id
         JOIN ledger_journal j ON j.id=r.journal_id AND j.company_id=b.company_id
         WHERE b.company_id=?1 AND j.status='POSTED' AND r.date<=?2
         ORDER BY c.display_name COLLATE fb_text,c.id,r.due_date,d.number,b.id",
    )
}

fn aging(
    book: &mut Book,
    actor: UserId,
    company: CompanyId,
    as_of: NaiveDate,
    sql: &str,
) -> FbResult<AgingSummary> {
    book.read(|tx| {
        access(tx, actor, company)?;
        let mut settled = HashMap::<String, Money>::new();
        for event in events(tx, company, as_of)? {
            *settled.entry(event.doc).or_default() += event.amount;
        }
        let mut stmt = tx.prepare(sql)?;
        let rows = stmt.query_map(params![company.to_db(), as_of.to_string()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, i64>(6)?,
            ))
        })?;
        let mut report = AgingSummary::default();
        for row in rows {
            let (id, identity, number, party_id, name, due_date, total) = row?;
            let balance =
                Money::from_db(total) - settled.get(&identity).copied().unwrap_or_default();
            let due_date = date_of(&due_date)?;
            let bucket = bucket((as_of - due_date).num_days());
            report.add(
                party_id,
                name,
                bucket,
                OpenDocument {
                    id,
                    number,
                    due_date,
                    balance,
                    credit_memo: false,
                },
                false,
            );
        }
        Ok(report)
    })
}

#[derive(Debug)]
pub struct VendorPayments {
    pub id: String,
    pub name: String,
    pub address: String,
    pub count: i64,
    pub total: Money,
}

#[derive(Debug, Default)]
pub struct Vendor1099Summary {
    pub vendors: Vec<VendorPayments>,
    pub count: i64,
    pub total: Money,
}

/// Effective-dated payment activity for 1099 review. Each posted bill payment or
/// vendor check/expense counts once on its payment date (+1, +amount), and each
/// posted payment reversal or check void offsets it on the reversal/void's effective
/// date (-1, -amount). This keeps closed-year reports stable when a payment is
/// reversed in a later year while netting same-period cancellations to zero.
pub fn vendor_1099(
    book: &mut Book,
    actor: UserId,
    company: CompanyId,
    start: NaiveDate,
    end: NaiveDate,
) -> FbResult<Vendor1099Summary> {
    book.read(|tx| {
        access(tx, actor, company)?;
        if start > end {
            return Err(FbError::validation(
                "The \"From\" date must not be after the \"To\" date.",
            ));
        }
        let mut stmt = tx.prepare(
            "WITH payments(vendor_id,amount,sign) AS (
               SELECT p.vendor_id,p.amount,1
               FROM payables_billpayment p
               JOIN ledger_journal j ON j.id=p.journal_id AND j.company_id=p.company_id
               WHERE p.company_id=?1 AND j.status='POSTED' AND p.date BETWEEN ?2 AND ?3
               UNION ALL
               SELECT p.vendor_id,p.amount,-1
               FROM payables_paymentreversal z
               JOIN payables_billpayment p ON p.id=z.payment_id AND p.company_id=?1
               JOIN ledger_journal j ON j.id=z.journal_id AND j.company_id=?1
               WHERE j.status='POSTED' AND j.date BETWEEN ?2 AND ?3
               UNION ALL
               SELECT r.vendor_id,r.total,1
               FROM checks_check c
               JOIN checks_checkrevision r ON r.document_id=c.id AND r.state='RECORDED' AND r.action='recorded'
               JOIN ledger_journal j ON j.id=r.journal_id AND j.company_id=c.company_id
               WHERE c.company_id=?1 AND r.payee_kind='VENDOR'
                 AND j.status='POSTED' AND r.date BETWEEN ?2 AND ?3
               UNION ALL
               SELECT v.vendor_id,v.total,-1
               FROM checks_check c
               JOIN checks_checkrevision v ON v.document_id=c.id AND v.state='VOID' AND v.action='voided'
               JOIN ledger_journal z ON z.id=v.reversal_id AND z.company_id=c.company_id
               WHERE c.company_id=?1 AND v.payee_kind='VENDOR'
                 AND z.status='POSTED' AND z.date BETWEEN ?2 AND ?3
             )
             SELECT v.id,v.display_name,v.billing_address,p.amount,p.sign
             FROM payments p JOIN directory_vendor v ON v.id=p.vendor_id AND v.company_id=?1
             ORDER BY v.display_name COLLATE fb_text,v.id",
        )?;
        let mut report = Vendor1099Summary::default();
        let rows = stmt.query_map(
            params![company.to_db(), start.to_string(), end.to_string()],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                ))
            },
        )?;
        for row in rows {
            let (id, name, address, amount, sign) = row?;
            let signed_amount = Money::from_cents(i128::from(sign) * i128::from(amount));
            if report.vendors.last().is_none_or(|vendor| vendor.id != id) {
                report.vendors.push(VendorPayments {
                    id,
                    name,
                    address,
                    count: 0,
                    total: Money::ZERO,
                });
            }
            let vendor = report.vendors.last_mut().expect("just added the vendor");
            vendor.count += sign;
            vendor.total += signed_amount;
        }
        report.vendors.retain(|vendor| !vendor.total.is_zero());
        for vendor in &report.vendors {
            report.count += vendor.count;
            report.total += vendor.total;
        }
        Ok(report)
    })
}

#[cfg(test)]
mod tests {
    use super::bucket;

    #[test]
    fn aging_bucket_boundaries() {
        for (days, expected) in [
            (-1, 0),
            (0, 0),
            (1, 1),
            (30, 1),
            (31, 2),
            (60, 2),
            (61, 3),
            (90, 3),
            (91, 4),
        ] {
            assert_eq!(bucket(days), expected, "{days} days past due");
        }
    }
}
