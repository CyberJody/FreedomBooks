//! Strict attachment dump schema and independent audit completeness checks.
use crate::dump_shape::Shape::{self, *};
use serde_json::Value;
use std::collections::BTreeSet;
const ROW: Shape = Object(&[
    ("ref", String),
    ("company", String),
    ("owner_kind", String),
    ("owner", String),
    ("original_name", String),
    ("content_type", String),
    ("size", Int),
    ("sha256", String),
    ("deleted", Bool),
    ("uploaded_by_name", String),
    ("deleted_by_name", String),
    ("delete_reason", String),
    ("request_key", String),
]);
pub(super) const SHAPE: Shape = Object(&[
    ("rows", Array(&ROW)),
    ("results", Detail),
    (
        "queries",
        Array(&Object(&[
            ("step", Int),
            ("op", String),
            ("ref", String),
            ("company", String),
            ("today", Nullable(&String)),
            ("days", Nullable(&Int)),
            ("attachment", Nullable(&String)),
            ("audit_count", Int),
        ])),
    ),
]);
fn digest(v: &Value) -> bool {
    v.as_str().is_some_and(|s| {
        s.len() == 64
            && s.bytes()
                .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
    })
}
pub(super) fn validate(v: &Value) -> Result<(), std::string::String> {
    validate_step_evidence(v)?;
    validate_results(v)?;
    let mut ids = BTreeSet::new();
    let rows = v["attachments"]["rows"]
        .as_array()
        .ok_or("attachment rows")?;
    let events = v["audit"]
        .as_object()
        .ok_or("attachment audit")?
        .values()
        .flat_map(|xs| xs.as_array().into_iter().flatten())
        .collect::<Vec<_>>();
    for r in rows {
        let id = r["ref"].as_str().ok_or("attachment ref")?;
        if !ids.insert(id)
            || !digest(&r["sha256"])
            || !r["size"].as_i64().is_some_and(|n| n > 0 && n <= 104857600)
            || !fb_core::attachments::sniff::TYPES
                .contains(&r["content_type"].as_str().unwrap_or(""))
            || !["journal", "document"].contains(&r["owner_kind"].as_str().unwrap_or(""))
        {
            return Err("invalid attachment fields".into());
        }
        let company = r["company"].as_str().ok_or("attachment company")?;
        let name = r["original_name"].as_str().ok_or("attachment name")?;
        let content_type = r["content_type"].as_str().ok_or("attachment type")?;
        let request = r["request_key"].as_str().ok_or("attachment request key")?;
        if fb_ids::parse_uuid(request).is_none_or(|u| u.to_string() != request)
            || !fb_core::attachments::sniff::display_name(name, content_type)
                .is_ok_and(|n| n == name)
            || !v["companies"]
                .as_array()
                .ok_or("companies")?
                .iter()
                .any(|c| c["ref"] == company)
        {
            return Err("attachment identity or name".into());
        }
        let owner = if r["owner_kind"] == "journal" {
            v["journals"]
                .as_array()
                .ok_or("journals")?
                .iter()
                .find(|j| j["ref"] == r["owner"] && j["company"] == company)
        } else {
            v["l2a"]["identities"]
                .as_array()
                .ok_or("identities")?
                .iter()
                .find(|d| d["id"] == r["owner"] && d["company_id"] == company)
        }
        .ok_or("attachment owner identity")?;
        let row_events = v["audit"][company]
            .as_array()
            .ok_or("attachment company audit")?;
        for (action, needed) in [
            ("attachment.added", true),
            ("attachment.removed", r["deleted"] == true),
        ] {
            let matches = row_events
                .iter()
                .filter(|e| e["action"] == action && e["detail"]["attachment"] == id)
                .collect::<Vec<_>>();
            if matches.len() != usize::from(needed) {
                return Err("attachment audit completeness".into());
            }
            if needed {
                let e = matches[0];
                if (r["owner_kind"] == "journal"
                    && e["target"] != format!("journal:{}", r["owner"].as_str().ok_or("owner")?))
                    || (r["owner_kind"] == "document" && e["detail"]["number"] != owner["number"])
                    || e["actor_name"]
                        != if action == "attachment.added" {
                            r["uploaded_by_name"].clone()
                        } else {
                            r["deleted_by_name"].clone()
                        }
                    || e["detail"]["name"] != r["original_name"]
                    || e["detail"]["size"] != r["size"]
                    || e["detail"]["sha256"] != r["sha256"]
                    || e["detail"]["content_type"] != r["content_type"]
                    || (action == "attachment.removed"
                        && e["detail"]["reason"] != r["delete_reason"])
                {
                    return Err("attachment audit payload".into());
                }
            }
        }
    }
    for e in events {
        if e["action"]
            .as_str()
            .is_some_and(|s| s.starts_with("attachment."))
            && !ids.contains(e["detail"]["attachment"].as_str().unwrap_or(""))
        {
            return Err("attachment omitted despite audit evidence".into());
        }
    }
    Ok(())
}

