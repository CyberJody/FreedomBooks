//! DESKTOP-BACKUPS: the Backups page (`backups.views.page` / `create` / `download`) and the desktop host
//! for `fb_core::backups`. Everything that touches a backup runs here, in the Rust host; the renderer only
//! posts the page's forms. A step that needs a native dialog returns a one-time ticket to the main process
//! (never to the page), which shows the dialog and hands the chosen path back through `dialog_result`.
//!
//! Desktop divergences (DESKTOP-WAVES.md "Deliberate divergences"): restore is a desktop flow into a NEW
//! book (Django only names a server command), backups use the book key, built-in or own password (no server
//! passphrase), there is no web-request size limit, and "Choose folder…" replaces the server setting.
use super::d2::{draw, put, token};
use super::*;
use crate::keys::MasterKey;
use fb_core::backups::{self as core, BackupOptions, RestoreOptions, retention};
use std::path::{Path, PathBuf};

const PAGE_ROWS: usize = 20;
const NAME_MAX: usize = 60;
const TICKET_SECONDS: u64 = 15 * 60;

enum Waiting {
    Folder { company: CompanyId },
    Restore { company: CompanyId, name: String },
    Copy { company: CompanyId, file: PathBuf },
    /// J-ATTACH: an attachments add POST; the native file chooser shares this one-dialog ticket.
    Attach(super::d9::attachments::Pick),
}

/// What the host knows: the app data folder, the open book and (while unlocked) a copy of its key.
#[derive(Default)]
pub struct Host {
    root: Option<PathBuf>,
    book: Option<PathBuf>,
    key: Option<MasterKey>,
    messages: Vec<(&'static str, String, Option<String>)>,
    restored: Option<(String, PathBuf, MasterKey)>,
    pub(crate) restore_pending: Option<(CompanyId, String, PathBuf)>,
    viewing: Option<String>,
    ticket: Option<(String, std::time::Instant, Waiting)>,
}

impl Host {
    /// The app data folder (Electron userData). Books live in `books/`, backups default to `backups/`.
    pub fn configure(&mut self, root: &Path) -> AppResult<()> {
        if !root.is_absolute() {
            return Err(AppError::Invalid("The data folder must be an absolute path.".into()));
        }
        self.root = Some(root.to_owned());
        Ok(())
    }
    pub(crate) fn opened(&mut self, book: &Path, key: &MasterKey) {
        self.book = Some(book.to_owned());
        self.key = Some(MasterKey::new(**key));
    }
    pub(crate) fn locked(&mut self) {
        self.book = None;
        self.key = None;
        self.messages.clear();
        self.restored = None;
        self.restore_pending = None;
        self.viewing = None;
        self.ticket = None;
    }
    /// The banner text while a restored book (not the main one) is open.
    pub fn viewing_restored(&self) -> Option<&str> {
        self.viewing.as_deref()
    }
    pub(crate) fn root(&self) -> AppResult<&Path> {
        self.root.as_deref().ok_or_else(|| AppError::Invalid("Backups are not set up in this window.".into()))
    }
    pub(crate) fn books(&self) -> AppResult<PathBuf> {
        Ok(self.root()?.join("books"))
    }
    pub(crate) fn work(&self) -> AppResult<PathBuf> {
        Ok(self.root()?.join("work"))
    }
    fn settings(&self) -> AppResult<PathBuf> {
        Ok(self.root()?.join("backup-folder.json"))
    }
    /// The chosen backup folder, or `<data>/backups`.
    pub fn folder(&self) -> AppResult<PathBuf> {
        let chosen = fs_read_small(&self.settings()?)
            .and_then(|raw| fb_db::parse_unique_json(&raw).ok())
            .and_then(|v: Value| v["folder"].as_str().map(PathBuf::from))
            .filter(|p| p.is_absolute());
        Ok(match chosen {
            Some(p) => p,
            None => self.root()?.join("backups"),
        })
    }
    pub(crate) fn open_book(&self) -> AppResult<(&Path, &MasterKey)> {
        match (&self.book, &self.key) {
            (Some(b), Some(k)) => Ok((b, k)),
            _ => Err(AppError::Locked),
        }
    }
    fn receipts(&self) -> AppResult<PathBuf> {
        Ok(parent(self.open_book()?.0).join("receipts"))
    }
    /// Every live path a restore must not touch (L4-BACKUPS: the bridge supplies all configured paths).
    fn protected(&self) -> AppResult<Vec<PathBuf>> {
        let (book, _) = self.open_book()?;
        let mut out = vec![book.to_owned(), sidecar(book, "-wal"), sidecar(book, "-shm"), self.receipts()?, self.folder()?, self.work()?];
        let main = self.books()?.join("default.fbbook");
        if main != book {
            out.extend([main.clone(), sidecar(&main, "-wal"), sidecar(&main, "-shm"), self.books()?.join("receipts")]);
        }
        Ok(out)
    }
    fn mint(&mut self, waiting: Waiting) -> AppResult<String> {
        let ticket = crate::keys::to_hex(&crate::keys::random_bytes::<32>().map_err(|e| AppError::Invalid(e.to_string()))?);
        self.ticket = Some((ticket.clone(), std::time::Instant::now(), waiting));
        Ok(ticket)
    }
    pub(crate) fn mint_pick(&mut self, pick: super::d9::attachments::Pick) -> AppResult<String> {
        self.open_book()?;
        self.mint(Waiting::Attach(pick))
    }
    fn say(&mut self, level: &'static str, text: impl Into<String>) {
        self.messages.push((level, text.into(), None));
    }
    /// A refused restore: the plain sentence, and the technical reason in a small line under it. An
    /// earlier "The restored book … is ready" panel goes, so it cannot be mistaken for this attempt.
    fn refuse_restore(&mut self, refusal: Refusal) {
        self.restored = None;
        self.messages.push(("error", format!("Not restored: {}", refusal.text), refusal.detail));
    }
}

fn parent(p: &Path) -> &Path {
    p.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."))
}

fn sidecar(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

fn fs_read_small(p: &Path) -> Option<String> {
    let meta = std::fs::symlink_metadata(p).ok()?;
    (meta.is_file() && meta.len() < 64 * 1024).then(|| std::fs::read_to_string(p).ok()).flatten()
}

/// Create (or tighten) a folder only this user can read, like Django's "the next backup sets 0700".
fn private_dir(p: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        builder.mode(0o700).create(p)?;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700))
    }
    #[cfg(not(unix))]
    builder.create(p)
}

fn new_private_file(p: &Path) -> std::io::Result<std::fs::File> {
    // Owner-only before any byte is written (create_new never opens an existing file or link).
    let file = std::fs::File::create_new(p)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(file)
}

