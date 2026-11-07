//! D9: the frozen attachments panel (`{% attachments_panel %}`) and its add / remove POSTs. The panel lives
//! on each document's detail page; every POST redirects back there with a panel note, as Django's views do.
//! Adding a file (deliberate divergence, DESKTOP-WAVES.md): the page never sends file bytes. The add POST
//! returns a one-time `pick-attachment` ticket to the main process, which shows the native file chooser
//! and hands the chosen path back (`backups::dialog_result`); this module reads the file, size-limited,
//! and stores it with `fb_core::attachments::add` under the open book's key.
use super::*;
use fb_core::attachments as a;
use std::io::Read;
use std::path::Path;

/// `attachments.services.DOCUMENTS`: identity kind -> (fb-core table, detail route).
const DOCUMENTS: [(&str, &str, &str); 13] = [
    ("EST", "sales_estimate", "sales:detail"),
    ("INV", "sales_invoice", "invoices:detail"),
    ("RCP", "sales_receipt", "receipts:detail"),
    ("SR", "sales_receipt", "sales_receipts:detail"),
    ("BILL", "payables_bill", "bills:detail"),
    ("BPAY", "payables_billpayment", "payments:detail"),
    ("DEP", "banking_deposit", "deposits:detail"),
    ("CHK", "checks_check", "checks:detail"),
    ("TRF", "transfers_transfer", "transfers:detail"),
    ("CM", "credits_creditmemo", "credits:detail"),
    ("RFD", "credits_creditrefund", "credits:refund_detail"),
    ("CDP", "sales_receipt", "prepayments:detail"),
    ("CDR", "prepayments_depositrefund", "prepayments:refund_detail"),
];

/// `Attachment.type_label`: the model's `CONTENT_TYPES` display names.
pub(super) fn type_label(t: &str) -> &str {
    match t {
        "application/pdf" => "PDF document",
        "image/png" => "PNG image",
        "image/jpeg" => "JPEG image",
        "image/webp" => "WebP image",
        other => other,
    }
}

/// The panel's view model for a document identity (what the inclusion tag returns).
pub(crate) fn panel(app: &mut App, c: CompanyId, identity: &str, vm: &mut Value) -> AppResult<()> {
    let add_url = fb_templates::django::reverse("attachments:add_document", &[c.to_db(), identity.to_owned()]).map_err(AppError::Render)?;
    panel_for(app, c, &a::Owner::Document(identity.to_owned()), add_url, vm)
}

/// The panel for a manual journal entry (`ledger/journal.html` includes it as `attachments_panel`).
pub(crate) fn journal_panel(app: &mut App, c: CompanyId, journal: fb_core::JournalId, company: &Value) -> AppResult<String> {
    let add_url = fb_templates::django::reverse("attachments:add_journal", &[c.to_db(), journal.to_db()]).map_err(AppError::Render)?;
    let mut vm = json!({"company": company});
    panel_for(app, c, &a::Owner::Journal(journal), add_url, &mut vm)?;
    let mut flat = vm["attachments"].take();
    flat["company"] = vm["company"].take();
    Ok(app.screens.render("attachments/panel.html", &flat)?)
}

