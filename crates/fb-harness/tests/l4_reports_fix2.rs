//! L4 reports fix 2: the corruptions that survived the second review (runs/2026-10-30/r2-l4-reports-review,
//! `recheck.py`), plus the consistent lies found by attacking the fixed comparer. Every case corrupts BOTH sides
//! the same way; only the scenario's manifest and the dump's own journal and settlement evidence can catch it.
use serde_json::{Value, json};
use std::sync::OnceLock;

/// The combined layer-3 company plus an empty second company, as the review ran it: (scenario, dump).
fn fixture() -> &'static (Value, Value) {
    static FIXTURE: OnceLock<(Value, Value)> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/scenarios/reports-combined.json")).unwrap();
        let mut scenario: Value = fb_db::parse_unique_json(&text).unwrap();
        scenario.as_object_mut().unwrap().remove("capture_steps");
        scenario["steps"].as_array_mut().unwrap().push(json!({"op": "create_company", "ref": "other", "name": "Empty synthetic review company"}));
        let dir = tempfile::tempdir().unwrap();
        let mut runner = fb_harness::Runner::new(&scenario, &dir.path().join("fix2.fbbook"));
        runner.run();
        let dump = runner.dump();
        let report = fb_harness::compare_scenario(Some(&scenario), &dump, &dump);
        assert!(report.ok(), "{:?}", report.failures);
        (scenario, dump)
    })
}

fn bases(v: &mut Value) -> impl Iterator<Item = (&String, &mut Value)> {
    v["financial_reports"]["companies"].as_object_mut().unwrap().values_mut().flat_map(|c| c.as_object_mut().unwrap().iter_mut())
}

fn each_report(v: &mut Value, family: &str, mut f: impl FnMut(&str, &mut Value)) {
    for (basis, b) in bases(v) {
        for report in b[family].as_object_mut().unwrap().values_mut() {
            f(basis, report);
        }
    }
}

fn kinds(v: &Value) -> std::collections::HashMap<String, String> {
    v["accounts"].as_array().unwrap().iter().map(|a| (a["ref"].as_str().unwrap().to_string(), a["kind"].as_str().unwrap().to_string())).collect()
}

/// Re-run every ledger section's running balances and totals after rows changed, so only the inventory is wrong.
fn rebalance(v: &mut Value) {
    let kinds = kinds(v);
    each_report(v, "general_ledger", |_, r| {
        if r.is_null() {
            return;
        }
        let (mut all_d, mut all_c) = (0, 0);
        for s in r["sections"].as_array_mut().unwrap() {
            let sign = if matches!(kinds[s["account"].as_str().unwrap()].as_str(), "ASSET" | "EXPENSE") { 1 } else { -1 };
            let mut balance = s["opening"].as_i64().unwrap();
            let (mut d, mut c) = (0, 0);
            for row in s["rows"].as_array_mut().unwrap() {
                balance += sign * (row[1].as_i64().unwrap() - row[2].as_i64().unwrap());
                d += row[1].as_i64().unwrap();
                c += row[2].as_i64().unwrap();
                row[10] = json!(balance);
            }
            s["closing"] = json!(balance);
            s["debit"] = json!(d);
            s["credit"] = json!(c);
            all_d += d;
            all_c += c;
        }
        r["debit"] = json!(all_d);
        r["credit"] = json!(all_c);
    });
}

type Mutation = fn(&mut Value);
/// (name, mutation, the failure that must catch it).
type Case = (&'static str, Mutation, &'static str);

