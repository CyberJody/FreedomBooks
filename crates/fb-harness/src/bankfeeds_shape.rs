//! Strict bank-feed dump contract: exact field shapes, syntax, references into the ledger dump, and
//! completeness against the audit trail (independent evidence), even when both runners agree.
use crate::dump_shape::Shape::{self, *};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) const SHAPE: Shape = Object(&[
    (
        "connections",
        Array(&Object(&[
            ("id", String),
            ("company_id", String),
            ("label", String),
            ("host", String),
            ("active", Bool),
            ("forgotten", Bool),
            ("created_by", String),
            ("last_sync_at", Nullable(&Int)),
            ("last_sync_ok", Nullable(&Bool)),
            ("last_sync_summary", String),
            ("sync_count", Int),
        ])),
    ),
    (
        "accounts",
        Array(&Object(&[
            ("id", String),
            ("connection_id", String),
            ("conn_key", String),
            ("remote_id", String),
            ("name", String),
            ("org_name", String),
            ("currency", String),
            ("balance", Nullable(&Int)),
            ("available_balance", Nullable(&Int)),
            ("balance_at", Nullable(&Int)),
            ("ledger_account_id", Nullable(&String)),
            ("mapped", Bool),
            ("last_seen_at", Nullable(&Int)),
            ("active", Bool),
        ])),
    ),
    (
        "transactions",
        Array(&Object(&[
            ("id", String),
            ("feed_account_id", String),
            ("remote_id", String),
            ("posted_on", Nullable(&String)),
            ("transacted_on", Nullable(&String)),
            ("amount", Int),
            ("description", String),
            ("pending", Bool),
            ("status", String),
            ("accepted_as", String),
            ("journal_id", Nullable(&String)),
            ("journal_line_id", Nullable(&String)),
            ("reference", String),
            ("reference_url", String),
            ("decided", Bool),
            ("decided_by", Nullable(&String)),
            ("return_count", Int),
        ])),
    ),
    (
        "rules",
        Array(&Object(&[
            ("id", String),
            ("company_id", String),
            ("position", Int),
            ("name", String),
            ("match_text", String),
            ("sign", String),
            ("amount_min", Nullable(&Int)),
            ("amount_max", Nullable(&Int)),
            ("account_id", String),
            ("payee_name", String),
            ("active", Bool),
        ])),
    ),
    ("results", Map(&Detail)),
]);

fn rows<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v[key].as_array().map(Vec::as_slice).unwrap_or(&[])
}
fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}
fn bad(why: &str) -> std::string::String {
    format!("/bankfeeds: {why}")
}
fn is_date(v: &Value) -> bool {
    v.is_null() || v.as_str().is_some_and(|d| fb_db::dates::parse_iso_date(d).is_some_and(|p| p == d))
}
fn named(s: &str) -> bool {
    !s.is_empty() && s != "?" && !s.contains("<unknown")
}
/// Python `str(Decimal)` money text from the audit, as cents.
fn audit_cents(v: &Value) -> Result<Option<i64>, std::string::String> {
    let Some(t) = v.as_str() else { return if v.is_null() { Ok(None) } else { Err(bad("audit money is not text")) } };
    let (neg, d) = t.strip_prefix('-').map_or((false, t), |d| (true, d));
    let (w, f) = d.split_once('.').ok_or_else(|| bad("audit money needs two places"))?;
    if f.len() != 2 || w.is_empty() || !w.bytes().chain(f.bytes()).all(|b| b.is_ascii_digit()) {
        return Err(bad("audit money syntax"));
    }
    let n: i64 = format!("{w}{f}").parse().map_err(|_| bad("audit money range"))?;
    Ok(Some(if neg { -n } else { n }))
}

