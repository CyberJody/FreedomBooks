//! Differential normalization only for architecture changes: PG tables/schema
//! cannot equal SQLCipher tables/schema. Each engine verifies its full native
//! manifest independently; per-company accounting manifests compare verbatim.
use super::*;
use fb_core::{
    accounts::{LockPolicy, UnlockThrottle},
    backups::{self, BackupOptions, RestoreOptions},
};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub(super) struct State {
    root: PathBuf,
    files: HashMap<String, PathBuf>,
    pub results: Map<String, Value>,
    throttle: UnlockThrottle,
    live: PathBuf,
    verified: HashMap<String, String>,
    identity: Option<Value>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            root: std::env::var_os("FB_HARNESS_VAR")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir)
                .join(format!("fbv2-l4b-harness-{}", fb_ids::new_v7())),
            files: HashMap::new(),
            results: Map::new(),
            live: PathBuf::new(),
            verified: HashMap::new(),
            identity: None,
            throttle: UnlockThrottle::new(LockPolicy::default()).expect("fixed policy"),
        }
    }
}
impl State {
    pub fn new(live: &Path) -> Self {
        Self {
            live: live.to_path_buf(),
            ..Self::default()
        }
    }
}
fn err(e: impl std::fmt::Display) -> StepError {
    FbError::validation(e.to_string()).into()
}

