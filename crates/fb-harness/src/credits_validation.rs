// Called only after the exact structural Shape has accepted the dump.
fn scaled(v: &Value, places: usize, max: i128) -> Option<i128> {
    let s = v.as_str()?;
    let (whole, fraction) = s.split_once('.')?;
    if whole.is_empty()
        || (whole.len() > 1 && whole.starts_with('0'))
        || fraction.len() != places
        || !whole
            .bytes()
            .chain(fraction.bytes())
            .all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let n = whole
        .parse::<i128>()
        .ok()?
        .checked_mul(10i128.pow(places as u32))?
        .checked_add(fraction.parse::<i128>().ok()?)?;
    (n <= max).then_some(n)
}
fn money(v: &Value) -> Option<i128> {
    scaled(v, 2, fb_money::MAX_TOTAL_CENTS)
}
fn percent(v: &Value) -> bool {
    scaled(v, 4, 1_000_000).is_some()
}
fn canonical_date(v: &Value) -> bool {
    v.as_str().is_some_and(|s| {
        s.len() == 10 && fb_core::py::model_date(s).is_ok_and(|d| d.to_string() == s)
    })
}
fn rows<'a>(v: &'a Value, table: &str) -> &'a [Value] {
    v[table].as_array().map(Vec::as_slice).unwrap_or(&[])
}
fn validate_values(d: &Value) -> Result<(), std::string::String> {
    let c = &d["credits"];
    let invalid = || "/credits: invalid values or incomplete evidence".to_owned();
    for table in [
        "creditmemo",
        "creditmemoline",
        "creditmemovoid",
        "creditapplication",
        "creditapplicationline",
        "creditunapplication",
        "creditrefund",
        "creditrefundvoid",
        "balances",
    ] {
        for r in rows(c, table) {
            if r.get("date").is_some_and(|v| !canonical_date(v)) {
                return Err(invalid());
            }
            for field in [
                "subtotal",
                "taxable",
                "tax",
                "total",
                "amount",
                "rate",
                "applied",
                "refunded",
                "available",
            ] {
                if r.get(field)
                    .is_some_and(|v| !v.is_boolean() && money(v).is_none())
                {
                    return Err(invalid());
                }
            }
            if table == "creditmemoline" {
                if !r["position"]
                    .as_u64()
                    .is_some_and(|p| (1..=50).contains(&p))
                    || fb_core::calculations::quantity(&r["quantity"]).is_err()
                    || r["account_snapshot"]["income_account"]["id"] != r["income_account_id"]
                    || r["account_snapshot"]["income_account"]["kind"] != "INCOME"
                {
                    return Err(invalid());
                }
                let item = &r["item_snapshot"];
                if r["item_id"].is_null() {
                    if !item.as_object().is_some_and(|m| m.is_empty()) {
                        return Err(invalid());
                    }
                } else if item["id"] != r["item_id"]
                    || !matches!(item["kind"].as_str(), Some("SERVICE" | "NONSTOCK"))
                    || (!item["sale_rate"].is_null() && money(&item["sale_rate"]).is_none())
                {
                    return Err(invalid());
                }
            }
            if matches!(table, "creditmemo" | "creditrefund")
                && (r["customer_snapshot"]["id"] != r["customer_id"]
                    || r["company_snapshot"]["id"] != r["company_id"])
            {
                return Err(invalid());
            }
            if table == "creditmemo" {
                if !matches!(
                    r["tax_mode"].as_str(),
                    Some("NONE" | "MANUAL" | "RATE" | "GROUP")
                ) || !percent(&r["tax_rate"])
                {
                    return Err(invalid());
                }
                if matches!(r["tax_mode"].as_str(), Some("RATE" | "GROUP")) {
                    let s = &r["tax_snapshot"];
                    let members = rows(s, "members");
                    if s["id"] != r["tax_item_id"]
                        || s["kind"] != r["tax_mode"]
                        || !percent(&s["rate"])
                        || money(&s["taxable"]).is_none()
                        || money(&s["tax"]).is_none()
                        || (r["tax_mode"] == "RATE"
                            && (members.len() != 1 || members[0]["id"] != r["tax_item_id"]))
                        || (r["tax_mode"] == "GROUP" && !(2..=20).contains(&members.len()))
                        || members
                            .iter()
                            .any(|m| !percent(&m["rate"]) || money(&m["tax"]).is_none())
                    {
                        return Err(invalid());
                    }
                } else if !r["tax_item_id"].is_null()
                    || !r["tax_snapshot"].as_object().is_some_and(|m| m.is_empty())
                {
                    return Err(invalid());
                }
            }
            if table == "creditrefund"
                && (!matches!(r["method"].as_str(), Some("CHECK" | "CASH" | "OTHER"))
                    || (r["method"] != "CHECK" && r["check_number"] != "")
                    || !r["check_number"].as_str().is_some_and(|s| {
                        s.chars().count() <= 20
                            && s.chars().all(fb_db::credit_check_number_char_allowed)
                    })
                    || money(&r["amount"]) == Some(0))
            {
                return Err(invalid());
            }
        }
    }
    validate_audit_coverage(d)?;
    // Posted CM/RFD sources require their complete document. L2a numbering
    // commands may also allocate unused identities without posting a journal.
    for (kind, table, voids, parent) in [
        ("CM", "creditmemo", "creditmemovoid", "credit_memo_id"),
        ("RFD", "creditrefund", "creditrefundvoid", "refund_id"),
    ] {
        let expected = rows(&d["l2a"], "identities")
            .iter()
            .filter(|r| {
                r["kind"] == kind
                    && rows(&d["l2a"], "source_journals")
                        .iter()
                        .any(|j| j[0] == r["id"])
            })
            .map(|r| r["id"].as_str().unwrap_or(""))
            .collect::<BTreeSet<_>>();
        let actual = rows(c, table)
            .iter()
            .map(|r| r["identity_id"].as_str().unwrap_or(""))
            .collect::<BTreeSet<_>>();
        if expected != actual {
            return Err(invalid());
        }
        for r in rows(c, table) {
            let source = rows(&d["l2a"], "identities")
                .iter()
                .find(|s| s["id"] == r["identity_id"])
                .ok_or_else(invalid)?;
            if source["company_id"] != r["company_id"] {
                return Err(invalid());
            }
            let void = rows(c, voids).iter().find(|v| v[parent] == r["id"]);
            let mut required = vec![&r["journal_id"]];
            if let Some(v) = void {
                required.push(&v["journal_id"]);
            }
            let actual = rows(&d["l2a"], "source_journals")
                .iter()
                .filter(|s| s[0] == r["identity_id"])
                .map(|s| s[1].as_str().unwrap_or(""))
                .collect::<BTreeSet<_>>();
            if actual != required.iter().map(|v| v.as_str().unwrap_or("")).collect() {
                return Err(invalid());
            }
            for (i, id) in required.iter().enumerate() {
                let j = rows(d, "journals")
                    .iter()
                    .find(|j| &j["ref"] == *id)
                    .ok_or_else(invalid)?;
                if j["company"] != r["company_id"]
                    || j["status"] != "POSTED"
                    || (i == 0 && (!j["reversal_of"].is_null() || j["date"] != r["date"]))
                    || (i == 1
                        && (j["reversal_of"] != r["journal_id"]
                            || void.is_none_or(|v| j["date"] != v["date"])))
                {
                    return Err(invalid());
                }
            }
        }
    }
    for m in rows(c, "creditmemo") {
        let mut lines = rows(c, "creditmemoline")
            .iter()
            .filter(|l| l["credit_memo_id"] == m["id"])
            .cloned()
            .collect::<Vec<_>>();
        lines.sort_by_key(|l| l["position"].as_u64());
        if !(1..=50).contains(&lines.len())
            || lines
                .iter()
                .enumerate()
                .any(|(i, l)| l["position"].as_u64() != Some(i as u64 + 1))
        {
            return Err(invalid());
        }
        let calc = fb_core::calculations::totals(
            &lines,
            m["tax_mode"].as_str().unwrap_or(""),
            Some(&m["tax_rate"]),
            &m["tax_label"],
            Some(&m["tax_snapshot"]),
            true,
        )
        .map_err(|_| invalid())?;
        if [
            "subtotal",
            "taxable",
            "tax",
            "total",
            "tax_mode",
            "tax_rate",
            "tax_label",
            "tax_snapshot",
        ]
        .iter()
        .any(|k| calc[*k] != m[*k])
            || lines
                .iter()
                .enumerate()
                .any(|(i, l)| l["amount"] != calc["lines"][i])
        {
            return Err(invalid());
        }
        let applied: i128 = rows(c, "creditapplication")
            .iter()
            .filter(|a| {
                a["credit_memo_id"] == m["id"]
                    && !rows(c, "creditunapplication")
                        .iter()
                        .any(|u| u["application_id"] == a["id"])
            })
            .map(|a| money(&a["amount"]).unwrap_or(0))
            .sum();
        let refunded: i128 = rows(c, "creditrefund")
            .iter()
            .filter(|a| {
                a["credit_memo_id"] == m["id"]
                    && !rows(c, "creditrefundvoid")
                        .iter()
                        .any(|u| u["refund_id"] == a["id"])
            })
            .map(|a| money(&a["amount"]).unwrap_or(0))
            .sum();
        let total = money(&m["total"]).ok_or_else(invalid)?;
        let void = rows(c, "creditmemovoid")
            .iter()
            .any(|v| v["credit_memo_id"] == m["id"]);
        let available = if void { 0 } else { total - applied - refunded };
        let state = if void {
            "VOID"
        } else if available == 0 {
            "USED"
        } else if available < total {
            "PARTIAL"
        } else {
            "AVAILABLE"
        };
        let b = rows(c, "balances")
            .iter()
            .find(|b| b["id"] == m["id"])
            .ok_or_else(invalid)?;
        if total <= 0
            || applied + refunded > total
            || (void && applied + refunded > 0)
            || b["company_id"] != m["company_id"]
            || b["customer_id"] != m["customer_id"]
            || b["total"] != m["total"]
            || money(&b["applied"]) != Some(applied)
            || money(&b["refunded"]) != Some(refunded)
            || money(&b["available"]) != Some(available)
            || b["credit_state"] != state
        {
            return Err(invalid());
        }
    }
    for a in rows(c, "creditapplication") {
        let ls = rows(c, "creditapplicationline")
            .iter()
            .filter(|l| l["application_id"] == a["id"])
            .collect::<Vec<_>>();
        let sum: i128 = ls.iter().map(|l| money(&l["amount"]).unwrap_or(0)).sum();
        if !(1..=50).contains(&ls.len()) || money(&a["amount"]) != Some(sum) || sum <= 0 {
            return Err(invalid());
        }
        for l in ls {
            if money(&l["amount"]) == Some(0)
                || !rows(&d["sales"], "invoices")
                    .iter()
                    .any(|i| i["ref"] == l["invoice_id"])
            {
                return Err(invalid());
            }
        }
    }
    Ok(())
}