fn panel_for(app: &mut App, c: CompanyId, o: &a::Owner, add_url: String, vm: &mut Value) -> AppResult<()> {
    let owner = app.open_mut()?.owner;
    let rows = app.open_mut()?.book.read(|tx| {
        fb_core::services::access(tx, owner, c)?;
        let o = match o {
            a::Owner::Document(identity) => a::document_owner(tx, c, identity)?,
            a::Owner::Journal(id) => a::journal_owner(tx, c, *id)?,
        };
        a::panel_rows(tx, c, &o)
    })?;
    let mut files = vec![];
    let mut removed = vec![];
    for row in rows {
        let id = row["id"].as_str().unwrap_or("").to_owned();
        if row["deleted"] == true {
            removed.push(json!({"pk": id, "original_name": row["original_name"], "deleted_by_name": row["deleted_by_name"], "deleted_at": row["deleted_at"], "delete_reason": row["delete_reason"]}));
            continue;
        }
        let remove = fb_templates::django::reverse("attachments:remove", &[c.to_db(), id.clone()]).map_err(AppError::Render)?;
        files.push(json!({"pk": id, "original_name": row["original_name"], "type_label": type_label(row["content_type"].as_str().unwrap_or("")),
            "size": row["size"], "uploaded_by_name": row["uploaded_by_name"], "uploaded_at": row["uploaded_at"], "remove_token": token(app, &remove)}));
    }
    // sorted((deleted rows), key=(deleted_at, pk), reverse=True)
    removed.sort_by(|x, y| (y["deleted_at"].as_str(), y["pk"].as_str()).cmp(&(x["deleted_at"].as_str(), x["pk"].as_str())));
    let notes: Vec<Value> = std::mem::take(&mut app.attachment_notes).into_iter().map(|(level, message)| json!({"level_tag": level, "message": message})).collect();
    let csrf = token(app, &add_url);
    let full = files.len() as i64 >= a::MAX_PER_OWNER;
    put(vm, "attachments", json!({"add_url": add_url, "csrf_token": csrf, "request_key": uuid::Uuid::new_v4().to_string(), "allowed": "PDF, PNG, JPEG or WebP",
        "max_mb": 10, "max_count": a::MAX_PER_OWNER, "files": files, "removed": removed, "notes": notes, "full": full}));
    Ok(())
}

/// `owner.url`: the owning document's (or journal's) detail page.
fn owner_url(tx: &(impl fb_db::Query + ?Sized), c: CompanyId, o: &a::Owner) -> Result<String, FbError> {
    let (kind, id) = o.key();
    if kind == "journal" {
        return Ok(fb_templates::django::reverse("journal", &[c.to_db(), id]).unwrap_or_default());
    }
    let identity_kind: String = tx.query_row("SELECT kind FROM documents_documentidentity WHERE id=?1 AND company_id=?2", [&id, &c.to_db()], |r| r.get(0))?;
    let Some((_, table, route)) = DOCUMENTS.iter().find(|(k, _, _)| *k == identity_kind) else { return Ok(String::new()) };
    let doc: String = tx.query_row(&format!("SELECT id FROM {table} WHERE identity_id=?1 AND company_id=?2"), [&id, &c.to_db()], |r| r.get(0))?;
    Ok(fb_templates::django::reverse(route, &[c.to_db(), doc]).unwrap_or_default())
}

/// What an add POST is waiting for: the file the user picks in the native chooser.
pub(crate) struct Pick {
    company: CompanyId,
    owner: a::Owner,
    request_key: String,
}

/// add_document / add_journal: scoped 404 for an owner outside the company (as Django's `_owner`), then
/// a one-time ticket for the main process's file chooser. Nothing is read or stored yet.
fn pick(app: &mut App, r: &Route, c: CompanyId, p: Posted<'_>) -> AppResult<Outcome> {
    let actor = app.open_mut()?.owner;
    let owner = app.open_mut()?.book.read(|tx| {
        fb_core::services::access(tx, actor, c)?;
        if r.name == "attachments:add_journal" {
            let id = r.args.get("journal_id").and_then(|s| fb_core::JournalId::parse(s)).ok_or_else(|| FbError::denied("No such journal."))?;
            a::journal_owner(tx, c, id)
        } else {
            a::document_owner(tx, c, r.args.get("document_id").map_or("", String::as_str))
        }
    }).map_err(|_| AppError::NotFound("No such document.".into()))?;
    let request_key = get(p, "request_key").unwrap_or("").to_owned();
    let ticket = app.backups.mint_pick(Pick { company: c, owner, request_key })?;
    Ok(Outcome::Done { data: json!({"desktop": "pick-attachment", "ticket": ticket}) })
}

