use super::*;
use fb_core::{
    calculations,
    directory::{self, Model},
    documents, salestax,
};
use fb_db::rusqlite::params;
fn model(s: &str) -> Model {
    match s {
        "customer" => Model::Customer,
        "vendor" => Model::Vendor,
        "item" => Model::Item,
        _ => panic!("model"),
    }
}
impl Runner<'_> {
    pub(super) fn l2_audit_ids(&mut self) -> audit::Ids {
        let refs = &self.l2_refs;
        let companies = &self.companies;
        let accounts = &self.accounts;
        self.book.read(|conn| -> fb_core::FbResult<_> {
            let mut ids = audit::Ids::new();
            for (r,id) in companies { ids.entry("company".into()).or_default().insert(id.to_string(), r.clone()); }
            for (r,id) in accounts { ids.entry("account".into()).or_default().insert(id.to_string(), r.clone()); }
            for (kind,table) in [("customer","directory_customer"),("vendor","directory_vendor"),("item","directory_item"),("taxitem","salestax_taxitem"),("documentidentity","documents_documentidentity"),("accountingsetup","documents_accountingsetup"),("salestaxsettings","salestax_salestaxsettings")] {
                let mut stmt=conn.prepare(&format!("SELECT id,company_id FROM {table} ORDER BY id"))?;
                let rows=stmt.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<Result<Vec<_>,_>>()?;
                for (rank,(id,company)) in rows.into_iter().enumerate() {
                    let cref=ids["company"].get(&company).cloned().unwrap_or_else(||format!("<unknown:{company}>"));
                    let mapped = if ["accountingsetup","salestaxsettings"].contains(&kind) { cref }
                        else { refs.iter().filter(|(_,v)| **v==id).map(|(k,_)|k.clone()).min().unwrap_or_else(||format!("unreferenced-{kind}:{rank}")) };
                    ids.entry(kind.into()).or_default().insert(id,mapped);
                }
            }
            Ok(ids)
        }).unwrap()
    }
    fn lref(&self, r: &str) -> String {
        self.l2_refs.get(r).cloned().unwrap_or_else(|| r.into())
    }
    fn resolve_input(&self, v: &Value) -> Value {
        let mut out = v.clone();
        if let Some(m) = out.as_object_mut() {
            for (k, v) in m {
                if k.ends_with("_account") || documents::ROLES.contains(&k.as_str()) {
                    if let Some(r) = v.as_str()
                        && let Some(id) = self.accounts.get(r)
                    {
                        *v = json!(id.to_db());
                    }
                } else if ["agency_id", "item_id", "tax_item_id", "default_item_id"].contains(&k.as_str()) {
                    if let Some(r) = v.as_str() {
                        *v = json!(self.lref(r));
                    }
                } else if k == "member_ids" {
                    if let Some(a) = v.as_array_mut() {
                        for x in a {
                            if let Some(r) = x.as_str() {
                                *x = json!(self.lref(r));
                            }
                        }
                    }
                } else if k == "banks"
                    && let Some(a) = v.as_array_mut()
                {
                    for x in a {
                        if let Some(id) = x.as_str().and_then(|r| self.accounts.get(r)) {
                            *x = json!(id.to_db());
                        }
                    }
                }
            }
        }
        out
    }
    pub(super) fn apply_l2a(&mut self, step: &Value, actor: UserId) -> Result<bool, StepError> {
        let op = s(step, "op");
        if !op.starts_with("l2_") {
            return Ok(false);
        }
        let c = find(&self.companies, s(step, "company"))?;
        let v = self.resolve_input(step.get("values").unwrap_or(&json!({})));
        match op {
            "l2_post_source" => {
                let source = self.lref(s(step, "source"));
                let lines = self.lines(&step["lines"]);
                let j = self
                    .book
                    .write(|tx| documents::post_source(tx, actor, c, &source, s(step, "date"), s(step, "memo"), lines))?;
                self.journals.insert(s(step, "ref").into(), j.id);
            }
            "l2_reverse_source" => {
                let source = self.lref(s(step, "source"));
                let j = self
                    .book
                    .write(|tx| documents::reverse_source(tx, actor, c, &source, s(step, "date"), s(step, "reason")))?;
                self.journals.insert(s(step, "ref").into(), j.id);
            }
            "l2_reminders" => {
                let mut built=fb_core::reminders::build(&mut self.book,actor,c,s(step,"today"),step["days"].as_i64())?;
                for family in ["bills","invoices"] {
                    for row in built[family]["rows"].as_array_mut().unwrap() {
                        let raw=row["document_id"].as_str().unwrap();
                        row["document_id"]=json!(self.l2_refs.iter().filter(|(_,v)|v.as_str()==raw).map(|(k,_)|k.clone()).min().expect("reminder document has a driver ref"));
                    }
                }
                self.l2_results.insert(s(step,"result").into(),built);
            }
            "l2_save" => {
                let id = step["record"].as_str().map(|r| self.lref(r));
                let obj = directory::save_record(&mut self.book, actor, c, model(s(step, "model")), &v, id.as_deref())?;
                if let Some(r) = step["ref"].as_str() {
                    self.l2_refs.insert(r.into(), obj["id"].as_str().unwrap().into());
                }
            }
            "l2_active" => {
                let id = self.lref(s(step, "record"));
                directory::set_active(
                    &mut self.book,
                    actor,
                    c,
                    model(s(step, "model")),
                    &id,
                    step["active"].as_bool().unwrap(),
                )?;
            }
            "l2_rate" | "l2_group" => {
                let obj = if op == "l2_rate" {
                    salestax::save_rate(&mut self.book, actor, c, &v)?
                } else {
                    salestax::save_group(&mut self.book, actor, c, &v)?
                };
                if let Some(r) = step["ref"].as_str() {
                    self.l2_refs.insert(r.into(), obj["id"].as_str().unwrap().into());
                    if op == "l2_rate" && v["new_agency_name"].as_str().is_some_and(|s| !s.trim().is_empty()) {
                        self.l2_refs
                            .insert(format!("agency-for:{r}"), obj["agency_id"].as_str().unwrap().into());
                    }
                }
            }
            "l2_tax_active" => {
                let id = self.lref(s(step, "record"));
                salestax::set_item_active(
                    &mut self.book,
                    actor,
                    c,
                    &id,
                    step["active"].as_bool().unwrap(),
                    step["expected_revision"].as_i64(),
                )?;
            }
            "l2_settings" => {
                let (_, offer) = salestax::save_settings(&mut self.book, actor, c, &v)?;
                if let Some(r) = step["result"].as_str() {
                    self.l2_results.insert(r.into(), json!(offer));
                }
            }
            "l2_bulk" => {
                let preview = self.book.read(|conn| salestax::bulk_preview(conn, c))?;
                let digest = if step["stale"] == true {
                    "stale"
                } else {
                    preview["digest"].as_str().unwrap()
                };
                salestax::finish_bulk_step(
                    &mut self.book,
                    actor,
                    c,
                    step["customers"].as_bool().unwrap(),
                    step["items"].as_bool().unwrap(),
                    digest,
                )?;
            }
            "l2_identity" => {
                let obj = self
                    .book
                    .write(|tx| documents::create_identity(tx, actor, c, s(step, "kind"), s(step, "request_key")))?;
                self.l2_refs.insert(s(step, "ref").into(), obj["id"].as_str().unwrap().into());
            }
            "l2_first_run_preview" => {
                let mut proposal=self.book.read(|tx|fb_core::first_run::propose(tx,c))?;
                proposal.as_object_mut().unwrap().remove("digest");
                proposal.as_object_mut().unwrap().remove("rows");
                for role in ["receivables","payables","undeposited","collected_tax"] {
                    if let Some(raw)=proposal["chosen"][role].as_str() {
                        let symbolic=self.accounts.iter().find(|(_,id)|id.to_db()==raw).map(|(key,_)|key.clone()).unwrap();
                        proposal["chosen"][role]=json!(symbolic);
                    }
                }
                if let Some(banks)=proposal.get_mut("chosen").and_then(|v|v.get_mut("banks")).and_then(Value::as_array_mut) {
                    for bank in banks {let raw=bank.as_str().unwrap();*bank=json!(self.accounts.iter().find(|(_,id)|id.to_db()==raw).map(|(key,_)|key.clone()).unwrap());}
                }
                proposal["checklist"]=self.book.read(|tx|fb_core::first_run::checklist(tx,c))?;
                self.l2_results.insert(s(step,"result").into(),proposal);
            }
            "l2_first_run_apply" => {
                let proposal=self.book.read(|tx|fb_core::first_run::propose(tx,c))?;
                fb_core::first_run::apply_proposal(&mut self.book,actor,c,if step["stale"]==true{"stale"}else{proposal["digest"].as_str().unwrap()},proposal["revision"].as_i64())?;
            }
            "l2_first_run_dismiss" => { fb_core::first_run::dismiss_first_run(&mut self.book,actor,c)?; }
            "l2_setup" => {
                documents::save_setup(&mut self.book, actor, c, &v, step["expected_revision"].as_i64())?;
            }
            "l2_snapshot" => {
                let id = self.lref(s(step, "source"));
                self.book.write(|tx| documents::snapshot_source_accounts(tx, actor, c, &id, &v))?;
            }
            "l2_totals" => {
                let resolved = if step["resolve"] == true {
                    self.book.read(|conn| salestax::document_tax(conn, c, &v, None, true))?
                } else {
                    v
                };
                let result = calculations::totals(
                    step["lines"].as_array().unwrap(),
                    resolved["tax_mode"].as_str().unwrap_or(""),
                    resolved.get("tax_rate").filter(|v| !v.is_null()),
                    resolved.get("tax_label").unwrap_or(&json!("")),
                    resolved.get("tax_snapshot").filter(|v| !v.is_null()),
                    step["for_posting"].as_bool().unwrap_or(false),
                )?;
                self.l2_results.insert(s(step, "result").into(), result);
            }
            _ => panic!("unknown l2 op {op}"),
        }
        Ok(true)
    }
    pub(super) fn dump_l2a(&mut self) -> Value {
        let view = L2aDump {
            company_order: &self.company_order, companies: &self.companies,
            accounts: &self.accounts, journals: &self.journals,
            l2_refs: &self.l2_refs, l2_results: &self.l2_results,
        };
        self.book.read(|conn| Ok::<_, fb_core::FbError>(view.dump(conn))).unwrap()
    }
}
struct L2aDump<'a> {
    company_order: &'a [String],
    companies: &'a HashMap<String, CompanyId>,
    accounts: &'a HashMap<String, AccountId>,
    journals: &'a HashMap<String, JournalId>,
    l2_refs: &'a HashMap<String, String>,
    l2_results: &'a Map<String, Value>,
}
impl L2aDump<'_> {
    fn reverse_l2(&self, id: &str) -> String {
        let mut refs = self
            .l2_refs
            .iter()
            .filter(|(_, v)| v.as_str() == id)
            .map(|(k, _)| k.clone())
            .collect::<Vec<_>>();
        refs.sort();
        refs.into_iter().next().unwrap_or_else(|| "?".into())
    }
    fn normalize_tax(&self, v: &Value) -> Value {
        let mut out = v.clone();
        if let Some(m) = out.as_object_mut() {
            for (k, v) in m {
                if ["id", "agency_id"].contains(&k.as_str()) {
                    if let Some(id) = v.as_str() {
                        let name = self.reverse_l2(id);
                        *v = json!(name);
                    }
                } else if v.is_array() {
                    for x in v.as_array_mut().unwrap() {
                        *x = self.normalize_tax(x);
                    }
                } else if v.is_object() {
                    *v = self.normalize_tax(v);
                }
            }
        }
        out
    }
    fn dump(&self, conn: &(impl fb_db::Query + ?Sized)) -> Value {
        let mut out = json!({"directory":[],"tax":[],"settings":{},"identities":[],"sequences":[],"setup":{},"snapshots":[],"results":{},"source_journals":[]});
        let aref = |id: &str| {
            self.accounts
                .iter()
                .find(|(_, v)| v.to_db() == id)
                .map(|(k, _)| k.clone())
                .unwrap_or_else(|| "?".into())
        };
        for c in self.company_order {
            let co = self.companies[c];
            for m in [Model::Customer, Model::Vendor, Model::Item] {
                for mut row in directory::list(conn, co, m).unwrap() {
                    let id = row["id"].as_str().unwrap();
                    let mut r = self.reverse_l2(id);
                    if r == "?" {
                        r = format!("vendor:{}", row["display_name"].as_str().unwrap_or(""));
                    }
                    row["id"] = json!(r);
                    row["company_id"] = json!(c);
                    row["model"] = json!(m.name());
                    for f in ["income_account", "expense_account", "default_expense_account"] {
                        if let Some(id) = row[f].as_str() {
                            row[f] = json!(aref(id));
                        }
                    }
                    out["directory"].as_array_mut().unwrap().push(row);
                }
            }
            for mut row in salestax::list(conn, co).unwrap() {
                row["id"] = json!(self.reverse_l2(row["id"].as_str().unwrap()));
                row["company"] = json!(c);
                if let Some(id) = row["agency_id"].as_str() {
                    row["agency_id"] = json!(self.reverse_l2(id));
                }
                if let Some(ids) = row.get_mut("member_ids").and_then(Value::as_array_mut) {
                    for id in ids {
                        *id = json!(self.reverse_l2(id.as_str().unwrap()));
                    }
                }
                out["tax"].as_array_mut().unwrap().push(row);
            }
            let mut settings = salestax::settings_for(conn, co).unwrap();
            settings.as_object_mut().unwrap().remove("id");
            if let Some(id) = settings["default_item_id"].as_str() {
                settings["default_item_id"] = json!(self.reverse_l2(id));
            }
            out["settings"][c] = settings;
            let mut setup = documents::setup_snapshot(conn, co).unwrap();
            setup.as_object_mut().unwrap().remove("id");
            for role in documents::ROLES {
                if let Some(id) = setup[*role].as_str() {
                    setup[*role] = json!(aref(id));
                }
            }
            for id in setup["banks"].as_array_mut().unwrap() {
                *id = json!(aref(id.as_str().unwrap()));
            }
            out["setup"][c] = setup;
            let mut stmt = conn
                .prepare("SELECT id FROM documents_documentidentity WHERE company_id=?1 ORDER BY id")
                .unwrap();
            let ids = stmt
                .query_map([co.to_db()], |r| r.get::<_, String>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            for id in ids {
                let mut row = documents::identity(conn, co, &id).unwrap();
                row["id"] = json!(self.reverse_l2(&id));
                row["company_id"] = json!(c);
                out["identities"].as_array_mut().unwrap().push(row);
                for mut row in documents::source_accounts(conn, &id).unwrap() {
                    row["source"] = json!(self.reverse_l2(&id));
                    row["account_id"] = json!(aref(row["account_id"].as_str().unwrap()));
                    out["snapshots"].as_array_mut().unwrap().push(row);
                }
            }
            let mut stmt = conn
                .prepare("SELECT kind,last_number FROM documents_documentsequence WHERE company_id=?1 ORDER BY kind")
                .unwrap();
            for row in stmt
                .query_map(params![co.to_db()], |r| Ok(json!([c, r.get::<_, String>(0)?, r.get::<_, i64>(1)?])))
                .unwrap()
            {
                out["sequences"].as_array_mut().unwrap().push(row.unwrap());
            }
        }
        let journal_refs = journal_aliases::journal_aliases(self.journals);
        let mut stmt = conn
            .prepare("SELECT source_id,id FROM ledger_journal WHERE source_id IS NOT NULL ORDER BY id")
            .unwrap();
        for row in stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .unwrap()
        {
            let (source, id) = row.unwrap();
            let reference = journal_refs
                .get(&id.parse().unwrap()).cloned().unwrap_or("?".into());
            out["source_journals"]
                .as_array_mut()
                .unwrap()
                .push(json!([self.reverse_l2(&source), reference]));
        }
        for (k, v) in self.l2_results {
            out["results"][k] = self.normalize_tax(v);
        }
        out
    }
}
