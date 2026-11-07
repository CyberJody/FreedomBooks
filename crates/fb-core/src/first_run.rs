//! Frozen documents.first_run: live checklist, bounded chart proposal and atomic fill/dismiss.
use crate::model::{Account, DetailType};
use crate::{CompanyId, FbError, FbResult, UserId, documents, services, store};
use fb_db::{Book, Query, Writer};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::LazyLock;

pub const BANK_LIMIT: usize = 20;
const ROLES: &[(&str, &str)] = &[
    ("receivables", "Money customers owe you"),
    ("payables", "Money you owe vendors"),
    ("undeposited", "Undeposited funds"),
    ("collected_tax", "Sales tax you have collected"),
    ("banks", "Bank accounts"),
];

pub fn checklist(tx: &(impl Query + ?Sized), company: CompanyId) -> FbResult<Value> {
    let has =
        |sql: &str| -> FbResult<bool> { Ok(tx.query_row(sql, [company.to_db()], |r| r.get(0))?) };
    let chart = has(
        "SELECT EXISTS(SELECT 1 FROM ledger_account WHERE company_id=?1 AND active=1 AND kind='INCOME') AND EXISTS(SELECT 1 FROM ledger_account WHERE company_id=?1 AND active=1 AND kind='EXPENSE')",
    )?;
    let special = has(
        "SELECT EXISTS(SELECT 1 FROM documents_accountingsetup s WHERE company_id=?1 AND receivables_id IS NOT NULL AND payables_id IS NOT NULL AND undeposited_id IS NOT NULL AND collected_tax_id IS NOT NULL AND EXISTS(SELECT 1 FROM documents_designatedbank b WHERE b.setup_id=s.id))",
    )?;
    let tax = has(
        "SELECT EXISTS(SELECT 1 FROM salestax_salestaxsettings WHERE company_id=?1 AND revision>=1)",
    )?;
    let customer =
        has("SELECT EXISTS(SELECT 1 FROM directory_customer WHERE company_id=?1 AND active=1)")?;
    let vendor =
        has("SELECT EXISTS(SELECT 1 FROM directory_vendor WHERE company_id=?1 AND active=1)")?;
    let item = has("SELECT EXISTS(SELECT 1 FROM directory_item WHERE company_id=?1 AND active=1)")?;
    Ok(
        json!({"chart":chart,"special":special,"tax":tax,"customer":customer,"vendor":vendor,"item":item}),
    )
}

fn slice(
    tx: &(impl Query + ?Sized),
    company: CompanyId,
    detail: DetailType,
    limit: usize,
    eligible: bool,
) -> FbResult<Vec<Account>> {
    let mut st = tx.prepare(&format!("SELECT {} FROM ledger_account WHERE company_id=?1 AND detail_type=?2 AND {} ORDER BY code COLLATE fb_text,id LIMIT ?3", store::ACCOUNT_COLS, if eligible {"active=1 AND postable=1"} else {"NOT(active=1 AND postable=1)"}))?;
    Ok(st
        .query_map(
            fb_db::rusqlite::params![company.to_db(), detail.as_str(), limit as i64],
            store::account_row,
        )?
        .collect::<Result<Vec<_>, _>>()?)
}

fn absent(label: &str, inactive: bool) -> String {
    if inactive {
        format!(
            "The {label} account is inactive or cannot be posted to. Turn it on, or pick one in Accounting setup."
        )
    } else {
        format!("No {label} account is in the chart yet.")
    }
}

// The regexes contain ASCII words, Python Unicode whitespace and Python word boundaries. Fold only
// that alphabet; preserve all other Python-word characters as x and all non-word characters as !.
// The alphanumeric table is generated from the frozen Python, never Rust's Unicode version.
fn name_alphabet(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            'İ' | 'ı' => 'i',
            'ſ' => 's',
            'K' => 'k',
            c if c.is_ascii_alphabetic() => c.to_ascii_lowercase(),
            c if fb_money::py_isspace(c) => ' ',
            '&' => '&',
            c if c == '_' || super::first_run_words::is_alnum(c) => 'x',
            _ => '!',
        })
        .collect()
}