/// The chosen file: a regular file of at most `max` bytes, read with a bounded reader (it may grow).
fn read_limited(path: &Path, max: u64) -> Result<Vec<u8>, String> {
    let too_large = || format!("The file is larger than {} MB, so it was not attached.", max / (1024 * 1024));
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // A FIFO must not wait for a writer before we can inspect its handle. Do not
        // substitute a path metadata check: the path can be replaced before open.
        // O_NOCTTY also avoids acquiring a terminal before rejecting a non-file.
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY);
    }
    let file = options.open(path).map_err(|e| format!("The file could not be opened ({e}), so it was not attached."))?;
    let meta = file.metadata().map_err(|e| format!("The file could not be read ({e}), so it was not attached."))?;
    if !meta.is_file() {
        return Err("Choose a file, not a folder.".into());
    }
    if meta.len() > max {
        return Err(too_large());
    }
    let mut data = Vec::with_capacity(usize::try_from(meta.len()).unwrap_or(0));
    file.take(max + 1).read_to_end(&mut data).map_err(|e| format!("The file could not be read ({e}), so it was not attached."))?;
    if data.len() as u64 > max {
        return Err(too_large());
    }
    Ok(data)
}

/// After the chooser (`chosen` is empty when it was cancelled): `attachments.views._add` on the picked file,
/// then back to the owner's page with the panel note. A cancel is silent, like closing a browser chooser.
pub(crate) fn picked(app: &mut App, pick: Pick, chosen: Option<&Path>, book: &Path, key: &[u8; 32]) -> AppResult<Outcome> {
    let Pick { company: c, owner: o, request_key } = pick;
    if let Some(path) = chosen {
        let limits = a::Limits::default();
        let max = u64::from(limits.max_mb) * 1024 * 1024;
        match read_limited(path, max) {
            Err(message) => app.attachment_notes.push(("error".into(), message)),
            Ok(data) => {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let open = app.open_mut()?;
                let actor = open.owner;
                let result = a::storage::Store::for_book(book, key)
                    .and_then(|files| a::add(&mut open.book, &files, actor, c, &o, &data, &name, &request_key, &limits));
                match result {
                    Ok((row, created)) => {
                        if created {
                            app.attachment_notes.push(("success".into(), format!("Attached {}.", row["original_name"].as_str().unwrap_or(""))));
                        }
                    }
                    // `storage.StoreError` in Django: the encrypted store itself failed.
                    Err(FbError::Refused(m)) if m.starts_with("Attachment store:") => app.attachment_notes.push((
                        "error".into(),
                        "The file could not be saved in this book's attachment folder. Nothing was attached.".into(),
                    )),
                    Err(FbError::Refused(m)) => app.attachment_notes.push(("error".into(), m)),
                    Err(FbError::Validation(m)) => app.attachment_notes.push(("error".into(), m.join(" "))),
                    Err(e) => return Err(e.into()),
                }
            }
        }
    }
    let url = app.open_mut()?.book.read(|tx| owner_url(tx, c, &o))?;
    Ok(Outcome::Redirect { location: format!("{url}#attachments") })
}

