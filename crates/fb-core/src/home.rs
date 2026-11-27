//! Company Home: what needs attention and how the money stands, read in one snapshot from the same queries
//! that drive the screens Home links to (Reminders and the invoice and bill lists, the Bank accounts list,
//! the profit and loss report, Review transactions and the setup checklist). Every read is bounded by the
//! company's open documents, its bank lines or one month of lines; nothing here computes a balance per
//! invoice or loads the whole ledger.
use crate::{
    CompanyId, FbError, FbResult, UserId, bankfeeds, banking, first_run, reminders,
    reports::{financial, params},
    services,
};
use chrono::NaiveDate;
use fb_db::rusqlite::{OptionalExtension, params};
use fb_db::{Book, Query};
use fb_money::Money;
use serde_json::{Value, json};

/// How many documents Recent activity lists.
pub const RECENT_LIMIT: usize = 10;
/// Audit events read, newest first, to find that many distinct documents.
const RECENT_SCAN: i64 = 400;

fn money(m: Money) -> String {
    m.to_string()
}

/// Every Home figure for `co` as of `today` (the host's display date). Company access is checked here;
/// role-limited parts (backups, bank feeds) are the host's to show or hide.
pub fn summary(book: &mut Book, actor: UserId, co: CompanyId, today: &str) -> FbResult<Value> {
    let today = fb_db::dates::parse_iso_date(today)
        .and_then(|d| NaiveDate::parse_from_str(&d, "%Y-%m-%d").ok())
        .ok_or_else(|| FbError::validation("Enter a valid date."))?;
    book.read(|q| {
        services::access(q, actor, co)?;
        let open = reminders::home_totals(q, co, today)?;
        let banks = banking::register_balances(q, co)?
            .into_iter()
            .filter(|(a, balance)| a.active || *balance != 0)
            .map(|(a, balance)| {
                json!({"id": a.id.to_db(), "code": a.code, "name": a.name, "active": a.active,
                       "card": a.section_balance() == crate::model::Side::Credit,
                       "balance": money(Money::from_db(balance))})
            })
            .collect::<Vec<_>>();
        let (start, end) = params::preset_range("this_month", today, 1)?;
        let pl = financial::accrual_period(q, co, start, end)?;
        let section = |key: &str| pl.section(key).map(|s| s.total).unwrap_or(Money::ZERO);
        let income = section("income") + section("other_income");
        let expenses = section("cogs") + section("expense") + section("other_expense");
        let count = |sql: &str| -> FbResult<i64> { Ok(q.query_row(sql, [co.to_db()], |r| r.get(0))?) };
        Ok(json!({
            "today": today.to_string(),
            "receivables": open["receivables"],
            "payables": open["payables"],
            "banks": banks,
            "month": {"start": start.to_string(), "end": end.to_string(), "income": money(income),
                      "expenses": money(expenses), "net": money(pl.net_income)},
            "drafts": {
                "invoices": count("SELECT count(*) FROM sales_invoice i JOIN sales_invoicerevision r ON r.invoice_id=i.id AND r.number=i.revision WHERE i.company_id=?1 AND r.state='DRAFT'")?,
                "bills": count("SELECT count(*) FROM payables_bill b JOIN payables_billrevision r ON r.bill_id=b.id AND r.number=b.revision WHERE b.company_id=?1 AND r.state='DRAFT'")?,
            },
            "to_review": bankfeeds::review_count(q, co)?,
            "reconnect": bankfeeds::reconnect_labels(q, co)?,
            "setup": first_run::checklist(q, co)?,
            "posted": count("SELECT EXISTS(SELECT 1 FROM ledger_journal WHERE company_id=?1 AND status='POSTED')")? == 1,
            "recent": recent(q, co, RECENT_LIMIT)?,
        }))
    })
}

