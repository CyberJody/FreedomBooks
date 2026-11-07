//! Prepayment workflow adapter and exact state/audit dump.
use super::*;
use fb_core::prepayments as pp;
use fb_db::rusqlite::OptionalExtension;
pub(super) const APP: &[&str] = &[
    "deposit_id",
    "invoice_id",
    "receivables_id",
    "liability_id",
    "date",
    "amount",
    "actor_name",
    "journal_id",
];
pub(super) const REFUND: &[&str] = &[
    "deposit_id",
    "customer_id",
    "customer_snapshot",
    "company_snapshot",
    "date",
    "amount",
    "bank_id",
    "liability_id",
    "method",
    "check_number",
    "memo",
    "journal_id",
    "actor_name",
];
impl Runner<'_> {
    fn pp_input(&self, p: &Value) -> Value {
        match p {
            Value::Array(a) => json!(a.iter().map(|v| self.pp_input(v)).collect::<Vec<_>>()),
            Value::Object(m) => Value::Object(
                m.iter()
                    .map(|(k, v)| {
                        let value =
                            if ["customer_id", "invoice_id", "estimate_id"].contains(&k.as_str()) {
                                v.as_str()
                                    .and_then(|s| self.l2_refs.get(s))
                                    .map(|s| json!(s))
                                    .unwrap_or(v.clone())
                            } else if k == "bank_id" {
                                v.as_str()
                                    .and_then(|s| self.accounts.get(s))
                                    .map(|s| json!(s.to_db()))
                                    .unwrap_or(v.clone())
                            } else {
                                self.pp_input(v)
                            };
                        (k.clone(), value)
                    })
                    .collect(),
            ),
            _ => p.clone(),
        }
    }
    fn pp_track(&mut self, name: &str, v: &Value) -> Result<(), fb_core::FbError> {
        self.l2_refs.entry(name.into()).or_insert(s(v, "id").into());
        if let Some(id) = v["identity_id"].as_str() {
            self.l2_refs
                .entry(format!("{name}:identity"))
                .or_insert(id.into());
        }
        if let Some(id) = v["journal_id"].as_str() {
            self.journals
                .entry(format!("{name}:posted"))
                .or_insert(id.parse().map_err(|_| {
                    fb_core::FbError::validation("Invalid prepayment journal identity.")
                })?);
        }
        Ok(())
    }
    pub(super) fn apply_prepayments(
        &mut self,
        step: &Value,
        actor: UserId,
    ) -> Result<bool, StepError> {
        let op = s(step, "op");
        if !op.starts_with("pp_") {
            return Ok(false);
        }
        let c = find(&self.companies, s(step, "company"))?;
        let input = self.pp_input(step.get("values").unwrap_or(&json!({})));
        let record = step["record"]
            .as_str()
            .and_then(|s| self.l2_refs.get(s))
            .cloned()
            .unwrap_or_default();
        let deposit = step["deposit"]
            .as_str()
            .and_then(|s| self.l2_refs.get(s))
            .cloned()
            .unwrap_or_default();
        let review = step["review"]
            .as_str()
            .and_then(|s| self.pp_reviews.get(s))
            .cloned();
        match op {
            "pp_review_deposit" => {
                let v = pp::review_deposit(&mut self.book, actor, c, &input)?;
                self.pp_reviews.insert(s(step, "review").into(), v);
            }
            "pp_record_deposit" => {
                let v = pp::record_deposit(&mut self.book, actor, c, review.as_ref(), &input)?;
                self.pp_track(s(step, "ref"), &v)?;
            }
            "pp_review_application" => {
                let v = pp::review_application(&mut self.book, actor, c, &deposit, &input)?;
                self.pp_reviews.insert(s(step, "review").into(), v);
            }
            "pp_apply" => {
                let v =
                    pp::apply_deposit(&mut self.book, actor, c, &deposit, review.as_ref(), &input)?;
                self.pp_track(s(step, "ref"), &v)?;
            }
            "pp_review_refund" => {
                let v = pp::review_refund(&mut self.book, actor, c, &deposit, &input)?;
                self.pp_reviews.insert(s(step, "review").into(), v);
            }
            "pp_record_refund" => {
                let v =
                    pp::record_refund(&mut self.book, actor, c, &deposit, review.as_ref(), &input)?;
                self.pp_track(s(step, "ref"), &v)?;
            }
            "pp_void_deposit" | "pp_unapply" | "pp_void_refund" => {
                let f = match op {
                    "pp_void_deposit" => pp::void_deposit,
                    "pp_unapply" => pp::unapply_deposit,
                    _ => pp::void_refund,
                };
                let v = f(
                    &mut self.book,
                    actor,
                    c,
                    &record,
                    step["expected_revision"].as_i64(),
                    &step["date"],
                    &step["reason"],
                )?;
                self.journals
                    .entry(format!("{}:reversed", s(step, "record")))
                    .or_insert(s(&v, "journal_id").parse().map_err(|_| {
                        fb_core::FbError::validation(
                            "Invalid prepayment reversal journal identity.",
                        )
                    })?);
            }
            _ => panic!("unknown prepayment op {op}"),
        }
        Ok(true)
    }
    fn pp_refs(&self) -> HashMap<String, String> {
        let mut pairs = self
            .l2_refs
            .iter()
            .map(|(n, k)| (k.clone(), n.clone()))
            .chain(self.accounts.iter().map(|(n, k)| (k.to_db(), n.clone())))
            .chain(self.companies.iter().map(|(n, k)| (k.to_db(), n.clone())))
            .chain(self.journal_names())
            .collect::<Vec<_>>();
        pairs.sort();
        let mut out = HashMap::new();
        for (k, n) in pairs {
            out.entry(k).or_insert(n);
        }
        out
    }
    pub(super) fn extend_pp_audit_ids(&mut self, ids: &mut audit::Ids) {
        let refs = self.pp_refs();
        self.book
            .read::<_, fb_core::FbError>(|q| {
                for (kind, table) in [
                    ("depositapplication", "prepayments_depositapplication"),
                    ("depositrefund", "prepayments_depositrefund"),
                ] {
                    let mut st = q.prepare(&format!("SELECT id FROM {table} ORDER BY id"))?;
                    let map = st
                        .query_map([], |r| r.get::<_, String>(0))?
                        .map(|r| {
                            let id = r?;
                            Ok((id.clone(), refs.get(&id).cloned().unwrap_or_default()))
                        })
                        .collect::<Result<_, fb_db::rusqlite::Error>>()?;
                    ids.insert(kind.into(), map);
                }
                Ok(())
            })
            .unwrap();
    }
    pub(super) fn dump_prepayments(&mut self) -> Value {
        self.try_dump_prepayments()
            .unwrap_or_else(|e| json!({"invalid_prepayment_evidence": e.to_string()}))
    }
    fn try_dump_prepayments(&mut self) -> Result<Value, fb_core::FbError> {
        let refs = self.pp_refs();
        // The frozen exporter groups applications/refunds by driver company order,
        // then creation PK. Global creation order diverges when companies interleave.
        let company_order=self.company_order.iter().map(|r|self.companies[r].to_db()).collect::<Vec<_>>();
        let mut details = vec![];
        for cref in &self.company_order {
            for r in pp::deposit_balances(
                &mut self.book,
                self.company_actor[cref],
                self.companies[cref],
            )? {
                details.push(norm(&refs,&json!({"ref":refs[s(&r,"id")],"liability_id":r["prepayment"]["liability_id"],"estimate_id":r["prepayment"]["estimate_id"],"order_reference":r["prepayment"]["order_reference"],"applied":r["applied"],"refunded":r["refunded"],"remaining":r["remaining"],"deposit_state":r["deposit_state"]})));
            }
        }
        self.book.read::<_,fb_core::FbError>(|q|{
            let mut out=json!({"details":details,"applications":[],"refunds":[]});
            for(key,table,fields,undo,fk)in[("applications","prepayments_depositapplication",APP,"prepayments_depositunapplication","application_id"),("refunds","prepayments_depositrefund",REFUND,"prepayments_depositrefundvoid","refund_id")]{
                let mut rows=vec![];let mut st=q.prepare(&format!("SELECT id,data FROM {table} ORDER BY id"))?;
                for row in st.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?{let(id,raw)=row?;let v=stored_snapshot(&raw,if key=="applications" {"application"} else {"refund"})?;let mut r=json!({"ref":refs.get(&id).ok_or_else(|| fb_core::FbError::validation("Missing prepayment reference."))?});for k in fields{r[*k]=v[*k].clone();}
                    if key=="applications"{let n:i64=q.query_row("SELECT number FROM sales_invoicerevision WHERE id=?",[s(&v,"invoice_revision_id")],|r|r.get(0))?;r["invoice_revision"]=json!(n);}
                    let undo:Option<String>=q.query_row(&format!("SELECT data FROM {undo} WHERE {fk}=?"),[&id],|r|r.get(0)).optional()?;
                    r["reversal"]=undo.map(|raw| -> Result<Value, fb_core::FbError> {let v=stored_snapshot(&raw,if key=="applications" {"unapplication"} else {"refundvoid"})?;Ok(json!({"date":v["date"],"reason":v["reason"],"actor_name":v["actor_name"],"journal_id":v["journal_id"]}))}).transpose()?.unwrap_or(Value::Null);
                    let rank=company_order.iter().position(|c|c==s(&v,"company_id")).ok_or_else(||fb_core::FbError::validation("Missing prepayment company provenance."))?;
                    rows.push((rank,id,norm(&refs,&r)));
                }
                rows.sort_by(|a,b|a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
                out[key]=json!(rows.into_iter().map(|(_,_,row)|row).collect::<Vec<_>>());
            }Ok(out)
        })
    }
}
fn stored_snapshot(raw: &str, kind: &str) -> Result<Value, fb_core::FbError> {
    let value: Value = fb_db::parse_unique_json_as(raw)
        .map_err(|_| fb_core::FbError::validation("Invalid prepayment JSON."))?;
    if !fb_db::prepayments::prepayment_snapshot_ok(kind, &value) {
        return Err(fb_core::FbError::validation(
            "Invalid stored prepayment evidence.",
        ));
    }
    Ok(value)
}
fn norm(refs: &HashMap<String, String>, v: &Value) -> Value {
    match v {
        Value::Array(a) => json!(a.iter().map(|v| norm(refs, v)).collect::<Vec<_>>()),
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, v)| {
                    let v = if (k == "id" || k.ends_with("_id")) && !v.is_null() {
                        json!(
                            refs.get(v.as_str().unwrap_or(""))
                                .cloned()
                                .unwrap_or_else(|| "?".into())
                        )
                    } else {
                        norm(refs, v)
                    };
                    (k.clone(), v)
                })
                .collect(),
        ),
        _ => v.clone(),
    }
}
pub(crate) fn normalize_audit(v: &mut Value, ids: &audit::Ids) -> bool {
    let action = v["action"].as_str().unwrap_or("");
    if !action.starts_with("customer_deposit.") && !action.starts_with("deposit_refund.") {
        return false;
    }
    let Some((kind, raw)) = v["target"].as_str().and_then(|s| s.split_once(':')) else {
        // Preserve invalid evidence so strict comparison refuses it.
        return false;
    };
    v["target"] = json!(format!(
        "{kind}:{}",
        ids.get(kind)
            .and_then(|m| m.get(raw))
            .cloned()
            .unwrap_or_else(|| format!("<unknown:{kind}:{raw}>"))
    ));
    for (k, kind) in [
        ("destination_account_id", "account"),
        ("liability_id", "account"),
        ("bank_id", "account"),
        ("application_id", "depositapplication"),
        ("invoice_id", "invoice"),
        ("journal_id", "journal"),
    ] {
        if let Some(x) = v["detail"].get_mut(k) {
            let raw = x.as_str().unwrap_or("");
            *x = json!(
                ids.get(kind)
                    .and_then(|m| m.get(raw))
                    .cloned()
                    .unwrap_or_else(|| format!("<unknown:{kind}:{raw}>"))
            );
        }
    }
    true
}
