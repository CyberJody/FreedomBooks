//! Accrual sales from posted document lines and their effective-dated reversals.

use super::{check_range, check_rows};
use crate::error::{FbError, FbResult};
use crate::{calculations, services::access};
use chrono::NaiveDate;
use fb_db::{Book, rusqlite::params};
use fb_ids::{CompanyId, UserId};
use fb_money::Money;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Customer,
    Item,
    CustomerItem,
}

impl Group {
    pub const CHOICES: [(&str, &str); 3] = [
        ("customer", "By Customer"),
        ("item", "By Item"),
        ("customer_item", "Customer & Item"),
    ];

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "" | "customer" => Some(Self::Customer),
            "item" => Some(Self::Item),
            "customer_item" => Some(Self::CustomerItem),
            _ => None,
        }
    }

    pub fn key(self) -> &'static str {
        Self::CHOICES[self as usize].0
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SalesAmounts {
    pub gross: Money,
    /// Sales discounts and credits are negative. Their reversals are positive.
    pub adjustments: Money,
    /// Signed quantity in units of 1/10,000. Discounts carry no quantity.
    pub quantity: i128,
    pub activity_lines: usize,
}

impl SalesAmounts {
    pub fn net(self) -> Money {
        self.gross + self.adjustments
    }

    fn add(&mut self, line: Self) {
        self.gross += line.gross;
        self.adjustments += line.adjustments;
        self.quantity += line.quantity;
        self.activity_lines += line.activity_lines;
    }

    pub fn quantity_text(self) -> String {
        let units = self.quantity.abs();
        let value = format!(
            "{}{}.{:04}",
            if self.quantity < 0 { "-" } else { "" },
            units / 10_000,
            units % 10_000
        );
        value.trim_end_matches('0').trim_end_matches('.').to_owned()
    }
}

#[derive(Debug, Default)]
pub struct SalesRow {
    pub customer_id: Option<String>,
    pub customer: String,
    pub item_id: Option<String>,
    pub item: String,
    pub amounts: SalesAmounts,
}

