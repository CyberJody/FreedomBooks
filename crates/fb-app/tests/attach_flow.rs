//! Attaching a file from a document or journal page: the add POST asks the main process for the native
//! file chooser (a one-time ticket), and the chosen path comes back through `backups::dialog_result`.
mod support;
use fb_app::{App, Command, Outcome, keys, screens};
use fb_core::{CompanyId, services};
use serde_json::json;
use std::path::{Path, PathBuf};

const PDF: &[u8] = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n1 0 obj<</Type/Catalog>>endobj\ntrailer<</Root 1 0 R>>\n%%EOF\n";

struct Fixture {
    _dir: tempfile::TempDir,
    app: App,
    co: CompanyId,
    files: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let files = dir.path().join("picked");
        std::fs::create_dir(&files).unwrap();
        std::fs::create_dir(dir.path().join("books")).unwrap();
        let mut app = App::new();
        let co = app.create_book(&dir.path().join("books/default.fbbook"), &keys::new_master_key().unwrap(), "Owner", "Attach Co").unwrap().company_id;
        support::sign_in(&mut app);
        Self { _dir: dir, app, co, files }
    }
    fn account(&mut self, code: &str, kind: &str) -> String {
        let o = self.app.open_mut().unwrap();
        let mut v = services::AccountInput::new(code, code);
        v.kind = Some(kind.into());
        services::save_account(&mut o.book, o.owner, self.co, v).unwrap().id.to_db()
    }
    fn invoice(&mut self) -> String {
        let (ar, uf, bank, income) = (self.account("1100", "ASSET"), self.account("1200", "ASSET"), self.account("1000", "ASSET"), self.account("4000", "INCOME"));
        let o = self.app.open_mut().unwrap();
        fb_core::documents::save_setup(&mut o.book, o.owner, self.co, &json!({"receivables":ar,"undeposited":uf,"banks":[bank]}), Some(0)).unwrap();
        let customer = fb_core::directory::save_record(&mut o.book, o.owner, self.co, fb_core::directory::Model::Customer, &json!({"display_name":"Harbor Street Cafe"}), None).unwrap()["id"].as_str().unwrap().to_owned();
        let values = serde_json::from_value(json!({"request_key":uuid::Uuid::new_v4().to_string(),"customer_id":customer,"date":"2026-10-01","due_date":"2026-11-01","tax_mode":"NONE","lines":[{"description":"Work","quantity":"1","rate":"100.00","income_account_id":income}]})).unwrap();
        let Outcome::Done { data } = self.app.run(Command::SaveInvoiceDraft { company_id: self.co.to_db(), invoice_id: None, expected_revision: 0, values }).unwrap() else { panic!() };
        format!("/c/{}/invoices/{}/", self.co.to_db(), data["id"].as_str().unwrap())
    }
    fn bill(&mut self) -> String {
        let expense = self.account("6000", "EXPENSE");
        let open = self.app.open_mut().unwrap();
        let vendor = fb_core::directory::save_record(&mut open.book, open.owner, self.co, fb_core::directory::Model::Vendor, &json!({"display_name":"Synthetic vendor"}), None).unwrap();
        let bill = fb_core::payables::save_bill(&mut open.book, open.owner, self.co, &json!({
            "vendor_id": vendor["id"], "date": "2026-10-01", "due_date": "2026-11-01",
            "request_key": uuid::Uuid::new_v4().to_string(),
            "lines": [{"description":"Supplies", "quantity":"1", "rate":"10.00", "expense_account_id":expense}]
        })).unwrap();
        format!("/c/{}/bills/{}/", self.co.to_db(), bill["id"].as_str().unwrap())
    }
    fn journal(&mut self) -> String {
        let (bank, equity) = (self.account("1000", "ASSET"), self.account("3000", "EQUITY"));
        let o = self.app.open_mut().unwrap();
        let (b, e) = (fb_core::AccountId::parse(&bank).unwrap(), fb_core::AccountId::parse(&equity).unwrap());
        let j = services::save_draft(&mut o.book, o.owner, self.co, services::DraftInput::new("2026-10-01", "Opening", vec![services::line(b).debit("10").build(), services::line(e).credit("10").build()])).unwrap();
        let id = services::post_journal(&mut o.book, o.owner, self.co, j.id, None).unwrap().id;
        format!("/c/{}/journals/{}/", self.co.to_db(), id.to_db())
    }
    fn file(&self, name: &str, data: &[u8]) -> PathBuf {
        let p = self.files.join(name);
        std::fs::write(&p, data).unwrap();
        p
    }
    fn count(&mut self) -> i64 {
        self.app.open_mut().unwrap().book.read(|q| q.query_row("SELECT count(*) FROM attachments_attachment", [], |r| r.get(0))).unwrap()
    }
    /// Render `page`, post its Attach form as the preload does (text fields only), and return the ticket.
    fn attach(&mut self, page: &str) -> String {
        let html = screens::render(&mut self.app, page).unwrap().html;
        let form = html.split_once(r#"<form class="attachments-add""#).expect("an Attach form").1;
        let action = attr(form, "action");
        let token = value(form, "fb_command_token");
        let request_key = value(form, "request_key");
        let outcome = self.app.run(Command::SubmitForm { path: action, token, fields: vec![("request_key".into(), request_key)] }).unwrap();
        let Outcome::Done { data } = outcome else { panic!("expected the chooser ticket, got {outcome:?}") };
        assert_eq!(data["desktop"], "pick-attachment");
        assert_eq!(data.as_object().unwrap().len(), 2, "only the kind and the ticket reach the main process: {data}");
        data["ticket"].as_str().unwrap().to_owned()
    }
    fn picked(&mut self, ticket: &str, chosen: Option<&Path>) -> Outcome {
        screens::backups::dialog_result(&mut self.app, ticket, chosen.map_or("", |p| p.to_str().unwrap())).unwrap()
    }
}

