//! DESKTOP-BACKUPS proof helper (test tooling, not shipped). Secrets come from a file and are never printed.
//!
//!   backup_proof seed    <data dir> <password file> <scenario.json>
//!       A scratch desktop data folder: `books/default.fbbook` from a harness scenario (invoices, bills,
//!       payments, deposits, a reconciliation …) plus receipts attached through fb-core into
//!       `books/receipts/`, and the app-lock wrap `book-key.lock.json` the Electron app unlocks.
//!   backup_proof report  <book> <password file> <data dir> <out.json>
//!       book check, per-company trial balance / balance sheet / P&L, per-table row counts and content
//!       hashes, every attachment's decrypted bytes hash, the audit trail. Reads a private copy.
//!   backup_proof compare <original.json> <restored.json>
//!       exit 1 unless the restored book equals the original as of its backup.
//!   backup_proof leakscan <data dir> <password file> <book> [log …]
//!       no password/key in clear anywhere; no book content in clear in backup-side files.
use fb_app::keys;
use fb_core::{CompanyId, UserId};
use fb_db::Book;
use fb_db::rusqlite::types::ValueRef;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02x}")).collect()
}
fn sha(b: &[u8]) -> String {
    hex(&Sha256::digest(b))
}
fn password(file: &str) -> zeroize::Zeroizing<String> {
    zeroize::Zeroizing::new(std::fs::read_to_string(file).expect("password file").trim_end_matches('\n').to_owned())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("seed") if args.len() == 5 => seed(Path::new(&args[2]), &args[3], Path::new(&args[4]), 0),
        // BACKUPS-POLISH: the same book plus about MB megabytes of incompressible receipts, so a backup
        // takes long enough to prove the window stays responsive while it runs.
        Some("seed-large") if args.len() == 6 => seed(Path::new(&args[2]), &args[3], Path::new(&args[4]), args[5].parse().expect("MB")),
        Some("report") if args.len() == 6 => report(Path::new(&args[2]), &args[3], Path::new(&args[4]), Path::new(&args[5])),
        Some("compare") if args.len() == 4 => std::process::exit(compare(Path::new(&args[2]), Path::new(&args[3]))),
        Some("leakscan") if args.len() >= 5 => std::process::exit(leakscan(Path::new(&args[2]), &args[3], Path::new(&args[4]), &args[5..])),
        _ => {
            eprintln!("usage: backup_proof seed|report|compare …");
            std::process::exit(2);
        }
    }
}

/// Incompressible bytes inside a PDF frame (plain filler at both ends, so the attachment checks pass).
fn bulk_pdf(bytes: usize, seed: u64) -> Vec<u8> {
    let mut out = b"%PDF-1.4\n% bulk receipt for the responsiveness proof\n".to_vec();
    out.resize(out.len() + 70_000, b' ');
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    while out.len() < bytes.saturating_sub(80_000) {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        out.extend_from_slice(&x.to_le_bytes());
    }
    out.resize(out.len() + 70_000, b' ');
    out.extend_from_slice(b"\n1 0 obj<</Type/Catalog>>endobj\ntrailer<</Root 1 0 R>>\n%%EOF\n");
    out
}