const REMINDER_ROW: Shape = Object(&[
    ("document_id", String),
    ("number", String),
    ("party", String),
    ("due_date", String),
    ("balance", String),
    ("days", Int),
    ("overdue", Bool),
    ("when", String),
]);
const GROUP: Shape = Object(&[
    ("count", Int),
    ("total", String),
    ("overdue_count", Int),
    ("overdue_total", String),
    ("capped", Bool),
    ("rows", Array(&REMINDER_ROW)),
]);
const BUILT: Shape = Object(&[
    ("today", String),
    ("days", Int),
    ("through", String),
    ("bills", GROUP),
    ("invoices", GROUP),
]);
fn money(v: &Value) -> Result<i128, std::string::String> {
    let s = v.as_str().ok_or("reminder money type")?;
    let (a, b) = s.split_once('.').ok_or("reminder money syntax")?;
    if a.is_empty() || b.len() != 2 || !a.bytes().chain(b.bytes()).all(|b| b.is_ascii_digit()) {
        return Err("reminder money syntax".into());
    }
    a.parse::<i128>()
        .ok()
        .and_then(|n| n.checked_mul(100))
        .and_then(|n| n.checked_add(b.parse::<i128>().ok()?))
        .ok_or("reminder money overflow".into())
}
fn day(v: &Value) -> Result<chrono::NaiveDate, std::string::String> {
    let s = v.as_str().ok_or("reminder date type")?;
    if fb_db::dates::parse_iso_date(s).as_deref() != Some(s) {
        return Err("noncanonical reminder date".into());
    }
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| "reminder date syntax".into())
}
fn input_day(v: &Value) -> Result<chrono::NaiveDate, std::string::String> {
    let normalized = fb_db::dates::parse_iso_date(v.as_str().ok_or("query date type")?)
        .ok_or("query date syntax")?;
    day(&Value::String(normalized))
}
fn validate_results(v: &Value) -> Result<(), std::string::String> {
    let queries = v["attachments"]["queries"]
        .as_array()
        .ok_or("attachment queries")?;
    let results = v["attachments"]["results"]
        .as_object()
        .ok_or("attachment results")?;
    let expected = v["outcomes"]
        .as_array()
        .ok_or("outcomes")?
        .iter()
        .filter(|o| {
            o["ok"] == true
                && ["reminder_build", "reminder_count", "attachment_download"]
                    .contains(&o["op"].as_str().unwrap_or(""))
        })
        .map(|o| o["step"].as_i64().ok_or("step"))
        .collect::<Result<BTreeSet<_>, _>>()?;
    let actual = queries
        .iter()
        .map(|q| q["step"].as_i64().ok_or("query step"))
        .collect::<Result<BTreeSet<_>, _>>()?;
    if expected != actual || actual.len() != queries.len() {
        return Err("query manifest completeness".into());
    }
    let keys = queries
        .iter()
        .map(|q| q["ref"].as_str().ok_or("query ref"))
        .collect::<Result<BTreeSet<_>, _>>()?;
    if keys.len() != queries.len() || keys != results.keys().map(|s| s.as_str()).collect() {
        return Err("query result key completeness".into());
    }
    for q in queries {
        let step = q["step"].as_i64().ok_or("query step")?;
        if !v["outcomes"]
            .as_array()
            .ok_or("outcomes")?
            .iter()
            .any(|o| o["step"] == step && o["ok"] == true && o["op"] == q["op"])
            || !v["companies"]
                .as_array()
                .ok_or("companies")?
                .iter()
                .any(|c| c["ref"] == q["company"])
            || !["reminder_build", "reminder_count", "attachment_download"]
                .contains(&q["op"].as_str().unwrap_or(""))
        {
            return Err("query identity".into());
        }
        let outcome = query_step(v, q)?;
        for field in ["ref", "company", "today", "days", "attachment"] {
            if q[field] != outcome["query"][field] {
                return Err(format!("query {field} disagrees with driver step input"));
            }
        }
        // Check the boundary even for selectors whose result happens to be empty.
        query_audit(v, q)?;
        let r = &results[q["ref"].as_str().ok_or("query ref")?];
        if q["op"] == "attachment_download" {
            validate_download(v, q, r)?;
            if !digest(r) {
                return Err("invalid download hash".into());
            }
            continue;
        }
        let source = reminder_source(v, q)?;
        let today = input_day(&q["today"])?;
        if q["op"] == "reminder_count" {
            if r.as_u64() != Some(source.iter().filter(|row| row.due < today).count() as u64) {
                return Err("nav reminder count disagrees with audit/source rows".into());
            }
            if !r.as_i64().is_some_and(|n| n >= 0) {
                return Err("invalid reminder count".into());
            }
            continue;
        }
        BUILT.validate(r, "reminder")?;
        let today = day(&r["today"])?;
        let days = r["days"].as_i64().ok_or("days")?;
        let default_days = query_audit(v, q)?
            .iter()
            .rev()
            .find(|e| e["action"] == "company.reminders_updated")
            .map(|e| {
                e["detail"]["after"]["bill_reminder_days"]
                    .as_i64()
                    .ok_or("reminder settings audit")
            })
            .transpose()?
            .unwrap_or(10);
        if days != q["days"].as_i64().unwrap_or(default_days) {
            return Err("reminder window disagrees with requested/audited settings".into());
        }
        if day(&r["today"])? != input_day(&q["today"])?
            || (!q["days"].is_null() && r["days"] != q["days"])
            || today.checked_add_signed(chrono::Duration::try_days(days).ok_or("days overflow")?)
                != Some(day(&r["through"])?)
        {
            return Err("reminder date boundary".into());
        }
        validate_reminder_source(&source, r)?;
        for kind in ["bills", "invoices"] {
            let g = &r[kind];
            let rows = g["rows"].as_array().ok_or("rows")?;
            let count = g["count"].as_i64().ok_or("count")?;
            let over = g["overdue_count"].as_i64().ok_or("overdue count")?;
            if count < 0
                || rows.len()
                    != usize::try_from(count.min(25)).map_err(|_| "reminder count bounds")?
                || count < rows.len() as i64
                || rows.len() > 25
                || g["capped"] != (count > rows.len() as i64)
                || over < 0
                || over > count
            {
                return Err("reminder counts".into());
            }
            let mut ids = BTreeSet::new();
            let mut total = 0i128;
            let mut ot = 0i128;
            let mut oc = 0i64;
            let mut previous = None;
            for row in rows {
                if !ids.insert(row["document_id"].as_str().ok_or("document id")?) {
                    return Err("duplicate reminder row".into());
                }
                let due = day(&row["due_date"])?;
                let delta = (due - today).num_days();
                let amount = money(&row["balance"])?;
                if amount <= 0
                    || row["days"] != delta
                    || row["overdue"] != (delta < 0)
                    || row["when"] != fb_core::reminders::when(delta)
                    || previous.is_some_and(|d| d > due)
                    || (kind == "invoices" && delta >= 0)
                    || (kind == "bills" && due > day(&r["through"])?)
                {
                    return Err("reminder row semantics".into());
                }
                previous = Some(due);
                total = total.checked_add(amount).ok_or("total overflow")?;
                if delta < 0 {
                    ot = ot.checked_add(amount).ok_or("total overflow")?;
                    oc += 1;
                }
            }
            let gt = money(&g["total"])?;
            let got = money(&g["overdue_total"])?;
            if gt < total
                || got < ot
                || over < oc
                || got > gt
                || (g["capped"] == false && (gt != total || got != ot || over != oc))
            {
                return Err("reminder aggregate semantics".into());
            }
        }
    }
    Ok(())
}

