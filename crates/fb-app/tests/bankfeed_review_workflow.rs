//! Focused review boundary checks. Optional HTML artifacts are rendered from isolated synthetic data.
use fb_app::{App, Command, Outcome, keys::MasterKey, screens};
use fb_core::{CompanyId, bankfeeds as bf, services};
use serde_json::json;
use std::collections::HashMap;
mod support;

struct Fixture {
    _dir: tempfile::TempDir,
    app: App,
    company: CompanyId,
    path: String,
    expense: String,
    bank: String,
    card: String,
    items: HashMap<String, bf::Item>,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new();
        app.simplefin = true;
        let company = app
            .create_book(
                &dir.path().join("synthetic.fbbook"),
                &MasterKey::new([68; 32]),
                "Review actor",
                "Synthetic review",
            )
            .unwrap()
            .company_id;
        support::sign_in(&mut app);
        let o = app.open_mut().unwrap();
        let expense = services::save_account(
            &mut o.book,
            o.owner,
            company,
            services::AccountInput::new("500", "Office supplies").detail("EXPENSE"),
        )
        .unwrap()
        .id
        .to_db();
        let conn = bf::connect(
            &mut o.book,
            o.owner,
            company,
            &json!("Synthetic downloads"),
            "bridge.simplefin.org",
            &bf::SecretSet::none(),
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let tx = |id: &str, amount, pending| bf::StagedTransaction {
            remote_id: id.into(),
            posted_on: Some("2026-11-06".into()),
            transacted_on: None,
            amount,
            description: id.into(),
            pending,
        };
        let accounts = [
            (
                "Checking",
                vec![
                    tx("OFFICE debit", -1200, false),
                    tx("OFFICE refund", 700, false),
                    tx("OFFICE pending", -300, true),
                    tx("OTHER transfer", -1500, false),
                ],
            ),
            (
                "Card",
                vec![
                    tx("OFFICE card charge", -2300, false),
                    tx("OFFICE card refund", 500, false),
                ],
            ),
            ("Unmapped", vec![tx("OFFICE unmapped", -400, false)]),
        ]
        .into_iter()
        .map(|(name, transactions)| bf::StagedAccount {
            conn_key: "synthetic".into(),
            remote_id: name.into(),
            name: name.into(),
            org_name: "Synthetic bank".into(),
            currency: "USD".into(),
            balance: None,
            available: None,
            balance_epoch: None,
            skipped: vec![],
            transactions,
        })
        .collect();
        bf::store_pull(
            &mut o.book,
            o.owner,
            company,
            &conn,
            &bf::StagedPull {
                accounts,
                ..Default::default()
            },
            "2026-11-07T12:00:00Z",
            false,
            None,
            &bf::SecretSet::none(),
        )
        .unwrap();
        let feeds: Vec<(String, String)> = o
            .book
            .read(|q| {
                let mut s = q.prepare("SELECT id,name FROM bankfeeds_account ORDER BY name")?;
                s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<Result<_, _>>()
            })
            .unwrap();
        let mappings: Vec<_> = feeds
            .iter()
            .filter(|(_, n)| n != "Unmapped")
            .map(|(id, n)| {
                (
                    id.clone(),
                    bf::MappingTarget::New {
                        name: format!("Ledger {n}"),
                        kind: if n == "Card" {
                            bf::NewAccountKind::CreditCard
                        } else {
                            bf::NewAccountKind::Checking
                        },
                    },
                )
            })
            .collect();
        bf::save_mappings(
            &mut o.book,
            o.owner,
            company,
            &conn,
            &mappings,
            &bf::SecretSet::none(),
        )
        .unwrap();
        let bank = o
            .book
            .read(|q| {
                q.query_row(
                    "SELECT ledger_account_id FROM bankfeeds_account WHERE name='Checking'",
                    [],
                    |r| r.get(0),
                )
            })
            .unwrap();
        let card = o
            .book
            .read(|q| {
                q.query_row(
                    "SELECT ledger_account_id FROM bankfeeds_account WHERE name='Card'",
                    [],
                    |r| r.get(0),
                )
            })
            .unwrap();
        let items = o
            .book
            .read(|q| {
                let mut s = q.prepare("SELECT id FROM bankfeeds_transaction")?;
                let ids = s
                    .query_map([], |r| r.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()?;
                ids.into_iter()
                    .map(|id| bf::item(q, company, &id).map(|i| (i.description.clone(), i)))
                    .collect::<Result<HashMap<_, _>, fb_core::FbError>>()
            })
            .unwrap();
        Self {
            _dir: dir,
            app,
            company,
            path: format!("/c/{company}/bank-feeds/review/"),
            expense,
            bank,
            card,
            items,
        }
    }
    fn post(&mut self, mut fields: Vec<(String, String)>, action: &str) -> (String, u16) {
        fields.push(("action".into(), action.into()));
        let token = self.app.tokens.mint(&self.path);
        match self
            .app
            .run_renderer(Command::SubmitForm {
                path: self.path.clone(),
                token,
                fields,
            })
            .unwrap()
        {
            Outcome::Page { html, status } => (html, status),
            other => panic!("unexpected outcome: {other:?}"),
        }
    }
    fn drafts(&self) -> Vec<(String, String)> {
        self.items
            .values()
            .flat_map(|i| {
                let pk = i.ordinal;
                [
                    (format!("kind_{pk}"), "JOURNAL".into()),
                    (format!("account_{pk}"), self.expense.clone()),
                    (format!("payee_{pk}"), format!("Edited {}", i.description)),
                    (format!("number_{pk}"), String::new()),
                ]
            })
            .collect()
    }
    fn count(&mut self, sql: &str) -> i64 {
        self.app
            .open_mut()
            .unwrap()
            .book
            .read(|q| q.query_row(sql, [], |r| r.get(0)))
            .unwrap()
    }
    fn render(&mut self, query: &str) -> String {
        screens::render(&mut self.app, &format!("{}{query}", self.path))
            .unwrap()
            .html
    }
}
fn artifact(name: &str, html: &str) {
    if let Ok(dir) = std::env::var("FB_REVIEW_ARTIFACTS") {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(std::path::Path::new(&dir).join(name), html).unwrap();
    }
}
fn row(html: &str, pk: i64) -> &str {
    html.split_once(&format!("data-review-row=\"{pk}\""))
        .unwrap()
        .1
        .split_once("</tr>")
        .unwrap()
        .0
}

#[test]
fn partial_failures_keep_all_drafts_selection_and_filters_without_posting_other_rows() {
    let mut f = Fixture::new();
    let good = f.items["OFFICE debit"].ordinal;
    let bad = f.items["OFFICE unmapped"].ordinal;
    let untouched = f.items["OFFICE card charge"].ordinal;
    let mut fields = f.drafts();
    fields.extend([
        ("sel".into(), good.to_string()),
        ("sel".into(), bad.to_string()),
        ("review_q".into(), "OFFICE".into()),
        ("review_status".into(), "ALL".into()),
    ]);
    let (html, status) = f.post(fields, "accept");
    assert_eq!(status, 400);
    assert!(html.contains("recorded as JE"));
    assert!(row(&html, bad).contains("Map "));
    assert!(row(&html, bad).contains(" checked"));
    assert!(row(&html, bad).contains("Edited OFFICE unmapped"));
    assert!(row(&html, untouched).contains("Edited OFFICE card charge"));
    assert!(!html.contains("OTHER transfer"));
    assert!(html.contains("name=\"review_q\" value=\"OFFICE\""));
    assert_eq!(
        f.count("SELECT count(*) FROM ledger_journal WHERE status='POSTED'"),
        1
    );
    assert_eq!(
        f.count("SELECT count(*) FROM bankfeeds_transaction WHERE status='ACCEPTED'"),
        1
    );
    assert_eq!(
        f.count("SELECT count(*) FROM bankfeeds_transaction WHERE status='NEW'"),
        6
    );
    let o = f.app.open_mut().unwrap();
    let actor: String = o
        .book
        .read(|q| {
            q.query_row(
                "SELECT decided_by_id FROM bankfeeds_transaction WHERE status='ACCEPTED'",
                [],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(actor, o.owner.to_db());
    let audits: i64 = o.book.read(|q| q.query_row("SELECT count(*) FROM ledger_auditevent WHERE action='bankfeed.accepted' AND actor_id=?1", [o.owner.to_db()], |r| r.get(0))).unwrap();
    assert_eq!(audits, 1);
    artifact("failed.html", &html);
}

#[test]
fn empty_selection_and_stale_accept_do_not_write_or_reset_drafts() {
    let mut f = Fixture::new();
    let drafts = f.drafts();
    let (html, status) = f.post(drafts.clone(), "accept");
    assert_eq!(status, 400);
    assert!(html.contains("Tick the rows"));
    assert!(html.contains("Edited OFFICE debit"));
    assert_eq!(f.count("SELECT count(*) FROM ledger_journal"), 0);
    let item = f.items["OFFICE card charge"].clone();
    let o = f.app.open_mut().unwrap();
    bf::accept(
        &mut o.book,
        o.owner,
        f.company,
        &item.id,
        &json!({"kind":"JOURNAL", "account_id":f.expense}),
        &bf::SecretSet::none(),
    )
    .unwrap();
    let mut selected = drafts;
    selected.push(("sel".into(), item.ordinal.to_string()));
    let (html, status) = f.post(selected, "accept");
    assert_eq!(status, 400);
    assert!(html.contains("no longer new"));
    assert_eq!(
        f.count("SELECT count(*) FROM ledger_journal WHERE status='POSTED'"),
        1
    );
    assert_eq!(
        f.count("SELECT count(*) FROM ledger_auditevent WHERE action='bankfeed.accepted'"),
        1
    );
}

#[test]
fn mixed_directions_rule_reuse_and_create_from_review_drafts_render_with_core_conditions() {
    let mut f = Fixture::new();
    let o = f.app.open_mut().unwrap();
    let rule = bf::save_rule(&mut o.book, o.owner, f.company, &json!({"name":"Office purchases", "match_text":"OFFICE", "sign":"OUT", "amount_min":"10.00", "amount_max":"25.00", "account_id":f.expense, "payee_name":"Office vendor"}), &bf::SecretSet::none()).unwrap();
    let html = f.render("");
    let debit = &f.items["OFFICE debit"];
    assert!(row(&html, debit.ordinal).contains(&format!("data-matching-rules=\"{}\"", rule.id)));
    assert!(row(&html, debit.ordinal).contains("value=\"CHECK\" selected"));
    assert!(
        row(&html, f.items["OFFICE card charge"].ordinal).contains("value=\"JOURNAL\" selected")
    );
    assert!(row(&html, f.items["OFFICE refund"].ordinal).contains("data-matching-rules=\"\""));
    assert!(row(&html, f.items["OFFICE pending"].ordinal).contains("Still pending"));
    assert!(row(&html, f.items["OFFICE unmapped"].ordinal).contains("Map to an active"));
    artifact("initial.html", &html);
    let rule_path = format!(
        "/c/{}/bank-feeds/rules/new/?from={}&category={}&payee=Reviewed%20payee",
        f.company, debit.ordinal, f.expense
    );
    let rule_html = screens::render(&mut f.app, &rule_path).unwrap().html;
    assert!(rule_html.contains("value=\"Reviewed payee\""));
    assert!(rule_html.contains(&format!("value=\"{}\" selected", f.expense)));
    artifact("rule.html", &rule_html);
    assert_eq!(f.count("SELECT count(*) FROM ledger_journal"), 0);
    if let Ok(dir) = std::env::var("FB_REVIEW_ARTIFACTS") {
        let ids: HashMap<_, _> = f
            .items
            .iter()
            .map(|(k, i)| (k.clone(), i.ordinal))
            .collect();
        std::fs::write(
            std::path::Path::new(&dir).join("fixture.json"),
            json!({"ids": ids, "expense":f.expense, "bank":f.bank, "card":f.card, "rule":rule.id})
                .to_string(),
        )
        .unwrap();
    }
}

#[test]
fn blank_number_and_transfer_target_survive_row_error_and_ignore_is_unchanged() {
    let mut f = Fixture::new();
    let pk = f.items["OTHER transfer"].ordinal;
    let mut fields = f.drafts();
    fields.retain(|(k, _)| k != &format!("kind_{pk}"));
    fields.extend([
        (format!("kind_{pk}"), "TRANSFER".into()),
        (format!("to_{pk}"), f.bank.clone()),
        ("sel".into(), pk.to_string()),
    ]);
    let (html, status) = f.post(fields, "accept");
    assert_eq!(status, 400);
    assert!(row(&html, pk).contains("value=\"TRANSFER\" selected"));
    assert!(row(&html, pk).contains(&format!("name=\"number_{pk}\" value=\"\"")));
    assert!(row(&html, pk).contains(&format!("value=\"{}\" selected>Unavailable bank", f.bank)));
    assert_eq!(
        f.count("SELECT count(*) FROM ledger_journal WHERE status='POSTED'"),
        0
    );
    let (html, status) = f.post(vec![("sel".into(), pk.to_string())], "ignore");
    assert_eq!(status, 200);
    assert!(html.contains("ignored"));
    assert_eq!(
        f.count("SELECT count(*) FROM bankfeeds_transaction WHERE status='IGNORED'"),
        1
    );
}

#[test]
fn duplicate_match_stays_available_and_accepting_it_does_not_post_another_journal() {
    let mut f = Fixture::new();
    let i = f.items["OFFICE debit"].clone();
    let o = f.app.open_mut().unwrap();
    let draft = services::DraftInput::new(
        "2026-11-06",
        "Previously entered payment",
        vec![
            json!({"account":f.bank, "debit":"0.00", "credit":"12.00", "name":"Existing payee"}),
            json!({"account":f.expense, "debit":"12.00", "credit":"0.00"}),
        ],
    );
    let journal = services::save_draft(&mut o.book, o.owner, f.company, draft).unwrap();
    services::post_journal(
        &mut o.book,
        o.owner,
        f.company,
        journal.id,
        Some(journal.revision),
    )
    .unwrap();
    let suggestion = bf::suggest(&mut o.book, o.owner, f.company, &i.id).unwrap();
    assert_eq!(suggestion.kind, "MATCH");
    let line = suggestion.matches[0].line_id.clone();
    let html = f.render("");
    assert!(row(&html, i.ordinal).contains("value=\"MATCH\" selected"));
    assert!(row(&html, i.ordinal).contains(&format!("value=\"{line}\" selected")));
    artifact("matching.html", &html);
    let (html, status) = f.post(
        vec![
            ("sel".into(), i.ordinal.to_string()),
            (format!("kind_{}", i.ordinal), "MATCH".into()),
            (format!("line_{}", i.ordinal), line),
        ],
        "accept",
    );
    assert_eq!(status, 200);
    assert!(html.contains("recorded as JE"));
    assert_eq!(
        f.count("SELECT count(*) FROM ledger_journal WHERE status='POSTED'"),
        1
    );
    assert_eq!(
        f.count("SELECT count(*) FROM bankfeeds_transaction WHERE status='MATCHED'"),
        1
    );
}

#[test]
fn rule_from_accepted_review_uses_reviewed_values_and_is_suggested_without_posting() {
    accepted_rule_reuses_reviewed_values("JOURNAL");
}

#[test]
fn rule_from_accepted_check_uses_document_payee_and_reuses_it() {
    accepted_rule_reuses_reviewed_values("CHECK");
}

fn accepted_rule_reuses_reviewed_values(kind: &str) {
    let mut f = Fixture::new();
    let pk = f.items["OFFICE debit"].ordinal;
    let fields = vec![
        ("sel".into(), pk.to_string()),
        (format!("kind_{pk}"), kind.into()),
        (format!("account_{pk}"), f.expense.clone()),
        (format!("payee_{pk}"), "Reviewed vendor".into()),
    ];
    assert_eq!(f.post(fields, "accept").1, 200);
    let path = format!("/c/{}/bank-feeds/rules/new/", f.company);
    let html = screens::render(&mut f.app, &format!("{path}?from={pk}"))
        .unwrap()
        .html;
    assert!(html.contains("value=\"Reviewed vendor\""));
    assert!(html.contains(&format!("value=\"{}\" selected", f.expense)));
    let token = f.app.tokens.mint(&path);
    let result = f
        .app
        .run_renderer(Command::SubmitForm {
            path,
            token,
            fields: vec![
                ("name".into(), "Reviewed office rule".into()),
                ("match_text".into(), "OFFICE".into()),
                ("sign".into(), "ANY".into()),
                ("account".into(), f.expense.clone()),
                ("payee_name".into(), "Reviewed vendor".into()),
                ("active".into(), "on".into()),
            ],
        })
        .unwrap();
    assert!(matches!(result, Outcome::Redirect { .. }));
    let html = f.render("");
    assert!(row(&html, f.items["OFFICE card charge"].ordinal).contains("Reviewed office rule"));
    assert!(
        row(&html, f.items["OFFICE card charge"].ordinal).contains("value=\"Reviewed vendor\"")
    );
    assert_eq!(f.count("SELECT count(*) FROM bankfeeds_rule"), 1);
    assert_eq!(
        f.count("SELECT count(*) FROM ledger_journal WHERE status='POSTED'"),
        1
    );
}

#[test]
fn foreign_category_is_refused_and_its_draft_is_retained_for_correction() {
    let mut f = Fixture::new();
    let o = f.app.open_mut().unwrap();
    let other = services::create_company(
        &mut o.book,
        o.owner,
        "Other synthetic company",
        "USD",
        false,
    )
    .unwrap()
    .id;
    let account = services::save_account(
        &mut o.book,
        o.owner,
        other,
        services::AccountInput::new("500", "Other expense").detail("EXPENSE"),
    )
    .unwrap()
    .id
    .to_db();
    let pk = f.items["OFFICE card charge"].ordinal;
    let (html, status) = f.post(
        vec![
            ("sel".into(), pk.to_string()),
            (format!("kind_{pk}"), "JOURNAL".into()),
            (format!("account_{pk}"), account.clone()),
            (format!("payee_{pk}"), "Keep reviewed payee".into()),
        ],
        "accept",
    );
    assert_eq!(status, 400);
    assert!(row(&html, pk).contains("Keep reviewed payee"));
    assert!(row(&html, pk).contains(&format!(
        "value=\"{account}\" selected>Unavailable category"
    )));
    assert_eq!(f.count("SELECT count(*) FROM ledger_journal"), 0);
}

#[test]
fn invalid_rule_keeps_reviewed_values_and_does_not_create_a_rule() {
    let mut f = Fixture::new();
    let path = format!("/c/{}/bank-feeds/rules/new/", f.company);
    let token = f.app.tokens.mint(&path);
    let outcome = f
        .app
        .run_renderer(Command::SubmitForm {
            path,
            token,
            fields: vec![
                ("name".into(), "Edited rule name".into()),
                ("match_text".into(), "OFFICE".into()),
                ("sign".into(), "OUT".into()),
                ("account".into(), f.expense.clone()),
                ("amount_min".into(), "25.00".into()),
                ("amount_max".into(), "10.00".into()),
                ("payee_name".into(), "Reviewed payee".into()),
                ("active".into(), "on".into()),
            ],
        })
        .unwrap();
    let Outcome::Invalid { html } = outcome else {
        panic!("invalid rule accepted")
    };
    for expected in [
        "Edited rule name",
        "Reviewed payee",
        "25.00",
        "10.00",
        "minimum amount cannot exceed",
    ] {
        assert!(html.contains(expected), "missing {expected}");
    }
    assert!(html.contains(&format!("value=\"{}\" selected", f.expense)));
    assert_eq!(f.count("SELECT count(*) FROM bankfeeds_rule"), 0);
    assert_eq!(f.count("SELECT count(*) FROM ledger_journal"), 0);
}

#[test]
fn bulk_drafts_preserve_match_receipt_and_transfer_through_acceptance() {
    use fb_core::{directory, documents, sales};
    let mut f = Fixture::new();
    let o = f.app.open_mut().unwrap();
    let mut account = |code, name, kind, detail| {
        services::save_account(
            &mut o.book,
            o.owner,
            f.company,
            services::AccountInput::new(code, name)
                .kind(kind)
                .detail(detail),
        )
        .unwrap()
        .id
        .to_db()
    };
    let savings = account("109", "Synthetic savings", "ASSET", "BANK");
    let undeposited = account(
        "105",
        "Synthetic undeposited",
        "ASSET",
        "OTHER_CURRENT_ASSET",
    );
    let income = account("400", "Synthetic sales", "INCOME", "INCOME");
    let mut setup = o
        .book
        .read(|q| documents::setup_snapshot(q, f.company))
        .unwrap();
    let revision = setup["revision"].as_i64();
    setup["banks"] = json!([f.bank, savings]);
    setup["undeposited"] = json!(undeposited);
    documents::save_setup(&mut o.book, o.owner, f.company, &setup, revision).unwrap();
    let customer = directory::save_record(
        &mut o.book,
        o.owner,
        f.company,
        directory::Model::Customer,
        &json!({"display_name":"Synthetic receipt customer"}),
        None,
    )
    .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let receipt_input = json!({"request_key":fb_ids::new_v7().to_string(),"customer_id":customer,"date":"2026-11-06",
        "destination":"UNDEPOSITED","method":"CASH","tax_mode":"NONE",
        "lines":[{"item_id":null,"taxable":false,"description":"Synthetic sale","quantity":"1","rate":"7.00","income_account_id":income}]});
    let receipt_review =
        sales::review_sales_receipt(&mut o.book, o.owner, f.company, &receipt_input).unwrap();
    let receipt = sales::record_sales_receipt(
        &mut o.book,
        o.owner,
        f.company,
        &receipt_input,
        Some(&receipt_review),
    )
    .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let draft = services::DraftInput::new(
        "2026-11-06",
        "Existing payment",
        vec![
            json!({"account":f.bank,"debit":"0.00","credit":"12.00"}),
            json!({"account":f.expense,"debit":"12.00","credit":"0.00"}),
        ],
    );
    let journal = services::save_draft(&mut o.book, o.owner, f.company, draft).unwrap();
    services::post_journal(
        &mut o.book,
        o.owner,
        f.company,
        journal.id,
        Some(journal.revision),
    )
    .unwrap();
    let match_item = f.items["OFFICE debit"].clone();
    let receipt_item = f.items["OFFICE refund"].clone();
    let transfer_item = f.items["OTHER transfer"].clone();
    let ordinary = f.items["OFFICE card charge"].clone();
    let suggestion = bf::suggest(&mut o.book, o.owner, f.company, &match_item.id).unwrap();
    assert_eq!(suggestion.kind, "MATCH");
    let match_line = suggestion.matches[0].line_id.clone();
    let suggestion = bf::suggest(&mut o.book, o.owner, f.company, &receipt_item.id).unwrap();
    assert_eq!(suggestion.kind, "DEPOSIT");
    assert_eq!(suggestion.receipts[0].receipt_id, receipt);
    let mut fields = vec![
        (format!("kind_{}", match_item.ordinal), "MATCH".into()),
        (format!("line_{}", match_item.ordinal), match_line),
        (format!("kind_{}", receipt_item.ordinal), "DEPOSIT".into()),
        (format!("receipt_{}", receipt_item.ordinal), receipt.clone()),
        (format!("kind_{}", transfer_item.ordinal), "TRANSFER".into()),
        (format!("to_{}", transfer_item.ordinal), savings.clone()),
        (format!("kind_{}", ordinary.ordinal), "JOURNAL".into()),
        (format!("account_{}", ordinary.ordinal), f.expense.clone()),
        (
            format!("payee_{}", ordinary.ordinal),
            "Bulk reviewed payee".into(),
        ),
    ];
    for item in [&match_item, &receipt_item, &transfer_item, &ordinary] {
        fields.push(("sel".into(), item.ordinal.to_string()));
    }
    // The focused UI command uses real renderer output as the actual App submission.
    // Normal Rust-only runs still exercise the same receipt/match/transfer service boundary.
    if let Ok(driver) = std::env::var("FB_REVIEW_BULK_DRIVER") {
        let dir = std::env::var_os("FB_REVIEW_ARTIFACTS")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| f._dir.path().join("ui"))
            .join("bulk-posting");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("review.html"), f.render("")).unwrap();
        std::fs::write(dir.join("input.json"),json!({"expense":f.expense,"savings":savings,
            "match":match_item.ordinal,"receipt":receipt_item.ordinal,"transfer":transfer_item.ordinal,
            "ordinary":ordinary.ordinal}).to_string()).unwrap();
        let status = std::process::Command::new("node")
            .arg(driver)
            .arg(&dir)
            .status()
            .unwrap();
        assert!(status.success(), "renderer bulk draft regression failed");
        fields =
            serde_json::from_str(&std::fs::read_to_string(dir.join("submission.json")).unwrap())
                .unwrap();
    }
    assert_eq!(
        f.count("SELECT count(*) FROM ledger_journal WHERE status='POSTED'"),
        2
    );
    let (html, status) = f.post(fields, "accept");
    assert_eq!(status, 200, "{html}");
    let o = f.app.open_mut().unwrap();
    for (item, kind, status) in [
        (&match_item, "MATCH", "MATCHED"),
        (&receipt_item, "DEPOSIT", "ACCEPTED"),
        (&transfer_item, "TRANSFER", "ACCEPTED"),
        (&ordinary, "JOURNAL", "ACCEPTED"),
    ] {
        let saved = o.book.read(|q| bf::item(q, f.company, &item.id)).unwrap();
        assert_eq!(saved.accepted_as, kind);
        assert_eq!(saved.status, status);
    }
    let deposited: i64 = o
        .book
        .read(|q| {
            q.query_row(
                "SELECT count(*) FROM banking_depositline WHERE receipt_id=?1",
                [&receipt],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(
        deposited, 1,
        "existing receipt must be settled exactly once"
    );
    let transfer = o
        .book
        .read(|q| bf::item(q, f.company, &transfer_item.id))
        .unwrap();
    let moved: i64 = o
        .book
        .read(|q| {
            q.query_row(
                "SELECT debit FROM ledger_journalline WHERE journal_id=?1 AND account_id=?2",
                fb_db::rusqlite::params![transfer.journal_id, savings],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(moved, 1500, "prepared transfer must reach the chosen bank");
    assert_eq!(
        f.count("SELECT count(*) FROM checks_check"),
        0,
        "matching must not create a duplicate check"
    );
    assert_eq!(
        f.count("SELECT count(*) FROM ledger_journal WHERE status='POSTED'"),
        5
    );
}