pub(crate) fn validate(dump: &Value) -> Result<(), std::string::String> {
    let v = &dump["bankfeeds"];
    let companies: BTreeSet<&str> = rows(dump, "companies").iter().map(|c| text(c, "ref")).collect();
    let accounts: BTreeMap<&str, &Value> = rows(dump, "accounts").iter().map(|a| (text(a, "ref"), a)).collect();
    let journals: BTreeMap<&str, &Value> = rows(dump, "journals").iter().map(|j| (text(j, "ref"), j)).collect();
    // events by target, per company, in audit order
    let mut events: BTreeMap<(&str, &str), Vec<&Value>> = BTreeMap::new();
    for (company, list) in dump["audit"].as_object().ok_or_else(|| bad("missing audit evidence"))? {
        for e in list.as_array().into_iter().flatten() {
            if text(e, "action").starts_with("bankfeed.") {
                if !text(e, "target").starts_with("bankfeed") {
                    return Err(bad("bank-feed audit with a foreign target"));
                }
                events.entry((company.as_str(), text(e, "target"))).or_default().push(e);
            }
        }
    }
    let of = |company: &str, target: std::string::String| events.get(&(company, target.as_str())).cloned().unwrap_or_default();
    let mut ids = BTreeSet::new();
    for key in ["connections", "accounts", "transactions", "rules"] {
        for row in rows(v, key) {
            if !named(text(row, "id")) || !ids.insert(format!("{key}:{}", text(row, "id"))) {
                return Err(bad(&format!("{key} row without a unique natural key")));
            }
        }
    }
    let mut conn_company = BTreeMap::new();
    let mut explained = BTreeSet::new();
    for c in rows(v, "connections") {
        let (id, company) = (text(c, "id"), text(c, "company_id"));
        if !companies.contains(company) || !named(text(c, "created_by")) || text(c, "label").is_empty() || text(c, "host").is_empty() {
            return Err(bad("connection references"));
        }
        conn_company.insert(id, company);
        let target = format!("bankfeedconnection:{id}");
        explained.insert((company, target.clone()));
        let evs = of(company, target);
        let count = |a: &str| evs.iter().filter(|e| e["action"] == a).count();
        let connected: Vec<_> = evs.iter().filter(|e| e["action"] == "bankfeed.connected").collect();
        if connected.len() != 1 || connected[0]["detail"]["label"] != c["label"] || connected[0]["detail"]["host"] != c["host"] {
            return Err(bad("connection without exactly one matching connected audit"));
        }
        if c["active"] == c["forgotten"] || count("bankfeed.forgotten") != usize::from(c["active"] == false) {
            return Err(bad("forgotten state disagrees with the audit"));
        }
        let synced: Vec<_> = evs.iter().filter(|e| e["action"] == "bankfeed.synced").collect();
        if c["sync_count"].as_u64() != Some(synced.len() as u64) || c["last_sync_at"].is_null() != synced.is_empty() || c["last_sync_ok"].is_null() != synced.is_empty() {
            return Err(bad("sync count disagrees with the audit"));
        }
        if let Some(last) = synced.last()
            && (last["detail"]["summary"] != c["last_sync_summary"] || last["detail"]["ok"] != c["last_sync_ok"])
        {
            return Err(bad("last sync disagrees with the audit"));
        }
        if evs.iter().any(|e| !["bankfeed.connected", "bankfeed.forgotten", "bankfeed.synced", "bankfeed.staged"].contains(&text(e, "action"))) {
            return Err(bad("unexpected connection audit"));
        }
        // rows the pulls and bridge stages created, less the pending rows the pulls withdrew (checked: the
        // counts are stored evidence, and a malformed count is a failure, never a panic)
        let counted = |e: &Value, k: &str| audit_count(&e["detail"][k]).ok_or_else(|| bad("bank-feed audit without valid counts"));
        let mut expected: u64 = 0;
        for e in synced.iter().filter(|e| e["detail"]["ok"] == true) {
            expected = expected.checked_add(counted(e, "new")?).and_then(|n| n.checked_sub(counted(e, "pending_dropped").ok()?)).ok_or_else(|| bad("sync audit counts out of range"))?;
        }
        for e in evs.iter().filter(|e| e["action"] == "bankfeed.staged") {
            expected = expected.checked_add(counted(e, "new")?).ok_or_else(|| bad("staging audit counts out of range"))?;
        }
        let actual = rows(v, "transactions").iter().filter(|t| text(t, "feed_account_id").split('/').next() == Some(id)).count() as u64;
        if expected != actual {
            return Err(bad("downloaded rows disagree with the sync audit counts"));
        }
    }
    let mut feeds = BTreeMap::new();
    for f in rows(v, "accounts") {
        let conn = text(f, "connection_id");
        let company = *conn_company.get(conn).ok_or_else(|| bad("feed account of an unknown connection"))?;
        let natural = text(f, "id") == format!("{conn}/{}/{}", text(f, "conn_key"), text(f, "remote_id"));
        let aliased = text(f, "id").strip_prefix(&format!("{conn}/~/")).is_some_and(alias_ref) && provider_redacted(text(f, "remote_id"));
        if !(natural || aliased) || text(f, "remote_id").is_empty() || text(f, "currency").is_empty() {
            return Err(bad("feed account key syntax"));
        }
        let mapped = f["ledger_account_id"].as_str();
        if f["mapped"] != mapped.is_some() || mapped.is_some_and(|a| accounts.get(a).is_none_or(|acc| acc["company"] != company)) {
            return Err(bad("feed mapping references"));
        }
        let target = format!("bankfeedaccount:{}", text(f, "id"));
        explained.insert((company, target.clone()));
        let evs = of(company, target);
        if evs.iter().any(|e| e["action"] != "bankfeed.mapped") || evs.last().map_or(Value::Null, |e| e["detail"]["after"].clone()) != f["ledger_account_id"] {
            return Err(bad("feed mapping disagrees with the audit"));
        }
        feeds.insert(text(f, "id"), (company, f));
    }
    for t in rows(v, "transactions") {
        let (company, _) = *feeds.get(text(t, "feed_account_id")).ok_or_else(|| bad("transaction of an unknown feed account"))?;
        let status = text(t, "status");
        let kind = text(t, "accepted_as");
        let decided = t["journal_id"].is_string();
        let shape = match status {
            "NEW" | "IGNORED" => kind.is_empty() && !decided && t["journal_line_id"].is_null(),
            "MATCHED" => kind == "MATCH" && decided && t["journal_line_id"].is_string(),
            "ACCEPTED" => ["CHECK", "DEPOSIT", "TRANSFER", "JOURNAL"].contains(&kind) && decided && t["journal_line_id"].is_string(),
            _ => false,
        };
        if !shape
            || !(text(t, "id") == format!("{}/{}", text(t, "feed_account_id"), text(t, "remote_id"))
                || (text(t, "id").strip_prefix(&format!("{}/", text(t, "feed_account_id"))).is_some_and(alias_ref) && provider_redacted(text(t, "remote_id"))))
            || !is_date(&t["posted_on"])
            || !is_date(&t["transacted_on"])
            || t["decided"] != (status != "NEW")
            || t["decided_by"].is_null() == (status != "NEW")
            || t["reference"].as_str().is_some_and(str::is_empty) == decided
            || t["reference_url"].as_str().is_some_and(str::is_empty) == decided
            || t["return_count"].as_i64().is_none_or(|n| n < 0)
            || t["amount"].as_i64().is_none_or(|a| !(-9_999_999_999_999_999..=9_999_999_999_999_999).contains(&a))
        {
            return Err(bad("transaction decision shape or syntax"));
        }
        if decided {
            let journal = journals.get(text(t, "journal_id")).ok_or_else(|| bad("decided transaction names an unknown journal"))?;
            let (jref, position) = text(t, "journal_line_id").rsplit_once(':').ok_or_else(|| bad("line reference syntax"))?;
            let line = rows(journal, "lines").iter().find(|l| l["position"].as_u64().map(|n| n.to_string()).as_deref() == Some(position));
            let amount = t["amount"].as_i64().unwrap_or(0);
            let (debit, credit) = if amount > 0 { (amount, 0) } else { (0, -amount) };
            if jref != text(t, "journal_id") || journal["company"] != company || journal["status"] != "POSTED" || line.is_none_or(|l| l["debit"] != debit || l["credit"] != credit) {
                return Err(bad("decided transaction line disagrees with the ledger"));
            }
        }
        let target = format!("bankfeedtransaction:{}", text(t, "id"));
        explained.insert((company, target.clone()));
        let (mut derived, mut returns, mut last) = ("NEW", 0, None);
        for e in of(company, target) {
            match text(e, "action") {
                "bankfeed.accepted" => {
                    derived = if e["detail"]["status"] == "MATCHED" { "MATCHED" } else { "ACCEPTED" };
                    last = Some(e);
                }
                "bankfeed.ignored" => derived = "IGNORED",
                "bankfeed.returned" => {
                    derived = "NEW";
                    returns += 1;
                }
                _ => return Err(bad("unexpected transaction audit")),
            }
            audit_cents(&e["detail"]["amount"])?; // undecided rows may be re-pulled with new values later
        }
        if derived != status || t["return_count"].as_i64() != Some(returns) {
            return Err(bad("transaction status disagrees with the audit"));
        }
        if decided {
            let d = &last.ok_or_else(|| bad("decision without audit"))?["detail"];
            if audit_cents(&d["amount"])? != t["amount"].as_i64() || d["description"] != t["description"] || d["kind"] != t["accepted_as"] || d["journal_id"] != t["journal_id"] || d["reference"] != t["reference"] || d["status"] != t["status"] || d["date"] != t["posted_on"].as_str().map_or(t["transacted_on"].clone(), Value::from) {
                return Err(bad("transaction decision disagrees with the audit"));
            }
        }
    }
    let mut positions = BTreeSet::new();
    let mut per_company: BTreeMap<&str, usize> = BTreeMap::new();
    for r in rows(v, "rules") {
        let company = text(r, "company_id");
        let (min, max) = (r["amount_min"].as_i64(), r["amount_max"].as_i64());
        if !companies.contains(company)
            || !["ANY", "IN", "OUT"].contains(&text(r, "sign"))
            || r["position"].as_i64().is_none_or(|p| p < 1)
            || !positions.insert((company, r["position"].to_string()))
            || min.is_some_and(|n| n < 0)
            || max.is_some_and(|n| n < 0)
            || min.zip(max).is_some_and(|(a, b)| a > b)
            || text(r, "match_text").trim().is_empty()
            || text(r, "name").trim().is_empty()
            || accounts.get(text(r, "account_id")).is_none_or(|a| a["company"] != company)
        {
            return Err(bad("rule syntax or references"));
        }
        *per_company.entry(company).or_default() += 1;
        let target = format!("bankfeedrule:{}", text(r, "id"));
        explained.insert((company, target.clone()));
        let evs = of(company, target);
        if evs.first().is_none_or(|e| e["action"] != "bankfeed.rule_created") || evs.iter().filter(|e| e["action"] == "bankfeed.rule_created").count() != 1 {
            return Err(bad("rule without exactly one created audit"));
        }
        let mut state = serde_json::Map::new();
        for e in &evs {
            match text(e, "action") {
                "bankfeed.rule_created" | "bankfeed.rule_edited" | "bankfeed.rule_moved" => {
                    for (k, x) in e["detail"].as_object().into_iter().flatten() {
                        state.insert(k.clone(), x.clone());
                    }
                }
                _ => return Err(bad("unexpected rule audit")),
            }
        }
        for field in ["name", "match_text", "sign", "account_id", "payee_name", "active"] {
            if state.get(field) != Some(&r[field]) {
                return Err(bad("rule disagrees with the audit"));
            }
        }
        for field in ["amount_min", "amount_max"] {
            if audit_cents(state.get(field).unwrap_or(&Value::Null))? != r[field].as_i64() {
                return Err(bad("rule amount disagrees with the audit"));
            }
        }
    }
    if per_company.values().any(|n| *n > 200) {
        return Err(bad("more than 200 rules in a company"));
    }
    let replayed_withdrawn = manifest_checks(dump)?;
    // every bank-feed audit target is a dumped row, except a withdrawn pending item that ended to review
    for ((company, target), evs) in &events {
        if explained.contains(&(*company, target.to_string())) {
            continue;
        }
        // FIX4: and the replay withdrew it; any other missing row is a failure.
        let withdrawn = target.strip_prefix("bankfeedtransaction:").is_some_and(|t| named(t) && replayed_withdrawn.contains(&(company.to_string(), t.to_owned())))
            && evs.last().is_some_and(|e| e["action"] == "bankfeed.returned");
        if !withdrawn {
            return Err(bad("bank-feed audit for a row missing from the dump"));
        }
    }
    Ok(())
}