pub(super) fn submit(app: &mut App, r: &Route, p: Posted<'_>) -> AppResult<Outcome> {
    let c = arg_company(r)?;
    if r.name != "attachments:remove" {
        return pick(app, r, c, p);
    }
    let id = r.args.get("attachment_id").cloned().unwrap_or_default();
    let owner = app.open_mut()?.owner;
    // Scoped 404 before the method / write, like Django's get_object_or_404 + owner_of.
    let o = app.open_mut()?.book.read(|tx| {
        fb_core::services::access(tx, owner, c)?;
        let row = a::get(tx, c, &id)?;
        a::owner_of(&row)
    }).map_err(|_| AppError::NotFound("No such attachment.".into()))?;
    let reason = get(p, "reason").unwrap_or("");
    let result = a::remove(&mut app.open_mut()?.book, owner, c, &id, reason);
    match result {
        Ok((row, _, changed)) => {
            if changed {
                app.attachment_notes.push(("success".into(), format!("Removed {}. It stays in the audit history and in backups.", row["original_name"].as_str().unwrap_or(""))));
            }
        }
        Err(FbError::Refused(m)) => app.attachment_notes.push(("error".into(), m)),
        Err(FbError::Validation(m)) => app.attachment_notes.push(("error".into(), m.join(" "))),
        Err(e) => return Err(e.into()),
    }
    let url = app.open_mut()?.book.read(|tx| owner_url(tx, c, &o))?;
    Ok(Outcome::Redirect { location: format!("{url}#attachments") })
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::read_limited;
    use std::path::Path;
    use std::process::{Child, Command};
    use std::time::{Duration, Instant};

    // A blocked read must fail the parent test and be reaped without opening a FIFO writer.
    struct OwnedChild(Child);
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn fifo(path: &Path) {
        assert!(Command::new("mkfifo").arg("--").arg(path).status().unwrap().success());
    }

    fn bounded_non_regular(case: &str) {
        const CHILD_CASE: &str = "FB_ATTACH_READ_CHILD_CASE";
        const CHILD_PATH: &str = "FB_ATTACH_READ_CHILD_PATH";
        const CHILD_READY: &str = "FB_ATTACH_READ_CHILD_READY";
        if std::env::var(CHILD_CASE).as_deref() == Ok(case) {
            let path = std::path::PathBuf::from(std::env::var_os(CHILD_PATH).unwrap());
            if case == "replacement" {
                // A successful preliminary inspection cannot authorize the later open:
                // another process can replace that path with a FIFO in between.
                assert!(std::fs::metadata(&path).unwrap().is_file());
                std::fs::remove_file(&path).unwrap();
                fifo(&path);
            }
            std::fs::write(std::env::var_os(CHILD_READY).unwrap(), b"ready").unwrap();
            assert!(read_limited(&path, 1024).is_err(), "non-regular input was accepted");
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("picked.pdf");
        let ready = dir.path().join("ready");
        if case == "replacement" {
            std::fs::write(&path, b"%PDF-1.4\n").unwrap();
        } else {
            fifo(&path);
        }
        let name = if case == "replacement" {
            "screens::d9::attachments::tests::regular_path_replaced_by_fifo_returns_promptly"
        } else {
            "screens::d9::attachments::tests::no_writer_fifo_returns_promptly"
        };
        let mut child = OwnedChild(Command::new(std::env::current_exe().unwrap())
            .args(["--exact", name, "--nocapture"])
            .env(CHILD_CASE, case)
            .env(CHILD_PATH, &path)
            .env(CHILD_READY, &ready)
            .spawn().unwrap());
        let mut deadline = Instant::now() + Duration::from_secs(120);
        let mut started = false;
        loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                assert!(status.success(), "{case} child failed: {status}");
                break;
            }
            if !started && ready.exists() {
                started = true;
                deadline = Instant::now() + Duration::from_secs(3);
            }
            assert!(Instant::now() < deadline, "{case} child exceeded deadline (read started: {started})");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn no_writer_fifo_returns_promptly() {
        bounded_non_regular("fifo");
    }

    #[test]
    fn regular_path_replaced_by_fifo_returns_promptly() {
        bounded_non_regular("replacement");
    }

    #[test]
    fn regular_file_size_and_open_guards_remain() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("picked.pdf");
        let pdf = b"%PDF-1.4\n";
        std::fs::write(&path, pdf).unwrap();
        assert_eq!(read_limited(&path, pdf.len() as u64).unwrap(), pdf);
        assert!(read_limited(&path, pdf.len() as u64 - 1).unwrap_err().contains("larger"));
        assert!(read_limited(dir.path(), 1024).is_err());
        assert!(read_limited(&dir.path().join("missing.pdf"), 1024).unwrap_err().contains("could not be opened"));
    }
}
