use crate::dump_shape::Shape::{self, *};
use serde_json::Value;
use std::collections::BTreeSet;
const REV: Shape = Object(&[
    ("date", String),
    ("reason", String),
    ("actor_name", String),
    ("journal_id", String),
]);
const PARTY: Shape = Object(&[
    ("id", String),
    ("display_name", String),
    ("business_name", String),
    ("contact_name", String),
    ("billing_address", String),
    ("email", String),
    ("phone", String),
]);
const COMPANY: Shape = Object(&[("id", String), ("name", String), ("currency", String)]);
pub(super) const SHAPE: Shape = Object(&[
    (
        "details",
        Array(&Object(&[
            ("ref", String),
            ("liability_id", String),
            ("estimate_id", Nullable(&String)),
            ("order_reference", String),
            ("applied", String),
            ("refunded", String),
            ("remaining", String),
            ("deposit_state", String),
        ])),
    ),
    (
        "applications",
        Array(&Object(&[
            ("ref", String),
            ("deposit_id", String),
            ("invoice_id", String),
            ("invoice_revision", Int),
            ("receivables_id", String),
            ("liability_id", String),
            ("date", String),
            ("amount", String),
            ("actor_name", String),
            ("journal_id", String),
            ("reversal", Nullable(&REV)),
        ])),
    ),
    (
        "refunds",
        Array(&Object(&[
            ("ref", String),
            ("deposit_id", String),
            ("customer_id", String),
            ("customer_snapshot", PARTY),
            ("company_snapshot", COMPANY),
            ("date", String),
            ("amount", String),
            ("bank_id", String),
            ("liability_id", String),
            ("method", String),
            ("check_number", String),
            ("memo", String),
            ("journal_id", String),
            ("actor_name", String),
            ("reversal", Nullable(&REV)),
        ])),
    ),
]);