/// The review's five survivors, ported from `recheck.py`.
const REVIEW_SURVIVORS: &[Case] = &[
    ("partial_manifest_erasure", |v| {
        let date = v["financial_reports"]["requests"]["balance_sheet"].as_array_mut().unwrap().remove(0);
        for (_, b) in bases(v) {
            b["balance_sheet"].as_object_mut().unwrap().remove(date.as_str().unwrap());
        }
    }, "not the scenario's financial_reports spec"),
    ("entire_financial_section_erased", |v| {
        v.as_object_mut().unwrap().remove("financial_reports");
    }, "the scenario's financial reports are missing"),
    ("csv_subtotals_erased", |v| {
        for family in ["profit_loss", "balance_sheet"] {
            each_report(v, family, |_, r| {
                let csv = r["csv"].as_array().unwrap().clone();
                let (last, body) = csv.split_last().unwrap();
                let mut kept: Vec<Value> = body.iter().filter(|row| row[0] == "account").cloned().collect();
                kept.push(last.clone());
                r["csv"] = json!(kept);
            });
        }
    }, "rows, the report's sections give"),
    ("cash_count_999999", |v| {
        for family in ["profit_loss", "balance_sheet", "general_ledger"] {
            each_report(v, family, |basis, r| {
                if basis == "CASH" && !r.is_null() {
                    r["unconverted"] = json!(999999);
                }
            });
        }
    }, "unconverted disagrees with the posted control-account lines"),
    ("recognised_rows_coalesced", |v| {
        let mut removed = 0;
        each_report(v, "general_ledger", |basis, r| {
            if basis != "CASH" || r.is_null() {
                return;
            }
            for s in r["sections"].as_array_mut().unwrap() {
                let mut rows: Vec<Value> = Vec::new();
                let mut groups: Vec<((String, bool, bool), usize)> = Vec::new();
                for row in s["rows"].as_array().unwrap().clone() {
                    let key = (row[0].as_str().unwrap().to_string(), row[1].as_i64().unwrap() > 0, row[2].as_i64().unwrap() > 0);
                    match groups.iter().find(|(k, _)| *k == key) {
                        Some(&(_, at)) if !row[9].is_null() => {
                            rows[at][1] = json!(rows[at][1].as_i64().unwrap() + row[1].as_i64().unwrap());
                            rows[at][2] = json!(rows[at][2].as_i64().unwrap() + row[2].as_i64().unwrap());
                            removed += 1;
                        }
                        _ => {
                            if !row[9].is_null() {
                                groups.push((key, rows.len()));
                            }
                            rows.push(row);
                        }
                    }
                }
                s["rows"] = json!(rows);
            }
        });
        assert_eq!(removed, 4, "the review coalesced four recognised rows");
        rebalance(v);
    }, "rows disagree with the ledger and settlement evidence"),
];

fn survivors(cases: &[Case]) -> Vec<String> {
    let (scenario, good) = fixture();
    let mut survived = Vec::new();
    for (name, mutate, why) in cases {
        let mut bad = good.clone();
        mutate(&mut bad);
        assert_ne!(&bad, good, "{name}: the mutation changed nothing");
        match std::panic::catch_unwind(|| fb_harness::compare_scenario(Some(scenario), &bad, &bad)) {
            Err(_) => survived.push(format!("{name}: panicked")),
            Ok(report) if report.ok() => survived.push(format!("{name}: MATCH")),
            Ok(report) if !report.failures.iter().any(|f| f.contains(why)) => survived.push(format!("{name}: caught for another reason {:?}", report.failures)),
            Ok(_) => {}
        }
    }
    survived
}

#[test]
fn review_survivors_never_match() {
    let survived = survivors(REVIEW_SURVIVORS);
    assert!(survived.is_empty(), "{survived:#?}");
}

fn csv_rows(v: &mut Value, family: &str, mut f: impl FnMut(&mut Vec<Value>)) {
    each_report(v, family, |_, r| f(r["csv"].as_array_mut().unwrap()));
}

fn ledger_rows(v: &mut Value, mut f: impl FnMut(&mut Value)) {
    each_report(v, "general_ledger", |_, r| {
        if r.is_null() {
            return;
        }
        for s in r["sections"].as_array_mut().unwrap() {
            for row in s["rows"].as_array_mut().unwrap() {
                f(row);
            }
        }
    });
}

