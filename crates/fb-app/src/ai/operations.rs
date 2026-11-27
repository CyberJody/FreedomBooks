use crate::{App, AppError, AppResult};
use fb_core::{bankfeeds, directory, reports, services};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "UPPERCASE", deny_unknown_fields)]
pub enum FeedCategory {
    Journal {
        account_id: String,
        #[serde(default)]
        payee_name: String,
    },
    Check {
        account_id: String,
        #[serde(default)]
        payee_name: String,
        #[serde(default)]
        check_number: String,
    },
    Transfer {
        transfer_account_id: String,
    },
    Deposit {
        receipt_ids: Vec<String>,
    },
    Match {
        journal_line_id: String,
    },
}
/// The bank row and mapping the caller reviewed before attempting to record it.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct FeedPreconditions {
    id: String,
    status: String,
    return_count: i64,
    amount_cents: String,
    date: Option<String>,
    feed_account_id: String,
    mapped_account_id: Option<String>,
    #[serde(default)]
    currency: Option<String>,
    #[serde(default)]
    category_detail_type: Option<String>,
    mapped_account_detail_type: Option<String>,
    provider_host: Option<String>,
    provider_account_id: Option<String>,
    provider_transaction_id: Option<String>,
    description: Option<String>,
    #[serde(default)]
    require_exact_account_id: bool,
    #[serde(default)]
    require_exact_transaction_id: bool,
    #[serde(default)]
    require_no_candidates: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoricalInvoice {
    pub invoice: crate::commands::InvoiceValues,
    pub source: Value,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum Operation {
    ConnectionInfo,
    AuditHistory {
        #[serde(default)]
        after: usize,
        #[serde(default = "page_size")]
        limit: usize,
    },
    Directory {
        kind: crate::commands::DirectoryKind,
    },
    Invoice {
        invoice_id: String,
    },
    ReviewInvoice {
        invoice_id: String,
        expected_revision: i64,
    },
    AccountingSetup,
    TaxItems,
    TaxSettings,
    InvoiceSettings,
    Bills,
    Bill {
        bill_id: String,
    },
    Checks,
    Check {
        check_id: String,
    },
    /// Job tickets with hours and unbilled amounts. `status`: OPEN, COMPLETED, INVOICED, CLOSED, ACTIVE (open and completed) or ALL.
    JobTickets {
        #[serde(default)]
        customer_id: Option<String>,
        #[serde(default)]
        status: Option<String>,
    },
    Journal {
        journal_id: String,
    },
    TrialBalance {
        as_of: String,
    },
    ProfitAndLoss {
        start: Option<String>,
        end: String,
        basis: String,
    },
    BalanceSheet {
        as_of: String,
        basis: String,
    },
    FeedAccounts,
    FeedReview,
    PendingFeed {
        #[serde(default)]
        after: usize,
        #[serde(default = "page_size")]
        limit: usize,
    },
    DecidedFeed {
        #[serde(default)]
        after: usize,
        #[serde(default = "page_size")]
        limit: usize,
    },
    SuggestFeed {
        transaction_id: String,
    },
    AcceptFeed {
        transaction_id: String,
        category: FeedCategory,
        #[serde(default)]
        preconditions: Option<Box<FeedPreconditions>>,
    },
    IgnoreFeed {
        transaction_id: String,
    },
    ReturnFeed {
        transaction_id: String,
    },
    InvoiceImportPreview {
        invoices: Vec<HistoricalInvoice>,
    },
    InvoiceImportApply {
        preview_token: String,
    },
    ImportPreview {
        #[schemars(length(min = 1, max = 100))]
        journals: Vec<super::import::Journal>,
    },
    ImportApply {
        preview_token: String,
    },
}
fn page_size() -> usize {
    50
}
pub fn item(i: &bankfeeds::Item) -> Value {
    json!({"id":i.id,"ordinal":i.ordinal,"feed_account_id":i.feed_account_id,"provider_transaction_id":i.remote_id,"date":i.date(),"amount_cents":i.amount.to_string(),"description":i.description,"pending":i.pending,"status":i.status,"accepted_as":i.accepted_as,"journal_id":i.journal_id,"journal_line_id":i.journal_line_id,"return_count":i.return_count})
}
fn review_account(account: &bankfeeds::ReviewAccount) -> Value {
    let f = &account.feed;
    json!({"id":f.id,"provider_account_id":f.remote_id,"name":f.label(),
        "currency":account.identity.currency,"provider_host":account.identity.provider_host,
        "provider_id_exact":account.identity.provider_id_exact,"mapped_account_id":f.ledger_account_id,
        "connection_active":account.connection_active})
}
fn review_row(row: &bankfeeds::ReviewRow) -> Value {
    let mut v = item(&row.item);
    v["mapped_account_id"] = json!(row.mapped_account_id);
    v["provider_host"] = json!(row.identity.provider_host);
    v["provider_id_exact"] = json!(row.identity.provider_id_exact);
    v["currency"] = json!(row.identity.currency);
    if let Some(decision) = &row.decision {
        v["decision"] = json!(decision);
    }
    if let Some(candidates) = &row.candidates {
        v["candidates"] = json!(candidates);
    }
    v
}
pub fn is_write(name: &str) -> bool {
    matches!(
        name,
        "AcceptFeed" | "IgnoreFeed" | "ReturnFeed" | "ImportApply" | "InvoiceImportApply"
    )
}
fn date(s: &str) -> AppResult<chrono::NaiveDate> {
    fb_core::py::form_date(s).map_err(AppError::Invalid)
}
fn basis(s: &str) -> AppResult<reports::financial::Basis> {
    reports::financial::Basis::parse(s)
        .ok_or_else(|| AppError::Invalid("Choose CASH or ACCRUAL basis.".into()))
}

pub fn execute(app: &mut App, co: fb_core::CompanyId, operation: Operation) -> AppResult<Value> {
    if let Operation::ReviewInvoice {
        invoice_id,
        expected_revision,
    } = operation
    {
        let open = app.open_mut()?;
        let review = fb_core::sales::review_invoice(
            &mut open.book,
            open.owner,
            co,
            &invoice_id,
            Some(expected_revision),
        )?;
        let display = review.display.clone();
        if app.sales_reviews.len() >= 1024 {
            app.sales_reviews.clear();
        }
        let token = uuid::Uuid::new_v4().to_string();
        app.sales_reviews.insert(token.clone(), review);
        return Ok(json!({"review":display,"token":token}));
    }
    let open = app.open_mut()?;
    let actor = open.owner;
    let book = &mut open.book;
    Ok(match operation {
        Operation::Directory { kind } => book
            .read(|tx| {
                services::access(tx, actor, co)?;
                let kind = match kind {
                    crate::commands::DirectoryKind::Customer => directory::Model::Customer,
                    crate::commands::DirectoryKind::Vendor => directory::Model::Vendor,
                    crate::commands::DirectoryKind::Item => directory::Model::Item,
                };
                directory::list(tx, co, kind)
            })
            .map(Value::Array)?,
        Operation::AuditHistory { after, limit } => {
            if !(1..=100).contains(&limit) || after > i64::MAX as usize {
                return Err(AppError::Invalid("Invalid audit page.".into()));
            }
            let rows=book.read(|tx| -> fb_core::FbResult<Vec<Value>> {
                services::access(tx,actor,co)?;
                let mut st=tx.prepare("SELECT id,actor_id,actor_name,occurred_at,action,target,detail FROM ledger_auditevent WHERE company_id=?1 ORDER BY occurred_at,id LIMIT ?2 OFFSET ?3")?;
                let raw=st.query_map(fb_db::rusqlite::params![co.to_db(),(limit+1) as i64,after as i64],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?,r.get::<_,String>(6)?)))?.collect::<Result<Vec<_>,_>>()?;
                raw.into_iter().map(|(id,user,name,at,action,target,detail)|{
                    let detail:Value=fb_db::parse_unique_json(&detail).map_err(|_|fb_core::FbError::validation("Invalid audit evidence."))?;
                    let detail=fb_core::attachments::public_audit_detail(tx,co,&action,&detail)?;
                    Ok(json!({"id":id,"actor_id":user,"actor_name":name,"occurred_at":at,"action":action,"target":target,"detail":fb_core::card_numbers::mask_text_fields(&detail)}))
                }).collect()
            })?;
            json!({"events":rows.iter().take(limit).collect::<Vec<_>>(),"next":(rows.len()>limit).then_some(after.saturating_add(limit))})
        }
        Operation::AccountingSetup => book.read(|tx| {
            services::access(tx, actor, co)?;
            fb_core::documents::setup_snapshot(tx, co)
        })?,
        Operation::InvoiceSettings => book.read(|tx| {
            services::access(tx,actor,co)?;
            let preferences=fb_core::invoice_preferences::get(tx,co)?;
            let gmail=fb_core::invoice_email::connection(tx,co)?;
            let stripe=fb_core::invoice_stripe::current_connection(tx,co,&preferences.settings.stripe_mode)?;
            Ok::<_,fb_core::FbError>(json!({"details":fb_core::company_documents::get(tx,co)?,"logo":fb_core::company_documents::logo_json(tx,actor,co)?,"preferences":preferences,"google_connected":gmail.is_some_and(|c|c.status=="CONNECTED"),"stripe_connected":stripe.is_some_and(|c|c["status"]=="CONNECTED")}))
        })?,
        Operation::TaxSettings => book.read(|tx| { services::access(tx, actor, co)?; fb_core::salestax::settings_for(tx, co) })?,
        Operation::TaxItems => book
            .read(|tx| {
                services::access(tx, actor, co)?;
                fb_core::salestax::list(tx, co)
            })
            .map(Value::Array)?,
        Operation::Bills => Value::Array(fb_core::payables::current_bills(book, actor, co)?),
        Operation::Bill { bill_id } => fb_core::payables::get_bill(book, actor, co, &bill_id)?,
        Operation::Checks => Value::Array(fb_core::checks::current_checks(book, actor, co)?),
        Operation::JobTickets { customer_id, status } => book
            .read(|tx| {
                services::access(tx, actor, co)?;
                fb_core::jobs::tickets(tx, co, &fb_core::jobs::TicketFilter { customer_id, status: Some(status.unwrap_or_else(|| "ALL".into())) })
            })
            .map(Value::Array)?,
        Operation::Check { check_id } => fb_core::checks::get_check(book, actor, co, &check_id)?,
        Operation::Invoice { invoice_id } => book.read(|tx| {
            services::access(tx, actor, co)?;
            fb_core::sales::invoice(tx, co, &invoice_id)
        })?,
        Operation::Journal { journal_id } => book.read(|tx| {
            services::access(tx, actor, co)?;
            let id = fb_core::JournalId::parse(&journal_id)
                .ok_or_else(|| fb_core::FbError::validation("Invalid journal id."))?;
            let journal = fb_core::store::journal_in(tx, id, co)?;
            services::journal_snapshot(tx, &journal)
        })?,
        Operation::TrialBalance { as_of } => {
            let r = reports::trial_balance(book, actor, co, &as_of)?;
            json!({"as_of":r.as_of,"debit":r.debit.to_string(),"credit":r.credit.to_string(),"rows":r.rows.iter().map(|r|json!({"account_id":r.account.id.to_db(),"code":r.account.code,"name":r.account.name,"debit":r.debit.to_string(),"credit":r.credit.to_string()})).collect::<Vec<_>>()})
        }
        Operation::ProfitAndLoss {
            start,
            end,
            basis: b,
        } => {
            let r = reports::financial::profit_and_loss(
                book,
                actor,
                co,
                basis(&b)?,
                start.as_deref().map(date).transpose()?,
                date(&end)?,
            )?;
            json!({"net_income":r.net_income.to_string(),"gross_profit":r.gross_profit.to_string(),"unconverted":r.unconverted,"sections":r.sections.iter().map(|s|json!({"heading":s.heading,"total":s.total.to_string(),"rows":s.rows.iter().map(|r|json!({"account_id":r.account.id.to_db(),"name":r.account.name,"amount":r.amount.to_string()})).collect::<Vec<_>>()})).collect::<Vec<_>>()})
        }
        Operation::BalanceSheet { as_of, basis: b } => {
            let r = reports::financial::balance_sheet(book, actor, co, basis(&b)?, date(&as_of)?)?;
            json!({"assets":r.assets.total.to_string(),"liabilities":r.liabilities.total.to_string(),"equity":r.total_equity.to_string(),"retained_earnings":r.retained_earnings.to_string(),"net_income":r.net_income.to_string(),"balanced":r.balanced,"unconverted":r.unconverted})
        }
        Operation::FeedAccounts => {
            let accounts = bankfeeds::review_accounts(book, actor, co)?;
            Value::Array(accounts.iter().map(review_account).collect())
        }
        Operation::FeedReview => {
            let (accounts, rows) = bankfeeds::review_snapshot(book, actor, co)?;
            json!({"accounts":accounts.iter().map(review_account).collect::<Vec<_>>(),
                "items":rows.iter().map(review_row).collect::<Vec<_>>()})
        }
        kind @ (Operation::PendingFeed { after, limit } | Operation::DecidedFeed { after, limit }) => {
            if !(1..=100).contains(&limit) {
                return Err(AppError::Invalid("limit must be 1–100.".into()));
            }
            let decided = matches!(kind, Operation::DecidedFeed { .. });
            let (rows, total) = bankfeeds::review_page(book, actor, co, decided, after, limit)?;
            let items: Vec<_> = rows.iter().map(review_row).collect();
            let next = after.saturating_add(limit);
            json!({"items":items,"next":(next < total).then_some(next),"total":total})
        }
        Operation::SuggestFeed { transaction_id } => {
            let s = bankfeeds::suggest(book, actor, co, &transaction_id)?;
            let matches = book.read(|q| {
                s.matches.iter().map(|m| {
                    let receipt_ids = bankfeeds::journal_receipt_ids(q, co, &m.journal_id)?;
                    Ok(json!({"line_id":m.line_id,"journal_id":m.journal_id,"label":m.label,"date":m.date,"receipt_ids":receipt_ids}))
                }).collect::<fb_core::FbResult<Vec<_>>>()
            })?;
            json!({"kind":s.kind,"account_id":s.account_id,"payee_name":s.payee_name,"matches":matches,"receipts":s.receipts.iter().map(|r|json!({"receipt_id":r.receipt_id,"label":r.label,"date":r.date})).collect::<Vec<_>>()})
        }
        Operation::AcceptFeed {
            transaction_id,
            category,
            preconditions,
        } => {
            let mut input = serde_json::to_value(category).expect("typed category");
            if let Some(preconditions) = preconditions {
                input["preconditions"] = json!(preconditions);
            }
            let accepted = bankfeeds::accept(
                book, actor, co, &transaction_id,
                &input,
                &bankfeeds::SecretSet::none(),
            )?;
            let mut v = item(&accepted);
            v["decision"] = json!(bankfeeds::review_decision(book, actor, co, &accepted)?);
            v
        }
        Operation::IgnoreFeed { transaction_id } => item(&bankfeeds::ignore(
            book,
            actor,
            co,
            &transaction_id,
            &bankfeeds::SecretSet::none(),
        )?),
        Operation::ReturnFeed { transaction_id } => item(&bankfeeds::return_to_review(
            book,
            actor,
            co,
            &transaction_id,
            &bankfeeds::SecretSet::none(),
        )?),
        _ => return Err(AppError::Invalid("Operation handled by authority.".into())),
    })
}