fn rows(v: &Value) -> Result<&Vec<Value>, std::string::String> {
    v.as_array()
        .ok_or_else(|| "invalid prepayment collection".into())
}
fn string(v: &Value) -> Result<&str, std::string::String> {
    v.as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "invalid prepayment identity".into())
}
fn amount(v: &Value) -> Result<i64, std::string::String> {
    fb_db::prepayments::prepayment_cents(v).ok_or_else(|| "invalid prepayment money".into())
}
fn date(v: &Value) -> Result<&str, std::string::String> {
    let s = string(v)?;
    if fb_db::dates::parse_iso_date(s).as_deref() != Some(s) {
        return Err("invalid prepayment date".into());
    }
    Ok(s)
}
fn index<'a>(
    v: &'a Value,
    field: &str,
) -> Result<std::collections::BTreeMap<&'a str, &'a Value>, std::string::String> {
    let mut out = std::collections::BTreeMap::new();
    for r in rows(v)? {
        if out.insert(string(&r[field])?, r).is_some() {
            return Err("duplicate prepayment reference".into());
        }
    }
    Ok(out)
}
pub(super) fn validate(v: &Value) -> Result<(), std::string::String> {
    let details = index(&v["prepayments"]["details"], "ref")?;
    let applications = index(&v["prepayments"]["applications"], "ref")?;
    let refunds = index(&v["prepayments"]["refunds"], "ref")?;
    let receipts = index(&v["sales"]["receipts"], "ref")?;
    let invoices = index(&v["sales"]["invoices"], "ref")?;
    let accounts = index(&v["accounts"], "ref")?;
    let journals = index(&v["journals"], "ref")?;
    let companies = index(&v["companies"], "ref")?;
    let parties = index(&v["l2a"]["directory"], "id")?;
    let identities = index(&v["l2a"]["identities"], "id")?;
    let sources = rows(&v["l2a"]["source_journals"])?;
    let deposit_keys: BTreeSet<_> = receipts
        .iter()
        .filter(|(_, r)| r["kind"] == "CDP")
        .map(|(k, _)| *k)
        .collect();
    if deposit_keys != details.keys().copied().collect() {
        return Err("CDP receipts require exactly matching deposit details".into());
    }
    let mut reported_journals = BTreeSet::new();
    let account = |id: &Value, company: &Value, kind: &str| -> Result<(), std::string::String> {
        let a = accounts
            .get(string(id)?)
            .ok_or("missing prepayment account")?;
        if a["company"] != *company || a["kind"] != kind {
            return Err("wrong prepayment account ownership/type".into());
        }
        Ok(())
    };
    let mut posting =
        |row: &Value, company: &Value, kind: &str| -> Result<(), std::string::String> {
            let journal = string(&row["journal_id"])?;
            let j = journals.get(journal).ok_or("missing prepayment journal")?;
            if !reported_journals.insert(journal.to_owned())
                || j["company"] != *company
                || j["status"] != "POSTED"
                || j["date"] != row["date"]
                || !j["reversal_of"].is_null()
            {
                return Err("invalid prepayment journal ownership".into());
            }
            date(&row["date"])?;
            let source = sources
                .iter()
                .find(|p| p[1] == journal)
                .ok_or("missing prepayment journal source")?;
            let identity = identities
                .get(string(&source[0])?)
                .ok_or("missing prepayment identity")?;
            if identity["kind"] != kind || identity["company_id"] != *company {
                return Err("invalid prepayment document identity".into());
            }
            string(&row["actor_name"])?;
            if !row["reversal"].is_null() {
                let reversal = &row["reversal"];
                let rj = string(&reversal["journal_id"])?;
                let reverse = journals
                    .get(rj)
                    .ok_or("missing prepayment reversal journal")?;
                if !reported_journals.insert(rj.to_owned())
                    || reverse["reversal_of"] != journal
                    || reverse["company"] != *company
                    || reverse["status"] != "POSTED"
                    || reverse["date"] != reversal["date"]
                    || date(&reversal["date"])? < date(&row["date"])?
                    || !sources.iter().any(|p| p[0] == source[0] && p[1] == rj)
                {
                    return Err("invalid prepayment reversal link".into());
                }
                string(&reversal["reason"])?;
                string(&reversal["actor_name"])?;
            }
            Ok(())
        };
    let snapshots = |row: &Value, company: &Value| -> Result<(), std::string::String> {
        let customer = parties
            .get(string(&row["customer_id"])?)
            .ok_or("missing prepayment customer")?;
        if row["customer_snapshot"]["id"] != row["customer_id"]
            || customer["model"] != "customer"
            || customer["company_id"] != *company
            || row["company_snapshot"]["id"] != *company
            || !companies.contains_key(string(company)?)
        {
            return Err("invalid prepayment snapshot ownership".into());
        }
        let currency = string(&row["company_snapshot"]["currency"])?;
        if currency.len() != 3 || !currency.bytes().all(|c| c.is_ascii_uppercase()) {
            return Err("invalid prepayment snapshot currency".into());
        }
        Ok(())
    };
    for (key, detail) in &details {
        let receipt = receipts[key];
        let company = &receipt["company_snapshot"]["id"];
        snapshots(receipt, company)?;
        posting(receipt, company, "CDP")?;
        account(&detail["liability_id"], company, "LIABILITY")?;
        account(&receipt["destination_account_id"], company, "ASSET")?;
        if !detail["estimate_id"].is_null()
            || !receipt["sale"].is_null()
            || !rows(&receipt["allocations"])?.is_empty()
            || !rows(&receipt["lines"])?.is_empty()
            || !matches!(
                receipt["destination"].as_str(),
                Some("BANK" | "UNDEPOSITED")
            )
            || !matches!(
                receipt["method"].as_str(),
                Some("CHECK" | "CASH" | "CARD" | "TRANSFER" | "OTHER")
            )
        {
            return Err("invalid deposit type or children".into());
        }
        let total = amount(&receipt["amount"])?;
        if total == 0 {
            return Err("nonpositive customer deposit".into());
        }
        let sum = |collection: &std::collections::BTreeMap<&str,&Value>| -> Result<i64,std::string::String> {
            collection.values().filter(|a| a["deposit_id"]==*key && a["reversal"].is_null()).try_fold(0_i64, |n,a| n.checked_add(amount(&a["amount"])?).ok_or_else(|| "prepayment amount overflow".into()))
        };
        let applied = sum(&applications)?;
        let refunded = sum(&refunds)?;
        let used = applied
            .checked_add(refunded)
            .ok_or("prepayment amount overflow")?;
        if used > total || (!receipt["reversal"].is_null() && used != 0) {
            return Err("deposit overspent or void with live use".into());
        }
        let remaining = if receipt["reversal"].is_null() {
            total - used
        } else {
            0
        };
        let state = if !receipt["reversal"].is_null() {
            "VOID"
        } else if remaining == 0 {
            "USED"
        } else if used > 0 {
            "PARTIAL"
        } else {
            "AVAILABLE"
        };
        if amount(&detail["applied"])? != applied
            || amount(&detail["refunded"])? != refunded
            || amount(&detail["remaining"])? != remaining
            || detail["deposit_state"] != state
        {
            return Err("invalid deposit balances/state".into());
        }
    }
    for (kind, collection) in [("CDA", &applications), ("CDR", &refunds)] {
        for row in collection.values() {
            let deposit = string(&row["deposit_id"])?;
            let detail = details
                .get(deposit)
                .ok_or("missing parent deposit detail")?;
            let receipt = receipts[deposit];
            let company = &receipt["company_snapshot"]["id"];
            if amount(&row["amount"])? == 0
                || date(&row["date"])? < date(&receipt["date"])?
                || row["liability_id"] != detail["liability_id"]
            {
                return Err("invalid prepayment parent/amount/date".into());
            }
            account(&row["liability_id"], company, "LIABILITY")?;
            posting(row, company, kind)?;
            if kind == "CDA" {
                let inv = invoices
                    .get(string(&row["invoice_id"])?)
                    .ok_or("missing prepayment invoice")?;
                let revision = row["invoice_revision"]
                    .as_i64()
                    .filter(|n| *n > 0)
                    .ok_or("invalid invoice revision")?;
                let history = rows(&inv["history"])?;
                let h = history
                    .iter()
                    .find(|h| h["number"] == revision)
                    .ok_or("missing invoice revision")?;
                if h["state"] != "OPEN"
                    || h["customer_id"] != receipt["customer_id"]
                    || h["company_snapshot"]["id"] != *company
                    || date(&row["date"])? < date(&h["date"])?
                {
                    return Err("invalid prepayment invoice ownership/state".into());
                }
                account(&row["receivables_id"], company, "ASSET")?;
            } else {
                snapshots(row, company)?;
                account(&row["bank_id"], company, "ASSET")?;
                let method = string(&row["method"])?;
                let number = row["check_number"]
                    .as_str()
                    .ok_or("invalid refund number")?;
                if row["customer_id"] != receipt["customer_id"]
                    || !["CHECK", "CASH", "OTHER"].contains(&method)
                    || (method != "CHECK" && !number.is_empty())
                    || fb_db::prepayments::validate_prepayment_number(number) != Ok(number)
                {
                    return Err("invalid refund method/number/customer".into());
                }
            }
        }
    }
    let mut expected_journals = BTreeSet::new();
    for pair in sources {
        if let Some(identity) = pair[0].as_str().and_then(|s| identities.get(s))
            && matches!(identity["kind"].as_str(), Some("CDP" | "CDA" | "CDR"))
        {
            expected_journals.insert(string(&pair[1])?.to_owned());
        }
    }
    if reported_journals != expected_journals {
        return Err("incomplete prepayment lifecycle collections".into());
    }
    Ok(())
}