fn attr(html: &str, name: &str) -> String {
    html.split_once(&format!("{name}=\"")).unwrap().1.split('"').next().unwrap().to_owned()
}
fn value(html: &str, name: &str) -> String {
    html.split_once(&format!("name=\"{name}\" value=\"")).unwrap_or_else(|| panic!("no {name} field: {html}")).1.split('"').next().unwrap().to_owned()
}

#[test]
fn both_download_links_export_the_original_pdf_and_png_only_from_the_owning_company() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    for document in [Fixture::invoice, Fixture::bill, Fixture::journal] {
        let mut f = Fixture::new();
        let page = document(&mut f);
        let open = f.app.open_mut().unwrap();
        let other = services::create_company(&mut open.book, open.owner, "Second company", "USD", false).unwrap().id;
        for (name, mime, bytes) in [("receipt.pdf", "application/pdf", PDF.to_vec()), ("scan.png", "image/png", png())] {
            let ticket = f.attach(&page);
            let path = f.file(name, &bytes);
            f.picked(&ticket, Some(&path));
            let html = screens::render(&mut f.app, &page).unwrap().html;
            let (before, _) = html.split_once(&format!(" download>{name}</a>")).unwrap();
            let url = before.rsplit_once("href=\"").unwrap().1.trim_end_matches('"');
            assert!(html.contains(&format!("href=\"{url}\" download>Download</a>")));
            let id = url.strip_prefix(&format!("/c/{}/attachments/", f.co.to_db())).unwrap().strip_suffix("/download/").unwrap();
            let export = |co: CompanyId, id: &str| Command::ExportAttachment { company_id: co.to_db(), attachment_id: id.into() };
            let Outcome::Done { data } = f.app.run_desktop(export(f.co, id)).unwrap() else { panic!("expected attachment bytes") };
            assert_eq!(data["filename"], name);
            assert_eq!(data["mime"], mime);
            assert_eq!(STANDARD.decode(data["base64"].as_str().unwrap()).unwrap(), bytes);
            assert!(f.app.run_desktop(export(other, id)).is_err());
            assert!(f.app.run_desktop(export(f.co, "invalid")).is_err());
        }
    }
}

