//! Typed differential adapter for the frozen credits services.
use super::*;
use fb_core::credits;

impl Runner<'_> {
    fn cr_ref(&self, key: &str) -> String {
        self.l2_refs.get(key).cloned().unwrap_or_else(|| key.into())
    }
    fn cr_input(&self, v: &Value) -> Value {
        match v {
            Value::Array(a) => json!(a.iter().map(|v| self.cr_input(v)).collect::<Vec<_>>()),
            Value::Object(m) => Value::Object(
                m.iter()
                    .map(|(k, v)| {
                        let v = if v.is_string()
                            && ["customer_id", "item_id", "invoice_id", "tax_item_id"]
                                .contains(&k.as_str())
                        {
                            json!(self.cr_ref(v.as_str().unwrap()))
                        } else if v.is_string()
                            && ["income_account_id", "bank_id"].contains(&k.as_str())
                        {
                            json!(
                                self.accounts
                                    .get(v.as_str().unwrap())
                                    .map(|a| a.to_db())
                                    .unwrap_or_else(|| AccountId::new().to_db())
                            )
                        } else {
                            self.cr_input(v)
                        };
                        (k.clone(), v)
                    })
                    .collect(),
            ),
            _ => v.clone(),
        }
    }
    fn cr_track(&mut self, name: &str, v: &Value) {
        self.l2_refs
            .entry(name.into())
            .or_insert(v["id"].as_str().unwrap().into());
        if let Some(id) = v["identity_id"].as_str() {
            self.l2_refs
                .entry(format!("{name}:source"))
                .or_insert(id.into());
        }
        if let Some(id) = v["journal_id"].as_str()
            && !self.journals.values().any(|old| old.to_db() == id)
        {
            self.journals
                .entry(format!("{name}:journal"))
                .or_insert(id.parse().unwrap());
        }
    }
    pub(super) fn apply_credits(&mut self, step: &Value, actor: UserId) -> Result<bool, StepError> {
        let op = s(step, "op");
        if !op.starts_with("credits_") {
            return Ok(false);
        }
        let c = find(&self.companies, s(step, "company"))?;
        let p = self.cr_input(step.get("values").unwrap_or(&json!({})));
        let id = self.cr_ref(step["record"].as_str().unwrap_or(""));
        match op {
            "credits_review_memo" | "credits_review_application" | "credits_review_refund" => {
                let r = match op {
                    "credits_review_memo" => {
                        credits::review_credit_memo(&mut self.book, actor, c, &p)?
                    }
                    "credits_review_application" => {
                        credits::review_application(&mut self.book, actor, c, &id, &p)?
                    }
                    _ => credits::review_refund(&mut self.book, actor, c, &id, &p)?,
                };
                self.credits_reviews.insert(s(step, "review").into(), r);
            }
            "credits_record_memo" | "credits_apply" | "credits_refund" => {
                let r = step["review"]
                    .as_str()
                    .and_then(|r| self.credits_reviews.get(r))
                    .cloned();
                let v = match op {
                    "credits_record_memo" => {
                        credits::record_credit_memo(&mut self.book, actor, c, &p, r.as_ref())?
                    }
                    "credits_apply" => {
                        credits::apply_credit(&mut self.book, actor, c, &id, &p, r.as_ref())?
                    }
                    _ => credits::record_refund(&mut self.book, actor, c, &id, &p, r.as_ref())?,
                };
                self.cr_track(s(step, "ref"), &v);
            }
            "credits_void_memo" | "credits_unapply" | "credits_void_refund" => {
                let expected = step["expected_revision"].as_i64();
                let v = match op {
                    "credits_void_memo" => credits::void_credit_memo(
                        &mut self.book,
                        actor,
                        c,
                        &id,
                        expected,
                        &step["date"],
                        &step["reason"],
                    )?,
                    "credits_unapply" => credits::unapply_credit(
                        &mut self.book,
                        actor,
                        c,
                        &id,
                        expected,
                        &step["date"],
                        &step["reason"],
                    )?,
                    _ => credits::void_refund(
                        &mut self.book,
                        actor,
                        c,
                        &id,
                        expected,
                        &step["date"],
                        &step["reason"],
                    )?,
                };
                if let Some(id) = v["journal_id"].as_str()
                    && !self.journals.values().any(|old| old.to_db() == id)
                {
                    self.journals
                        .entry(format!("{}:reversal", s(step, "record")))
                        .or_insert(id.parse().unwrap());
                }
            }
            _ => {
                return Err(StepError::MissingRef(format!(
                    "unknown credits operation {op}"
                )));
            }
        }
        Ok(true)
    }
    fn cr_ids(&mut self) -> audit::Ids {
        let mut ids = audit::Ids::new();
        for (kind, map) in [
            (
                "company",
                self.companies
                    .iter()
                    .map(|(k, v)| (v.to_db(), k.clone()))
                    .collect::<HashMap<_, _>>(),
            ),
            (
                "account",
                self.accounts
                    .iter()
                    .map(|(k, v)| (v.to_db(), k.clone()))
                    .collect(),
            ),
            (
                "journal",
                self.journal_names(),
            ),
            (
                "user",
                self.users
                    .iter()
                    .map(|(k, v)| (v.to_db(), k.clone()))
                    .collect(),
            ),
        ] {
            ids.insert(kind.into(), map);
        }
        let refs = self.l2_refs.clone();
        self.book
            .read(|tx| -> fb_core::FbResult<()> {
                for (kind, table) in [
                    ("creditmemo", "credits_creditmemo"),
                    ("creditapplication", "credits_creditapplication"),
                    ("creditrefund", "credits_creditrefund"),
                    ("invoice", "sales_invoice"),
                    ("customer", "directory_customer"),
                    ("item", "directory_item"),
                    ("taxitem", "salestax_taxitem"),
                    ("vendor", "directory_vendor"),
                    ("identity", "documents_documentidentity"),
                ] {
                    let mut stmt = tx.prepare(&format!("SELECT id FROM {table} ORDER BY id"))?;
                    let rows = stmt
                        .query_map([], |r| r.get::<_, String>(0))?
                        .collect::<Result<Vec<_>, _>>()?;
                    for (rank, id) in rows.into_iter().enumerate() {
                        let name = refs
                            .iter()
                            .filter(|(_, v)| **v == id)
                            .map(|(k, _)| k.clone())
                            .min()
                            .unwrap_or_else(|| format!("unreferenced-{kind}:{rank}"));
                        ids.entry(kind.into()).or_default().insert(id, name);
                    }
                }
                for (kind, table, parent_field, parent_kind, suffix) in [
                    (
                        "creditmemovoid",
                        "credits_creditmemovoid",
                        "credit_memo_id",
                        "creditmemo",
                        "void",
                    ),
                    (
                        "creditrefundvoid",
                        "credits_creditrefundvoid",
                        "refund_id",
                        "creditrefund",
                        "void",
                    ),
                    (
                        "creditunapplication",
                        "credits_creditunapplication",
                        "application_id",
                        "creditapplication",
                        "unapply",
                    ),
                    (
                        "creditmemoline",
                        "credits_creditmemoline",
                        "credit_memo_id",
                        "creditmemo",
                        "line",
                    ),
                    (
                        "creditapplicationline",
                        "credits_creditapplicationline",
                        "application_id",
                        "creditapplication",
                        "invoice",
                    ),
                    (
                        "invoicerevision",
                        "sales_invoicerevision",
                        "invoice_id",
                        "invoice",
                        "revision",
                    ),
                ] {
                    let extra = match kind {
                        "creditmemoline" => ",position",
                        "creditapplicationline" => ",invoice_id",
                        "invoicerevision" => ",number",
                        _ => "",
                    };
                    let mut stmt = tx.prepare(&format!(
                        "SELECT id,{parent_field}{extra} FROM {table} ORDER BY id"
                    ))?;
                    let rows = stmt
                        .query_map([], |r| {
                            Ok((
                                r.get::<_, String>(0)?,
                                r.get::<_, String>(1)?,
                                if extra.is_empty() {
                                    None
                                } else {
                                    Some(match kind {
                                        "creditapplicationline" => r.get::<_, String>(2)?,
                                        _ => r.get::<_, i64>(2)?.to_string(),
                                    })
                                },
                            ))
                        })?
                        .collect::<Result<Vec<_>, _>>()?;
                    for (id, parent, extra) in rows {
                        let p = ids
                            .get(parent_kind)
                            .and_then(|m| m.get(&parent))
                            .cloned()
                            .unwrap_or_else(|| "?".into());
                        let suffix = if let Some(extra) = extra {
                            let extra = if kind == "creditapplicationline" {
                                ids["invoice"]
                                    .get(&extra)
                                    .cloned()
                                    .unwrap_or_else(|| "?".into())
                            } else {
                                extra
                            };
                            format!("{suffix}:{extra}")
                        } else {
                            suffix.into()
                        };
                        ids.entry(kind.into())
                            .or_default()
                            .insert(id, format!("{p}:{suffix}"));
                    }
                }
                Ok(())
            })
            .unwrap();
        ids
    }
    pub(super) fn extend_credits_audit_ids(&mut self, ids: &mut audit::Ids) {
        ids.extend(self.cr_ids());
        let sources = self
            .book
            .read(|tx| -> fb_core::FbResult<HashMap<String, String>> {
                let mut st = tx.prepare("SELECT identity_id,id FROM credits_creditmemo")?;
                Ok(st
                    .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<Result<_, _>>()?)
            })
            .unwrap();
        ids.insert("credits_sources".into(), sources);
    }
    pub(super) fn dump_credits(&mut self) -> Value {
        match self.try_dump_credits() {
            Ok(v) => v,
            Err(e) => json!({"error":e.message()}),
        }
    }
    fn try_dump_credits(&mut self) -> fb_core::FbResult<Value> {
        let ids = self.cr_ids();
        let mut out = json!({});
        self.book.read(|tx| -> fb_core::FbResult<()> {
            for t in [
                "creditmemo",
                "creditmemoline",
                "creditmemovoid",
                "creditapplication",
                "creditapplicationline",
                "creditunapplication",
                "creditrefund",
                "creditrefundvoid",
            ] {
                let mut stmt = tx.prepare(&format!("SELECT data FROM credits_{t} ORDER BY id"))?;
                let raw = stmt
                    .query_map([], |r| r.get::<_, std::string::String>(0))?
                    .collect::<Result<Vec<_>, _>>()?;
                let mut normalized = Vec::new();
                for raw in raw {
                    let mut v: Value = fb_db::parse_unique_json_as(&raw)
                        .map_err(|_| FbError::validation("Invalid stored credit evidence."))?;
                    if !fb_db::credit_fields_valid(t, &v) {
                        return Err(FbError::validation("Invalid stored credit evidence."));
                    }
                    let object = v
                        .as_object_mut()
                        .ok_or_else(|| FbError::validation("Invalid stored credit evidence."))?;
                    object.remove("creation_digest");
                    object.remove("review_digest");
                    normalize_row(&mut v, t, &ids);
                    normalized.push(v);
                }
                normalized.sort_by_key(|v| v["id"].as_str().unwrap_or("").to_owned());
                out[t] = json!(normalized);
            }
            Ok(())
        })?;
        let mut balances = vec![];
        for name in self.company_order.clone() {
            let c = self.companies[&name];
            let actor = self.company_actor[&name];
            for mut row in credits::memo_balances(&mut self.book, actor, c)? {
                normalize_row(&mut row, "creditmemo", &ids);
                row["company_id"] = json!(name);
                balances.push(row);
            }
        }
        balances.sort_by_key(|v| v["id"].as_str().unwrap_or("").to_owned());
        out["balances"] = json!(balances);
        Ok(out)
    }
    pub(super) fn credits_l2(&mut self, mut value: Value) -> Value {
        let names: HashMap<_, _> = self
            .accounts
            .iter()
            .map(|(k, v)| (v.to_db(), k.clone()))
            .collect();
        let sources = self
            .book
            .read(|tx| -> fb_core::FbResult<Vec<String>> {
                let mut st = tx.prepare("SELECT identity_id FROM credits_creditmemo")?;
                Ok(st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?)
            })
            .unwrap();
        let mut groups = std::collections::BTreeSet::new();
        for row in value["snapshots"].as_array_mut().unwrap() {
            let source = row["source"].as_str().unwrap();
            if !self
                .l2_refs
                .get(source)
                .is_some_and(|id| sources.contains(id))
            {
                continue;
            }
            groups.insert(source.to_owned());
            if let Some(raw) = row["role"].as_str().and_then(|s| s.strip_prefix("income:"))
                && let Some(name) = names.get(raw)
            {
                row["role"] = json!(format!("income:{name}"));
            }
        }
        let rows = value["snapshots"].as_array_mut().unwrap();
        for source in groups {
            let indices = rows
                .iter()
                .enumerate()
                .filter(|(_, v)| v["source"] == source)
                .map(|(i, _)| i)
                .collect::<Vec<_>>();
            let mut group = indices.iter().map(|i| rows[*i].clone()).collect::<Vec<_>>();
            group.sort_by_key(|v| v["role"].as_str().unwrap_or("").to_owned());
            for (i, row) in indices.into_iter().zip(group) {
                rows[i] = row;
            }
        }
        value
    }
}
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
fn tax(v: &mut Value, ids: &audit::Ids) {
    match v {
        Value::Array(a) => {
            for v in a {
                tax(v, ids)
            }
        }
        Value::Object(m) => {
            for (k, v) in m {
                if ["id", "agency_id"].contains(&k.as_str()) {
                    *v = mapped(v, if k == "id" { "taxitem" } else { "vendor" }, ids);
                } else {
                    tax(v, ids)
                }
            }
        }
        _ => {}
    }
}
fn normalize_row(v: &mut Value, kind: &str, ids: &audit::Ids) {
    let Some(m) = v.as_object_mut() else { return };
    for (k, v) in m {
        let fk = match k.as_str() {
            "id" => Some(kind),
            "credit_memo_id" => Some("creditmemo"),
            "application_id" => Some("creditapplication"),
            "refund_id" => Some("creditrefund"),
            "company_id" => Some("company"),
            "identity_id" => Some("identity"),
            "customer_id" => Some("customer"),
            "actor_id" => Some("user"),
            "journal_id" => Some("journal"),
            "item_id" => Some("item"),
            "income_account_id" | "receivables_id" | "collected_tax_id" | "bank_id" => {
                Some("account")
            }
            "tax_item_id" => Some("taxitem"),
            "invoice_id" => Some("invoice"),
            "invoice_revision_id" => Some("invoicerevision"),
            _ => None,
        };
        if let Some(fk) = fk {
            *v = mapped(v, fk, ids);
        } else if k == "tax_snapshot" {
            tax(v, ids)
        } else if k == "account_snapshot" {
            if let Some(m) = v.as_object_mut() {
                for a in m.values_mut() {
                    if let Some(id) = a.get_mut("id") {
                        *id = mapped(id, "account", ids);
                    }
                }
            }
        } else if let Some(fk) = match k.as_str() {
            "customer_snapshot" => Some("customer"),
            "company_snapshot" => Some("company"),
            "item_snapshot" => Some("item"),
            _ => None,
        } && v.get("id").is_some()
        {
            v["id"] = mapped(&v["id"], fk, ids);
        }
    }
}
pub(super) fn normalize_audit(event: &mut Value, ids: &audit::Ids) -> bool {
    if event["action"] == "document.accounts"
        && event["target"]
            .as_str()
            .and_then(|s| s.strip_prefix("documentidentity:"))
            .is_some_and(|id| {
                ids.get("credits_sources")
                    .is_some_and(|m| m.contains_key(id))
            })
    {
        let Some(rows) = event["detail"]["accounts"].as_array_mut() else {
            return false;
        };
        for a in rows.iter_mut() {
            if let Some(raw) = a["role"].as_str().and_then(|s| s.strip_prefix("income:")) {
                let name = mapped(&json!(raw), "account", ids);
                a["role"] = json!(format!("income:{}", name.as_str().unwrap_or("")));
            }
        }
        rows.sort_by_key(|v| v["role"].as_str().unwrap_or("").to_owned());
    }
    let Some((kind, raw)) = event["target"].as_str().and_then(|s| s.split_once(':')) else {
        return false;
    };
    if !["creditmemo", "creditrefund"].contains(&kind) {
        return false;
    }
    event["target"] = json!(format!(
        "{kind}:{}",
        mapped(&json!(raw), kind, ids).as_str().unwrap_or("")
    ));
    let detail = &mut event["detail"];
    for (k, kind) in [
        ("journal_id", "journal"),
        ("receivables_id", "account"),
        ("bank_id", "account"),
        ("application_id", "creditapplication"),
    ] {
        if let Some(v) = detail.get_mut(k) {
            *v = mapped(v, kind, ids);
        }
    }
    for (k, field, kind) in [
        ("lines", "income_account_id", "account"),
        ("invoices", "invoice_id", "invoice"),
    ] {
        if let Some(rows) = detail.get_mut(k).and_then(Value::as_array_mut) {
            for row in rows {
                if let Some(value) = row.get_mut(field) {
                    *value = mapped(value, kind, ids);
                }
            }
        }
    }
    true
}

#[cfg(test)]
include!("credits_review_tests.rs");
