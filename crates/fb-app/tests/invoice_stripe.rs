mod support;
use fb_app::{App, Command, Outcome, screens};
use fb_core::{
    CompanyId, company_documents, directory, documents, invoice_preferences,
    invoice_stripe as core, sales, services,
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
const KEY: &str = "rk_live_SYNTHETIC_ONLY_1234";
#[derive(Default)]
struct Log {
    ops: Vec<String>,
    paid: bool,
    refund: bool,
    unknown: bool,
    offline: bool,
    closed: bool,
    foreign_request: bool,
    reactivated_request: String,
}
struct Net(Arc<Mutex<Log>>);
impl fb_app::net::NetHost for Net {
    fn call(&mut self, r: &Value) -> Result<Value, String> {
        let mut l = self.0.lock().unwrap();
        let op = r["op"].as_str().unwrap();
        l.ops.push(op.into());
        if l.offline {
            return Err("synthetic offline".into());
        }
        let record: Value = serde_json::from_str(r["record"].as_str().unwrap()).unwrap();
        assert_eq!(record["key"], KEY);
        assert_eq!(record["mode"], "LIVE");
        if op != "stripe_probe" {
            assert_eq!(record["account"], "acct_fixture")
        };
        let remote =
            json!({"id":"plink_fixture","url":"https://buy.stripe.com/fixture","active":!l.closed});
        Ok(match op {
            "stripe_probe" => json!({"ok":true,"account":"acct_fixture","mode":"LIVE"}),
            "stripe_conflicts" => {
                let blocking = if l.reactivated_request.is_empty() {
                    vec![]
                } else {
                    let known = json!({"request":l.reactivated_request,"provider":"plink_fixture"});
                    assert!(
                        r["stripe"]["known_links"]
                            .as_array()
                            .unwrap()
                            .contains(&known)
                    );
                    vec![known]
                };
                json!({"ok":true,"conflict":l.foreign_request || !blocking.is_empty(),"blocking":blocking,"next":""})
            }
            "stripe_create" => {
                assert_eq!(r["stripe"]["link"]["amount"], 1000);
                if l.unknown {
                    return Err("synthetic timeout after creation".into());
                }
                json!({"ok":true,"outcome":"created","link":remote})
            }
            "stripe_recover" => json!({"ok":true,"found":remote,"next":""}),
            "stripe_deactivate" => {
                l.closed = true;
                json!({"ok":true,"link":{"id":"plink_fixture","url":"https://buy.stripe.com/fixture","active":false},"closed":true})
            }
            "stripe_refresh" => {
                json!({"ok":true,"link":remote,"observations":if l.paid{vec![json!({"session":"cs_fixture","intent":"pi_fixture","amount":1000,"received":1000,"refunded":if l.refund{500}else{0},"disputed":false,"currency":"USD","status":if l.refund{"REFUNDED"}else{"READY"},"created":1790899200})]}else{vec![]},"next":""})
            }
            _ => panic!("unexpected operation {op}"),
        })
    }
}
struct F {
    app: App,
    c: CompanyId,
    invoice: String,
    log: Arc<Mutex<Log>>,
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
                "Synthetic owner",
                "Synthetic seller",
            )
            .unwrap()
            .company_id;
        support::sign_in(&mut app);
        let o = app.open.as_mut().unwrap();
        let ar = services::save_account(
            &mut o.book,
            o.owner,
            c,
            services::AccountInput::new("110", "Receivables").kind("ASSET"),
        )
        .unwrap()
        .id;
        let uf = services::save_account(
            &mut o.book,
            o.owner,
            c,
            services::AccountInput::new("120", "Undeposited Funds").kind("ASSET"),
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
            &json!({"receivables":ar.to_db(),"undeposited":uf.to_db()}),
            Some(0),
        )
        .unwrap();
        let customer = directory::save_record(
            &mut o.book,
            o.owner,
            c,
            directory::Model::Customer,
            &json!({"display_name":"Synthetic buyer","email":"buyer@example.test"}),
            None,
        )
        .unwrap();
        let inv=sales::save_invoice(&mut o.book,o.owner,c,&json!({"customer_id":customer["id"],"date":"2026-10-01","due_date":"2026-10-15","request_key":uuid::Uuid::new_v4().to_string(),"tax_mode":"NONE","lines":[{"description":"Synthetic service","quantity":"1","rate":"10.00","taxable":false,"income_account_id":income.to_db()}]})).unwrap();
        let invoice = inv["id"].as_str().unwrap().to_owned();
        let review = sales::review_invoice(&mut o.book, o.owner, c, &invoice, Some(1)).unwrap();
        sales::post_invoice(&mut o.book, o.owner, c, &invoice, Some(1), Some(&review)).unwrap();
        company_documents::save_with_preferences(
            &mut o.book,
            o.owner,
            c,
            "Synthetic seller",
            "USD",
            company_documents::Changes {
                details: Default::default(),
                logo_upload: None,
                remove_logo: false,
            },
            Some(invoice_preferences::Change {
                expected_revision: 0,
                settings: invoice_preferences::Settings {
                    stripe_enabled: true,
                    stripe_mode: "LIVE".into(),
                    ..Default::default()
                },
            }),
        )
        .unwrap();
        let log = Arc::new(Mutex::new(Log::default()));
        app.net = Some(Box::new(Net(log.clone())));
        Self {
            app,
            c,
            invoice,
            log,
            _dir: dir,
        }
    }
    fn page(&self) -> String {
        format!("/c/{}/stripe/", self.c)
    }
    fn settings(&self) -> String {
        format!("/c/{}/edit/", self.c)
    }
    fn invoice_path(&self) -> String {
        format!("/c/{}/invoices/{}/", self.c, self.invoice)
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
        self.submit(
            self.settings(),
            &[
                ("action", "stripe_connect"),
                ("stripe_mode", "LIVE"),
                ("stripe_revision", "0"),
                ("stripe_key", KEY),
            ],
        )
        .unwrap();
    }
    fn create(&mut self) {
        let invoice = self.invoice.clone();
        self.submit(
            self.invoice_path(),
            &[("action", "stripe_create"), ("id", &invoice)],
        )
        .unwrap();
    }
    fn link(&mut self) -> Value {
        let o = self.app.open.as_mut().unwrap();
        o.book
            .read(|q| core::links(q, o.owner, self.c))
            .unwrap()
            .pop()
            .unwrap()
    }
    fn payment(&mut self) -> Value {
        let o = self.app.open.as_mut().unwrap();
        o.book
            .read(|q| core::payments(q, o.owner, self.c))
            .unwrap()
            .pop()
            .unwrap()
    }
    fn count(&mut self) -> i64 {
        self.app
            .open
            .as_mut()
            .unwrap()
            .book
            .read(|q| q.query_row("SELECT count(*) FROM sales_receipt", [], |r| r.get(0)))
            .unwrap()
    }
    fn review(&mut self) -> String {
        let p = self.payment();
        let result = self
            .submit(
                self.page(),
                &[
                    ("action", "stripe_review"),
                    ("id", p["id"].as_str().unwrap()),
                    ("date", "2026-10-02"),
                ],
            )
            .unwrap();
        let Outcome::Redirect { location } = result else {
            panic!()
        };
        location
    }
}
#[test]
fn signed_in_owner_can_save_a_first_credential_after_deferred_key_setup() {
    let mut f = F::new();
    let key = f.app.device_key.take();
    f.app.connection_keys = fb_app::ConnectionKeys::Deferred;
    f.app.os_store_usable = false;
    f.app.connection_keys_changed().unwrap();
    let path = f.settings();
    let page = screens::render(&mut f.app, &path).unwrap().html;
    assert!(page.contains("when you connect"));
    assert!(page.contains("name=\"stripe_key\"") && page.contains("name=\"gmail_client_id\""));
    assert!(!f.app.has_credentials().unwrap());
    f.app.device_key = key;
    f.app.connection_keys = fb_app::ConnectionKeys::Ready;
    f.app.os_store_usable = true;
    f.app.connection_keys_changed().unwrap();
    f.connect();
    f.app.sign_out();
    assert!(f.app.has_credentials().unwrap(), "startup checks presence before sign-in");
}

