//! Offline adapter: Python invokes the frozen parser/settings/state helpers without a provider.
use super::*;
use fb_core::assistant;
use fb_db::rusqlite::params;

impl Runner<'_> {
    pub(super) fn apply_assistant(
        &mut self,
        step: &Value,
        actor: UserId,
    ) -> Result<bool, StepError> {
        let op = s(step, "op");
        if !op.starts_with("assistant_") {
            return Ok(false);
        }
        let co = find(&self.companies, s(step, "company"))?;
        let refid = |name: &str| {
            self.l2_refs
                .get(name)
                .cloned()
                .unwrap_or_else(|| name.into())
        };
        let now = step["now"].as_i64().unwrap_or(1_792_944_000);
        match op {
            "assistant_connect" => {
                assistant::connected(&mut self.book, actor, co, &step["state"])?;
            }
            "assistant_switch" => {
                assistant::set_read_attachments(
                    &mut self.book,
                    actor,
                    co,
                    step["enabled"].as_bool().unwrap(),
                )?;
            }
            "assistant_model" => {
                assistant::set_model(&mut self.book, actor, co, s(step, "model"))?;
            }
            "assistant_models" => {
                assistant::refresh_models(&mut self.book, actor, co, &step["choices"])?;
            }
            "assistant_dismiss" => {
                assistant::dismiss_notice(&mut self.book, actor, co)?;
            }
            "assistant_disconnect" => {
                assistant::disconnect(&mut self.book, actor, co, step["revoked"].as_bool())?;
            }
            "assistant_needs_signin" => {
                assistant::needs_signin(&mut self.book, actor, co,
                    assistant::SigninFailure::new(step["action"].as_str().unwrap_or("ai.call_failed"), step["code"].as_str().unwrap_or("unauthorized"))?)?;
            }
            "assistant_metadata" => {
                let row = assistant::connection_for(&mut self.book, actor, co)?
                    .ok_or_else(|| FbError::validation(assistant::NOT_CONNECTED))?;
                assistant::provider_metadata(
                    &mut self.book,
                    actor,
                    co,
                    row["generation"].as_i64().unwrap(),
                    step["expires"].as_i64(),
                    if step["used"] == true {
                        Some(now)
                    } else {
                        None
                    },
                )?;
            }
            "assistant_parse" => {
                let currency = self.book.read(|c| store::get_company(c, co))?.currency;
                let raw = step["raw"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| step["raw"].to_string());
                let result = assistant::receipts::parse_reply(
                    &raw,
                    &currency,
                    step["today"].as_str().unwrap_or("2026-10-30"),
                ).map_err(FbError::from)?;
                self.assistant_results.insert(s(step, "ref").into(), result);
            }
            "assistant_begin" => {
                let mut file = step["file"].clone();
                if let Some(b) = file["bill_id"].as_str() {
                    file["bill_id"] = json!(refid(b));
                }
                let row = assistant::begin_read(
                    &mut self.book,
                    actor,
                    co,
                    s(step, "request_key"),
                    &file,
                    now,
                )?;
                self.l2_refs
                    .entry(s(step, "ref").into())
                    .or_insert(row["id"].as_str().unwrap().into());
            }
            "assistant_finish" => {
                let id = refid(s(step, "reading"));
                let row = assistant::get_reading(&mut self.book, actor, co, &id)?;
                assistant::finish_read(
                    &mut self.book,
                    actor,
                    co,
                    &id,
                    row["attempt"].as_i64().unwrap(),
                    &self.assistant_results[s(step, "result")],
                    now,
                )?;
            }
            "assistant_reply" => {
                let id = refid(s(step, "reading"));
                let row = assistant::get_reading(&mut self.book, actor, co, &id)?;
                let raw = step["raw"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| step["raw"].to_string());
                assistant::complete_reply(
                    &mut self.book,
                    actor,
                    co,
                    &id,
                    row["attempt"].as_i64().unwrap(),
                    assistant::Reply {
                        text: &raw,
                        today: step["today"].as_str().unwrap_or("2026-10-30"),
                    },
                    now,
                )?;
            }
            "assistant_fail" => {
                let id = refid(s(step, "reading"));
                let row = assistant::get_reading(&mut self.book, actor, co, &id)?;
                assistant::fail_read(
                    &mut self.book,
                    actor,
                    co,
                    &id,
                    row["attempt"].as_i64().unwrap(),
                    s(step, "code"),
                    now,
                )?;
            }
            "assistant_initial" => {
                let id = refid(s(step, "reading"));
                let mut result = assistant::form_initial(&mut self.book, actor, co, &id)?;
                let names = self.assistant_names();
                normalize_initial(&mut result, &names);
                self.assistant_results.insert(s(step, "ref").into(), result);
            }
            "assistant_queue" => {
                let rows = assistant::review_queue(&mut self.book, actor, co)?;
                let names = self.assistant_names();
                self.assistant_results.insert(s(step,"ref").into(),json!({"queue":rows.iter().map(|v|names.get(v["id"].as_str().unwrap()).cloned().unwrap_or("?".into())).collect::<Vec<_>>()}));
            }
            "assistant_save" => {
                let id = refid(s(step, "reading"));
                let payload = self.pinput(&step["values"]);
                let bill = assistant::save_review(&mut self.book, actor, co, &id, &payload)?;
                self.ptrack(s(step, "ref"), &bill, true);
            }
            _ => panic!("unknown assistant op {op}"),
        }
        Ok(true)
    }
    fn assistant_names(&self) -> HashMap<String, String> {
        let mut names = HashMap::new();
        let mut refs = self.l2_refs.iter().collect::<Vec<_>>();
        refs.sort();
        for (name, id) in refs {
            names.entry(id.clone()).or_insert(name.clone());
        }
        for (name, id) in &self.users {
            names.insert(id.to_db(), name.clone());
        }
        for (name, id) in &self.companies {
            names.insert(id.to_db(), name.clone());
        }
        for (name, id) in &self.accounts {
            names.insert(id.to_db(), name.clone());
        }
        names
    }
    pub(super) fn extend_assistant_audit(&mut self, ids: &mut audit::Ids) {
        let names = self.assistant_names();
        let readings = self
            .book
            .read(|c| -> fb_core::FbResult<Vec<String>> {
                Ok(c.prepare("SELECT id FROM assistant_reading ORDER BY id")?
                    .query_map([], |r| r.get(0))?
                    .collect::<Result<_, _>>()?)
            })
            .unwrap();
        for (i, id) in readings.iter().enumerate() {
            ids.entry("reading".into()).or_default().insert(
                id.clone(),
                names
                    .get(id)
                    .cloned()
                    .unwrap_or_else(|| format!("reading:{}", i + 1)),
            );
        }
    }
    pub(super) fn dump_assistant(&mut self) -> Value {
        let names = self.assistant_names();
        let mut connections = vec![];
        let mut readings = vec![];
        let mut companies = self
            .companies
            .iter()
            .map(|(r, id)| (r.clone(), *id))
            .collect::<Vec<_>>();
        companies.sort_by_key(|(_, id)| *id);
        for (name, co) in companies {
            let actor = self.company_actor[&name];
            if let Some(mut row) = assistant::connection_for(&mut self.book, actor, co).unwrap() {
                let m = row.as_object_mut().unwrap();
                m.remove("id");
                m.remove("generation");
                m.insert("company".into(), json!(name));
                connections.push(row);
            }
            let ids = self
                .book
                .read(|c| -> fb_core::FbResult<Vec<String>> {
                    Ok(c.prepare(
                        "SELECT id FROM assistant_reading WHERE company_id=? ORDER BY id",
                    )?
                    .query_map(params![co.to_db()], |r| r.get(0))?
                    .collect::<Result<_, _>>()?)
                })
                .unwrap();
            for id in ids {
                let mut row = assistant::get_reading(&mut self.book, actor, co, &id).unwrap();
                let m = row.as_object_mut().unwrap();
                for k in [
                    "source_file_id",
                    "origin_bill_id",
                    "attempt",
                    "connection_generation",
                ] {
                    m.remove(k);
                }
                for k in ["id", "bill_id", "created_by_id"] {
                    if let Some(raw) = m[k].as_str() {
                        let name = names.get(raw).cloned().unwrap_or_else(|| "?".into());
                        m.insert(k.into(), json!(name));
                    }
                }
                m.insert("company".into(), json!(name));
                readings.push(row);
            }
        }
        json!({"connections":connections,"readings":readings,"results":self.assistant_results})
    }
}
fn normalize_initial(v: &mut Value, names: &HashMap<String, String>) {
    fn id(v: &mut Value, names: &HashMap<String, String>) {
        if let Some(s) = v.as_str() {
            *v = json!(names.get(s).cloned().unwrap_or("?".into()));
        }
    }
    id(&mut v["initial"]["vendor"], names);
    id(&mut v["vendor"]["matched"], names);
    if let Some(a) = v["vendor"]["candidates"].as_array_mut() {
        for v in a {
            id(v, names);
        }
    }
    if let Some(a) = v["lines"].as_array_mut() {
        for v in a {
            id(&mut v["expense_account"], names);
        }
    }
}
pub(super) fn normalize_audit(v: &mut Value, ids: &audit::Ids) -> bool {
    let action = v["action"].as_str().unwrap_or("");
    if !action.starts_with("ai.") && action != "bill.ai_lines_kept" {
        return false;
    }
    if let Some(reading) = v["detail"].get_mut("reading") {
        let raw = reading
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| reading.to_string());
        *reading = json!(
            ids.get("reading")
                .and_then(|m| m.get(&raw))
                .cloned()
                .unwrap_or("?".into())
        );
    }
    if let Some(target) = v["target"]
        .as_str()
        .and_then(|s| s.strip_prefix("bill:"))
        .filter(|s| *s != "new")
    {
        let mapped = ids
            .get("bill")
            .and_then(|m| m.get(target))
            .cloned()
            .unwrap_or("?".into());
        v["target"] = json!(format!("bill:{mapped}"));
    }
    true
}