/// What the runner's own step record says each bank-feed step named (see `bankfeeds::manifest`). Only
/// successful steps count: a refused step created nothing and stored no result.
/// A successful query step: its op, manifest, evidence and step number.
struct Query<'a> {
    op: &'a str,
    step: &'a Value,
    evidence: &'a Value,
    index: u64,
}
/// One successful pull: its step, company, feeds, listed transactions and those listed as pending.
struct Pull<'a> {
    index: u64,
    company: &'a str,
    feeds: Vec<&'a str>,
    transactions: Vec<&'a str>,
    pending: Vec<&'a str>,
}
#[derive(Default)]
struct Manifest<'a> {
    results: BTreeMap<&'a str, Query<'a>>,
    connections: BTreeSet<&'a str>,
    rules: BTreeSet<&'a str>,
    feeds: BTreeSet<&'a str>,
    transactions: BTreeSet<&'a str>,
    /// Every successful pull (sync or store), in step order: the source rows the review-list inventory
    /// is replayed from.
    pulls: Vec<Pull<'a>>,
    /// (step, company, account ref) for every account a successful `save_account` created.
    created: Vec<(u64, &'a str, &'a str)>,
    /// FIX5: aliased transaction name -> its id as the runner's sync audit writes it (manifest `aliases`).
    aliases: BTreeMap<&'a str, &'a str>,
}
fn strings(v: &Value) -> Option<Vec<&str>> {
    v.as_array()?.iter().map(Value::as_str).collect()
}
fn manifest(dump: &Value) -> Result<Manifest<'_>, std::string::String> {
    let mut m = Manifest::default();
    let outcomes = rows(dump, "outcomes");
    let any_feed_step = outcomes.iter().any(|o| text(o, "op").starts_with("bankfeeds_"));
    for o in outcomes {
        let op = text(o, "op");
        let index = o["step"].as_u64().ok_or_else(|| bad("outcome step syntax"))?;
        let query = matches!(op, "bankfeeds_mappable" | "bankfeeds_rule_accounts" | "bankfeeds_items");
        if o.get("evidence").is_some() != (query && o["ok"] == true) {
            return Err(bad("query-list evidence belongs to exactly the successful query steps"));
        }
        if op == "save_account" && any_feed_step {
            // Accounts a query list may draw from: required wherever bank-feed steps run.
            let step = o.get("manifest").ok_or_else(|| bad("account step without a manifest"))?;
            if !exact_keys(step, &["company", "created"]) || !step["company"].is_string() || !(step["created"].is_null() || step["created"].is_string()) {
                return Err(bad("account manifest syntax"));
            }
            if o["ok"] == true
                && let Some(r) = step["created"].as_str()
            {
                m.created.push((index, text(step, "company"), r));
            }
            continue;
        }
        if !op.starts_with("bankfeeds_") {
            continue;
        }
        let step = o.get("manifest").filter(|s| s.is_object()).ok_or_else(|| bad("bank-feed step without a manifest"))?;
        let keys: BTreeSet<&str> = step.as_object().into_iter().flatten().map(|(k, _)| k.as_str()).collect();
        let expected: BTreeSet<&str> = match op {
            "bankfeeds_connect" => ["company", "result", "connection"].into(),
            "bankfeeds_sync" | "bankfeeds_store" if step.get("aliases").is_some() => ["company", "result", "feeds", "transactions", "pending", "aliases"].into(),
            "bankfeeds_sync" | "bankfeeds_store" => ["company", "result", "feeds", "transactions", "pending"].into(),
            "bankfeeds_save_rule" if step.get("rule").is_some() => ["company", "result", "rule"].into(),
            _ => ["company", "result"].into(),
        };
        let optional_text = |k: &str| step.get(k).is_none_or(|v| v.is_null() || v.is_string());
        if keys != expected || !step["company"].is_string() || !optional_text("result") || !optional_text("connection") || !optional_text("rule") {
            return Err(bad("bank-feed manifest syntax"));
        }
        let (feeds, txs, pending) = match (strings(&step["feeds"]), strings(&step["transactions"]), strings(&step["pending"])) {
            (Some(f), Some(t), Some(p)) if p.iter().all(|x| t.contains(x)) => (f, t, p),
            _ if expected.contains("feeds") => return Err(bad("bank-feed manifest syntax")),
            _ => (vec![], vec![], vec![]),
        };
        let aliases: Option<BTreeMap<&str, &str>> = match step.get("aliases") {
            None => Some(BTreeMap::new()),
            Some(a) => a.as_object().filter(|a| !a.is_empty()).and_then(|a| a.iter().map(|(k, v)| Some((k.as_str(), v.as_str()?))).collect()),
        };
        let Some(aliases) = aliases.filter(|a| a.keys().all(|k| txs.contains(k))) else {
            return Err(bad("bank-feed manifest syntax"));
        };
        if o["ok"] != true {
            continue;
        }
        m.aliases.extend(aliases);
        if expected.contains("feeds") {
            m.pulls.push(Pull { index, company: text(step, "company"), feeds: feeds.clone(), transactions: txs.clone(), pending });
        }
        if let Some(r) = step["result"].as_str() {
            m.results.insert(r, Query { op, step, evidence: &o["evidence"], index });
        }
        m.connections.extend(step["connection"].as_str());
        m.rules.extend(step["rule"].as_str());
        m.feeds.extend(feeds);
        m.transactions.extend(txs);
    }
    Ok(m)
}
fn exact_keys(v: &Value, keys: &[&str]) -> bool {
    v.as_object().is_some_and(|o| o.len() == keys.len() && keys.iter().all(|k| o.contains_key(*k)))
}
fn audit_count(v: &Value) -> Option<u64> {
    v.as_u64().filter(|n| *n <= i64::MAX as u64)
}
/// What the replay knows after a company's pulls: the rows present, and every row a pull withdrew.
struct Replay<'a> {
    present: BTreeSet<&'a str>,
    withdrawn: BTreeSet<&'a str>,
}
/// The company's transaction inventory just before step `at`, replayed over time from the source rows
/// (successful pulls, in step order) and the audit (FIX3; FIX2 exempted every row ever listed as pending,
/// so any of them could be dropped from a review list and its evidence). The k-th successful pull of a
/// company wrote the k-th `bankfeed.synced` event with `ok: true` of that company (a failed pull writes
/// `ok: false`; nothing else writes `synced`). A pull adds every transaction it lists, and withdraws the
/// rows its event names in `pending_dropped_ids`. The service withdraws feed by feed in the pull's order,
/// and within a feed in ordinal (first insertion) order, so that list is the concatenation, over the
/// pull's feeds, of a subsequence of each feed's candidates: present rows whose last listing was
/// pending, in a feed of the event's connection, that this pull does not list again. Remote ids are
/// unique within a feed, so [`withdrawals`] finds every such alignment.
///
/// FIX4: the replay is exact or it fails. Evidence that does not line up is a comparer failure, never
/// permission to treat rows as optional (FIX3 made every candidate "uncertain" when no alignment existed,
/// so a forged withdrawal id let a live row vanish from a review list and its evidence). Each of these
/// is an error: pulls and events that do not pair up, an event naming fewer ids than it counts (the
/// service audits at most 50; no scenario withdraws more in one pull, and one that does needs fuller
/// evidence first), a named id no alignment explains, and ids that align more than one way with
/// different rows withdrawn.
fn inventory<'a>(dump: &'a Value, m: &Manifest<'a>, company: &str, at: u64) -> Result<Replay<'a>, std::string::String> {
    let events: Vec<&Value> = dump["audit"][company]
        .as_array()
        .ok_or_else(|| bad("missing audit evidence"))?
        .iter()
        .filter(|e| text(e, "action") == "bankfeed.synced" && e["detail"]["ok"] == true)
        .collect();
    let pulls: Vec<&Pull<'a>> = m.pulls.iter().filter(|p| p.company == company).collect();
    if events.len() != pulls.len() {
        return Err(bad("successful pulls and their sync audits do not pair up"));
    }
    let mut replay = Replay { present: BTreeSet::new(), withdrawn: BTreeSet::new() };
    // Per transaction: last listing pending, feed, first-insertion sequence (the ordinal's order).
    let mut last_pending: BTreeMap<&str, bool> = BTreeMap::new();
    let mut feed_of: BTreeMap<&str, &str> = BTreeMap::new();
    let mut inserted: BTreeMap<&str, usize> = BTreeMap::new();
    let mut next = 0usize;
    for (pull, event) in pulls.into_iter().zip(events) {
        if pull.index >= at {
            break;
        }
        let connection = text(event, "target").strip_prefix("bankfeedconnection:").ok_or_else(|| bad("sync audit target syntax"))?;
        let named: Vec<std::string::String> = event["detail"]["pending_dropped_ids"]
            .as_array()
            .and_then(|a| a.iter().map(|v| v.as_str().map(str::to_owned)).collect::<Option<_>>())
            .ok_or_else(|| bad("sync audit withdrawn ids syntax"))?;
        let count = audit_count(&event["detail"]["pending_dropped"]).ok_or_else(|| bad("bank-feed audit without valid counts"))?;
        if named.len() as u64 != count {
            return Err(bad("a sync audit names a different number of withdrawn rows than it counts"));
        }
        let remote = |t: &str| match m.aliases.get(t) {
            Some(audited) => feed_of.get(t).map(|_| (*audited).to_owned()),
            None => feed_of.get(t).and_then(|f| t.strip_prefix(*f)).and_then(|r| r.strip_prefix('/')).map(|r| fb_core::bankfeeds::text::sanitize(r, 200)),
        };
        let mut per_feed: Vec<Vec<(&str, std::string::String)>> = vec![];
        for f in pull.feeds.iter().filter(|f| f.strip_prefix(connection).is_some_and(|r| r.starts_with('/'))) {
            let mut c: Vec<&str> = replay.present.iter().copied().filter(|t| feed_of.get(t) == Some(f) && last_pending.get(t) == Some(&true) && !pull.transactions.contains(t)).collect();
            c.sort_by_key(|t| inserted.get(t).copied().unwrap_or(usize::MAX));
            per_feed.push(c.into_iter().map(|t| remote(t).map(|r| (t, r)).ok_or_else(|| bad("withdrawal candidate without a feed"))).collect::<Result<_, _>>()?);
        }
        let gone = withdrawals(&per_feed, &named)?;
        for t in gone {
            replay.present.remove(t);
            replay.withdrawn.insert(t);
        }
        for t in &pull.transactions {
            if !replay.present.contains(t) {
                inserted.insert(*t, next);
                next += 1;
            }
            replay.present.insert(*t);
            last_pending.insert(*t, pull.pending.contains(t));
            if let Some(f) = pull.feeds.iter().filter(|f| t.strip_prefix(**f).is_some_and(|r| r.starts_with('/'))).max_by_key(|f| f.len()) {
                feed_of.insert(*t, f);
            }
        }
    }
    Ok(replay)
}
/// The rows `named` withdraws: `named` splits into consecutive runs, one per feed in order, each run a
/// subsequence of that feed's candidate remote ids (in order). Fails when no split exists (an id no live
/// candidate explains) or when the splits disagree on which rows were withdrawn (FIX4).
fn withdrawals<'a>(per_feed: &[Vec<(&'a str, std::string::String)>], named: &[std::string::String]) -> Result<BTreeSet<&'a str>, std::string::String> {
    fn walk<'a>(feeds: &[Vec<(&'a str, std::string::String)>], named: &[std::string::String], chosen: &mut Vec<&'a str>, ways: &mut Vec<BTreeSet<&'a str>>) {
        let Some((feed, rest)) = feeds.split_first() else {
            if named.is_empty() {
                ways.push(chosen.iter().copied().collect());
            }
            return;
        };
        // Take the longest run first; remote ids are distinct within a feed, so each length has at most
        // one embedding (greedy in order).
        for len in (0..=named.len().min(feed.len())).rev() {
            let mut picked = vec![];
            let mut at = 0;
            for id in &named[..len] {
                match feed[at..].iter().position(|(_, r)| r == id) {
                    Some(k) => {
                        picked.push(feed[at + k].0);
                        at += k + 1;
                    }
                    None => break,
                }
            }
            if picked.len() == len {
                let mark = chosen.len();
                chosen.extend(picked);
                walk(rest, &named[len..], chosen, ways);
                chosen.truncate(mark);
            }
        }
    }
    let mut ways = vec![];
    walk(per_feed, named, &mut vec![], &mut ways);
    let first = ways.first().ok_or_else(|| bad("a sync audit names a withdrawn row no live pending candidate explains"))?;
    if ways.iter().any(|w| w != first) {
        return Err(bad("a sync audit's withdrawn ids fit more than one set of rows"));
    }
    Ok(first.clone())
}