#[test]
fn connect_create_refresh_review_post_and_print_use_scoped_sealed_key() {
    let mut f = F::new();
    f.connect();
    let path = f.settings();
    let settings = screens::render(&mut f.app, &path).unwrap().html;
    assert!(!settings.contains(KEY));
    assert!(settings.contains("key ending 1234"));
    f.create();
    let print = screens::render(
        &mut f.app,
        &format!("/c/{}/invoices/{}/print/", f.c, f.invoice),
    )
    .unwrap()
    .html;
    assert!(print.contains("https://buy.stripe.com/fixture"));
    assert!(print.contains("aria-label=\"Stripe payment link QR code\""));
    assert!(!print.contains("QR code<br>after link setup"));
    f.log.lock().unwrap().paid = true;
    fb_app::invoice_stripe::desktop_tick(&mut f.app, &f.c.to_db()).unwrap();
    assert_eq!(f.count(), 0);
    assert_eq!(f.payment()["provider_state"], "READY");
    let review = f.review();
    let html = screens::render(&mut f.app, &review).unwrap().html;
    assert!(html.contains("Post reviewed payment"));
    assert!(html.contains("10.00"));
    assert!(html.contains("Synthetic buyer"));
    assert!(!html.contains(KEY));
    if let Ok(dir) = std::env::var("FB_STRIPE_EVIDENCE") {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(format!("{dir}/review.html"), &html).unwrap();
        std::fs::write(format!("{dir}/settings.html"), settings).unwrap();
        std::fs::write(format!("{dir}/invoice.html"), print).unwrap();
    }
    let token = review.split("?review=").nth(1).unwrap();
    f.submit(
        f.page(),
        &[("action", "stripe_post"), ("review_token", token)],
    )
    .unwrap();
    assert_eq!(f.count(), 1);
    assert!(
        f.submit(
            f.page(),
            &[("action", "stripe_post"), ("review_token", token)]
        )
        .is_err()
    );
    let ops = f.log.lock().unwrap().ops.clone();
    let close = ops.iter().position(|o| o == "stripe_deactivate").unwrap();
    assert_eq!(ops[close + 1], "stripe_refresh");
    assert_eq!(ops.last().unwrap(), "stripe_refresh");
    assert!(
        f.app
            .open
            .as_ref()
            .unwrap()
            .book
            .check()
            .unwrap()
            .is_empty()
    );
}
#[test]
fn close_catches_previously_unseen_completed_payment_before_unlock() {
    let mut f = F::new();
    f.connect();
    f.create();
    f.log.lock().unwrap().paid = true;
    let link = f.link();
    f.submit(
        f.page(),
        &[
            ("action", "stripe_deactivate"),
            ("id", link["id"].as_str().unwrap()),
        ],
    )
    .unwrap();
    assert_eq!(f.payment()["provider_state"], "READY");
    assert_eq!(f.link()["state"], "INACTIVE");
    assert_eq!(f.count(), 0);
}
#[test]
fn refund_between_review_and_post_invalidates_review_without_journal() {
    let mut f = F::new();
    f.connect();
    f.create();
    f.log.lock().unwrap().paid = true;
    fb_app::invoice_stripe::desktop_tick(&mut f.app, &f.c.to_db()).unwrap();
    let review = f.review();
    f.log.lock().unwrap().refund = true;
    assert!(
        f.submit(
            f.page(),
            &[
                ("action", "stripe_post"),
                ("review_token", review.split("?review=").nth(1).unwrap())
            ]
        )
        .is_err()
    );
    assert_eq!(f.count(), 0);
    assert_eq!(f.payment()["provider_state"], "REFUNDED");
}
#[test]
fn ambiguous_creation_recovers_without_second_post() {
    let mut f = F::new();
    f.connect();
    f.log.lock().unwrap().unknown = true;
    f.create();
    assert_eq!(f.link()["state"], "UNKNOWN");
    f.create();
    assert_eq!(f.link()["state"], "READY");
    assert_eq!(
        f.log
            .lock()
            .unwrap()
            .ops
            .iter()
            .filter(|op| op.as_str() == "stripe_create")
            .count(),
        1
    );
}
#[test]
fn stale_settings_and_sign_out_refuse_before_provider_calls() {
    let mut f = F::new();
    f.connect();
    let n = f.log.lock().unwrap().ops.len();
    assert!(
        f.submit(
            f.settings(),
            &[
                ("action", "stripe_connect"),
                ("stripe_mode", "LIVE"),
                ("stripe_revision", "0"),
                ("stripe_key", KEY)
            ]
        )
        .is_err()
    );
    assert_eq!(f.log.lock().unwrap().ops.len(), n);
    f.create();
    f.log.lock().unwrap().paid = true;
    fb_app::invoice_stripe::desktop_tick(&mut f.app, &f.c.to_db()).unwrap();
    let review = f.review();
    f.app.sign_out();
    assert!(
        f.submit(
            f.page(),
            &[
                ("action", "stripe_post"),
                ("review_token", review.split("?review=").nth(1).unwrap())
            ]
        )
        .is_err()
    );
    assert_eq!(f.count(), 0);
}
#[test]
fn offline_close_keeps_balance_guard_and_refresh_never_posts() {
    let mut f = F::new();
    f.connect();
    f.create();
    f.log.lock().unwrap().offline = true;
    let link = f.link();
    assert!(
        f.submit(
            f.page(),
            &[
                ("action", "stripe_deactivate"),
                ("id", link["id"].as_str().unwrap())
            ]
        )
        .is_err()
    );
    assert_eq!(f.link()["state"], "READY");
    fb_app::invoice_stripe::desktop_tick(&mut f.app, &f.c.to_db()).unwrap();
    assert_eq!(f.count(), 0);
}
#[test]
fn unavailable_helper_refuses_before_creating_a_durable_request() {
    let mut f = F::new();
    f.connect();
    f.app.net = None;
    let inv = f.invoice.clone();
    assert!(
        f.submit(
            f.invoice_path(),
            &[("action", "stripe_create"), ("id", &inv)]
        )
        .is_err()
    );
    let o = f.app.open.as_mut().unwrap();
    assert!(
        o.book
            .read(|q| core::links(q, o.owner, f.c))
            .unwrap()
            .is_empty()
    );
}
#[test]
fn provider_reactivation_restores_local_balance_guards() {
    let mut f = F::new();
    f.connect();
    f.create();
    let id = f.link()["id"].as_str().unwrap().to_owned();
    f.submit(f.page(), &[("action", "stripe_deactivate"), ("id", &id)])
        .unwrap();
    assert_eq!(f.link()["state"], "INACTIVE");
    f.log.lock().unwrap().closed = false;
    assert!(
        f.submit(f.page(), &[("action", "stripe_refresh"), ("id", &id)])
            .is_err()
    );
    assert_eq!(f.link()["state"], "REACTIVATED");
}

