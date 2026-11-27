//! Company-scoped snapshots of the bank review queue and its decisions.
use super::*;
use std::collections::BTreeSet;

#[derive(serde::Serialize)]
pub struct ReviewIdentity {
    pub provider_id_exact: bool,
    pub provider_host: String,
    pub currency: String,
}

impl ReviewIdentity {
    fn new(provider_host: String, currency: String, key: &str, shown: &str, domain: &str, secret: &[u8], verified: bool) -> Self {
        Self {
            provider_id_exact: verified && !shown.is_empty()
                && !shown.contains("***")
                && !secret.is_empty()
                && fb_db::bankfeeds_text::identity_key(secret, domain, shown) == key,
            provider_host,
            currency,
        }
    }
}

pub struct ReviewRow {
    pub item: Item,
    /// Current feed mapping. The original ledger account of a posted decision is in `decision`.
    pub mapped_account_id: Option<String>,
    pub identity: ReviewIdentity,
    pub decision: Option<ReviewDecision>,
    pub candidates: Option<ReviewCandidates>,
}

#[derive(serde::Serialize)]
pub struct ReviewCandidates {
    #[serde(rename = "match")]
    matches: usize,
    receipt: usize,
    recorded: bool,
}

impl ReviewCandidates {
    pub(super) fn any(&self) -> bool {
        self.matches != 0 || self.receipt != 0 || self.recorded
    }
}

pub(super) fn review_candidates(q: &impl Query, company: CompanyId, item: &Item, limit: i64) -> FbResult<ReviewCandidates> {
    Ok(ReviewCandidates {
        matches: match_candidates(q, company, item, limit)?.len(),
        receipt: receipt_candidates(q, company, item, limit)?.len(),
        recorded: already_recorded(q, company, item)?.is_some(),
    })
}

pub struct ReviewAccount {
    pub feed: Feed,
    pub identity: ReviewIdentity,
    pub connection_active: bool,
}

fn identity_secret(q: &impl Query) -> FbResult<Vec<u8>> {
    Ok(q.query_row("SELECT secret FROM bankfeeds_identity_secret WHERE name='identity'", [], |r| r.get(0))
        .optional()?.unwrap_or_default())
}

