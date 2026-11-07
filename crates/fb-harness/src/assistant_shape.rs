//! Validate syntax AND independent receipt-audit completeness, including identical malformed dumps.
use crate::dump_shape::Shape::{self, *};
use serde_json::Value;
use std::collections::BTreeSet;
const CONNECTION: Shape = Object(&[
    ("company", String),
    ("mode", String),
    ("status", String),
    ("credential_present", Bool),
    ("account_email", String),
    ("key_last4", String),
    ("model", String),
    (
        "model_choices",
        Array(&Object(&[("slug", String), ("display_name", String)])),
    ),
    ("plan_usage", Bool),
    ("notice_pending", Bool),
    ("notice_shown", Bool),
    ("read_attachments", Bool),
    ("revoke_confirmed", Nullable(&Bool)),
    ("last_error", String),
    ("token_expires_at", Nullable(&Int)),
    ("last_used_at", Nullable(&Int)),
]);
const READING: Shape = Object(&[
    ("id", String),
    ("company", String),
    ("created_by_id", String),
    ("request_key", String),
    ("bill_id", Nullable(&String)),
    ("original_name", String),
    ("content_type", String),
    ("size", Int),
    ("sha256", String),
    ("status", String),
    ("mode", String),
    ("model", String),
    ("result", Detail),
    ("error", String),
    ("created_at", Int),
    ("finished_at", Nullable(&Int)),
]);
pub(super) const SHAPE: Shape = Object(&[
    ("connections", Array(&CONNECTION)),
    ("readings", Array(&READING)),
    ("results", Map(&Detail)),
]);
const INITIAL: Shape = Object(&[
    (
        "initial",
        Object(&[
            ("reference", String),
            ("vendor", Nullable(&String)),
            ("date", Optional(&String)),
            ("due_date", Optional(&String)),
            ("terms", Optional(&String)),
        ]),
    ),
    (
        "lines",
        Array(&Object(&[
            ("description", String),
            ("quantity", String),
            ("rate", String),
            ("ai_line", Int),
            ("expense_account", Nullable(&String)),
            ("amount", String),
            ("mismatch", Bool),
            ("product", Optional(&String)),
        ])),
    ),
    ("marked", Array(&String)),
    ("notes", Array(&String)),
    ("warnings", Array(&String)),
    (
        "vendor",
        Object(&[
            ("name", String),
            ("matched", Nullable(&String)),
            ("candidates", Array(&String)),
            (
                "new_vendor",
                Object(&[("display_name", String), ("billing_address", String)]),
            ),
        ]),
    ),
    ("mismatched", Bool),
    ("file_name", String),
    ("total", String),
    ("subtotal", String),
    ("tax", String),
]);
pub(super) fn validate(v: &Value) -> Result<(), std::string::String> {
    let a = &v["assistant"];
    let mut ids = BTreeSet::new();
    let mut requests = BTreeSet::new();
    let mut connections = BTreeSet::new();
    let companies = v["companies"]
        .as_array()
        .ok_or("companies")?
        .iter()
        .map(|c| c["ref"].as_str().ok_or("company reference"))
        .collect::<Result<BTreeSet<_>, _>>()?;
    for c in a["connections"].as_array().ok_or("assistant connections")? {
        if !companies.contains(c["company"].as_str().ok_or("assistant company")?)
            || !connections.insert(c["company"].as_str().ok_or("assistant company")?)
            || !["siwc", "api_key"].contains(&c["mode"].as_str().unwrap_or(""))
            || !["connected", "disconnected", "needs_signin"]
                .contains(&c["status"].as_str().unwrap_or(""))
            || (c["status"] == "disconnected" && c["credential_present"] != false)
            || !fb_db::assistant::choices(&c["model_choices"])
            || !fb_db::assistant::selection(c["model"].as_str().unwrap_or(""), &c["model_choices"])
            || ["token_expires_at", "last_used_at"]
                .iter()
                .any(|k| !c[*k].is_null() && c[*k].as_i64().is_none_or(|n| n < 0))
        {
            return Err("invalid assistant connection".into());
        }
    }
    for r in a["readings"].as_array().ok_or("assistant readings")? {
        let id = r["id"].as_str().ok_or("assistant id")?;
        if !ids.insert(id)
            || !requests.insert((r["company"].to_string(), r["request_key"].to_string()))
            || !connections.contains(r["company"].as_str().ok_or("assistant company")?)
            || !fb_db::banking_evidence::canonical_uuid(r["request_key"].as_str().unwrap_or(""))
            || !["pending", "ok", "failed"].contains(&r["status"].as_str().unwrap_or(""))
            || !["", "siwc", "api_key"].contains(&r["mode"].as_str().unwrap_or(""))
            || r["size"].as_i64().is_none_or(|n| n <= 0 || n > 10_485_760)
            || r["sha256"].as_str().is_none_or(|s| {
                s.len() != 64
                    || !s
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
        {
            return Err("invalid assistant reading".into());
        }
        let created = r["created_at"]
            .as_i64()
            .filter(|n| *n >= 0)
            .ok_or("invalid receipt created clock")?;
        if !["image/png", "image/jpeg", "image/webp", "application/pdf"]
            .contains(&r["content_type"].as_str().unwrap_or(""))
            || (!r["finished_at"].is_null()
                && r["finished_at"].as_i64().is_none_or(|n| n < created))
            || (r["status"] == "pending" && (!r["finished_at"].is_null() || r["error"] != ""))
        {
            return Err("invalid receipt lifecycle".into());
        }
        if let Some(bid) = r["bill_id"].as_str()
            && !v["payables"]["bill"]
                .as_array()
                .ok_or("bills")?
                .iter()
                .any(|b| b["id"] == bid && b["company_id"] == r["company"])
        {
            return Err("invalid receipt bill ownership".into());
        }
        if r["status"] == "ok" {
            if !fb_db::assistant::result(&r["result"]) || r["finished_at"].is_null() {
                return Err("invalid successful receipt evidence".into());
            }
            let events = v["audit"][r["company"].as_str().ok_or("company")?]
                .as_array()
                .ok_or("company audit")?;
            let events = events
                .iter()
                .filter(|e| e["action"] == "ai.receipt_read" && e["detail"]["reading"] == id)
                .collect::<Vec<_>>();
            if events.len() != 1
                || events[0]["detail"]["lines"].as_u64()
                    != r["result"]["lines"].as_array().map(|a| a.len() as u64)
                || events[0]["detail"]["notes"].as_u64()
                    != r["result"]["notes"].as_array().map(|a| a.len() as u64)
            {
                return Err("receipt completeness differs from audit".into());
            }
        } else if r["result"] != serde_json::json!({}) {
            return Err("unfinished reading has result".into());
        }
    }
    for (company, events) in v["audit"].as_object().ok_or("audit")? {
        for e in events.as_array().ok_or("events")? {
            if e["action"].as_str().is_some_and(|s| s.starts_with("ai."))
                && !connections.contains(company.as_str())
            {
                return Err("audit names omitted assistant connection".into());
            }
            if ["ai.receipt_read", "ai.receipt_failed", "bill.ai_lines_kept"]
                .contains(&e["action"].as_str().unwrap_or(""))
                && !ids.contains(
                    e["detail"]["reading"]
                        .as_str()
                        .ok_or("receipt audit reading")?,
                )
            {
                return Err("audit names omitted receipt reading".into());
            }
        }
    }
    let results = a["results"].as_object().ok_or("assistant results")?;
    let expected = v["outcomes"]
        .as_array()
        .ok_or("outcomes")?
        .iter()
        .filter(|o| {
            o["ok"] == true
                && ["assistant_parse", "assistant_initial", "assistant_queue"]
                    .contains(&o["op"].as_str().unwrap_or(""))
        })
        .map(|o| {
            o["result_ref"]
                .as_str()
                .ok_or("missing assistant result evidence")
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    if results
        .keys()
        .map(std::string::String::as_str)
        .collect::<BTreeSet<_>>()
        != expected
    {
        return Err("assistant result set differs from successful requests".into());
    }
    for value in a["results"]
        .as_object()
        .ok_or("assistant results")?
        .values()
    {
        if value.get("vendor_name").is_some() {
            if !fb_db::assistant::result(value) {
                return Err("invalid parsed receipt result".into());
            }
        } else if value.get("initial").is_some() {
            INITIAL.validate(value, "assistant.initial")?;
            let lines = value["lines"].as_array().ok_or("initial lines")?;
            for line in lines {
                if line["mismatch"] == true {
                    let q = fb_db::assistant::scaled(
                        &line["quantity"],
                        4,
                        999_999_999_999_999_999,
                        true,
                    )
                    .ok_or("invalid initial quantity")?;
                    let rate =
                        fb_db::assistant::scaled(&line["rate"], 2, fb_db::assistant::MAX, false)
                            .ok_or("invalid initial rate")?;
                    let product = fb_db::assistant::scaled(&line["product"], 2, i128::MAX, false)
                        .ok_or("invalid initial product")?;
                    if fb_db::assistant::product(q, rate) != Some(product) {
                        return Err("initial product disagrees with receipt arithmetic".into());
                    }
                } else if line.get("product").is_some() {
                    return Err("unmarked initial line has a product override".into());
                }
            }
            if value["mismatched"] != lines.iter().any(|line| line["mismatch"] == true)
                || value["marked"] != serde_json::json!(["date", "due_date", "reference", "vendor"])
            {
                return Err("invalid initial receipt markers".into());
            }
            for key in ["date", "due_date"] {
                if let Some(date) = value["initial"].get(key).and_then(Value::as_str)
                    && !fb_db::banking_evidence::calendar_date(date)
                {
                    return Err("invalid initial receipt date".into());
                }
            }
            let matches = a["readings"].as_array().ok_or("readings")?.iter().any(|r| {
                r["status"] == "ok"
                    && r["original_name"] == value["file_name"]
                    && r["result"]["document_number"] == value["initial"]["reference"]
                    && ["subtotal", "tax", "total", "warnings"]
                        .iter()
                        .all(|k| r["result"][*k] == value[*k])
                    && r["result"]["lines"].as_array().is_some_and(|source| {
                        source.len() == lines.len()
                            && source.iter().zip(lines).enumerate().all(|(i, (s, l))| {
                                l["ai_line"].as_u64() == Some((i + 1) as u64)
                                    && ["quantity", "rate", "description", "amount"]
                                        .iter()
                                        .all(|k| s[*k] == l[*k])
                                    && l["mismatch"] == (s["mismatch"] == true)
                            })
                    })
            });
            if !matches {
                return Err("initial receipt lines differ from saved evidence".into());
            }
        } else if !value.as_object().is_some_and(|m| m.len() == 1)
            || !value["queue"].as_array().is_some_and(|a| {
                let mut seen = BTreeSet::new();
                a.iter().all(|v| {
                    v.as_str()
                        .is_some_and(|s| ids.contains(s) && seen.insert(s))
                })
            })
        {
            return Err("invalid assistant result shape".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn identical_corrupted_receipt_dump_is_never_match() {
        let scenario = serde_json::json!({"users":[{"username":"staff","is_staff":true}],"steps":[{"op":"create_company","ref":"co","name":"Synthetic"}]});
        let path =
            std::env::temp_dir().join(format!("assistant-comparer-{}.fbbook", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut runner = crate::Runner::new(&scenario, &path);
        runner.run();
        let good = runner.dump();
        assert!(crate::compare(&good, &good).ok());
        for bad in [
            serde_json::json!(null),
            serde_json::json!([]),
            serde_json::json!({}),
            serde_json::json!({"connections":[],"readings":[],"results":{"x":12}}),
        ] {
            let mut dump = good.clone();
            dump["assistant"] = bad;
            assert!(!crate::compare(&dump, &dump).ok());
        }
    }

    #[test]
    fn populated_identical_omissions_and_corrupt_values_cannot_match() {
        use serde_json::json;
        let raw = json!({"vendor_name":"Synthetic","vendor_address":[],"document_number":"Synthetic 1","document_date":"","due_date":"","currency":"USD","subtotal":"1","tax":"0","total":"1","lines":[{"description":"A","quantity":"1","unit_price":"1","amount":"1"},{"description":"Zero retained","quantity":"1","unit_price":"0","amount":"0"}]});
        let scenario = json!({"users":[{"username":"staff","is_staff":true}],"steps":[
          {"op":"create_company","ref":"co","name":"Synthetic"},
          {"op":"assistant_connect","company":"co","state":{"mode":"api_key","model_choices":[]}},
          {"op":"assistant_switch","company":"co","enabled":true},
          {"op":"assistant_parse","company":"co","ref":"parsed","raw":raw},
          {"op":"assistant_begin","company":"co","ref":"reading","request_key":"00000000-0000-0000-0000-000000000001","file":{"source_file_id":"00000000-0000-0000-0000-000000000002","original_name":"synthetic.png","content_type":"image/png","size":1,"sha256":"a".repeat(64)}},
          {"op":"assistant_finish","company":"co","reading":"reading","result":"parsed"},
          {"op":"assistant_initial","company":"co","reading":"reading","ref":"initial"},
          {"op":"assistant_queue","company":"co","ref":"queue"}
        ]});
        let path = std::env::temp_dir().join(format!(
            "assistant-populated-comparer-{}.fbbook",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let mut runner = crate::Runner::new(&scenario, &path);
        runner.run();
        let good = runner.dump();
        assert!(
            crate::compare(&good, &good).ok(),
            "{:?}",
            crate::compare(&good, &good)
        );
        for (pointer, bad) in [
            ("/assistant/connections", json!([])),
            ("/assistant/connections/0/model", json!("not-in-the-list")),
            ("/assistant/readings", json!([])),
            ("/assistant/results", json!({})),
            ("/assistant/results/queue/queue", json!(["unknown"])),
            (
                "/assistant/results/queue/queue",
                json!(["reading", "reading"]),
            ),
            ("/assistant/readings/0/result/total", json!("NaN")),
            ("/assistant/readings/0/result/lines/0/mismatch", json!(true)),
            ("/assistant/readings/0/created_at", json!(-1)),
            ("/assistant/readings/0/bill_id", json!("unknown")),
            (
                "/assistant/readings/0/content_type",
                json!("application/x-executable"),
            ),
            ("/assistant/results/initial/lines", json!([])),
            ("/assistant/results/initial/lines/0/quantity", json!("NaN")),
            ("/assistant/results/initial/lines/0/product", json!("NaN")),
            ("/assistant/results/initial/mismatched", json!(true)),
            ("/assistant/results/initial/marked", json!([])),
            (
                "/assistant/results/initial/initial/date",
                json!("2026-02-30"),
            ),
        ] {
            let mut broken = good.clone();
            if let Some(slot) = broken.pointer_mut(pointer) {
                *slot = bad;
            } else {
                let (parent, key) = pointer.rsplit_once('/').unwrap();
                broken
                    .pointer_mut(parent)
                    .unwrap()
                    .as_object_mut()
                    .unwrap()
                    .insert(key.into(), bad);
            }
            assert!(!crate::compare(&broken, &broken).ok(), "{pointer}");
        }
        let mut broken = good.clone();
        broken["assistant"]["readings"][0]["result"]["lines"]
            .as_array_mut()
            .unwrap()
            .pop();
        assert!(!crate::compare(&broken, &broken).ok());
        let mut broken = good.clone();
        broken["assistant"]["readings"][0]["result"]["notes"]
            .as_array_mut()
            .unwrap()
            .pop();
        assert!(!crate::compare(&broken, &broken).ok());
        let mut broken = good.clone();
        let duplicate = broken["assistant"]["readings"][0].clone();
        broken["assistant"]["readings"]
            .as_array_mut()
            .unwrap()
            .push(duplicate);
        assert!(!crate::compare(&broken, &broken).ok());
    }
}