pub fn sales_tax_name(name: &str) -> bool {
    static DENY: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"\b(?:payroll|income|property|withholding|withheld|franchise|excise|unemployment|fica|futa|suta|medicare|social\s+security|employee|employer)s?\b").unwrap()
    });
    static ALLOW: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"(?:\bsales\s*(?:(?:&|and)\s*use\s*)?tax(?:es)?\b|\btax(?:es)?\s+collected\b|\bcollected\s+(?:sales\s+)?tax(?:es)?\b|\b(?:gst|hst|pst|qst|vat)\b)").unwrap()
    });
    let text = name_alphabet(name);
    !DENY.is_match(&text) && ALLOW.is_match(&text)
}

fn undeposited_name(name: &str) -> bool {
    static RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\bundeposited\b").unwrap());
    RE.is_match(&name_alphabet(name))
}

pub fn propose(tx: &(impl Query + ?Sized), company: CompanyId) -> FbResult<Value> {
    let mut chosen = json!({});
    let mut reasons = json!({});
    for (role, detail, label, pattern) in [
        (
            "receivables",
            DetailType::Receivable,
            "Accounts receivable",
            None,
        ),
        ("payables", DetailType::Payable, "Accounts payable", None),
        (
            "undeposited",
            DetailType::OtherCurrentAsset,
            "undeposited funds",
            Some(undeposited_name as fn(&str) -> bool),
        ),
        (
            "collected_tax",
            DetailType::OtherCurrentLiability,
            "sales tax",
            Some(sales_tax_name as fn(&str) -> bool),
        ),
    ] {
        let limit = if pattern.is_some() { BANK_LIMIT + 1 } else { 3 };
        let rows = slice(tx, company, detail, limit, true)?;
        let matched: Vec<_> = rows
            .iter()
            .filter(|a| pattern.is_none_or(|p| p(&a.name)))
            .collect();
        let reason = if pattern.is_some() && rows.len() >= limit {
            Some(format!(
                "Too many {label} accounts to choose from automatically. Pick one in Accounting setup."
            ))
        } else if matched.len() == 1 {
            chosen[role] = json!(matched[0].id.to_db());
            None
        } else if matched.is_empty() {
            let inactive = slice(
                tx,
                company,
                detail,
                if pattern.is_some() { BANK_LIMIT + 1 } else { 1 },
                false,
            )?
            .iter()
            .any(|a| pattern.is_none_or(|p| p(&a.name)));
            Some(absent(label, inactive))
        } else {
            Some(format!(
                "More than one {label} account is in the chart. Pick one in Accounting setup."
            ))
        };
        if let Some(reason) = reason {
            reasons[role] = json!(reason);
        }
    }
    let banks = slice(tx, company, DetailType::Bank, BANK_LIMIT + 1, true)?;
    if banks.len() > BANK_LIMIT {
        reasons["banks"] = json!(format!(
            "More than {BANK_LIMIT} bank accounts are in the chart. Pick the ones to use in Accounting setup."
        ));
    } else if banks.is_empty() {
        reasons["banks"] = json!(absent(
            "bank",
            !slice(tx, company, DetailType::Bank, 1, false)?.is_empty()
        ));
    } else {
        chosen["banks"] = json!(banks.iter().map(|a| a.id.to_db()).collect::<Vec<_>>());
    }
    let revision = documents::setup_snapshot(tx, company)?["revision"]
        .as_i64()
        .unwrap();
    let digest = proposal_digest(company, revision, &chosen);
    let rows:Vec<Value>=ROLES.iter().map(|(role,label)| {
        let ids:Vec<&str>=if *role=="banks"{chosen["banks"].as_array().map(|v|v.iter().filter_map(Value::as_str).collect()).unwrap_or_default()}else{chosen[*role].as_str().into_iter().collect()};
        let accounts=ids.iter().map(|id|store::account(tx,crate::AccountId::parse(id).unwrap()).map(|a|a.map(|a|a.label()))).collect::<FbResult<Vec<_>>>()?;
        Ok(json!({"role":role,"label":label,"accounts":accounts.into_iter().flatten().collect::<Vec<_>>(),"reason":reasons[*role]}))
    }).collect::<FbResult<_>>()?;
    Ok(
        json!({"empty":chosen.as_object().unwrap().is_empty(),"chosen":chosen,"reasons":reasons,"revision":revision,"digest":digest,"rows":rows}),
    )
}

