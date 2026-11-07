//! Sales scenario vocabulary and full frozen-domain dump (no test-only core writes).
pub(crate) mod audit;
pub(crate) mod estimates_manifest;
pub(super) mod shape;
use super::*;
use fb_core::sales;
use fb_db::rusqlite::OptionalExtension;
const HEADER: &[&str] = &[
    "number",
    "state",
    "action",
    "reason",
    "actor_name",
    "customer_id",
    "customer_snapshot",
    "company_snapshot",
    "date",
    "due_date",
    "memo",
    "tax_mode",
    "tax_item_id",
    "tax_snapshot",
    "tax_label",
    "tax_rate",
    "subtotal",
    "taxable",
    "tax",
    "total",
    "journal_id",
    "reversal_id",
];
const RECEIPT: &[&str] = &[
    "kind",
    "actor_name",
    "customer_id",
    "customer_snapshot",
    "company_snapshot",
    "date",
    "amount",
    "method",
    "reference",
    "memo",
    "destination",
    "destination_account_id",
    "journal_id",
];
impl Runner<'_> {
    fn sales_ref(&self, key: &str) -> String {
        self.l2_refs.get(key).cloned().unwrap_or_else(|| key.into())
    }
    fn sales_input(&self, value: &Value) -> Value {
        match value {
            Value::Array(a) => json!(a.iter().map(|v| self.sales_input(v)).collect::<Vec<_>>()),
            Value::Object(m) => {
                let mut out = Map::new();
                for (k, v) in m {
                    let v = if ["customer_id", "item_id", "tax_item_id", "invoice_id"]
                        .contains(&k.as_str())
                    {
                        v.as_str()
                            .map(|s| json!(self.sales_ref(s)))
                            .unwrap_or(v.clone())
                    } else if k.ends_with("_account_id") || k == "bank_id" {
                        v.as_str()
                            .and_then(|r| self.accounts.get(r))
                            .map(|id| json!(id.to_db()))
                            .unwrap_or(v.clone())
                    } else {
                        self.sales_input(v)
                    };
                    out.insert(k.clone(), v);
                }
                Value::Object(out)
            }
            _ => value.clone(),
        }
    }
    fn sales_track(&mut self, name: &str, v: &Value) -> fb_core::FbResult<()> {
        if let Some(id) = v["id"].as_str() {
            self.l2_refs.entry(name.into()).or_insert(id.into());
        }
        if let Some(id) = v["identity_id"].as_str() {
            self.l2_refs
                .entry(format!("{name}:identity"))
                .or_insert(id.into());
        }
        let h = v.get("current").unwrap_or(v);
        for (k, suffix) in [("journal_id", "posted"), ("reversal_id", "reversed")] {
            if let Some(id) = h[k].as_str() {
                self.journals
                    .entry(format!("{name}:{suffix}"))
                    .or_insert(id.parse().map_err(|_| sales_evidence_error())?);
            }
        }
        Ok(())
    }
    pub(super) fn apply_sales(&mut self, step: &Value, actor: UserId) -> Result<bool, StepError> {
        let op = s(step, "op");
        if !op.starts_with("sales_") {
            return Ok(false);
        }
        let c = find(&self.companies, s(step, "company"))?;
        let input = self.sales_input(step.get("values").unwrap_or(&json!({})));
        let key = step["record"].as_str().map(|s| self.sales_ref(s));
        let expected = step["expected_revision"].as_i64();
        match op {
            "sales_save_estimate" => {
                let mut input = input;
                input["expected_revision"] =
                    step.get("expected_revision").cloned().unwrap_or(json!(0));
                if let Some(key) = key {
                    input["estimate_id"] = json!(key);
                }
                let v = sales::save_estimate(&mut self.book, actor, c, &input)?;
                self.sales_track(s(step, "ref"), &v)?;
            }
            "sales_transition_estimate" => {
                let v = sales::transition_estimate(
                    &mut self.book,
                    actor,
                    c,
                    key.as_deref().ok_or_else(sales_evidence_error)?,
                    s(step, "state"),
                    expected,
                    step["reason"].as_str().unwrap_or(""),
                )?;
                self.sales_track(s(step, "record"), &v)?;
            }
            "sales_convert_estimate" => {
                let v = sales::convert_estimate(
                    &mut self.book,
                    actor,
                    c,
                    key.as_deref().ok_or_else(sales_evidence_error)?,
                    expected,
                    s(step, "date"),
                    s(step, "due_date"),
                )?;
                self.sales_track(s(step, "ref"), &v)?;
            }
            "sales_save_invoice" => {
                let mut input = input;
                input["expected_revision"] =
                    step.get("expected_revision").cloned().unwrap_or(json!(0));
                if let Some(key) = key {
                    input["invoice_id"] = json!(key)
                }
                let v = sales::save_invoice(&mut self.book, actor, c, &input)?;
                self.sales_track(s(step, "ref"), &v)?;
            }
            "sales_review_invoice" => {
                let v = sales::review_invoice(
                    &mut self.book,
                    actor,
                    c,
                    key.as_deref().ok_or_else(sales_evidence_error)?,
                    expected,
                )?;
                self.sales_reviews.insert(s(step, "review").into(), v);
            }
            "sales_post_invoice" => {
                let r = step["review"]
                    .as_str()
                    .and_then(|r| self.sales_reviews.get(r))
                    .cloned();
                let v = sales::post_invoice(
                    &mut self.book,
                    actor,
                    c,
                    key.as_deref().ok_or_else(sales_evidence_error)?,
                    expected,
                    r.as_ref(),
                )?;
                self.sales_track(s(step, "record"), &v)?;
            }
            "sales_void_invoice" => {
                let v = sales::void_invoice(
                    &mut self.book,
                    actor,
                    c,
                    key.as_deref().ok_or_else(sales_evidence_error)?,
                    expected,
                    &step["date"],
                    &step["reason"],
                )?;
                self.sales_track(s(step, "record"), &v)?;
            }
            "sales_review_receipt" | "sales_review_sale" => {
                let f = if op == "sales_review_sale" {
                    sales::review_sales_receipt
                } else {
                    sales::review_receipt
                };
                let v = f(&mut self.book, actor, c, &input)?;
                self.sales_reviews.insert(s(step, "review").into(), v);
            }
            "sales_record_receipt" | "sales_record_sale" => {
                let r = step["review"]
                    .as_str()
                    .and_then(|r| self.sales_reviews.get(r))
                    .cloned();
                let f = if op == "sales_record_sale" {
                    sales::record_sales_receipt
                } else {
                    sales::record_receipt
                };
                let v = f(&mut self.book, actor, c, &input, r.as_ref())?;
                self.sales_track(s(step, "ref"), &v)?;
            }
            "sales_reverse_receipt" | "sales_void_sale" => {
                let f = if op == "sales_void_sale" {
                    sales::void_sales_receipt
                } else {
                    sales::reverse_receipt
                };
                let v = f(
                    &mut self.book,
                    actor,
                    c,
                    key.as_deref().ok_or_else(sales_evidence_error)?,
                    expected,
                    &step["date"],
                    &step["reason"],
                )?;
                if let Some(id) = v["journal_id"].as_str() {
                    self.journals
                        .entry(format!("{}:reversed", s(step, "record")))
                        .or_insert(id.parse().map_err(|_| sales_evidence_error())?);
                }
            }
            _ => panic!("unknown sales op {op}"),
        }
        Ok(true)
    }
    fn sales_refs(&self) -> HashMap<String, String> {
        let mut out = HashMap::new();
        let mut pairs: Vec<(String, String)> = self
            .l2_refs
            .iter()
            .map(|(k, v)| (v.clone(), k.clone()))
            .chain(self.accounts.iter().map(|(k, v)| (v.to_db(), k.clone())))
            .chain(self.companies.iter().map(|(k, v)| (v.to_db(), k.clone())))
            .chain(self.journal_names())
            .collect();
        pairs.sort();
        for (id, name) in pairs {
            out.entry(id).or_insert(name);
        }
        out
    }
    fn sales_name(&self, key: &str) -> String {
        sales_name(&self.sales_refs(), key)
    }
    fn sales_norm(&self, v: &Value) -> Value {
        sales_norm(&self.sales_refs(), v)
    }
    pub(super) fn sales_normalize_l2(&mut self, mut value: Value) -> Value {
        let generated=self.book.read(|tx|->fb_core::FbResult<Vec<String>>{let mut st=tx.prepare("SELECT i.identity_id FROM sales_estimateconversion c JOIN sales_invoice i ON i.id=c.invoice_id")?;Ok(st.query_map([],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?)}).expect("conversion identities");
        let generated = generated
            .iter()
            .map(|id| self.sales_name(id))
            .collect::<Vec<_>>();
        for identity in value["identities"].as_array_mut().unwrap() {
            if generated.iter().any(|id| identity["id"] == *id) {
                identity["request_key"] = json!("generated-estimate-conversion");
            }
        }
        let mut generated_roles = std::collections::BTreeSet::new();
        let Some(rows) = value["snapshots"].as_array_mut() else {
            return json!({"invalid_sales_evidence":"Expected snapshot rows"});
        };
        for row in rows {
            let Some(source) = row["source"].as_str() else {
                return json!({"invalid_sales_evidence":"Expected snapshot source"});
            };
            let sid = self.l2_refs.get(source);
            let kind = sid.and_then(|id| {
                self.book
                    .read(|conn| {
                        conn.query_row(
                            "SELECT kind FROM documents_documentidentity WHERE id=?1",
                            [id],
                            |r| r.get::<_, String>(0),
                        )
                    })
                    .ok()
            });
            if matches!(kind.as_deref(), Some("SR" | "RCP")) {
                generated_roles.insert(source.to_string());
                let Some(role) = row["role"].as_str() else {
                    return json!({"invalid_sales_evidence":"Expected snapshot role"});
                };
                if let Some((prefix, key)) = role.split_once(':') {
                    row["role"] = json!(format!("{prefix}:{}", self.sales_name(key)));
                }
            }
        }
        // Only generated account-role suffixes change representation. Preserve
        // source order and every ordinary L2a snapshot ordering observation.
        let Some(rows) = value["snapshots"].as_array_mut() else {
            return json!({"invalid_sales_evidence":"Expected snapshot rows"});
        };
        for source in generated_roles {
            let indices: Vec<_> = rows
                .iter()
                .enumerate()
                .filter(|(_, r)| r["source"] == source)
                .map(|(i, _)| i)
                .collect();
            let mut group: Vec<_> = indices.iter().map(|i| rows[*i].clone()).collect();
            group.sort_by_key(|r| r["role"].to_string());
            for (i, row) in indices.into_iter().zip(group) {
                rows[i] = row;
            }
        }
        value
    }
    pub(super) fn dump_sales(&mut self) -> Value {
        self.try_dump_sales()
            .unwrap_or_else(|error| json!({"invalid_sales_dump":error.to_string()}))
    }
    fn try_dump_sales(&mut self) -> fb_core::FbResult<Value> {
        let mut balances = Vec::new();
        for cref in &self.company_order {
            balances.extend(sales::invoice_balances(
                &mut self.book,
                self.company_actor[cref],
                self.companies[cref],
            )?);
        }
        let balances: Vec<_> = balances.iter().map(|v| self.sales_norm(v)).collect();
        let refs = self.sales_refs();
        let company_order = self.company_order.clone();
        let companies = self.companies.clone();
        self.book.read(|conn| -> fb_core::FbResult<Value> {
        let mut invoices = Vec::new();
        let mut estimates=Vec::new();
        let mut receipts = Vec::new();
        for cref in &company_order {
            let c = companies[cref];
            let mut stmt=conn.prepare("SELECT id,revision FROM sales_estimate WHERE company_id=?1 ORDER BY id")?;
            for row in stmt.query_map([c.to_db()],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?)))?{
                let (key,revision)=row?;let mut history=vec![];let mut stmt=conn.prepare("SELECT id,data FROM sales_estimaterevision WHERE estimate_id=?1 ORDER BY number")?;
                for row in stmt.query_map([&key],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?{let (rid,raw)=row?;let data:Value=parse_sales_json(&raw)?;let mut h=json!({});for k in HEADER{if !["due_date","journal_id","reversal_id"].contains(k){h[*k]=data[*k].clone();}}h["expiry"]=data["expiry"].clone();
                    let mut st=conn.prepare("SELECT data FROM sales_estimateline WHERE revision_id=?1 ORDER BY position")?;let mut lines=vec![];for row in st.query_map([rid],|r|r.get::<_,String>(0))?{let mut line:Value=parse_sales_json(&row?)?;line.as_object_mut().unwrap().remove("origin_position");line.as_object_mut().unwrap().remove("origin_revision_id");lines.push(line);}h["lines"]=json!(lines);history.push(sales_norm(&refs,&h));
                }
                let link:Option<(i64,String)>=conn.query_row("SELECT r.number,x.invoice_id FROM sales_estimateconversion x JOIN sales_estimaterevision r ON r.id=x.revision_id WHERE x.estimate_id=?1",[&key],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
                estimates.push(json!({"ref":sales_name(&refs,&key),"revision":revision,"history":history,"conversion":link.map(|(revision,id)|json!({"revision":revision,"invoice":sales_name(&refs,&id)}))}));
            }
            let mut stmt = conn
                .prepare("SELECT id,revision FROM sales_invoice WHERE company_id=?1 ORDER BY id")
                ?;
            for row in stmt
                .query_map([c.to_db()], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
                })
                ?
            {
                let (key, revision) = row?;
                let mut history = Vec::new();
                let mut stmt=conn.prepare("SELECT id,data FROM sales_invoicerevision WHERE invoice_id=?1 ORDER BY number")?;
                for r in stmt
                    .query_map([&key], |r| {
                        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                    })
                    ?
                {
                    let (rid, data) = r?;
                    let data: Value = parse_sales_json(&data)?;
                    let mut h = json!({});
                    for k in HEADER {
                        h[*k] = data[*k].clone()
                    }
                    let mut ls=conn.prepare("SELECT data FROM sales_invoiceline WHERE revision_id=?1 ORDER BY position")?;
                    h["lines"] = json!(
                        ls.query_map([rid], |r| r.get::<_, String>(0))
                            ?
                            .map(|r| -> fb_core::FbResult<Value> {
                                let mut line: Value = parse_sales_json(&r?)?;
                                if let Some(m) = line.as_object_mut() {
                                    m.remove("origin_position");
                                    m.remove("origin_revision_id");
                                }
                                Ok(line)
                            })
                            .collect::<fb_core::FbResult<Vec<_>>>()?
                    );
                    history.push(sales_norm(&refs, &h));
                }
                invoices.push(
                    json!({"ref":sales_name(&refs, &key),"revision":revision,"history":history}),
                );
            }
            let mut stmt = conn
                .prepare("SELECT id,data FROM sales_receipt WHERE company_id=?1 ORDER BY id")
                ?;
            for row in stmt
                .query_map([c.to_db()], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })
                ?
            {
                let (key, data) = row?;
                let data: Value = parse_sales_json(&data)?;
                let mut h = json!({"ref":sales_name(&refs, &key),"sale":null,"lines":[],"allocations":[],"reversal":null});
                for k in RECEIPT {
                    h[*k] = data[*k].clone()
                }
                let mut st=conn.prepare("SELECT a.invoice_id,v.number,a.receivables_id,a.amount FROM sales_receiptallocation a JOIN sales_invoicerevision v ON v.id=a.invoice_revision_id WHERE a.receipt_id=?1 ORDER BY a.invoice_id")?;
                h["allocations"]=json!(st.query_map([&key],|r|Ok(json!({"invoice_id":r.get::<_,String>(0)?,"revision":r.get::<_,i64>(1)?,"receivables_id":r.get::<_,String>(2)?,"amount":fb_core::Money::from_db(r.get(3)?).to_string()})))?.collect::<Result<Vec<_>,_>>()?);
                let detail: Option<(String, String)> = conn
                    .query_row(
                        "SELECT id,data FROM sales_salesreceipt WHERE receipt_id=?1",
                        [&key],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()
                    ?;
                if let Some((id, s)) = detail {
                    h["sale"] = parse_sales_json(&s)?;
                    let mut st=conn.prepare("SELECT data FROM sales_salesreceiptline WHERE sale_id=?1 ORDER BY position")?;
                    h["lines"] = json!(
                        st.query_map([id], |r| r.get::<_, String>(0))
                            ?
                            .map(|r| -> fb_core::FbResult<Value> {
                                let mut line: Value = parse_sales_json(&r?)?;
                                if let Some(m) = line.as_object_mut() {
                                    m.remove("origin_position");
                                    m.remove("origin_revision_id");
                                }
                                Ok(line)
                            })
                            .collect::<fb_core::FbResult<Vec<_>>>()?
                    );
                }
                let rev: Option<String> = conn
                    .query_row(
                        "SELECT data FROM sales_receiptreversal WHERE receipt_id=?1",
                        [&key],
                        |r| r.get(0),
                    )
                    .optional()
                    ?;
                if let Some(s) = rev {
                    let mut v: Value = parse_sales_json(&s)?;
                    v.as_object_mut().ok_or_else(sales_evidence_error)?.remove("id");
                    v.as_object_mut().ok_or_else(sales_evidence_error)?.remove("receipt_id");
                    h["reversal"] = v;
                }
                receipts.push(sales_norm(&refs, &h));
            }

        }
        Ok(json!({"estimates":estimates,"invoices":invoices,"receipts":receipts,"balances":balances}))
        })
    }
}

