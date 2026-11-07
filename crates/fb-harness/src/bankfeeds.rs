//! Differential adapter for the bank-feed core. The scenario hands over an already parsed pull (the
//! simplefin plugin's half); `bankfeeds_sync` runs `begin_sync` + `store_pull` (or
//! `record_sync_failure`) exactly where Python's `sync` runs `_sync_locked`, `_store` or `_record_sync`.
//! Natural keys and the final key pass match `py/bankfeeds_adapter.py`; see README "Bank feeds".
use super::*;
use fb_core::bankfeeds::{self, StagedAccount, StagedPull, StagedTransaction};
use fb_db::rusqlite::{OptionalExtension, params};
use std::collections::BTreeMap as Sorted;
use std::collections::HashSet;

#[derive(Default)]
pub(crate) struct State {
    connections: HashMap<String, String>,
    rules: HashMap<String, String>,
    results: Map<String, Value>,
    feed_names: HashMap<String, String>,
    tx_names: HashMap<String, String>,
    tx_ordinals: HashMap<i64, String>,
    docs: HashMap<(String, String), String>,
    /// Provenance for the final key pass, per record, never per value: (check document id, number) for
    /// every check revision a CHECK accept with a blank number created (the core's `default_check_number`,
    /// recorded even when the accept fails after its draft committed), and the result keys of suggestion
    /// steps (`check_number` there is always generated). Every other entry point (ordinary check, deposit,
    /// transfer and journal services, or an accept with an explicit number) writes literal numbers, which
    /// stay exact even when they look like `EFT-<n>`.
    generated_checks: HashSet<(String, String)>,
    suggested: HashSet<String>,
    /// Raw-table facts behind a successful query step (`evidence` in its outcome; see `bf_evidence`).
    pub(crate) evidence: Option<Value>,
    /// Runner-recorded provenance (FIX5; `py/bankfeeds_adapter.py` `record_provenance`): the raw provider ids
    /// each feed / transaction name stands for. Rows are found and named through the identity keys of those
    /// raw ids, never through the stored (redacted) text, so both runners partition rows by raw identity.
    feed_raw: HashMap<String, (String, String, String)>,
    tx_raw: HashMap<String, (String, String)>,
}

/// Decimal text to integer cents (the scenario writes money as Python Decimal text).
fn cents(v: &Value) -> Option<i64> {
    let text = v.as_str()?;
    let (neg, digits) = text.strip_prefix('-').map_or((false, text), |d| (true, d));
    let (whole, frac) = digits.split_once('.').unwrap_or((digits, ""));
    let frac = format!("{frac:0<2}");
    if frac.len() != 2 || whole.is_empty() || !whole.bytes().chain(frac.bytes()).all(|b| b.is_ascii_digit()) {
        panic!("scenario money must be decimal text with at most two places: {text}");
    }
    let n: i64 = format!("{whole}{frac}").parse().ok()?;
    Some(if neg { -n } else { n })
}
fn pull(v: &Value) -> StagedPull {
    let strings = |v: &Value| -> Vec<String> { v.as_array().map(|a| a.iter().map(|s| s.as_str().unwrap().to_owned()).collect()).unwrap_or_default() };
    let accounts = v["accounts"].as_array().cloned().unwrap_or_default();
    StagedPull {
        accounts: accounts
            .iter()
            .map(|a| StagedAccount {
                conn_key: a["conn_key"].as_str().unwrap().into(),
                remote_id: a["remote_id"].as_str().unwrap().into(),
                name: a["name"].as_str().unwrap().into(),
                org_name: a["org_name"].as_str().unwrap_or("").into(),
                currency: a["currency"].as_str().unwrap().into(),
                balance: cents(&a["balance"]),
                available: cents(&a["available"]),
                balance_epoch: a["balance_epoch"].as_i64(),
                skipped: strings(&a["skipped"]),
                transactions: a["transactions"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .map(|t| StagedTransaction {
                        remote_id: t["remote_id"].as_str().unwrap().into(),
                        posted_on: t["posted_on"].as_str().map(Into::into),
                        transacted_on: t["transacted_on"].as_str().map(Into::into),
                        amount: cents(&t["amount"]).expect("transaction amount"),
                        description: t["description"].as_str().unwrap_or("").into(),
                        pending: t["pending"].as_bool().unwrap_or(false),
                    })
                    .collect(),
            })
            .collect(),
        skipped: strings(&v["skipped"]),
        errors: strings(&v["errors"]),
    }
}
/// A pulled account's natural key, or `<conn>/~/<ref>` when the scenario names it (ids that differ only in
/// URL userinfo; no name carries a credential).
fn feed_name(conn: &str, a: &Value) -> String {
    match a["ref"].as_str() {
        Some(r) => format!("{conn}/~/{r}"),
        None => format!("{conn}/{}/{}", a["conn_key"].as_str().unwrap_or(""), a["remote_id"].as_str().unwrap_or("")),
    }
}
fn tx_name(feed: &str, t: &Value) -> String {
    format!("{feed}/{}", t["ref"].as_str().or(t["remote_id"].as_str()).unwrap_or(""))
}
/// The scenario's own record of what a bank-feed step names (identical in both runners; see
/// `py/bankfeeds_adapter.py` `manifest`): result name, created connection / rule, and the feed accounts and
/// transactions a pull lists. The comparer proves the dumped inventories against it.
pub(crate) fn manifest(step: &Value) -> Option<Value> {
    let op = step["op"].as_str().unwrap_or("");
    if op == "save_account" {
        // the account a step creates (an edit names `account` instead), for query-list completeness
        let created = if step.get("account").is_none() { step.get("ref").cloned().unwrap_or(Value::Null) } else { Value::Null };
        return Some(json!({"company": step.get("company").cloned().unwrap_or(Value::Null), "created": created}));
    }
    if !op.starts_with("bankfeeds_") {
        return None;
    }
    let mut out = json!({"company": step.get("company").cloned().unwrap_or(Value::Null), "result": step.get("result").cloned().unwrap_or(Value::Null)});
    if op == "bankfeeds_connect" {
        out["connection"] = step.get("ref").cloned().unwrap_or(Value::Null);
    }
    if op == "bankfeeds_save_rule" && step.get("rule").is_none() {
        out["rule"] = step.get("ref").cloned().unwrap_or(Value::Null);
    }
    if op == "bankfeeds_sync" || op == "bankfeeds_store" {
        let conn = step["connection"].as_str().unwrap_or("");
        let accounts = step["pull"]["accounts"].as_array().cloned().unwrap_or_default();
        let feed = |a: &Value| feed_name(conn, a);
        out["feeds"] = json!(accounts.iter().map(feed).collect::<Vec<_>>());
        out["transactions"] = json!(accounts
            .iter()
            .flat_map(|a| a["transactions"].as_array().cloned().unwrap_or_default().into_iter().map(move |t| tx_name(&feed(a), &t)))
            .collect::<Vec<_>>());
        out["pending"] = json!(accounts
            .iter()
            .flat_map(|a| a["transactions"].as_array().cloned().unwrap_or_default().into_iter().filter(|t| t["pending"] == true).map(move |t| tx_name(&feed(a), &t)))
            .collect::<Vec<_>>());
        // FIX5: an aliased transaction's id as this runner's `bankfeed.synced` audit writes it (the core
        // redacts at entry, then sanitizes); only scenarios with aliases carry the key.
        let aliases: Map<String, Value> = accounts
            .iter()
            .flat_map(|a| a["transactions"].as_array().cloned().unwrap_or_default().into_iter().map(move |t| (a.clone(), t)))
            .filter(|(_, t)| t["ref"].is_string())
            .map(|(a, t)| (tx_name(&feed(&a), &t), json!(bankfeeds::text::sanitize(&bankfeeds::text::redact(t["remote_id"].as_str().unwrap_or("")), 200))))
            .collect();
        if !aliases.is_empty() {
            out["aliases"] = Value::Object(aliases);
        }
    }
    Some(out)
}
fn epoch(text: Option<String>) -> Value {
    text.map(|t| json!(chrono::DateTime::parse_from_rfc3339(&t).expect("stored timestamp").timestamp())).unwrap_or(Value::Null)
}
fn min_name<'m>(names: impl Iterator<Item = &'m String>) -> String {
    names.min().cloned().unwrap_or_else(|| "?".into())
}