fn accounts(q: &impl Query, company: CompanyId, secret: &[u8]) -> FbResult<Vec<ReviewAccount>> {
    let mut statement = q.prepare(&format!(
        "SELECT {FEED_COLS},c.host,f.remote_identity,c.active AND NOT c.needs_reconnect AS connection_active,f.provider_id_verified
         FROM bankfeeds_account f JOIN bankfeeds_connection c ON c.id=f.connection_id
         WHERE c.company_id=?1 AND f.active=1 ORDER BY f.id"
    ))?;
    let accounts = statement.query_map([company.to_db()], |r| {
        let feed = feed_row(r)?;
        let identity = ReviewIdentity::new(r.get("host")?, feed.currency.clone(), &r.get::<_, String>("remote_identity")?, &feed.remote_id, ACCOUNT_DOMAIN, secret, r.get("provider_id_verified")?);
        Ok(ReviewAccount { feed, identity, connection_active: r.get("connection_active")? })
    })?;
    accounts.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub fn review_accounts(book: &mut Book, actor: UserId, company: CompanyId) -> FbResult<Vec<ReviewAccount>> {
    book.read(|q| {
        services::access(q, actor, company)?;
        accounts(q, company, &identity_secret(q)?)
    })
}

#[derive(serde::Serialize)]
pub struct ReviewDecision {
    pub kind: String,
    pub reference: String,
    pub journal_id: Option<String>,
    pub document_kind: Option<String>,
    pub bank_account_id: Option<String>,
    pub counterpart_account_ids: BTreeSet<String>,
    pub receipt_ids: Vec<String>,
    pub stale: bool,
}

/// Read one page in the same order as the bank review screen, without provider access.
pub fn review_page(
    book: &mut Book,
    actor: UserId,
    company: CompanyId,
    decided: bool,
    after: usize,
    limit: usize,
) -> FbResult<(Vec<ReviewRow>, usize)> {
    if !(1..=100).contains(&limit) {
        return Err(FbError::validation("Limit must be 1–100."));
    }
    book.read(|q| {
        services::access(q, actor, company)?;
        let total: i64 = q.query_row(
            "SELECT count(*) FROM bankfeeds_transaction t JOIN bankfeeds_account f ON f.id=t.feed_account_id
             JOIN bankfeeds_connection c ON c.id=f.connection_id WHERE c.company_id=?1 AND (t.status<>'NEW')=?2",
            params![company.to_db(), decided],
            |r| r.get(0),
        )?;
        let page = rows(q, company, &identity_secret(q)?, Some(decided), limit as i64, after)?;
        Ok((page, usize::try_from(total).unwrap_or(usize::MAX)))
    })
}

/// Both states and their feed mappings, read under one Book read transaction.
pub fn review_snapshot(book: &mut Book, actor: UserId, company: CompanyId) -> FbResult<(Vec<ReviewAccount>, Vec<ReviewRow>)> {
    book.read(|q| {
        services::access(q, actor, company)?;
        let secret = identity_secret(q)?;
        Ok((accounts(q, company, &secret)?, rows(q, company, &secret, None, -1, 0)?))
    })
}

fn rows(q: &impl Query, company: CompanyId, secret: &[u8], decided: Option<bool>, limit: i64, after: usize) -> FbResult<Vec<ReviewRow>> {
    let mut rows = q.prepare(&format!(
        "SELECT {ITEM_COLS},f.ledger_account_id,c.host,f.currency,t.remote_identity,t.provider_id_verified
         FROM bankfeeds_transaction t JOIN bankfeeds_account f ON f.id=t.feed_account_id
         JOIN bankfeeds_connection c ON c.id=f.connection_id
         WHERE c.company_id=?1 AND (?2 IS NULL OR (t.status<>'NEW')=?2)
         ORDER BY t.posted_on IS NOT NULL,t.posted_on DESC,t.ordinal DESC LIMIT ?3 OFFSET ?4"
    ))?;
    let after = i64::try_from(after).unwrap_or(i64::MAX);
    rows.query_map(params![company.to_db(), decided, limit, after], |r| {
        let item = item_row(r)?;
        let identity = ReviewIdentity::new(r.get(18)?, r.get(19)?, &r.get::<_, String>(20)?, &item.remote_id, TRANSACTION_DOMAIN, secret, r.get(21)?);
        Ok((item, r.get(17)?, identity))
    })?
    .map(|row| {
        let (item, mapped_account_id, identity) = row?;
        let decision = (item.status != "NEW").then(|| decision(q, company, &item)).transpose()?;
        let candidates = (decided.is_none() && item.status == "NEW" && !item.pending)
            .then(|| review_candidates(q, company, &item, 3)).transpose()?;
        Ok(ReviewRow { item, mapped_account_id, identity, decision, candidates })
    })
    .collect()
}

/// Resolve the recorded side of a decision from the posted journal, independently of today's feed mapping.
fn decision(
    q: &(impl Query + ?Sized),
    company: CompanyId,
    item: &Item,
) -> FbResult<ReviewDecision> {
    let linked = if item.status == "IGNORED" {
        let prior: Option<String> = q.query_row(
            "SELECT e.id FROM bankfeeds_link k JOIN bankfeeds_transaction e ON e.ordinal=k.earlier_ordinal
             WHERE k.item_ordinal=?1", [item.ordinal], |r| r.get(0),
        ).optional()?;
        prior.map(|id| super::item(q, company, &id)).transpose()?
    } else {
        None
    };
    let recorded = linked.as_ref().unwrap_or(item);
    let kind = if linked.is_some() {
        "LINKED"
    } else if item.status == "IGNORED" {
        "IGNORED"
    } else {
        item.accepted_as.as_str()
    };
    let mut out = ReviewDecision {
        kind: kind.into(),
        reference: recorded.reference.clone(),
        journal_id: recorded.journal_id.clone(),
        document_kind: None,
        bank_account_id: None,
        counterpart_account_ids: BTreeSet::new(),
        receipt_ids: Vec::new(),
        stale: stale_decision(q, item)?.is_some(),
    };
    if let Some(id) = recorded
        .journal_id
        .as_deref()
        .and_then(crate::JournalId::parse)
    {
        let journal = store::journal_in(q, id, company)?;
        if let Some(source) = journal.source_id {
            out.document_kind = q
                .query_row(
                    "SELECT kind FROM documents_documentidentity WHERE id=?1 AND company_id=?2",
                    params![source, company.to_db()],
                    |r| r.get(0),
                )
                .optional()?;
        }
        let lines = store::lines(q, id)?;
        out.bank_account_id = lines
            .iter()
            .find(|l| Some(l.id.to_db()) == recorded.journal_line_id)
            .map(|l| l.account_id.to_db());
        if let Some(account) = out.bank_account_id.as_deref() {
            out.counterpart_account_ids.extend(
                lines
                    .iter()
                    .map(|l| l.account_id.to_db())
                    .filter(|key| key != account),
            );
            if recorded.status == "MATCHED" {
                for extra in matched_entries(q, company, recorded)? {
                    if extra.journal_id != id.to_db()
                        && let Some(jid) = crate::JournalId::parse(&extra.journal_id)
                    {
                        out.counterpart_account_ids.extend(
                            store::lines(q, jid)?
                                .into_iter()
                                .map(|l| l.account_id.to_db())
                                .filter(|key| key != account),
                        );
                    }
                }
            }
        }
        let mut receipt_ids = journal_receipt_ids(q, company, &id.to_db())?;
        if recorded.status == "MATCHED" {
            for extra in matched_entries(q, company, recorded)? {
                if extra.journal_id != id.to_db() {
                    for rid in journal_receipt_ids(q, company, &extra.journal_id)? {
                        if !receipt_ids.contains(&rid) {
                            receipt_ids.push(rid);
                        }
                    }
                }
            }
        }
        out.receipt_ids = receipt_ids;
    }
    Ok(out)
}

pub fn review_decision(
    book: &mut Book,
    actor: UserId,
    company: CompanyId,
    item: &Item,
) -> FbResult<ReviewDecision> {
    book.read(|q| {
        services::access(q, actor, company)?;
        decision(q, company, &super::item(q, company, &item.id)?)
    })
}

pub fn journal_receipt_ids(
    q: &(impl Query + ?Sized),
    company: CompanyId,
    journal_id: &str,
) -> FbResult<Vec<String>> {
    let mut receipts = q.prepare(
        "SELECT l.receipt_id FROM banking_depositline l JOIN banking_deposit d ON d.id=l.deposit_id
         WHERE d.journal_id=?1 AND d.company_id=?2
         UNION SELECT id FROM sales_receipt WHERE journal_id=?1 AND company_id=?2 ORDER BY 1",
    )?;
    Ok(receipts
        .query_map(params![journal_id, company.to_db()], |r| r.get(0))?
        .collect::<Result<_, _>>()?)
}