/// Consistent lies found by attacking the fixed comparer: every value it accepts, rewritten the same way in
/// both dumps (the evidence itself left as it is).
const SELF_ATTACK: &[Case] = &[
    ("ledger rows reordered within a date", |v| {
        each_report(v, "general_ledger", |_, r| {
            if r.is_null() {
                return;
            }
            for s in r["sections"].as_array_mut().unwrap() {
                let rows = s["rows"].as_array_mut().unwrap();
                if let Some(i) = (1..rows.len()).find(|&i| rows[i][0] == rows[i - 1][0] && rows[i] != rows[i - 1]) {
                    rows.swap(i - 1, i);
                }
            }
        });
        rebalance(v);
    }, "rows are not in the ledger's (date, journal, position) order"),
    ("recognised share moved to another journal", |v| {
        ledger_rows(v, |row| {
            if row[9].is_string() {
                row[3] = json!("j0");
            }
        })
    }, "rows disagree with the ledger and settlement evidence"),
    ("recognised share relabelled as a posted line", |v| {
        ledger_rows(v, |row| {
            if row[9].is_string() {
                row[9] = Value::Null;
            }
        })
    }, "rows disagree with the ledger and settlement evidence"),
    ("posted line memo, name and source rewritten", |v| {
        ledger_rows(v, |row| {
            if row[9].is_null() {
                row[7] = json!("Rewritten");
                row[8] = json!("Someone");
                row[4] = json!("RCP-999999");
            }
        })
    }, "rows disagree with the ledger and settlement evidence"),
    ("reversal flags flipped", |v| ledger_rows(v, |row| row[5] = json!(!row[5].as_bool().unwrap())), "rows disagree with the ledger and settlement evidence"),
    ("CSV subtotal amount changed", |v| {
        csv_rows(v, "profit_loss", |rows| {
            let first = rows.iter_mut().find(|r| r[0] == "total").unwrap();
            first[6] = json!(first[6].as_i64().unwrap() + 1);
        })
    }, "is not the report's row"),
    ("CSV rows reordered", |v| csv_rows(v, "balance_sheet", |rows| rows.swap(0, 1)), "is not the report's row"),
    ("CSV computed rows relabelled", |v| {
        csv_rows(v, "balance_sheet", |rows| {
            for r in rows.iter_mut().filter(|r| r[0] == "computed") {
                r[4] = json!("Profit");
            }
        })
    }, "is not the report's row"),
    ("cash unconverted set to a plausible 1", |v| {
        for family in ["profit_loss", "balance_sheet", "general_ledger"] {
            each_report(v, family, |basis, r| {
                if basis == "CASH" && !r.is_null() {
                    r["unconverted"] = json!(1);
                }
            });
        }
    }, "unconverted disagrees with the posted control-account lines"),
    ("refused step reported as success, flag dropped", |v| {
        let outcomes = v["outcomes"].as_array_mut().unwrap();
        let o = outcomes.iter_mut().find(|o| o["ok"] == false).unwrap();
        *o = json!({"step": o["step"], "op": o["op"], "ok": true});
    }, "the outcome is not the one the scenario intends"),
    ("last outcome dropped", |v| {
        v["outcomes"].as_array_mut().unwrap().pop();
    }, "outcomes: "),
    ("company erased everywhere", |v| {
        v["companies"].as_array_mut().unwrap().retain(|c| c["ref"] != "other");
        v["financial_reports"]["companies"].as_object_mut().unwrap().remove("other");
        for key in ["trial_balances", "audit"] {
            v[key].as_object_mut().unwrap().remove("other");
        }
    }, "the scenario created"),
];

#[test]
fn self_attack_lies_never_match() {
    let survived = survivors(SELF_ATTACK);
    assert!(survived.is_empty(), "{survived:#?}");
}

#[test]
fn fixture_exercises_the_rebuilt_evidence() {
    let v = &fixture().1["financial_reports"]["companies"]["c"]["CASH"];
    let gl = &v["general_ledger"]["..2099-12-31||"];
    let rows: Vec<&Value> = gl["sections"].as_array().unwrap().iter().flat_map(|s| s["rows"].as_array().unwrap()).collect();
    for note in ["receipt", "receipt reversal", "bill payment", "bill payment reversal", "credit applied", "credit unapplied", "refund", "refund void", "deposit applied", "deposit unapplied"] {
        assert!(rows.iter().any(|r| r[9] == note), "no recognised share for {note}");
    }
}

#[test]
fn reports_need_the_scenario() {
    let (scenario, good) = fixture();
    // Without the scenario no report can be checked; a scenario that requests none rejects any.
    assert!(!fb_harness::compare(good, good).ok());
    let mut silent = scenario.clone();
    silent.as_object_mut().unwrap().remove("financial_reports");
    assert!(!fb_harness::compare_scenario(Some(&silent), good, good).ok());
}
