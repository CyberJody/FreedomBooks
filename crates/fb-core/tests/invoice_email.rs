mod common;
use common::*;
use fb_core::{
    directory::{self, Model},
    documents, invoice_email as mail, sales,
};
use serde_json::{Value, json};

const HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const PDF: &[u8] = b"%PDF-1.7\nsynthetic accounting fixture";
struct F {
    fx: Fx,
    c: fb_core::CompanyId,
    invoice: Value,
}
impl F {
    fn new() -> Self {
        let mut fx = Fx::new("invoice-email");
        let c = fx.company("Synthetic email seller");
        let ar = fx.account(c, "110", "Receivables", "ASSET");
        let income = fx.account(c, "400", "Services", "INCOME");
        documents::save_setup(
            &mut fx.book,
            fx.user,
            c,
            &json!({"receivables":ar.to_db()}),
            Some(0),
        )
        .unwrap();
        let customer = directory::save_record(
            &mut fx.book,
            fx.user,
            c,
            Model::Customer,
            &json!({"display_name":"Sample customer","email":"customer@example.test"}),
            None,
        )
        .unwrap();
        let invoice=sales::save_invoice(&mut fx.book,fx.user,c,&json!({"customer_id":customer["id"],"date":"2026-10-01","due_date":"2026-10-15","request_key":uuid::Uuid::new_v4().to_string(),"tax_mode":"MANUAL","tax_rate":"0","tax_label":"Synthetic","lines":[{"description":"Work","quantity":"1","rate":"10.00","taxable":false,"income_account_id":income.to_db()}]})).unwrap();
        let id = invoice["id"].as_str().unwrap();
        let review = sales::review_invoice(&mut fx.book, fx.user, c, id, Some(1)).unwrap();
        let invoice =
            sales::post_invoice(&mut fx.book, fx.user, c, id, Some(1), Some(&review)).unwrap();
        mail::begin_connection(
            &mut fx.book,
            fx.user,
            c,
            0,
            "synthetic.apps.googleusercontent.com",
            "sender@example.test",
        )
        .unwrap();
        mail::set_connection_state(
            &mut fx.book,
            fx.user,
            c,
            1,
            mail::ConnectionState::Connected,
        )
        .unwrap();
        Self { fx, c, invoice }
    }
    fn prepare(&mut self, mode: mail::Mode) -> mail::Attempt {
        mail::prepare(
            &mut self.fx.book,
            self.fx.user,
            self.c,
            mail::Preparation {
                invoice: self.invoice["id"].as_str().unwrap(),
                revision: self.invoice["current"]["id"].as_str().unwrap(),
                connection_revision: 2,
                mode,
                to: "customer@example.test",
                subject: "Invoice 100 — sample",
                document_hash: HASH,
                checked_unknown: None,
            },
        )
        .unwrap()
    }
}
#[test]
fn draft_and_send_history_survive_reopen_without_ledger_changes() {
    let mut f = F::new();
    let before = fb_db::test_support::snapshot(f.fx.book.connection()).unwrap();
    for mode in [mail::Mode::Draft, mail::Mode::Send] {
        let p = f.prepare(mode);
        assert!(
            mail::finish(
                &mut f.fx.book,
                f.fx.user,
                f.c,
                &p.id,
                mail::ResultRecord::Accepted {
                    message: "too-early",
                    draft: None
                }
            )
            .is_err()
        );
        let submitting =
            mail::mark_submitting(&mut f.fx.book, f.fx.user, f.c, &p.id, HASH, PDF).unwrap();
        assert_eq!(submitting.state, "SUBMITTING");
        assert_eq!(submitting.pdf_sha256.len(), 64);
        let end = mail::finish(
            &mut f.fx.book,
            f.fx.user,
            f.c,
            &p.id,
            mail::ResultRecord::Accepted {
                message: "provider123",
                draft: (mode == mail::Mode::Draft).then_some("draft123"),
            },
        )
        .unwrap();
        assert_eq!(
            end.state,
            if mode == mail::Mode::Draft {
                "DRAFTED"
            } else {
                "SENT"
            }
        );
        assert!(mail::mark_submitting(&mut f.fx.book, f.fx.user, f.c, &p.id, HASH, PDF).is_err());
        assert!(
            mail::finish(
                &mut f.fx.book,
                f.fx.user,
                f.c,
                &p.id,
                mail::ResultRecord::Unknown
            )
            .is_err()
        );
        assert!(
            attack(
                &f.fx.book,
                &format!("DELETE FROM invoice_email_delivery WHERE id='{}'", p.id)
            )
            .is_some()
        );
    }
    let after = fb_db::test_support::snapshot(f.fx.book.connection()).unwrap();
    for (table, rows) in before {
        if !["invoice_email_delivery", "ledger_auditevent"].contains(&table.as_str()) {
            assert_eq!(after[&table], rows, "email changed {table}");
        }
    }
    assert_eq!(f.fx.audit_count("invoice_email.completed"), 2);
    assert!(f.fx.book.check().unwrap().is_empty());
    let backup = book_path("email-reopen");
    f.fx.book.snapshot(&backup, &KEY).unwrap();
    let reopened = fb_core::Book::open(&backup, &KEY).unwrap();
    assert_eq!(
        mail::history(
            reopened.connection(),
            f.fx.user,
            f.c,
            f.invoice["id"].as_str().unwrap()
        )
        .unwrap()
        .len(),
        2
    );
}
#[test]
fn pending_crash_and_ambiguous_result_cannot_be_resubmitted() {
    let mut f = F::new();
    let p = f.prepare(mail::Mode::Send);
    mail::mark_submitting(&mut f.fx.book, f.fx.user, f.c, &p.id, HASH, PDF).unwrap();
    let backup = book_path("email-crash");
    f.fx.book.snapshot(&backup, &KEY).unwrap();
    let mut restored = fb_core::Book::open(&backup, &KEY).unwrap();
    let a = mail::attempt(restored.connection(), f.fx.user, f.c, &p.id).unwrap();
    assert_eq!(a.state, "SUBMITTING");
    assert!(mail::mark_submitting(&mut restored, f.fx.user, f.c, &p.id, HASH, PDF).is_err());
    assert!(
        mail::prepare(
            &mut restored,
            f.fx.user,
            f.c,
            mail::Preparation {
                invoice: &p.invoice_id,
                revision: &p.invoice_revision_id,
                connection_revision: 2,
                mode: mail::Mode::Send,
                to: &p.to_address,
                subject: &p.subject,
                document_hash: HASH,
                checked_unknown: None
            }
        )
        .is_err()
    );
    mail::finish(
        &mut restored,
        f.fx.user,
        f.c,
        &p.id,
        mail::ResultRecord::Unknown,
    )
    .unwrap();
    assert_eq!(
        mail::attempt(restored.connection(), f.fx.user, f.c, &p.id)
            .unwrap()
            .state,
        "UNKNOWN"
    );
    assert!(mail::mark_submitting(&mut restored, f.fx.user, f.c, &p.id, HASH, PDF).is_err());
}
#[test]
fn stale_connections_documents_and_actors_are_refused_atomically() {
    let mut f = F::new();
    let p = f.prepare(mail::Mode::Draft);
    let before = fb_db::test_support::snapshot(f.fx.book.connection()).unwrap();
    assert!(
        mail::mark_submitting(&mut f.fx.book, f.fx.user, f.c, &p.id, &"b".repeat(64), PDF).is_err()
    );
    assert!(
        mail::mark_submitting(&mut f.fx.book, f.fx.user, f.c, &p.id, HASH, b"not-pdf").is_err()
    );
    assert!(
        mail::begin_connection(
            &mut f.fx.book,
            f.fx.user,
            f.c,
            1,
            "other-client",
            "other@example.test"
        )
        .is_err()
    );
    assert_eq!(
        fb_db::test_support::snapshot(f.fx.book.connection()).unwrap(),
        before
    );
    let other = f.fx.company("Other company");
    assert!(mail::attempt(f.fx.book.connection(), f.fx.user, other, &p.id).is_err());
    let outsider = fb_core::users::create_user(&mut f.fx.book, "Other operator", false).unwrap();
    assert!(mail::attempt(f.fx.book.connection(), outsider, f.c, &p.id).is_err());
    mail::set_connection_state(
        &mut f.fx.book,
        f.fx.user,
        f.c,
        2,
        mail::ConnectionState::Disconnected,
    )
    .unwrap();
    assert!(mail::mark_submitting(&mut f.fx.book, f.fx.user, f.c, &p.id, HASH, PDF).is_err());
    assert!(
        mail::set_connection_state(
            &mut f.fx.book,
            f.fx.user,
            f.c,
            1,
            mail::ConnectionState::Connected
        )
        .is_err()
    );
    mail::finish(
        &mut f.fx.book,
        f.fx.user,
        f.c,
        &p.id,
        mail::ResultRecord::Refused(mail::Failure::Stale),
    )
    .unwrap();
    assert!(f.fx.book.check().unwrap().is_empty());
}
#[test]
fn delivery_corruption_is_refused_and_detected_on_reopen() {
    let mut f = F::new();
    let p = f.prepare(mail::Mode::Draft);
    let q = f.fx.book.connection();
    for sql in [
        "UPDATE invoice_email_delivery SET mode='SEND'",
        "UPDATE invoice_email_delivery SET state='SENT'",
        "UPDATE invoice_email_delivery SET provider_message='unearned'",
        "INSERT OR REPLACE INTO invoice_email_connection SELECT * FROM invoice_email_connection",
    ] {
        assert!(attack(&f.fx.book, sql).is_some(), "{sql}");
    }
    let sql: String = q
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE name='invoice_email_delivery_update'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    q.execute_batch(
        "DROP TRIGGER invoice_email_delivery_update; PRAGMA ignore_check_constraints=ON",
    )
    .unwrap();
    q.execute(
        "UPDATE invoice_email_delivery SET state='SENT' WHERE id=?1",
        [p.id],
    )
    .unwrap();
    q.execute_batch("PRAGMA ignore_check_constraints=OFF")
        .unwrap();
    q.execute_batch(&sql).unwrap();
    assert!(!f.fx.book.check().unwrap().is_empty());
}