fn seed(data: &Path, password_file: &str, scenario: &Path, extra_mb: usize) {
    let books = data.join("books");
    std::fs::create_dir_all(&books).unwrap();
    let book_path = books.join("default.fbbook");
    assert!(!book_path.exists(), "seed refuses an existing book");
    let key = keys::new_master_key().unwrap();
    let scenario: Value = fb_db::parse_unique_json(&std::fs::read_to_string(scenario).unwrap()).unwrap();
    {
        let mut runner = fb_harness::Runner::with_key(&scenario, &book_path, &key);
        runner.run();
        let dump = runner.dump();
        let bad = dump["outcomes"].as_array().unwrap().iter().filter(|o| o["ok"] != true).count();
        assert_eq!(bad, 0, "scenario steps failed");
    }
    let mut book = Book::open(&book_path, &key).unwrap();
    let store = fb_core::attachments::storage::Store::for_book(&book_path, &key).unwrap();
    let (owner, company): (String, String) = book
        .read(|tx| tx.query_row("SELECT m.user_id, c.id FROM ledger_company c JOIN ledger_company_members m ON m.company_id=c.id ORDER BY c.name, c.id LIMIT 1", [], |r| Ok((r.get(0)?, r.get(1)?))))
        .unwrap();
    let (owner, company) = (UserId::parse(&owner).unwrap(), CompanyId::parse(&company).unwrap());
    let docs: Vec<(String, String)> = book
        .read(|tx| {
            let mut st = tx.prepare("SELECT id, kind FROM documents_documentidentity WHERE company_id=?1 AND kind IN ('BILL','INV','DEP') ORDER BY kind, number")?;
            st.query_map([company.to_db()], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<Vec<_>, _>>()
        })
        .unwrap();
    let pdf = b"%PDF-1.4\n% proof receipt\n1 0 obj<</Type/Catalog>>endobj\ntrailer<</Root 1 0 R>>\n%%EOF\n".to_vec();
    let png = hex_bytes("89504e470d0a1a0a0000000d4948445200000001000000010802000000907753de0000000c49444154789c6338b1a50200043c01f5201698a50000000049454e44ae426082");
    let mut added = 0;
    for kind in ["BILL", "INV", "DEP"] {
        let Some((id, _)) = docs.iter().find(|(_, k)| k == kind) else { continue };
        let owner_doc = book.read(|tx| fb_core::attachments::document_owner(tx, company, id)).unwrap();
        for (n, (data, name)) in [(&pdf, "receipt.pdf"), (&png, "photo.png")].into_iter().enumerate() {
            let request = format!("00000000-0000-4000-8000-{:012}", added * 10 + n);
            fb_core::attachments::add(&mut book, &store, owner, company, &owner_doc, data, name, &request, &Default::default()).unwrap();
        }
        added += 1;
    }
    assert!(added >= 2, "scenario has bills and invoices to attach to");
    const BULK_MB: usize = 32;
    let limits = fb_core::attachments::Limits { max_mb: 40, uploads_per_window: 1000, company_max_mb: 8192 };
    for n in 0..extra_mb.div_ceil(BULK_MB) {
        let (id, _) = &docs[n % docs.len()];
        let owner_doc = book.read(|tx| fb_core::attachments::document_owner(tx, company, id)).unwrap();
        let request = format!("00000000-0000-4000-9000-{:012}", n);
        let data = bulk_pdf(BULK_MB * 1024 * 1024, 2 * n as u64 + 7);
        fb_core::attachments::add(&mut book, &store, owner, company, &owner_doc, &data, "bulk-receipt.pdf", &request, &limits).unwrap();
    }
    drop(book);
    let wrapped = keys::wrap(&key, &password(password_file)).unwrap();
    let lock = data.join("book-key.lock.json");
    std::fs::write(&lock, serde_json::to_string(&wrapped).unwrap()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for p in [data.to_owned(), books.clone(), lock] {
            let mode = if p.is_dir() { 0o700 } else { 0o600 };
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
        }
    }
    println!("seeded {} (attachments on {added} documents)", book_path.display());
}

fn hex_bytes(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

fn cell(v: ValueRef<'_>) -> String {
    match v {
        ValueRef::Null => "N".into(),
        ValueRef::Integer(i) => format!("I{i}"),
        ValueRef::Real(f) => format!("R{f:?}"),
        ValueRef::Text(t) => format!("T{}", sha(t)),
        ValueRef::Blob(b) => format!("B{}", sha(b)),
    }
}

/// Report a private copy of `book` (and its receipts folder): the original files are only read.
fn report(book_path: &Path, password_file: &str, data: &Path, out: &Path) {
    let blob: keys::WrappedKey = serde_json::from_str(&std::fs::read_to_string(data.join("book-key.lock.json")).unwrap()).unwrap();
    let key = keys::unwrap(&blob, &password(password_file)).expect("unlock");
    let main_hash = sha(&std::fs::read(book_path).unwrap());
    for sidecar in ["-wal", "-shm"] {
        let mut s = book_path.as_os_str().to_owned();
        s.push(sidecar);
        assert!(!PathBuf::from(s).exists(), "report needs a closed book (no {sidecar})");
    }
    let copy_dir = tempfile::tempdir().unwrap();
    let copy = copy_dir.path().join("book.fbbook");
    std::fs::copy(book_path, &copy).unwrap();
    let receipts = book_path.parent().unwrap().join("receipts");
    std::fs::create_dir(copy_dir.path().join("receipts")).unwrap();
    let mut receipt_files = BTreeMap::new();
    if receipts.is_dir() {
        for e in std::fs::read_dir(&receipts).unwrap() {
            let e = e.unwrap();
            let bytes = std::fs::read(e.path()).unwrap();
            receipt_files.insert(e.file_name().to_string_lossy().into_owned(), sha(&bytes));
            std::fs::write(copy_dir.path().join("receipts").join(e.file_name()), bytes).unwrap();
        }
    }
    let mut book = Book::open(&copy, &key).unwrap();
    let problems = book.check().unwrap();
    let (tables, audit, attachments, companies) = book
        .read(|tx| -> fb_db::rusqlite::Result<_> {
            let names: Vec<String> = tx.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")?.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
            let mut tables = BTreeMap::new();
            let mut audit_rows = Vec::new();
            for name in names {
                let mut st = tx.prepare(&format!("SELECT * FROM \"{name}\""))?;
                let n = st.column_count();
                let cols: Vec<String> = st.column_names().map(str::to_owned).collect();
                let mut rows: Vec<String> = st.query_map([], |r| Ok((0..n).map(|i| r.get_ref(i).map(cell)).collect::<Result<Vec<_>, _>>()?.join("|")))?.collect::<Result<_, _>>()?;
                rows.sort();
                if name == "ledger_auditevent" {
                    let mut st = tx.prepare("SELECT action, occurred_at, target FROM ledger_auditevent ORDER BY occurred_at, id")?;
                    audit_rows = st.query_map([], |r| Ok(json!({"action": r.get::<_, String>(0)?, "at": r.get::<_, String>(1)?, "target": r.get::<_, String>(2)?})))?.collect::<Result<_, _>>()?;
                }
                tables.insert(name, json!({"rows": rows.len(), "sha256": sha(rows.join("\n").as_bytes()), "row_hashes": rows.iter().map(|r| sha(r.as_bytes())).collect::<Vec<_>>(), "columns": cols.len()}));
            }
            let mut st = tx.prepare("SELECT id, company_id, stored_name, content_type, size, sha256, original_sha256, wrapped_key, wrap_nonce, nonce, original_name FROM attachments_attachment ORDER BY id")?;
            let att: Vec<(String, String, fb_core::attachments::storage::Stored, String)> = st
                .query_map([], |r| {
                    Ok((r.get(0)?, r.get(1)?, fb_core::attachments::storage::Stored {
                        stored_name: r.get(2)?, content_type: r.get(3)?, size: r.get(4)?, sha256: r.get(5)?, original_sha256: r.get(6)?,
                        wrapped_key: r.get(7)?, wrap_nonce: r.get(8)?, nonce: r.get(9)?,
                    }, r.get(10)?))
                })?
                .collect::<Result<_, _>>()?;
            let companies: Vec<(String, String)> = tx.prepare("SELECT id, name FROM ledger_company ORDER BY name, id")?.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
            Ok((tables, audit_rows, att, companies))
        })
        .unwrap();
    let store = fb_core::attachments::storage::Store::for_book(&copy, &key).unwrap();
    let files: Vec<Value> = attachments
        .iter()
        .map(|(id, co, stored, name)| {
            let plain = store.open_read(CompanyId::parse(co).unwrap(), stored).expect("attachment decrypts");
            assert_eq!(sha(&plain), stored.sha256, "attachment bytes match their row");
            json!({"id": id, "name": name, "bytes": plain.len(), "plain_sha256": sha(&plain), "stored": stored.stored_name, "ciphertext_sha256": receipt_files.get(&stored.stored_name)})
        })
        .collect();
    let owner: String = book.read(|tx| tx.query_row("SELECT id FROM fb_user WHERE is_active=1 ORDER BY id LIMIT 1", [], |r| r.get(0))).unwrap();
    let owner = UserId::parse(&owner).unwrap();
    let end = chrono::NaiveDate::from_ymd_opt(2099, 12, 31).unwrap();
    let mut reports = BTreeMap::new();
    for (id, name) in &companies {
        let c = CompanyId::parse(id).unwrap();
        let tb = fb_core::reports::trial_balance(&mut book, owner, c, "2099-12-31").map(|r| format!("{r:?}")).unwrap_or_else(|e| format!("error {e}"));
        let bs = fb_core::reports::financial::balance_sheet(&mut book, owner, c, fb_core::reports::financial::Basis::Accrual, end).map(|r| format!("{r:?}")).unwrap_or_else(|e| format!("error {e}"));
        let pl = fb_core::reports::financial::profit_and_loss(&mut book, owner, c, fb_core::reports::financial::Basis::Accrual, None, end).map(|r| format!("{r:?}")).unwrap_or_else(|e| format!("error {e}"));
        let tb_totals = fb_core::reports::trial_balance(&mut book, owner, c, "2099-12-31").map(|r| format!("debit {} credit {} ({} accounts)", r.debit, r.credit, r.rows.len())).unwrap_or_default();
        reports.insert(name.clone(), json!({"trial_balance_sha256": sha(tb.as_bytes()), "balance_sheet_sha256": sha(bs.as_bytes()), "profit_loss_sha256": sha(pl.as_bytes()), "trial_balance": tb_totals}));
    }
    let report = json!({"book": book_path.display().to_string(), "file_sha256": main_hash, "check_problems": problems,
        "companies": reports, "tables": tables, "attachments": files, "receipt_files": receipt_files, "audit": audit});
    std::fs::write(out, serde_json::to_string_pretty(&report).unwrap()).unwrap();
    println!("report {}: check {} problem(s), {} tables, {} attachments, {} audit events", out.display(), report["check_problems"].as_array().unwrap().len(), report["tables"].as_object().unwrap().len(), files.len(), report["audit"].as_array().unwrap().len());
}

fn walk(p: &Path, out: &mut Vec<PathBuf>) {
    let Ok(meta) = std::fs::symlink_metadata(p) else { return };
    if meta.is_dir() {
        for e in std::fs::read_dir(p).unwrap().flatten() {
            walk(&e.path(), out);
        }
    } else if meta.is_file() {
        out.push(p.to_owned());
    }
}

fn count(hay: &[u8], needle: &[u8]) -> usize {
    if needle.is_empty() || hay.len() < needle.len() {
        return 0;
    }
    hay.windows(needle.len()).filter(|w| *w == needle).count()
}

/// Secrets (app-lock password, master key bytes/hex) must not appear in clear anywhere under the data
/// folder or in the given logs; book content (company names, receipt plaintext) must not appear in clear
/// in any backup-side file (backups, verification notes, folder setting, work folder). Counts only.
fn leakscan(data: &Path, password_file: &str, book: &Path, logs: &[String]) -> i32 {
    let pw = password(password_file);
    let blob: keys::WrappedKey = serde_json::from_str(&std::fs::read_to_string(data.join("book-key.lock.json")).unwrap()).unwrap();
    let key = keys::unwrap(&blob, &pw).expect("unlock");
    let key_hex = zeroize::Zeroizing::new(hex(&key[..]));
    let secrets: Vec<(&str, Vec<u8>)> = vec![
        ("password", pw.as_bytes().to_vec()),
        ("key bytes", key.to_vec()),
        ("key hex", key_hex.as_bytes().to_vec()),
        ("key HEX", key_hex.to_uppercase().into_bytes()),
    ];
    let mut content: Vec<(String, Vec<u8>)> = Vec::new();
    {
        let copy = tempfile::tempdir().unwrap();
        std::fs::copy(book, copy.path().join("book.fbbook")).unwrap();
        let mut b = Book::open(&copy.path().join("book.fbbook"), &key).unwrap();
        let names: Vec<String> = b.read(|tx| tx.prepare("SELECT name FROM ledger_company")?.query_map([], |r| r.get(0))?.collect()).unwrap();
        for n in names {
            content.push((format!("company name ({} bytes)", n.len()), n.into_bytes()));
        }
        content.push(("receipt plaintext".into(), b"% proof receipt".to_vec()));
        content.push(("SQLite header".into(), b"SQLite format 3".to_vec()));
    }
    let mut files = Vec::new();
    walk(data, &mut files);
    let mut failures = 0;
    let mut scanned = (0, 0);
    for f in &files {
        let rel = f.strip_prefix(data).unwrap();
        if rel == Path::new("book-key.lock.json") {
            continue; // the wrap itself: Argon2id + ChaCha20-Poly1305 ciphertext, by design
        }
        let bytes = std::fs::read(f).unwrap();
        scanned.0 += 1;
        for (what, needle) in &secrets {
            let n = count(&bytes, needle);
            if n > 0 {
                println!("LEAK {what} in {}", rel.display());
                failures += 1;
            }
        }
        let backup_side = rel.starts_with("backups") || rel.starts_with("work") || rel == Path::new("backup-folder.json");
        if backup_side {
            scanned.1 += 1;
            for (what, needle) in &content {
                if count(&bytes, needle) > 0 {
                    println!("CLEAR {what} in {}", rel.display());
                    failures += 1;
                }
            }
        }
    }
    for log in logs {
        let bytes = std::fs::read(log).unwrap();
        for (what, needle) in &secrets {
            if count(&bytes, needle) > 0 {
                println!("LEAK {what} in log {log}");
                failures += 1;
            }
        }
    }
    println!("leakscan: {} data files ({} backup-side) and {} logs scanned for {} secrets and {} content markers: {failures} finding(s)", scanned.0, scanned.1, logs.len(), secrets.len(), content.len());
    i32::from(failures != 0)
}

fn compare(a: &Path, b: &Path) -> i32 {
    let load = |p: &Path| -> Value { serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap() };
    let (o, r) = (load(a), load(b));
    let mut failures = 0;
    let mut check = |ok: bool, what: String| {
        println!("{} {what}", if ok { "SAME" } else { "DIFF" });
        if !ok {
            failures += 1;
        }
    };
    for (name, side) in [("original", &o), ("restored", &r)] {
        let n = side["check_problems"].as_array().unwrap().len();
        check(n == 0, format!("book check on {name}: {n} issue(s)"));
    }
    check(o["companies"] == r["companies"], format!("trial balance, balance sheet and P&L for all {} companies", o["companies"].as_object().unwrap().len()));
    for (company, rep) in o["companies"].as_object().unwrap() {
        println!("     {company}: TB {}", rep["trial_balance"].as_str().unwrap_or(""));
    }
    let (ot, rt) = (o["tables"].as_object().unwrap(), r["tables"].as_object().unwrap());
    check(ot.keys().eq(rt.keys()), format!("same {} tables", ot.len()));
    let mut total = 0;
    for (name, t) in ot {
        if name == "ledger_auditevent" {
            continue;
        }
        let same = rt.get(name) == Some(t);
        total += t["rows"].as_u64().unwrap();
        if !same {
            check(false, format!("table {name}: {} vs {} rows", t["rows"], rt.get(name).map_or(Value::Null, |x| x["rows"].clone())));
        }
    }
    check(ot.iter().filter(|(n, _)| *n != "ledger_auditevent").all(|(n, t)| rt.get(n) == Some(t)), format!("row counts and row contents of every other table ({total} rows)"));
    // The audit trail: every restored event is in the original; the original's extra events are only the
    // backup's own create/copy events, written after the snapshot (as Django audits after the dump).
    let (oa, ra) = (&ot["ledger_auditevent"], &rt["ledger_auditevent"]);
    let orows: Vec<&str> = oa["row_hashes"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    let rrows: Vec<&str> = ra["row_hashes"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    check(rrows.iter().all(|h| orows.contains(h)), format!("audit trail present: all {} restored events are original events", rrows.len()));
    let restored_actions: Vec<&Value> = r["audit"].as_array().unwrap().iter().collect();
    let extra: Vec<&Value> = o["audit"].as_array().unwrap().iter().filter(|e| !restored_actions.contains(e)).collect();
    check(extra.iter().all(|e| e["action"].as_str().is_some_and(|a| a.starts_with("backup."))) && orows.len() == rrows.len() + extra.len(),
        format!("original has {} later event(s), all backup.* ({})", extra.len(), extra.iter().map(|e| e["action"].as_str().unwrap_or("")).collect::<Vec<_>>().join(", ")));
    let strip = |v: &Value| v.as_array().unwrap().iter().map(|a| json!([a["id"], a["name"], a["bytes"], a["plain_sha256"], a["ciphertext_sha256"]])).collect::<Vec<_>>();
    check(strip(&o["attachments"]) == strip(&r["attachments"]), format!("all {} attachments byte-identical (decrypted and ciphertext)", o["attachments"].as_array().unwrap().len()));
    check(o["receipt_files"] == r["receipt_files"], format!("receipt folders hold the same {} files", o["receipt_files"].as_object().unwrap().len()));
    println!("{}", if failures == 0 { "INTEGRITY OK" } else { "INTEGRITY FAILED" });
    i32::from(failures != 0)
}
