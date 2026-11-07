//! L4 reports fix 1: identical corrupted report dumps never MATCH. Each case corrupts BOTH sides the same way,
//! so only the per-side manifest, inventory and evidence checks can catch it. The fixture is the combined
//! layer-3 company (deferred documents, settlements, recognised shares) plus an empty second company.
use serde_json::{Value, json};
use std::sync::OnceLock;

fn fixture() -> &'static (Value, Value) {
    static FIXTURE: OnceLock<(Value, Value)> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/scenarios/reports-combined.json")).unwrap();
        let mut scenario: Value = fb_db::parse_unique_json(&text).unwrap();
        scenario.as_object_mut().unwrap().remove("capture_steps");
        scenario["steps"].as_array_mut().unwrap().push(json!({"op": "create_company", "ref": "other", "name": "Empty synthetic company"}));
        scenario["financial_reports"]["forms"] = json!([
            {"form": "range", "data": {"preset": "custom", "start": "2026-10-01", "end": "2026-10-31"}, "today": "2026-10-29"},
            {"form": "ledger", "data": {"group": "pl"}, "today": "2026-10-29"},
            {"form": "as_of", "data": {"as_of": "garbage"}, "today": "2026-10-29"}
        ]);
        // A ledger for another company's account (null on both sides, as Python's 404).
        scenario["financial_reports"]["general_ledger"].as_array_mut().unwrap().push(json!(["2026-10-01", "2026-10-31", "", "nowhere"]));
        let dir = tempfile::tempdir().unwrap();
        let mut runner = fb_harness::Runner::new(&scenario, &dir.path().join("fix1.fbbook"));
        runner.run();
        let dump = runner.dump();
        let report = fb_harness::compare_scenario(Some(&scenario), &dump, &dump);
        assert!(report.ok(), "{:?}", report.failures);
        (scenario, dump)
    })
}

fn companies(v: &mut Value) -> &mut serde_json::Map<String, Value> {
    v["financial_reports"]["companies"].as_object_mut().unwrap()
}

fn each_report(v: &mut Value, family: &str, mut f: impl FnMut(&str, &mut Value)) {
    for bases in companies(v).values_mut() {
        for (basis, b) in bases.as_object_mut().unwrap() {
            for report in b[family].as_object_mut().unwrap().values_mut() {
                f(basis, report);
            }
        }
    }
}

type Mutation = fn(&mut Value);