// Audit lengths are captured by the driver after each step, never reconstructed
// from the generated queries. In checkpoint dumps the last record is additionally
// checked against that checkpoint's complete audit history.
fn validate_step_evidence(v: &Value) -> Result<(), std::string::String> {
    let outcomes = v["outcomes"].as_array().ok_or("outcomes")?;
    let audit = v["audit"].as_object().ok_or("audit")?;
    let mut previous = std::collections::BTreeMap::new();
    for (index, outcome) in outcomes.iter().enumerate() {
        if outcome["step"].as_u64() != Some(index as u64) {
            return Err("driver step index out of sequence".into());
        }
        if let Some(history) = v["book_identity"]["history"].as_array()
            && !history.contains(&serde_json::json!(index)) { continue; }
        let counts = outcome["audit_counts"]
            .as_object()
            .ok_or("driver audit counts")?;
        for (company, count) in counts {
            let count = count.as_u64().ok_or("driver audit count bounds")?;
            let events = audit
                .get(company)
                .and_then(Value::as_array)
                .ok_or("driver audit company")?;
            if count > events.len() as u64 || previous.get(company).is_some_and(|p| *p > count) {
                return Err("driver audit counts outside step history".into());
            }
            previous.insert(company.clone(), count);
        }
        if previous.keys().any(|co| !counts.contains_key(co)) {
            return Err("driver audit company disappeared".into());
        }
        let needs_query = outcome["ok"] == true
            && ["reminder_build", "reminder_count", "attachment_download"]
                .contains(&outcome["op"].as_str().unwrap_or(""));
        if needs_query != outcome.get("query").is_some() {
            return Err("driver query input completeness".into());
        }
    }
    if previous.len() != audit.len()
        || audit.iter().any(|(co, events)| {
            previous.get(co).copied() != events.as_array().map(|xs| xs.len() as u64)
        })
    {
        return Err("driver audit counts disagree with checkpoint history".into());
    }
    Ok(())
}
fn query_step<'a>(v: &'a Value, q: &Value) -> Result<&'a Value, std::string::String> {
    let step = q["step"]
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .ok_or("query step")?;
    let outcome = v["outcomes"]
        .as_array()
        .and_then(|xs| xs.get(step))
        .ok_or("query step absent")?;
    if outcome["step"] != q["step"] || outcome["op"] != q["op"] || outcome["ok"] != true {
        return Err("query disagrees with driver step".into());
    }
    Ok(outcome)
}
fn query_audit<'a>(v: &'a Value, q: &Value) -> Result<&'a [Value], std::string::String> {
    let co = q["company"].as_str().ok_or("query company")?;
    let events = v["audit"][co].as_array().ok_or("query company audit")?;
    let outcome = query_step(v, q)?;
    let count = &outcome["audit_counts"][co];
    if q["audit_count"] != *count {
        return Err("query audit boundary disagrees with driver step".into());
    }
    let n = count
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .ok_or("query audit boundary")?;
    events
        .get(..n)
        .ok_or("query audit boundary outside evidence".into())
}
fn validate_download(v: &Value, q: &Value, r: &Value) -> Result<(), std::string::String> {
    let id = q["attachment"]
        .as_str()
        .ok_or("download attachment identity")?;
    let row = v["attachments"]["rows"]
        .as_array()
        .ok_or("attachment rows")?
        .iter()
        .find(|a| a["ref"] == id && a["company"] == q["company"])
        .ok_or("download attachment missing")?;
    let added = query_audit(v, q)?
        .iter()
        .find(|e| e["action"] == "attachment.added" && e["detail"]["attachment"] == id)
        .ok_or("download predates attachment audit")?;
    if *r != row["sha256"] || *r != added["detail"]["sha256"] {
        return Err("download hash disagrees with attachment and audit".into());
    }
    Ok(())
}
struct SourceRow {
    kind: &'static str,
    id: std::string::String,
    number: Value,
    party: std::string::String,
    due: chrono::NaiveDate,
    amount: i128,
    rank: usize,
}
fn reminder_source(v: &Value, q: &Value) -> Result<Vec<SourceRow>, std::string::String> {
    use std::collections::BTreeMap;
    if !q["attachment"].is_null() {
        return Err("reminder has attachment identity".into());
    }
    let events = query_audit(v, q)?;
    let mut documents = BTreeMap::new();
    let mut parties = BTreeMap::new();
    let mut live = BTreeMap::<std::string::String, Vec<(&str, std::string::String, i128)>>::new();
    for (rank, e) in events.iter().enumerate() {
        let action = e["action"].as_str().ok_or("audit action")?;
        let target = e["target"].as_str().ok_or("audit target")?;
        let d = &e["detail"];
        if let Some((kind, id)) = target.split_once(':') {
            if ["bill", "invoice"].contains(&kind) && d["revision"].is_i64() {
                let entry = documents.entry((kind, id)).or_insert((rank, e));
                entry.1 = e;
            }
            if ["vendor", "customer"].contains(&kind) && d["after"].is_object() {
                parties.insert((kind, id), d["after"].clone());
            }
        }
        match action {
            "payment.recorded" | "receipt.recorded" => {
                let (kind, key) = if action == "payment.recorded" {
                    ("bill", "bill_id")
                } else {
                    ("invoice", "invoice_id")
                };
                let mut allocations = vec![];
                for a in d["allocations"]
                    .as_array()
                    .ok_or("settlement audit allocations")?
                {
                    allocations.push((
                        kind,
                        a[key].as_str().ok_or("settlement document")?.to_owned(),
                        money(&a["amount"])?,
                    ));
                }
                live.insert(target.to_owned(), allocations);
            }
            "payment.reversed" | "receipt.reversed" => {
                live.remove(target);
            }
            "customer_deposit.applied" => {
                let key = format!(
                    "deposit:{}",
                    d["application_id"]
                        .as_str()
                        .ok_or("deposit application audit")?
                );
                live.insert(
                    key,
                    vec![(
                        "invoice",
                        d["invoice_id"]
                            .as_str()
                            .ok_or("deposit invoice audit")?
                            .to_owned(),
                        money(&d["amount"])?,
                    )],
                );
            }
            "credit_memo.applied" => {
                let key = format!(
                    "credit:{}",
                    d["application_id"]
                        .as_str()
                        .ok_or("credit application audit")?
                );
                let mut allocations = vec![];
                for a in d["invoices"].as_array().ok_or("credit invoice audit")? {
                    allocations.push((
                        "invoice",
                        a["invoice_id"].as_str().ok_or("credit invoice")?.to_owned(),
                        money(&a["amount"])?,
                    ));
                }
                live.insert(key, allocations);
            }
            "customer_deposit.unapplied" | "credit_memo.unapplied" => {
                let prefix = if action == "customer_deposit.unapplied" {
                    "deposit"
                } else {
                    "credit"
                };
                live.remove(&format!(
                    "{prefix}:{}",
                    d["application_id"].as_str().ok_or("unapplication audit")?
                ));
            }
            _ => {}
        }
    }
    let mut out = vec![];
    for ((kind, id), (rank, e)) in documents {
        let d = &e["detail"];
        if d["state"] != "OPEN" {
            continue;
        }
        let (revision, party_kind, party_id, total) = if kind == "bill" {
            let r = v["payables"]["billrevision"]
                .as_array()
                .ok_or("bill revisions")?
                .iter()
                .find(|r| {
                    r["bill_id"] == id
                        && r["number"] == d["revision"]
                        && r["company_snapshot"]["id"] == q["company"]
                })
                .ok_or("reminder bill revision absent from source")?;
            (
                r,
                "vendor",
                r["vendor_id"].as_str().ok_or("bill vendor")?,
                i128::from(r["total"].as_i64().ok_or("bill total")?),
            )
        } else {
            let invoice = v["sales"]["invoices"]
                .as_array()
                .ok_or("invoices")?
                .iter()
                .find(|r| r["ref"] == id)
                .ok_or("reminder invoice absent")?;
            let r = invoice["history"]
                .as_array()
                .ok_or("invoice revisions")?
                .iter()
                .find(|r| {
                    r["number"] == d["revision"] && r["company_snapshot"]["id"] == q["company"]
                })
                .ok_or("reminder invoice revision absent from source")?;
            (
                r,
                "customer",
                r["customer_id"].as_str().ok_or("invoice customer")?,
                money(&r["total"])?,
            )
        };
        if total != money(&d["total"])? || revision["state"] != d["state"] {
            return Err("reminder source revision disagrees with audit".into());
        }
        let mut balance = total;
        for (_, _, n) in live
            .values()
            .flatten()
            .filter(|(k, i, _)| *k == kind && i == id)
        {
            balance = balance
                .checked_sub(*n)
                .ok_or("reminder source balance overflow")?;
        }
        if balance <= 0 {
            continue;
        }
        let party = parties
            .get(&(party_kind, party_id))
            .ok_or("reminder party audit missing")?;
        let peers = parties
            .iter()
            .filter(|((k, _), p)| *k == party_kind && p["display_name"] == party["display_name"])
            .map(|(_, p)| p.clone())
            .collect::<Vec<_>>();
        let mut duplicates = BTreeSet::new();
        if peers.iter().filter(|p| p["active"] == true).count() > 1 {
            duplicates.insert(
                party["display_name"]
                    .as_str()
                    .ok_or("party label")?
                    .to_owned(),
            );
        }
        let label =
            fb_core::party_display::contact_list_label(party, &duplicates, Some(&peers), "");
        out.push(SourceRow {
            kind: if kind == "bill" { "bills" } else { "invoices" },
            id: id.into(),
            number: d["number"].clone(),
            party: label,
            due: day(&revision["due_date"])?,
            amount: balance,
            rank,
        });
    }
    Ok(out)
}
fn validate_reminder_source(source: &[SourceRow], r: &Value) -> Result<(), std::string::String> {
    let today = day(&r["today"])?;
    let through = day(&r["through"])?;
    for kind in ["bills", "invoices"] {
        let mut all = source
            .iter()
            .filter(|x| {
                x.kind == kind
                    && if kind == "bills" {
                        x.due <= through
                    } else {
                        x.due < today
                    }
            })
            .collect::<Vec<_>>();
        all.sort_by_key(|x| (x.due, x.rank));
        let g = &r[kind];
        let mut total = 0i128;
        let mut overdue_total = 0i128;
        let mut overdue_count = 0usize;
        for x in &all {
            total = total.checked_add(x.amount).ok_or("source total overflow")?;
            if x.due < today {
                overdue_total = overdue_total
                    .checked_add(x.amount)
                    .ok_or("source overdue overflow")?;
                overdue_count += 1;
            }
        }
        if g["count"].as_u64() != Some(all.len() as u64)
            || money(&g["total"])? != total
            || g["overdue_count"].as_u64() != Some(overdue_count as u64)
            || money(&g["overdue_total"])? != overdue_total
        {
            return Err(format!(
                "{kind} reminder totals/counts disagree with audit/source members"
            ));
        }
        let displayed = g["rows"].as_array().ok_or("reminder rows")?;
        if displayed.len() != all.len().min(25) {
            return Err("reminder source display cap".into());
        }
        for (x, row) in all.iter().take(25).zip(displayed) {
            if row["document_id"] != x.id
                || row["number"] != x.number
                || row["party"] != x.party
                || day(&row["due_date"])? != x.due
                || money(&row["balance"])? != x.amount
            {
                return Err(
                    "reminder displayed member disagrees with audit/source evidence".into(),
                );
            }
        }
    }
    Ok(())
}