#[test]
fn an_invoice_attaches_the_picked_pdf_and_lists_it_with_a_note() {
    let mut f = Fixture::new();
    let page = f.invoice();
    let ticket = f.attach(&page);
    let pdf = f.file("inv1.pdf", PDF);
    assert_eq!(f.picked(&ticket, Some(&pdf)), Outcome::Redirect { location: format!("{page}#attachments") });
    assert_eq!(f.count(), 1);
    let html = screens::render(&mut f.app, &page).unwrap().html;
    assert!(html.contains(r#"<span class="attachments-count">(1)</span>"#), "{html}");
    assert!(html.contains(">inv1.pdf</a>") && html.contains("PDF document"), "{html}");
    assert!(html.contains(r#"role="status">Attached inv1.pdf.</p>"#), "{html}");
    // The row's remove form carries its own command token, and removing works from this page too.
    let remove = html.split_once(r#"<form class="attachments-remove-form""#).unwrap().1;
    let (path, token) = (attr(remove, "action"), value(remove, "fb_command_token"));
    f.app.run(Command::SubmitForm { path, token, fields: vec![("reason".into(), "Wrong file".into())] }).unwrap();
    let html = screens::render(&mut f.app, &page).unwrap().html;
    assert!(html.contains("Removed inv1.pdf.") && html.contains("Removed (1)"), "{html}");
    // A ticket is single use.
    assert!(screens::backups::dialog_result(&mut f.app, &ticket, pdf.to_str().unwrap()).is_err());
}

#[test]
fn a_manual_journal_attaches_through_the_same_chooser() {
    let mut f = Fixture::new();
    let page = f.journal();
    let ticket = f.attach(&page);
    let png = f.file("scan.png", &png());
    assert_eq!(f.picked(&ticket, Some(&png)), Outcome::Redirect { location: format!("{page}#attachments") });
    let html = screens::render(&mut f.app, &page).unwrap().html;
    assert!(html.contains(r#"<span class="attachments-count">(1)</span>"#) && html.contains(">scan.png</a>"), "{html}");
}

/// Worker item 41: a chooser that hands back no file (Cancel; and Enter or a double-click in Electron's own
/// GTK chooser, used when the desktop has no file-chooser portal, which reports those as a cancel) stores
/// nothing and says so, instead of looking like an attach that silently did nothing.
#[test]
fn a_chooser_that_returns_no_file_stores_nothing_and_says_so() {
    let mut f = Fixture::new();
    let page = f.invoice();
    let ticket = f.attach(&page);
    assert_eq!(f.picked(&ticket, None), Outcome::Redirect { location: format!("{page}#attachments") });
    assert_eq!(f.count(), 0);
    let html = screens::render(&mut f.app, &page).unwrap().html;
    assert!(html.contains(r#"<p class="attachments-note attachments-warning" role="status">No file was chosen, so nothing was attached. Choose a file and click Open.</p>"#), "{html}");
    // The note is shown once: the next visit to the page says nothing.
    let html = screens::render(&mut f.app, &page).unwrap().html;
    assert!(!html.contains("attachments-note"), "{html}");
    // The file the Open button returns for the same chooser is attached.
    let ticket = f.attach(&page);
    let pdf = f.file("R001.pdf", PDF);
    f.picked(&ticket, Some(&pdf));
    assert_eq!(f.count(), 1);
}

/// Worker item 44: the same receipt (the same bytes, whatever its name) attached twice to one document is
/// refused with the document named; attached to another document it is kept, with a warning naming the first.
#[test]
fn the_same_file_twice_is_refused_on_one_document_and_named_on_another() {
    let mut f = Fixture::new();
    let page = f.invoice();
    let first = f.file("R001-2026-06-06-Stewarts.jpg.pdf", PDF);
    let ticket = f.attach(&page);
    f.picked(&ticket, Some(&first));
    assert_eq!(f.count(), 1);
    let number = f.app.open_mut().unwrap().book.read(|q| q.query_row("SELECT number FROM documents_documentidentity WHERE kind='INV'", [], |r| r.get::<_, String>(0))).unwrap();
    // The same bytes under another name, on the same invoice: refused, nothing stored.
    let copy = f.file("copy of receipt.pdf", PDF);
    let ticket = f.attach(&page);
    f.picked(&ticket, Some(&copy));
    assert_eq!(f.count(), 1);
    let html = screens::render(&mut f.app, &page).unwrap().html;
    assert!(html.contains(&format!(r#"role="alert">This file is already attached to {number}, as R001-2026-06-06-Stewarts.jpg.pdf. It was not attached again.</p>"#)), "{html}");
    assert!(html.contains(r#"<span class="attachments-count">(1)</span>"#), "{html}");
    // Another file on the same invoice is attached as before.
    let other = f.file("other.pdf", b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n1 0 obj<</Type/Catalog>>endobj\n% another receipt\ntrailer<</Root 1 0 R>>\n%%EOF\n");
    let ticket = f.attach(&page);
    f.picked(&ticket, Some(&other));
    assert_eq!(f.count(), 2);
    // The first file on a journal entry: attached, with a warning naming the invoice.
    let journal = {
        let id = |f: &mut Fixture, code: &str| f.app.open_mut().unwrap().book.read(|q| q.query_row("SELECT id FROM ledger_account WHERE code=?1", [code], |r| r.get::<_, String>(0))).unwrap();
        let (b, e) = (fb_core::AccountId::parse(&id(&mut f, "1000")).unwrap(), fb_core::AccountId::parse(&id(&mut f, "4000")).unwrap());
        let o = f.app.open_mut().unwrap();
        let j = services::save_draft(&mut o.book, o.owner, f.co, services::DraftInput::new("2026-10-02", "Supplies", vec![services::line(b).debit("10").build(), services::line(e).credit("10").build()])).unwrap();
        let id = services::post_journal(&mut o.book, o.owner, f.co, j.id, None).unwrap().id;
        format!("/c/{}/journals/{}/", f.co.to_db(), id.to_db())
    };
    let ticket = f.attach(&journal);
    f.picked(&ticket, Some(&first));
    assert_eq!(f.count(), 3);
    let html = screens::render(&mut f.app, &journal).unwrap().html;
    assert!(html.contains(r#"role="status">Attached R001-2026-06-06-Stewarts.jpg.pdf.</p>"#), "{html}");
    assert!(html.contains(&format!(r#"<p class="attachments-note attachments-warning" role="status">The same file is also attached to {number}. Check that this is the right document for it.</p>"#)), "{html}");
    // Once removed from the invoice, the file no longer counts there: it may be attached to it again.
    let html = screens::render(&mut f.app, &page).unwrap().html;
    let remove = html.split_once(r#"<form class="attachments-remove-form""#).unwrap().1;
    let (path, token) = (attr(remove, "action"), value(remove, "fb_command_token"));
    f.app.run(Command::SubmitForm { path, token, fields: vec![("reason".into(), "Attached twice".into())] }).unwrap();
    let ticket = f.attach(&page);
    f.picked(&ticket, Some(&first));
    assert_eq!(f.count(), 4);
}

#[test]
fn refused_files_store_nothing_and_say_why_on_the_panel() {
    let mut f = Fixture::new();
    let page = f.invoice();
    let big = f.file("big.pdf", &[PDF, &vec![b' '; 10 * 1024 * 1024]].concat());
    let text = f.file("notes.txt", b"plain text, not a receipt");
    let folder = f.files.clone();
    let cases: [(&Path, &str); 3] = [
        (&big, "The file is larger than 10 MB, so it was not attached."),
        (&text, "notes.txt"),
        (&folder, "Choose a file, not a folder."),
    ];
    for (chosen, said) in cases {
        let ticket = f.attach(&page);
        f.picked(&ticket, Some(chosen));
        let html = screens::render(&mut f.app, &page).unwrap().html;
        let note = html.split_once(r#"role="alert">"#).unwrap_or_else(|| panic!("no error note for {chosen:?}: {html}")).1;
        assert!(note.split("</p>").next().unwrap().contains(said), "{chosen:?}: {note}");
    }
    assert_eq!(f.count(), 0);
}

#[test]
fn the_chosen_path_must_be_absolute_and_an_outsider_document_is_not_found() {
    let mut f = Fixture::new();
    let page = f.invoice();
    let ticket = f.attach(&page);
    assert!(screens::backups::dialog_result(&mut f.app, &ticket, "inv1.pdf").is_err());
    assert_eq!(f.count(), 0);
    let path = format!("/c/{}/attachments/document/{}/add/", f.co.to_db(), uuid::Uuid::new_v4());
    let token = f.app.tokens.mint(&path);
    assert!(matches!(f.app.run(Command::SubmitForm { path, token, fields: vec![] }), Err(fb_app::AppError::NotFound(_))));
}

#[test]
fn a_form_without_a_command_token_is_not_called_expired() {
    let mut f = Fixture::new();
    let page = f.invoice();
    let err = f.app.run(Command::SubmitForm { path: page.clone(), token: String::new(), fields: vec![] }).unwrap_err().to_string();
    assert!(!err.contains("expired") && err.contains("could not be sent"), "{err}");
    let err = f.app.run(Command::SubmitForm { path: page, token: "00".into(), fields: vec![] }).unwrap_err().to_string();
    assert!(err.contains("expired"), "{err}");
}

/// A valid 1x1 PNG (the same bytes as examples/backup_proof.rs).
fn png() -> Vec<u8> {
    let hex = "89504e470d0a1a0a0000000d4948445200000001000000010802000000907753de0000000c49444154789c6338b1a50200043c01f5201698a50000000049454e44ae426082";
    (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect()
}

/// Run the real dialog completion in an owned child so the pre-fix FIFO cannot hang the suite.
#[cfg(target_os = "linux")]
#[test]
fn fifo_dialog_rejection_is_bounded_consumes_ticket_and_leaves_book_unchanged() {
    const CHILD_READY: &str = "FB_ATTACH_FLOW_CHILD_READY";
    if let Some(ready) = std::env::var_os(CHILD_READY) {
        let mut f = Fixture::new();
        let page = f.invoice();
        let path = f.files.join("receipt.pdf");
        assert!(std::process::Command::new("mkfifo").arg("--").arg(&path).status().unwrap().success());
        let ticket = f.attach(&page);
        let changes = |f: &mut Fixture| -> i64 {
            f.app.open_mut().unwrap().book.read(|q| q.query_row("SELECT total_changes()", [], |r| r.get(0))).unwrap()
        };
        let before = changes(&mut f);
        std::fs::write(&ready, b"ready").unwrap();
        assert_eq!(f.picked(&ticket, Some(&path)), Outcome::Redirect { location: format!("{page}#attachments") });
        std::fs::write(PathBuf::from(&ready).with_extension("returned"), b"returned").unwrap();
        assert_eq!(changes(&mut f), before, "refusal wrote to the book, including audit or financial rows");
        assert_eq!(f.count(), 0);
        assert!(screens::backups::dialog_result(&mut f.app, &ticket, path.to_str().unwrap()).is_err());
        let html = screens::render(&mut f.app, &page).unwrap().html;
        assert!(html.contains(r#"role="alert">Choose a file, not a folder.</p>"#), "{html}");
        // A subsequent genuine PDF proves dialog completion released the pending operation.
        let ticket = f.attach(&page);
        let pdf = f.file("ordinary.pdf", PDF);
        f.picked(&ticket, Some(&pdf));
        assert_eq!(f.count(), 1);
        return;
    }

    struct OwnedChild(std::process::Child);
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let ready = dir.path().join("ready");
    let returned = ready.with_extension("returned");
    let mut child = OwnedChild(std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "fifo_dialog_rejection_is_bounded_consumes_ticket_and_leaves_book_unchanged", "--nocapture"])
        .env(CHILD_READY, &ready)
        .spawn().unwrap());
    let mut deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    let mut started = false;
    let mut completed_read = false;
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert!(status.success(), "dialog child failed: {status}");
            break;
        }
        if !started && ready.exists() {
            started = true;
            deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        }
        if started && !completed_read && returned.exists() {
            completed_read = true;
            deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        }
        assert!(std::time::Instant::now() < deadline, "dialog child exceeded deadline (read started: {started}, returned: {completed_read})");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
