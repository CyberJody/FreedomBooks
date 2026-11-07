//! Accrual financial checkpoints for the combined layer-3 scenario. The oracle
//! calls Python's real financial reports; Rust aggregates the complete posted dump.
use super::*;
use std::collections::BTreeSet;

pub(super) fn financial(dump: &Value) -> Result<Value, &'static str> {
    let mut result = json!({});
    for company in dump["companies"].as_array().unwrap() {
        let reference = company["ref"].as_str().unwrap();
        let mut nets: HashMap<&str, i64> = HashMap::new();
        let mut prior: HashMap<&str, i64> = HashMap::new();
        for j in dump["journals"].as_array().unwrap().iter().filter(|j| {
            j["company"] == reference
                && j["status"] == "POSTED"
                && j["date"].as_str().unwrap() <= "2026-10-31"
        }) {
            for line in j["lines"].as_array().unwrap() {
                let delta = line["debit"]
                    .as_i64()
                    .unwrap()
                    .checked_sub(
                        line["credit"]
                            .as_i64()
                            .ok_or("Financial checkpoint exceeds supported integer totals.")?,
                    )
                    .ok_or("Financial checkpoint exceeds supported integer totals.")?;
                let account = line["account"].as_str().unwrap();
                let net = nets.entry(account).or_default();
                *net = net
                    .checked_add(delta)
                    .ok_or("Financial checkpoint exceeds supported integer totals.")?;
                if j["date"].as_str().unwrap() < "2026-01-01" {
                    let net = prior.entry(account).or_default();
                    *net = net
                        .checked_add(delta)
                        .ok_or("Financial checkpoint exceeds supported integer totals.")?;
                }
            }
        }
        let mut pl = json!({"sections":{},"gross_profit":0,"net_ordinary_income":0,"net_other_income":0,"net_income":0});
        for key in ["income", "cogs", "expense", "other_income", "other_expense"] {
            pl["sections"][key] = json!({"rows":[],"total":0});
        }
        let mut bs = json!({"assets":{"rows":[],"total":0},"liabilities":{"rows":[],"total":0},"equity":{"rows":[],"total":0},"retained_earnings":0,"net_income":0,"total_equity":0,"total_liabilities_equity":0,"balanced":true});
        let mut accounts: Vec<_> = dump["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|a| a["company"] == reference)
            .collect();
        accounts.sort_by_key(|a| a["code"].as_str().unwrap());
        let mut retained = 0i64;
        for a in accounts {
            let r = a["ref"].as_str().unwrap();
            let Some(net) = nets.get(r).copied() else {
                continue;
            };
            let section = match a["detail_type"].as_str().unwrap() {
                "INCOME" => Some("income"),
                "COST_OF_GOODS_SOLD" => Some("cogs"),
                "EXPENSE" => Some("expense"),
                "OTHER_INCOME" => Some("other_income"),
                "OTHER_EXPENSE" => Some("other_expense"),
                _ => None,
            };
            if let Some(section) = section {
                let old = prior.get(r).copied().unwrap_or(0);
                retained = retained
                    .checked_sub(old)
                    .ok_or("Financial checkpoint exceeds supported integer totals.")?;
                // P&L includes active-in-period accounts even when their net is zero.
                let active = dump["journals"].as_array().unwrap().iter().any(|j| {
                    j["company"] == reference
                        && j["status"] == "POSTED"
                        && ("2026-01-01"..="2026-10-31").contains(&j["date"].as_str().unwrap())
                        && j["lines"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|l| l["account"] == r)
                });
                if active {
                    let delta = net
                        .checked_sub(old)
                        .ok_or("Financial checkpoint exceeds supported integer totals.")?;
                    let amount = if matches!(section, "income" | "other_income") {
                        delta
                            .checked_neg()
                            .ok_or("Financial checkpoint exceeds supported integer totals.")?
                    } else {
                        delta
                    };
                    let group = &mut pl["sections"][section];
                    group["rows"]
                        .as_array_mut()
                        .unwrap()
                        .push(json!([r, amount]));
                    group["total"] = json!(
                        group["total"]
                            .as_i64()
                            .unwrap()
                            .checked_add(amount)
                            .ok_or("Financial checkpoint exceeds supported integer totals.")?
                    );
                }
            } else {
                let key = match a["kind"].as_str().unwrap() {
                    "ASSET" => "assets",
                    "LIABILITY" => "liabilities",
                    _ => "equity",
                };
                let amount = if key == "assets" {
                    net
                } else {
                    net.checked_neg()
                        .ok_or("Financial checkpoint exceeds supported integer totals.")?
                };
                if amount != 0 {
                    bs[key]["rows"]
                        .as_array_mut()
                        .unwrap()
                        .push(json!([r, amount]));
                    bs[key]["total"] = json!(
                        bs[key]["total"]
                            .as_i64()
                            .unwrap()
                            .checked_add(amount)
                            .ok_or("Financial checkpoint exceeds supported integer totals.")?
                    );
                }
            }
        }
        let total = |k: &str| pl["sections"][k]["total"].as_i64().unwrap();
        let gross = total("income")
            .checked_sub(total("cogs"))
            .ok_or("Financial checkpoint exceeds supported integer totals.")?;
        let ordinary = gross
            .checked_sub(total("expense"))
            .ok_or("Financial checkpoint exceeds supported integer totals.")?;
        let other = total("other_income")
            .checked_sub(total("other_expense"))
            .ok_or("Financial checkpoint exceeds supported integer totals.")?;
        let income = ordinary
            .checked_add(other)
            .ok_or("Financial checkpoint exceeds supported integer totals.")?;
        pl["gross_profit"] = json!(gross);
        pl["net_ordinary_income"] = json!(ordinary);
        pl["net_other_income"] = json!(other);
        pl["net_income"] = json!(income);
        let equity = bs["equity"]["total"]
            .as_i64()
            .unwrap()
            .checked_add(retained)
            .ok_or("Financial checkpoint exceeds supported integer totals.")?
            .checked_add(income)
            .ok_or("Financial checkpoint exceeds supported integer totals.")?;
        let combined = bs["liabilities"]["total"]
            .as_i64()
            .unwrap()
            .checked_add(equity)
            .ok_or("Financial checkpoint exceeds supported integer totals.")?;
        bs["retained_earnings"] = json!(retained);
        bs["net_income"] = json!(income);
        bs["total_equity"] = json!(equity);
        bs["total_liabilities_equity"] = json!(combined);
        bs["balanced"] = json!(Some(combined) == bs["assets"]["total"].as_i64());
        result[reference] = json!({"basis":"ACCRUAL","start":"2026-01-01","end":"2026-10-31","profit_loss":pl,"balance_sheet":bs});
    }
    Ok(result)
}