impl Runner<'_> {
    pub(super) fn backup_identity(&mut self) -> Option<Value> {
        self.backups.identity.clone().map(|mut identity| {
            // The path travels with the handle opened by SelectedBook, including
            // temporary swaps. The core intentionally denies database_list.
            identity["selected"] = json!(self.book.path);
            identity["history"] = json!(history(self.scenario, self.outcomes.len()));
            identity
        })
    }

    fn normalized_backup(&self, data: &backups::Manifest) -> Result<Value, StepError> {
        let mut companies = data.contents.companies.clone();
        for company in &mut companies {
            company.id = self
                .companies
                .iter()
                .find(|(_, v)| v.to_db() == company.id)
                .map(|(k, _)| k.clone())
                .ok_or_else(|| err("Unknown backup company."))?;
            for account in &mut company.accounts {
                account.id = self
                    .accounts
                    .iter()
                    .find(|(_, v)| v.to_db() == account.id)
                    .map(|(k, _)| k.clone())
                    .ok_or_else(|| err("Unknown backup account."))?;
            }
        }
        let company_count = data
            .contents
            .tables
            .get("ledger_company")
            .ok_or_else(|| err("Missing ledger_company count."))?;
        let account_count = data
            .contents
            .tables
            .get("ledger_account")
            .ok_or_else(|| err("Missing ledger_account count."))?;
        Ok(
            json!({"kind":"snapshot","companies":companies,"company_count":company_count,"account_count":account_count}),
        )
    }
    pub(super) fn apply_backups(&mut self, step: &Value) -> Result<bool, StepError> {
        let op = s(step, "op");
        if !op.starts_with("backups_") {
            return Ok(false);
        }
        let root = self.backups.root.clone();
        std::fs::create_dir_all(root.join("out")).map_err(err)?;
        std::fs::create_dir_all(root.join("restored")).map_err(err)?;
        let work = root.join("private");
        let key = self.outcomes.len().to_string();
        let result = match op {
            "backups_live_post" => {
                if self.backups.identity.is_none() {
                    return Err(err("Restore a book before the live-only probe."));
                }
                let actor = self.actor(step);
                let company = find(&self.companies, s(step, "company"))?;
                let account = find(&self.accounts, s(step, "account"))?;
                let mut live = SelectedBook::open(&self.backups.live, &self.key).map_err(err)?;
                std::mem::swap(&mut self.book, &mut live);
                let applied = (|| {
                    let before =
                        reports::register(&mut self.book, actor, company, account, None, None)?
                            .closing
                            .cents();
                    self.apply(&step["draft"])?;
                    self.apply(&step["post"])?;
                    let after =
                        reports::register(&mut self.book, actor, company, account, None, None)?
                            .closing
                            .cents();
                    Ok::<_, StepError>((before, after))
                })();
                std::mem::swap(&mut self.book, &mut live);
                let (before, after) = applied?;
                let restored =
                    reports::register(&mut self.book, actor, company, account, None, None)?
                        .closing
                        .cents();
                json!({"kind":"isolation","live_before":before,"live_after":after,"restored":restored})
            }
            "backups_attempt" => {
                let (value, locked) = self.backups.throttle.attempt(
                    step.get("now")
                        .and_then(Value::as_u64)
                        .ok_or_else(|| err("Invalid throttle time."))?,
                    || {
                        if step["correct"] == true {
                            Some(true)
                        } else {
                            None
                        }
                    },
                );
                json!({"kind":"throttle","authenticated":value.is_some(),"locked":locked})
            }
            "backups_retention" => {
                let raw = step
                    .get("label")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| err("Missing retention label."))?;
                let label = backups::retention::safe_db_label(raw);
                let keep = step
                    .get("keep")
                    .and_then(Value::as_u64)
                    .and_then(|k| usize::try_from(k).ok())
                    .filter(|k| *k > 0)
                    .ok_or_else(|| err("Invalid retention count."))?;
                let dir = root.join(format!("retention-{key}"));
                std::fs::create_dir(&dir).map_err(err)?;
                let mut mine = Vec::new();
                for day in 1..=5 {
                    let name =
                        backups::retention::filename(raw, &format!("2020-01-{day:02}T00:00:00Z"))
                            .map_err(err)?;
                    let path = dir.join(name);
                    std::fs::write(&path, b"synthetic").map_err(err)?;
                    mine.push(path);
                }
                let verified = mine
                    .get(1)
                    .ok_or_else(|| err("Missing verified fixture."))?;
                backups::retention::record_verified(verified).map_err(err)?;
                let other =
                    backups::retention::filename(&format!("{raw}_extra"), "2020-01-01T00:00:00Z")
                        .map_err(err)?;
                std::fs::write(dir.join(other), b"other").map_err(err)?;
                std::fs::write(
                    dir.join(format!("freedombooks-{label}-notstamp.fbbackup")),
                    b"invalid",
                )
                .map_err(err)?;
                std::fs::create_dir(
                    dir.join(format!("freedombooks-{label}-20200107T000000Z.fbbackup")),
                )
                .map_err(err)?;
                #[cfg(unix)]
                std::os::unix::fs::symlink(
                    verified,
                    dir.join(format!("freedombooks-{label}-20200106T000000Z.fbbackup")),
                )
                .map_err(err)?;
                let before = backups::retention::backup_files(&dir, Some(&label)).map_err(err)?;
                let removed = backups::retention::prune(
                    &dir,
                    &label,
                    keep,
                    mine.first().map(PathBuf::as_path),
                )
                .map_err(err)?;
                let after = backups::retention::backup_files(&dir, Some(&label)).map_err(err)?;
                let all = backups::retention::backup_files(&dir, None).map_err(err)?;
                let names = |paths: &[PathBuf]| -> Result<Vec<String>, StepError> {
                    paths
                        .iter()
                        .map(|p| {
                            p.file_name()
                                .and_then(|n| n.to_str())
                                .map(str::to_owned)
                                .ok_or_else(|| err("Invalid retention filename."))
                        })
                        .collect()
                };
                if !backups::retention::verified_note(verified)
                    || removed
                        .iter()
                        .any(|p| backups::retention::sidecar(p).exists())
                {
                    return Err(err("Retention sidecar protection failed."));
                }
                json!({"kind":"retention","label":label,"keep":keep,"before":names(&before)?,"removed":names(&removed)?,"after":names(&after)?,"all_after":names(&all)?,"notes_after":[verified.file_name().and_then(|n|n.to_str()).ok_or_else(||err("Invalid verified name."))?]})
            }
            "backups_snapshot" => {
                let name = s(step, "ref");
                let output = root
                    .join("out")
                    .join(format!("{}.fbbackup", self.outcomes.len()));
                let result = backups::create_backup(
                    &self.book,
                    &self.key,
                    &BackupOptions {
                        output: &output,
                        work_root: &work,
                        receipts_root: Some(self.attachment_store.folder()),
                        verify_receipt: Some(&backups::attachment_receipt_verifier(&self.key)),
                        label: "harness",
                        app_version: "test",
                        created_at: "2026-10-30T12:00:00Z",
                        passphrase: None,
                        keep: None,
                    },
                )
                .map_err(err)?;
                self.backups.files.insert(name.into(), output);
                self.normalized_backup(&result.manifest)?
            }
            "backups_verify" | "backups_restore" => {
                let file = self
                    .backups
                    .files
                    .get(s(step, "backup"))
                    .ok_or_else(|| err("Unknown backup."))?
                    .clone();
                let data = if op == "backups_verify" {
                    let verifier = backups::attachment_receipt_verifier(&self.key);
                    let data = backups::verify_backup_with_receipts(
                        &file,
                        &self.key,
                        &work,
                        None,
                        false,
                        Some(&verifier),
                    )
                    .map_err(err)?;
                    self.backups
                        .verified
                        .insert(s(step, "backup").into(), data.book_sha256.clone());
                    data
                } else {
                    let folder = root.join("restored").join(self.outcomes.len().to_string());
                    std::fs::create_dir(&folder).map_err(err)?;
                    let target = folder.join("book.fbbook");
                    let receipts = folder.join("receipts");
                    let result = backups::restore_backup(
                        &file,
                        &self.key,
                        &RestoreOptions {
                            verify_receipt: Some(&backups::attachment_receipt_verifier(&self.key)),
                            target: &target,
                            receipts_target: Some(&receipts),
                            work_root: &work,
                            protected: &[],
                            passphrase: None,
                            require_encrypted: false,
                        },
                    )
                    .map_err(err)?;
                    let payload = folder.join("verified-input.fbbook");
                    let mut archive = tar::Archive::new(std::fs::File::open(&file).map_err(err)?);
                    let mut found = false;
                    for entry in archive.entries().map_err(err)? {
                        let mut entry = entry.map_err(err)?;
                        if entry.path().map_err(err)? == Path::new("book.fbbook") {
                            let mut output = std::fs::File::create(&payload).map_err(err)?;
                            std::io::copy(&mut entry, &mut output).map_err(err)?;
                            found = true;
                        }
                    }
                    if !found {
                        return Err(err("Missing verified restore input."));
                    }
                    let digest = format!(
                        "{:x}",
                        Sha256::digest(std::fs::read(&payload).map_err(err)?)
                    );
                    let target_digest =
                        format!("{:x}", Sha256::digest(std::fs::read(&target).map_err(err)?));
                    // Lifecycle scenarios may restore without a separate verify operation.
                    let verified = self
                        .backups
                        .verified
                        .get(s(step, "backup"))
                        .cloned()
                        .unwrap_or_else(|| result.manifest.book_sha256.clone());
                    if digest != verified {
                        return Err(err("Restored file differs from verified backup."));
                    }
                    self.book = SelectedBook::open(&target, &self.key).map_err(err)?;
                    if self
                        .book
                        .read(|c| backups::manifest::contents(c))
                        .map_err(err)?
                        != result.manifest.contents
                    {
                        return Err(err("Opened restored book differs from verified contents."));
                    }
                    self.attachment_store =
                        fb_core::attachments::storage::Store::for_book(&target, &self.key)?;
                    self.backups.identity = Some(
                        json!({"restore_step":self.outcomes.len(),"backup":s(step,"backup"),"target":std::fs::canonicalize(&target).map_err(err)?,"live":std::fs::canonicalize(&self.backups.live).map_err(err)?,"sha256":digest,"verified_sha256":verified,"hash_kind":"sqlcipher-restore-input","hash_path":payload,"target_sha256":target_digest}),
                    );
                    result.manifest
                };
                self.normalized_backup(&data)?
            }
            "backups_refuse_existing" => {
                let file = self
                    .backups
                    .files
                    .get(s(step, "backup"))
                    .ok_or_else(|| err("Unknown backup."))?;
                let target = root.join("restored").join("occupied.fbbook");
                std::fs::write(&target, b"sentinel").map_err(err)?;
                let refusal = backups::restore_backup(
                    file,
                    &self.key,
                    &RestoreOptions {
                        verify_receipt: None,
                        target: &target,
                        receipts_target: None,
                        work_root: &work,
                        protected: &[],
                        passphrase: None,
                        require_encrypted: false,
                    },
                )
                .err()
                .ok_or_else(|| err("Existing target was not protected."))?;
                if refusal.to_string().contains("existing book")
                    && std::fs::read(&target).map_err(err)? == b"sentinel"
                {
                    return Err(err("Restore only goes into a NEW target."));
                }
                return Err(err("Existing target was not protected."));
            }
            _ => return Err(err("Unknown backups operation.")),
        };
        self.backups.results.insert(key, result);
        Ok(true)
    }
}