impl Runner<'_> {
    fn bf_connection(&self, name: &str) -> String {
        self.bf.connections.get(name).cloned().unwrap_or_else(|| fb_ids::new_v7().to_string())
    }
    fn bf_lookup(&mut self, sql: &str, a: String, b: &str, c: &str) -> String {
        self.book
            .read(|q| -> fb_core::FbResult<Option<String>> { Ok(q.query_row(sql, params![a, b, c], |r| r.get(0)).optional()?) })
            .unwrap()
            .unwrap_or_else(|| fb_ids::new_v7().to_string())
    }
    /// Records which raw ids a sync / store step's names stand for (before the step runs).
    fn bf_provenance(&mut self, step: &Value) {
        let conn = s(step, "connection");
        for a in step["pull"]["accounts"].as_array().cloned().unwrap_or_default() {
            let feed = feed_name(conn, &a);
            self.bf.feed_raw.insert(feed.clone(), (conn.to_owned(), a["conn_key"].as_str().unwrap_or("").into(), a["remote_id"].as_str().unwrap_or("").into()));
            for t in a["transactions"].as_array().cloned().unwrap_or_default() {
                self.bf.tx_raw.insert(tx_name(&feed, &t), (feed.clone(), t["remote_id"].as_str().unwrap_or("").into()));
            }
        }
    }
    /// The identity key of a raw provider id in this book (an unknown key when the book has no secret yet).
    fn bf_identity(&mut self, domain: &str, raw: &str) -> String {
        self.book.read(|q| bankfeeds::identity_of(q, domain, raw)).unwrap().unwrap_or_else(|| "-".into())
    }
    fn bf_feed(&mut self, name: &str) -> String {
        let (conn, key, remote) = self.bf.feed_raw.get(name).cloned().unwrap_or_else(|| {
            let mut parts = name.splitn(3, '/');
            (parts.next().unwrap().into(), parts.next().unwrap().into(), parts.next().unwrap().into())
        });
        let conn = self.bf_connection(&conn);
        let (key, remote) = (self.bf_identity(bankfeeds::CONN_KEY_DOMAIN, &key), self.bf_identity(bankfeeds::ACCOUNT_DOMAIN, &remote));
        self.bf_lookup("SELECT id FROM bankfeeds_account WHERE connection_id=?1 AND conn_key_identity=?2 AND remote_identity=?3", conn, &key, &remote)
    }
    fn bf_tx(&mut self, name: &str) -> String {
        let (feed, remote) = self.bf.tx_raw.get(name).cloned().unwrap_or_else(|| {
            let (feed, remote) = name.rsplit_once('/').unwrap();
            (feed.into(), remote.into())
        });
        let feed = self.bf_feed(&feed);
        let remote = self.bf_identity(bankfeeds::TRANSACTION_DOMAIN, &remote);
        self.bf_lookup("SELECT id FROM bankfeeds_transaction WHERE feed_account_id=?1 AND remote_identity=?2 AND ?3=?3", feed, &remote, "")
    }
    fn bf_line(&mut self, name: &str) -> String {
        let Some((j, p)) = name.rsplit_once(':') else { return fb_ids::new_v7().to_string() };
        let j = self.journals.get(j).map(|j| j.to_db()).unwrap_or_default();
        self.bf_lookup("SELECT id FROM ledger_journalline WHERE journal_id=?1 AND position=?2 AND ?3=?3", j, p, "")
    }
    fn bf_account(&self, v: &Value) -> Value {
        match v.as_str() {
            Some(r) => json!(self.accounts.get(r).map(|a| a.to_db()).unwrap_or_else(|| AccountId::new().to_db())),
            None => v.clone(),
        }
    }
    fn bf_values(&mut self, v: &Value) -> Value {
        let mut out = v.clone();
        for k in ["account_id", "transfer_account_id", "ledger_account_id"] {
            if let Some(x) = v.get(k) {
                out[k] = self.bf_account(x);
            }
        }
        if let Some(r) = v.get("vendor_id").and_then(Value::as_str) {
            out["vendor_id"] = json!(self.l2_refs.get(r).cloned().unwrap_or_else(|| fb_ids::new_v7().to_string()));
        }
        if let Some(r) = v.get("journal_line_id").and_then(Value::as_str) {
            out["journal_line_id"] = json!(self.bf_line(r));
        }
        if let Some(a) = v.get("receipt_ids").and_then(Value::as_array) {
            out["receipt_ids"] = json!(a.iter().map(|x| x.as_str().map(|s| json!(self.l2_refs.get(s).cloned().unwrap_or_else(|| s.into()))).unwrap_or(x.clone())).collect::<Vec<_>>());
        }
        out
    }
    /// Natural keys of every row seen so far, so audit rows of a later-withdrawn pending item still map.
    fn bf_remember(&mut self) {
        let names: HashMap<String, String> = self.bf.connections.iter().map(|(k, v)| (v.clone(), k.clone())).collect();
        type Row = (String, String, String, String, String);
        let (secret, feeds, txs) = self
            .book
            .read(|q| -> fb_core::FbResult<_> {
                let secret: Option<Vec<u8>> = q.query_row("SELECT secret FROM bankfeeds_identity_secret", [], |r| r.get(0)).optional()?;
                let mut st = q.prepare("SELECT id,connection_id,conn_key,remote_id,conn_key_identity||'/'||remote_identity FROM bankfeeds_account")?;
                let feeds: Vec<Row> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?.collect::<Result<_, _>>()?;
                let mut st = q.prepare("SELECT id,feed_account_id,remote_id,remote_identity,ordinal FROM bankfeeds_transaction")?;
                let txs = st
                    .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, i64>(4)?)))?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok((secret, feeds, txs))
            })
            .unwrap();
        // Provenance: a row is named by the raw id whose identity key it carries.
        let key = |domain: &str, raw: &str| secret.as_deref().map(|s| fb_db::bankfeeds_text::identity_key(s, domain, raw)).unwrap_or_default();
        let feeds_by_key: HashMap<(String, String), String> = self
            .bf
            .feed_raw
            .iter()
            .filter_map(|(name, (conn, k, remote))| {
                let conn = self.bf.connections.get(conn)?;
                Some(((conn.clone(), format!("{}/{}", key(bankfeeds::CONN_KEY_DOMAIN, k), key(bankfeeds::ACCOUNT_DOMAIN, remote))), name.clone()))
            })
            .collect();
        for (id, conn, k, remote, identity) in feeds {
            let name = feeds_by_key
                .get(&(conn.clone(), identity))
                .cloned()
                .unwrap_or_else(|| format!("{}/{k}/{remote}", names.get(&conn).map(String::as_str).unwrap_or("?")));
            self.bf.feed_names.insert(id, name);
        }
        let txs_by_key: HashMap<(String, String), String> =
            self.bf.tx_raw.iter().map(|(name, (feed, remote))| ((feed.clone(), key(bankfeeds::TRANSACTION_DOMAIN, remote)), name.clone())).collect();
        for (id, feed, remote, identity, ordinal) in txs {
            let feed = self.bf.feed_names[&feed].clone();
            let name = txs_by_key.get(&(feed.clone(), identity)).cloned().unwrap_or_else(|| format!("{feed}/{remote}"));
            self.bf.tx_names.insert(id, name.clone());
            self.bf.tx_ordinals.insert(ordinal, name);
        }
    }
    fn bf_jref(&self, id: &str) -> String {
        min_name(self.journals.iter().filter(|(_, j)| j.to_db() == id).map(|(k, _)| k))
    }
    fn bf_aref(&self, id: &str) -> String {
        min_name(self.accounts.iter().filter(|(_, a)| a.to_db() == id).map(|(k, _)| k))
    }
    fn bf_line_name(&mut self, line: &str) -> String {
        let (j, p): (String, i64) =
            self.book.read(|q| -> fb_core::FbResult<_> { Ok(q.query_row("SELECT journal_id,position FROM ledger_journalline WHERE id=?1", [line], |r| Ok((r.get(0)?, r.get(1)?)))?) }).unwrap();
        format!("{}:{p}", self.bf_jref(&j))
    }
    /// Name the document an accept created, through the owning module's own tracking.
    fn bf_track(&mut self, name: Option<&str>, item: &bankfeeds::Item, actor: UserId, c: CompanyId) -> Result<(), StepError> {
        let Some(journal) = item.journal_id.clone() else { return Ok(()) };
        if item.accepted_as == "MATCH" {
            return Ok(()); // a match names an existing entry; nothing new to track
        }
        let auto = format!("bf:{}:{}", self.bf.tx_names.get(&item.id).cloned().unwrap_or_else(|| item.id.clone()), item.return_count);
        let name = name.unwrap_or(&auto);
        let (view, id) = item.reference_url.rsplit_once(':').map(|(v, i)| (v.to_owned(), i.to_owned())).unwrap_or_default();
        if self.bf.docs.contains_key(&(view.clone(), id.clone())) {
            return Ok(()); // a retry hands back the item; its document is named already
        }
        self.bf.docs.insert((view, id.clone()), name.into());
        match item.accepted_as.as_str() {
            "CHECK" => {
                let b = fb_core::checks::get_check(&mut self.book, actor, c, &id)?;
                self.ctrack(name, &b, true);
            }
            "TRANSFER" => {
                let v = self.book.read(|q| -> fb_core::FbResult<Value> {
                    Ok(q.query_row("SELECT id,identity_id,journal_id FROM transfers_transfer WHERE id=?1", [&id], |r| {
                        Ok(json!({"id":r.get::<_,String>(0)?,"identity_id":r.get::<_,String>(1)?,"journal_id":r.get::<_,String>(2)?}))
                    })?)
                })?;
                self.ctrack(name, &v, false);
            }
            "DEPOSIT" => {
                let v = self.book.read(|q| fb_core::banking::deposit(q, c, &id))?;
                self.btrack(name, &v);
            }
            _ => {
                self.journals.entry(name.into()).or_insert(journal.parse().unwrap());
            }
        }
        Ok(())
    }

    pub(super) fn apply_bankfeeds(&mut self, step: &Value, actor: UserId) -> Result<bool, StepError> {
        let op = s(step, "op");
        if !op.starts_with("bankfeeds_") {
            return Ok(false);
        }
        if op == "bankfeeds_sync" || op == "bankfeeds_store" {
            self.bf_provenance(step);
        }
        let result = self.bf_apply(step, actor, op);
        self.bf_remember();
        result.map(|()| true)
    }

    fn bf_apply(&mut self, step: &Value, actor: UserId, op: &str) -> Result<(), StepError> {
        let c = find(&self.companies, s(step, "company"))?;
        let flag = |k: &str| step[k].as_bool().unwrap_or(false);
        let store_result = |runner: &mut Self, v: Value| {
            if let Some(r) = step["result"].as_str() {
                runner.bf.suggested.remove(r);
                runner.bf.results.insert(r.into(), v);
            }
        };
        match op {
            "bankfeeds_connect" => {
                let label = step.get("label").cloned().unwrap_or(json!(bankfeeds::DEFAULT_LABEL));
                // Synthetic host policy: keep every later main/desktop service under a real retained
                // mask in the combined integration scenario. Never a provider login or real token.
                let secrets = if step["test_mask"] == true {
                    bankfeeds::SecretSet::from_credentials("HarnessUser12345", "HarnessPassword12345", Vec::<String>::new())?
                } else { bankfeeds::SecretSet::none() };
                let v = bankfeeds::connect(&mut self.book, actor, c, &label, step["host"].as_str().unwrap_or("bridge.example.test"), &secrets)?;
                crate::fake_credentials::store(&mut self.book, &self.key, &c.to_db(), v["id"].as_str().expect("connected id"))?;
                self.bf.connections.entry(s(step, "ref").into()).or_insert(v["id"].as_str().unwrap().into());
            }
            "bankfeeds_forget" => {
                let conn = self.bf_connection(s(step, "connection"));
                bankfeeds::forget(&mut self.book, actor, c, &conn, &bankfeeds::SecretSet::none())?;
                self.book.write(|tx| tx.execute("DELETE FROM plugin_credential WHERE company_id=?1 AND plugin_id='simplefin' AND connection_id=?2", params![c.to_db(), conn])).map_err(FbError::from)?;
            }
            "bankfeeds_sync" => {
                let conn = self.bf_connection(s(step, "connection"));
                let interval = step["min_interval"].as_i64().unwrap_or(bankfeeds::SCHEDULED_MIN_INTERVAL_SECS);
                let start = bankfeeds::begin_sync(&mut self.book, actor, c, &conn, s(step, "now"), interval, flag("force"))?;
                if let Some(message) = step["fail"].as_str() {
                    bankfeeds::record_sync_failure(&mut self.book, actor, c, &conn, s(step, "now"), message, flag("scheduled"), &bankfeeds::SecretSet::none())?;
                    // the plugin's SimpleFinError carries a redacted message, as Python's does
                    return Err(FbError::validation(bankfeeds::text::redact(message)).into());
                }
                let v = bankfeeds::store_pull(&mut self.book, actor, c, &conn, &pull(&step["pull"]), s(step, "now"), flag("scheduled"), Some(start), &bankfeeds::SecretSet::none())?;
                store_result(self, v);
            }
            "bankfeeds_store" => {
                let conn = self.bf_connection(s(step, "connection"));
                let window = step["window_start"].as_str().map(|t| chrono::DateTime::parse_from_rfc3339(t).expect("window").timestamp());
                let v = bankfeeds::store_pull(&mut self.book, actor, c, &conn, &pull(&step["pull"]), s(step, "now"), flag("scheduled"), window, &bankfeeds::SecretSet::none())?;
                store_result(self, v);
            }
            "bankfeeds_map" => {
                let feed = self.bf_feed(s(step, "feed"));
                let account = self.bf_account(step.get("account").unwrap_or(&Value::Null));
                bankfeeds::map_account(&mut self.book, actor, c, &feed, &account, &bankfeeds::SecretSet::none())?;
            }
            "bankfeeds_mappable" | "bankfeeds_rule_accounts" => {
                let list = if op == "bankfeeds_mappable" {
                    bankfeeds::mappable_accounts(&mut self.book, actor, c)?
                } else {
                    bankfeeds::rule_accounts(&mut self.book, actor, c)?
                };
                let v = json!({"accounts": list.iter().map(|a| self.bf_aref(&a.id.to_db())).collect::<Vec<_>>()});
                store_result(self, v);
                self.bf.evidence = Some(self.bf_evidence(c, false));
            }
            "bankfeeds_save_rule" => {
                let mut v = self.bf_values(step.get("values").unwrap_or(&json!({})));
                if let Some(r) = step["rule"].as_str() {
                    v["rule_id"] = json!(self.bf.rules.get(r).cloned().unwrap_or_else(|| fb_ids::new_v7().to_string()));
                }
                let rule = bankfeeds::save_rule(&mut self.book, actor, c, &v, &bankfeeds::SecretSet::none())?;
                if let Some(r) = step["ref"].as_str() {
                    self.bf.rules.entry(r.into()).or_insert(rule.id);
                }
            }
            "bankfeeds_move_rule" | "bankfeeds_rule_active" => {
                let rule = self.bf.rules.get(s(step, "rule")).cloned().unwrap_or_else(|| fb_ids::new_v7().to_string());
                if op == "bankfeeds_move_rule" {
                    bankfeeds::move_rule(&mut self.book, actor, c, &rule, &step["direction"], &bankfeeds::SecretSet::none())?;
                } else {
                    bankfeeds::set_rule_active(&mut self.book, actor, c, &rule, &step["active"], &bankfeeds::SecretSet::none())?;
                }
            }
            "bankfeeds_suggest" => {
                let tx = self.bf_tx(s(step, "tx"));
                let got = bankfeeds::suggest(&mut self.book, actor, c, &tx)?;
                let item = self.book.read(|q| bankfeeds::item(q, c, &tx))?;
                let rules: HashMap<String, String> = self.bf.rules.iter().map(|(k, v)| (v.clone(), k.clone())).collect();
                let receipts: Vec<Value> = got
                    .receipts
                    .iter()
                    .map(|x| json!({"receipt":min_name(self.l2_refs.iter().filter(|(_, v)| **v == x.receipt_id).map(|(k, _)| k)),"label":x.label,"date":x.date}))
                    .collect();
                let mut matches = vec![];
                for m in &got.matches {
                    matches.push(json!({"line":self.bf_line_name(&m.line_id),"journal":self.bf_jref(&m.journal_id),"label":m.label,"date":m.date}));
                }
                let v = json!({"kind":got.kind,"matches":matches,"receipts":receipts,
                    "rule":got.rule.as_ref().map(|r| rules.get(&r.id).cloned().unwrap_or_else(|| "?".into())),
                    "account_id":got.account_id.as_deref().map(|a| self.bf_aref(a)),"payee_name":got.payee_name,
                    "check_number":bankfeeds::default_check_number(&item)});
                store_result(self, v);
                if let Some(r) = step["result"].as_str() {
                    self.bf.suggested.insert(r.into());
                }
            }
            "bankfeeds_accept" => {
                let tx = self.bf_tx(s(step, "tx"));
                let v = self.bf_values(step.get("values").unwrap_or(&json!({})));
                // `_text(check_number, 20) or default_check_number(item)`: a blank number is generated. The
                // revisions this step creates with that number are the generated ones, whatever happens next.
                let blank = !v.get("check_number").and_then(Value::as_str).is_some_and(|n| !fb_core::py::py_strip(n).is_empty());
                let generated = (blank && v["kind"] == "CHECK").then(|| self.book.read(|q| bankfeeds::item(q, c, &tx)).ok()).flatten().map(|i| bankfeeds::default_check_number(&i));
                let before = if generated.is_some() { self.bf_check_revisions() } else { vec![] };
                let accepted = bankfeeds::accept(&mut self.book, actor, c, &tx, &v, &bankfeeds::SecretSet::none());
                if let Some(number) = generated {
                    let known: HashSet<String> = before.into_iter().map(|(rev, _, _)| rev).collect();
                    for (rev, doc, n) in self.bf_check_revisions() {
                        if !known.contains(&rev) && n == number {
                            self.bf.generated_checks.insert((doc, n));
                        }
                    }
                }
                let item = accepted?;
                self.bf_track(step["ref"].as_str(), &item, actor, c)?;
            }
            "bankfeeds_undo" => self.bf_undo(step, actor, c)?,
            "bankfeeds_ignore" => {
                let tx = self.bf_tx(s(step, "tx"));
                bankfeeds::ignore(&mut self.book, actor, c, &tx, &bankfeeds::SecretSet::none())?;
            }
            "bankfeeds_return" => {
                let tx = self.bf_tx(s(step, "tx"));
                bankfeeds::return_to_review(&mut self.book, actor, c, &tx, &bankfeeds::SecretSet::none())?;
            }
            "bankfeeds_items" => {
                let list = bankfeeds::items(&mut self.book, actor, c)?;
                self.bf_remember();
                let v = json!({"items": list.iter().map(|i| self.bf.tx_names.get(&i.id).cloned().unwrap_or_else(|| "?".into())).collect::<Vec<_>>()});
                store_result(self, v);
                self.bf.evidence = Some(self.bf_evidence(c, true));
            }
            _ => return Err(StepError::MissingRef(format!("unknown bank-feed operation {op}"))),
        }
        Ok(())
    }

    /// Every check revision: (revision id, document id, check number).
    fn bf_check_revisions(&mut self) -> Vec<(String, String, String)> {
        self.book
            .read(|q| -> fb_core::FbResult<Vec<(String, String, String)>> {
                let mut st = q.prepare("SELECT id,document_id,check_number FROM checks_checkrevision")?;
                Ok(st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?)
            })
            .unwrap()
    }

    /// The raw rows a query list is drawn from, read straight from the tables (not through the service), so
    /// the comparer can recompute the list's members and order: every account of the company (ref, code,
    /// kind, active, postable, control role) or every transaction of the company (name, posted date, rank
    /// of its ordinal). Sorted by name so both runners record the same value.
    fn bf_evidence(&mut self, c: CompanyId, transactions: bool) -> Value {
        if transactions {
            let rows = self
                .book
                .read(|q| -> fb_core::FbResult<Vec<(String, Option<String>)>> {
                    let mut st = q.prepare(
                        "SELECT t.id,t.posted_on FROM bankfeeds_transaction t JOIN bankfeeds_account f ON f.id=t.feed_account_id JOIN bankfeeds_connection n ON n.id=f.connection_id WHERE n.company_id=?1 ORDER BY t.ordinal",
                    )?;
                    Ok(st.query_map([c.to_db()], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?)
                })
                .unwrap();
            let mut out: Vec<Value> = rows
                .iter()
                .enumerate()
                .map(|(i, (id, posted))| json!({"tx": self.bf.tx_names.get(id).cloned().unwrap_or_else(|| "?".into()), "posted_on": posted, "seq": i + 1}))
                .collect();
            out.sort_by(|a, b| a["tx"].as_str().cmp(&b["tx"].as_str()));
            return json!({"transactions": out});
        }
        // id, code, kind, active, postable, control
        type Facts = (String, String, String, bool, bool, bool);
        let rows = self
            .book
            .read(|q| -> fb_core::FbResult<Vec<Facts>> {
                let mut st = q.prepare(
                    "SELECT a.id,a.code,a.kind,a.active,a.postable,EXISTS(SELECT 1 FROM fb_control_accounts x WHERE x.company_id=a.company_id AND x.account_id=a.id) FROM ledger_account a WHERE a.company_id=?1",
                )?;
                Ok(st.query_map([c.to_db()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?.collect::<Result<_, _>>()?)
            })
            .unwrap();
        let mut out: Vec<Value> = rows
            .iter()
            .map(|(id, code, kind, active, postable, control)| json!({"account": self.bf_aref(id), "code": code, "kind": kind, "active": active, "postable": postable, "control": control}))
            .collect();
        out.sort_by(|a, b| a["account"].as_str().cmp(&b["account"].as_str()));
        json!({"accounts": out})
    }

    /// Harness op: void / reverse the document an accepted item became, through its own module.
    fn bf_undo(&mut self, step: &Value, actor: UserId, c: CompanyId) -> Result<(), StepError> {
        let tx = self.bf_tx(s(step, "tx"));
        let item = self.book.read(|q| bankfeeds::item(q, c, &tx))?;
        if item.status != "ACCEPTED" {
            return Err(FbError::validation("Nothing to undo: this item is not accepted.").into());
        }
        let (view, id) = item.reference_url.rsplit_once(':').map(|(v, i)| (v.to_owned(), i.to_owned())).unwrap_or_default();
        let name = self.bf.docs.get(&(view, id.clone())).cloned().unwrap_or_else(|| "?".into());
        let (date, reason) = (&step["date"], &step["reason"]);
        match item.accepted_as.as_str() {
            "CHECK" => {
                let revision = fb_core::checks::get_check(&mut self.book, actor, c, &id)?["revision"].as_i64();
                let b = fb_core::checks::void_check(&mut self.book, actor, c, &id, revision, date, reason)?;
                self.ctrack(&name, &b, true);
            }
            "TRANSFER" => {
                let v = fb_core::transfers::void_transfer(&mut self.book, actor, c, &id, Some(1), date, reason)?;
                self.journals.entry(format!("{name}:reversal")).or_insert(v["journal_id"].as_str().unwrap().parse().unwrap());
            }
            "DEPOSIT" => {
                let v = fb_core::banking::reverse_deposit(&mut self.book, actor, c, &id, Some(1), date, reason)?;
                self.journals.entry(format!("{name}:reversed")).or_insert(v["journal_id"].as_str().unwrap().parse().unwrap());
            }
            _ => {
                let j = item.journal_id.clone().unwrap_or_default().parse().unwrap();
                let v = services::reverse_journal(&mut self.book, actor, c, j, date.as_str().unwrap_or(""), reason.as_str().unwrap_or(""))?;
                self.journals.entry(format!("{name}:undo")).or_insert(v.id);
            }
        }
        Ok(())
    }

    fn bf_reference_url(&self, url: &str) -> String {
        let Some((view, id)) = url.rsplit_once(':') else { return url.into() };
        if view == "journal" {
            return format!("journal:{}", self.bf_jref(id));
        }
        format!("{view}:{}", self.bf.docs.get(&(view.to_owned(), id.to_owned())).cloned().unwrap_or_else(|| "?".into()))
    }

    pub(super) fn dump_bankfeeds(&mut self) -> Value {
        self.bf_remember();
        let conns: HashMap<String, String> = self.bf.connections.iter().map(|(k, v)| (v.clone(), k.clone())).collect();
        let rules: HashMap<String, String> = self.bf.rules.iter().map(|(k, v)| (v.clone(), k.clone())).collect();
        let companies: HashMap<String, String> = self.companies.iter().map(|(k, v)| (v.to_db(), k.clone())).collect();
        let users: HashMap<String, String> = self.users.iter().map(|(k, v)| (v.to_db(), k.clone())).collect();
        let raw = self
            .book
            .read(|q| -> fb_core::FbResult<Vec<Vec<Value>>> {
                let mut tables = vec![];
                for sql in [
                    "SELECT id,company_id,label,host,active,forgotten_at,created_by_id,last_sync_at,last_sync_ok,last_sync_summary,sync_count FROM bankfeeds_connection",
                    "SELECT id,connection_id,conn_key,remote_id,name,org_name,currency,balance,available_balance,balance_at,ledger_account_id,mapped_at,last_seen_at,active FROM bankfeeds_account",
                    "SELECT id,feed_account_id,remote_id,posted_on,transacted_on,amount,description,pending,status,accepted_as,journal_id,journal_line_id,reference,reference_url,decided_at,decided_by_id,return_count FROM bankfeeds_transaction",
                    "SELECT id,company_id,position,name,match_text,sign,amount_min,amount_max,account_id,payee_name,active FROM bankfeeds_rule",
                ] {
                    let mut st = q.prepare(sql)?;
                    let n = st.column_count();
                    let rows = st
                        .query_map([], |r| {
                            (0..n)
                                .map(|i| {
                                    Ok(match r.get::<_, fb_db::rusqlite::types::Value>(i)? {
                                        fb_db::rusqlite::types::Value::Null => Value::Null,
                                        fb_db::rusqlite::types::Value::Integer(n) => json!(n),
                                        fb_db::rusqlite::types::Value::Text(t) => json!(t),
                                        _ => Value::Null,
                                    })
                                })
                                .collect::<Result<Vec<_>, _>>()
                                .map(Value::Array)
                        })?
                        .collect::<Result<Vec<_>, _>>()?;
                    tables.push(rows);
                }
                Ok(tables)
            })
            .unwrap();
        let t = |v: &Value| v.as_str().unwrap_or("").to_owned();
        let name_of = |m: &HashMap<String, String>, v: &Value| m.get(&t(v)).cloned().unwrap_or_else(|| "?".into());
        let mut out = json!({"connections":[],"accounts":[],"transactions":[],"rules":[],"results":self.bf.results});
        for r in &raw[0] {
            out["connections"].as_array_mut().unwrap().push(json!({"id":name_of(&conns,&r[0]),"company_id":name_of(&companies,&r[1]),"label":r[2],"host":r[3],
                "active":r[4]==1,"forgotten":!r[5].is_null(),"created_by":name_of(&users,&r[6]),"last_sync_at":epoch(r[7].as_str().map(Into::into)),
                "last_sync_ok":r[8].as_i64().map(|n| n==1),"last_sync_summary":r[9],"sync_count":r[10]}));
        }
        for r in &raw[1] {
            out["accounts"].as_array_mut().unwrap().push(json!({"id":name_of(&self.bf.feed_names,&r[0]),"connection_id":name_of(&conns,&r[1]),"conn_key":r[2],"remote_id":r[3],
                "name":r[4],"org_name":r[5],"currency":r[6],"balance":r[7],"available_balance":r[8],"balance_at":epoch(r[9].as_str().map(Into::into)),
                "ledger_account_id":r[10].as_str().map(|a| self.bf_aref(a)),"mapped":!r[11].is_null(),"last_seen_at":epoch(r[12].as_str().map(Into::into)),"active":r[13]==1}));
        }
        let lines: Vec<Option<String>> = raw[2].iter().map(|r| r[11].as_str().map(str::to_owned)).collect();
        let mut line_names = vec![];
        for line in lines {
            line_names.push(line.map(|l| self.bf_line_name(&l)));
        }
        for (r, line) in raw[2].iter().zip(line_names) {
            out["transactions"].as_array_mut().unwrap().push(json!({"id":name_of(&self.bf.tx_names,&r[0]),"feed_account_id":name_of(&self.bf.feed_names,&r[1]),"remote_id":r[2],
                "posted_on":r[3],"transacted_on":r[4],"amount":r[5],"description":r[6],"pending":r[7]==1,"status":r[8],"accepted_as":r[9],
                "journal_id":r[10].as_str().map(|j| self.bf_jref(j)),"journal_line_id":line,"reference":r[12],"reference_url":self.bf_reference_url(&t(&r[13])),
                "decided":!r[14].is_null(),"decided_by":r[15].as_str().map(|u| name_of(&users,&json!(u))),"return_count":r[16]}));
        }
        for r in &raw[3] {
            out["rules"].as_array_mut().unwrap().push(json!({"id":name_of(&rules,&r[0]),"company_id":name_of(&companies,&r[1]),"position":r[2],"name":r[3],"match_text":r[4],
                "sign":r[5],"amount_min":r[6],"amount_max":r[7],"account_id":self.bf_aref(&t(&r[8])),"payee_name":r[9],"active":r[10]==1}));
        }
        for key in ["connections", "accounts", "transactions", "rules"] {
            out[key].as_array_mut().unwrap().sort_by_key(|v| v["id"].as_str().unwrap_or("").to_owned());
        }
        out
    }

    pub(super) fn extend_bankfeeds_audit(&mut self, ids: &mut audit::Ids) {
        self.bf_remember();
        let flip = |m: &HashMap<String, String>| m.iter().map(|(k, v)| (v.clone(), k.clone())).collect::<HashMap<_, _>>();
        ids.insert("bankfeedconnection".into(), flip(&self.bf.connections));
        ids.insert("bankfeedaccount".into(), self.bf.feed_names.clone());
        ids.insert("bankfeedtransaction".into(), self.bf.tx_names.clone());
        ids.insert("bankfeedrule".into(), flip(&self.bf.rules));
        ids.insert("bf_account".into(), self.accounts.values().map(|a| (a.to_db(), self.bf_aref(&a.to_db()))).collect());
        ids.insert("bf_journal".into(), self.journals.values().map(|j| (j.to_db(), self.bf_jref(&j.to_db()))).collect());
    }

    /// Rewrite `EFT-<ordinal>` and the uuid5 accept request keys to natural-key labels, in generated values
    /// only, by record provenance (`generated_checks`, `suggested`), never by field name or value: a check row's
    /// `number_key`, a check revision's `check_number` and a `check:` audit event's `check_number` when that
    /// document got that number from the core, and a suggestion result's `check_number`. A whole
    /// `request_key` value derived here becomes its label. Names, memos and every other field stay exact.
    /// Candidates come from this side's own check revisions and journal lines with v2's formula, so a key
    /// the core derived differently stays a raw uuid and mismatches Python's label.
    pub(super) fn bankfeeds_finalize(&mut self, mut out: Value) -> Value {
        self.bf_remember();
        let accounts: HashMap<String, String> = self.accounts.values().map(|a| (a.to_db(), self.bf_aref(&a.to_db()))).collect();
        let mut vendor_names: Vec<(&String, &String)> = self.l2_refs.iter().collect();
        vendor_names.sort();
        let mut vendors: HashMap<String, String> = HashMap::new();
        for (name, id) in vendor_names {
            vendors.entry(id.clone()).or_insert(name.clone());
        }
        // EFT-<ordinal> -> EFT#T<rank of the natural key>: short enough to stay a valid 20-character check number
        let mut ranked: Vec<&String> = self.bf.tx_names.values().collect();
        ranked.sort();
        ranked.dedup();
        let rank: HashMap<&String, usize> = ranked.into_iter().enumerate().map(|(i, n)| (n, i + 1)).collect();
        let eft: HashMap<String, String> = self.bf.tx_ordinals.iter().map(|(k, v)| (k.to_string(), format!("EFT#T{}", rank[v]))).collect();
        let fix_eft = |text: &str| {
            text.strip_prefix("EFT-")
                .filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
                .and_then(|n| eft.get(n).cloned())
                .unwrap_or_else(|| text.to_owned())
        };
        let generated = &self.bf.generated_checks;
        let names = self.cnames();
        let generated_refs: HashSet<(String, String)> = generated.iter().filter_map(|(doc, n)| names.get(doc).map(|r| (r.clone(), n.clone()))).collect();
        let fix_if = |doc: &Value, field: &mut Value| {
            if let (Some(doc), Some(n)) = (doc.as_str(), field.as_str())
                && generated_refs.contains(&(doc.to_owned(), n.to_owned()))
            {
                *field = json!(fix_eft(n));
            }
        };
        // `pointer_mut`, not indexing: indexing a missing key would add it to the dump.
        for row in out.pointer_mut("/checks/check").and_then(Value::as_array_mut).into_iter().flatten() {
            let doc = row["id"].clone();
            if let Some(n) = row.get_mut("number_key") {
                fix_if(&doc, n);
            }
        }
        for row in out.pointer_mut("/checks/checkrevision").and_then(Value::as_array_mut).into_iter().flatten() {
            let doc = row["document_id"].clone();
            if let Some(n) = row.get_mut("check_number") {
                fix_if(&doc, n);
            }
        }
        for events in out.pointer_mut("/audit").and_then(Value::as_object_mut).into_iter().flat_map(|m| m.values_mut()) {
            for e in events.as_array_mut().into_iter().flatten() {
                let Some(doc) = e["target"].as_str().and_then(|t| t.strip_prefix("check:")).map(|d| json!(d)) else { continue };
                if let Some(n) = e.pointer_mut("/detail/check_number") {
                    fix_if(&doc, n);
                }
            }
        }
        for key in &self.bf.suggested {
            if let Some(n) = out.pointer_mut("/bankfeeds/results").and_then(|r| r.get_mut(key)).and_then(|r| r.get_mut("check_number"))
                && let Some(text) = n.as_str()
            {
                *n = json!(fix_eft(text));
            }
        }
        let (checks, lines, txs) = self
            .book
            .read(|q| -> fb_core::FbResult<_> {
                let mut st = q.prepare("SELECT (SELECT account_id FROM checks_checkline l WHERE l.revision_id=v.id ORDER BY position LIMIT 1),v.vendor_id,v.payee_name,v.check_number,v.document_id FROM checks_checkrevision v")?;
                let checks = st
                    .query_map([], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?)))?
                    .collect::<Result<Vec<_>, _>>()?;
                let mut st = q.prepare("SELECT DISTINCT account_id,name FROM ledger_journalline")?;
                let lines = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?.collect::<Result<Vec<_>, _>>()?;
                let mut st = q.prepare("SELECT ordinal,return_count FROM bankfeeds_transaction")?;
                let txs = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?.collect::<Result<HashMap<_, _>, _>>()?;
                Ok((checks, lines, txs))
            })
            .unwrap();
        let mut intents: Sorted<&str, Vec<(String, String)>> = Sorted::new();
        intents.insert("DEPOSIT", vec![(String::new(), String::new())]);
        intents.insert("TRANSFER", vec![(String::new(), String::new())]);
        let none = "None".to_owned();
        let label_of = |m: &HashMap<String, String>, id: &Option<String>| id.as_ref().map(|i| m.get(i).cloned().unwrap_or_else(|| "?".into())).unwrap_or_else(|| none.clone());
        intents.insert(
            "CHECK",
            checks
                .iter()
                .flat_map(|(category, vendor, payee, number, doc)| {
                    let payees = if vendor.is_some() { vec![payee.clone(), String::new()] } else { vec![payee.clone()] };
                    payees.into_iter().map(move |payee| (category, vendor, payee, number, doc))
                })
                .map(|(category, vendor, payee, number, doc)| {
                    let label = if generated.contains(&(doc.clone(), number.clone())) { fix_eft(number) } else { number.clone() };
                    (
                        format!("{}:{}:{payee}:{number}", category.clone().unwrap_or_else(|| none.clone()), vendor.clone().unwrap_or_else(|| none.clone())),
                        format!("{}:{}:{payee}:{label}", label_of(&accounts, category), label_of(&vendors, vendor)),
                    )
                })
                .collect(),
        );
        intents.insert(
            "JOURNAL",
            lines.iter().map(|(account, name)| (format!("{account}:{name}"), format!("{}:{name}", accounts.get(account).cloned().unwrap_or_else(|| "?".into())))).collect(),
        );
        let ns = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, b"https://freedombooks.invalid/bankfeeds");
        let mut keys: HashMap<String, String> = HashMap::new();
        for (ordinal, name) in &self.bf.tx_ordinals {
            for (kind, options) in &intents {
                for rc in 0..=txs.get(ordinal).copied().unwrap_or(0) {
                    for (raw, label) in options {
                        let key = uuid::Uuid::new_v5(&ns, format!("{kind}:{ordinal}:{rc}:{raw}").as_bytes()).to_string();
                        keys.insert(key, format!("bankfeed:{kind}:{name}:{rc}:{label}"));
                    }
                }
            }
        }
        fn walk(v: Value, key: &dyn Fn(&str) -> String) -> Value {
            match v {
                Value::Object(m) => Value::Object(
                    m.into_iter()
                        .map(|(k, x)| {
                            let x = match (k.as_str(), x) {
                                ("request_key", Value::String(s)) => Value::String(key(&s)),
                                (_, x) => walk(x, key),
                            };
                            (k, x)
                        })
                        .collect(),
                ),
                Value::Array(a) => Value::Array(a.into_iter().map(|x| walk(x, key)).collect()),
                other => other,
            }
        }
        walk(out, &|s: &str| keys.get(s).cloned().unwrap_or_else(|| s.to_owned()))
    }
}