fn mode_of(meta: &std::fs::Metadata) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Some(meta.permissions().mode() & 0o777)
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        None
    }
}

fn overlaps(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

/// Why a backup folder overlapping the live folders is refused, said the right way round: a folder inside
/// the books (or work) folder, or one that contains them (such as the home folder or the data folder).
fn folder_refusal(folder: &Path, live: &[PathBuf]) -> Option<&'static str> {
    if live.iter().any(|l| folder.starts_with(l)) {
        Some("backups cannot go inside the books folder. Choose a folder outside it.")
    } else if live.iter().any(|l| l.starts_with(folder)) {
        Some("this folder contains the books folder, and backups cannot contain the books folder. Choose a folder outside the books folder that does not hold it, such as a separate Backups folder.")
    } else {
        None
    }
}

/// A refused restore, in plain words with the details kept for a small line.
#[derive(Debug)]
struct Refusal {
    text: String,
    detail: Option<String>,
}

impl From<String> for Refusal {
    fn from(text: String) -> Self {
        Refusal { text, detail: None }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)?;
        if let Some(d) = &self.detail {
            write!(f, " ({d})")?;
        }
        Ok(())
    }
}

pub(super) fn is_route(name: &str) -> bool {
    matches!(name, "backups:page" | "backups:create" | "backups:download")
}

/// `backups.views._company`: a member who is staff; everyone else gets 404 (the page is not revealed).
fn company(app: &mut App, route: &Route) -> AppResult<CompanyId> {
    let c = arg_company(route)?;
    let open = app.open_mut()?;
    let owner = open.owner;
    open.book.read(|tx| fb_core::services::access(tx, owner, c))?;
    if !open.staff {
        return Err(AppError::NotFound("backups".into()));
    }
    Ok(c)
}

fn created_iso(name: &str) -> Option<String> {
    let stamp = name.strip_suffix(".fbbackup")?.rsplit_once('-')?.1;
    let t = chrono::NaiveDateTime::parse_from_str(stamp, "%Y%m%dT%H%M%SZ").ok()?;
    Some(t.format("%Y-%m-%dT%H:%M:%SZ").to_string())
}

fn row(path: &Path) -> Value {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_owned();
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    // The note counts only when it still binds this exact file (size, mtime and full hash).
    let note = retention::verified_note(path)
        .then(|| fs_read_small(&retention::sidecar(path)))
        .flatten()
        .and_then(|raw| fb_db::parse_unique_json(&raw).ok())
        .unwrap_or(Value::Null);
    let verified_at = note["verified_at"].as_str().filter(|s| chrono::DateTime::parse_from_rfc3339(s).is_ok());
    // Django's `_row`: a file whose header cannot be read is marked Unreadable (and not encrypted). Only
    // own-password backups count as encrypted: the built-in key is public.
    let header = core::read_header(path).ok();
    let readable = header.is_some();
    let encrypted = header.is_some_and(|h| h["book_keys"]["mode"] != "built-in" && (h["database"] == "SQLCipher" || h["encrypted"] == true));
    json!({"name": name, "size": size, "created": created_iso(&name), "encrypted": encrypted, "readable": readable,
           "verified_at": verified_at, "needs_migrate": note["needs_migrate"] == true})
}

fn book_bytes(host: &Host) -> AppResult<u64> {
    let (book, _) = host.open_book()?;
    let mut total = 0;
    for p in [book.to_owned(), sidecar(book, "-wal")] {
        total += std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    }
    if let Ok(dir) = std::fs::read_dir(host.receipts()?) {
        for entry in dir.flatten() {
            total += entry.metadata().ok().filter(|m| m.is_file()).map_or(0, |m| m.len());
        }
    }
    Ok(total)
}

/// `backups.views.status`.
fn status(host: &Host) -> AppResult<Value> {
    let directory = host.folder()?;
    let files = retention::backup_files(&directory, None).map_err(|e| AppError::Invalid(e.to_string()))?;
    let rows: Vec<Value> = files.iter().take(PAGE_ROWS).map(|p| row(p)).collect();
    let verified = rows.iter().find(|r| !r["verified_at"].is_null()).cloned();
    let meta = std::fs::symlink_metadata(&directory).ok().filter(|m| m.is_dir());
    let mode = meta.as_ref().and_then(mode_of);
    let writable = meta.as_ref().is_none_or(|m| !m.permissions().readonly());
    Ok(json!({
        "directory": directory.display().to_string(), "exists": meta.is_some(), "writable": writable,
        "rows": rows, "count": files.len(), "more": files.len() > PAGE_ROWS,
        "total_bytes": files.iter().map(|p| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)).sum::<u64>(),
        "last": rows.first(), "last_verified": verified, "db_bytes": book_bytes(host)?, "web_allowed": true,
        "folder_open": mode.is_some_and(|m| m & 0o077 != 0), "folder_mode": mode.map(|m| format!("{m:04o}")).unwrap_or_default(),
    }))
}

pub(super) fn render(app: &mut App, route: &Route, params: &[(String, String)], path: &str) -> AppResult<(String, u16)> {
    let c = company(app, route)?;
    if route.name != "backups:page" {
        // GET of create (405 in Django) or a download link opened directly: the page handles both.
        return Ok((url("backups:page", &[&c.to_db()]), 302));
    }
    let mut vm = serde_json::to_value(shell(app, Some(c), &Current { route: &route.name, kind: None })?)
        .map_err(|e| AppError::Invalid(e.to_string()))?;
    crate::protection::checkpoint("backups-before-status");
    let st = status(&app.backups)?;
    if let Value::Object(map) = st {
        for (k, v) in map {
            vm[k] = v;
        }
    }
    let created = get(params, "created").unwrap_or("");
    let shown = vm["rows"].as_array().is_some_and(|rows| rows.iter().any(|r| r["name"] == created));
    put(&mut vm, "created", if shown { created } else { "" });
    let messages: Vec<Value> = app.backups.messages.drain(..).map(|(level, text, detail)| json!({"level_tag": level, "text": text, "detail": detail})).collect();
    put(&mut vm, "messages", messages);
    put(&mut vm, "restored", app.backups.restored.as_ref().map(|r| r.0.clone()));
    put(&mut vm, "new_book", "");
    put(&mut vm, "builtin", app.builtin());
    put(&mut vm, "portable", app.protection.is_some());
    put(&mut vm, "recovery_unsaved", app.protection.as_ref().is_some_and(|r| !app.builtin() && !r.recovery_saved));
    let create = url("backups:create", &[&c.to_db()]);
    put(&mut vm, "csrf_token", token(app, &create));
    put(&mut vm, "folder_token", token(app, path));
    put(&mut vm, "restore_token", token(app, path));
    put(&mut vm, "open_token", token(app, path));
    Ok((draw(app, "backups/page.html", &vm)?, 200))
}

