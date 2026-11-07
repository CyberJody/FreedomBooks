//! Audited starting points. Reads find the first unused number at or above the selected start.
//! Existing books retain their original allocation rules until a start is explicitly saved.
use crate::{AccountId, Book, CompanyId, FbError, FbResult, UserId, documents, services, store};
use fb_db::{
    Query,
    rusqlite::{OptionalExtension, params},
};
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub const CHECK: &str = "CHECK";
pub const MAX_DOCUMENT: i64 = 999_999_999_999;

pub fn label(kind: &str) -> &'static str {
    match kind {
        "EST" => "Estimates",
        "INV" => "Invoices",
        "RCP" => "Customer payments",
        "BILL" => "Bills",
        "BPAY" => "Bill payments",
        "DEP" => "Bank deposits",
        "CHK" => "Check document references",
        "SR" => "Sales receipts",
        "TRF" => "Transfers",
        "CM" => "Credit memos",
        "RFD" => "Customer refunds",
        "CDP" => "Customer deposits",
        "CDA" => "Deposit applications",
        "CDR" => "Deposit refunds",
        _ => "Checks",
    }
}

fn preference(
    q: &(impl Query + ?Sized),
    co: CompanyId,
    kind: &str,
    scope: &str,
) -> FbResult<Option<(String, i64)>> {
    Ok(q.query_row(
        "SELECT start,revision FROM fb_numbering WHERE company_id=?1 AND kind=?2 AND scope=?3",
        params![co.to_db(), kind, scope],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()?)
}

pub fn clean_start(kind: &str, value: &str) -> FbResult<String> {
    if kind != CHECK && !documents::KINDS.contains(&kind) {
        return Err(FbError::validation("Choose a supported document kind."));
    }
    let max = if kind == CHECK { 20 } else { 12 };
    if value.is_empty()
        || value.len() > max
        || !value.bytes().all(|b| b.is_ascii_digit())
        || value.bytes().all(|b| b == b'0')
    {
        return Err(FbError::validation(format!(
            "Enter a positive whole number of up to {max} digits."
        )));
    }
    Ok(if kind == CHECK {
        value.to_owned()
    } else {
        value.trim_start_matches('0').to_owned()
    })
}

/// Used by document creation inside its owning write transaction. Historical identities never move.
pub(crate) fn document_next(
    q: &(impl Query + ?Sized),
    co: CompanyId,
    kind: &str,
) -> FbResult<Option<i64>> {
    let start = if let Some((start, _)) = preference(q, co, kind, "")? {
        start
            .parse::<i64>()
            .map_err(|_| FbError::validation("Invalid document numbering settings."))?
    } else {
        q.query_row(
            "SELECT last_number+1 FROM documents_documentsequence WHERE company_id=?1 AND kind=?2",
            params![co.to_db(), kind],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(1)
    };
    let mut next = start;
    let mut stmt=q.prepare("SELECT ordinal FROM documents_documentidentity WHERE company_id=?1 AND kind=?2 AND ordinal>=?3 ORDER BY ordinal")?;
    for n in stmt.query_map(params![co.to_db(), kind, start], |r| r.get::<_, i64>(0))? {
        let n = n?;
        if n > next {
            break;
        }
        if n == next {
            next += 1;
        }
    }
    Ok((next <= MAX_DOCUMENT).then_some(next))
}

/// A bank's check namespace includes written checks, bill payments and both refund families.
pub(crate) fn check_next(
    q: &(impl Query + ?Sized),
    co: CompanyId,
    bank: AccountId,
) -> FbResult<Option<String>> {
    let Some((start, _)) = preference(q, co, CHECK, &bank.to_db())? else {
        return Ok(None);
    };
    // Current checks can change number or bank; revisions keep assigned numbers reserved.
    let mut stmt =
        q.prepare("SELECT number FROM fb_check_number_history WHERE company_id=?1 AND bank_id=?2")?;
    let used = stmt
        .query_map(params![co.to_db(), bank.to_db()], |r| r.get::<_, String>(0))?
        .collect::<Result<BTreeSet<_>, _>>()?;
    let mut next = start
        .parse::<u128>()
        .map_err(|_| FbError::validation("Invalid check numbering settings."))?;
    loop {
        let text = format!("{next:0>width$}", width = start.len());
        if text.len() > 20 {
            return Err(FbError::validation(
                "This check sequence is exhausted. Choose a new starting number.",
            ));
        }
        if !used.contains(&text) {
            return Ok(Some(text));
        }
        next += 1;
    }
}

fn current(
    q: &(impl Query + ?Sized),
    co: CompanyId,
    kind: &str,
    bank: Option<AccountId>,
) -> FbResult<String> {
    if kind == CHECK {
        crate::checks::numbering::next_number_tx(
            q,
            co,
            bank.ok_or_else(|| FbError::validation("Choose a bank account."))?,
        )
    } else {
        Ok(document_next(q, co, kind)?
            .map(|n| n.to_string())
            .unwrap_or_default())
    }
}

fn scope(
    q: &(impl Query + ?Sized),
    co: CompanyId,
    kind: &str,
    bank: Option<AccountId>,
) -> FbResult<String> {
    if kind == CHECK {
        let id = bank.ok_or_else(|| FbError::validation("Choose a designated bank account."))?;
        let eligible:bool=q.query_row("SELECT EXISTS(SELECT 1 FROM ledger_account a JOIN documents_designatedbank b ON b.account_id=a.id JOIN documents_accountingsetup s ON s.id=b.setup_id WHERE a.id=?1 AND a.company_id=?2 AND s.company_id=?2 AND a.active AND a.postable AND a.kind='ASSET')",params![id.to_db(),co.to_db()],|r|r.get(0))?;
        if !eligible {
            return Err(FbError::validation(
                "Choose an active designated bank account in this company.",
            ));
        }
        Ok(id.to_db())
    } else if documents::KINDS.contains(&kind) && bank.is_none() {
        Ok(String::new())
    } else {
        Err(FbError::validation("Choose a supported document kind."))
    }
}

pub fn settings(book: &mut Book, actor: UserId, co: CompanyId) -> FbResult<Value> {
    book.read(|q| {
        services::access(q, actor, co)?;
        let mut rows = Vec::new();
        for kind in documents::KINDS {
            rows.push(row(q, co, kind, None, label(kind).into())?);
        }
        let setup = documents::setup_snapshot(q, co)?;
        for account in store::accounts(q, co)? {
            if account.active
                && account.postable
                && account.kind == crate::model::Kind::Asset
                && setup["banks"]
                    .as_array()
                    .is_some_and(|banks| banks.contains(&json!(account.id.to_db())))
            {
                rows.push(row(
                    q,
                    co,
                    CHECK,
                    Some(account.id),
                    format!("Checks · {}", account.label()),
                )?);
            }
        }
        Ok(json!(rows))
    })
}
fn row(
    q: &(impl Query + ?Sized),
    co: CompanyId,
    kind: &str,
    bank: Option<AccountId>,
    label: String,
) -> FbResult<Value> {
    let scope = bank.map(|b| b.to_db()).unwrap_or_default();
    let next = match current(q, co, kind, bank) {
        Ok(n) => n,
        Err(FbError::Validation(e))
            if e.first()
                .is_some_and(|s| s.starts_with("This check sequence is exhausted.")) =>
        {
            String::new()
        }
        Err(e) => return Err(e),
    };
    let display = if next.is_empty() {
        "Exhausted".into()
    } else if kind == CHECK {
        next.clone()
    } else {
        format!("{kind}-{:06}", next.parse::<i64>().unwrap())
    };
    Ok(
        json!({"kind":kind,"bank_id":bank.map(|b|b.to_db()),"scope":scope,"label":label,"next":next,"display":display,"revision":preference(q,co,kind,&scope)?.map(|(_,r)|r).unwrap_or(0),"max_digits":if kind==CHECK {20}else{12}}),
    )
}

#[derive(Debug, Clone)]
pub struct Change<'a> {
    pub kind: &'a str,
    pub bank: Option<AccountId>,
    pub next: &'a str,
    pub expected_next: &'a str,
    pub expected_revision: i64,
}

pub fn set_next(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    change: Change<'_>,
) -> FbResult<Value> {
    book.write(|tx| {
        let user=services::access(tx,actor,co)?;
        if !user.is_staff && !user.is_superuser {return Err(FbError::denied("Only an administrator can change document numbering."));}
        let kind=change.kind;
        let scope=scope(tx,co,kind,change.bank)?;
        let start=clean_start(kind,change.next)?;
        let before=preference(tx,co,kind,&scope)?;
        let revision=before.as_ref().map(|(_,r)|*r).unwrap_or(0);
        let old_next=match current(tx,co,kind,change.bank) {
            Ok(n)=>n,
            Err(FbError::Validation(e)) if e.first().is_some_and(|s|s.starts_with("This check sequence is exhausted."))=>String::new(),
            Err(e)=>return Err(e),
        };
        if change.expected_revision!=revision || change.expected_next!=old_next {
            return Err(FbError::validation("The next number changed since you opened settings. Reload and review before saving."));
        }
        let conflict:Option<(String,String)>=if kind==CHECK {
            tx.query_row("SELECT kind,document FROM fb_check_number_history WHERE company_id=?1 AND bank_id=?2 AND number=?3 ORDER BY document LIMIT 1",params![co.to_db(),scope,start],|r|Ok((r.get(0)?,r.get(1)?))).optional()?
        } else {
            tx.query_row("SELECT kind,number FROM documents_documentidentity WHERE company_id=?1 AND kind=?2 AND ordinal=?3",params![co.to_db(),kind,start.parse::<i64>().unwrap()],|r|Ok((r.get(0)?,r.get(1)?))).optional()?
        };
        if let Some((holder,document))=conflict {
            let owner=if kind==CHECK {format!("{holder} {document}")}else{document};
            return Err(FbError::validation(format!("Number {start} is already used by {owner}. Choose another number.")));
        }
        if start==old_next {return row(tx,co,kind,change.bank,label(kind).into());}
        let revision=documents::next_revision(revision)?;
        let target=format!("numbering:{kind}:{scope}");
        services::audit(tx,&user,co,"numbering.changed",target.clone(),json!({"kind":kind,"scope":scope,"bank_id":change.bank.map(|b|b.to_db()),"old_next":old_next,"new_next":start,"old_start":before.map(|(s,_)|s),"new_start":start,"revision":revision}))?;
        let audit:String=tx.query_row("SELECT id FROM ledger_auditevent WHERE company_id=?1 AND action='numbering.changed' AND target=?2 AND json_extract(detail,'$.revision')=?3 ORDER BY id DESC LIMIT 1",params![co.to_db(),target,revision],|r|r.get(0))?;
        if revision==1 {
            tx.execute("INSERT INTO fb_numbering(company_id,kind,scope,start,revision,audit_id) VALUES(?1,?2,?3,?4,?5,?6)",params![co.to_db(),kind,scope,start,revision,audit])?;
        } else {
            tx.execute("UPDATE fb_numbering SET start=?4,revision=?5,audit_id=?6 WHERE company_id=?1 AND kind=?2 AND scope=?3",params![co.to_db(),kind,scope,start,revision,audit])?;
        }
        row(tx,co,kind,change.bank,label(kind).into())
    })
}
