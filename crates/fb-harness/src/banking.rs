//! Differential adapter for the real banking services and their complete financial evidence.
use super::*;
use fb_core::banking;
impl Runner<'_> {
    fn bref(&self, key: &str) -> String {
        self.l2_refs.get(key).cloned().unwrap_or_else(|| key.into())
    }
    fn binput(&self, v: &Value) -> Value {
        match v {
            Value::Array(a) => json!(a.iter().map(|v| self.binput(v)).collect::<Vec<_>>()),
            Value::Object(m) => {
                let mut out = json!({});
                for (k, v) in m {
                    out[k] = if [
                        "bank_id",
                        "account_id",
                        "service_account_id",
                        "interest_account_id",
                    ]
                    .contains(&k.as_str())
                        && v.is_string()
                    {
                        json!(
                            self.accounts
                                .get(v.as_str().unwrap())
                                .map(|a| a.to_db())
                                .unwrap_or_else(|| AccountId::new().to_db())
                        )
                    } else if k == "receipt_ids" && v.is_array() {
                        json!(
                            v.as_array()
                                .unwrap()
                                .iter()
                                .map(|v| v
                                    .as_str()
                                    .map(|s| json!(self.bref(s)))
                                    .unwrap_or_else(|| v.clone()))
                                .collect::<Vec<_>>()
                        )
                    } else {
                        self.binput(v)
                    }
                }
                out
            }
            _ => v.clone(),
        }
    }
    pub(super) fn btrack(&mut self, name: &str, v: &Value) {
        self.l2_refs
            .entry(name.into())
            .or_insert(v["id"].as_str().unwrap().into());
        if let Some(s) = v["identity_id"].as_str() {
            self.l2_refs
                .entry(format!("{name}:identity"))
                .or_insert(s.into());
        }
        for (field, suffix) in [
            ("journal_id", "posted"),
            ("service_journal_id", "service"),
            ("interest_journal_id", "interest"),
        ] {
            if let Some(id) = v[field].as_str() {
                self.journals
                    .entry(format!("{name}:{suffix}"))
                    .or_insert(id.parse().unwrap());
            }
        }
    }
    fn bline(&mut self, name: &str) -> Result<String, StepError> {
        let Some((j, p)) = name.rsplit_once(':') else {
            return Err(StepError::MissingRef(name.into()));
        };
        let j = find(&self.journals, j)?;
        let p = p
            .parse::<i64>()
            .map_err(|_| StepError::MissingRef(name.into()))?;
        self.book
            .read(|q| -> fb_core::FbResult<_> {
                Ok(q.query_row(
                    "SELECT id FROM ledger_journalline WHERE journal_id=?1 AND position=?2",
                    fb_db::rusqlite::params![j.to_db(), p],
                    |r| r.get(0),
                )?)
            })
            .map_err(Into::into)
    }
    pub(super) fn apply_banking(&mut self, step: &Value, actor: UserId) -> Result<bool, StepError> {
        let op = s(step, "op");
        if !op.starts_with("banking_") {
            return Ok(false);
        }
        let c = find(&self.companies, s(step, "company"))?;
        let p = self.binput(step.get("values").unwrap_or(&json!({})));
        let expected = step["expected_revision"].as_i64();
        match op {
            "banking_receipt_guard" => {
                let key = self.bref(s(step, "receipt"));
                self.book.read(|q| {
                    services::access(q, actor, c)?;
                    banking::refuse_live_deposit(q, c, &key, s(step, "date"))
                })?;
            }
            "banking_reverse_journal" => {
                let j = find(&self.journals, s(step, "journal"))?;
                let j = banking::reverse_journal(
                    &mut self.book,
                    actor,
                    c,
                    j,
                    s(step, "date"),
                    s(step, "reason"),
                )?;
                self.journals.insert(s(step, "ref").into(), j.id);
            }
            "banking_review" => {
                let r = banking::review_deposit(&mut self.book, actor, c, &p)?;
                self.banking_reviews.insert(s(step, "review").into(), r);
            }
            "banking_deposit" => {
                let r = if let Some(name) = step["review"].as_str() {
                    self.banking_reviews
                        .get(name)
                        .cloned()
                        .ok_or_else(|| StepError::MissingRef(name.into()))?
                } else {
                    banking::review_deposit(&mut self.book, actor, c, &p)?
                };
                let v = banking::record_deposit(&mut self.book, actor, c, &p, &r)?;
                self.btrack(s(step, "ref"), &v);
            }
            "banking_reverse" => {
                let name = s(step, "deposit");
                let key = self.bref(name);
                let v = banking::reverse_deposit(
                    &mut self.book,
                    actor,
                    c,
                    &key,
                    expected,
                    &step["date"],
                    &step["reason"],
                )?;
                self.journals
                    .entry(format!("{name}:reversed"))
                    .or_insert(v["journal_id"].as_str().unwrap().parse().unwrap());
            }
            "banking_clear" => {
                let line = self.bline(s(step, "line"))?;
                banking::set_cleared(
                    &mut self.book,
                    actor,
                    c,
                    &line,
                    s(step, "state"),
                    expected,
                    None,
                )?;
            }
            "banking_begin" => {
                let aid = find(&self.accounts, s(step, "account"))?.to_db();
                let v = banking::begin(&mut self.book, actor, c, &aid, &p)?;
                self.btrack(s(step, "ref"), &v);
            }
            "banking_ticks" => {
                let aid = find(&self.accounts, s(step, "account"))?.to_db();
                let rec = self.bref(s(step, "reconciliation"));
                let mut v = p;
                let mut shown = json!({});
                for (k, rev) in step["shown"].as_object().unwrap() {
                    shown[self.bline(k)?] = rev.clone();
                }
                v["shown"] = shown;
                v["ticked"] = json!(
                    step["ticked"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|v| self.bline(v.as_str().unwrap()))
                        .collect::<Result<Vec<_>, _>>()?
                );
                v["expected_revision"] = step["expected_revision"].clone();
                banking::save_ticks(&mut self.book, actor, c, &rec, &aid, &v)?;
            }
            "banking_finish" => {
                let aid = find(&self.accounts, s(step, "account"))?.to_db();
                let rec = self.bref(s(step, "reconciliation"));
                banking::reconcile_now(
                    &mut self.book,
                    actor,
                    c,
                    &rec,
                    &aid,
                    expected,
                    &step["completion_key"],
                )?;
            }
            "banking_undo" => {
                let aid = find(&self.accounts, s(step, "account"))?.to_db();
                let rec = self.bref(s(step, "reconciliation"));
                banking::undo_last(
                    &mut self.book,
                    actor,
                    c,
                    &rec,
                    &aid,
                    expected,
                    &step["reason"],
                    &step["undo_key"],
                )?;
            }
            "banking_screen" | "banking_report" => {
                let aid = find(&self.accounts, s(step, "account"))?.to_db();
                let key = self.bref(s(step, "reconciliation"));
                let v = if op == "banking_screen" {
                    banking::screen(&mut self.book, actor, c, &key, &aid)?
                } else {
                    banking::report(&mut self.book, actor, c, &key, &aid)?
                };
                if op == "banking_screen" {
                    self.banking_screens.insert(s(step, "result").into(), v);
                } else {
                    self.banking_reports.insert(s(step, "result").into(), v);
                }
            }
            "banking_register" => {
                let aid = find(&self.accounts, s(step, "account"))?.to_db();
                let v = banking::build_on(
                    &mut self.book,
                    actor,
                    c,
                    &aid,
                    &p,
                    step["page"].as_i64().unwrap_or(1),
                    50,
                    self.scenario["today"].as_str().unwrap_or("2026-10-30"),
                )?;
                self.banking_results.insert(s(step, "result").into(), v);
            }
            _ => panic!("unknown banking op {op}"),
        };
        Ok(true)
    }
    fn bnames(&mut self) -> HashMap<String, String> {
        let mut names = HashMap::new();
        for (name, key) in &self.l2_refs {
            let old = names.entry(key.clone()).or_insert(name.clone());
            if name < old {
                *old = name.clone();
            }
        }
        for (name, key) in &self.companies {
            names.insert(key.to_db(), name.clone());
        }
        for (name, key) in &self.accounts {
            names.insert(key.to_db(), name.clone());
        }
        names.extend(self.journal_names());
        self.book
            .read(|q| -> fb_core::FbResult<()> {
                let mut st =
                    q.prepare("SELECT id,journal_id,position FROM ledger_journalline ORDER BY id")?;
                for row in st.query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                    ))
                })? {
                    let (id, j, p) = row?;
                    names.insert(id, format!("{}:{p}", names.get(&j).unwrap_or(&"?".into())));
                }
                Ok(())
            })
            .unwrap();
        names
    }
    pub(super) fn dump_banking(&mut self) -> Value {
        let names = self.bnames();
        let mut out = json!({"deposits":[],"deposit_lines":[],"reversals":[],"reconciliations":[],"statuses":[],"reconciliation_lines":[],"registers":{},"screens":{},"reports":{}});
        self.book.read(|q|->fb_core::FbResult<()>{
  for(table,fields,dest)in[("banking_deposit",DEPOSIT,"deposits"),("banking_depositreversal",REVERSAL,"reversals"),("banking_reconciliation",REC,"reconciliations")]{let mut st=q.prepare(&format!("SELECT data FROM {table} ORDER BY id"))?;for raw in st.query_map([],|r|r.get::<_,String>(0))?{let mut raw:Value=fb_db::parse_unique_json_as(&raw?).unwrap();if table=="banking_reconciliation"{raw=banking::reconciliation(q,raw["company_id"].as_str().unwrap().parse().unwrap(),raw["id"].as_str().unwrap(),raw["account_id"].as_str().unwrap())?;}let mut v=json!({});for k in fields{v[*k]=raw[*k].clone();}out[dest].as_array_mut().unwrap().push(normalize(&v,&names));}}
  for(sql,fields,dest)in[("SELECT deposit_id,receipt_id,undeposited_id,amount FROM banking_depositline ORDER BY deposit_id,receipt_id",&["deposit_id","receipt_id","undeposited_id","amount"][..],"deposit_lines"),("SELECT reconciliation_id,journal_line_id,debit,credit FROM banking_reconciliationline ORDER BY reconciliation_id,journal_line_id",&["reconciliation_id","line_id","debit","credit"][..],"reconciliation_lines")]{let mut st=q.prepare(sql)?;let rows=st.query_map([],|r|{let mut v=json!({});for(i,k)in fields.iter().enumerate(){v[*k]=if ["amount","debit","credit"].contains(k){json!(fb_core::Money::from_db(r.get(i)?).to_string())}else{json!(r.get::<_,String>(i)?)};}Ok(v)})?.collect::<Result<Vec<_>,_>>()?;out[dest]=normalize(&json!(rows),&names);}
  let mut st=q.prepare("SELECT journal_line_id,account_id,state,revision,reconciliation_id,u.username FROM banking_banklinestatus s JOIN fb_user u ON u.id=s.changed_by_id ORDER BY s.journal_line_id")?;let rows=st.query_map([],|r|Ok(json!({"line_id":r.get::<_,String>(0)?,"account_id":r.get::<_,String>(1)?,"state":r.get::<_,String>(2)?,"revision":r.get::<_,i64>(3)?,"reconciliation_id":r.get::<_,Option<String>>(4)?,"changed_by":r.get::<_,String>(5)?})))?.collect::<Result<Vec<_>,_>>()?;out["statuses"]=normalize(&json!(rows),&names);Ok(())}).unwrap();
        for (k, v) in &self.banking_results {
            out["registers"][k] = register_dump(v, &names);
        }
        for (k, v) in &self.banking_screens {
            let mut d = json!({});
            for field in [
                "words",
                "totals",
                "total",
                "too_many",
                "cap",
                "hidden_cleared",
                "notes",
                "sign",
            ] {
                d[field] = v[field].clone();
            }
            d["reconciliation_id"] = v["rec"]["id"].clone();
            for side in ["left", "right"] {
                d[side] = json!(
                    v[side]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(read_row)
                        .collect::<Vec<_>>()
                );
            }
            out["screens"][k] = normalize(&d, &names);
        }
        for (k, v) in &self.banking_reports {
            let mut d = json!({"reconciliation_id":v["rec"]["id"],"words":v["words"],"summary":v["summary"],"sides":v["sides"]});
            for side in d["sides"].as_array_mut().unwrap() {
                for group in side["groups"].as_array_mut().unwrap() {
                    group["rows"] = json!(
                        group["rows"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(read_row)
                            .collect::<Vec<_>>()
                    );
                }
            }
            out["reports"][k] = normalize(&d, &names);
        }
        for field in [
            "deposits",
            "reversals",
            "reconciliations",
            "deposit_lines",
            "reconciliation_lines",
            "statuses",
        ] {
            out[field]
                .as_array_mut()
                .unwrap()
                .sort_by_key(|v| v.to_string());
        }
        out
    }
    pub(super) fn banking_l2(&self, mut v: Value) -> Value {
        let names = self
            .accounts
            .iter()
            .map(|(k, v)| (v.to_db(), k))
            .collect::<HashMap<_, _>>();
        for row in v["snapshots"].as_array_mut().unwrap() {
            if let Some((prefix, key)) = row["role"].as_str().and_then(|s| s.split_once(':'))
                && prefix == "undeposited"
            {
                row["role"] = json!(format!("undeposited:{}", names.get(key).unwrap()));
            }
        }
        // Preserve source creation order. Only generated account-ID role suffixes
        // need canonical natural-reference order, as in the sales adapter.
        let rows = v["snapshots"].as_array_mut().unwrap();
        let mut start = 0;
        while start < rows.len() {
            let mut end = start + 1;
            while end < rows.len() && rows[end]["source"] == rows[start]["source"] {
                end += 1;
            }
            if rows[start..end]
                .iter()
                .any(|v| v["role"].as_str().unwrap().starts_with("undeposited:"))
            {
                rows[start..end].sort_by_key(|v| v["role"].as_str().unwrap().to_owned());
            }
            start = end;
        }
        v
    }
    pub(super) fn extend_banking_audit(&mut self, ids: &mut audit::Ids) {
        let names = self.bnames();
        self.book
            .read(|q| -> fb_core::FbResult<()> {
                for (kind, table) in [
                    ("deposit", "banking_deposit"),
                    ("reconciliation", "banking_reconciliation"),
                    ("line", "ledger_journalline"),
                ] {
                    let mut st = q.prepare(&format!("SELECT id FROM {table}"))?;
                    for id in st.query_map([], |r| r.get::<_, String>(0))? {
                        let id = id?;
                        ids.entry(kind.into())
                            .or_default()
                            .insert(id.clone(), names.get(&id).cloned().unwrap_or("?".into()));
                    }
                }
                Ok(())
            })
            .unwrap();
    }
}
pub(super) const DEPOSIT: &[&str] = &[
    "id",
    "company_id",
    "identity_id",
    "bank_id",
    "journal_id",
    "date",
    "amount",
    "reference",
    "memo",
    "actor_name",
    "company_snapshot",
];
pub(super) const REVERSAL: &[&str] = &["deposit_id", "journal_id", "date", "reason", "actor_name"];
pub(super) const REC: &[&str] = &[
    "id",
    "company_id",
    "account_id",
    "status",
    "statement_date",
    "ending_balance",
    "hide_later",
    "request_key",
    "revision",
    "started_by",
    "completed_by",
    "undone_by",
    "completion_key",
    "undo_key",
    "undo_reason",
    "beginning_balance",
    "cleared_debit",
    "cleared_credit",
    "cleared_count",
    "service_charge",
    "service_account_id",
    "service_journal_id",
    "interest",
    "interest_account_id",
    "interest_journal_id",
];
fn normalize(v: &Value, names: &HashMap<String, String>) -> Value {
    match v {
        Value::Array(a) => json!(a.iter().map(|v| normalize(v, names)).collect::<Vec<_>>()),
        Value::Object(m) => {
            let mut out = json!({});
            for (k, v) in m {
                out[k] = if [
                    "id",
                    "company_id",
                    "identity_id",
                    "bank_id",
                    "journal_id",
                    "account_id",
                    "receipt_id",
                    "deposit_id",
                    "undeposited_id",
                    "reconciliation_id",
                    "line_id",
                    "service_account_id",
                    "interest_account_id",
                    "service_journal_id",
                    "interest_journal_id",
                ]
                .contains(&k.as_str())
                    && v.is_string()
                {
                    json!(
                        names
                            .get(v.as_str().unwrap())
                            .map(String::as_str)
                            .unwrap_or("?")
                    )
                } else {
                    normalize(v, names)
                }
            }
            out
        }
        _ => v.clone(),
    }
}
fn register_dump(v: &Value, names: &HashMap<String, String>) -> Value {
    let mut out = json!({});
    for k in [
        "account_id",
        "page",
        "total",
        "opening",
        "closing",
        "totals",
        "labels",
    ] {
        out[k] = v[k].clone();
    }
    out["rows"] = json!(
        v["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                let mut row = json!({});
                for k in [
                    "id",
                    "journal_id",
                    "account_id",
                    "date",
                    "debit",
                    "credit",
                    "state",
                    "revision",
                    "balance",
                    "number",
                    "memo",
                    "party",
                    "other",
                    "more_splits",
                    "splits",
                ] {
                    row[k] = r[k].clone();
                }
                row
            })
            .collect::<Vec<_>>()
    );
    normalize(&out, names)
}
pub(super) fn normalize_audit(v: &mut Value, ids: &audit::Ids) -> bool {
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
    let action = v["action"].as_str().unwrap_or("").to_owned();
    if action == "document.accounts" {
        if let Some(rows) = v["detail"]["accounts"].as_array_mut() {
            for row in rows {
                if let Some(key) = row["role"]
                    .as_str()
                    .and_then(|s| s.strip_prefix("undeposited:"))
                {
                    row["role"] = json!(format!(
                        "undeposited:{}",
                        mapped(&json!(key), "account", ids).as_str().unwrap()
                    ));
                }
            }
        }
        return false;
    }
    if !(action.starts_with("deposit.")
        || action.starts_with("bank_line.")
        || action.starts_with("reconciliation."))
    {
        return false;
    }
    let (kind, key) = v["target"].as_str().unwrap().split_once(':').unwrap();
    let name = mapped(&json!(key), kind, ids);
    v["target"] = json!(format!("{kind}:{}", name.as_str().unwrap()));
    let d = &mut v["detail"];
    for (field, kind) in [
        ("bank_id", "account"),
        ("line_id", "line"),
        ("journal_id", "journal"),
        ("reconciliation_id", "reconciliation"),
        ("service_journal_id", "journal"),
        ("interest_journal_id", "journal"),
    ] {
        if let Some(v) = d.get_mut(field) {
            *v = mapped(v, kind, ids);
        }
    }
    for (field, kind) in [("line_ids", "line"), ("kept_journal_ids", "journal")] {
        if let Some(rows) = d.get_mut(field).and_then(Value::as_array_mut) {
            for v in rows {
                *v = mapped(v, kind, ids);
            }
        }
    }
    if let Some(rows) = d.get_mut("receipts").and_then(Value::as_array_mut) {
        for r in rows {
            r["receipt_id"] = mapped(&r["receipt_id"], "receipt", ids);
        }
    }
    true
}

fn read_row(v: &Value) -> Value {
    let mut row = json!({"line_id":v["id"]});
    for k in [
        "date", "type", "number", "memo", "party", "left", "right", "state", "revision",
    ] {
        row[k] = v[k].clone();
    }
    row
}