pub(super) fn compare(py: &Value, rs: &Value) -> Report {
    let mut result = Report::default();
    fn keys(v: &Value) -> Option<BTreeSet<&str>> {
        v.as_object()
            .map(|m| m.keys().map(String::as_str).collect())
    }
    for (side, label) in [(py, "python"), (rs, "rust")] {
        if keys(side) != Some(BTreeSet::from(["checkpoints", "final"]))
            || !side["checkpoints"].is_array()
        {
            result
                .failures
                .push(format!("{label}: malformed checkpoint envelope"));
            continue;
        }
        let count = side["final"]["outcomes"].as_array().map(Vec::len);
        if count != side["checkpoints"].as_array().map(Vec::len) {
            result
                .failures
                .push(format!("{label}: incomplete checkpoints"));
        }
        for (index, cp) in side["checkpoints"].as_array().unwrap().iter().enumerate() {
            if keys(cp) != Some(BTreeSet::from(["step", "dump", "financial"]))
                || cp["step"].as_u64() != Some(index as u64)
                || cp["dump"]["outcomes"].as_array().map(Vec::len) != Some(index + 1)
            {
                result
                    .failures
                    .push(format!("{label}: malformed checkpoint {index}"));
                continue;
            }
            if let Err(e) = dump_shape::validate(&cp["dump"]) {
                result
                    .failures
                    .push(format!("{label} checkpoint {index}: {e}"));
                continue;
            }
            if !matches!(financial(&cp["dump"]), Ok(expected) if cp["financial"] == expected) {
                result.failures.push(format!(
                    "{label} checkpoint {index}: financial rows/totals disagree with posted books"
                ));
            }
        }
    }
    if !result.ok() {
        return result;
    }
    for (side, label) in [(py, "python"), (rs, "rust")] {
        let checkpoints = side["checkpoints"].as_array().unwrap();
        if checkpoints
            .last()
            .is_some_and(|cp| cp["dump"] != side["final"])
        {
            result
                .failures
                .push(format!("{label}: final dump differs from last checkpoint"));
        }
    }
    if !result.ok() {
        return result;
    }
    let mut merge = |label: &str, report: Report| {
        result
            .failures
            .extend(report.failures.into_iter().map(|s| format!("{label}: {s}")));
        result
            .warnings
            .extend(report.warnings.into_iter().map(|s| format!("{label}: {s}")));
        result.collation_ties += report.collation_ties;
        for (k, v) in report.checked {
            *result.checked.entry(k).or_default() += v;
        }
    };
    merge("final", super::compare(&py["final"], &rs["final"]));
    let (p, r) = (
        py["checkpoints"].as_array().unwrap(),
        rs["checkpoints"].as_array().unwrap(),
    );
    if p.len() != r.len() {
        result.failures.push("checkpoint count differs".into());
        return result;
    }
    for (index, (p, r)) in p.iter().zip(r).enumerate() {
        merge(
            &format!("checkpoint {index}"),
            super::compare(&p["dump"], &r["dump"]),
        );
    }
    // Both financial records have already been checked against complete books.
    result.checked.insert("checkpoints", p.len());
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checkpoint_comparer_requires_complete_steps_and_financial_rows() {
        let scenario = json!({"capture_steps":true,"users":[{"username":"checkpoint","is_staff":true}],"steps":[{"op":"create_company","ref":"c","name":"Synthetic checkpoints"},{"op":"save_account","company":"c","ref":"income","code":"400","name":"Income","kind":"INCOME"}]});
        let dir = tempfile::tempdir().unwrap();
        let mut runner = Runner::new(&scenario, &dir.path().join("checkpoints.fbbook"));
        runner.run();
        let good = runner.dump();
        assert!(super::compare(&good, &good).ok());
        let mut missing = good.clone();
        missing["checkpoints"].as_array_mut().unwrap().pop();
        assert!(!super::compare(&missing, &missing).ok());
        let mut bad = good.clone();
        bad["checkpoints"][0]["financial"]["c"]["balance_sheet"]["total_equity"] = json!(1);
        assert!(!super::compare(&bad, &bad).ok());
        let mut missing = good.clone();
        missing["checkpoints"][1]["dump"]
            .as_object_mut()
            .unwrap()
            .remove("prepayments");
        assert!(!super::compare(&missing, &missing).ok());
    }
}

#[cfg(test)]
mod aggregate_tests {
    use super::*;
    #[test]
    fn financial_aggregate_overflow_is_a_refusal() {
        let journal = json!({"company":"c","status":"POSTED","date":"2026-10-01","lines":[{"account":"a","debit":i64::MAX,"credit":0}]});
        let dump =
            json!({"companies":[{"ref":"c"}],"journals":[journal.clone(),journal],"accounts":[]});
        assert!(financial(&dump).is_err());
    }
}