#[derive(Debug, Default)]
pub struct SalesByCustomerItem {
    pub rows: Vec<SalesRow>,
    pub totals: SalesAmounts,
    pub document_net: Money,
    pub document_events: usize,
    pub manual_income: Money,
    pub manual_income_lines: usize,
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum Item {
    All,
    Catalog(String),
    Unassigned,
    Discount,
}

pub fn sales_by_customer_item(
    book: &mut Book,
    actor: UserId,
    company: CompanyId,
    start: NaiveDate,
    end: NaiveDate,
    group: Group,
) -> FbResult<SalesByCustomerItem> {
    book.read(|tx| {
        access(tx, actor, company)?;
        check_range(start, end)?;
        let mut stmt = tx.prepare(
            "WITH documents(kind,id,customer_id,posted,reversed,original_date,total,tax,customer_snapshot) AS (
               SELECT 'INV',r.id,r.customer_id,j.id,v.reversal_id,j.date,r.total,
                      json_extract(r.data,'$.tax'),json_extract(r.data,'$.customer_snapshot.display_name')
               FROM sales_invoice i JOIN sales_invoicerevision r ON r.invoice_id=i.id AND r.state='OPEN'
               JOIN ledger_journal j ON j.id=r.journal_id AND j.company_id=i.company_id AND j.status='POSTED'
               LEFT JOIN sales_invoicerevision v ON v.invoice_id=i.id AND v.state='VOID'
               WHERE i.company_id=?1 AND j.date<=?3
               UNION ALL
               SELECT 'SR',s.id,r.customer_id,j.id,v.journal_id,j.date,s.total,
                      json_extract(s.data,'$.tax'),json_extract(r.data,'$.customer_snapshot.display_name')
               FROM sales_receipt r JOIN sales_salesreceipt s ON s.receipt_id=r.id
               JOIN ledger_journal j ON j.id=r.journal_id AND j.company_id=r.company_id AND j.status='POSTED'
               LEFT JOIN sales_receiptreversal v ON v.receipt_id=r.id
               WHERE r.company_id=?1 AND r.kind='SR' AND j.date<=?3
               UNION ALL
               SELECT 'CM',m.id,m.customer_id,j.id,v.journal_id,j.date,m.total,
                      json_extract(m.data,'$.tax'),json_extract(m.data,'$.customer_snapshot.display_name')
               FROM credits_creditmemo m
               JOIN ledger_journal j ON j.id=m.journal_id AND j.company_id=m.company_id AND j.status='POSTED'
               LEFT JOIN credits_creditmemovoid v ON v.credit_memo_id=m.id
               WHERE m.company_id=?1 AND j.date<=?3
             ), activity(journal_id,sign,kind,id,customer_id,original_date,total,tax,customer_snapshot) AS (
               SELECT posted,1,kind,id,customer_id,original_date,total,tax,customer_snapshot FROM documents WHERE original_date>=?2
               UNION ALL
               SELECT j.id,-1,d.kind,d.id,d.customer_id,d.original_date,d.total,d.tax,d.customer_snapshot
               FROM documents d JOIN ledger_journal j ON j.id=d.reversed AND j.company_id=?1 AND j.status='POSTED'
               WHERE j.date BETWEEN ?2 AND ?3
             ), lines AS (
               SELECT a.*,coalesce(i.position,s.position,m.position) position,
                      coalesce(i.item_id,s.item_id,m.item_id) item_id,
                      coalesce(i.amount,s.amount,m.amount) amount,
                      coalesce(i.data,s.data,m.data) line_data
               FROM activity a
               LEFT JOIN sales_invoiceline i ON a.kind='INV' AND i.revision_id=a.id
               LEFT JOIN sales_salesreceiptline s ON a.kind='SR' AND s.sale_id=a.id
               LEFT JOIN credits_creditmemoline m ON a.kind='CM' AND m.credit_memo_id=a.id
             )
             SELECT l.journal_id,l.sign,l.kind,l.total,CAST(l.tax AS TEXT),c.id,coalesce(l.customer_snapshot,c.display_name),c.active,
                    l.item_id,coalesce(json_extract(l.line_data,'$.item_snapshot.code'),i.code),i.active,
                    l.amount,CAST(json_extract(l.line_data,'$.quantity') AS TEXT),json_extract(l.line_data,'$.line_kind')
             FROM lines l JOIN directory_customer c ON c.id=l.customer_id AND c.company_id=?1
             LEFT JOIN directory_item i ON i.id=l.item_id AND i.company_id=?1
             ORDER BY l.original_date,l.id,l.journal_id,l.position",
        )?;
        let args = params![company.to_db(), start.to_string(), end.to_string()];
        let mut rows = stmt.query(args)?;
        let mut report = SalesByCustomerItem::default();
        let mut groups = BTreeMap::<(Option<String>, Item), SalesRow>::new();
        let mut customer_labels = BTreeMap::new();
        let mut item_labels = BTreeMap::new();
        let mut journal = String::new();
        let (mut expected, mut summed) = (Money::ZERO, Money::ZERO);
        while let Some(row) = rows.next()? {
            let journal_id: String = row.get(0)?;
            let sign: i128 = i128::from(row.get::<_, i64>(1)?);
            let credit = row.get::<_, String>(2)? == "CM";
            let direction = if credit { -sign } else { sign };
            if journal != journal_id {
                reconcile(expected, summed)?;
                let tax = row.get::<_, String>(4)?;
                let tax = Money::from_decimal_exact(calculations::unit_rate(&Value::String(tax))?)
                    .ok_or_else(|| FbError::validation("Invalid stored sales tax."))?;
                expected = Money::from_cents(direction * (Money::from_db(row.get(3)?) - tax).cents());
                summed = Money::ZERO;
                report.document_net += expected;
                report.document_events += 1;
                journal = journal_id;
            }
            let kind: Option<String> = row.get(13)?;
            if kind.as_deref() == Some("NOTE") {
                continue;
            }
            let discount = kind.as_deref() == Some("DISCOUNT");
            let amount = Money::from_cents(i128::from(row.get::<_, i64>(11)?));
            let mut line = SalesAmounts { activity_lines: 1, ..Default::default() };
            if discount || credit {
                line.adjustments = Money::from_cents(-sign * amount.cents());
            } else {
                line.gross = Money::from_cents(sign * amount.cents());
            }
            if !discount {
                line.quantity = direction * calculations::quantity(&Value::String(row.get(12)?))?.mantissa();
            }
            summed += line.net();
            report.totals.add(line);

            // Source order uses original posting date and stable identities. Every
            // occurrence of a stable customer or item displays the same latest snapshot.
            let customer = if group != Group::Item {
                let id: String = row.get(5)?;
                customer_labels.insert(id.clone(), label(row.get(6)?, row.get(7)?));
                Some(id)
            } else { None };
            let item = if group == Group::Customer {
                Item::All
            } else if let Some(id) = row.get::<_, Option<String>>(8)? {
                let name = row.get::<_, Option<String>>(9)?.unwrap_or_else(|| id.clone());
                item_labels.insert(id.clone(), label(name, row.get::<_, Option<bool>>(10)?.unwrap_or(false)));
                Item::Catalog(id)
            } else if discount {
                Item::Discount
            } else {
                Item::Unassigned
            };
            groups.entry((customer, item)).or_default().amounts.add(line);
            check_rows(groups.len())?;
        }
        reconcile(expected, summed)?;

        report.rows = groups.into_iter().map(|((customer, item), mut row)| {
            if let Some(id) = &customer {
                row.customer = customer_labels[id].clone();
            }
            row.customer_id = customer;
            match item {
                Item::All => {}
                Item::Catalog(id) => {
                    row.item = item_labels[&id].clone();
                    row.item_id = Some(id);
                }
                Item::Discount => row.item = "Unassigned discounts".into(),
                Item::Unassigned => row.item = "Unassigned item".into(),
            }
            row
        }).collect();
        report.rows.sort_by(|a, b| (&a.customer, &a.customer_id, &a.item, &a.item_id).cmp(&(&b.customer, &b.customer_id, &b.item, &b.item_id)));

        let mut stmt = tx.prepare(
            "SELECT l.credit,l.debit FROM ledger_journal j JOIN ledger_journalline l ON l.journal_id=j.id
             JOIN ledger_account a ON a.id=l.account_id AND a.company_id=?1
             WHERE j.company_id=?1 AND j.status='POSTED' AND j.source_id IS NULL
               AND j.date BETWEEN ?2 AND ?3 AND a.kind='INCOME'",
        )?;
        let mut manual = stmt.query(args)?;
        while let Some(row) = manual.next()? {
            report.manual_income += Money::from_db(row.get(0)?) - Money::from_db(row.get(1)?);
            report.manual_income_lines += 1;
        }
        Ok(report)
    })
}

fn reconcile(expected: Money, summed: Money) -> FbResult<()> {
    if expected != summed {
        return Err(FbError::validation(
            "Posted sales lines do not reconcile to the document total excluding sales tax.",
        ));
    }
    Ok(())
}

fn label(snapshot: String, active: bool) -> String {
    if active {
        snapshot
    } else {
        format!("{snapshot} (archived)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantity_is_exact_and_preserves_fractional_returns() {
        for (quantity, expected) in [
            (0, "0"),
            (100_000, "10"),
            (10_001, "1.0001"),
            (-12_340, "-1.234"),
        ] {
            let amounts = SalesAmounts {
                quantity,
                ..Default::default()
            };
            assert_eq!(amounts.quantity_text(), expected);
        }
    }
}