#[test]
fn restored_snapshot_cannot_create_a_second_remote_invoice_request() {
    let mut f = F::new();
    f.connect();
    f.log.lock().unwrap().foreign_request = true;
    let inv = f.invoice.clone();
    assert!(
        f.submit(
            f.invoice_path(),
            &[("action", "stripe_create"), ("id", &inv)]
        )
        .is_err()
    );
    assert_eq!(f.link()["state"], "REFUSED");
    assert!(
        !f.log
            .lock()
            .unwrap()
            .ops
            .iter()
            .any(|op| op == "stripe_create")
    );
}

#[test]
fn new_link_scan_persists_reactivated_known_link_before_refusing_creation() {
    let mut f = F::new();
    f.connect();
    f.create();
    let id = f.link()["id"].as_str().unwrap().to_owned();
    f.submit(f.page(), &[("action", "stripe_deactivate"), ("id", &id)])
        .unwrap();
    {
        let mut log = f.log.lock().unwrap();
        log.closed = false;
        log.reactivated_request = id.clone();
    }
    let inv = f.invoice.clone();
    assert!(
        f.submit(
            f.invoice_path(),
            &[("action", "stripe_create"), ("id", &inv)]
        )
        .is_err()
    );
    let o = f.app.open.as_mut().unwrap();
    let rows = o.book.read(|q| core::links(q, o.owner, f.c)).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows.iter().find(|v| v["id"] == id).unwrap()["state"],
        "REACTIVATED"
    );
    assert_eq!(
        rows.iter().find(|v| v["id"] != id).unwrap()["state"],
        "REFUSED"
    );
    assert!(
        f.submit(
            f.invoice_path(),
            &[("action", "stripe_create"), ("id", &inv)]
        )
        .is_err()
    );
    assert_eq!(
        f.log
            .lock()
            .unwrap()
            .ops
            .iter()
            .filter(|op| op.as_str() == "stripe_create")
            .count(),
        1
    );
    assert_eq!(f.count(), 0);
}