const CASES: &[(&str, Mutation)] = &[
    ("erased report sets", |v| {
        for bases in companies(v).values_mut() {
            for b in bases.as_object_mut().unwrap().values_mut() {
                for family in ["profit_loss", "balance_sheet", "general_ledger"] {
                    b[family] = json!({});
                }
            }
        }
    }),
    ("omitted company", |v| {
        companies(v).remove("other");
    }),
    ("omitted basis everywhere", |v| {
        for bases in companies(v).values_mut() {
            bases.as_object_mut().unwrap().remove("CASH");
        }
    }),
    ("one report dropped everywhere", |v| {
        for bases in companies(v).values_mut() {
            for b in bases.as_object_mut().unwrap().values_mut() {
                b["balance_sheet"].as_object_mut().unwrap().remove("2026-10-31");
            }
        }
    }),
    ("manifest and report dropped together", |v| {
        v["financial_reports"]["requests"]["balance_sheet"].as_array_mut().unwrap().retain(|d| d != "2026-10-31");
        for bases in companies(v).values_mut() {
            for b in bases.as_object_mut().unwrap().values_mut() {
                b["balance_sheet"].as_object_mut().unwrap().remove("2026-10-31");
            }
        }
        // The manifest is the scenario's: the other side's echo still names it. Here both sides agree, so the
        // removal must be caught by the manifest's own minimums or the evidence still covering it. Drop all of it.
        v["financial_reports"]["requests"]["balance_sheet"] = json!([]);
        for bases in companies(v).values_mut() {
            for b in bases.as_object_mut().unwrap().values_mut() {
                b["balance_sheet"] = json!({});
            }
        }
    }),
    ("manifest without the evidence ledger", |v| {
        let gl = v["financial_reports"]["requests"]["general_ledger"].as_array_mut().unwrap();
        gl.retain(|g| !(g[0].is_null() && g[2] == "" && g[3].is_null()));
        let keep: Vec<String> = gl.iter().map(|g| format!("{}..{}|{}|{}", g[0].as_str().unwrap_or(""), g[1].as_str().unwrap(), g[2].as_str().unwrap(), g[3].as_str().unwrap_or(""))).collect();
        each_report(v, "general_ledger", |_, _| {});
        for bases in companies(v).values_mut() {
            for b in bases.as_object_mut().unwrap().values_mut() {
                b["general_ledger"].as_object_mut().unwrap().retain(|k, _| keep.contains(k));
            }
        }
    }),
    ("duplicate manifest request", |v| {
        let gl = v["financial_reports"]["requests"]["general_ledger"].as_array_mut().unwrap();
        let first = gl[0].clone();
        gl.push(first);
    }),
    ("non-canonical manifest date", |v| {
        v["financial_reports"]["requests"]["balance_sheet"][0] = json!("x");
    }),
    ("null valid ledgers", |v| each_report(v, "general_ledger", |_, r| *r = Value::Null)),
    ("empty valid ledgers", |v| {
        each_report(v, "general_ledger", |_, r| {
            if !r.is_null() {
                r["sections"] = json!([]);
                r["debit"] = json!(0);
                r["credit"] = json!(0);
            }
        })
    }),
    ("ledger for a missing account made up", |v| {
        let real = v["financial_reports"]["companies"]["c"]["ACCRUAL"]["general_ledger"]["2026-10-01..2026-10-31||income"].clone();
        for bases in companies(v).values_mut() {
            for b in bases.as_object_mut().unwrap().values_mut() {
                b["general_ledger"]["2026-10-01..2026-10-31||nowhere"] = real.clone();
            }
        }
    }),
    ("ledger row deleted with totals fixed", |v| {
        each_report(v, "general_ledger", |_, r| {
            if let Some(s) = r["sections"].as_array_mut().and_then(|s| s.iter_mut().find(|s| s["rows"].as_array().is_some_and(|r| r.len() > 1))) {
                let row = s["rows"].as_array_mut().unwrap().pop().unwrap();
                let last = s["rows"].as_array().unwrap().last().unwrap()[10].clone();
                s["closing"] = last;
                s["debit"] = json!(s["debit"].as_i64().unwrap() - row[1].as_i64().unwrap());
                s["credit"] = json!(s["credit"].as_i64().unwrap() - row[2].as_i64().unwrap());
                r["debit"] = json!(r["debit"].as_i64().unwrap() - row[1].as_i64().unwrap());
                r["credit"] = json!(r["credit"].as_i64().unwrap() - row[2].as_i64().unwrap());
            }
        })
    }),
    ("CSV rows deleted", |v| {
        for family in ["profit_loss", "balance_sheet"] {
            each_report(v, family, |_, r| r["csv"] = json!([]));
        }
    }),
    ("one CSV account row deleted", |v| {
        each_report(v, "balance_sheet", |_, r| r["csv"].as_array_mut().unwrap().retain(|c| c[0] != "account"));
    }),
    ("CSV account renamed", |v| {
        each_report(v, "profit_loss", |_, r| {
            for row in r["csv"].as_array_mut().unwrap().iter_mut().filter(|c| c[0] == "account") {
                row[4] = json!("Renamed");
            }
        })
    }),
    ("cash replaced by accrual", |v| {
        for bases in companies(v).values_mut() {
            bases["CASH"] = bases["ACCRUAL"].clone();
        }
    }),
    ("accrual replaced by cash", |v| {
        for bases in companies(v).values_mut() {
            bases["ACCRUAL"] = bases["CASH"].clone();
        }
    }),
    ("balance-sheet groups and accounts invalid", |v| {
        each_report(v, "balance_sheet", |_, r| {
            for section in ["assets", "liabilities", "equity"] {
                for g in r[section]["groups"].as_array_mut().unwrap() {
                    g["key"] = json!("INVALID");
                    for t in g["types"].as_array_mut().unwrap() {
                        t["type"] = json!("INVALID");
                        for row in t["rows"].as_array_mut().unwrap() {
                            row[0] = json!("foreign-nonexistent-account");
                        }
                    }
                }
            }
        })
    }),
    ("negative unconverted", |v| {
        for family in ["profit_loss", "balance_sheet", "general_ledger"] {
            each_report(v, family, |_, r| {
                if !r.is_null() {
                    r["unconverted"] = json!(-1);
                }
            });
        }
    }),
    ("accrual unconverted", |v| {
        each_report(v, "profit_loss", |basis, r| {
            if basis == "ACCRUAL" {
                r["unconverted"] = json!(1);
            }
        })
    }),
    ("short balance-sheet key", |v| {
        for bases in companies(v).values_mut() {
            for b in bases.as_object_mut().unwrap().values_mut() {
                let first = b["balance_sheet"].as_object().unwrap().values().next().unwrap().clone();
                b["balance_sheet"] = json!({"x": first});
            }
        }
        v["financial_reports"]["requests"]["balance_sheet"] = json!(["x"]);
    }),
    ("malformed successful form", |v| {
        v["financial_reports"]["forms"][0] = json!({"ok": true, "basis": "ACCRUAL", "preset": [], "start": {}, "end": "not-a-date"});
    }),
    ("unknown preset and group", |v| {
        v["financial_reports"]["forms"][1]["preset"] = json!("someday");
        v["financial_reports"]["forms"][1]["group"] = json!("nope");
    }),
    ("as-of form shaped like a range", |v| {
        v["financial_reports"]["forms"][2] = json!({"ok": true, "basis": "CASH", "preset": "custom", "start": null, "end": "2026-10-31"});
    }),
    ("form dropped", |v| {
        v["financial_reports"]["forms"].as_array_mut().unwrap().pop();
    }),
];

#[test]
fn identical_corruptions_never_match() {
    let (scenario, good) = fixture();
    let mut survived = Vec::new();
    for (name, mutate) in CASES {
        let mut bad = good.clone();
        mutate(&mut bad);
        assert_ne!(&bad, good, "{name}: the mutation changed nothing");
        let result = std::panic::catch_unwind(|| fb_harness::compare_scenario(Some(scenario), &bad, &bad));
        match result {
            Err(_) => survived.push(format!("{name}: panicked")),
            Ok(report) if report.ok() => survived.push(format!("{name}: MATCH")),
            Ok(_) => {}
        }
    }
    assert!(survived.is_empty(), "{survived:#?}");
}

#[test]
fn cash_reports_carry_independent_evidence() {
    // The fixture really exercises the cash rules: excluded document lines and recognised shares both occur.
    let v = &fixture().1["financial_reports"]["companies"]["c"];
    let rows = |basis: &str| -> Vec<Value> {
        v[basis]["general_ledger"]["..2099-12-31||"]["sections"].as_array().unwrap().iter().flat_map(|s| s["rows"].as_array().unwrap().clone()).collect()
    };
    assert!(rows("CASH").iter().any(|r| r[9].is_string()), "no recognised shares");
    assert!(rows("ACCRUAL").iter().all(|r| r[9].is_null()));
    assert!(rows("ACCRUAL").len() != rows("CASH").iter().filter(|r| r[9].is_null()).count(), "no excluded lines");
}