/// The document families whose audit targets Recent activity lists, with the SQL that describes one by id:
/// (display kind, number, party, amount cents, state). Voided payments, deposits and the like report VOID.
const FAMILIES: &[(&str, &str)] = &[
    ("salestaxpayment", "SELECT 'STP',di.number,t.agency_name,t.tax_amount+t.penalty_amount,CASE WHEN EXISTS(SELECT 1 FROM salestax_paymentvoid v WHERE v.payment_id=t.id) THEN 'VOID' ELSE 'OPEN' END FROM salestax_payment t JOIN documents_displayidentity di ON di.id=t.identity_id WHERE t.id=?1 AND t.company_id=?2"),
    ("invoice", "SELECT 'INV',di.number,coalesce(json_extract(r.data,'$.customer_snapshot.display_name'),''),r.total,r.state FROM sales_invoice i JOIN sales_invoicerevision r ON r.invoice_id=i.id AND r.number=i.revision JOIN documents_displayidentity di ON di.id=i.identity_id WHERE i.id=?1 AND i.company_id=?2"),
    ("estimate", "SELECT 'EST',di.number,coalesce(json_extract(r.data,'$.customer_snapshot.display_name'),c.display_name,''),r.total,r.state FROM sales_estimate e JOIN sales_estimaterevision r ON r.estimate_id=e.id AND r.number=e.revision JOIN documents_displayidentity di ON di.id=e.identity_id LEFT JOIN directory_customer c ON c.id=r.customer_id WHERE e.id=?1 AND e.company_id=?2"),
    ("receipt", "SELECT r.kind,di.number,coalesce(json_extract(r.data,'$.customer_snapshot.display_name'),''),r.amount,CASE WHEN EXISTS(SELECT 1 FROM sales_receiptreversal z WHERE z.receipt_id=r.id) THEN 'VOID' ELSE 'OPEN' END FROM sales_receipt r JOIN documents_displayidentity di ON di.id=r.identity_id WHERE r.id=?1 AND r.company_id=?2"),
    ("bill", "SELECT 'BILL',di.number,coalesce(json_extract(r.vendor_snapshot,'$.display_name'),''),r.total,r.state FROM payables_bill b JOIN payables_billrevision r ON r.bill_id=b.id AND r.number=b.revision JOIN documents_displayidentity di ON di.id=b.identity_id WHERE b.id=?1 AND b.company_id=?2"),
    ("billpayment", "SELECT 'BPAY',di.number,coalesce(json_extract(p.vendor_snapshot,'$.display_name'),''),p.amount,CASE WHEN EXISTS(SELECT 1 FROM payables_paymentreversal z WHERE z.payment_id=p.id) THEN 'VOID' ELSE 'OPEN' END FROM payables_billpayment p JOIN documents_displayidentity di ON di.id=p.identity_id WHERE p.id=?1 AND p.company_id=?2"),
    ("check", "SELECT 'CHK',CASE WHEN r.check_number<>'' THEN r.check_number ELSE di.number END,r.payee_name,r.total,c.state FROM checks_check c JOIN checks_checkrevision r ON r.document_id=c.id AND r.number=c.revision JOIN documents_displayidentity di ON di.id=c.identity_id WHERE c.id=?1 AND c.company_id=?2"),
    ("deposit", "SELECT 'DEP',di.number,a.name,d.amount,CASE WHEN EXISTS(SELECT 1 FROM banking_depositreversal z WHERE z.deposit_id=d.id) THEN 'VOID' ELSE 'OPEN' END FROM banking_deposit d JOIN documents_displayidentity di ON di.id=d.identity_id JOIN ledger_account a ON a.id=d.bank_id WHERE d.id=?1 AND d.company_id=?2"),
    ("transfer", "SELECT 'TRF',di.number,f.name||' to '||o.name,t.amount,CASE WHEN EXISTS(SELECT 1 FROM transfers_transfervoid z WHERE z.transfer_id=t.id) THEN 'VOID' ELSE 'OPEN' END FROM transfers_transfer t JOIN documents_displayidentity di ON di.id=t.identity_id JOIN ledger_account f ON f.id=t.from_account_id JOIN ledger_account o ON o.id=t.to_account_id WHERE t.id=?1 AND t.company_id=?2"),
    ("creditmemo", "SELECT 'CM',di.number,coalesce(json_extract(m.data,'$.customer_snapshot.display_name'),''),m.total,CASE WHEN EXISTS(SELECT 1 FROM credits_creditmemovoid z WHERE z.credit_memo_id=m.id) THEN 'VOID' ELSE 'OPEN' END FROM credits_creditmemo m JOIN documents_displayidentity di ON di.id=m.identity_id WHERE m.id=?1 AND m.company_id=?2"),
    ("creditrefund", "SELECT 'RFD',di.number,coalesce(json_extract(f.data,'$.customer_snapshot.display_name'),''),f.amount,CASE WHEN EXISTS(SELECT 1 FROM credits_creditrefundvoid z WHERE z.refund_id=f.id) THEN 'VOID' ELSE 'OPEN' END FROM credits_creditrefund f JOIN documents_displayidentity di ON di.id=f.identity_id WHERE f.id=?1 AND f.company_id=?2"),
    ("depositrefund", "SELECT 'CDR',di.number,coalesce(json_extract(f.data,'$.customer_snapshot.display_name'),''),f.amount,CASE WHEN EXISTS(SELECT 1 FROM prepayments_depositrefundvoid z WHERE z.refund_id=f.id) THEN 'VOID' ELSE 'OPEN' END FROM prepayments_depositrefund f JOIN documents_displayidentity di ON di.id=f.identity_id WHERE f.id=?1 AND f.company_id=?2"),
];

/// The last `limit` documents of the company that someone saved, posted, recorded or voided, newest first:
/// the latest audit event of each, described from the document as it stands now. A draft deleted since is
/// skipped. One indexed read of at most `RECENT_SCAN` events, then one primary-key read per document listed.
pub fn recent(q: &(impl Query + ?Sized), co: CompanyId, limit: usize) -> FbResult<Vec<Value>> {
    let families = FAMILIES.iter().map(|(m, _)| format!("'{m}'")).collect::<Vec<_>>().join(",");
    let mut st = q.prepare(&format!(
        "SELECT target,actor_id,actor_name,occurred_at FROM ledger_auditevent
         WHERE company_id=?1 AND instr(target,':')>1 AND substr(target,1,instr(target,':')-1) IN ({families})
         ORDER BY occurred_at DESC,id DESC LIMIT ?2"
    ))?;
    let events = st
        .query_map(params![co.to_db(), RECENT_SCAN], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for (target, actor_id, actor_name, occurred_at) in events {
        if out.len() >= limit {
            break;
        }
        if !seen.insert(target.clone()) {
            continue;
        }
        let Some((model, id)) = target.split_once(':') else { continue };
        let Some((_, sql)) = FAMILIES.iter().find(|(m, _)| *m == model) else { continue };
        let found = q
            .query_row(sql, params![id, co.to_db()], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?, r.get::<_, String>(4)?))
            })
            .optional()?;
        let Some((kind, number, party, amount, state)) = found else { continue };
        let by = q
            .query_row("SELECT username FROM fb_user WHERE id=?1", [&actor_id], |r| r.get::<_, String>(0))
            .optional()?
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| if actor_name.trim().is_empty() { "Unknown user".into() } else { actor_name.trim().into() });
        out.push(json!({"kind": kind, "document_id": id, "number": number, "party": party,
                        "amount": money(Money::from_db(amount)), "state": state, "occurred_at": occurred_at, "by": by}));
    }
    Ok(out)
}