// Totals cannot prove child coverage (notably zero-value memo lines), and
// released applications no longer affect balances. Cross-check each side's
// complete child tuples against immutable audit evidence before comparing dumps.
fn validate_audit_coverage(d: &Value) -> Result<(), std::string::String> {
    let c = &d["credits"];
    let invalid = || "/credits: audit/document coverage mismatch".to_owned();
    let events = d["audit"]
        .as_object()
        .into_iter()
        .flat_map(|m| m.values())
        .flat_map(|v| v.as_array().into_iter().flatten())
        .collect::<Vec<_>>();
    for (action, table, key) in [
        ("credit_memo.recorded", "creditmemo", "id"),
        ("credit_memo.voided", "creditmemovoid", "credit_memo_id"),
        ("credit_refund.recorded", "creditrefund", "id"),
        ("credit_refund.voided", "creditrefundvoid", "refund_id"),
        ("credit_memo.applied", "creditapplication", "id"),
        (
            "credit_memo.unapplied",
            "creditunapplication",
            "application_id",
        ),
    ] {
        let audit_keys = events
            .iter()
            .filter(|a| a["action"] == action)
            .map(|a| {
                if matches!(action, "credit_memo.applied" | "credit_memo.unapplied") {
                    a["detail"]["application_id"].as_str().unwrap_or("")
                } else {
                    a["target"]
                        .as_str()
                        .and_then(|s| s.split_once(':'))
                        .map(|(_, id)| id)
                        .unwrap_or("")
                }
            })
            .collect::<Vec<_>>();
        let record_keys = rows(c, table)
            .iter()
            .map(|r| r[key].as_str().unwrap_or(""))
            .collect::<BTreeSet<_>>();
        if audit_keys.len() != record_keys.len()
            || audit_keys.into_iter().collect::<BTreeSet<_>>() != record_keys
        {
            return Err(invalid());
        }
    }
    for m in rows(c, "creditmemo") {
        let event = events
            .iter()
            .find(|e| {
                e["action"] == "credit_memo.recorded"
                    && e["target"] == format!("creditmemo:{}", m["id"].as_str().unwrap_or(""))
            })
            .ok_or_else(invalid)?;
        let detail = &event["detail"];
        if detail["total"] != m["total"] || detail["receivables_id"] != m["receivables_id"] {
            return Err(invalid());
        }
        fn tuple(l: &Value) -> Option<(u64, &str, i128)> {
            Some((
                l["position"].as_u64()?,
                l["income_account_id"].as_str()?,
                money(&l["amount"])?,
            ))
        }
        let mut actual = rows(c, "creditmemoline")
            .iter()
            .filter(|l| l["credit_memo_id"] == m["id"])
            .map(tuple)
            .collect::<Option<Vec<_>>>()
            .ok_or_else(invalid)?;
        let mut audited = detail["lines"]
            .as_array()
            .ok_or_else(invalid)?
            .iter()
            .map(tuple)
            .collect::<Option<Vec<_>>>()
            .ok_or_else(invalid)?;
        actual.sort_unstable();
        audited.sort_unstable();
        // Vec equality checks count and multiplicity as well as tuple values.
        if actual != audited {
            return Err(invalid());
        }
    }
    for a in rows(c, "creditapplication") {
        let event = events
            .iter()
            .find(|e| {
                e["action"] == "credit_memo.applied" && e["detail"]["application_id"] == a["id"]
            })
            .ok_or_else(invalid)?;
        let detail = &event["detail"];
        if detail["date"] != a["date"]
            || detail["amount"] != a["amount"]
            || event["target"]
                != format!("creditmemo:{}", a["credit_memo_id"].as_str().unwrap_or(""))
        {
            return Err(invalid());
        }
        fn tuple(l: &Value) -> Option<(&str, i128)> {
            Some((l["invoice_id"].as_str()?, money(&l["amount"])?))
        }
        let mut allocations = rows(c, "creditapplicationline")
            .iter()
            .filter(|l| l["application_id"] == a["id"])
            .map(tuple)
            .collect::<Option<Vec<_>>>()
            .ok_or_else(invalid)?;
        let mut audited = detail["invoices"]
            .as_array()
            .ok_or_else(invalid)?
            .iter()
            .map(tuple)
            .collect::<Option<Vec<_>>>()
            .ok_or_else(invalid)?;
        allocations.sort_unstable();
        audited.sort_unstable();
        if audited != allocations {
            return Err(invalid());
        }
    }
    // Refund children are journal lines, not a separate credits table. The
    // recorded amount/bank and original memo's audited receivables mapping
    // require exactly two lines, including after the refund is voided.
    for r in rows(c, "creditrefund") {
        let event = events
            .iter()
            .find(|e| {
                e["action"] == "credit_refund.recorded"
                    && e["target"] == format!("creditrefund:{}", r["id"].as_str().unwrap_or(""))
            })
            .ok_or_else(invalid)?;
        let detail = &event["detail"];
        let memo = rows(c, "creditmemo")
            .iter()
            .find(|m| m["id"] == r["credit_memo_id"])
            .ok_or_else(invalid)?;
        let identity = rows(&d["l2a"], "identities")
            .iter()
            .find(|i| i["id"] == memo["identity_id"])
            .ok_or_else(invalid)?;
        if ["amount", "bank_id", "method", "check_number", "memo"]
            .iter()
            .any(|k| detail[*k] != r[*k])
            || detail["credit_memo"] != identity["number"]
            || r["receivables_id"] != memo["receivables_id"]
        {
            return Err(invalid());
        }
        let amount = money(&detail["amount"]).ok_or_else(invalid)?;
        let bank = detail["bank_id"].as_str().ok_or_else(invalid)?;
        let receivables = memo["receivables_id"].as_str().ok_or_else(invalid)?;
        let mut journals = vec![(&r["journal_id"], false)];
        if let Some(void) = rows(c, "creditrefundvoid")
            .iter()
            .find(|v| v["refund_id"] == r["id"])
        {
            journals.push((&void["journal_id"], true));
        }
        for (id, reverse) in journals {
            let journal = rows(d, "journals")
                .iter()
                .find(|j| j["ref"] == *id)
                .ok_or_else(invalid)?;
            let mut actual = rows(journal, "lines")
                .iter()
                .map(|l| {
                    Some((
                        l["position"].as_u64()?,
                        l["account"].as_str()?,
                        i128::from(l["debit"].as_i64()?),
                        i128::from(l["credit"].as_i64()?),
                    ))
                })
                .collect::<Option<Vec<_>>>()
                .ok_or_else(invalid)?;
            actual.sort_unstable();
            let (debit, credit) = if reverse { (0, amount) } else { (amount, 0) };
            if actual != vec![(1, receivables, debit, credit), (2, bank, credit, debit)] {
                return Err(invalid());
            }
        }
    }
    Ok(())
}
