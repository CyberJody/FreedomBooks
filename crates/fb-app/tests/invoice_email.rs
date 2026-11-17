mod support;
use fb_app::{App, Command, Outcome, invoice_email, screens};
use fb_core::{CompanyId, directory, documents, invoice_email as core, sales, services};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

const SECRET: &str = "SYNTHETIC_GOOGLE_CLIENT_SECRET";
const RECORD: &str = "SYNTHETIC_PRIVATE_REFRESH_TOKEN";
const PDF: &[u8] = b"%PDF-1.7\nsynthetic invoice PDF";
#[derive(Default)]
struct Requests {
    ops: Vec<String>,
    mail: Vec<Value>,
    uncertain: bool,
}
struct Net(Arc<Mutex<Requests>>);
impl fb_app::net::NetHost for Net {
    fn call(&mut self, r: &Value) -> Result<Value, String> {
        let mut log = self.0.lock().unwrap();
        let op = r["op"].as_str().unwrap();
        log.ops.push(op.into());
        Ok(match op {
            "gmail_begin" => {
                assert_eq!(r["gmail"]["client_secret"], SECRET);
                json!({"ok":true,"authorization_url":"https://accounts.google.com/o/oauth2/v2/auth?synthetic=1"})
            }
            "gmail_poll" => json!({"ok":true,"record":RECORD,"from_address":"sender@example.test"}),
            "gmail_cancel" => json!({"ok":true}),
            "gmail_disconnect" => json!({"ok":true,"revoked":true}),
            "gmail_deliver" => {
                assert_eq!(r["record"], RECORD);
                log.mail.push(r["gmail"]["delivery"].clone());
                if log.uncertain {
                    return Err("Synthetic broken pipe after acceptance".into());
                }
                json!({"ok":true,"outcome":"accepted","record":RECORD,"message_id":"message123","draft_id":if r["gmail"]["delivery"]["mode"]=="DRAFT"{Some("draft123")}else{None}})
            }
            _ => panic!("unexpected operation {op}"),
        })
    }
}
struct F {
    app: App,
    c: CompanyId,
    invoice: String,
    log: Arc<Mutex<Requests>>,
    _dir: tempfile::TempDir,
}
impl F {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new();
        app.invoice_delivery = true;
        app.os_store_usable = true;
        let c = app
            .create_book(
                &dir.path().join("synthetic.fbbook"),
                &fb_app::keys::new_master_key().unwrap(),
                "Owner",
                "Synthetic seller",
            )
            .unwrap()
            .company_id;
        support::sign_in(&mut app);
        let o = app.open_mut().unwrap();
        let ar = services::save_account(
            &mut o.book,
            o.owner,
            c,
            services::AccountInput::new("110", "Receivables").kind("ASSET"),
        )
        .unwrap()
        .id;
        let income = services::save_account(
            &mut o.book,
            o.owner,
            c,
            services::AccountInput::new("400", "Services").kind("INCOME"),
        )
        .unwrap()
        .id;
        documents::save_setup(
            &mut o.book,
            o.owner,
            c,
            &json!({"receivables":ar.to_db()}),
            Some(0),
        )
        .unwrap();
        let customer = directory::save_record(
            &mut o.book,
            o.owner,
            c,
            directory::Model::Customer,
            &json!({"display_name":"Synthetic <buyer>","email":"buyer@example.test"}),
            None,
        )
        .unwrap();
        let invoice=sales::save_invoice(&mut o.book,o.owner,c,&json!({"customer_id":customer["id"],"date":"2026-10-01","due_date":"2026-10-15","request_key":uuid::Uuid::new_v4().to_string(),"tax_mode":"NONE","lines":[{"description":"Synthetic service","quantity":"1","rate":"10.00","taxable":false,"income_account_id":income.to_db()}]})).unwrap();
        let invoice = invoice["id"].as_str().unwrap().to_owned();
        let review = sales::review_invoice(&mut o.book, o.owner, c, &invoice, Some(1)).unwrap();
        sales::post_invoice(&mut o.book, o.owner, c, &invoice, Some(1), Some(&review)).unwrap();
        fb_core::company_documents::save_with_preferences(
            &mut o.book,
            o.owner,
            c,
            "Synthetic seller",
            "USD",
            fb_core::company_documents::Changes {
                details: Default::default(),
                logo_upload: None,
                remove_logo: false,
            },
            Some(fb_core::invoice_preferences::Change {
                expected_revision: 0,
                settings: fb_core::invoice_preferences::Settings {
                    email_greeting: "Hi {customer_name},".into(),
                    ..Default::default()
                },
            }),
        )
        .unwrap();
        let log = Arc::new(Mutex::new(Requests::default()));
        app.net = Some(Box::new(Net(log.clone())));
        Self {
            app,
            c,
            invoice,
            log,
            _dir: dir,
        }
    }
    fn settings(&self) -> String {
        format!("/c/{}/edit/", self.c)
    }
    fn preview(&self) -> String {
        format!("/c/{}/invoices/{}/email-preview/", self.c, self.invoice)
    }
    fn submit(&mut self, path: String, fields: &[(&str, &str)]) -> fb_app::AppResult<Outcome> {
        let token = self.app.tokens.mint(&path);
        self.app.run_renderer(Command::SubmitForm {
            path,
            token,
            fields: fields
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        })
    }
    fn connect(&mut self) {
        let out = self
            .submit(
                self.settings(),
                &[
                    ("action", "gmail_connect"),
                    ("gmail_revision", "0"),
                    ("gmail_client_id", "synthetic.apps.googleusercontent.com"),
                    ("gmail_client_secret", SECRET),
                    ("gmail_from", "sender@example.test"),
                ],
            )
            .unwrap();
        assert!(matches!(out, Outcome::External { .. }));
        self.submit(self.settings(), &[("action", "gmail_poll")])
            .unwrap();
    }
    fn prepare(&mut self, mode: &str) -> String {
        let Outcome::Done { data } = self.submit(self.preview(), &[("action", mode)]).unwrap()
        else {
            panic!("not prepared")
        };
        data["invoice_email_pdf"].as_str().unwrap().into()
    }
    fn history(&mut self) -> Vec<core::Attempt> {
        let o = self.app.open_mut().unwrap();
        o.book
            .read(|q| core::history(q, o.owner, self.c, &self.invoice))
            .unwrap()
    }
}
#[test]
fn company_google_connection_drafts_and_send_use_host_snapshot_and_sealed_tokens() {
    let mut f = F::new();
    f.connect();
    let settings = f.settings();
    let html = screens::render(&mut f.app, &settings).unwrap().html;
    assert!(html.contains("CONNECTED"));
    assert!(!html.contains(SECRET) && !html.contains(RECORD));
    for action in ["email_draft", "email_send"] {
        let ticket = f.prepare(action);
        let print = invoice_email::print_document(&mut f.app, &ticket).unwrap();
        assert!(print.contains("Synthetic service"));
        let result = invoice_email::finish_pdf(&mut f.app, &ticket, PDF).unwrap();
        assert!(matches!(result, Outcome::Redirect { .. }));
        assert!(invoice_email::finish_pdf(&mut f.app, &ticket, PDF).is_err());
    }
    let log = f.log.lock().unwrap();
    assert_eq!(log.mail.len(), 2);
    assert_eq!(log.mail[0]["mode"], "DRAFT");
    assert_eq!(log.mail[1]["mode"], "SEND");
    assert!(
        log.mail[0]["html"]
            .as_str()
            .unwrap()
            .contains("Synthetic &lt;buyer&gt;")
    );
    assert_eq!(log.mail[0]["to"], "buyer@example.test");
    drop(log);
    let history = f.history();
    assert_eq!(history[0].state, "SENT");
    assert_eq!(history[1].state, "DRAFTED");
    let o = f.app.open_mut().unwrap();
    let snapshot = fb_db::test_support::snapshot(o.book.connection()).unwrap();
    for rows in snapshot.values() {
        for value in rows.iter().flatten() {
            if let fb_db::rusqlite::types::Value::Text(text) = value {
                assert!(!text.contains(SECRET) && !text.contains(RECORD));
            }
        }
    }
    assert!(o.book.check().unwrap().is_empty());
}
#[test]
fn changed_invoice_settings_and_print_failure_never_contact_gmail() {
    let mut f = F::new();
    f.connect();
    let ticket = f.prepare("email_draft");
    let o = f.app.open_mut().unwrap();
    fb_core::company_documents::save(
        &mut o.book,
        o.owner,
        f.c,
        "Synthetic seller",
        "USD",
        fb_core::company_documents::Changes {
            details: fb_core::company_documents::Details {
                street1: "Changed billing address".into(),
                ..Default::default()
            },
            logo_upload: None,
            remove_logo: false,
        },
    )
    .unwrap();
    invoice_email::finish_pdf(&mut f.app, &ticket, PDF).unwrap();
    assert_eq!(f.history()[0].state, "REFUSED");
    let ticket = f.prepare("email_send");
    invoice_email::finish_pdf(&mut f.app, &ticket, &[]).unwrap();
    assert_eq!(f.history()[0].state, "REFUSED");
    assert!(f.log.lock().unwrap().mail.is_empty());
}
#[test]
fn lost_response_is_unknown_and_session_change_expires_pdf_capability() {
    let mut f = F::new();
    f.connect();
    f.log.lock().unwrap().uncertain = true;
    let ticket = f.prepare("email_send");
    invoice_email::finish_pdf(&mut f.app, &ticket, PDF).unwrap();
    assert_eq!(f.history()[0].state, "UNKNOWN");
    assert_eq!(f.log.lock().unwrap().mail.len(), 1);
    assert!(invoice_email::finish_pdf(&mut f.app, &ticket, PDF).is_err());
    assert!(f.submit(f.preview(), &[("action", "email_draft")]).is_err());
    let unknown = f.history()[0].id.clone();
    let Outcome::Done { data } = f
        .submit(
            f.preview(),
            &[
                ("action", "email_draft"),
                ("email_checked", "on"),
                ("unknown_attempt", &unknown),
            ],
        )
        .unwrap()
    else {
        panic!("new explicit attempt")
    };
    let ticket = data["invoice_email_pdf"].as_str().unwrap().to_owned();
    f.app.sign_out();
    assert!(invoice_email::print_document(&mut f.app, &ticket).is_err());
    assert!(invoice_email::finish_pdf(&mut f.app, &ticket, PDF).is_err());
    assert_eq!(f.log.lock().unwrap().mail.len(), 1);
}
#[test]
fn default_build_and_missing_keyring_cannot_connect_or_send() {
    let mut f = F::new();
    f.app.invoice_delivery = false;
    assert!(f.submit(f.settings(), &[("action", "gmail_poll")]).is_err());
    f.app.invoice_delivery = true;
    f.app.os_store_usable = false;
    assert!(
        f.submit(
            f.settings(),
            &[
                ("action", "gmail_connect"),
                ("gmail_revision", "0"),
                ("gmail_client_id", "synthetic"),
                ("gmail_client_secret", SECRET),
                ("gmail_from", "sender@example.test")
            ]
        )
        .is_err()
    );
    assert!(f.log.lock().unwrap().ops.is_empty());
}