#[test]
fn administrator_can_resolve_disabled_operators_abandoned_attempt_without_claiming_acceptance() {
    let mut f = F::new();
    let p = f.prepare(mail::Mode::Send);
    mail::mark_submitting(&mut f.fx.book, f.fx.user, f.c, &p.id, HASH, PDF).unwrap();
    let admin =
        fb_core::users::create_user(&mut f.fx.book, "Recovery administrator", true).unwrap();
    fb_core::users::add_member(&mut f.fx.book, f.c, admin).unwrap();
    f.fx.book
        .connection()
        .execute(
            "INSERT INTO desktop_user(user_id,role) VALUES(?1,'administrator')",
            [admin.to_db()],
        )
        .unwrap();
    let member = fb_core::users::create_user(&mut f.fx.book, "Another bookkeeper", false).unwrap();
    fb_core::users::add_member(&mut f.fx.book, f.c, member).unwrap();
    fb_core::users::set_active(&mut f.fx.book, f.fx.user, false).unwrap();
    assert!(mail::resolve_interrupted(&mut f.fx.book, member, f.c, &p.id).is_err());
    assert!(
        mail::finish(
            &mut f.fx.book,
            admin,
            f.c,
            &p.id,
            mail::ResultRecord::Accepted {
                message: "not-authorized",
                draft: None
            }
        )
        .is_err()
    );
    assert_eq!(
        mail::resolve_interrupted(&mut f.fx.book, admin, f.c, &p.id)
            .unwrap()
            .state,
        "UNKNOWN"
    );
    let detail:String=f.fx.book.connection().query_row("SELECT detail FROM ledger_auditevent WHERE action='invoice_email.interrupted_resolved' AND actor_id=?1",[admin.to_db()],|r|r.get(0)).unwrap();
    assert!(detail.contains(&f.fx.user.to_db()));
    let input = || mail::Preparation {
        invoice: &p.invoice_id,
        revision: &p.invoice_revision_id,
        connection_revision: 2,
        mode: mail::Mode::Draft,
        to: &p.to_address,
        subject: &p.subject,
        document_hash: HASH,
        checked_unknown: None,
    };
    assert!(mail::prepare(&mut f.fx.book, admin, f.c, input()).is_err());
    let mut checked = input();
    checked.checked_unknown = Some(&p.id);
    mail::prepare(&mut f.fx.book, admin, f.c, checked).unwrap();
    assert!(f.fx.book.check().unwrap().is_empty());
}

/// Populated identity matrix for the 0189 email tables (registered in tests/identity_permanence.rs):
/// no primary key of a connection or a delivery attempt can move, by UPDATE or UPDATE OR REPLACE.
#[test]
fn email_connection_and_delivery_identities_are_permanent() {
    let mut f = F::new();
    let p = f.prepare(mail::Mode::Draft);
    mail::mark_submitting(&mut f.fx.book, f.fx.user, f.c, &p.id, HASH, PDF).unwrap();
    let tables = ["invoice_email_connection", "invoice_email_delivery"];
    let accepted = fb_db::test_support::primary_key_changes_accepted(f.fx.book.connection(), &tables);
    assert!(accepted.is_empty(), "accepted primary-key changes: {accepted:#?}");
    assert!(f.fx.book.check().unwrap().is_empty());
}