fn page(c: CompanyId, created: Option<&str>) -> Outcome {
    let base = url("backups:page", &[&c.to_db()]);
    Outcome::Redirect { location: match created {
        Some(name) => format!("{base}?{}", urlencode(&[("created".into(), name.into())])),
        None => base,
    } }
}

fn dialog(kind: &str, ticket: String, extra: Value) -> Outcome {
    let mut data = json!({"desktop": kind, "ticket": ticket});
    if let (Value::Object(d), Value::Object(e)) = (&mut data, extra) {
        d.extend(e);
    }
    Outcome::Done { data }
}

pub(super) fn submit(app: &mut App, route: &Route, fields: &[(String, String)]) -> AppResult<Outcome> {
    let c = company(app, route)?;
    match route.name.as_str() {
        "backups:create" => {
            match back_up(app, c) {
                Ok(name) => Ok(page(c, Some(&name))),
                Err(message) => {
                    app.backups.say("error", format!("Backup not created: {message}"));
                    Ok(page(c, None))
                }
            }
        }
        "backups:page" => match get(fields, "action") {
            Some("folder") => {
                let folder = app.backups.folder()?.display().to_string();
                let ticket = app.backups.mint(Waiting::Folder { company: c })?;
                Ok(dialog("choose-backup-folder", ticket, json!({"folder": folder})))
            }
            Some("restore") => {
                let raw = get(fields, "new_book").unwrap_or("");
                match book_name(raw).and_then(|(name, slug)| {
                    name_free(&app.backups.books()?.join(&slug), &name)?;
                    Ok(name)
                }) {
                    Ok(name) => {
                        let folder = app.backups.folder()?.display().to_string();
                        let ticket = app.backups.mint(Waiting::Restore { company: c, name })?;
                        Ok(dialog("pick-backup", ticket, json!({"folder": folder})))
                    }
                    Err(e) => {
                        app.backups.refuse_restore(e.to_string().into());
                        Ok(page(c, None))
                    }
                }
            }
            Some("open") => open_restored(app),
            _ => Err(AppError::Invalid("Unknown backups action.".into())),
        },
        _ => Err(AppError::NotFound("no form here".into())),
    }
}

/// The renderer's Download link (`backups.views.download`): validate the name, then ask for a destination.
pub(crate) fn export(app: &mut App, c: CompanyId, name: &str) -> AppResult<Outcome> {
    let route = Route { name: "backups:page".into(), args: BTreeMap::from([("company_id".into(), c.to_db())]) };
    company(app, &route)?;
    let folder = app.backups.folder()?;
    let files = retention::backup_files(&folder, None).map_err(|e| AppError::Invalid(e.to_string()))?;
    let file = files.into_iter().find(|p| p.file_name().and_then(|n| n.to_str()) == Some(name)).ok_or_else(|| AppError::NotFound("No such backup.".into()))?;
    let ticket = app.backups.mint(Waiting::Copy { company: c, file })?;
    Ok(dialog("save-backup", ticket, json!({"filename": name})))
}

/// The restored book's folder must not exist. A folder left by a restore that crashed (only dead-pid
/// partial copies and their half-written receipts) is cleared first and the name is free again; a
/// restore that may still be running (live pid), a real book or anything else is refused.
fn name_free(dir: &Path, name: &str) -> AppResult<()> {
    if std::fs::symlink_metadata(dir).is_err() {
        return Ok(());
    }
    let cleared = core::cleanup::clear_interrupted_restore(dir).unwrap_or(false);
    if cleared && std::fs::symlink_metadata(dir).is_err() {
        return Ok(());
    }
    let pending = std::fs::read_dir(dir).is_ok_and(|entries| {
        let names: Vec<String> = entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        !names.is_empty() && names.iter().all(|n| n.starts_with(".fb-backup.") || n == "receipts") && names.iter().any(|n| n.starts_with(".fb-backup."))
    });
    if pending {
        return Err(AppError::Invalid(format!("a restore into “{name}” is still running or did not finish. Choose another name.")));
    }
    Err(AppError::Invalid(format!("A book named “{name}” already exists here. Choose another name.")))
}