#[test]
fn stale_google_connect_does_not_cancel_the_current_authorization() {
    let mut f = F::new();
    let fields = [
        ("action", "gmail_connect"),
        ("gmail_revision", "0"),
        ("gmail_client_id", "synthetic.apps.googleusercontent.com"),
        ("gmail_client_secret", SECRET),
        ("gmail_from", "sender@example.test"),
    ];
    f.submit(f.settings(), &fields).unwrap();
    let before = f.log.lock().unwrap().ops.clone();
    assert!(f.submit(f.settings(), &fields).is_err());
    assert_eq!(f.log.lock().unwrap().ops, before);
    f.submit(f.settings(), &[("action", "gmail_poll")]).unwrap();
    let o = f.app.open_mut().unwrap();
    assert_eq!(
        o.book
            .read(|q| core::connection(q, f.c))
            .unwrap()
            .unwrap()
            .status,
        "CONNECTED"
    );
}

#[test]
fn branded_email_uses_compiled_inline_assets_without_local_image_urls() {
    let mut f=F::new();
    let o=f.app.open_mut().unwrap();
    let mut settings=o.book.read(|q|fb_core::invoice_preferences::get(q,f.c)).unwrap();
    settings.settings.zelle_enabled=true;settings.settings.zelle_recipient="seller@example.test".into();
    let details=o.book.read(|q|fb_core::company_documents::get(q,f.c)).unwrap();
    fb_core::company_documents::save_with_preferences(&mut o.book,o.owner,f.c,"Synthetic seller","USD",fb_core::company_documents::Changes{details,logo_upload:None,remove_logo:false},Some(fb_core::invoice_preferences::Change{expected_revision:settings.revision,settings:settings.settings})).unwrap();
    f.connect();let ticket=f.prepare("email_draft");invoice_email::finish_pdf(&mut f.app,&ticket,PDF).unwrap();
    let log=f.log.lock().unwrap();let mail=&log.mail[0];
    assert_eq!(mail["brand_images"],json!(["zelle"]));
    let html=mail["html"].as_str().unwrap();assert!(html.contains("cid:freedombooks-pay-zelle"));assert!(!html.contains("src=\"/static/"));
}