/// A query list's members and order, recomputed from the step's evidence (raw rows each runner read from
/// its tables), with that evidence bounded by the same dump and the scenario manifests: the accounts are
/// accounts of the company, and include every account a successful step created before this one; the
/// transactions are exactly the company's inventory at that step ([`inventory`]).
fn list_ok(dump: &Value, m: &Manifest<'_>, q: &Query<'_>, v: &Value) -> bool {
    let company = text(q.step, "company");
    let Some(list) = strings(&v[if q.op == "bankfeeds_items" { "items" } else { "accounts" }]) else { return false };
    if q.op == "bankfeeds_items" {
        let Some(rows) = q.evidence["transactions"].as_array().filter(|_| exact_keys(q.evidence, &["transactions"])) else { return false };
        let row_ok = |r: &Value| exact_keys(r, &["tx", "posted_on", "seq"]) && named(text(r, "tx")) && is_date(&r["posted_on"]) && r["seq"].as_u64().is_some_and(|n| n >= 1);
        if !rows.iter().all(row_ok) {
            return false;
        }
        let names: BTreeSet<&str> = rows.iter().map(|r| text(r, "tx")).collect();
        let seqs: BTreeSet<u64> = rows.iter().filter_map(|r| r["seq"].as_u64()).collect();
        let Ok(replay) = inventory(dump, m, company, q.index) else { return false };
        if names.len() != rows.len() || seqs.len() != rows.len() || names != replay.present {
            return false;
        }
        // views.review: order_by('-posted_on', '-pk'); PostgreSQL DESC puts undated rows first.
        let mut expected: Vec<&Value> = rows.iter().collect();
        expected.sort_by(|a, b| (b["posted_on"].is_null(), b["posted_on"].as_str(), b["seq"].as_u64()).cmp(&(a["posted_on"].is_null(), a["posted_on"].as_str(), a["seq"].as_u64())));
        return list == expected.iter().map(|r| text(r, "tx")).collect::<Vec<_>>();
    }
    let Some(rows) = q.evidence["accounts"].as_array().filter(|_| exact_keys(q.evidence, &["accounts"])) else { return false };
    let account_of = |r: &str| rows_of(dump, "accounts").iter().any(|a| text(a, "ref") == r && a["company"] == company);
    let row_ok = |r: &Value| {
        exact_keys(r, &["account", "code", "kind", "active", "postable", "control"])
            && account_of(text(r, "account"))
            && r["code"].is_string()
            && r["kind"].is_string()
            && r["active"].is_boolean()
            && r["postable"].is_boolean()
            && r["control"].is_boolean()
    };
    if !rows.iter().all(row_ok) {
        return false;
    }
    let refs: BTreeSet<&str> = rows.iter().map(|r| text(r, "account")).collect();
    let codes: BTreeSet<&str> = rows.iter().map(|r| text(r, "code")).collect();
    let created = m.created.iter().filter(|(i, c, _)| *i < q.index && *c == company).map(|(_, _, r)| *r);
    if refs.len() != rows.len() || codes.len() != rows.len() || !created.into_iter().all(|r| refs.contains(r)) {
        return false;
    }
    // services.mappable_accounts / rule_accounts: active postable non-control (ASSET/LIABILITY), by code.
    let mut expected: Vec<&Value> = rows
        .iter()
        .filter(|r| r["active"] == true && r["postable"] == true && r["control"] == false)
        .filter(|r| q.op == "bankfeeds_rule_accounts" || matches!(text(r, "kind"), "ASSET" | "LIABILITY"))
        .collect();
    expected.sort_by_key(|r| text(r, "code"));
    list == expected.iter().map(|r| text(r, "account")).collect::<Vec<_>>()
}
fn rows_of<'a>(dump: &'a Value, key: &str) -> &'a [Value] {
    if key == "transactions" { rows(&dump["bankfeeds"], key) } else { rows(dump, key) }
}
/// Typed query results: the shape of each op's result, its references, and its counts against the step.
fn result_ok(dump: &Value, m: &Manifest<'_>, q: &Query<'_>, v: &Value) -> bool {
    let (op, step) = (q.op, q.step);
    let company = text(step, "company");
    let account_of = |r: &str| rows(dump, "accounts").iter().any(|a| text(a, "ref") == r && a["company"] == company);
    let rule_of = |r: &str| rows(&dump["bankfeeds"], "rules").iter().any(|x| text(x, "id") == r && x["company_id"] == company);
    let unique = |list: &[&str]| list.iter().collect::<BTreeSet<_>>().len() == list.len();
    match op {
        "bankfeeds_mappable" | "bankfeeds_rule_accounts" => {
            exact_keys(v, &["accounts"]) && strings(&v["accounts"]).is_some_and(|a| unique(&a) && a.iter().all(|r| account_of(r))) && list_ok(dump, m, q, v)
        }
        "bankfeeds_items" => exact_keys(v, &["items"]) && strings(&v["items"]).is_some_and(|a| unique(&a) && a.iter().all(|t| named(t))) && list_ok(dump, m, q, v),
        "bankfeeds_sync" | "bankfeeds_store" => {
            let fields = ["summary", "provider_errors", "reasons", "accounts", "new", "updated", "posted", "unchanged", "skipped", "errors", "pending_dropped"];
            let n = |k: &str| audit_count(&v[k]);
            let listed = |k: &str| step[k].as_array().map_or(0, Vec::len) as u64;
            let (errors, reasons) = (strings(&v["provider_errors"]), strings(&v["reasons"]));
            let upserts = [n("new"), n("updated"), n("posted"), n("unchanged")].into_iter().try_fold(0u64, |sum, x| sum.checked_add(x?));
            exact_keys(v, &fields)
                && v["summary"].is_string()
                && fields[3..].iter().all(|k| n(k).is_some())
                && n("accounts") == Some(listed("feeds"))
                && upserts == Some(listed("transactions"))
                && errors.is_some_and(|e| n("errors") == Some(e.len() as u64))
                && reasons.is_some_and(|r| n("skipped") == Some(r.len() as u64))
        }
        "bankfeeds_suggest" => {
            let kind = text(v, "kind");
            let (matches, receipts) = (v["matches"].as_array(), v["receipts"].as_array());
            let match_ok = |x: &Value| exact_keys(x, &["line", "journal", "label", "date"]) && named(text(x, "line")) && named(text(x, "journal")) && x["label"].is_string() && is_date(&x["date"]) && !x["date"].is_null();
            let receipt_ok = |x: &Value| exact_keys(x, &["receipt", "label", "date"]) && named(text(x, "receipt")) && x["label"].is_string() && is_date(&x["date"]) && !x["date"].is_null();
            let (Some(matches), Some(receipts)) = (matches, receipts) else { return false };
            let rule = v["rule"].as_str();
            let expected_kind = if !matches.is_empty() {
                kind == "MATCH"
            } else if !receipts.is_empty() {
                kind == "DEPOSIT"
            } else if rule.is_some() {
                kind == "CHECK" || kind == "JOURNAL"
            } else {
                kind.is_empty()
            };
            exact_keys(v, &["kind", "matches", "receipts", "rule", "account_id", "payee_name", "check_number"])
                && v["kind"].is_string()
                && expected_kind
                && matches.len() <= 3
                && receipts.len() <= 3
                && matches.iter().all(match_ok)
                && receipts.iter().all(receipt_ok)
                && (v["rule"].is_null() || rule.is_some_and(rule_of))
                && v["account_id"].is_null() == v["rule"].is_null()
                && (v["account_id"].is_null() || v["account_id"].as_str().is_some_and(account_of))
                && v["payee_name"].is_string()
                && v["check_number"].as_str().is_some_and(|c| !c.is_empty())
        }
        _ => false,
    }
}
/// Rule priority from the audit alone: creation appends, a move swaps with its neighbour and records the
/// moved rule's new position. Every company's dumped positions must equal that replay.
fn rule_positions(dump: &Value) -> Result<BTreeMap<(std::string::String, std::string::String), u64>, std::string::String> {
    let mut out = BTreeMap::new();
    for (company, list) in dump["audit"].as_object().ok_or_else(|| bad("missing audit evidence"))? {
        let mut order: Vec<&str> = vec![];
        for e in list.as_array().into_iter().flatten() {
            let Some(id) = text(e, "target").strip_prefix("bankfeedrule:") else { continue };
            match text(e, "action") {
                "bankfeed.rule_created" => order.push(id),
                "bankfeed.rule_moved" => {
                    let at = order.iter().position(|r| *r == id).ok_or_else(|| bad("rule moved before it was created"))?;
                    let other = match text(&e["detail"], "direction") {
                        "up" => at.checked_sub(1),
                        "down" => at.checked_add(1).filter(|o| *o < order.len()),
                        _ => None,
                    }
                    .ok_or_else(|| bad("rule move audit without a neighbour"))?;
                    order.swap(at, other);
                    if audit_count(&e["detail"]["position"]) != Some(other as u64 + 1) {
                        return Err(bad("rule move audit disagrees with the replayed order"));
                    }
                }
                _ => {}
            }
        }
        for (i, id) in order.into_iter().enumerate() {
            out.insert((company.clone(), id.to_owned()), i as u64 + 1);
        }
    }
    Ok(out)
}
/// Returns (company, transaction) for every row the full replay withdrew.
fn manifest_checks(dump: &Value) -> Result<BTreeSet<(std::string::String, std::string::String)>, std::string::String> {
    let v = &dump["bankfeeds"];
    let m = manifest(dump)?;
    // FIX4: the whole replay, not only the part before some review list, must line up, and its final
    // inventory is exactly each company's dumped rows (no scenario op stages outside a pull).
    let feed_company: BTreeMap<&str, &Value> = rows(v, "accounts")
        .iter()
        .filter_map(|f| rows(v, "connections").iter().find(|c| c["id"] == f["connection_id"]).map(|c| (text(f, "id"), &c["company_id"])))
        .collect();
    let mut withdrawn = BTreeSet::new();
    for company in m.pulls.iter().map(|p| p.company).collect::<BTreeSet<_>>() {
        let replay = inventory(dump, &m, company, u64::MAX)?;
        let dumped: BTreeSet<&str> = rows(v, "transactions").iter().filter(|t| feed_company.get(text(t, "feed_account_id")).is_some_and(|c| *c == company)).map(|t| text(t, "id")).collect();
        if dumped != replay.present {
            return Err(bad("downloaded rows disagree with the replay of the successful pulls and their withdrawals"));
        }
        withdrawn.extend(replay.withdrawn.into_iter().map(|t| (company.to_owned(), t.to_owned())));
    }
    let ids = |key: &str| -> BTreeSet<&str> { rows(v, key).iter().map(|r| text(r, "id")).collect() };
    if ids("connections") != m.connections {
        return Err(bad("connections disagree with the scenario's successful connects"));
    }
    if ids("rules") != m.rules {
        return Err(bad("rules disagree with the scenario's successful rule creations"));
    }
    if ids("accounts") != m.feeds {
        return Err(bad("downloaded accounts disagree with the accounts the successful pulls listed"));
    }
    if !ids("transactions").is_subset(&m.transactions) {
        return Err(bad("a downloaded transaction no successful pull listed"));
    }
    let results = v["results"].as_object().ok_or_else(|| bad("results syntax"))?;
    if results.keys().map(std::string::String::as_str).collect::<BTreeSet<_>>() != m.results.keys().copied().collect() {
        return Err(bad("query results disagree with the scenario's successful result steps"));
    }
    for (name, q) in &m.results {
        if !result_ok(dump, &m, q, &results[*name]) {
            return Err(bad(&format!("query result {name} has the wrong shape, references, counts or members for {}", q.op)));
        }
    }
    let positions = rule_positions(dump)?;
    for r in rows(v, "rules") {
        if positions.get(&(text(r, "company_id").to_owned(), text(r, "id").to_owned())).copied() != audit_count(&r["position"]) {
            return Err(bad("rule priority disagrees with the audit replay"));
        }
    }
    Ok(withdrawn)
}