#[test]
fn provider_notices_survive_redirect_once_on_the_action_screen() {
    let mut f = F::new();
    f.connect();
    let settings = f.settings();
    let connected = "Stripe is connected for this company and environment.";
    let html = screens::render(&mut f.app, &settings).unwrap().html;
    // Worker item 30: marked to be brought into view after the page reloads in place.
    assert!(html.contains(&format!("<h1>Edit company</h1><p role=\"status\" data-reveal>{connected}</p>")));
    assert!(!screens::render(&mut f.app, &settings).unwrap().html.contains(connected));

    let page = f.page();
    f.submit(page.clone(), &[("action", "stripe_refresh_company")]).unwrap();
    let refreshed = "Refreshed 0 payment links. Payments still require review.";
    assert!(screens::render(&mut f.app, &page).unwrap().html.contains(refreshed));
    assert!(!screens::render(&mut f.app, &page).unwrap().html.contains(refreshed));
    assert_eq!(f.count(), 0);

    f.log.lock().unwrap().unknown = true;
    f.create();
    let invoice = f.invoice_path();
    let uncertain = "The payment link was not confirmed. Recover its status before creating another request.";
    assert!(screens::render(&mut f.app, &invoice).unwrap().html.contains(uncertain));
    assert!(!screens::render(&mut f.app, &invoice).unwrap().html.contains(uncertain));

    // Provider feedback is text, including on settings shared with Google OAuth.
    f.app.messages.push("<script>synthetic notice</script>".into());
    let html = screens::render(&mut f.app, &settings).unwrap().html;
    assert!(!html.contains("<script>synthetic notice</script>"));
    assert!(html.contains("&lt;script&gt;synthetic notice&lt;&#x2f;script&gt;") || html.contains("&lt;script&gt;synthetic notice&lt;/script&gt;"));
}