pub(super) fn validate(dump: &Value) -> Result<(), String> {
    validate_identity(dump)?;
    let Some(results) = dump.get("backups") else {
        if dump["outcomes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["op"].as_str().unwrap().starts_with("backups_") && o["ok"] == true)
        {
            return Err("Missing backups results.".into());
        }
        return Ok(());
    };
    let results = results.as_object().ok_or("Malformed backups results.")?;
    let expected: BTreeMap<String, &Value> = dump["outcomes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|o| o["op"].as_str().unwrap().starts_with("backups_") && o["ok"] == true)
        .map(|o| (o["step"].to_string(), o))
        .collect();
    if results.keys().collect::<BTreeSet<_>>() != expected.keys().collect() {
        return Err("Incomplete backups result keys.".into());
    }
    for (key, row) in results {
        if expected[key]["op"] == "backups_live_post" {
            if row["kind"] != "isolation"
                || ["live_before", "live_after", "restored"]
                    .iter()
                    .any(|k| row[k].as_i64().is_none())
            {
                return Err("Malformed isolation probe.".into());
            }
        } else if expected[key]["op"] == "backups_attempt" {
            if row.as_object().is_none_or(|o| o.len() != 3)
                || row["kind"] != "throttle"
                || !row["authenticated"].is_boolean()
                || !row["locked"].is_boolean()
                || (row["authenticated"] == true && row["locked"] == true)
            {
                return Err("Malformed throttle result.".into());
            }
        } else if expected[key]["op"] == "backups_retention" {
            validate_retention(row)?;
        } else {
            if row.as_object().is_none_or(|o| o.len() != 4)
                || row["kind"] != "snapshot"
                || !row["companies"].is_array()
            {
                return Err("Malformed snapshot result.".into());
            }
            let mut companies = BTreeSet::new();
            if row["company_count"].as_u64()
                != Some(row["companies"].as_array().unwrap().len() as u64)
            {
                return Err("Incomplete snapshot company set.".into());
            }
            let mut account_count = 0u64;
            for company in row["companies"].as_array().unwrap() {
                if company.as_object().is_none_or(|o| o.len() != 6)
                    || !company["id"].is_string()
                    || !companies.insert(company["id"].as_str().unwrap())
                    || !company["name"].is_string()
                    || !company["currency"].is_string()
                {
                    return Err("Malformed snapshot company.".into());
                }
                for key in ["posted", "draft"] {
                    if company["journals"][key].as_i64().is_none_or(|n| n < 0) {
                        return Err("Malformed journal count.".into());
                    }
                }
                if company["journals"].as_object().is_none_or(|o| o.len() != 2) {
                    return Err("Incomplete journal counts.".into());
                }
                let money = |v: &Value| -> Option<i128> {
                    let s = v.as_str()?;
                    let (a, b) = s.split_once('.')?;
                    if a.is_empty()
                        || !a.bytes().all(|c| c.is_ascii_digit())
                        || b.len() != 2
                        || !b.bytes().all(|c| c.is_ascii_digit())
                    {
                        return None;
                    }
                    a.parse::<i128>()
                        .ok()?
                        .checked_mul(100)?
                        .checked_add(b.parse::<i128>().ok()?)
                };
                for key in ["debit", "credit"] {
                    if money(&company["trial_balance"][key]).is_none() {
                        return Err("Invalid snapshot money.".into());
                    }
                }
                if company["trial_balance"]
                    .as_object()
                    .is_none_or(|o| o.len() != 2)
                {
                    return Err("Incomplete snapshot totals.".into());
                }
                let accounts = company["accounts"]
                    .as_array()
                    .ok_or("Missing snapshot accounts.")?;
                let mut ids = BTreeSet::new();
                account_count += accounts.len() as u64;
                let mut total_d = 0i128;
                let mut total_c = 0i128;
                for a in accounts {
                    if a.as_object().is_none_or(|o| o.len() != 8)
                        || ["id", "code", "name", "kind"]
                            .iter()
                            .any(|k| !a[k].is_string())
                        || !ids.insert(a["id"].as_str().unwrap())
                    {
                        return Err("Malformed snapshot account.".into());
                    }
                    if !matches!(
                        a["kind"].as_str(),
                        Some("ASSET" | "LIABILITY" | "EQUITY" | "INCOME" | "EXPENSE")
                    ) || !dump["accounts"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|live| live["ref"] == a["id"] && live["company"] == company["id"])
                    {
                        return Err("Invalid snapshot account identity/kind.".into());
                    }
                    for key in [
                        "posted_debits",
                        "posted_credits",
                        "debit_balance",
                        "credit_balance",
                    ] {
                        if money(&a[key]).is_none() {
                            return Err("Invalid snapshot account money.".into());
                        }
                    }
                    let d = money(&a["posted_debits"]).ok_or("Invalid debit.")?;
                    let c = money(&a["posted_credits"]).ok_or("Invalid credit.")?;
                    let db = money(&a["debit_balance"]).ok_or("Invalid debit balance.")?;
                    let cb = money(&a["credit_balance"]).ok_or("Invalid credit balance.")?;
                    let net = d.checked_sub(c).ok_or("Snapshot net overflow.")?;
                    if db != net.max(0)
                        || cb != net.checked_neg().ok_or("Snapshot net overflow.")?.max(0)
                    {
                        return Err("Snapshot account balance mismatch.".into());
                    }
                    total_d = total_d.checked_add(db).ok_or("Snapshot total overflow.")?;
                    total_c = total_c.checked_add(cb).ok_or("Snapshot total overflow.")?;
                }
                if money(&company["trial_balance"]["debit"]) != Some(total_d)
                    || money(&company["trial_balance"]["credit"]) != Some(total_c)
                    || total_d != total_c
                {
                    return Err("Snapshot trial balance does not reconcile.".into());
                }
            }
            if row["account_count"].as_u64() != Some(account_count) {
                return Err("Incomplete snapshot account set.".into());
            }
        }
    }
    Ok(())
}

pub(super) fn compare(report: &mut Report, py: &Value, rs: &Value) {
    let mut p = py["backups"].clone();
    let mut r = rs["backups"].clone();
    if let (Some(p), Some(r)) = (p.as_object_mut(), r.as_object_mut()) {
        for (step, r) in r {
            if r["kind"] != "snapshot" {
                continue;
            }
            let Some(p) = p.get_mut(step).filter(|p| p["kind"] == "snapshot") else {
                continue;
            };
            for (pc, rc) in p["companies"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .zip(r["companies"].as_array_mut().unwrap())
            {
                let label = format!("backup {step} account order");
                let pa = pc["accounts"].as_array_mut().unwrap();
                let ra = rc["accounts"].as_array_mut().unwrap();
                check_rust_order(report, &label, ra, |a| {
                    ("company".into(), a["code"].as_str().unwrap().into())
                });
                let mut ps = pa.clone();
                let mut rsorted = ra.clone();
                ps.sort_by_key(|a| a["id"].to_string());
                rsorted.sort_by_key(|a| a["id"].to_string());
                if ps == rsorted {
                    check_order(
                        report,
                        &label,
                        pa,
                        ra,
                        |a| a["id"].to_string(),
                        |a| ("company".into(), a["code"].as_str().unwrap().into()),
                    );
                }
                *pa = ps;
                *ra = rsorted;
            }
        }
    }
    if !p.is_null() || !r.is_null() {
        same(report, "backups", &p, &r);
    }
}

fn validate_retention(row: &Value) -> Result<(), String> {
    let label = row["label"].as_str().ok_or("Missing retention label.")?;
    let keep = row["keep"]
        .as_u64()
        .filter(|k| *k > 0)
        .ok_or("Invalid retention keep.")?;
    if row.as_object().is_none_or(|o| o.len() != 8) || row["kind"] != "retention" {
        return Err("Malformed retention result.".into());
    }
    let before: Vec<_> = (1..=5)
        .rev()
        .map(|d| format!("freedombooks-{label}-202001{d:02}T000000Z.fbbackup"))
        .collect();
    let protected = before.get(4).ok_or("Missing protected name.")?;
    let verified = before.get(3).ok_or("Missing verified name.")?;
    let removed: Vec<_> = before
        .iter()
        .skip(usize::try_from(keep).map_err(|_| "Retention keep overflow.")?)
        .filter(|p| *p != protected && *p != verified)
        .cloned()
        .collect();
    let after: Vec<_> = before
        .iter()
        .filter(|p| !removed.contains(p))
        .cloned()
        .collect();
    if row["before"] != json!(before)
        || row["removed"] != json!(removed)
        || row["after"] != json!(after)
        || row["notes_after"] != json!([verified])
        || row["all_after"]
            .as_array()
            .is_none_or(|a| a.len() != after.len() + 1)
        || after.iter().any(|p| {
            !row["all_after"]
                .as_array()
                .is_some_and(|a| a.contains(&json!(p)))
        })
    {
        return Err("Retention inventory/protection mismatch.".into());
    }
    Ok(())
}

pub(super) fn history(scenario: &Value, count: usize) -> Vec<usize> {
    let mut snapshots: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut history = Vec::new();
    for (index, step) in scenario["steps"]
        .as_array()
        .unwrap()
        .iter()
        .take(count)
        .enumerate()
    {
        if step["op"] == "backups_restore" {
            history = snapshots[step["backup"].as_str().unwrap()].clone();
        }
        history.push(index);
        if step["op"] == "backups_snapshot" {
            snapshots.insert(step["ref"].as_str().unwrap(), history.clone());
        }
    }
    history
}
fn validate_identity(dump: &Value) -> Result<(), String> {
    let restore = dump["outcomes"]
        .as_array()
        .ok_or("Missing driver outcomes.")?
        .iter()
        .rev()
        .find(|o| o["op"] == "backups_restore" && o["ok"] == true);
    let Some(restore) = restore else {
        return if dump.get("book_identity").is_some() {
            Err("Unexpected restored identity.".into())
        } else {
            Ok(())
        };
    };
    let id = &dump["book_identity"];
    let hash = id["sha256"].as_str().unwrap_or("");
    if id["restore_step"].as_u64().is_none()
        || id["restore_step"] != restore["step"]
        || id["selected"] != id["target"]
        || id["selected"] == id["live"]
        || ["selected", "target", "live", "backup", "hash_path"]
            .iter()
            .any(|k| id[k].as_str().is_none_or(str::is_empty))
        || hash.len() != 64
        || !hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || id["sha256"] != id["verified_sha256"]
        || !matches!(
            id["hash_kind"].as_str(),
            Some("sqlcipher-restore-input" | "pg-restore-input")
        )
        || !id["history"].is_array()
    {
        return Err(
            "Restored book identity/hash/history is missing or differs from the selected target."
                .into(),
        );
    }
    if id["hash_kind"] == "sqlcipher-restore-input" {
        let hash = id["target_sha256"].as_str().unwrap_or("");
        if hash.len() != 64
            || !hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("Missing restored SQLCipher target file hash.".into());
        }
    }
    Ok(())
}
pub(super) fn intended(scenario: &Value, side: &Value) -> Vec<String> {
    let mut errors = Vec::new();
    let check = |dump: &Value, errors: &mut Vec<String>| {
        if let Err(e) = validate_identity(dump) {
            errors.push(e);
            return;
        }
        if let Some(id) = dump.get("book_identity") {
            let step = id["restore_step"].as_u64().unwrap() as usize;
            if id["backup"] != scenario["steps"][step]["backup"]
                || id["history"]
                    != json!(history(
                        scenario,
                        dump["outcomes"].as_array().unwrap().len()
                    ))
            {
                errors.push("Restored identity differs from scenario provenance.".into());
            }
        }
        for (index, step) in scenario["steps"]
            .as_array()
            .unwrap()
            .iter()
            .take(dump["outcomes"].as_array().unwrap().len())
            .enumerate()
        {
            if step["op"] == "backups_live_post"
                && (dump["backups"][index.to_string()] != step["expected"]
                    || dump["registers"][s(step, "account")]["closing"]
                        != step["expected"]["restored"]
                    || dump["journals"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|j| j["ref"] == step["draft"]["ref"]))
            {
                errors.push(format!(
                    "Live/restored report isolation pin differs at step {index}."
                ));
            }
        }
    };
    check(side.get("final").unwrap_or(side), &mut errors);
    for cp in side["checkpoints"].as_array().into_iter().flatten() {
        check(&cp["dump"], &mut errors);
    }
    errors
}

/// Keep selected-handle provenance inseparable from its successful open. No core
/// connection-policy escape is added just to expose SQLite's filename pragma.
pub(super) struct SelectedBook {
    inner: Book,
    path: PathBuf,
}
impl SelectedBook {
    pub fn create(path: &Path, key: &[u8; 32]) -> Result<Self, fb_db::DbError> {
        let inner = Book::create(path, key)?;
        Ok(Self {
            inner,
            path: std::fs::canonicalize(path).expect("created book path"),
        })
    }
    pub(super) fn open(path: &Path, key: &[u8; 32]) -> Result<Self, fb_db::DbError> {
        let mut inner = Book::open(path, key)?;
        let connections = inner.read(|q| {
            q.prepare("SELECT id,created_by_id,company_id FROM bankfeeds_connection WHERE active=1")?
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?
                .collect::<Result<Vec<_>, fb_db::rusqlite::Error>>()
        }).map_err(fb_db::DbError::from)?;
        for (id, actor, company) in connections {
            let restore = inner.authenticate_bankfeed_restore(&id, &crate::fake_credentials::wrapping_key(key))
                .map_err(fb_db::DbError::from)?;
            fb_core::bankfeeds::restore_connection_credentials(&mut inner, actor.parse().expect("stored actor"), company.parse().expect("stored company"), &id,
                &restore, Vec::<String>::new())
                .map_err(|_| fb_db::DbError::Pragma("Synthetic harness credential restore failed".into()))?;
        }
        Ok(Self {
            inner,
            path: std::fs::canonicalize(path).expect("opened book path"),
        })
    }
}
impl std::ops::Deref for SelectedBook {
    type Target = Book;
    fn deref(&self) -> &Book {
        &self.inner
    }
}
impl std::ops::DerefMut for SelectedBook {
    fn deref_mut(&mut self) -> &mut Book {
        &mut self.inner
    }
}

#[cfg(test)]
mod review_tests {
    use super::*;
    #[test]
    fn normalization_missing_table_returns_step_error_instead_of_panicking() {
        let scenario = json!({"users":[], "steps":[]});
        let dir = tempfile::tempdir().unwrap();
        let runner = Runner::new(&scenario, &dir.path().join("book.fbbook"));
        let contents = backups::manifest::contents(runner.book.connection()).unwrap();
        let mut data = backups::Manifest {
            format: backups::FORMAT.into(),
            format_version: 3,
            created_at: "test".into(),
            app_version: "test".into(),
            migrations: vec![],
            book_bytes: 0,
            book_sha256: String::new(),
            contents,
            attachments: vec![],
            not_included: vec![],
        };
        data.contents.tables.remove("ledger_company");
        assert!(runner.normalized_backup(&data).is_err());
        data.contents.tables.insert("ledger_company".into(), 0);
        data.contents.tables.remove("ledger_account");
        assert!(runner.normalized_backup(&data).is_err());
    }
}
