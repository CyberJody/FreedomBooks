//! Attachments through a book's whole life, on books made the way first run makes them: attach through the
//! native chooser (`backups::dialog_result`), back up and verify, upgrade (with its automatic verified
//! copy), change the password, restore, and read the file back. Tests that build the attachment store by
//! hand cannot see which key the real attach path chose.
mod support;
use fb_app::{App, Command, Outcome, keys::MasterKey, protection::Record, screens};
use fb_core::{
    CompanyId,
    attachments::{self as a, storage::Store},
    backups::{self, retention},
    services,
};
use std::{
    fs,
    path::{Path, PathBuf},
};

const PASSWORD: &str = "Synthetic owner password";
const NEW_PASSWORD: &str = "Synthetic replacement password";
const PDF: &[u8] = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n1 0 obj<</Type/Catalog>>endobj\ntrailer<</Root 1 0 R>>\n%%EOF\n";
const SECOND: &[u8] = b"%PDF-1.4\n%second\n1 0 obj<</Type/Catalog>>endobj\ntrailer<</Root 1 0 R>>\n%%EOF\n";

fn app_at(root: &Path) -> App {
    let mut app = App::new();
    app.backups.configure(root).unwrap();
    app.device_key = Some(MasterKey::new([57; 32]));
    app
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

struct Books {
    root: tempfile::TempDir,
    app: App,
    path: PathBuf,
    co: CompanyId,
    password: Option<&'static str>,
}

impl Books {
    /// A first-run book (random receipt key, different from the master key).
    fn first_run(password: Option<&'static str>) -> Self {
        let root = tempfile::tempdir().unwrap();
        let mut app = app_at(root.path());
        let path = root.path().join("books/default.fbbook");
        app.create_protected_book(&path, "Synthetic owner", "Synthetic attachment company", password).unwrap();
        support::sign_in(&mut app);
        let co = app.first_company().unwrap().unwrap();
        let receipts = app.receipt_key().unwrap();
        assert_ne!(*receipts, **app.master_key().unwrap(), "first run gives the receipts their own key");
        Self { root, app, path, co, password }
    }

    fn journal(&mut self) -> String {
        let o = self.app.open_mut().unwrap();
        let cash = services::save_account(&mut o.book, o.owner, self.co, services::AccountInput::new("1000", "Synthetic cash").kind("ASSET")).unwrap().id;
        let equity = services::save_account(&mut o.book, o.owner, self.co, services::AccountInput::new("3000", "Synthetic equity").kind("EQUITY")).unwrap().id;
        let lines = vec![services::line(cash).debit("10").build(), services::line(equity).credit("10").build()];
        let j = services::save_draft(&mut o.book, o.owner, self.co, services::DraftInput::new("2026-10-01", "Synthetic opening", lines)).unwrap();
        let id = services::post_journal(&mut o.book, o.owner, self.co, j.id, None).unwrap().id;
        format!("/c/{}/journals/{}/", self.co.to_db(), id.to_db())
    }

    /// Post the page's Attach form, then hand the chosen file back as the main process does.
    fn attach(&mut self, page: &str, name: &str, data: &[u8]) -> String {
        let html = screens::render(&mut self.app, page).unwrap().html;
        let form = html.split_once(r#"<form class="attachments-add""#).expect("an Attach form").1;
        let action = attr(form, "action");
        let token = value(form, "fb_command_token");
        let request_key = value(form, "request_key");
        let Outcome::Done { data: ticket } = self.app.run(Command::SubmitForm { path: action, token, fields: vec![("request_key".into(), request_key)] }).unwrap() else { panic!("no chooser ticket") };
        let picked = self.root.path().join(name);
        fs::write(&picked, data).unwrap();
        screens::backups::dialog_result(&mut self.app, ticket["ticket"].as_str().unwrap(), picked.to_str().unwrap()).unwrap();
        let html = screens::render(&mut self.app, page).unwrap().html;
        assert!(html.contains(&format!("Attached {name}.")), "{html}");
        let o = self.app.open_mut().unwrap();
        o.book.read(|q| q.query_row("SELECT id FROM attachments_attachment WHERE original_name=?1", [name], |r| r.get(0))).unwrap()
    }

    /// The Backups page's "Create backup", returning the created file (or the Backups page that says why not).
    fn back_up(&mut self) -> Result<PathBuf, String> {
        let path = format!("/c/{}/backups/create/", self.co.to_db());
        let token = self.app.tokens.mint(&path);
        let Outcome::Redirect { location } = self.app.run_renderer(Command::SubmitForm { path, token, fields: vec![] }).unwrap() else { panic!("no redirect") };
        match location.split("created=").nth(1) {
            Some(name) => Ok(self.app.backups.folder().unwrap().join(name)),
            None => {
                Err(screens::render(&mut self.app, &format!("/c/{}/backups/", self.co.to_db())).unwrap().html)
            }
        }
    }

    fn read_back(&mut self, id: &str) -> Vec<u8> {
        let store = Store::for_book(&self.path, &self.app.receipt_key().unwrap()).unwrap();
        let o = self.app.open_mut().unwrap();
        a::download(&mut o.book, &store, o.owner, self.co, id).unwrap()
    }

    /// Replace the live book with the same data one migration behind, and open it: the app makes and
    /// verifies its pre-upgrade copy (a rehearsal upgrade of that copy) before changing the book.
    fn upgrade(&mut self) -> PathBuf {
        let key = MasterKey::new(**self.app.master_key().unwrap());
        let prior = self.path.with_extension("prior");
        self.app.lock();
        {
            let source = fb_db::Book::open(&self.path, &key).unwrap();
            let last = fb_db::MIGRATIONS.last().unwrap().0;
            fb_db::test_support::populated_copy(source.connection(), &prior, &key, last, false).unwrap();
        }
        self.app.lock();
        for suffix in ["-wal", "-shm"] {
            let _ = fs::remove_file(sidecar(&self.path, suffix));
        }
        fs::rename(&prior, &self.path).unwrap();
        let before: Vec<PathBuf> = retention::backup_files(&self.app.backups.folder().unwrap(), None).unwrap();
        self.app.open_with_secret(&self.path, self.password.unwrap_or("")).unwrap();
        support::sign_in(&mut self.app);
        assert!(self.app.upgrade_notice.is_some(), "the book was upgraded");
        let after = retention::backup_files(&self.app.backups.folder().unwrap(), None).unwrap();
        let copy = after.into_iter().find(|p| !before.contains(p)).expect("a pre-upgrade copy");
        assert!(retention::verified_note(&copy));
        copy
    }
}

fn attr(html: &str, name: &str) -> String {
    html.split_once(&format!("{name}=\"")).unwrap().1.split('"').next().unwrap().to_owned()
}
fn value(html: &str, name: &str) -> String {
    html.split_once(&format!("name=\"{name}\" value=\"")).unwrap_or_else(|| panic!("no {name} field: {html}")).1.split('"').next().unwrap().to_owned()
}

/// Restore `archive` as a new book on another installation and read attachment `id` back there with the
/// receipt key alone.
fn restore_elsewhere(archive: &Path, secret: &str, co: CompanyId, id: &str) -> Vec<u8> {
    restore_and_read(archive, secret, co, id, false)
}

/// `legacy`: the backup may still hold a file wrapped under its master key (a copy made before the
/// re-wrap). The restored book reads it with that key, and its first backup moves it.
fn restore_and_read(archive: &Path, secret: &str, co: CompanyId, id: &str, legacy: bool) -> Vec<u8> {
    let other_root = tempfile::tempdir().unwrap();
    let mut other = app_at(other_root.path());
    other.device_key = Some(MasterKey::new([58; 32]));
    let restored = screens::backups::restore_from_gate(&mut other, archive, "Attachment restore", secret).unwrap();
    support::sign_in(&mut other);
    let mut store = Store::for_book(&restored, &other.receipt_key().unwrap()).unwrap();
    if legacy {
        store = store.with_legacy(other.master_key().unwrap()).unwrap();
    }
    let o = other.open_mut().unwrap();
    a::download(&mut o.book, &store, o.owner, co, id).unwrap()
}

fn lifecycle(password: Option<&'static str>) {
    let mut b = Books::first_run(password);
    let page = b.journal();
    let id = b.attach(&page, "receipt.pdf", PDF);
    assert_eq!(b.read_back(&id), PDF);
    let archive = b.back_up().expect("a backup after attaching through the chooser");
    assert!(retention::verified_note(&archive));
    let copy = b.upgrade();
    assert_eq!(b.read_back(&id), PDF);
    assert_eq!(restore_elsewhere(&copy, password.unwrap_or(""), b.co, &id), PDF);
    let next = Some(NEW_PASSWORD);
    b.app.change_protection(password.unwrap_or(""), next).unwrap();
    b.password = next;
    assert_eq!(b.read_back(&id), PDF);
    // A second file after the change, and a backup holding both.
    let second = b.attach(&page, "second.pdf", SECOND);
    let archive = b.back_up().expect("a backup after the password change");
    assert_eq!(restore_elsewhere(&archive, NEW_PASSWORD, b.co, &id), PDF);
    assert_eq!(restore_elsewhere(&archive, NEW_PASSWORD, b.co, &second), SECOND);
    // Reopen from the gate with the new password: everything still reads.
    b.app.lock();
    b.app.open_with_secret(&b.path, NEW_PASSWORD).unwrap();
    support::sign_in(&mut b.app);
    assert_eq!(b.read_back(&id), PDF);
    assert_eq!(b.read_back(&second), SECOND);
    let receipts = Record::read(&b.path).unwrap().unwrap().keys.receipts(b.app.master_key().unwrap()).unwrap();
    let work = b.root.path().join("verify");
    fs::create_dir_all(&work).unwrap();
    #[cfg(unix)]
    fs::set_permissions(&work, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
    let verifier = backups::attachment_receipt_verifier(&receipts);
    backups::verify_backup_with_receipts(&archive, b.app.master_key().unwrap(), &work, None, false, Some(&verifier)).unwrap();
}

#[test]
fn own_password_first_run_book_attaches_backs_up_upgrades_rekeys_and_restores() {
    lifecycle(Some(PASSWORD));
}

#[test]
fn builtin_key_first_run_book_attaches_backs_up_upgrades_rekeys_and_restores() {
    lifecycle(None);
}

impl Books {
    /// An attachment written the way the previous build's chooser wrote it: its key wrapped under the
    /// book's master key.
    fn attach_as_before(&mut self, page: &str, name: &str, data: &[u8]) -> String {
        let journal = fb_core::JournalId::parse(page.trim_end_matches('/').rsplit('/').next().unwrap()).unwrap();
        let master = MasterKey::new(**self.app.master_key().unwrap());
        let store = Store::for_book(&self.path, &master).unwrap();
        let o = self.app.open_mut().unwrap();
        a::add(&mut o.book, &store, o.owner, self.co, &a::Owner::Journal(journal), data, name, &uuid::Uuid::new_v4().to_string(), &a::Limits::default()).unwrap().0["id"].as_str().unwrap().to_owned()
    }
    /// The newest wrap of an attachment's file key (the newest recorded further wrap, else the row's own).
    fn wrap_of(&mut self, id: &str) -> String {
        let co = self.co;
        self.app.open_mut().unwrap().book.read(|q| {
            let (name, own): (String, String) = q.query_row("SELECT stored_name, wrapped_key FROM attachments_attachment WHERE id=?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            Ok::<_, fb_core::FbError>(a::recorded_wraps(q, co, &name)?.into_iter().next().map_or(own, |w| w.wrapped_key))
        }).unwrap()
    }
    fn stored_name(&mut self, id: &str) -> String {
        self.app.open_mut().unwrap().book.read(|q| q.query_row("SELECT stored_name FROM attachments_attachment WHERE id=?1", [id], |r| r.get(0))).unwrap()
    }
    fn audits(&mut self, action: &str) -> i64 {
        self.app.open_mut().unwrap().book.read(|q| q.query_row("SELECT count(*) FROM ledger_auditevent WHERE action=?1", [action], |r| r.get(0))).unwrap()
    }
    fn stored_file(&mut self, id: &str) -> PathBuf {
        let name: String = self.app.open_mut().unwrap().book.read(|q| q.query_row("SELECT stored_name FROM attachments_attachment WHERE id=?1", [id], |r| r.get(0))).unwrap();
        self.path.parent().unwrap().join("receipts").join(name)
    }
    fn backups_page(&mut self) -> String {
        screens::render(&mut self.app, &format!("/c/{}/backups/", self.co.to_db())).unwrap().html
    }
}

#[test]
fn files_attached_by_the_previous_build_back_up_move_to_the_receipt_key_and_survive_a_password_change() {
    let mut b = Books::first_run(Some(PASSWORD));
    let page = b.journal();
    let id = b.attach_as_before(&page, "before-the-fix.pdf", PDF);
    // The app reads it (with the master key as the legacy reader).
    assert!(screens::render(&mut b.app, &page).unwrap().html.contains("before-the-fix.pdf"));
    let before = b.wrap_of(&id);
    let archive = b.back_up().expect("a backup of a book holding a master-wrapped file");
    assert_ne!(b.wrap_of(&id), before, "the backup re-wrapped the key under the receipt key first");
    assert_eq!(b.audits(a::REWRAPPED), 1);
    assert_eq!(b.read_back(&id), PDF, "readable with the receipt key alone");
    assert_eq!(restore_elsewhere(&archive, PASSWORD, b.co, &id), PDF);
    b.app.change_protection(PASSWORD, Some(NEW_PASSWORD)).unwrap();
    b.password = Some(NEW_PASSWORD);
    assert_eq!(b.read_back(&id), PDF);
    let archive = b.back_up().expect("a backup after the password change");
    assert_eq!(b.audits(a::REWRAPPED), 1, "nothing left to move");
    assert_eq!(restore_elsewhere(&archive, NEW_PASSWORD, b.co, &id), PDF);
}

#[test]
fn a_book_holding_a_master_wrapped_file_upgrades_and_the_next_backup_moves_it() {
    let mut b = Books::first_run(Some(PASSWORD));
    let page = b.journal();
    let id = b.attach_as_before(&page, "before-the-fix.pdf", PDF);
    // The upgrade copy is made before the schema that allows a re-wrap: it verifies with both keys.
    let copy = b.upgrade();
    assert_eq!(restore_and_read(&copy, PASSWORD, b.co, &id, true), PDF);
    b.back_up().expect("a backup after the upgrade");
    assert_eq!(b.audits(a::REWRAPPED), 1);
    assert_eq!(b.read_back(&id), PDF);
}

#[test]
fn a_missing_file_stops_a_routine_backup_with_its_name_and_the_owner_can_back_up_without_it() {
    let mut b = Books::first_run(Some(PASSWORD));
    let page = b.journal();
    let kept = b.attach(&page, "kept.pdf", PDF);
    let gone = b.attach(&page, "gone.pdf", SECOND);
    let file = b.stored_file(&gone);
    fs::remove_file(file).unwrap();
    let refused = b.back_up().expect_err("a routine backup stops first");
    assert!(refused.contains("“gone.pdf” on Journal entry") && refused.contains("(missing)"), "{refused}");
    assert!(refused.contains("Backup not created: 1 attached file is missing or damaged"), "{refused}");
    assert!(!refused.contains("disk space"), "{refused}");
    assert!(refused.contains("Back up without the missing files"), "the page offers the way forward: {refused}");
    assert!(retention::backup_files(&b.app.backups.folder().unwrap(), None).unwrap_or_default().is_empty());
    // The offer: a form posting incomplete=1 to Create backup.
    let html = b.backups_page();
    assert!(!html.contains("Back up without the missing files"), "the offer is shown once, with its refusal");
    let path = format!("/c/{}/backups/create/", b.co.to_db());
    let token = b.app.tokens.mint(&path);
    let Outcome::Redirect { location } = b.app.run_renderer(Command::SubmitForm { path, token, fields: vec![("incomplete".into(), "1".into())] }).unwrap() else { panic!() };
    let name = location.split("created=").nth(1).expect("an incomplete backup").to_owned();
    let archive = b.app.backups.folder().unwrap().join(&name);
    assert!(retention::verified_note(&archive));
    assert_eq!(backups::read_header(&archive).unwrap()["incomplete"], true);
    let html = b.backups_page();
    assert!(html.contains("Incomplete backup created") && html.contains("“gone.pdf” on Journal entry"), "{html}");
    assert!(html.contains("Incomplete: 1 attached file missing"), "the file list says so: {html}");
    // Review finding 7: the summary never presents it as the last good backup.
    assert!(html.contains("No complete backup verified yet") && html.contains("is verified but <strong>incomplete</strong>"), "{html}");
    assert!(html.contains("· <strong>incomplete</strong></dd>"), "the last backup is marked: {html}");
    // It restores, and the kept file reads back there.
    assert_eq!(restore_elsewhere(&archive, PASSWORD, b.co, &kept), PDF);
}

#[test]
fn a_missing_file_does_not_stop_an_upgrade_and_the_notice_names_it() {
    let mut b = Books::first_run(Some(PASSWORD));
    let page = b.journal();
    let gone = b.attach(&page, "gone.pdf", PDF);
    let file = b.stored_file(&gone);
    fs::remove_file(file).unwrap();
    let copy = b.upgrade();
    assert_eq!(backups::read_header(&copy).unwrap()["incomplete"], true);
    let notice = b.app.upgrade_notice.clone().unwrap();
    assert!(notice.contains("That copy is incomplete") && notice.contains("“gone.pdf” on Journal entry"), "{notice}");
}

#[test]
fn a_missing_file_does_not_block_a_password_change_and_the_next_page_names_it() {
    let mut b = Books::first_run(Some(PASSWORD));
    let page = b.journal();
    let gone = b.attach(&page, "gone.pdf", PDF);
    let file = b.stored_file(&gone);
    fs::remove_file(file).unwrap();
    b.app.change_protection(PASSWORD, Some(NEW_PASSWORD)).unwrap();
    let notice = b.app.upgrade_notice.clone().unwrap();
    assert!(notice.contains("safety backup made before this change is incomplete") && notice.contains("“gone.pdf”"), "{notice}");
    let home = screens::render(&mut b.app, &format!("/c/{}/home/", b.co.to_db())).unwrap().html;
    assert!(home.contains("“gone.pdf”"), "shown on the next page");
}

// ------------------------------------------------------------------------------------------------
// Review fixes (November 2026): lost key records, further wraps, ordering, exposure, restore notices.

impl Books {
    /// Read attachment `id` of `co` from the book open in `app` at `path`, with that book's keys.
    fn read_in(app: &mut App, path: &Path, co: CompanyId, id: &str) -> fb_core::FbResult<Vec<u8>> {
        let store = Store::for_book(path, &app.receipt_key().unwrap()).unwrap().with_legacy(app.master_key().unwrap()).unwrap();
        let o = app.open_mut().unwrap();
        a::download(&mut o.book, &store, o.owner, co, id)
    }
    fn record_bytes(&self) -> Vec<u8> {
        fs::read(fb_app::protection::record_path(&self.path)).unwrap()
    }
}

/// Review finding 1: a first-run book copied or moved without its key record holds attachments that only
/// its (lost) receipt key opens. Opening must refuse, in words the owner can act on, and must never write
/// a new record that can never open them. Putting the record back, or restoring a backup, gets them back.
fn lost_key_record(password: Option<&'static str>) {
    let mut b = Books::first_run(password);
    let page = b.journal();
    let id = b.attach(&page, "new.pdf", PDF);
    let archive = b.back_up().expect("a backup");
    let master = MasterKey::new(**b.app.master_key().unwrap());
    let record = fb_app::protection::record_path(&b.path);
    let saved = b.record_bytes();
    b.app.lock();
    fs::remove_file(&record).unwrap();
    let before = fs::read(&b.path).unwrap();
    // Every way in: the unlock screen (built-in key), and the saved key / app-lock password route that
    // opens with the master key itself.
    let mut refusals = vec![b.app.open_book(&b.path, &master).unwrap_err().to_string()];
    if password.is_none() {
        refusals.push(b.app.open_with_secret(&b.path, "").unwrap_err().to_string());
    }
    for refused in refusals {
        assert!(refused.contains("These books' key file is missing") && refused.contains("“default.fbbook.keys.json”"), "{refused}");
        assert!(refused.contains("1 attached file in these books cannot be opened") && refused.contains("nothing was changed"), "{refused}");
        assert!(refused.contains("copy that key file from where they came from") && refused.contains("Restore a backup"), "{refused}");
        assert!(!refused.to_lowercase().contains("damaged"), "a missing key is not damage: {refused}");
        assert!(!record.exists(), "no replacement record was written");
        assert!(!b.app.is_open(), "the books stay closed");
    }
    assert_eq!(fs::read(&b.path).unwrap(), before, "the book file is unchanged");
    // Putting the key file back is the way forward, and everything reads again.
    fs::write(&record, &saved).unwrap();
    b.app.open_with_secret(&b.path, password.unwrap_or("")).unwrap();
    support::sign_in(&mut b.app);
    assert_eq!(b.read_back(&id), PDF);
    // So is restoring a backup: it carries the receipt key.
    assert_eq!(restore_elsewhere(&archive, password.unwrap_or(""), b.co, &id), PDF);
}

#[test]
fn a_built_in_key_book_moved_without_its_key_record_is_refused_not_adopted() {
    lost_key_record(None);
}

#[test]
fn an_own_password_book_moved_without_its_key_record_is_refused_not_adopted() {
    lost_key_record(Some(PASSWORD));
}

/// Both adopt paths check on their own as well: a book open without a record (made by `create_book`)
/// holding a file its key does not open gets no record from either.
#[test]
fn the_adopt_paths_refuse_a_book_whose_files_its_own_key_does_not_open() {
    let root = tempfile::tempdir().unwrap();
    let mut app = app_at(root.path());
    let path = root.path().join("books/default.fbbook");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    app.create_book(&path, &MasterKey::new([61; 32]), "Synthetic owner", "Synthetic adopt company").unwrap();
    support::sign_in(&mut app);
    let co = app.first_company().unwrap().unwrap();
    let o = app.open_mut().unwrap();
    let cash = services::save_account(&mut o.book, o.owner, co, services::AccountInput::new("1000", "Synthetic cash").kind("ASSET")).unwrap().id;
    let equity = services::save_account(&mut o.book, o.owner, co, services::AccountInput::new("3000", "Synthetic equity").kind("EQUITY")).unwrap().id;
    let lines = vec![services::line(cash).debit("10").build(), services::line(equity).credit("10").build()];
    let j = services::save_draft(&mut o.book, o.owner, co, services::DraftInput::new("2026-10-01", "Synthetic opening", lines)).unwrap();
    let foreign = Store::for_book(&path, &[5; 32]).unwrap();
    a::add(&mut o.book, &foreign, o.owner, co, &a::Owner::Journal(j.id), PDF, "foreign.pdf", &uuid::Uuid::new_v4().to_string(), &a::Limits::default()).unwrap();
    let record = fb_app::protection::record_path(&path);
    for refused in [app.adopt_legacy_keys("Synthetic new password").unwrap_err(), app.adopt_builtin_keys().unwrap_err()] {
        assert!(refused.to_string().contains("These books' key file is missing"), "{refused}");
        assert!(!record.exists());
    }
}

/// The base build's behaviour stays: a book without a record whose files all open under its own key (files
/// attached before the receipt-key fix) adopts a new record and reads them.
#[test]
fn a_book_without_a_key_record_whose_files_its_key_opens_still_adopts_one() {
    let mut b = Books::first_run(None);
    let page = b.journal();
    let id = b.attach_as_before(&page, "old.pdf", PDF);
    b.app.lock();
    fs::remove_file(fb_app::protection::record_path(&b.path)).unwrap();
    b.app.open_with_secret(&b.path, "").unwrap();
    support::sign_in(&mut b.app);
    assert!(fb_app::protection::record_path(&b.path).exists(), "adopted");
    assert_eq!(b.read_back(&id), PDF);
}

/// Review finding 2, through the app: a further wrap that opens under no key does not make a good file
/// unreadable, and the backup stays complete.
#[test]
fn a_bad_further_wrap_record_does_not_break_a_good_attachment() {
    let mut b = Books::first_run(Some(PASSWORD));
    let page = b.journal();
    let id = b.attach(&page, "good.pdf", PDF);
    let stored = b.stored_name(&id);
    let co = b.co;
    let o = b.app.open_mut().unwrap();
    let owner = o.owner;
    o.book.write(|tx| -> fb_core::FbResult<()> {
        let user = services::authenticated(tx, owner)?;
        services::audit(tx, &user, co, a::REWRAPPED, "Attachment keys".into(), serde_json::json!({"attachments": [id], "files": [stored]}))?;
        let event: String = tx.query_row("SELECT id FROM ledger_auditevent WHERE action=?1", [a::REWRAPPED], |r| r.get(0))?;
        tx.execute("INSERT INTO attachments_keywrap VALUES(?1,?2,1,?3,?4,?5)", fb_db::rusqlite::params![co.to_db(), stored, "00".repeat(48), "00".repeat(12), event])?;
        Ok(())
    }).unwrap();
    assert_eq!(b.read_back(&id), PDF);
    let archive = b.back_up().expect("a complete backup");
    assert!(backups::read_header(&archive).unwrap().get("incomplete").is_none());
    assert_eq!(restore_elsewhere(&archive, PASSWORD, b.co, &id), PDF);
}

/// The same bytes attached again (the de-duplicating path) after a re-wrap and a password change.
#[test]
fn the_same_file_attached_again_after_a_rewrap_and_a_password_change_reads() {
    let mut b = Books::first_run(Some(PASSWORD));
    let page = b.journal();
    let id = b.attach_as_before(&page, "first.pdf", PDF);
    b.back_up().expect("the backup re-wraps");
    b.app.change_protection(PASSWORD, Some(NEW_PASSWORD)).unwrap();
    b.password = Some(NEW_PASSWORD);
    assert_eq!(b.read_back(&id), PDF);
    let again = b.attach(&page, "again.pdf", PDF);
    assert_eq!(b.read_back(&again), PDF);
    assert_eq!(b.stored_name(&again), b.stored_name(&id), "one stored file");
}

/// Review finding 4 (and the reviewer's first probe): a master-wrapped file whose receipts folder is away
/// during a password change reads once it is back; neither the history screen nor the AI audit history
/// shows any key wrap.
#[test]
fn a_file_away_during_a_password_change_reads_after_and_no_wrap_reaches_history_or_ai() {
    let mut b = Books::first_run(Some(PASSWORD));
    let page = b.journal();
    let id = b.attach_as_before(&page, "away.pdf", PDF);
    let own = b.wrap_of(&id);
    let receipts = b.path.parent().unwrap().join("receipts");
    let away = b.root.path().join("receipts-away");
    fs::rename(&receipts, &away).unwrap();
    b.app.change_protection(PASSWORD, Some(NEW_PASSWORD)).unwrap();
    b.password = Some(NEW_PASSWORD);
    fs::rename(&away, &receipts).unwrap();
    assert_eq!(b.read_back(&id), PDF);
    b.back_up().expect("a complete backup once the folder is back");
    let wrap = b.wrap_of(&id);
    assert_ne!(wrap, own, "the file gained a receipt-key wrap");
    let (wrapped, nonce): (String, String) = b.app.open_mut().unwrap().book.read(|q| q.query_row("SELECT wrapped_key, wrap_nonce FROM attachments_keywrap", [], |r| Ok((r.get(0)?, r.get(1)?)))).unwrap();
    let history = screens::render(&mut b.app, &format!("/c/{}/history/", b.co.to_db())).unwrap().html;
    let token = b.app.ai_grant(b.co, "Synthetic history agent", fb_app::ai::Permission::ReadOnly).unwrap()["token"].as_str().unwrap().to_owned();
    let names: Vec<String> = b.app.ai_catalogue(&token).unwrap()["operations"].as_array().unwrap().iter().map(|o| o["name"].as_str().unwrap().to_owned()).collect();
    assert!(names.iter().any(|n| n == "app_AuditHistory"), "{names:?}");
    let ai = b.app.ai_call(&token, &serde_json::json!({"operation": "app_AuditHistory", "arguments": {"limit": 100}}).to_string()).unwrap().to_string();
    assert!(ai.contains(a::REWRAPPED), "the event itself is visible: {ai}");
    for shown in [&history, &ai] {
        for secret in [&wrapped, &nonce, &own] {
            assert!(!shown.contains(secret.as_str()), "a key wrap is exposed");
        }
    }
}

/// Review finding 5: every path that changes the master key or replaces the key record, with an
/// attachment added before it read after it. The recovery-key password reset (the Emergency Kit key
/// in place of the current password) runs the same re-wrap and check as any password change.
#[test]
fn the_recovery_key_password_reset_keeps_older_attachments_readable() {
    let mut b = Books::first_run(Some(PASSWORD));
    let page = b.journal();
    let old = b.attach_as_before(&page, "before-the-fix.pdf", PDF);
    let new = b.attach(&page, "new.pdf", SECOND);
    // The Emergency Kit's recovery key (read here through the password-gated display, as the owner would
    // have printed it), used in place of the current password.
    b.app.authorize_recovery(PASSWORD).unwrap();
    let recovery = b.app.recovery_text().unwrap();
    b.app.change_protection(&recovery, Some(NEW_PASSWORD)).unwrap();
    b.password = Some(NEW_PASSWORD);
    assert_eq!(b.read_back(&old), PDF);
    assert_eq!(b.read_back(&new), SECOND);
    b.app.lock();
    b.app.open_with_secret(&b.path, NEW_PASSWORD).unwrap();
    support::sign_in(&mut b.app);
    let strict = Store::for_book(&b.path, &b.app.receipt_key().unwrap()).unwrap();
    let o = b.app.open_mut().unwrap();
    assert_eq!(a::download(&mut o.book, &strict, o.owner, b.co, &old).unwrap(), PDF, "the receipt key alone reads it");
    // Removing the password (built-in key) is the same path.
    b.app.change_protection(NEW_PASSWORD, None).unwrap();
    assert_eq!(b.read_back(&old), PDF);
}

/// Restore and "Use this as my books": the restored copy reads its attachments, and so does the previous
/// main book after it is set aside.
#[test]
fn restored_books_made_the_working_books_and_the_set_aside_book_keep_their_attachments() {
    let mut b = Books::first_run(Some(PASSWORD));
    let page = b.journal();
    let old = b.attach_as_before(&page, "before-the-fix.pdf", PDF);
    let new = b.attach(&page, "new.pdf", SECOND);
    let archive = b.back_up().expect("a backup");
    b.app.lock();
    let copy = screens::backups::restore_from_gate(&mut b.app, &archive, "Working copy", PASSWORD).unwrap();
    support::sign_in(&mut b.app);
    assert_eq!(Books::read_in(&mut b.app, &copy, b.co, &old).unwrap(), PDF);
    assert_eq!(Books::read_in(&mut b.app, &copy, b.co, &new).unwrap(), SECOND);
    // What the shell does after the owner confirms "Use this as my books".
    let made = b.app.make_open_book_main(Some(&b.path)).unwrap();
    let moved = made.set_aside.expect("the previous main book is set aside");
    assert_eq!(Books::read_in(&mut b.app, &copy, b.co, &old).unwrap(), PDF);
    b.app.lock();
    b.app.open_with_secret(&moved, PASSWORD).unwrap();
    support::sign_in(&mut b.app);
    assert_eq!(Books::read_in(&mut b.app, &moved, b.co, &old).unwrap(), PDF);
    assert_eq!(Books::read_in(&mut b.app, &moved, b.co, &new).unwrap(), SECOND);
}

/// The first author's open item: restoring an incomplete backup tells the owner which attached files the
/// restored book does not have, on the Backups page and from the unlock screen.
#[test]
fn restoring_an_incomplete_backup_names_the_files_it_lacks() {
    let mut b = Books::first_run(Some(PASSWORD));
    let page = b.journal();
    let kept = b.attach(&page, "kept.pdf", PDF);
    let gone = b.attach(&page, "gone.pdf", SECOND);
    let file = b.stored_file(&gone);
    fs::remove_file(file).unwrap();
    let path = format!("/c/{}/backups/create/", b.co.to_db());
    let token = b.app.tokens.mint(&path);
    let Outcome::Redirect { location } = b.app.run_renderer(Command::SubmitForm { path, token, fields: vec![("incomplete".into(), "1".into())] }).unwrap() else { panic!() };
    let archive = b.app.backups.folder().unwrap().join(location.split("created=").nth(1).expect("an incomplete backup"));
    b.backups_page();
    // From the Backups page: Restore, then the chosen file.
    let page_path = format!("/c/{}/backups/", b.co.to_db());
    let token = b.app.tokens.mint(&page_path);
    let Outcome::Done { data } = b.app.run(Command::SubmitForm { path: page_path, token, fields: vec![("action".into(), "restore".into()), ("new_book".into(), "Partial copy".into())] }).unwrap() else { panic!("no dialog") };
    screens::backups::dialog_result(&mut b.app, data["ticket"].as_str().unwrap(), archive.to_str().unwrap()).unwrap();
    let html = b.backups_page();
    assert!(html.contains("Restored “Partial copy”"), "{html}");
    assert!(html.contains("That backup was incomplete: 1 attached file was not in it") && html.contains("“gone.pdf” on Journal entry"), "{html}");
    assert!(!html.contains("“kept.pdf” on"), "only the absent file is named: {html}");
    // From the unlock screen.
    let other_root = tempfile::tempdir().unwrap();
    let mut other = app_at(other_root.path());
    screens::backups::restore_from_gate(&mut other, &archive, "Gate copy", PASSWORD).unwrap();
    let notice = other.upgrade_notice.clone().unwrap_or_default();
    assert!(notice.contains("That backup was incomplete") && notice.contains("“gone.pdf” on Journal entry"), "{notice}");
    support::sign_in(&mut other);
    let restored = other.book_path().unwrap().to_owned();
    assert_eq!(Books::read_in(&mut other, &restored, b.co, &kept).unwrap(), PDF);
}


#[test]
fn an_older_book_missing_its_receipt_key_record_is_unchanged_before_refusal() {
    let mut b = Books::first_run(None);
    let page = b.journal();
    let id = b.attach(&page, "new.pdf", PDF);
    let key = MasterKey::new(**b.app.master_key().unwrap());
    let record = fb_app::protection::record_path(&b.path);
    let saved = b.record_bytes();
    b.app.lock();
    let prior = b.path.with_extension("prior");
    {
        let source = fb_db::Book::open(&b.path, &key).unwrap();
        fb_db::test_support::populated_copy(source.connection(), &prior, &key, "0198_legacy_attachment_key_wraps", false).unwrap();
    }
    fs::rename(&prior, &b.path).unwrap();
    fs::remove_file(&record).unwrap();
    let before = fs::read(&b.path).unwrap();
    let folder = b.app.backups.folder().unwrap();
    let backups_before = retention::backup_files(&folder, None).unwrap_or_default();
    let refusal = b.app.open_with_secret(&b.path, "").unwrap_err().to_string();
    assert!(refusal.contains("key file is missing"), "{refusal}");
    assert_eq!(fs::read(&b.path).unwrap(), before, "not even a schema upgrade precedes refusal");
    assert_eq!(retention::backup_files(&folder, None).unwrap_or_default(), backups_before, "no incomplete safety copy is made with the wrong key");
    assert!(!record.exists());
    assert!(!b.app.is_open());
    fs::write(&record, saved).unwrap();
    b.app.open_with_secret(&b.path, "").unwrap();
    support::sign_in(&mut b.app);
    assert!(b.app.upgrade_notice.is_some());
    assert_eq!(b.read_back(&id), PDF);
}

#[test]
fn a_genuine_legacy_backup_opens_without_exposing_legacy_wraps_or_other_company_ids() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fb-core/tests/data/attachment-first-lane").canonicalize().unwrap();
    let meta: serde_json::Value = serde_json::from_slice(&fs::read(fixture.join("fixture.json")).unwrap()).unwrap();
    let old_detail: serde_json::Value = serde_json::from_str(meta["audit_detail"].as_str().unwrap()).unwrap();
    let root = tempfile::tempdir().unwrap();
    let mut app = app_at(root.path());
    let restored = screens::backups::restore_from_gate(&mut app, &fixture.join("first-lane.fbbackup"), "Legacy restored", "Synthetic legacy book password").unwrap();
    support::sign_in(&mut app);
    let co = CompanyId::parse(meta["company"].as_str().unwrap()).unwrap();
    for (company, id) in [("company", "attachment"), ("other_company", "other_attachment")] {
        assert_eq!(Books::read_in(&mut app, &restored, CompanyId::parse(meta[company].as_str().unwrap()).unwrap(), meta[id].as_str().unwrap()).unwrap(), b"%PDF-1.4\n1 0 obj<</Type/Catalog>>endobj\n%%EOF\n");
    }
    let other_name: String = app.open_mut().unwrap().book.read(|q| q.query_row("SELECT stored_name FROM attachments_attachment WHERE id=?1", [meta["other_attachment"].as_str().unwrap()], |r| r.get(0))).unwrap();
    let history = screens::render(&mut app, &format!("/c/{}/history/", co.to_db())).unwrap().html;
    let csv = screens::render(&mut app, &format!("/c/{}/history/?format=csv", co.to_db())).unwrap().html;
    assert!(csv.starts_with('\u{feff}'), "the CSV export is exercised");
    let token = app.ai_grant(co, "Synthetic legacy history reader", fb_app::ai::Permission::ReadOnly).unwrap()["token"].as_str().unwrap().to_owned();
    let ai = app.ai_call(&token, &serde_json::json!({"operation":"app_AuditHistory","arguments":{"limit":100}}).to_string()).unwrap().to_string();
    for shown in [&history, &csv, &ai] {
        assert!(!shown.contains(meta["other_attachment"].as_str().unwrap()));
        assert!(!shown.contains(&other_name));
        for file in old_detail["files"].as_array().unwrap() {
            for field in ["wrapped_key", "wrap_nonce"] { assert!(!shown.contains(file[field].as_str().unwrap()), "legacy key material escaped"); }
        }
    }
    let raw: String = app.open_mut().unwrap().book.read(|q| q.query_row("SELECT detail FROM ledger_auditevent WHERE action=?1", [a::REWRAPPED], |r| r.get(0))).unwrap();
    assert_eq!(raw, meta["audit_detail"].as_str().unwrap(), "only presentation changes; stored actor evidence does not");
}

#[test]
fn last_verified_finds_a_complete_backup_beyond_the_displayed_page() {
    let mut b = Books::first_run(Some(PASSWORD));
    let page = b.journal();
    let id = b.attach(&page, "gone.pdf", PDF);
    let master = MasterKey::new(**b.app.master_key().unwrap());
    let receipt_key = b.app.receipt_key().unwrap();
    let record = Record::read(&b.path).unwrap().unwrap();
    let folder = b.app.backups.folder().unwrap(); fs::create_dir_all(&folder).unwrap();
    let work = b.root.path().join("status-work"); fs::create_dir(&work).unwrap();
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; fs::set_permissions(&work, fs::Permissions::from_mode(0o700)).unwrap(); }
    let verifier = backups::attachment_receipt_verifier_with(&receipt_key, Some(&master));
    let receipts = b.path.parent().unwrap().join("receipts");
    let make = |app: &App, stamp: &str, incomplete: bool| {
        let output = folder.join(retention::filename("pagination", stamp).unwrap());
        backups::create_portable_backup(&app.open.as_ref().unwrap().book, &master, &backups::BackupOptions {
            output: &output, work_root: &work, receipts_root: Some(&receipts), verify_receipt: Some(&verifier), label: "pagination", app_version: "test", created_at: stamp, passphrase: None, keep: None, allow_incomplete: incomplete,
        }, &record.keys).unwrap();
        backups::verify_backup_with_receipts(&output, &master, &work, None, true, Some(&verifier)).unwrap();
    };
    make(&b.app, "2026-01-01T00:00:00Z", false);
    fs::remove_file(b.stored_file(&id)).unwrap();
    for day in 1..=21 { make(&b.app, &format!("2026-02-{day:02}T00:00:00Z"), true); }
    let html = b.backups_page();
    assert!(html.contains("Showing the 20 newest of 22"), "{html}");
    assert!(!html.contains("No complete backup verified yet"), "the complete backup still counts: {html}");
    assert!(html.contains("2026-01-01 00:00"), "the older complete backup is named: {html}");
    assert!(html.contains("is <strong>incomplete</strong>"), "the newer incomplete backup is still identified: {html}");
}
