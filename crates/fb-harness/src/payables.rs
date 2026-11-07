//! Differential adapters for the real payables service boundary.
use super::*;
use fb_core::payables;
use fb_db::rusqlite::{OptionalExtension, types::Value as SqlValue};
impl Runner<'_> {
    fn pref(&self, r: &str) -> String {
        self.l2_refs.get(r).cloned().unwrap_or_else(|| r.into())
    }
    pub(super) fn pinput(&self, v: &Value) -> Value {
        match v {
            Value::Array(a) => json!(a.iter().map(|x| self.pinput(x)).collect::<Vec<_>>()),
            Value::Object(m) => {
                let mut o = json!({});
                for (k, v) in m {
                    o[k] = if ["vendor_id", "item_id", "bill_id", "supersedes_id"]
                        .contains(&k.as_str())
                        && v.is_string()
                    {
                        json!(self.pref(v.as_str().unwrap()))
                    } else if ["expense_account_id", "bank_id"].contains(&k.as_str())
                        && v.is_string()
                    {
                        json!(
                            self.accounts
                                .get(v.as_str().unwrap())
                                .map(|a| a.to_db())
                                .unwrap_or_else(|| AccountId::new().to_db())
                        )
                    } else {
                        self.pinput(v)
                    };
                }
                o
            }
            _ => v.clone(),
        }
    }
    pub(super) fn ptrack(&mut self, name: &str, v: &Value, bill: bool) {
        self.l2_refs
            .entry(name.into())
            .or_insert(v["id"].as_str().unwrap().into());
        if let Some(id) = v["identity_id"].as_str() {
            self.l2_refs
                .entry(format!("{name}:source"))
                .or_insert(id.into());
        }
        let r = if bill { &v["current"] } else { v };
        for (field, suffix) in [("journal_id", "journal"), ("reversal_id", "reversal")] {
            if let Some(id) = r[field].as_str() {
                self.journals
                    .entry(format!("{name}:{suffix}"))
                    .or_insert(id.parse().unwrap());
            }
        }
    }
    pub(super) fn apply_payables(
        &mut self,
        step: &Value,
        actor: UserId,
    ) -> Result<bool, StepError> {
        let op = s(step, "op");
        if !op.starts_with("payables_") {
            return Ok(false);
        }
        let co = find(&self.companies, s(step, "company"))?;
        let p = self.pinput(step.get("values").unwrap_or(&json!({})));
        let expected = step["expected_revision"].as_i64();
        match op {
            "payables_receipt" => {
                let vendor = self.pref(s(step, "vendor"));
                let b = payables::save_receipt_draft(
                    &mut self.book,
                    actor,
                    co,
                    &vendor,
                    &step["fields"],
                    s(step, "request_key"),
                )?;
                self.ptrack(s(step, "ref"), &b, true);
            }
            "payables_save" => {
                let b = payables::save_bill(&mut self.book, actor, co, &p)?;
                self.ptrack(s(step, "ref"), &b, true);
            }
            "payables_review" => {
                let id = self.pref(s(step, "bill"));
                let r = payables::review_bill(&mut self.book, actor, co, &id, expected)?;
                self.payables_reviews.insert(s(step, "review").into(), r);
            }
            "payables_post" => {
                let name = s(step, "bill");
                let id = self.pref(name);
                let r = if let Some(key) = step["review"].as_str() {
                    self.payables_reviews
                        .get(key)
                        .cloned()
                        .ok_or_else(|| StepError::MissingRef(format!("unknown ref {key}")))?
                } else {
                    payables::review_bill(&mut self.book, actor, co, &id, expected)?
                };
                let b = payables::post_bill(&mut self.book, actor, co, &id, expected, &r)?;
                self.ptrack(name, &b, true);
            }
            "payables_void" => {
                let name = s(step, "bill");
                let id = self.pref(name);
                let b = payables::void_bill(
                    &mut self.book,
                    actor,
                    co,
                    &id,
                    expected,
                    &step["date"],
                    &step["reason"],
                )?;
                self.ptrack(name, &b, true);
            }
            "payables_review_payment" => {
                let r = payables::review_payment(&mut self.book, actor, co, &p)?;
                self.payables_reviews.insert(s(step, "review").into(), r);
            }
            "payables_pay" => {
                let r = if let Some(key) = step["review"].as_str() {
                    self.payables_reviews
                        .get(key)
                        .cloned()
                        .ok_or_else(|| StepError::MissingRef(format!("unknown ref {key}")))?
                } else {
                    payables::review_payment(&mut self.book, actor, co, &p)?
                };
                let b = payables::record_payment(&mut self.book, actor, co, &p, &r)?;
                self.ptrack(s(step, "ref"), &b, false);
            }
            "payables_reverse" => {
                let name = s(step, "payment");
                let id = self.pref(name);
                let b = payables::reverse_payment(
                    &mut self.book,
                    actor,
                    co,
                    &id,
                    expected,
                    &step["date"],
                    &step["reason"],
                )?;
                self.journals
                    .entry(format!("{name}:reversal"))
                    .or_insert(b["journal_id"].as_str().unwrap().parse().unwrap());
            }
            _ => panic!("unknown payables op {op}"),
        };
        Ok(true)
    }
    fn pnames(&self) -> HashMap<String, String> {
        let mut m = HashMap::new();
        let mut keys = self.l2_refs.keys().collect::<Vec<_>>();
        keys.sort();
        for k in keys {
            m.entry(self.l2_refs[k].clone()).or_insert(k.clone());
        }
        for (k, v) in &self.companies {
            m.insert(v.to_db(), k.clone());
        }
        for (k, v) in &self.accounts {
            m.insert(v.to_db(), k.clone());
        }
        m.extend(self.journal_names());
        for (k, v) in &self.users {
            m.insert(v.to_db(), k.clone());
        }
        m
    }
    pub(super) fn dump_payables(&mut self) -> Value {
        let mut names = self.pnames();
        let mut out = json!({});
        self.book
            .read(|c| -> fb_core::FbResult<()> {
                let mut all = vec![];
                for table in [
                    "bill",
                    "billrevision",
                    "billline",
                    "billpayment",
                    "paymentallocation",
                    "paymentreversal",
                ] {
                    let mut stmt = c
                        .prepare(&format!("SELECT * FROM payables_{table} ORDER BY id"))
                        .unwrap();
                    let cols = stmt.column_names().map(str::to_string).collect::<Vec<_>>();
                    let rows = stmt
                        .query_map([], |r| {
                            let mut v = json!({});
                            for (i, k) in cols.iter().enumerate() {
                                if [
                                    "created_at",
                                    "creation_digest",
                                    "review_digest",
                                    "origin_position",
                                ]
                                .contains(&k.as_str())
                                {
                                    continue;
                                }
                                v[k] = match r.get::<_, SqlValue>(i)? {
                                    SqlValue::Null => Value::Null,
                                    SqlValue::Integer(n) => json!(n),
                                    SqlValue::Text(t) => {
                                        if k.ends_with("_snapshot") {
                                            fb_db::parse_unique_json_as(&t).unwrap()
                                        } else {
                                            json!(t)
                                        }
                                    }
                                    _ => panic!("unexpected type"),
                                };
                            }
                            Ok(v)
                        })
                        .unwrap()
                        .collect::<Result<Vec<_>, _>>()
                        .unwrap();
                    for v in &rows {
                        let name = match table {
                            "billrevision" => format!(
                                "{}:revision:{}",
                                names.get(v["bill_id"].as_str().unwrap()).unwrap(),
                                v["number"]
                            ),
                            "billline" => format!(
                                "{}:line:{}",
                                names.get(v["revision_id"].as_str().unwrap()).unwrap(),
                                v["position"]
                            ),
                            "paymentallocation" => format!(
                                "{}:allocation:{}",
                                names.get(v["payment_id"].as_str().unwrap()).unwrap(),
                                names.get(v["bill_id"].as_str().unwrap()).unwrap()
                            ),
                            "paymentreversal" => format!(
                                "{}:reversal-record",
                                names.get(v["payment_id"].as_str().unwrap()).unwrap()
                            ),
                            _ => names
                                .get(v["id"].as_str().unwrap())
                                .cloned()
                                .unwrap_or("?".into()),
                        };
                        names.insert(v["id"].as_str().unwrap().into(), name);
                    }
                    all.push((table, rows));
                }
                for (table, rows) in all {
                    out[table] = normalize(&json!(rows), &names);
                }
                Ok(())
            })
            .unwrap();
        let mut balances = vec![];
        let mut tracker = vec![];
        let mut calendar = vec![];
        for co in self.companies.values().copied().collect::<Vec<_>>() {
            let actor:Option<String>=self.book.read(|c| c.query_row("SELECT u.id FROM fb_user u WHERE u.is_active=1 AND (u.is_staff=1 OR EXISTS(SELECT 1 FROM ledger_company_members m WHERE m.company_id=? AND m.user_id=u.id)) ORDER BY u.id LIMIT 1",[co.to_db()],|r|r.get(0)).optional()).unwrap();
            let Some(actor) = actor else { continue };
            let actor = actor.parse().unwrap();
            for b in payables::current_bills(&mut self.book, actor, co).unwrap() {
                balances.push(json!({"company_id":co.to_db(),"bill_id":b["id"],"payment_state":b["payment_state"],"allocated":b["allocated"],"outstanding":b["outstanding"],"paid_on":b["paid_on"]}));
            }
            let t =
                payables::bill_tracker(&mut self.book, actor, co, &json!({}), &json!("2026-11-23"))
                    .unwrap();
            let mut filtered = vec![];
            for spec in self.scenario["payables_tracker_filters"]
                .as_array()
                .into_iter()
                .flatten()
            {
                let mut filters = spec["filters"].clone();
                if let Some(vendor) = filters["vendor"].as_str() {
                    filters["vendor"] = json!(self.pref(vendor));
                }
                let result = payables::bill_tracker(
                    &mut self.book,
                    actor,
                    co,
                    &filters,
                    &json!("2026-11-23"),
                )
                .unwrap();
                let mut ids = result["rows"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|b| names[s(b, "id")].clone())
                    .collect::<Vec<_>>();
                ids.sort();
                filtered
                    .push(json!({"name":spec["name"],"bill_ids":ids,"totals":result["totals"]}));
            }
            tracker.push(json!({"company_id":co.to_db(),"totals":t["totals"],"filtered":filtered}));
            let cal = payables::bill_calendar(
                &mut self.book,
                actor,
                co,
                2026,
                11,
                &json!("2026-11-23"),
                true,
                None,
            )
            .unwrap();
            calendar.push(json!({"company_id":co.to_db(),"data":cal}));
        }
        out["balances"] = normalize(&json!(balances), &names);
        out["tracker"] = normalize(&json!(tracker), &names);
        out["calendar"] = normalize(&json!(calendar), &names);
        for key in ["balances", "tracker", "calendar"] {
            out[key].as_array_mut().unwrap().sort_by_key(|v| {
                format!(
                    "{}:{}",
                    s(v, "company_id"),
                    v["bill_id"].as_str().unwrap_or("")
                )
            });
        }
        out
    }
    pub(super) fn payables_l2(&self, mut v: Value) -> Value {
        let names = self.pnames();
        for row in v["snapshots"].as_array_mut().unwrap() {
            if let Some(role) = row["role"]
                .as_str()
                .and_then(|s| s.strip_prefix("payables:"))
            {
                row["role"] = json!(format!("payables:{}", names.get(role).unwrap()));
            }
        }
        v
    }
}
fn normalize(v: &Value, names: &HashMap<String, String>) -> Value {
    match v {
        Value::Array(a) => json!(a.iter().map(|v| normalize(v, names)).collect::<Vec<_>>()),
        Value::Object(m) => {
            let mut o = json!({});
            for (k, v) in m {
                o[k] = if [
                    "id",
                    "company_id",
                    "identity_id",
                    "vendor_id",
                    "item_id",
                    "bill_id",
                    "supersedes_id",
                    "revision_id",
                    "bill_revision_id",
                    "payment_id",
                    "journal_id",
                    "reversal_id",
                    "bank_id",
                    "payables_id",
                    "income_account_id",
                    "expense_account_id",
                    "actor_id",
                ]
                .contains(&k.as_str())
                {
                    v.as_str()
                        .map(|s| json!(names.get(s).map_or(s, String::as_str)))
                        .unwrap_or_else(|| v.clone())
                } else {
                    normalize(v, names)
                };
            }
            o
        }
        _ => v.clone(),
    }
}

