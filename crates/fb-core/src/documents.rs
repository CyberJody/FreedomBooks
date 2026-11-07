//! Document identities, numbering, revision checks, configuration and frozen account evidence.
//! Transaction primitives take `&Writer`, making the owning outer transaction compulsory.
use crate::{AccountId, Book, CompanyId, FbError, FbResult, UserId, py, services, store};
use fb_db::Writer;
use fb_db::rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};

pub const KINDS: &[&str] = &[
    "EST", "INV", "RCP", "BILL", "BPAY", "DEP", "CHK", "SR", "TRF", "CM", "RFD", "CDP", "CDA", "CDR",
];
pub const ROLES: &[&str] = &["receivables", "payables", "undeposited", "collected_tax", "customer_deposits"];
pub fn check_revision(actual: i64, expected: Option<i64>) -> FbResult<()> {
    if expected.is_none_or(|e| e < 0 || e != actual) {
        Err(FbError::validation(
            "This record changed since you opened it. Reload and review before saving.",
        ))
    } else {
        Ok(())
    }
}
pub fn identity(conn: &(impl fb_db::Query + ?Sized), company: CompanyId, id: &str) -> FbResult<Value> {
    conn.query_row("SELECT id,company_id,kind,ordinal,number,request_key,currency FROM documents_documentidentity WHERE company_id=?1 AND id=?2",params![company.to_db(),id],|r|Ok(json!({"id":r.get::<_,String>(0)?,"company_id":r.get::<_,String>(1)?,"kind":r.get::<_,String>(2)?,"ordinal":r.get::<_,i64>(3)?,"number":r.get::<_,String>(4)?,"request_key":r.get::<_,String>(5)?,"currency":r.get::<_,String>(6)?}))).optional()?.ok_or_else(||FbError::missing("DocumentIdentity"))
}
/// Requires the owning Writer, never a read callback handle.
/// ```compile_fail
/// fn invalid(reader: &fb_db::Reader<'_>, actor: fb_core::UserId, company: fb_core::CompanyId) {
///     fb_core::documents::create_identity(reader, actor, company, "INV", "00000000-0000-0000-0000-000000000001");
/// }
/// ```
pub fn create_identity(tx: &Writer<'_>, actor: UserId, company: CompanyId, kind: &str, request_key: &str) -> FbResult<Value> {
    atomic_primitive(tx, || {
        let user = services::access(tx, actor, company)?;
        let co = store::get_company(tx, company)?;
        if !KINDS.contains(&kind) {
            return Err(FbError::validation("Unsupported document type."));
        }
        let key = py::py_uuid(request_key)
            .ok_or_else(|| FbError::validation("Supply a stable UUID request key."))?
            .0;
        let existing: Option<String> = tx
            .query_row(
                "SELECT id FROM documents_documentidentity WHERE company_id=?1 AND request_key=?2",
                params![company.to_db(), key],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = existing {
            let found = identity(tx, company, &id)?;
            if found["kind"] != kind {
                return Err(FbError::validation("This request key belongs to a different document type."));
            }
            return Ok(found);
        }
        let last: Option<i64> = tx.query_row(
            "SELECT last_number FROM documents_documentsequence WHERE company_id=?1 AND kind=?2",
            params![company.to_db(), kind], |r| r.get(0),
        ).optional()?;
        if last.is_none() {
            tx.execute("INSERT INTO documents_documentsequence(id,company_id,kind) VALUES(?1,?2,?3)",
                params![fb_ids::new_v7().to_string(), company.to_db(), kind])?;
        }
        let last = last.unwrap_or(0);
        let next = crate::numbering::document_next(tx, company, kind)?
            .ok_or_else(|| FbError::validation("This document sequence is exhausted."))?;
        tx.execute(
            "UPDATE documents_documentsequence SET last_number=?3 WHERE company_id=?1 AND kind=?2",
            params![company.to_db(), kind, next.max(last)],
        )?;
        let id = fb_ids::new_v7().to_string();
        let number = format!("{kind}-{next:06}");
        tx.execute("INSERT INTO documents_documentidentity(id,company_id,kind,ordinal,number,request_key,currency,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![id,company.to_db(),kind,next,number,key,co.currency,py::now_utc()])?;
        services::audit(
            tx,
            &user,
            company,
            "document.identified",
            format!("documentidentity:{id}"),
            json!({"number":number,"currency":co.currency,"request_key":key}),
        )?;
        identity(tx, company, &id)
    })
}
fn setup_record(conn: &(impl fb_db::Query + ?Sized), company: CompanyId) -> FbResult<Value> {
    let sql = "SELECT id,receivables_id,payables_id,undeposited_id,collected_tax_id,customer_deposits_id,revision FROM documents_accountingsetup WHERE company_id=?1";
    let found: Option<Value> = conn
        .query_row(sql, [company.to_db()], |r| {
            let mut v = json!({"id":r.get::<_,String>(0)?,"revision":r.get::<_,i64>(6)?,"banks":[]});
            for (i, role) in ROLES.iter().enumerate() {
                v[*role] = json!(r.get::<_, Option<String>>(i + 1)?);
            }
            Ok(v)
        })
        .optional()?;
    let mut v = found.unwrap_or_else(|| {
        let mut v = json!({"revision":0,"banks":[]});
        for r in ROLES {
            v[*r] = Value::Null;
        }
        v
    });
    if let Some(id) = v["id"].as_str() {
        let mut stmt = conn.prepare("SELECT account_id FROM documents_designatedbank WHERE setup_id=?1 ORDER BY account_id")?;
        v["banks"] = json!(stmt.query_map([id], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?);
    }
    Ok(v)
}
fn validate_mapping(conn: &(impl fb_db::Query + ?Sized), company: CompanyId, values: &Value, required: Option<&[&str]>, banks: &[String]) -> FbResult<()> {
    let configured = ROLES.iter().filter_map(|r| values[*r].as_str()).collect::<Vec<_>>();
    let unique = configured.iter().collect::<std::collections::BTreeSet<_>>();
    if unique.len() != configured.len() || banks.iter().any(|b| configured.contains(&b.as_str())) {
        return Err(FbError::validation(
            "Accounting roles must be distinct, including banks versus receivables and undeposited funds.",
        ));
    }
    for role in ROLES {
        if required.is_some_and(|r| !r.contains(role)) {
            continue;
        }
        let Some(id) = values[*role].as_str() else {
            if required.is_some() {
                return Err(FbError::validation(format!(
                    "Configure {} in Accounting setup first.",
                    role.replace('_', " ")
                )));
            }
            continue;
        };
        let kind = if ["receivables", "undeposited"].contains(role) {
            "ASSET"
        } else {
            "LIABILITY"
        };
        let account = AccountId::parse(id).map(|id| store::account(conn, id)).transpose()?.flatten();
        if account
            .as_ref()
            .is_none_or(|a| a.company_id != company || !a.active || !a.postable || a.kind.as_str() != kind)
        {
            return Err(FbError::validation(format!(
                "Choose an active, postable {} account in this company, or clear this default.",
                kind.to_lowercase()
            )));
        }
        if *role == "customer_deposits" && account.unwrap().detail_type.as_str() != "OTHER_CURRENT_LIABILITY" {
            return Err(FbError::validation(
                "Choose an active, postable account of type Other current liability for customer deposits, or clear this default.",
            ));
        }
    }
    for id in banks {
        let account = AccountId::parse(id).map(|id| store::account(conn, id)).transpose()?.flatten();
        if account.is_none_or(|a| a.company_id != company || !a.active || !a.postable || a.kind.as_str() != "ASSET") {
            return Err(FbError::validation(
                "Designated banks must be active, postable asset accounts in this company. Replace or clear unavailable banks.",
            ));
        }
    }
    Ok(())
}
pub(crate) fn next_revision(revision: i64) -> FbResult<i64> {
    revision.checked_add(1).ok_or_else(|| FbError::validation("This record's revision is exhausted."))
}
/// Values contain role ids and banks; omitted customer_deposits preserves the stored role.
/// All other omitted roles are cleared, exactly as Python's default keyword arguments.
pub fn save_setup(book: &mut Book, actor: UserId, company: CompanyId, values: &Value, expected_revision: Option<i64>) -> FbResult<Value> {
    book.write(|tx| save_setup_tx(tx, actor, company, values, expected_revision))
}

pub(crate) fn save_setup_tx(tx: &Writer<'_>, actor: UserId, company: CompanyId, values: &Value, expected_revision: Option<i64>) -> FbResult<Value> {
        let user=services::access(tx,actor,company)?;
        let before=setup_record(tx,company)?;check_revision(before["revision"].as_i64().unwrap(),expected_revision)?;
        let mut v=json!({});for r in ROLES{v[*r]=if *r=="customer_deposits"{values.get(r).unwrap_or(&before[*r]).clone()}else{values[*r].clone()};}
        if !values.is_object(){return Err(FbError::validation("Supply accounting setup fields."));}
        for r in ROLES {
            if !v[*r].is_null() && v[*r].as_str().and_then(AccountId::parse).is_none() {
                return Err(FbError::validation("Choose an account from this company."));
            }
        }
        let banks=match values.get("banks") {
            None => Vec::new(),
            Some(v) => v.as_array().cloned().ok_or_else(||FbError::validation("Select each bank account once."))?,
        };
        let banks=banks.iter().map(|b|b.as_str().map(str::to_string)).collect::<Option<Vec<_>>>().ok_or_else(||FbError::validation("Select each bank account once."))?;
        if banks.iter().collect::<std::collections::BTreeSet<_>>().len()!=banks.len(){return Err(FbError::validation("Select each bank account once."));}
        validate_mapping(tx,company,&v,None,&banks)?;
        let id=before["id"].as_str().map(str::to_string).unwrap_or_else(||fb_ids::new_v7().to_string());
        let sql = if before["id"].is_null() {
            "INSERT INTO documents_accountingsetup(id,company_id,receivables_id,payables_id,undeposited_id,collected_tax_id,customer_deposits_id,revision) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)"
        } else {
            "UPDATE documents_accountingsetup SET receivables_id=?3,payables_id=?4,undeposited_id=?5,collected_tax_id=?6,customer_deposits_id=?7,revision=?8 WHERE id=?1 AND company_id=?2"
        };
        tx.execute(sql,params![id,company.to_db(),v["receivables"].as_str(),v["payables"].as_str(),v["undeposited"].as_str(),v["collected_tax"].as_str(),v["customer_deposits"].as_str(),next_revision(before["revision"].as_i64().unwrap())?])?;
        tx.execute("DELETE FROM documents_designatedbank WHERE setup_id=?1",[&id])?;
        for b in banks{tx.execute("INSERT INTO documents_designatedbank VALUES(?1,?2,?3)",params![fb_ids::new_v7().to_string(),id,b])?;}
        let after=setup_snapshot(tx,company)?;
        services::audit(tx,&user,company,"accounting.setup",format!("accountingsetup:{id}"),json!({"before":without_id(&before),"after":without_id(&after)}))?;
        Ok(after)
}
pub fn lock_configuration(
    tx: &Writer<'_>,
    actor: UserId,
    company: CompanyId,
    required: &[&str],
    bank: Option<AccountId>,
    additional: &[AccountId],
) -> FbResult<Value> {
    services::access(tx, actor, company)?;
    if required.iter().any(|r| !ROLES.contains(r)) {
        return Err(FbError::validation("Unknown accounting role."));
    }
    let v = setup_snapshot(tx, company)?;
    let selected = bank.map(|b| b.to_db()).into_iter().collect::<Vec<_>>();
    if selected.iter().any(|b| !v["banks"].as_array().unwrap().contains(&json!(b))) {
        return Err(FbError::validation("Choose a designated bank account in Accounting setup."));
    }
    for id in additional {
        if store::account(tx, *id)?.is_none_or(|a| a.company_id != company || !a.active || !a.postable) {
            return Err(FbError::validation(
                "Additional accounts must be active posting accounts in this company.",
            ));
        }
    }
    validate_mapping(tx, company, &v, Some(required), &selected)?;
    let mut result = json!({});
    for r in required {
        result[*r] = v[*r].clone();
    }
    if let Some(b) = bank {
        result["bank"] = json!(b.to_db());
    }
    Ok(result)
}
pub fn snapshot_source_accounts(
    tx: &Writer<'_>,
    actor: UserId,
    company: CompanyId,
    source_id: &str,
    mapping: &Value,
) -> FbResult<Vec<Value>> {
    atomic_primitive(tx, || {
        let user = services::access(tx, actor, company)?;
        identity(tx, company, source_id)?;
        let map = mapping
            .as_object()
            .filter(|m| !m.is_empty() && m.keys().all(|r| !py::py_strip(r).is_empty() && py::py_len(r) <= 80))
            .ok_or_else(|| FbError::validation("Supply named account roles of 1–80 characters."))?;
        let existing = source_accounts(tx, source_id)?;
        if !existing.is_empty() {
            let mut old = json!({});
            for r in &existing {
                old[r["role"].as_str().unwrap()] = r["account_id"].clone();
            }
            if old != *mapping {
                return Err(FbError::validation("Source account snapshots cannot be replaced."));
            }
            return Ok(existing);
        }
        let mut roles = map.keys().collect::<Vec<_>>();
        roles.sort();
        for role in roles {
            let account = map[role]
                .as_str()
                .and_then(AccountId::parse)
                .map(|id| store::account(tx, id))
                .transpose()?
                .flatten()
                .filter(|a| a.company_id == company && a.active && a.postable)
                .ok_or_else(|| FbError::validation("Source accounts must be active posting accounts in this company."))?;
            tx.execute(
                "INSERT INTO documents_documentaccount VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![
                    fb_ids::new_v7().to_string(),
                    source_id,
                    role,
                    account.id.to_db(),
                    account.code,
                    account.name,
                    account.kind.as_str()
                ],
            )?;
        }
        let mut refs = source_accounts(tx, source_id)?;
        // Python returns the just-created refs in sorted(mapping.items()) code-point order.
        // An exact retry returns the database Meta.ordering (locale order), an intentional oddity.
        refs.sort_by(|a, b| a["role"].as_str().cmp(&b["role"].as_str()));
        services::audit(
            tx,
            &user,
            company,
            "document.accounts",
            format!("documentidentity:{source_id}"),
            json!({"accounts":refs.iter().map(|r|json!({"role":r["role"],"id":r["account_id"],"code":r["account_code"],"name":r["account_name"],"kind":r["account_kind"]})).collect::<Vec<_>>()}),
        )?;
        Ok(refs)
    })
}
pub fn source_accounts(conn: &(impl fb_db::Query + ?Sized), source: &str) -> FbResult<Vec<Value>> {
    let mut stmt=conn.prepare("SELECT role,account_id,account_code,account_name,account_kind FROM documents_documentaccount WHERE source_id=?1 ORDER BY role COLLATE fb_text")?;
    Ok(stmt.query_map([source],|r|Ok(json!({"role":r.get::<_,String>(0)?,"account_id":r.get::<_,String>(1)?,"account_code":r.get::<_,String>(2)?,"account_name":r.get::<_,String>(3)?,"account_kind":r.get::<_,String>(4)?})))?.collect::<Result<Vec<_>,_>>()?)
}
/// INTERNAL trusted module boundary: the module checks its own financial state,
/// configuration, snapshots and allocations inside this same outer transaction.
pub fn post_source(
    tx: &Writer<'_>,
    actor: UserId,
    company: CompanyId,
    source_id: &str,
    date: &str,
    memo: &str,
    lines: Vec<Value>,
) -> FbResult<crate::model::Journal> {
    atomic_primitive(tx, || {
        services::access(tx, actor, company)?;
        let source = identity(tx, company, source_id)?;
        if source["kind"] == "EST" {
            return Err(FbError::validation("Estimates do not post journals."));
        }
        let existing: Option<String> = tx
            .query_row(
                "SELECT id FROM ledger_journal WHERE source_id=?1 AND reversal_of_id IS NULL",
                [source_id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = existing {
            let journal = store::journal_in(tx, id.parse().expect("stored UUID"), company)?;
            if journal.status != crate::model::Status::Posted {
                return Err(FbError::validation("Unexpected source draft: use the owning module to repair it."));
            }
            return Ok(journal);
        }
        let input = services::DraftInput::new(date, memo, lines);
        let journal = services::save_draft_source_tx(tx, actor, company, &input, None, Some(source_id))?;
        services::post_journal_source_tx(tx, actor, company, journal.id, None, Some(source_id))
    })
}
pub fn reverse_source(
    tx: &Writer<'_>,
    actor: UserId,
    company: CompanyId,
    source_id: &str,
    date: &str,
    reason: &str,
) -> FbResult<crate::model::Journal> {
    atomic_primitive(tx, || {
        services::access(tx, actor, company)?;
        identity(tx, company, source_id)?;
        let original: Option<String> = tx
            .query_row(
                "SELECT id FROM ledger_journal WHERE company_id=?1 AND source_id=?2 AND reversal_of_id IS NULL",
                params![company.to_db(), source_id],
                |r| r.get(0),
            )
            .optional()?;
        let id = original.ok_or_else(|| FbError::missing("Journal"))?;
        let existing: Option<String> = tx
            .query_row("SELECT id FROM ledger_journal WHERE reversal_of_id=?1", [&id], |r| r.get(0))
            .optional()?;
        if let Some(id) = existing {
            return store::journal_in(tx, id.parse().expect("stored UUID"), company);
        }
        services::reverse_journal_source_tx(tx, actor, company, id.parse().expect("stored UUID"), date, reason, Some(source_id))
    })
}

pub fn control_role(conn: &(impl fb_db::Query + ?Sized), company: CompanyId, account: AccountId) -> FbResult<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT role FROM fb_control_accounts WHERE company_id=?1 AND account_id=?2 ORDER BY priority LIMIT 1",
            params![company.to_db(), account.to_db()],
            |r| r.get(0),
        )
        .optional()?)
}
/// The shared FX6 purchase-line rule (also used for vendor default hints).
pub fn purchase_line_allowed(
    conn: &(impl fb_db::Query + ?Sized),
    company: CompanyId,
    account: &crate::model::Account,
    paying_bank: Option<AccountId>,
) -> FbResult<bool> {
    Ok(crate::directory::purchase_line_allowed(account)
        && control_role(conn, company, account.id)?.is_none()
        && paying_bank != Some(account.id))
}
pub fn line_account_error(
    conn: &(impl fb_db::Query + ?Sized),
    company: CompanyId,
    account: Option<&crate::model::Account>,
    paying_bank: Option<AccountId>,
    retained: bool,
) -> FbResult<Option<String>> {
    let Some(a) = account else {
        return Ok(None);
    };
    if purchase_line_allowed(conn, company, a, paying_bank)? {
        return Ok(None);
    }
    let role = control_role(conn, company, a.id)?;
    let mut eligible_shape = a.clone();
    eligible_shape.active = true;
    eligible_shape.postable = true;
    if retained && crate::directory::purchase_line_allowed(&eligible_shape) && role.is_none() && paying_bank != Some(a.id) {
        return Ok(None);
    }
    if paying_bank == Some(a.id) {
        return Ok(Some(format!(
            "{} · {} is the bank this check is paid from; a check cannot also charge it.",
            a.code, a.name
        )));
    }
    if let Some(role) = role {
        return Ok(Some(control_message(a, &role)));
    }
    Ok(Some(format!(
        "{} · {}: Choose an active account this line may charge: an expense, cost, asset, liability or equity account. Receivables, payables, undeposited funds and collected sales tax are refused.",
        a.code, a.name
    )))
}
pub fn eligible_line_accounts(conn: &(impl fb_db::Query + ?Sized), company: CompanyId) -> FbResult<Vec<crate::model::Account>> {
    let mut result = Vec::new();
    for account in store::accounts(conn, company)? {
        if purchase_line_allowed(conn, company, &account, None)? {
            result.push(account);
        }
    }
    Ok(result)
}

pub fn control_message(a: &crate::model::Account, role: &str) -> String {
    let guide = match role {
        "receivables" => {
            "is a receivables (A/R) control account. Customer balances must tie out to it: record an invoice or a customer receipt instead (or a credit memo or refund)."
        }
        "payables" => "is a payables (A/P) control account. Vendor balances must tie out to it: record a bill or a bill payment instead.",
        "undeposited" => {
            "is an undeposited-funds control account. Record customer receipts or sales receipts to undeposited funds and move them to a bank with Make deposits instead."
        }
        "collected_tax" => {
            "is the collected sales tax control account. Sales tax is posted by invoices and sales receipts; a sales tax payment or adjustment screen is not built yet."
        }
        _ => {
            "is the customer deposits (prepayments) control account. Customer deposit balances must tie out to it: receive, apply or refund a customer deposit instead."
        }
    };
    format!("{} · {} {guide}", a.code, a.name)
}

// The database library owns savepoint control. The callback stays authorized,
// including when a caller catches a failed primitive and continues composing.
fn atomic_primitive<T>(tx: &Writer<'_>, f: impl FnOnce() -> FbResult<T>) -> FbResult<T> {
    tx.atomic(f)
}

fn without_id(v: &Value) -> Value {
    let mut out = v.clone();
    out.as_object_mut().unwrap().remove("id");
    out
}
pub fn setup_snapshot(conn: &(impl fb_db::Query + ?Sized), company: CompanyId) -> FbResult<Value> {
    Ok(without_id(&setup_record(conn, company)?))
}
