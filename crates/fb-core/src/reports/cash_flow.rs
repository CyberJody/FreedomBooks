//! Direct-method cash rollforward from posted bank-account movements.

use super::{check_range, check_rows, financial::date_of};
use crate::error::{FbError, FbResult};
use crate::model::DetailType;
use crate::services::access;
use chrono::NaiveDate;
use fb_db::{Book, rusqlite::params};
use fb_ids::{CompanyId, UserId};
use fb_money::Money;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Activity {
    Operating,
    Investing,
    Financing,
    Unclassified,
}

impl Activity {
    pub const ALL: [Self; 4] = [
        Self::Operating,
        Self::Investing,
        Self::Financing,
        Self::Unclassified,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Operating => "Operating activities",
            Self::Investing => "Investing activities",
            Self::Financing => "Financing activities",
            Self::Unclassified => "Unclassified cash movements",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CashMovement {
    pub date: NaiveDate,
    pub journal: String,
    pub number: i64,
    pub memo: String,
    pub activity: Activity,
    pub category: &'static str,
    pub amount: Money,
}

#[derive(Debug, Default)]
pub struct CashFlow {
    pub opening: Money,
    pub closing: Money,
    pub net: [Money; 4],
    pub rows: Vec<CashMovement>,
}

type Category = (Activity, &'static str);
const UNCLASSIFIED: Category = (Activity::Unclassified, "Review counterparties");

fn counterpart(
    detail: DetailType,
    role: Option<i64>,
    source: Option<&str>,
    cash: Money,
) -> Category {
    use Activity::*;
    use DetailType::*;
    match (detail, role, source) {
        (Receivable, _, Some("RFD")) | (_, Some(0), Some("RFD")) => (Operating, "Customer refunds"),
        (Receivable, _, _) | (_, Some(0), _) => (Operating, "Customer cash receipts"),
        (Payable, _, _) | (_, Some(1), _) => (Operating, "Supplier payments"),
        (_, Some(2), _) => (Operating, "Customer receipts deposited"),
        (_, Some(3), Some("SR")) => (Operating, "Customer cash receipts"),
        (_, Some(3), _) => (Operating, "Sales tax payments"),
        (_, Some(4), _) => (Operating, "Customer deposits and refunds"),
        (CreditCard, _, _) if cash < Money::ZERO => (Operating, "Credit card payments"),
        (CreditCard, _, _) => (Financing, "Cash from credit card"),
        (FixedAsset | OtherAsset, _, _) => (Investing, "Long-term asset purchases and disposals"),
        (LongTermLiability, _, _) => (Financing, "Debt proceeds and principal repayments"),
        (Equity, _, _) => (Financing, "Equity contributions and draws"),
        (Income | OtherIncome, _, Some("SR")) => (Operating, "Customer cash receipts"),
        (Income | OtherIncome, _, _) => (Operating, "Operating income"),
        (CostOfGoodsSold | Expense | OtherExpense, _, _) => (Operating, "Operating expenses"),
        _ => UNCLASSIFIED,
    }
}

#[derive(Debug)]
struct Journal {
    movement: CashMovement,
    source: Option<String>,
    reversal: bool,
    counters: Vec<(DetailType, Option<i64>, Money)>,
    deposit: Option<DepositCash>,
}

#[derive(Debug, Default)]
struct DepositCash {
    receipts: Money,
    fees: Money,
    unclassified: Vec<Money>,
}

impl Journal {
    fn append(self, report: &mut CashFlow) -> FbResult<()> {
        let Self {
            mut movement,
            source,
            reversal,
            counters,
            deposit,
        } = self;
        if let Some(deposit) = deposit {
            let unclassified_sum: Money = deposit.unclassified.iter().copied().sum();
            let residual = movement.amount - deposit.receipts - deposit.fees - unclassified_sum;
            let mut entries = vec![
                (
                    deposit.receipts,
                    (Activity::Operating, "Customer receipts deposited"),
                ),
                (
                    deposit.fees,
                    (Activity::Operating, "Processing and bank fees"),
                ),
            ];
            for amount in deposit.unclassified {
                entries.push((amount, UNCLASSIFIED));
            }
            entries.push((residual, UNCLASSIFIED));
            for (amount, (activity, category)) in entries {
                if !amount.is_zero() {
                    report.net[activity as usize] += amount;
                    report.rows.push(CashMovement {
                        amount,
                        activity,
                        category,
                        ..movement.clone()
                    });
                }
            }
            return check_rows(report.rows.len());
        }
        let original_cash = if reversal {
            -movement.amount
        } else {
            movement.amount
        };
        if movement.amount.is_zero() && counters.is_empty() {
            return Ok(()); // Internal bank transfers offset within the cash account set.
        }
        let mut category = movement.amount.is_zero().then_some(UNCLASSIFIED);
        for (detail, role, amount) in counters {
            let candidate = counterpart(detail, role, source.as_deref(), original_cash);
            // Opposing counterparties can conceal both a cash leg and a noncash
            // leg. A single classification requires evidence for the whole net.
            if amount.cents().signum() == movement.amount.cents().signum()
                || candidate == UNCLASSIFIED
            {
                category = Some(UNCLASSIFIED);
                break;
            }
            category = match category {
                None => Some(candidate),
                Some(previous) if previous == candidate => Some(previous),
                Some(previous) if previous.0 == candidate.0 => {
                    Some((previous.0, "Other movements in this activity"))
                }
                _ => Some(UNCLASSIFIED),
            };
            if category == Some(UNCLASSIFIED) {
                break;
            }
        }
        (movement.activity, movement.category) = category.unwrap_or(UNCLASSIFIED);
        report.net[movement.activity as usize] += movement.amount;
        report.rows.push(movement);
        check_rows(report.rows.len())
    }
}

pub fn cash_flow(
    book: &mut Book,
    actor: UserId,
    company: CompanyId,
    start: NaiveDate,
    end: NaiveDate,
) -> FbResult<CashFlow> {
    book.read(|tx| {
        access(tx, actor, company)?;
        check_range(start, end)?;
        // Resolve one account set for the entire rollforward. Preserve legacy
        // banks using retained setup and banking evidence, with control roles
        // taking precedence over an account's detail type or bank history.
        let mut stmt = tx.prepare(
            "WITH setup_history AS (
                SELECT snapshot.value AS setup
                FROM ledger_auditevent e, json_each(e.detail) snapshot
                WHERE e.company_id=?1 AND e.action='accounting.setup'
                  AND snapshot.key IN ('before','after') AND snapshot.type='object'
             ), control_evidence(account_id,priority) AS (
                SELECT account_id,priority FROM fb_control_accounts WHERE company_id=?1
                UNION ALL
                SELECT role.value, CASE role.key
                    WHEN 'receivables' THEN 0 WHEN 'payables' THEN 1
                    WHEN 'undeposited' THEN 2 WHEN 'collected_tax' THEN 3 ELSE 4 END
                FROM setup_history h, json_each(h.setup) role
                WHERE role.key IN ('receivables','payables','undeposited','collected_tax','customer_deposits')
                  AND role.type='text'
             ), roles AS (
                SELECT account_id,min(priority) priority FROM control_evidence GROUP BY account_id
             ), bank_history(account_id) AS (
                SELECT bank.value FROM setup_history h, json_each(h.setup,'$.banks') bank
                WHERE bank.type='text'
                UNION
                SELECT da.account_id FROM documents_documentaccount da
                JOIN documents_documentidentity s ON s.id=da.source_id AND s.company_id=?1
                WHERE (da.role='bank' OR (s.kind='TRF' AND da.role IN ('from','to')))
                  AND EXISTS (SELECT 1 FROM ledger_journal j
                              WHERE j.source_id=s.id AND j.company_id=?1 AND j.status='POSTED'
                                AND j.reversal_of_id IS NULL)
                UNION
                SELECT r.destination_account_id FROM sales_receipt r
                JOIN ledger_journal j ON j.id=r.journal_id AND j.company_id=?1 AND j.status='POSTED'
                WHERE r.company_id=?1 AND r.destination='BANK'
                UNION
                SELECT r.account_id FROM banking_reconciliation r
                WHERE r.company_id=?1 AND r.status IN ('DONE','UNDONE')
                UNION
                SELECT f.ledger_account_id FROM bankfeeds_account f
                JOIN bankfeeds_connection c ON c.id=f.connection_id
                WHERE c.company_id=?1 AND f.ledger_account_id IS NOT NULL
                UNION
                SELECT mapping.value FROM ledger_auditevent e, json_each(e.detail) mapping
                WHERE e.company_id=?1 AND e.action='bankfeed.mapped'
                  AND mapping.key IN ('before','after') AND mapping.type='text'
             ), cash AS MATERIALIZED (
                SELECT id FROM banking_register_account a
                WHERE a.company_id=?1 AND a.kind='ASSET'
                  AND NOT EXISTS (SELECT 1 FROM roles r WHERE r.account_id=a.id)
                UNION
                SELECT a.id FROM bank_history h JOIN ledger_account a ON a.id=h.account_id
                WHERE a.company_id=?1 AND a.kind='ASSET' AND a.detail_type='OTHER_CURRENT_ASSET'
                  AND NOT EXISTS (SELECT 1 FROM roles r WHERE r.account_id=a.id)
             )
             SELECT j.id,j.number,j.date,j.memo,s.kind,l.debit,l.credit,c.id IS NOT NULL,
                    a.detail_type,r.priority,j.reversal_of_id IS NOT NULL,
                    d.bank_id IN (SELECT id FROM cash),
                    CASE WHEN EXISTS (
                        SELECT 1 FROM banking_depositline p
                        WHERE p.deposit_id=d.id AND p.undeposited_id=l.account_id
                    ) THEN 1 WHEN EXISTS (
                        SELECT 1 FROM json_each(d.data,'$.adjustments') fee
                        WHERE json_extract(fee.value,'$.account_id')=l.account_id
                    ) THEN 2 ELSE 0 END
             FROM ledger_journal j JOIN ledger_journalline l ON l.journal_id=j.id
             JOIN ledger_account a ON a.id=l.account_id AND a.company_id=?1
             LEFT JOIN cash c ON c.id=l.account_id
             LEFT JOIN roles r ON r.account_id=l.account_id
             LEFT JOIN documents_documentidentity s ON s.id=j.source_id AND s.company_id=?1
             LEFT JOIN banking_deposit d ON d.journal_id=coalesce(j.reversal_of_id,j.id)
                AND d.company_id=?1 AND j.date>=?2
             WHERE j.company_id=?1 AND j.status='POSTED' AND j.date<=?3 AND
                 (c.id IS NOT NULL OR (j.date>=?2 AND EXISTS (
                     SELECT 1 FROM ledger_journalline k JOIN cash b ON b.id=k.account_id WHERE k.journal_id=j.id)))
             ORDER BY j.date,j.number,l.position,l.id",
        )?;
        let mut rows = stmt.query(params![company.to_db(), start.to_string(), end.to_string()])?;
        let mut report = CashFlow::default();
        let mut pending: Option<Journal> = None;
        while let Some(row) = rows.next()? {
            let date = date_of(&row.get::<_, String>(2)?)?;
            let amount = Money::from_db(row.get(5)?) - Money::from_db(row.get(6)?);
            let is_cash: bool = row.get(7)?;
            if is_cash {
                report.closing += amount;
            }
            if date < start {
                if is_cash {
                    report.opening += amount;
                }
                continue;
            }
            let number = row.get(1)?;
            if pending.as_ref().is_some_and(|p| p.movement.number != number) {
                pending.take().expect("journal is pending").append(&mut report)?;
            }
            if pending.is_none() {
                let deposit_bank: Option<bool> = row.get(11)?;
                pending = Some(Journal {
                    movement: CashMovement { date, journal: row.get(0)?, number, memo: row.get(3)?,
                        activity: Activity::Unclassified, category: "", amount: Money::ZERO },
                    source: row.get(4)?,
                    reversal: row.get(10)?,
                    counters: Vec::new(),
                    deposit: (deposit_bank == Some(true)).then(DepositCash::default),
                });
            }
            let journal = pending.as_mut().expect("journal was added");
            if is_cash {
                journal.movement.amount += amount;
            } else if !amount.is_zero() {
                let detail = DetailType::parse(&row.get::<_, String>(8)?)
                    .ok_or_else(|| FbError::validation("Invalid stored account type."))?;
                if let Some(deposit) = &mut journal.deposit {
                    match row.get::<_, i64>(12)? {
                        1 => deposit.receipts -= amount,
                        2 if matches!(detail, DetailType::Expense | DetailType::OtherExpense)
                            && (amount > Money::ZERO) != journal.reversal =>
                        {
                            deposit.fees -= amount;
                        }
                        _ => deposit.unclassified.push(-amount), // Keep each unmatched adjustment as a signed unclassified row.
                    }
                } else {
                    journal.counters.push((detail, row.get(9)?, amount));
                }
            }
        }
        if let Some(journal) = pending {
            journal.append(&mut report)?;
        }
        if report.opening + report.net.into_iter().sum::<Money>() != report.closing {
            return Err(FbError::validation("Cash flow does not reconcile to posted bank balances."));
        }
        report.rows.sort_by_key(|r| (r.activity, r.date, r.number));
        Ok(report)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn historical_control_accounts_classify_by_role_instead_of_account_name() {
        use Activity::Operating;
        use DetailType::{OtherCurrentAsset, OtherCurrentLiability};
        let inflow = Money::from_db(100);
        assert_eq!(
            counterpart(OtherCurrentAsset, Some(0), Some("RCP"), inflow),
            (Operating, "Customer cash receipts")
        );
        assert_eq!(
            counterpart(OtherCurrentLiability, Some(1), Some("BPAY"), -inflow),
            (Operating, "Supplier payments")
        );
        assert_eq!(
            counterpart(OtherCurrentAsset, Some(2), Some("DEP"), inflow),
            (Operating, "Customer receipts deposited")
        );
        assert_eq!(
            counterpart(OtherCurrentLiability, Some(3), Some("STP"), -inflow),
            (Operating, "Sales tax payments")
        );
        assert_eq!(
            counterpart(OtherCurrentLiability, Some(4), Some("CDP"), inflow),
            (Operating, "Customer deposits and refunds")
        );
        assert_eq!(
            counterpart(OtherCurrentLiability, None, None, inflow),
            UNCLASSIFIED
        );
    }
}