fn sales_evidence_error() -> fb_core::FbError {
    fb_core::FbError::validation("Invalid stored sales dump evidence.")
}
fn parse_sales_json(raw: &str) -> fb_core::FbResult<Value> {
    fb_db::parse_unique_json_as(raw).map_err(|_| sales_evidence_error())
}
fn sales_name(refs: &HashMap<String, String>, key: &str) -> String {
    refs.get(key).cloned().unwrap_or_else(|| key.into())
}
fn sales_norm(refs: &HashMap<String, String>, v: &Value) -> Value {
    let mut out = v.clone();
    let Some(m) = out.as_object_mut() else {
        return out;
    };
    for (key, value) in m {
        if value.is_null() {
            continue;
        }
        match key.as_str() {
            "customer_id"
            | "item_id"
            | "tax_item_id"
            | "invoice_id"
            | "journal_id"
            | "reversal_id"
            | "income_account_id"
            | "expense_account_id"
            | "destination_account_id"
            | "receivables_id"
            | "collected_tax_id" => {
                if let Some(id) = value.as_str() {
                    *value = json!(sales_name(refs, id));
                }
            }
            "customer_snapshot" | "company_snapshot" | "item_snapshot" => {
                if let Some(id) = value.get("id").and_then(Value::as_str) {
                    value["id"] = json!(sales_name(refs, id));
                }
            }
            "account_snapshot" => {
                if let Some(accounts) = value.as_object_mut() {
                    for a in accounts.values_mut() {
                        if let Some(id) = a["id"].as_str() {
                            a["id"] = json!(sales_name(refs, id));
                        }
                    }
                }
            }
            "tax_snapshot" => {
                tax_norm(refs, value);
            }
            "sale" | "reversal" => {
                *value = sales_norm(refs, value);
            }
            "lines" | "allocations" => {
                if let Some(rows) = value.as_array_mut() {
                    for row in rows {
                        *row = sales_norm(refs, row);
                    }
                }
            }
            _ => {}
        }
    }
    out
}
fn tax_norm(refs: &HashMap<String, String>, v: &mut Value) {
    if let Some(m) = v.as_object_mut() {
        for key in ["id", "agency_id"] {
            if let Some(value) = m.get_mut(key).filter(|v| !v.is_null())
                && let Some(id) = value.as_str()
            {
                *value = json!(sales_name(refs, id));
            }
        }
        if let Some(rows) = m.get_mut("members").and_then(Value::as_array_mut) {
            for row in rows {
                tax_norm(refs, row);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn comparer_rejects_mutated_sales_history_balances_and_allocations() {
        let scenario: Value =
            serde_json::from_str(include_str!("../scenarios/sales-invoice-payments.json")).unwrap();
        let path =
            std::env::temp_dir().join(format!("sales-comparer-{}.fbbook", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut runner = Runner::new(&scenario, &path);
        runner.run();
        let base = runner.dump();
        assert!(compare(&base, &base).ok());
        for (path, value) in [
            ("/sales/invoices/0/history/0/state", json!("CORRUPTED")),
            ("/sales/invoices/0/history/0/total", json!("10.01")),
            ("/sales/invoices/0/history/0/lines/0/amount", json!("10.01")),
            (
                "/sales/invoices/0/history/0/customer_snapshot/display_name",
                json!("Changed"),
            ),
            ("/sales/receipts/0/allocations/0/invoice_id", json!("other")),
            ("/sales/receipts/0/reversal/date", json!("2026-10-04")),
            ("/sales/balances/0/outstanding", json!("6.99")),
        ] {
            let mut mutated = base.clone();
            *mutated.pointer_mut(path).unwrap() = value;
            let report = compare(&base, &mutated);
            assert!(!report.ok(), "missed {path}");
            assert!(!report.failures.is_empty());
        }
    }
}
