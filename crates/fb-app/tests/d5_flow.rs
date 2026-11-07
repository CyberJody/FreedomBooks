//! Credit/deposit forms use the same opaque, exact-intent review capabilities as the core.
use fb_app::{App, Command, Outcome, commands::parse_command, keys, screens};
use serde_json::{Value, json};
fn run(app: &mut App, v: Value) -> Value {
    match app.run(parse_command(&v.to_string()).unwrap()).unwrap() {
        Outcome::Done { data } => data,
        v => panic!("{v:?}"),
    }
}
fn field(html: &str, name: &str) -> String {
    html.split_once(&format!("name=\"{name}\" value=\""))
        .unwrap_or_else(|| panic!("missing {name}: {html}"))
        .1
        .split('"')
        .next()
        .unwrap()
        .into()
}
fn submit(app: &mut App, path: &str, p: &[(String, String)]) -> Outcome {
    let html = screens::render(app, path).unwrap().html;
    app.run(Command::SubmitForm {
        path: path.into(),
        token: field(&html, "fb_command_token"),
        fields: p.to_vec(),
    })
    .unwrap()
}
fn count(app: &mut App, table: &str) -> i64 {
    app.open_mut()
        .unwrap()
        .book
        .read(|tx| tx.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0)))
        .unwrap()
}
fn pairs(v: Value) -> Vec<(String, String)> {
    v.as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap().into()))
        .collect()
}
struct F {
    app: App,
    c: String,
    customer: String,
    income: String,
    bank: String,
    invoice: String,
}
impl F {
    fn new() -> Self {
        let mut app = App::new();
        app.today = || chrono::NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
        let p = std::env::temp_dir().join(format!("fb-d5-{}.fbbook", uuid::Uuid::new_v4()));
        let c = app
            .create_book(
                &p,
                &keys::new_master_key().unwrap(),
                "Owner",
                "Synthetic D5",
            )
            .unwrap()
            .company_id
            .to_db();
        let mut account = |code: &str, kind: &str| {
            run(&mut app,json!({"type":"SaveAccount","company_id":c,"code":code,"name":code,"detail_type":kind}))["id"].as_str().unwrap().to_owned()
        };
        let bank = account("1000", "BANK");
        let ar = account("1100", "RECEIVABLE");
        let uf = account("1200", "OTHER_CURRENT_ASSET");
        let liab = account("2300", "OTHER_CURRENT_LIABILITY");
        let income = account("4000", "INCOME");
        run(
            &mut app,
            json!({"type":"SaveSetup","company_id":c,"expected_revision":0,"receivables_id":ar,"undeposited_id":uf,"customer_deposits_id":liab,"bank_ids":[bank]}),
        );
        let customer = run(
            &mut app,
            json!({"type":"SaveCustomer","company_id":c,"display_name":"Buyer"}),
        )["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let invoice=run(&mut app,json!({"type":"SaveInvoiceDraft","company_id":c,"invoice_id":null,"expected_revision":0,"values":{"request_key":uuid::Uuid::new_v4().to_string(),"customer_id":customer,"date":"2026-10-01","tax_mode":"NONE","lines":[{"description":"Work","quantity":"1","rate":"100.00","income_account_id":income}]}}))["id"].as_str().unwrap().to_owned();
        run(
            &mut app,
            json!({"type":"PostInvoice","company_id":c,"invoice_id":invoice,"expected_revision":1}),
        );
        Self {
            app,
            c,
            customer,
            income,
            bank,
            invoice,
        }
    }
    fn credit_fields(&self) -> Vec<(String, String)> {
        pairs(
            json!({"request_key":format!(" {} ",uuid::Uuid::new_v4()),"customer":self.customer,"date":"2026-10-02","memo":"  refund service  ","tax_mode":"NONE","lines-TOTAL_FORMS":"3","lines-INITIAL_FORMS":"0","lines-0-description":"Return","lines-0-quantity":"2","lines-0-rate":"10.00","lines-0-income_account":self.income,"lines-1-description":" \t\n ","lines-1-quantity":" ","lines-1-rate":"\t","lines-1-DELETE":"false","lines-1-origin_position":"ignored","lines-1-refresh_item":"true","lines-1-keep_archived":"true"}),
        )
    }
    fn reviewed(&mut self, path: &str, p: &[(String, String)], action: &str) -> Outcome {
        let out = submit(&mut self.app, path, p);
        let Outcome::Page { html, status } = out else {
            panic!("{out:?}")
        };
        assert_eq!(status, 200);
        let mut p = p.to_vec();
        p.push(("review_token".into(), field(&html, "review_token")));
        p.push(("action".into(), action.into()));
        submit(&mut self.app, path, &p)
    }
}
fn location(out: Outcome) -> String {
    let Outcome::Redirect { location } = out else {
        panic!("{out:?}")
    };
    location
}
#[test]
fn credit_and_deposit_lifecycles_through_forms_tie_out() {
    let mut f = F::new();
    let c = f.c.clone();
    let p = f.credit_fields();
    let path = format!("/c/{c}/credit-memos/new/");
    let cm = location(f.reviewed(&path, &p, "record"));
    assert_eq!(count(&mut f.app, "credits_creditmemoline"), 1);
    let p = pairs(
        json!({"request_key":uuid::Uuid::new_v4().to_string(),"date":"2026-10-03",format!("amount_{}",f.invoice):"5.00"}),
    );
    location(f.reviewed(&format!("{cm}apply/"), &p, "apply"));
    let html = screens::render(&mut f.app, &cm).unwrap().html;
    assert!(html.contains("15.00"));
    let unapply = html
        .split("href=\"")
        .map(|s| s.split('"').next().unwrap())
        .find(|s| s.contains("credit-applications/"))
        .unwrap()
        .to_owned();
    let p =
        pairs(json!({"expected_revision":"1","date":"2026-10-04","reason":"Synthetic correction"}));
    location(submit(&mut f.app, &unapply, &p));
    let p = pairs(
        json!({"request_key":uuid::Uuid::new_v4().to_string(),"date":"2026-10-04","amount":"2.00","bank":f.bank,"method":"CASH","check_number":"","memo":"Refund"}),
    );
    let refund = location(f.reviewed(&format!("{cm}refund/"), &p, "record"));
    let p = pairs(json!({"expected_revision":"1","date":"2026-10-05","reason":"Correct refund"}));
    location(submit(&mut f.app, &format!("{refund}void/"), &p));
    location(submit(&mut f.app, &format!("{cm}void/"), &p));
    let p = pairs(
        json!({"request_key":uuid::Uuid::new_v4().to_string(),"customer":f.customer,"date":"2026-10-02","amount":"30.00","method":"CHECK","destination":"UNDEPOSITED","reference":"D5","order_reference":"Order 1","memo":"Deposit"}),
    );
    let dp = location(f.reviewed(&format!("/c/{c}/customer-deposits/new/"), &p, "record"));
    let p = pairs(
        json!({"request_key":uuid::Uuid::new_v4().to_string(),"date":"2026-10-03","amount":"10.00","invoice":f.invoice}),
    );
    location(f.reviewed(&format!("{dp}apply/"), &p, "apply"));
    let html = screens::render(&mut f.app, &dp).unwrap().html;
    let unapply = html
        .split("href=\"")
        .map(|s| s.split('"').next().unwrap())
        .find(|s| s.contains("deposit-applications/"))
        .unwrap()
        .to_owned();
    let p = pairs(json!({"expected_revision":"1","date":"2026-10-04","reason":"Release deposit"}));
    location(submit(&mut f.app, &unapply, &p));
    let p = pairs(
        json!({"request_key":uuid::Uuid::new_v4().to_string(),"date":"2026-10-04","amount":"2.00","bank":f.bank,"method":"CASH","check_number":"","memo":"Refund deposit"}),
    );
    let refund = location(f.reviewed(&format!("{dp}refund/"), &p, "record"));
    let p = pairs(
        json!({"expected_revision":"1","date":"2026-10-05","reason":"Correct deposit refund"}),
    );
    location(submit(&mut f.app, &format!("{refund}void/"), &p));
    location(submit(&mut f.app, &format!("{dp}void/"), &p));
    let o = f.app.open_mut().unwrap();
    let c = c.parse().unwrap();
    let invoices = fb_core::sales::invoice_balances(&mut o.book, o.owner, c).unwrap();
    assert_eq!(invoices[0]["outstanding"], "100.00");
    let deps = fb_core::prepayments::deposit_balances(&mut o.book, o.owner, c).unwrap();
    assert_eq!(deps[0]["remaining"], "0.00");
    let credits = fb_core::credits::memo_balances(&mut o.book, o.owner, c).unwrap();
    assert_eq!(credits[0]["available"], "0.00");
}
#[test]
fn stale_review_and_forged_capability_cannot_record_money() {
    let mut f = F::new();
    let path = format!("/c/{}/credit-memos/new/", f.c);
    let mut p = f.credit_fields();
    let out = submit(&mut f.app, &path, &p);
    let Outcome::Page { html, .. } = out else {
        panic!("{out:?}")
    };
    p.push(("review_token".into(), field(&html, "review_token")));
    p.push(("action".into(), "record".into()));
    for (k, v) in &mut p {
        if k == "lines-0-rate" {
            *v = "11.00".into();
        }
    }
    let before = count(&mut f.app, "ledger_auditevent");
    let out = submit(&mut f.app, &path, &p);
    let Outcome::Invalid { html } = out else {
        panic!("{out:?}")
    };
    assert!(html.contains("changed"));
    assert_eq!(count(&mut f.app, "credits_creditmemo"), 0);
    assert_eq!(count(&mut f.app, "ledger_auditevent"), before);
    for (k, v) in &mut p {
        if k == "review_token" {
            *v = "forged".into();
        }
    }
    let out = submit(&mut f.app, &path, &p);
    let Outcome::Invalid { html } = out else {
        panic!("{out:?}")
    };
    assert!(html.contains("fresh credit memo review"));
    assert_eq!(count(&mut f.app, "ledger_auditevent"), before);
}
#[test]
fn field_cleaning_collects_errors_without_writes() {
    let mut f = F::new();
    let path = format!("/c/{}/customer-deposits/new/", f.c);
    let before = count(&mut f.app, "ledger_auditevent");
    let p = pairs(
        json!({"request_key":"bad","date":"bad","customer":format!(" {} ",f.customer),"method":" CHECK ","destination":" UNDEPOSITED ","amount":"1.00"}),
    );
    let Outcome::Invalid { html } = submit(&mut f.app, &path, &p) else {
        panic!()
    };
    assert!(html.contains("Enter a valid UUID."));
    assert!(html.contains("Enter a valid date."));
    assert!(html.contains("Select a valid choice."));
    assert_eq!(count(&mut f.app, "ledger_auditevent"), before);
    let path = format!("/c/{}/credit-memos/new/", f.c);
    let mut p = f.credit_fields();
    for (k, v) in &mut p {
        if k == "lines-TOTAL_FORMS" {
            *v = "0.0".into();
        }
    }
    let Outcome::Invalid { html } = submit(&mut f.app, &path, &p) else {
        panic!()
    };
    assert!(html.contains("Please submit at least 1 form."));
    assert_eq!(count(&mut f.app, "ledger_auditevent"), before);
}

#[test]
fn invoice_side_deposit_application_and_integer_revision_match_django() {
    let mut f = F::new();
    let values = json!({"request_key":uuid::Uuid::new_v4().to_string(),"customer_id":f.customer,"date":"2026-10-02","amount":"25.00","method":"CASH","destination":"UNDEPOSITED","bank_id":null,"estimate_id":null,"reference":"","order_reference":"","memo":"Deposit"});
    let review = run(
        &mut f.app,
        json!({"type":"ReviewCustomerDeposit","company_id":f.c,"values":values}),
    );
    let dep = run(
        &mut f.app,
        json!({"type":"RecordCustomerDeposit","company_id":f.c,"values":values,"review_token":review["token"]}),
    );
    let path = format!("/c/{}/invoices/{}/apply-deposit/", f.c, f.invoice);
    let p = pairs(
        json!({"request_key":uuid::Uuid::new_v4().to_string(),"date":"2026-10-03","amount":"5.00","deposit":dep["id"]}),
    );
    assert_eq!(
        location(f.reviewed(&path, &p, "apply")),
        format!("/c/{}/invoices/{}/", f.c, f.invoice)
    );
    let path = format!(
        "/c/{}/customer-deposits/{}/",
        f.c,
        dep["id"].as_str().unwrap()
    );
    let html = screens::render(&mut f.app, &path).unwrap().html;
    let unapply = html
        .split("href=\"")
        .map(|s| s.split('"').next().unwrap())
        .find(|s| s.contains("deposit-applications/"))
        .unwrap()
        .to_owned();
    let before = count(&mut f.app, "ledger_auditevent");
    let p = pairs(json!({"expected_revision":"0.0","date":"2026-10-04","reason":"Correction"}));
    let Outcome::Invalid { html } = submit(&mut f.app, &unapply, &p) else {
        panic!()
    };
    assert!(html.contains("greater than or equal to 1"));
    assert_eq!(count(&mut f.app, "ledger_auditevent"), before);
    // Django IntegerField accepts a zero fractional suffix, unlike Python int().
    let p = pairs(json!({"expected_revision":"1.0","date":"2026-10-04","reason":"Correction"}));
    assert_eq!(location(submit(&mut f.app, &unapply, &p)), path);
}

#[test]
fn company_scope_applies_to_each_document_and_application_route() {
    let mut f = F::new();
    let p = f.credit_fields();
    let c = f.c.clone();
    let path = format!("/c/{c}/credit-memos/new/");
    let cm = location(f.reviewed(&path, &p, "record"));
    let other = run(
        &mut f.app,
        json!({"type":"SaveCompany","company_id":null,"name":"Other synthetic books","currency":"USD"}),
    );
    let other_id = other["id"].as_str().unwrap();
    let before = count(&mut f.app, "ledger_auditevent");
    for suffix in ["", "apply/", "void/", "refund/"] {
        let path = format!("{cm}{suffix}").replace(&c, other_id);
        assert!(screens::render(&mut f.app, &path).is_err());
        let token = f.app.tokens.mint(&path);
        assert!(
            f.app
                .run(Command::SubmitForm {
                    path,
                    token,
                    fields: p.clone()
                })
                .is_err()
        );
    }
    assert_eq!(count(&mut f.app, "ledger_auditevent"), before);
}


#[test]
fn credit_memo_has_no_print_route_like_the_frozen_reference() {
    let company = "0195b4a4-55af-719e-96cb-a2f946b067bf";
    let memo = "0195b4a4-55af-719e-96cb-a2f946b067c0";
    assert!(screens::resolve(&format!("/c/{company}/credit-memos/{memo}/")).is_some());
    assert!(screens::resolve(&format!("/c/{company}/credit-memos/{memo}/print/")).is_none());
    assert!(screens::resolve(&format!("/c/{company}/invoices/{memo}/print/")).is_some());
}
