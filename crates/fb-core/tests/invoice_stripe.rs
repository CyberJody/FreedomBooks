mod common;
use common::*;
use fb_core::{
    company_documents,
    directory::{self, Model},
    documents, invoice_preferences, invoice_stripe as stripe, sales,
};
use serde_json::{Value, json};
struct F {
    fx: Fx,
    c: fb_core::CompanyId,
    invoice: Value,
    link: Value,
}
impl F {
    fn new(mode: &str) -> Self {
        let mut fx = Fx::new("stripe");
        let c = fx.company("Synthetic Stripe seller");
        let ar = fx.account(c, "110", "Receivables", "ASSET");
        let uf = fx.account(c, "120", "Undeposited Funds", "ASSET");
        let income = fx.account(c, "400", "Services", "INCOME");
        documents::save_setup(
            &mut fx.book,
            fx.user,
            c,
            &json!({"receivables":ar.to_db(),"undeposited":uf.to_db()}),
            Some(0),
        )
        .unwrap();
        company_documents::save_with_preferences(
            &mut fx.book,
            fx.user,
            c,
            "Synthetic Stripe seller",
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
                    stripe_mode: mode.into(),
                    ..Default::default()
                },
            }),
        )
        .unwrap();
        let customer = directory::save_record(
            &mut fx.book,
            fx.user,
            c,
            Model::Customer,
            &json!({"display_name":"Sample buyer","email":"buyer@example.test"}),
            None,
        )
        .unwrap();
        let draft=sales::save_invoice(&mut fx.book,fx.user,c,&json!({"customer_id":customer["id"],"date":"2026-10-01","due_date":"2026-10-15","request_key":uuid::Uuid::new_v4().to_string(),"tax_mode":"MANUAL","tax_rate":"0","tax_label":"Synthetic","lines":[{"description":"Work","quantity":"1","rate":"10.00","taxable":false,"income_account_id":income.to_db()}]})).unwrap();
        let id = draft["id"].as_str().unwrap();
        let review = sales::review_invoice(&mut fx.book, fx.user, c, id, Some(1)).unwrap();
        let invoice =
            sales::post_invoice(&mut fx.book, fx.user, c, id, Some(1), Some(&review)).unwrap();
        stripe::connect(&mut fx.book, fx.user, c, mode, 0, "acct_synthetic", "1234").unwrap();
        let link = stripe::prepare_link(&mut fx.book, fx.user, c, id).unwrap();
        Self {
            fx,
            c,
            invoice,
            link,
        }
    }
    fn id(&self) -> String {
        self.link["id"].as_str().unwrap().into()
    }
    fn update(&mut self) {
        self.link = stripe::link(self.fx.book.connection(), self.c, &self.id()).unwrap();
    }
    fn created(&mut self) {
        self.link = stripe::finish_link(
            &mut self.fx.book,
            self.fx.user,
            self.c,
            self.link["id"].as_str().unwrap(),
            self.link["revision"].as_i64().unwrap(),
            stripe::LinkResult::Created(stripe::RemoteLink {
                id: "plink_synthetic".into(),
                url: "https://buy.stripe.com/synthetic".into(),
                active: true,
            }),
        )
        .unwrap();
    }
    fn stage(&mut self, o: &stripe::Observation) {
        let id = self.id();
        stripe::stage_page(
            &mut self.fx.book,
            self.fx.user,
            self.c,
            &id,
            self.link["revision"].as_i64().unwrap(),
            std::slice::from_ref(o),
            "",
        )
        .unwrap();
        self.update();
    }
    fn close(&mut self) {
        let id = self.id();
        stripe::mark_deactivated(
            &mut self.fx.book,
            self.fx.user,
            self.c,
            &id,
            self.link["revision"].as_i64().unwrap(),
        )
        .unwrap();
        self.update();
    }
    fn payment(&self) -> Value {
        stripe::payments(self.fx.book.connection(), self.fx.user, self.c)
            .unwrap()
            .pop()
            .unwrap()
    }
    fn review(&mut self) -> fb_core::FbResult<stripe::PaymentReview> {
        let id = self.payment()["id"].as_str().unwrap().to_owned();
        stripe::review_payment(&mut self.fx.book, self.fx.user, self.c, &id, "2026-10-02")
    }
}
fn ready() -> stripe::Observation {
    stripe::Observation {
        session: "cs_synthetic".into(),
        intent: "pi_synthetic".into(),
        amount: 1000,
        received: 1000,
        refunded: 0,
        disputed: false,
        currency: "USD".into(),
        status: "READY".into(),
        created: 1790899200,
    }
}
#[test]
fn live_payment_requires_review_posts_once_and_survives_reopen() {
    let mut f = F::new("LIVE");
    f.created();
    let before = fb_db::test_support::snapshot(f.fx.book.connection()).unwrap();
    f.stage(&ready());
    assert!(f.review().is_err());
    assert_eq!(f.fx.count("SELECT count(*) FROM sales_receipt"), 0);
    let after = fb_db::test_support::snapshot(f.fx.book.connection()).unwrap();
    for (table, rows) in &before {
        if ![
            "invoice_stripe_link",
            "invoice_stripe_payment",
            "ledger_auditevent",
        ]
        .contains(&table.as_str())
        {
            assert_eq!(&after[table], rows, "refresh changed {table}");
        }
    }
    f.close();
    let review = f.review().unwrap();
    let receipt = stripe::post_payment(&mut f.fx.book, f.fx.user, f.c, &review).unwrap();
    assert_eq!(f.payment()["receipt_id"], receipt["id"]);
    assert!(stripe::post_payment(&mut f.fx.book, f.fx.user, f.c, &review).is_err());
    assert_eq!(f.fx.count("SELECT count(*) FROM sales_receipt"), 1);
    assert_eq!(f.fx.audit_count("stripe.payment_posted"), 1);
    assert_eq!(
        sales::invoice_balances(&mut f.fx.book, f.fx.user, f.c).unwrap()[0]["outstanding"],
        "0.00"
    );
    assert!(f.fx.book.check().unwrap().is_empty());
    let backup = book_path("stripe-reopen");
    f.fx.book.snapshot(&backup, &KEY).unwrap();
    let reopened = fb_core::Book::open(&backup, &KEY).unwrap();
    assert_eq!(
        stripe::payments(reopened.connection(), f.fx.user, f.c).unwrap()[0]["receipt_id"],
        receipt["id"]
    );
    let mut refund = ready();
    refund.refunded = 500;
    refund.status = "REFUNDED".into();
    f.stage(&refund);
    assert_eq!(f.payment()["receipt_id"], receipt["id"]);
    assert_eq!(f.fx.count("SELECT count(*) FROM sales_receipt"), 1);
    assert!(f.fx.book.check().unwrap().is_empty());
}
#[test]
fn test_mode_never_posts_or_leaks_into_customer_documents() {
    let mut f = F::new("TEST");
    f.created();
    assert!(
        stripe::payment_url(
            f.fx.book.connection(),
            f.fx.user,
            f.c,
            f.invoice["id"].as_str().unwrap()
        )
        .unwrap()
        .is_none()
    );
    f.stage(&ready());
    f.close();
    assert!(f.review().is_err());
    assert_eq!(f.fx.count("SELECT count(*) FROM sales_receipt"), 0);
}
#[test]
fn delayed_payment_and_changed_evidence_invalidate_review_atomically() {
    let mut f = F::new("LIVE");
    f.created();
    let mut o = ready();
    o.status = "PENDING".into();
    o.received = 0;
    f.stage(&o);
    f.close();
    assert!(f.review().is_err());
    f.stage(&ready());
    let review = f.review().unwrap();
    o = ready();
    o.disputed = true;
    o.status = "DISPUTED".into();
    f.stage(&o);
    let before = fb_db::test_support::snapshot(f.fx.book.connection()).unwrap();
    assert!(stripe::post_payment(&mut f.fx.book, f.fx.user, f.c, &review).is_err());
    assert_eq!(
        fb_db::test_support::snapshot(f.fx.book.connection()).unwrap(),
        before
    );
}
#[test]
fn unknown_creation_reuses_request_and_all_scopes_reject_stale_access() {
    let mut f = F::new("LIVE");
    let id = f.id();
    f.link = stripe::finish_link(
        &mut f.fx.book,
        f.fx.user,
        f.c,
        &id,
        1,
        stripe::LinkResult::Unknown,
    )
    .unwrap();
    let again = stripe::prepare_link(
        &mut f.fx.book,
        f.fx.user,
        f.c,
        f.invoice["id"].as_str().unwrap(),
    )
    .unwrap();
    assert_eq!(again["id"], id);
    assert_eq!(again["state"], "UNKNOWN");
    let c2 = f.fx.company("Other seller");
    assert!(stripe::link(f.fx.book.connection(), c2, &id).is_err());
    let stranger = fb_core::users::create_user(&mut f.fx.book, "Stranger", false).unwrap();
    assert!(stripe::links(f.fx.book.connection(), stranger, f.c).is_err());
    assert!(
        stripe::finish_link(
            &mut f.fx.book,
            f.fx.user,
            f.c,
            &id,
            1,
            stripe::LinkResult::Refused
        )
        .is_err()
    );
    f.created();
    assert!(
        stripe::payment_url(
            f.fx.book.connection(),
            f.fx.user,
            f.c,
            f.invoice["id"].as_str().unwrap()
        )
        .unwrap()
        .is_some()
    );
}
#[test]
fn duplicate_intent_rolls_back_whole_page_and_cursor() {
    let mut f = F::new("LIVE");
    f.created();
    let o = ready();
    let mut duplicate = o.clone();
    duplicate.session = "cs_second".into();
    let before = fb_db::test_support::snapshot(f.fx.book.connection()).unwrap();
    let id = f.id();
    assert!(
        stripe::stage_page(
            &mut f.fx.book,
            f.fx.user,
            f.c,
            &id,
            2,
            &[o, duplicate],
            "cs_next"
        )
        .is_err()
    );
    assert_eq!(
        fb_db::test_support::snapshot(f.fx.book.connection()).unwrap(),
        before
    );
}
#[test]
fn payable_link_guards_invoice_balance_and_receipt_request_key() {
    let mut f = F::new("LIVE");
    f.created();
    let input = json!({"request_key":uuid::Uuid::new_v4().to_string(),"customer_id":f.invoice["current"]["customer_id"],"date":"2026-10-02","amount":"10.00","method":"CASH","reference":"","memo":"","destination":"UNDEPOSITED","bank_id":null,"allocations":[{"invoice_id":f.invoice["id"],"amount":"10.00"}]});
    let review = sales::review_receipt(&mut f.fx.book, f.fx.user, f.c, &input).unwrap();
    let before = fb_db::test_support::snapshot(f.fx.book.connection()).unwrap();
    assert!(sales::record_receipt(&mut f.fx.book, f.fx.user, f.c, &input, Some(&review)).is_err());
    assert_eq!(
        fb_db::test_support::snapshot(f.fx.book.connection()).unwrap(),
        before
    );
    f.stage(&ready());
    f.close();
    let mut hijack = input;
    hijack["request_key"] = f.payment()["id"].clone();
    let review = sales::review_receipt(&mut f.fx.book, f.fx.user, f.c, &hijack).unwrap();
    assert!(sales::record_receipt(&mut f.fx.book, f.fx.user, f.c, &hijack, Some(&review)).is_err());
    assert_eq!(f.fx.count("SELECT count(*) FROM sales_receipt"), 0);
}
#[test]
fn old_payment_cannot_be_freshened_by_unrelated_page_or_link_cleanup() {
    let mut f = F::new("LIVE");
    f.created();
    f.stage(&ready());
    let pid = f.payment()["id"].as_str().unwrap().to_owned();
    f.fx.book.connection().execute("UPDATE invoice_stripe_payment SET observed_at='2026-01-01T00:00:00.000000Z',revision=revision+1 WHERE id=?1",[pid]).unwrap();
    f.close();
    assert!(f.review().is_err());
    let id = f.id();
    stripe::stage_page(
        &mut f.fx.book,
        f.fx.user,
        f.c,
        &id,
        f.link["revision"].as_i64().unwrap(),
        &[],
        "",
    )
    .unwrap();
    f.update();
    assert!(f.review().is_err());
    f.stage(&ready());
    let review = f.review().unwrap();
    f.stage(&ready());
    stripe::post_payment(&mut f.fx.book, f.fx.user, f.c, &review).unwrap();
}
#[test]
fn test_money_does_not_block_real_receipts() {
    let mut f = F::new("TEST");
    f.created();
    f.stage(&ready());
    f.close();
    let input = json!({"request_key":uuid::Uuid::new_v4().to_string(),"customer_id":f.invoice["current"]["customer_id"],"date":"2026-10-02","amount":"10.00","method":"CASH","reference":"","memo":"","destination":"UNDEPOSITED","bank_id":null,"allocations":[{"invoice_id":f.invoice["id"],"amount":"10.00"}]});
    let review = sales::review_receipt(&mut f.fx.book, f.fx.user, f.c, &input).unwrap();
    sales::record_receipt(&mut f.fx.book, f.fx.user, f.c, &input, Some(&review)).unwrap();
    assert!(f.fx.book.check().unwrap().is_empty());
}
#[test]
fn reactivated_old_link_stays_blocking_when_replacement_is_closed() {
    let mut f = F::new("LIVE");
    f.created();
    f.close();
    let old = f.link.clone();
    f.link = stripe::prepare_link(
        &mut f.fx.book,
        f.fx.user,
        f.c,
        f.invoice["id"].as_str().unwrap(),
    )
    .unwrap();
    f.link = stripe::finish_link(
        &mut f.fx.book,
        f.fx.user,
        f.c,
        f.link["id"].as_str().unwrap(),
        1,
        stripe::LinkResult::Created(stripe::RemoteLink {
            id: "plink_replacement".into(),
            url: "https://buy.stripe.com/replacement".into(),
            active: true,
        }),
    )
    .unwrap();
    stripe::mark_reactivated(
        &mut f.fx.book,
        f.fx.user,
        f.c,
        old["id"].as_str().unwrap(),
        old["revision"].as_i64().unwrap(),
    )
    .unwrap();
    f.close();
    assert_eq!(
        stripe::link(f.fx.book.connection(), f.c, old["id"].as_str().unwrap()).unwrap()["state"],
        "REACTIVATED"
    );
    assert!(
        stripe::prepare_link(
            &mut f.fx.book,
            f.fx.user,
            f.c,
            f.invoice["id"].as_str().unwrap()
        )
        .is_err()
    );
    assert!(f.fx.book.check().unwrap().is_empty());
}

/// Populated identity matrix for the 0190 Stripe tables (registered in tests/identity_permanence.rs):
/// no primary key of a connection, payment link or payment can move, by UPDATE or UPDATE OR REPLACE.
#[test]
fn stripe_connection_link_and_payment_identities_are_permanent() {
    let mut f = F::new("LIVE");
    f.created();
    f.stage(&ready());
    f.close();
    let review = f.review().unwrap();
    stripe::post_payment(&mut f.fx.book, f.fx.user, f.c, &review).unwrap();
    let tables = ["invoice_stripe_connection", "invoice_stripe_link", "invoice_stripe_payment"];
    let accepted = fb_db::test_support::primary_key_changes_accepted(f.fx.book.connection(), &tables);
    assert!(accepted.is_empty(), "accepted primary-key changes: {accepted:#?}");
    assert!(f.fx.book.check().unwrap().is_empty());
}
