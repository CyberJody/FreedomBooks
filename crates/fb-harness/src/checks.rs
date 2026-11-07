//! Checks and transfers differential adapter; full rows and typed audit IDs.
use super::*;
use fb_core::{checks, transfers};
use fb_db::rusqlite::types::Value as SqlValue;
impl Runner<'_> {
    fn cref(&self, r: &str) -> String {
        self.l2_refs.get(r).cloned().unwrap_or_else(|| r.into())
    }
    fn cinput(&self, v: &Value) -> Value {
        match v {
            Value::Array(a) => json!(a.iter().map(|x| self.cinput(x)).collect::<Vec<_>>()),
            Value::Object(m) => {
                let mut o = json!({});
                for (k, v) in m {
                    o[k] = if ["vendor_id", "item_id", "check_id", "bill_id"].contains(&k.as_str())
                        && v.is_string()
                    {
                        json!(self.cref(v.as_str().unwrap()))
                    } else if ["account_id", "bank_id", "from_account_id", "to_account_id"]
                        .contains(&k.as_str())
                        && v.is_string()
                    {
                        json!(
                            self.accounts
                                .get(v.as_str().unwrap())
                                .map(|a| a.to_db())
                                .unwrap_or_else(|| AccountId::new().to_db())
                        )
                    } else {
                        self.cinput(v)
                    };
                }
                o
            }
            _ => v.clone(),
        }
    }
    pub(super) fn ctrack(&mut self, name: &str, v: &Value, bill: bool) {
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
    pub(super) fn apply_checks(&mut self, step: &Value, actor: UserId) -> Result<bool, StepError> {
        let op = s(step, "op");
        if !op.starts_with("checks_") && !op.starts_with("transfers_") {
            return Ok(false);
        }
        let co = find(&self.companies, s(step, "company"))?;
        let p = self.cinput(step.get("values").unwrap_or(&json!({})));
        let expected = step["expected_revision"].as_i64();
        match op {
            "checks_save" => {
                let b = checks::save_check(&mut self.book, actor, co, &p)?;
                self.ctrack(s(step, "ref"), &b, true);
            }
            "checks_review" => {
                let id = self.cref(s(step, "check"));
                let r = checks::review_check(&mut self.book, actor, co, &id, expected)?;
                self.checks_reviews.insert(s(step, "review").into(), r);
            }
            "checks_record" => {
                let name = s(step, "check");
                let id = self.cref(name);
                let r = if let Some(key) = step["review"].as_str() {
                    self.checks_reviews[key].clone()
                } else {
                    checks::review_check(&mut self.book, actor, co, &id, expected)?
                };
                let b = checks::record_check(&mut self.book, actor, co, &id, expected, &r)?;
                self.ctrack(name, &b, true);
            }
            "checks_void" => {
                let name = s(step, "check");
                let id = self.cref(name);
                let b = checks::void_check(
                    &mut self.book,
                    actor,
                    co,
                    &id,
                    expected,
                    &step["date"],
                    &step["reason"],
                )?;
                self.ctrack(name, &b, true);
            }
            "checks_number" => {
                let name = s(step, "check");
                let id = self.cref(name);
                let b = checks::assign_number(
                    &mut self.book,
                    actor,
                    co,
                    &id,
                    expected,
                    &step["number"],
                )?;
                self.ctrack(name, &b, true);
            }
            "checks_next" => {
                let bank = find(&self.accounts, s(step, "bank"))?;
                let n = checks::next_number(&mut self.book, actor, co, bank)?;
                self.checks_numbers.insert(s(step, "ref").into(), json!(n));
            }
            "checks_review_payment" => {
                let r = checks::review_bill_payment(&mut self.book, actor, co, &p)?;
                self.payables_reviews.insert(s(step, "review").into(), r);
            }
            "checks_pay" => {
                let r = if let Some(key) = step["review"].as_str() {
                    self.payables_reviews[key].clone()
                } else {
                    checks::review_bill_payment(&mut self.book, actor, co, &p)?
                };
                let b = checks::record_bill_payment(&mut self.book, actor, co, &p, &r)?;
                self.ctrack(s(step, "ref"), &b, false);
            }
            "transfers_review" => {
                let r = transfers::review_transfer(&mut self.book, actor, co, &p)?;
                self.transfers_reviews.insert(s(step, "review").into(), r);
            }
            "transfers_record" => {
                let r = if let Some(key) = step["review"].as_str() {
                    self.transfers_reviews[key].clone()
                } else {
                    transfers::review_transfer(&mut self.book, actor, co, &p)?
                };
                let b = transfers::record_transfer(&mut self.book, actor, co, &p, &r)?;
                self.ctrack(s(step, "ref"), &b, false);
            }
            "transfers_void" => {
                let name = s(step, "transfer");
                let id = self.cref(name);
                let v = transfers::void_transfer(
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
                    .or_insert(v["journal_id"].as_str().unwrap().parse().unwrap());
            }
            _ => {
                return Err(StepError::MissingRef(format!(
                    "unknown checks operation {op}"
                )));
            }
        }
        Ok(true)
    }
    pub(super) fn cnames(&self) -> HashMap<String, String> {
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
    pub(super) fn dump_checks(&mut self) -> Value {
        let mut names = self.cnames();
        let mut out = json!({});
        self.book
            .read(|c| -> fb_core::FbResult<()> {
                let mut all = vec![];
                for table in [
                    "check",
                    "checkrevision",
                    "checkline",
                    "transfer",
                    "transfervoid",
                ] {
                    let mut stmt = c
                        .prepare(&format!(
                            "SELECT * FROM {}_{table} ORDER BY id",
                            if table.starts_with("transfer") {
                                "transfers"
                            } else {
                                "checks"
                            }
                        ))
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
                            "checkrevision" => format!(
                                "{}:revision:{}",
                                names.get(v["document_id"].as_str().unwrap()).unwrap(),
                                v["number"]
                            ),
                            "checkline" => format!(
                                "{}:line:{}",
                                names.get(v["revision_id"].as_str().unwrap()).unwrap(),
                                v["position"]
                            ),
                            "transfervoid" => format!(
                                "{}:void-record",
                                names.get(v["transfer_id"].as_str().unwrap()).unwrap()
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
                    out[table]
                        .as_array_mut()
                        .unwrap()
                        .sort_by_key(|r| s(r, "id").to_owned());
                }
                Ok(())
            })
            .unwrap();
        out["numbers"] = json!(self.checks_numbers);
        out
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
                    "document_id",
                    "transfer_id",
                    "supersedes_id",
                    "revision_id",
                    "bill_revision_id",
                    "payment_id",
                    "journal_id",
                    "reversal_id",
                    "bank_id",
                    "account_id",
                    "from_account_id",
                    "to_account_id",
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
    pub(super) fn extend_checks_audit_ids(&mut self, ids: &mut audit::Ids) {
        let names = self.cnames();
        self.book
            .read(|c| -> fb_core::FbResult<()> {
                for kind in ["check", "transfer"] {
                    let mut stmt = c.prepare(&format!(
                        "SELECT id FROM {}_{kind} ORDER BY id",
                        if kind == "transfer" {
                            "transfers"
                        } else {
                            "checks"
                        }
                    ))?;
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
    let Some((kind, raw)) = event["target"].as_str().and_then(|s| s.split_once(':')) else {
        return false;
    };
    if !["check", "transfer"].contains(&kind) {
        return false;
    }
    let name = mapped(&json!(raw), kind, ids);
    event["target"] = json!(format!("{kind}:{}", name.as_str().unwrap()));
    for (field, kind) in [
        ("journal_id", "journal"),
        ("bank_id", "account"),
        ("from_account_id", "account"),
        ("to_account_id", "account"),
    ] {
        if let Some(v) = event["detail"].get_mut(field) {
            *v = mapped(v, kind, ids);
        }
    }
    true
}