pub fn proposal_digest(company: CompanyId, revision: i64, chosen: &Value) -> String {
    let payload = json!({"company":company.to_db(),"revision":revision,"receivables":chosen["receivables"],"payables":chosen["payables"],"undeposited":chosen["undeposited"],"collected_tax":chosen["collected_tax"],"banks":chosen.get("banks").cloned().unwrap_or(json!([]))});
    // All digest values are ASCII identifiers, numbers or null; Python's compact sorted JSON.
    let sorted: std::collections::BTreeMap<_, _> = payload.as_object().unwrap().iter().collect();
    format!("{:x}", Sha256::digest(serde_json::to_vec(&sorted).unwrap()))
}

fn fill_tx(
    tx: &Writer<'_>,
    actor: UserId,
    company: CompanyId,
    values: &Value,
    revision: Option<i64>,
) -> FbResult<Value> {
    let user = services::access(tx, actor, company)?;
    for role in ["receivables", "payables", "undeposited", "collected_tax"] {
        if let Some(id) = values[role].as_str() {
            let a = crate::AccountId::parse(id)
                .map(|a| store::account(tx, a))
                .transpose()?
                .flatten();
            if a.is_none_or(|a| a.company_id != company) {
                return Err(FbError::validation("Choose an account from this company."));
            }
        }
    }
    for id in values["banks"].as_array().into_iter().flatten() {
        let a = id
            .as_str()
            .and_then(crate::AccountId::parse)
            .map(|a| store::account(tx, a))
            .transpose()?
            .flatten();
        if a.is_none_or(|a| a.company_id != company) {
            return Err(FbError::validation("Choose an account from this company."));
        }
    }
    let saved = documents::save_setup_tx(tx, actor, company, values, revision)?;
    let id: String = tx.query_row(
        "SELECT id FROM documents_accountingsetup WHERE company_id=?1",
        [company.to_db()],
        |r| r.get(0),
    )?;
    services::audit(
        tx,
        &user,
        company,
        "setup.filled",
        format!("accountingsetup:{id}"),
        json!({"filled":values}),
    )?;
    Ok(saved)
}

pub fn fill_setup(
    book: &mut Book,
    actor: UserId,
    company: CompanyId,
    values: &Value,
    revision: Option<i64>,
) -> FbResult<Value> {
    book.write(|tx| fill_tx(tx, actor, company, values, revision))
}

pub fn apply_proposal(
    book: &mut Book,
    actor: UserId,
    company: CompanyId,
    digest: &str,
    revision: Option<i64>,
) -> FbResult<Value> {
    book.write(|tx| {
        services::access(tx,actor,company)?;
        let current=propose(tx,company)?;
        if current["digest"].as_str()!=Some(digest) || current["revision"].as_i64()!=revision {
            return Err(FbError::validation("The chart or Accounting setup changed. Review the fill again before saving."));
        }
        if current["empty"]==true {return Err(FbError::validation("Nothing on the chart is clear enough to fill. Set the accounts in Accounting setup."));}
        let snapshot=documents::setup_snapshot(tx,company)?;
        let mut values=json!({});
        for role in ["receivables","payables","undeposited","collected_tax","banks"] {values[role]=current["chosen"].get(role).unwrap_or(&snapshot[role]).clone();}
        fill_tx(tx,actor,company,&values,revision)
    })
}

pub fn dismiss_first_run(
    book: &mut Book,
    actor: UserId,
    company: CompanyId,
) -> FbResult<crate::model::Company> {
    book.write(|tx| {
        let user = services::access(tx, actor, company)?;
        let mut c = store::get_company(tx, company)?;
        if c.first_run_open {
            c.first_run_open = false;
            store::update_company(tx, &c)?;
            services::audit(
                tx,
                &user,
                company,
                "setup.checklist.dismissed",
                format!("company:{company}"),
                json!({}),
            )?;
        }
        Ok(c)
    })
}