/// A Django-style display name and the folder/file stem made from it (letters, digits, dashes).
fn book_name(raw: &str) -> AppResult<(String, String)> {
    fb_core::py::clean_free_text(raw).map_err(|e| AppError::Invalid(e.message()))?;
    let name = raw.trim();
    if name.is_empty() || name.chars().count() > NAME_MAX {
        return Err(AppError::Invalid(format!("Enter a name for the restored book (up to {NAME_MAX} characters).")));
    }
    let mut slug = String::new();
    for ch in name.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-').to_owned();
    if slug.is_empty() {
        return Err(AppError::Invalid("Use at least one letter or digit in the name.".into()));
    }
    // Reserved names inside the books folder: the main book's own files and folders.
    if ["default", "receipts", "work"].contains(&slug.as_str()) {
        return Err(AppError::Invalid(format!("“{name}” is reserved. Choose another name.")));
    }
    Ok((name.to_owned(), format!("restored-{slug}")))
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn audit(app: &mut App, c: CompanyId, action: &str, name: &str, detail: Value) -> AppResult<()> {
    let open = app.open_mut()?;
    Ok(core::audit::record(&mut open.book, open.owner, c, action, name, detail)?)
}

/// `backups.views.create`, then `verify_backup` on the written file (recorded beside it).
pub(crate) fn verified_backup(app: &mut App, c: CompanyId) -> AppResult<PathBuf> {
    let name = back_up(app, c).map_err(AppError::Invalid)?;
    let file = app.backups.folder()?.join(name);
    if !retention::verified_note(&file) { return Err(AppError::Invalid("The safety backup did not verify. Password protection was not changed.".into())); }
    Ok(file)
}

fn back_up(app: &mut App, c: CompanyId) -> Result<String, String> {
    let receipt_key = app.receipt_key().map_err(|e| e.to_string())?;
    let envelope = app.protection.as_ref().map(|r| r.keys.clone());
    let own = !app.builtin();
    let (book, key) = app.backups.open_book().map_err(|e| e.to_string())?;
    let (book, key) = (book.to_owned(), MasterKey::new(**key));
    let open = app.open.as_ref().ok_or("The books are locked.")?;
    let result = write_copy(&app.backups, &open.book, &book, &key, &receipt_key, envelope.as_ref(), None)?;
    let name = result.path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_owned();
    let bytes = std::fs::metadata(&result.path).map(|m| m.len()).unwrap_or(0);
    let detail = json!({"file": name, "bytes": bytes, "encrypted": own, "book_sha256": result.manifest.book_sha256,
                        "companies": result.manifest.contents.companies.len(), "attachments": result.manifest.attachments.len()});
    audit(app, c, "backup.create", &name, detail).map_err(|e| e.to_string())?;
    app.backups.say("success", format!("Backup created: {name} ({bytes} bytes)."));
    match verify_copy(&app.backups, &result.path, &key, &receipt_key) {
        Ok(_) => app.backups.say("success", "Verified: the backup opens and matches these books."),
        Err(e) => return Err(format!("The backup was written but did not verify: {e} Make another backup before relying on it.")),
    }
    Ok(name)
}

/// Write a backup of `source` into the backup folder, named after the book (plus `suffix`).
fn write_copy(host: &Host, source: &dyn fb_db::SnapshotSource, book: &Path, key: &MasterKey, receipt_key: &MasterKey,
              envelope: Option<&core::portable::KeyEnvelope>, suffix: Option<&str>) -> Result<core::BackupResult, String> {
    let folder = host.folder().map_err(|e| e.to_string())?;
    let work = host.work().map_err(|e| e.to_string())?;
    let books = host.books().map_err(|e| e.to_string())?;
    if let Some(reason) = folder_refusal(&folder, &[books, parent(book).to_owned(), work.clone()]) {
        return Err(reason.replacen("backups cannot go inside", "the backup folder is inside", 1).replacen("this folder contains", "the backup folder contains", 1));
    }
    private_dir(&folder).map_err(|e| format!("the backup folder could not be prepared ({e})."))?;
    private_dir(&work).map_err(|e| format!("the work folder could not be prepared ({e})."))?;
    let stem = book.file_stem().and_then(|s| s.to_str()).unwrap_or("book");
    let mut label = match suffix {
        // Keep the suffix readable: a plain stem, short enough that the label is never hashed.
        Some(suffix) => format!("{}-{suffix}", stem.chars().filter(|c| c.is_ascii_alphanumeric() || "_-".contains(*c)).take(30).collect::<String>()),
        None => retention::safe_db_label(stem),
    };
    let created_at = now();
    let mut output = folder.join(retention::filename(&label, &created_at).map_err(|e| e.to_string())?);
    if output.exists() {
        label = format!("{label}-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
        output = folder.join(retention::filename(&label, &created_at).map_err(|e| e.to_string())?);
    }
    let receipts = parent(book).join("receipts");
    let receipts = receipts.is_dir().then_some(receipts);
    let verifier = core::attachment_receipt_verifier(receipt_key);
    let options = BackupOptions {
        output: &output, work_root: &work, receipts_root: receipts.as_deref(), verify_receipt: Some(&verifier),
        label: &label, app_version: env!("CARGO_PKG_VERSION"), created_at: &created_at, passphrase: None, keep: None,
    };
    core::create_backup_from(source, key, &options, envelope).map_err(|e| e.to_string())
}

/// `verify_backup` on a written file; success is recorded beside it.
fn verify_copy(host: &Host, file: &Path, key: &MasterKey, receipt_key: &MasterKey) -> Result<(), String> {
    let work = host.work().map_err(|e| e.to_string())?;
    let verifier = core::attachment_receipt_verifier(receipt_key);
    core::verify_backup_with_receipts(file, key, &work, None, true, Some(&verifier)).map(|_| ()).map_err(|e| e.to_string())
}

/// The copy made before an upgrade: a normal backup of the not-yet-migrated book, in the backup
/// folder, verified before anything changes. A copy that fails verification is removed.
pub(crate) fn verified_copy(host: &Host, source: &fb_db::Unmigrated, book: &Path, key: &MasterKey, receipt_key: &MasterKey,
                            envelope: Option<&core::portable::KeyEnvelope>, suffix: &str) -> Result<PathBuf, String> {
    let result = write_copy(host, source, book, key, receipt_key, envelope, Some(suffix))?;
    crate::protection::checkpoint("upgrade-copy-written");
    let verified = if crate::protection::fault("upgrade-verify") { Err("the copy did not match these books.".into()) }
        else { verify_copy(host, &result.path, key, receipt_key) };
    if let Err(e) = verified.and_then(|_| if retention::verified_note(&result.path) { Ok(()) } else { Err("the copy's check was not recorded.".into()) }) {
        let _ = std::fs::remove_file(retention::sidecar(&result.path));
        let _ = std::fs::remove_file(&result.path);
        return Err(format!("the copy did not verify ({e})"));
    }
    Ok(result.path)
}

/// The main process hands back the path the user picked (empty when the dialog was cancelled).
pub fn dialog_result(app: &mut App, ticket: &str, chosen: &str) -> AppResult<Outcome> {
    let Some((expected, at, waiting)) = app.backups.ticket.take() else {
        return Err(AppError::Invalid("This action has expired. Open the screen again and retry.".into()));
    };
    if expected.len() != ticket.len() || expected != ticket || at.elapsed().as_secs() > TICKET_SECONDS {
        return Err(AppError::Invalid("This action has expired. Open the screen again and retry.".into()));
    }
    let chosen = (!chosen.is_empty()).then(|| PathBuf::from(chosen));
    if chosen.as_ref().is_some_and(|p| !p.is_absolute() || p.as_os_str().len() > 4096) {
        return Err(AppError::Invalid("Choose a file or folder on this computer.".into()));
    }
    match waiting {
        Waiting::Attach(pick) => {
            let (book, key) = app.backups.open_book()?;
            let (book, key) = (book.to_owned(), MasterKey::new(**key));
            super::d9::attachments::picked(app, pick, chosen.as_deref(), &book, &key)
        }
        Waiting::Folder { company } => {
            if let Some(folder) = chosen {
                match set_folder(app, &folder) {
                    Ok(()) => app.backups.say("success", format!("New backups will be saved in {}.", folder.display())),
                    Err(e) => app.backups.say("error", format!("Folder not changed: {e}")),
                }
            }
            Ok(page(company, None))
        }
        Waiting::Restore { company, name } => {
            if let Some(file) = chosen {
                let keys = archive_keys(&file).map_err(|e| AppError::Invalid(e.to_string()))?;
                if keys.as_ref().is_some_and(|k| k.mode == core::portable::Mode::OwnPassword && k.receipts(app.backups.open_book().expect("open book").1).is_err()) {
                    app.backups.restore_pending = Some((company, name, file));
                    return Ok(Outcome::Redirect { location: "/settings/restore-backup/".into() });
                }
                match restore(app, &file, &name) {
                    Ok(()) => app.backups.say("success", format!("Restored “{name}” from {} into a new book. Your open book was not changed.", file.file_name().and_then(|n| n.to_str()).unwrap_or("the backup"))),
                    Err(e) => app.backups.refuse_restore(e),
                }
            }
            Ok(page(company, None))
        }
        Waiting::Copy { company, file } => {
            if let Some(dest) = chosen {
                match save_copy(&file, &dest) {
                    Ok(bytes) => {
                        let name = file.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_owned();
                        audit(app, company, "backup.download", &name, json!({"file": name, "bytes": bytes}))?;
                        app.backups.say("success", format!("Saved a copy of {name} to {}.", dest.display()));
                    }
                    Err(e) => app.backups.say("error", format!("Copy not saved: {e}")),
                }
            }
            Ok(page(company, None))
        }
    }
}

fn set_folder(app: &mut App, folder: &Path) -> AppResult<()> {
    let host = &app.backups;
    let refused = |m: &str| Err(AppError::Invalid(m.into()));
    if std::fs::symlink_metadata(folder).is_ok_and(|m| !m.is_dir()) {
        return refused("that is not a folder.");
    }
    let (book, _) = host.open_book()?;
    if let Some(reason) = folder_refusal(folder, &[host.books()?, parent(book).to_owned(), host.work()?]) {
        return refused(reason);
    }
    let settings = host.settings()?;
    let tmp = sidecar(&settings, ".tmp");
    {
        use std::io::Write;
        let _ = std::fs::remove_file(&tmp);
        let mut f = new_private_file(&tmp).map_err(|e| AppError::Invalid(e.to_string()))?;
        f.write_all(json!({"folder": folder.display().to_string()}).to_string().as_bytes()).and_then(|()| f.sync_all()).map_err(|e| AppError::Invalid(e.to_string()))?;
    }
    std::fs::rename(&tmp, &settings).map_err(|e| AppError::Invalid(e.to_string()))
}

fn save_copy(file: &Path, dest: &Path) -> Result<u64, String> {
    if std::fs::symlink_metadata(dest).is_ok() {
        return Err("a file with that name already exists there; choose a new name (existing files are never replaced).".into());
    }
    let mut src = std::fs::File::open(file).map_err(|e| e.to_string())?;
    let mut out = new_private_file(dest).map_err(|e| e.to_string())?;
    let copied = std::io::copy(&mut src, &mut out).and_then(|n| out.sync_all().map(|()| n));
    copied.map_err(|e| {
        let _ = std::fs::remove_file(dest);
        e.to_string()
    })
}

/// The core's refusal in plain words. Refusals that already say what to do keep their text; anything else
/// that stops a backup from opening or matching its own records means the file is damaged or incomplete,
/// and the technical reason goes in the small details line.
fn plain(e: core::BackupError) -> Refusal {
    let text = e.to_string();
    if text.contains("a passphrase is required") {
        return "this backup was exported with an extra password. Restoring password-protected exports is not available in the desktop app yet. Nothing was created.".to_owned().into();
    }
    const AS_SAID: [&str; 14] = ["Wrong master key", "NEWER or incompatible", "Unsupported FreedomBooks backup format", "Not enough free space",
        "exceeds the restore limit", "symbolic links", "Use a plain path", "Refusing to restore into an existing book", "overlap", "folder",
        "sidecars", "parent", "is running", "regular file"];
    let local_io = text.starts_with("Backup I/O failed") && ["denied", "space", "Read-only", "No such file"].iter().any(|m| text.contains(m));
    if local_io || AS_SAID.iter().any(|m| text.contains(m)) {
        return format!("{text}{}", if text.ends_with("Nothing was restored.") { "" } else { " Nothing was created." }).into();
    }
    Refusal { text: "This backup file is damaged or incomplete. Nothing was restored.".into(), detail: Some(format!("Details: {}", text.trim_end_matches(" Nothing was restored."))) }
}

/// Restore `file` into a NEW book `<data>/books/restored-<name>/restored-<name>.fbbook` with its own
/// receipts folder. The backup is verified first, so a bad file is refused before anything is created.
fn restore(app: &mut App, file: &Path, name: &str) -> Result<(), Refusal> {
    restore_with_secret(app, file, name, "")
}

/// Backups without a key envelope (made before recovery keys) open only with the book's current key,
/// so they cannot be restored once a protection switch replaces it.
pub(crate) fn has_pre_portable_backups(app: &App) -> bool {
    let Ok(folder) = app.backups.folder() else { return false };
    retention::backup_files(&folder, None).is_ok_and(|files| files.iter().any(|f| core::read_header(f).is_ok_and(|h| h["book_keys"].is_null())))
}

pub(crate) fn archive_keys(file: &Path) -> AppResult<Option<core::portable::KeyEnvelope>> {
    let header = core::read_header(file).map_err(|e| AppError::Invalid(e.to_string()))?;
    if header["book_keys"].is_null() { return Ok(None); }
    let keys: core::portable::KeyEnvelope = serde_json::from_value(header["book_keys"].clone()).map_err(|_| AppError::Invalid("The backup key record is damaged.".into()))?;
    keys.validate().map_err(|e| AppError::Invalid(e.to_string()))?;
    Ok(Some(keys))
}

pub(crate) fn finish_portable_restore(app: &mut App, secret: &str) -> AppResult<Outcome> {
    let (company, name, file) = app.backups.restore_pending.clone().ok_or_else(|| AppError::Invalid("Choose a backup to restore first.".into()))?;
    restore_with_secret(app, &file, &name, secret).map_err(|e| AppError::Invalid(e.to_string()))?;
    app.backups.restore_pending = None;
    Ok(page(company, None))
}

fn restore_with_secret(app: &mut App, file: &Path, name: &str, secret: &str) -> Result<(), Refusal> {
    let (_, slug) = book_name(name).map_err(|e| e.to_string())?;
    let host = &app.backups;
    let (_, current) = host.open_book().map_err(|e| e.to_string())?;
    let envelope = archive_keys(file).map_err(|e| e.to_string())?;
    let key = match &envelope {
        Some(e) if !secret.is_empty() || e.mode == core::portable::Mode::BuiltIn => e.unlock(secret).map_err(|e| e.to_string())?,
        _ => MasterKey::new(**current),
    };
    let receipt_key = match &envelope { Some(e) => e.receipts(&key).map_err(|e| e.to_string())?, None => MasterKey::new(*key) };
    let work = host.work().map_err(|e| e.to_string())?;
    let dir = host.books().map_err(|e| e.to_string())?.join(&slug);
    let protected = host.protected().map_err(|e| e.to_string())?;
    // The name was free when the dialog opened; a crash residue that appeared since is cleared the same way.
    name_free(&dir, name).map_err(|e| e.to_string())?;
    let target = dir.join(format!("{slug}.fbbook"));
    let record = envelope.map(crate::protection::Record::new).or_else(|| {
        // A pre-portable backup shares this installation's legacy master and receipt key.
        // Reuse its already-authorized portable wraps when they match that older layout.
        app.protection.as_ref().filter(|r| r.keys.receipts(&key).is_ok_and(|k| *k == *key)).cloned().map(|mut r| {
            r.ask_password = r.keys.mode == core::portable::Mode::OwnPassword;
            r.recovery_saved = false;
            r
        })
    });
    restore_into_with_record(file, &key, &receipt_key, &work, &target, &protected, record.as_ref())?;
    app.backups.restored = Some((name.to_owned(), target, key));
    Ok(())
}

/// The restore itself, for any target: core refusals (existing file, protected overlap) stay in force.
#[cfg(test)]
fn restore_into(file: &Path, key: &MasterKey, work: &Path, target: &Path, protected: &[PathBuf]) -> Result<(), Refusal> {
    restore_into_with_receipts(file, key, key, work, target, protected)
}

#[cfg(test)]
fn restore_into_with_receipts(file: &Path, key: &MasterKey, receipts: &MasterKey, work: &Path, target: &Path, protected: &[PathBuf]) -> Result<(), Refusal> {
    restore_into_with_record(file, key, receipts, work, target, protected, None)
}

fn restore_into_with_record(file: &Path, key: &MasterKey, receipts: &MasterKey, work: &Path, target: &Path, protected: &[PathBuf], record: Option<&crate::protection::Record>) -> Result<(), Refusal> {
    let verifier = core::attachment_receipt_verifier(receipts);
    private_dir(work).map_err(|e| e.to_string())?;
    if !file.is_file() {
        return Err("choose a backup file.".to_owned().into());
    }
    // A new book folder is never made inside (or around) a live path; the core checks the target itself.
    if std::fs::symlink_metadata(parent(target)).is_err() && protected.iter().any(|p| overlaps(parent(target), p)) {
        return Err("the new book would overlap the open book, its receipts or the backup folder. Nothing was created.".to_owned().into());
    }
    core::verify_backup_with_receipts(file, key, work, None, false, Some(&verifier)).map_err(plain)?;
    let dir = parent(target).to_owned();
    let made = std::fs::symlink_metadata(&dir).is_err();
    if made {
        std::fs::create_dir(&dir).and_then(|()| private_dir(&dir)).map_err(|e| e.to_string())?;
    }
    if let Some(record) = record { record.save(target).map_err(|e| e.to_string())?; }
    let result = core::restore_backup(file, key, &RestoreOptions {
        verify_receipt: Some(&verifier), target, receipts_target: Some(&dir.join("receipts")), work_root: work,
        protected, passphrase: None, require_encrypted: false,
    });
    match result {
        Ok(_) => Ok(()),
        Err(e) => {
            // The key record was written first; a folder made here goes with it, so the name is free again.
            if made {
                let _ = std::fs::remove_dir_all(&dir);
            } else if record.is_some() {
                let _ = std::fs::remove_file(crate::protection::record_path(target));
            }
            Err(plain(e))
        }
    }
}

/// "Open restored book": the open book closes (its key copy goes too) and the restored one opens.
fn open_restored(app: &mut App) -> AppResult<Outcome> {
    let Some((name, path, key)) = app.backups.restored.clone() else {
        return Err(AppError::Invalid("There is no restored book to open. Restore a backup first.".into()));
    };
    app.open_book(&path, &key)?;
    app.backups.viewing = Some(name);
    let home = app.first_company()?.map(|c| url("company_home", &[&c.to_db()])).unwrap_or_else(|| "/".into());
    Ok(Outcome::Redirect { location: home })
}

pub fn restore_from_gate(app: &mut App, file: &Path, name: &str, secret: &str) -> AppResult<PathBuf> {
    let (_, slug) = book_name(name)?;
    let envelope = archive_keys(file)?.ok_or_else(|| AppError::Invalid("This older backup needs the original computer's key. Open it there and create a portable backup first.".into()))?;
    let key = envelope.unlock(secret).map_err(crate::protection::error)?;
    let receipts = envelope.receipts(&key).map_err(crate::protection::error)?;
    let books = app.backups.books()?;
    private_dir(&books).map_err(crate::protection::error)?;
    let dir = books.join(&slug);
    name_free(&dir, name)?;
    let target = dir.join(format!("{slug}.fbbook"));
    let protected = vec![books.join("default.fbbook"), books.join("receipts"), app.backups.folder()?, app.backups.work()?];
    let record = crate::protection::Record::new(envelope);
    restore_into_with_record(file, &key, &receipts, &app.backups.work()?, &target, &protected, Some(&record)).map_err(crate::protection::error)?;
    app.open_book(&target, &key)?;
    app.backups.viewing = Some(name.to_owned());
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_make_private_restored_folders_and_refuse_reserved_ones() {
        assert_eq!(book_name(" TechQuest copy ").unwrap(), ("TechQuest copy".into(), "restored-techquest-copy".into()));
        assert_eq!(book_name("a/../b").unwrap().1, "restored-a-b");
        for bad in ["", "   ", "///", "Default", "receipts", &"x".repeat(61)] {
            assert!(book_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn created_time_comes_from_the_filename_stamp() {
        assert_eq!(created_iso("freedombooks-default-20261103T150405Z.fbbackup").as_deref(), Some("2026-11-03T15:04:05Z"));
        assert_eq!(created_iso("freedombooks-default-2026.fbbackup"), None);
    }

    #[test]
    fn restore_refuses_live_paths_existing_books_and_damaged_files() {
        let d = tempfile::tempdir().unwrap();
        let key = crate::keys::new_master_key().unwrap();
        let books = d.path().join("books");
        std::fs::create_dir_all(&books).unwrap();
        let live = books.join("default.fbbook");
        let mut app = App::new();
        app.backups.configure(d.path()).unwrap();
        app.create_book(&live, &key, "Owner", "Scratch Co").unwrap();
        let c = app.first_company().unwrap().unwrap();
        let name = back_up(&mut app, c).unwrap();
        let file = app.backups.folder().unwrap().join(&name);
        assert!(retention::verified_note(&file), "the backup is verified as it is made");
        let work = app.backups.work().unwrap();
        let protected = app.backups.protected().unwrap();
        let before = std::fs::read(&live).unwrap();
        // Onto the open book, and onto another existing book file: refused by the core.
        let err = restore_into(&file, &key, &work, &live, &protected).unwrap_err();
        assert!(err.text.contains("existing book"), "{err}");
        let other = books.join("other").join("other.fbbook");
        std::fs::create_dir_all(other.parent().unwrap()).unwrap();
        std::fs::write(&other, b"not empty").unwrap();
        let err = restore_into(&file, &key, &work, &other, &protected).unwrap_err();
        assert!(err.text.contains("existing book"), "{err}");
        // Inside the live receipts folder or the backup folder: refused as a protected overlap.
        std::fs::create_dir_all(books.join("receipts")).unwrap();
        let err = restore_into(&file, &key, &work, &books.join("receipts").join("x").join("x.fbbook"), &protected).unwrap_err();
        assert!(err.text.contains("overlap"), "{err}");
        let err = restore_into(&file, &key, &work, &books.join("receipts").join("x.fbbook"), &protected).unwrap_err();
        assert!(err.text.contains("overlaps"), "{err}");
        assert!(!books.join("receipts").join("x").exists() && !books.join("receipts").join("x.fbbook").exists());
        // A damaged backup is refused before any folder or book is created.
        let bad = d.path().join("damaged.fbbackup");
        let mut bytes = std::fs::read(&file).unwrap();
        let mid = bytes.len() / 2;
        bytes[mid] ^= 0x55;
        std::fs::write(&bad, bytes).unwrap();
        let target = books.join("restored-bad").join("restored-bad.fbbook");
        assert!(restore_into(&bad, &key, &work, &target, &protected).is_err());
        assert!(!books.join("restored-bad").exists(), "nothing left behind");
        #[cfg(unix)]
        {
            // A refusal after verification (a symlinked protected path) removes the key record and folder too.
            let link = d.path().join("protected-link");
            std::os::unix::fs::symlink(d.path().join("elsewhere"), &link).unwrap();
            let record = crate::protection::Record::new(core::portable::KeyEnvelope::new(&core::portable::BUILTIN_KEY, &[3; 32], None).unwrap());
            let refused = books.join("restored-refused");
            let err = restore_into_with_record(&file, &key, &key, &work, &refused.join("restored-refused.fbbook"), &[link], Some(&record)).unwrap_err();
            assert!(err.text.contains("symbolic links"), "{err}");
            assert!(!refused.exists(), "no key record or folder is left behind");
            name_free(&refused, "refused").unwrap();
        }
        // The good one restores into a new book with its own receipts folder, and opens.
        let good = books.join("restored-ok").join("restored-ok.fbbook");
        restore_into(&file, &key, &work, &good, &protected).unwrap();
        assert!(good.is_file() && books.join("restored-ok").join("receipts").is_dir());
        assert_eq!(std::fs::read(&live).unwrap(), before, "the open book is untouched");
        let restored = fb_db::Book::open(&good, &key).unwrap();
        assert!(restored.check().unwrap().is_empty());
    }

    #[test]
    fn dialog_tickets_are_single_use_and_folder_choice_avoids_the_books_folder() {
        let d = tempfile::tempdir().unwrap();
        let key = crate::keys::new_master_key().unwrap();
        let mut app = App::new();
        app.backups.configure(d.path()).unwrap();
        std::fs::create_dir_all(d.path().join("books")).unwrap();
        app.create_book(&d.path().join("books/default.fbbook"), &key, "Owner", "Scratch Co").unwrap();
        let c = app.first_company().unwrap().unwrap();
        let ticket = app.backups.mint(Waiting::Folder { company: c }).unwrap();
        assert!(dialog_result(&mut app, "0".repeat(64).as_str(), "").is_err());
        assert!(dialog_result(&mut app, &ticket, "").is_err(), "a failed attempt spends the ticket");
        let ticket = app.backups.mint(Waiting::Folder { company: c }).unwrap();
        dialog_result(&mut app, &ticket, d.path().join("books/sub").to_str().unwrap()).unwrap();
        assert_eq!(app.backups.messages.last().unwrap().0, "error");
        assert_eq!(app.backups.folder().unwrap(), d.path().join("backups"));
        let ticket = app.backups.mint(Waiting::Folder { company: c }).unwrap();
        dialog_result(&mut app, &ticket, d.path().join("elsewhere").to_str().unwrap()).unwrap();
        assert_eq!(app.backups.folder().unwrap(), d.path().join("elsewhere"));
    }

    #[test]
    fn the_restore_and_folder_dialogs_start_in_the_backup_folder() {
        let (d, mut app, c, _, _) = scratch();
        let route = crate::screens::resolve(&format!("/c/{}/backups/", c.to_db())).unwrap();
        for (action, extra, kind) in [("restore", ("new_book", "Starts here"), "pick-backup"), ("folder", ("", ""), "choose-backup-folder")] {
            let fields = vec![("action".to_owned(), action.to_owned()), (extra.0.to_owned(), extra.1.to_owned())];
            let Outcome::Done { data } = submit(&mut app, &route, &fields).unwrap() else { panic!("{action}: not a dialog") };
            assert_eq!(data["desktop"], kind);
            assert_eq!(data["folder"], d.path().join("backups").display().to_string(), "{action}");
        }
    }

    /// A scratch app with one backup made through the host, as the page does it.
    fn scratch() -> (tempfile::TempDir, App, CompanyId, MasterKey, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let key = crate::keys::new_master_key().unwrap();
        std::fs::create_dir_all(d.path().join("books")).unwrap();
        let mut app = App::new();
        app.backups.configure(d.path()).unwrap();
        app.create_book(&d.path().join("books/default.fbbook"), &key, "Owner", "Scratch Co").unwrap();
        let c = app.first_company().unwrap().unwrap();
        let name = back_up(&mut app, c).unwrap();
        let file = app.backups.folder().unwrap().join(name);
        app.backups.messages.clear();
        (d, app, c, key, file)
    }

    /// The name cleanup_stale recognises: `.fb-backup.<this host>.<pid>.<random>.partial`.
    fn partial_name(pid: u32) -> String {
        use sha2::Digest;
        let host = std::fs::read_to_string("/etc/hostname").unwrap();
        let host = format!("{:x}", sha2::Sha256::digest(host.trim().as_bytes()));
        format!(".fb-backup.{}.{pid}.Zq81xK2p.partial", &host[..16])
    }

    fn restore_via_dialog(app: &mut App, c: CompanyId, name: &str, file: &Path) {
        let ticket = app.backups.mint(Waiting::Restore { company: c, name: name.into() }).unwrap();
        dialog_result(app, &ticket, file.to_str().unwrap()).unwrap();
    }

    #[test]
    fn crash_residue_of_a_dead_restore_frees_the_name_but_a_live_one_is_refused() {
        let (d, mut app, c, key, file) = scratch();
        let books = d.path().join("books");
        // kill -9 mid-restore: the host's folder, the core's dead-pid partial and half the receipts.
        let dead = books.join("restored-crashed");
        std::fs::create_dir_all(dead.join("receipts")).unwrap();
        std::fs::write(dead.join(partial_name(4_294_967_295)), b"half a book").unwrap();
        std::fs::write(dead.join("receipts").join("half"), b"r").unwrap();
        name_free(&dead, "Crashed").unwrap();
        assert!(!dead.exists(), "the residue is removed and the name is free");
        restore_via_dialog(&mut app, c, "Crashed", &file);
        let (level, text, _) = app.backups.messages.pop().unwrap();
        assert_eq!(level, "success", "{text}");
        let restored = dead.join("restored-crashed.fbbook");
        assert!(fb_db::Book::open(&restored, &key).unwrap().check().unwrap().is_empty());
        // The same residue planted after the dialog opened is cleared at restore time too.
        let late = books.join("restored-late");
        std::fs::create_dir_all(&late).unwrap();
        std::fs::write(late.join(partial_name(4_294_967_295)), b"x").unwrap();
        restore_via_dialog(&mut app, c, "Late", &file);
        assert_eq!(app.backups.messages.pop().unwrap().0, "success");
        assert!(late.join("restored-late.fbbook").is_file());
        // A live pid (this process) may still be restoring: refused, nothing removed.
        let live = books.join("restored-running");
        std::fs::create_dir_all(&live).unwrap();
        let partial = live.join(partial_name(std::process::id()));
        std::fs::write(&partial, b"in progress").unwrap();
        let err = name_free(&live, "Running").unwrap_err().to_string();
        assert!(err.contains("still running or did not finish"), "{err}");
        assert!(partial.exists());
        restore_via_dialog(&mut app, c, "Running", &file);
        let (level, text, _) = app.backups.messages.pop().unwrap();
        assert!(level == "error" && text.contains("still running"), "{text}");
        assert_eq!(std::fs::read_dir(&live).unwrap().count(), 1, "the live partial is untouched");
        // A real restored book keeps its name.
        let err = name_free(&dead, "Crashed").unwrap_err().to_string();
        assert!(err.contains("already exists here"), "{err}");
    }

    #[test]
    fn an_ancestor_of_the_books_folder_is_refused_for_the_right_reason() {
        let (d, mut app, c, _, _) = scratch();
        for (chosen, says) in [(d.path().to_owned(), "cannot contain the books folder"), (d.path().join("books/sub"), "cannot go inside the books folder")] {
            let ticket = app.backups.mint(Waiting::Folder { company: c }).unwrap();
            dialog_result(&mut app, &ticket, chosen.to_str().unwrap()).unwrap();
            let (level, text, _) = app.backups.messages.pop().unwrap();
            assert!(level == "error" && text.starts_with("Folder not changed: ") && text.contains(says), "{text}");
        }
        assert!(app.backups.messages.is_empty());
        assert_eq!(app.backups.folder().unwrap(), d.path().join("backups"));
        let books = d.path().join("books");
        let ancestor = folder_refusal(Path::new("/"), std::slice::from_ref(&books)).unwrap();
        assert!(ancestor.contains("choose a folder outside the books folder") || ancestor.contains("Choose a folder outside the books folder"), "{ancestor}");
        assert_eq!(folder_refusal(&d.path().join("elsewhere"), &[books]), None);
    }

    #[test]
    fn unreadable_files_in_the_backup_folder_are_marked_like_django() {
        let (_d, mut app, c, _, file) = scratch();
        let folder = file.parent().unwrap().to_owned();
        let bytes = std::fs::read(&file).unwrap();
        let truncated = folder.join("freedombooks-default-20260101T000000Z.fbbackup");
        std::fs::write(&truncated, &bytes[..bytes.len() / 2]).unwrap();
        let foreign = folder.join("freedombooks-default-20250101T000000Z.fbbackup");
        std::fs::write(&foreign, b"%PDF-1.4 this is somebody's invoice, not a backup").unwrap();
        let rows = status(&app.backups).unwrap()["rows"].as_array().unwrap().clone();
        let of = |p: &Path| rows.iter().find(|r| r["name"] == p.file_name().unwrap().to_str().unwrap()).unwrap().clone();
        assert_eq!((of(&file)["readable"].clone(), of(&file)["encrypted"].clone()), (json!(true), json!(true)));
        for bad in [&truncated, &foreign] {
            assert_eq!((of(bad)["readable"].clone(), of(bad)["encrypted"].clone()), (json!(false), json!(false)), "{}", bad.display());
        }
        // The page says so in the Protection column (Django's Encrypted column, renamed for built-in keys).
        let html = crate::screens::render(&mut app, &format!("/c/{}/backups/", c.to_db())).unwrap().html;
        assert_eq!(html.matches(">Unreadable<").count(), 2, "{html}");
        assert_eq!(html.matches("<td data-label=\"Protection\">Own password</td>").count(), 1, "{html}");
    }

    #[test]
    fn a_damaged_backup_is_refused_in_plain_words_and_clears_the_earlier_restored_panel() {
        let (d, mut app, c, _, file) = scratch();
        restore_via_dialog(&mut app, c, "First", &file);
        assert_eq!(app.backups.messages.pop().unwrap().0, "success");
        assert!(app.backups.restored.is_some(), "the restored-book panel shows");
        let damaged = d.path().join("damaged.fbbackup");
        let mut bytes = std::fs::read(&file).unwrap();
        let mid = bytes.len() / 2;
        bytes[mid] ^= 0x55;
        std::fs::write(&damaged, bytes).unwrap();
        restore_via_dialog(&mut app, c, "Second", &damaged);
        let (level, text, detail) = app.backups.messages.pop().unwrap();
        assert_eq!((level, text.as_str()), ("error", "Not restored: This backup file is damaged or incomplete. Nothing was restored."));
        let detail = detail.unwrap();
        assert!(detail.starts_with("Details: ") && !detail.ends_with("Nothing was restored."), "{detail}");
        assert!(app.backups.restored.is_none(), "the earlier ready panel is gone after a refusal");
        assert!(!d.path().join("books/restored-second").exists());
        // A refusal at submit time (existing name) clears it too.
        restore_via_dialog(&mut app, c, "Third", &file);
        assert!(app.backups.restored.is_some());
        app.backups.refuse_restore(name_free(&d.path().join("books/restored-third"), "Third").unwrap_err().to_string().into());
        assert!(app.backups.restored.is_none());
        // Refusals that already say what to do keep their own words.
        assert!(plain(core::BackupError::refused("Not enough free space to safely complete this operation.")).detail.is_none());
        assert!(plain(core::BackupError::refused("Wrong master key or changed backup manifest/header.")).text.starts_with("Wrong master key"));
    }
}