/// Every bank-feed string a runner persisted or reported: bank-feed step messages (and their pins), the `bankfeeds`
/// section and `bankfeed.*` audit events (target and detail).
fn feed_strings(side: &mut Value, visit: &mut dyn FnMut(&str, &mut std::string::String)) {
    if side.get("checkpoints").is_some() {
        if let Some(checkpoints) = side.get_mut("checkpoints").and_then(Value::as_array_mut) {
            for checkpoint in checkpoints {
                if let Some(dump) = checkpoint.get_mut("dump") { feed_strings(dump, visit); }
            }
        }
        if let Some(final_book) = side.get_mut("final") { feed_strings(final_book, visit); }
        return;
    }
    fn walk(v: &mut Value, at: &str, visit: &mut dyn FnMut(&str, &mut std::string::String)) {
        match v {
            Value::String(s) => visit(at, s),
            Value::Array(a) => a.iter_mut().enumerate().for_each(|(i, x)| walk(x, &format!("{at}/{i}"), visit)),
            Value::Object(m) => m.iter_mut().for_each(|(k, x)| walk(x, &format!("{at}/{k}"), visit)),
            _ => {}
        }
    }
    if let Some(outcomes) = side.get_mut("outcomes").and_then(Value::as_array_mut) {
        for o in outcomes.iter_mut().filter(|o| text(o, "op").starts_with("bankfeeds_")) {
            // The message and its pin (FIX3): a pin is the v2 message, so the reference's leak is redacted
            // in both before the comparer holds the side to it.
            for field in ["message", "expect_message"] {
                let at = format!("/outcomes/step {}/{field}", o["step"]);
                if let Some(message) = o.get_mut(field) {
                    walk(message, &at, visit);
                }
            }
        }
    }
    if let Some(feeds) = side.get_mut("bankfeeds") {
        walk(feeds, "/bankfeeds", visit);
    }
    if let Some(audit) = side.get_mut("audit").and_then(Value::as_object_mut) {
        for (company, events) in audit.iter_mut() {
            for (i, e) in events.as_array_mut().into_iter().flatten().enumerate() {
                if text(e, "action").starts_with("bankfeed.") {
                    walk(e, &format!("/audit/{company}/{i}"), visit);
                }
            }
        }
    }
}
/// FIX5 provenance names: a provider id that carries URL userinfo (raw on the Python side, redacted to
/// `***` on the Rust side) may be named by its scenario `ref` instead of the id itself, so ids that differ
/// only in userinfo keep distinct names and no name carries a credential. Only such ids may be aliased.
fn alias_ref(r: &str) -> bool {
    !r.is_empty() && !r.contains('/') && !fb_core::bankfeeds::text::has_url_userinfo(r)
}
fn provider_redacted(remote: &str) -> bool {
    remote.contains("***@") || fb_core::bankfeeds::text::has_url_userinfo(remote)
}
/// Python bugs not copied (L4-BANKFEEDS.md): the frozen `redact` keeps URL userinfo glued to a word
/// character or under a non-http scheme; v2 removes it. The reference side gets v2's stricter rule
/// applied to those strings only, so every other byte still compares exactly.
pub(crate) fn redacted_reference(py: &Value) -> Value {
    let mut out = py.clone();
    feed_strings(&mut out, &mut |_, s| {
        if fb_core::bankfeeds::text::has_url_userinfo(s) {
            *s = fb_core::bankfeeds::text::strip_url_userinfo(s);
        }
    });
    out
}
/// Where a side still carries URL credentials in what it persisted or reported (must be nowhere).
pub(crate) fn kept_credentials(side: &Value) -> Vec<std::string::String> {
    let mut found = vec![];
    feed_strings(&mut side.clone(), &mut |at, s| {
        if fb_core::bankfeeds::text::has_url_userinfo(s) {
            found.push(at.to_owned());
        }
    });
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    /// FIX3: on every bank-feed scenario the replayed inventory equals every review list's members
    /// (FIX4: the replay is exact or an error, so there is no uncertain set left to check).
    #[test]
    fn fix3_review_list_inventory_replays_exactly_on_every_scenario() {
        let dir = tempfile::tempdir().unwrap();
        let mut lists = 0;
        for name in ["bankfeeds-accept", "bankfeeds-connect-map", "bankfeeds-pulls", "bankfeeds-random-500", "bankfeeds-rules"] {
            let scenario: Value = serde_json::from_str(&std::fs::read_to_string(format!("{}/scenarios/{name}.json", env!("CARGO_MANIFEST_DIR"))).unwrap()).unwrap();
            let mut runner = crate::Runner::new(&scenario, &dir.path().join(format!("{name}.fbbook")));
            runner.run();
            let dump = runner.dump();
            let m = manifest(&dump).unwrap();
            for o in rows(&dump, "outcomes").iter().filter(|o| o["op"] == "bankfeeds_items" && o["ok"] == true) {
                let company = text(&o["manifest"], "company");
                let present = inventory(&dump, &m, company, o["step"].as_u64().unwrap()).expect("the replay lines up").present;
                let key = text(&o["manifest"], "result");
                let listed: BTreeSet<&str> = strings(&dump["bankfeeds"]["results"][key]["items"]).unwrap().into_iter().collect();
                assert_eq!(listed, present, "{name} step {}", o["step"]);
                lists += 1;
            }
        }
        assert!(lists >= 5, "review lists checked: {lists}");
    }
}