impl Runner<'_> {
    pub(super) fn extend_payables_audit_ids(&mut self, ids: &mut audit::Ids) {
        let names = self.pnames();
        self.book
            .read(|c| -> fb_core::FbResult<()> {
                for kind in ["bill", "billpayment"] {
                    let mut stmt =
                        c.prepare(&format!("SELECT id FROM payables_{kind} ORDER BY id"))?;
                    for row in stmt.query_map([], |r| r.get::<_, String>(0))? {
                        let id = row?;
                        ids.entry(kind.into()).or_default().insert(
                            id.clone(),
                            names
                                .get(&id)
                                .cloned()
                                .unwrap_or_else(|| format!("<unknown:{id}>")),
                        );
                    }
                }
                Ok(())
            })
            .unwrap();
        ids.insert("journal".into(), self.journal_names());
    }
}

pub(super) fn normalize_audit(event: &mut Value, ids: &audit::Ids) -> bool {
    fn mapped(v: &Value, kind: &str, ids: &audit::Ids) -> Value {
        if v.is_null() {
            return Value::Null;
        }
        let raw = v
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| v.to_string());
        json!(
            ids.get(kind)
                .and_then(|m| m.get(&raw))
                .cloned()
                .unwrap_or_else(|| format!("<unknown:{kind}:{raw}>"))
        )
    }
    if event["action"] == "document.accounts" {
        for account in event["detail"]["accounts"].as_array_mut().unwrap() {
            if let Some(raw) = account["role"]
                .as_str()
                .and_then(|s| s.strip_prefix("payables:"))
            {
                let name = mapped(&json!(raw), "account", ids);
                account["role"] = json!(format!("payables:{}", name.as_str().unwrap()));
            }
        }
    }
    let Some((kind, raw)) = event["target"].as_str().and_then(|s| s.split_once(':')) else {
        return false;
    };
    if !["bill", "billpayment"].contains(&kind) {
        return false;
    }
    let name = mapped(&json!(raw), kind, ids);
    event["target"] = json!(format!("{kind}:{}", name.as_str().unwrap()));
    for (field, kind) in [("journal_id", "journal"), ("supersedes_id", "bill")] {
        if let Some(value) = event["detail"].get_mut(field) {
            *value = mapped(value, kind, ids);
        }
    }
    if let Some(rows) = event["detail"]
        .get_mut("allocations")
        .and_then(Value::as_array_mut)
    {
        for row in rows {
            row["bill_id"] = mapped(&row["bill_id"], "bill", ids);
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_payables_normalization_keeps_matching_free_text_and_business_numbers() {
        let names = [("raw-id".into(), "mapped".into())].into_iter().collect();
        let row = json!({"id":"raw-id","reference":"raw-id","memo":"raw-id","quantity":1,"vendor_snapshot":{"id":"raw-id","display_name":"raw-id"},"lines":[{"expense_account_id":"raw-id","description":"raw-id"}]});
        let actual = normalize(&row, &names);
        assert_eq!(actual["id"], "mapped");
        assert_eq!(actual["vendor_snapshot"]["id"], "mapped");
        assert_eq!(actual["lines"][0]["expense_account_id"], "mapped");
        for pointer in [
            "/reference",
            "/memo",
            "/vendor_snapshot/display_name",
            "/lines/0/description",
        ] {
            assert_eq!(actual.pointer(pointer), row.pointer(pointer));
        }
        assert_eq!(actual["quantity"], 1);
        let ids = [(
            "bill".into(),
            [("1".into(), "bill-one".into())].into_iter().collect(),
        )]
        .into_iter()
        .collect();
        let mut event = json!({"action":"bill.created","target":"bill:1","detail":{"supersedes_id":1,"reference":"1","memo":"1","total":"1.00","revision":1}});
        assert!(normalize_audit(&mut event, &ids));
        assert_eq!(event["target"], "bill:bill-one");
        assert_eq!(event["detail"]["supersedes_id"], "bill-one");
        assert_eq!(event["detail"]["reference"], "1");
        assert_eq!(event["detail"]["memo"], "1");
        assert_eq!(event["detail"]["total"], "1.00");
        assert_eq!(event["detail"]["revision"], 1);
        assert!(event["detail"].get("allocations").is_none());
    }
}
