//! Validate each dump against independent completion, ledger and frozen-line evidence.
//! Internally consistent report totals alone cannot prove that rows are complete.
use serde_json::Value;
use std::collections::BTreeSet;

fn rows<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v[key].as_array().map(Vec::as_slice).unwrap_or(&[])
}
fn money(v: &Value) -> Result<i64, String> {
    fb_db::banking_evidence::money(v, false).ok_or_else(|| "Invalid reconciliation money".into())
}
fn add(a: i64, b: i64) -> Result<i64, String> {
    a.checked_add(b)
        .ok_or_else(|| "Reconciliation total overflow".into())
}
fn ledger_line<'a>(dump: &'a Value, id: &str) -> Result<(&'a Value, &'a Value), String> {
    let (journal, position) = id.rsplit_once(':').ok_or("Invalid ledger line reference")?;
    let position: u64 = position.parse().map_err(|_| "Invalid ledger position")?;
    let journal = rows(dump, "journals")
        .iter()
        .find(|j| j["ref"] == journal)
        .ok_or("Missing reconciliation ledger journal")?;
    let line = rows(journal, "lines")
        .iter()
        .find(|l| l["position"].as_u64() == Some(position))
        .ok_or("Missing reconciliation ledger line")?;
    Ok((journal, line))
}

pub(super) fn validate(dump: &Value) -> Result<(), String> {
    let banking = &dump["banking"];
    let recs = rows(banking, "reconciliations");
    let lines = rows(banking, "reconciliation_lines");
    let events = dump["audit"]
        .as_object()
        .ok_or("Missing audit evidence")?
        .iter()
        .flat_map(|(company, events)| rows_in(events).map(move |e| (company.as_str(), e)))
        .filter(|(_, e)| e["action"] == "reconciliation.completed")
        .collect::<Vec<_>>();

    let expected = recs
        .iter()
        .filter(|r| r["status"] != "OPEN")
        .map(|r| {
            (
                r["company_id"].as_str().unwrap_or(""),
                r["id"].as_str().unwrap_or(""),
            )
        })
        .collect::<BTreeSet<_>>();
    let actual = events
        .iter()
        .map(|(company, e)| {
            e["target"]
                .as_str()
                .and_then(|s| s.strip_prefix("reconciliation:"))
                .map(|id| (*company, id))
                .ok_or("Invalid reconciliation audit target")
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    if actual != expected || events.len() != expected.len() {
        return Err("Incomplete reconciliation completion audit coverage".into());
    }
    for line in lines {
        if !recs
            .iter()
            .any(|r| r["id"] == line["reconciliation_id"] && r["status"] != "OPEN")
        {
            return Err("Orphan reconciliation line".into());
        }
    }
    for rec in recs {
        let saved = lines
            .iter()
            .filter(|l| l["reconciliation_id"] == rec["id"])
            .collect::<Vec<_>>();
        if rec["status"] == "OPEN" {
            if !saved.is_empty() {
                return Err("Open reconciliation has frozen lines".into());
            }
            continue;
        }
        let target = format!(
            "reconciliation:{}",
            rec["id"].as_str().ok_or("Invalid reconciliation ID")?
        );
        let (_, event) = events
            .iter()
            .find(|(company, e)| rec["company_id"] == *company && e["target"] == target)
            .ok_or("Missing reconciliation completion event")?;
        let detail = &event["detail"];
        for field in [
            "account_id",
            "statement_date",
            "beginning_balance",
            "ending_balance",
            "cleared_debit",
            "cleared_credit",
            "cleared_count",
        ] {
            if detail[field] != rec[field] {
                return Err(format!("Reconciliation audit mismatch: {field}"));
            }
        }
        let mut audited = detail["line_ids"]
            .as_array()
            .ok_or("Missing audited reconciliation lines")?
            .iter()
            .map(|id| id.as_str().ok_or("Invalid audited line ID"))
            .collect::<Result<Vec<_>, _>>()?;
        let mut ids = saved
            .iter()
            .map(|l| {
                l["line_id"]
                    .as_str()
                    .ok_or("Invalid reconciliation line ID")
            })
            .collect::<Result<Vec<_>, _>>()?;
        audited.sort_unstable();
        ids.sort_unstable();
        if ids != audited
            || ids.iter().copied().collect::<BTreeSet<_>>().len() != ids.len()
            || rec["cleared_count"].as_u64() != Some(saved.len() as u64)
        {
            return Err("Incomplete reconciliation line membership".into());
        }
        let (mut debit, mut credit) = (0, 0);
        for line in saved {
            let (journal, ledger) =
                ledger_line(dump, line["line_id"].as_str().ok_or("Invalid line ID")?)?;
            let (d, c) = (money(&line["debit"])?, money(&line["credit"])?);
            if journal["status"] != "POSTED"
                || journal["company"] != rec["company_id"]
                || ledger["account"] != rec["account_id"]
                || ledger["debit"].as_i64() != Some(d)
                || ledger["credit"].as_i64() != Some(c)
            {
                return Err("Reconciliation line differs from ledger evidence".into());
            }
            debit = add(debit, d)?;
            credit = add(credit, c)?;
        }
        if money(&rec["cleared_debit"])? != debit || money(&rec["cleared_credit"])? != credit {
            return Err("Reconciliation frozen totals differ from line evidence".into());
        }
    }

    for report in banking["reports"]
        .as_object()
        .ok_or("Missing reports")?
        .values()
    {
        let rec = recs
            .iter()
            .find(|r| r["id"] == report["reconciliation_id"])
            .ok_or("Report references a missing reconciliation")?;
        let account = rows(dump, "accounts")
            .iter()
            .find(|a| a["ref"] == rec["account_id"] && a["company"] == rec["company_id"])
            .ok_or("Report references a missing account")?;
        let card = matches!(
            account["kind"].as_str(),
            Some("LIABILITY" | "EQUITY" | "INCOME")
        );
        if report["words"]["card"] != card {
            return Err("Invalid report account side".into());
        }
        let saved = lines
            .iter()
            .filter(|l| l["reconciliation_id"] == rec["id"])
            .collect::<Vec<_>>();
        for side in rows(report, "sides") {
            let key = side["key"].as_str().ok_or("Invalid report side")?;
            let amount_key = if key == "left" { "credit" } else { "debit" };
            let mut expected = BTreeSet::new();
            let mut total = 0;
            for line in &saved {
                let amount = money(&line[amount_key])?;
                if amount > 0 {
                    expected.insert(line["line_id"].as_str().ok_or("Invalid frozen line")?);
                    total = add(total, amount)?;
                }
            }
            let mut actual = BTreeSet::new();
            for group in rows(side, "groups") {
                for row in rows(group, "rows") {
                    let id = row["line_id"].as_str().ok_or("Invalid report line ID")?;
                    let (journal, ledger) = ledger_line(dump, id)?;
                    if !expected.contains(id)
                        || !actual.insert(id)
                        || row["date"] != journal["date"]
                        || ledger["account"] != rec["account_id"]
                        || journal["company"] != rec["company_id"]
                        || money(&row[key])?
                            != ledger[amount_key].as_i64().ok_or("Invalid ledger amount")?
                    {
                        return Err("Report row differs from reconciliation/ledger evidence".into());
                    }
                }
            }
            if actual != expected
                || side["count"].as_u64() != Some(expected.len() as u64)
                || money(&side["total"])? != total
            {
                return Err("Incomplete report membership or amounts".into());
            }
        }
    }
    Ok(())
}
fn rows_in(v: &Value) -> impl Iterator<Item = &Value> {
    v.as_array().into_iter().flatten()
}