/// Typed bank-feed audit identifiers; every non-ID field stays exact.
pub(super) fn normalize_audit(v: &mut Value, ids: &audit::Ids) -> bool {
    let Some((kind, key)) = v["target"].as_str().and_then(|t| t.split_once(':')).map(|(a, b)| (a.to_owned(), b.to_owned())) else { return false };
    if !["bankfeedconnection", "bankfeedaccount", "bankfeedtransaction", "bankfeedrule"].contains(&kind.as_str()) {
        return false;
    }
    let mapped = |x: &Value, k: &str| -> Value {
        if x.is_null() {
            return Value::Null;
        }
        let raw = x.as_str().map(str::to_owned).unwrap_or_else(|| x.to_string());
        json!(ids.get(k).and_then(|m| m.get(&raw)).cloned().unwrap_or_else(|| format!("<unknown:{k}:{raw}>")))
    };
    v["target"] = json!(format!("{kind}:{}", mapped(&json!(key), &kind).as_str().unwrap()));
    let d = &mut v["detail"];
    for field in ["before", "after"] {
        if let Some(x) = d.get(field).cloned() {
            d[field] = mapped(&x, "bf_account");
        }
    }
    if let Some(x) = d.get("journal_id").cloned() {
        d["journal_id"] = mapped(&x, "bf_journal");
    }
    if let Some(x) = d.get("previous").and_then(|p| p.get("journal_id")).cloned() {
        d["previous"]["journal_id"] = mapped(&x, "bf_journal");
    }
    true
}
