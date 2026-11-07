//! Attachment and reminders differential adapter. Generated ciphertext and timestamps are omitted only.
use super::*;
use fb_core::{attachments as a, reminders};
fn unhex(s: &str) -> Vec<u8> {
    s.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect()
}
impl Runner<'_> {
    // The driver records these after EVERY step, independently of the attachment
    // adapter's generated query/result evidence (also when capture_steps=false).
    pub(super) fn record_step_evidence(&mut self, step: &Value, outcome: &mut Map<String, Value>) {
        let mut counts = Map::new();
        for (name, co) in &self.companies {
            let count: i64 = self
                .book
                .read(|c| {
                    c.query_row(
                        "SELECT count(*) FROM ledger_auditevent WHERE company_id=?",
                        [co.to_db()],
                        |r| r.get(0),
                    )
                })
                .unwrap();
            counts.insert(name.clone(), json!(count));
        }
        outcome.insert("audit_counts".into(), Value::Object(counts));
        if outcome["ok"] == true
            && ["reminder_build", "reminder_count", "attachment_download"]
                .contains(&step["op"].as_str().unwrap_or(""))
        {
            let attachment = step["attachment"].as_str().map(|r| self.l2_refs[r].clone());
            outcome.insert(
                "query".into(),
                json!({
                    "ref":step["ref"], "company":step["company"],
                    "today":step.get("today"), "days":step.get("days"), "attachment":attachment
                }),
            );
        }
    }
    pub(super) fn dump_step_evidence(&self) -> Value {
        let mut outcomes = json!(self.outcomes);
        for outcome in outcomes.as_array_mut().unwrap() {
            if let Some(id) = outcome["query"]["attachment"].as_str() {
                outcome["query"]["attachment"] = json!(self.aref(id));
            }
        }
        outcomes
    }

    fn aref(&self, id: &str) -> String {
        self.l2_refs
            .iter()
            .filter(|(_, v)| v.as_str() == id)
            .map(|(k, _)| k.clone())
            .min()
            .or_else(|| {
                self.journal_names().get(id).cloned()
            })
            .unwrap_or_else(|| format!("<unknown:{id}>"))
    }
    fn attachment_normalize(&self, v: &Value) -> Value {
        match v {
            Value::Array(xs) => json!(
                xs.iter()
                    .map(|x| self.attachment_normalize(x))
                    .collect::<Vec<_>>()
            ),
            Value::Object(m) => Value::Object(
                m.iter()
                    .map(|(k, v)| {
                        (
                            k.clone(),
                            if k == "document_id" {
                                json!(self.aref(v.as_str().unwrap()))
                            } else {
                                self.attachment_normalize(v)
                            },
                        )
                    })
                    .collect(),
            ),
            _ => v.clone(),
        }
    }
    fn record_attachment_query(&mut self, s: &Value, co: CompanyId) {
        let count: i64 = self
            .book
            .read(|c| {
                c.query_row(
                    "SELECT count(*) FROM ledger_auditevent WHERE company_id=?",
                    [co.to_db()],
                    |r| r.get(0),
                )
            })
            .unwrap();
        let attachment = s["attachment"].as_str().map(|r| self.l2_refs[r].clone());
        self.attachment_queries.push(
            json!({"step":self.outcomes.len(),"op":s["op"],"ref":s["ref"],
            "company":s["company"],"today":s.get("today"),"days":s.get("days"),
            "attachment":attachment,"audit_count":count}),
        );
    }
    pub(super) fn apply_attachments(
        &mut self,
        s: &Value,
        actor: UserId,
    ) -> Result<bool, StepError> {
        let op = super::s(s, "op");
        if !op.starts_with("attachment_") && !op.starts_with("reminder_") {
            return Ok(false);
        }
        let co = find(&self.companies, super::s(s, "company"))?;
        if op == "reminder_build" {
            let v = reminders::build(
                &mut self.book,
                actor,
                co,
                super::s(s, "today"),
                s["days"].as_i64(),
            )?;
            self.attachment_results.insert(super::s(s, "ref").into(), v);
            self.record_attachment_query(s, co);
            return Ok(true);
        }
        if op == "reminder_count" {
            let v = reminders::overdue_count(&mut self.book, actor, co, super::s(s, "today"))?;
            self.attachment_results
                .insert(super::s(s, "ref").into(), json!(v));
            self.record_attachment_query(s, co);
            return Ok(true);
        }
        if op == "attachment_remove" {
            let id = self
                .l2_refs
                .get(super::s(s, "attachment"))
                .cloned()
                .ok_or_else(|| StepError::MissingRef("unknown attachment".into()))?;
            a::remove(&mut self.book, actor, co, &id, super::s(s, "reason"))?;
            return Ok(true);
        }
        if op == "attachment_download" {
            let id = self.l2_refs[super::s(s, "attachment")].clone();
            let bytes = a::download(&mut self.book, &self.attachment_store, actor, co, &id)?;
            use sha2::{Digest, Sha256};
            self.attachment_results.insert(
                super::s(s, "ref").into(),
                json!(format!("{:x}", Sha256::digest(&bytes))),
            );
            self.record_attachment_query(s, co);
            return Ok(true);
        }
        let owner = if let Some(r) = s["journal"].as_str() {
            a::Owner::Journal(find(&self.journals, r)?)
        } else {
            a::Owner::Document(self.l2_refs[super::s(s, "document")].clone())
        };
        let data = unhex(super::s(s, "hex"));
        let name = super::s(s, "name");
        let key = super::s(s, "request_key");
        let limits = a::Limits {
            max_mb: s["max_mb"].as_u64().unwrap_or(10) as u32,
            uploads_per_window: s["uploads_per_window"].as_u64().unwrap_or(60) as u32,
            company_max_mb: s["company_max_mb"].as_u64().unwrap_or(2048) as u32,
        };
        let (row, _) = if op == "attachment_adopt" {
            let inspected = a::inspect(&data, name, &limits)?;
            let file = self
                .attachment_store
                .write_atomic(co, &data, &inspected.content_type)?;
            self.book.write(|tx| {
                a::adopt_tx(
                    tx,
                    &self.attachment_store,
                    actor,
                    co,
                    &owner,
                    &file,
                    &inspected.name,
                    key,
                    &limits,
                )
            })?
        } else {
            a::add(
                &mut self.book,
                &self.attachment_store,
                actor,
                co,
                &owner,
                &data,
                name,
                key,
                &limits,
            )?
        };
        self.l2_refs
            .entry(super::s(s, "ref").into())
            .or_insert(row["id"].as_str().unwrap().into());
        Ok(true)
    }
    pub(super) fn dump_attachments(&mut self) -> Value {
        let mut rows = vec![];
        for (co, name) in self
            .companies
            .iter()
            .map(|(r, c)| (*c, r.clone()))
            .collect::<Vec<_>>()
        {
            for row in self.book.read(|tx| a::all(tx, co)).unwrap() {
                let id = row["id"].as_str().unwrap();
                let o = a::owner_of(&row).unwrap();
                let owner = o.key();
                let mut r = json!({"ref":self.aref(id),"company":name,"owner_kind":owner.0,"owner":self.aref(&owner.1)});
                for k in [
                    "original_name",
                    "content_type",
                    "size",
                    "sha256",
                    "deleted",
                    "uploaded_by_name",
                    "deleted_by_name",
                    "delete_reason",
                    "request_key",
                ] {
                    r[k] = row[k].clone();
                }
                rows.push(r);
            }
        }
        rows.sort_by_key(|r| r["ref"].as_str().unwrap().to_string());
        let queries = self
            .attachment_queries
            .iter()
            .map(|q| {
                let mut q = q.clone();
                if let Some(id) = q["attachment"].as_str() {
                    q["attachment"] = json!(self.aref(id));
                }
                q
            })
            .collect::<Vec<_>>();
        // Keep captured IDs raw until this dump's complete alias mapping is available.
        let results = self.attachment_normalize(&Value::Object(self.attachment_results.clone()));
        json!({"rows":rows,"results":results,"queries":queries})
    }
    pub(super) fn normalize_attachment_audit(&self, event: &mut Value, _ids: &audit::Ids) -> bool {
        if event["action"]
            .as_str()
            .is_some_and(|s| s.starts_with("attachment."))
            && let Some(id) = event["detail"]["attachment"].as_str()
        {
            event["detail"]["attachment"] = json!(self.aref(id));
        }
        false
    }
}
