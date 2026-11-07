//! Estimate inventories come from successful scenario requests, never from the dump being judged.
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
#[derive(Default)]
struct Expected {
    names: BTreeSet<String>,
    history: Vec<(usize, String)>,
    conversion: Option<(i64, BTreeSet<String>)>,
}
pub(crate) fn check(scenario: &Value, dump: &Value) -> Vec<String> {
    let dump = dump.get("final").unwrap_or(dump);
    let mut roots: BTreeMap<String, Expected> = BTreeMap::new();
    let mut aliases: BTreeMap<String, String> = BTreeMap::new();
    let mut keys = BTreeMap::new();
    let steps = scenario["steps"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let outcomes = dump["outcomes"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let mut failures = vec![];
    for (i, s) in steps.iter().enumerate() {
        if s["expect_error"] == true || outcomes.get(i).is_none_or(|o| o["ok"] != true) {
            continue;
        }
        match s["op"].as_str().unwrap_or("") {
            "sales_save_estimate" => {
                let name = s["ref"].as_str().unwrap_or("").to_owned();
                let count = s["values"]["lines"].as_array().map(Vec::len).unwrap_or(0);
                let root = if let Some(record) = s["record"].as_str() {
                    aliases.get(record).cloned()
                } else {
                    keys.get(&(
                        s["company"].to_string(),
                        s["values"]["request_key"].to_string(),
                    ))
                    .cloned()
                };
                if let Some(root) = root {
                    if let Some(e) = roots.get_mut(&root) {
                        e.names.insert(name.clone());
                        if s["record"].is_string() {
                            e.history.push((count, "DRAFT".into()));
                        }
                    }
                    aliases.insert(name, root);
                } else {
                    let mut e = Expected::default();
                    e.names.insert(name.clone());
                    e.history.push((count, "DRAFT".into()));
                    roots.insert(name.clone(), e);
                    aliases.insert(name.clone(), name.clone());
                    keys.insert(
                        (
                            s["company"].to_string(),
                            s["values"]["request_key"].to_string(),
                        ),
                        name,
                    );
                }
            }
            "sales_transition_estimate" | "sales_convert_estimate" => {
                let Some(e) = s["record"]
                    .as_str()
                    .and_then(|r| aliases.get(r))
                    .and_then(|r| roots.get_mut(r))
                else {
                    failures.push(format!("estimate manifest: unknown record at step {i}"));
                    continue;
                };
                if s["op"] == "sales_transition_estimate" {
                    let count = e.history.last().map(|v| v.0).unwrap_or(0);
                    e.history
                        .push((count, s["state"].as_str().unwrap_or("").into()));
                } else {
                    let (revision, names) = e.conversion.get_or_insert((
                        s["expected_revision"].as_i64().unwrap_or(0),
                        BTreeSet::new(),
                    ));
                    if Some(*revision) != s["expected_revision"].as_i64() {
                        failures.push("estimate manifest: inconsistent conversion revision".into());
                    }
                    names.insert(s["ref"].as_str().unwrap_or("").into());
                }
            }
            _ => {}
        }
    }
    let actual = dump["sales"]["estimates"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let expected_names = roots
        .values()
        .filter_map(|e| e.names.first().cloned())
        .collect::<BTreeSet<_>>();
    let actual_names = actual
        .iter()
        .filter_map(|e| e["ref"].as_str().map(str::to_owned))
        .collect::<BTreeSet<_>>();
    if expected_names != actual_names || actual.len() != roots.len() {
        failures
            .push("estimate manifest: complete estimate inventory differs from scenario".into());
    }
    for e in roots.values() {
        let Some(name) = e.names.first() else {
            continue;
        };
        let Some(a) = actual.iter().find(|a| a["ref"] == *name) else {
            continue;
        };
        let history = a["history"].as_array().map(Vec::as_slice).unwrap_or(&[]);
        if a["revision"].as_u64() != Some(e.history.len() as u64)
            || history.len() != e.history.len()
        {
            failures.push(format!(
                "estimate {name}: revision inventory differs from scenario"
            ));
        }
        for (index, ((count, state), h)) in e.history.iter().zip(history).enumerate() {
            if h["number"].as_u64() != Some(index as u64 + 1) || h["state"] != *state {
                failures.push(format!(
                    "estimate {name}: revision {} state differs from scenario",
                    index + 1
                ));
            }
            if h["lines"].as_array().map(Vec::len) != Some(*count) {
                failures.push(format!(
                    "estimate {name}: revision {} lines differ from scenario",
                    index + 1
                ));
            }
        }
        match &e.conversion {
            Some((revision, names)) => {
                if a["conversion"]["revision"] != *revision
                    || names
                        .first()
                        .is_none_or(|n| a["conversion"]["invoice"] != *n)
                {
                    failures.push(format!("estimate {name}: conversion differs from scenario"));
                }
            }
            None => {
                if !a["conversion"].is_null() {
                    failures.push(format!("estimate {name}: unrequested conversion"));
                }
            }
        }
    }
    failures
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identical_missing_estimates_revisions_or_zero_value_lines_do_not_match() {
        let scenario: Value =
            serde_json::from_str(include_str!("../../scenarios/sales-estimates-d4.json")).unwrap();
        let path = std::env::temp_dir().join(format!("d4-manifest-{}.fbbook", fb_ids::new_v7()));
        let mut runner = crate::Runner::new(&scenario, &path);
        runner.run();
        let base = runner.dump();
        assert!(check(&scenario, &base).is_empty());
        for (field, needle) in [
            ("estimates", "inventory"),
            ("history", "revision"),
            ("lines", "lines"),
        ] {
            let mut bad = base.clone();
            if field == "estimates" {
                bad["sales"]["estimates"] = serde_json::json!([]);
            } else if field == "history" {
                bad["sales"]["estimates"][0]["history"] = serde_json::json!([]);
            } else {
                bad["sales"]["estimates"][0]["history"][0]["lines"]
                    .as_array_mut()
                    .unwrap()
                    .pop();
            }
            let report = crate::compare_scenario(Some(&scenario), &bad, &bad);
            assert!(
                report.failures.iter().any(|f| f.contains(needle)),
                "missed {field}: {:?}",
                report.failures
            );
        }
    }
}
