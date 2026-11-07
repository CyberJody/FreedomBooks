use super::*;

#[test]
fn fix1_typed_parser_codes() {
    for (value, code) in [
        ("not JSON".to_owned(), "bad_json"),
        ("{}".to_owned(), "schema"),
        (
            {
                let mut v = raw(false);
                v["currency"] = json!("CAD");
                v.to_string()
            },
            "currency",
        ),
        (
            {
                let mut v = raw(false);
                v["total"] = json!("-1");
                v.to_string()
            },
            "negative",
        ),
    ] {
        assert_eq!(
            receipts::parse_reply(&value, "USD", "2026-10-30")
                .unwrap_err()
                .code,
            code
        );
    }
}

#[test]
fn fix1_signin_closed_pairs_preserve_audit_and_message() {
    assert!(SigninFailure::new("provider text", "unauthorized").is_err());
    assert!(SigninFailure::new("ai.call_failed", "provider text").is_err());
    for action in ["ai.call_failed", "ai.refresh_failed"] {
        for code in ["unauthorized", "invalid_grant", "plan_not_granted"] {
            for mode in ["siwc", "api_key"] {
                let mut f = Fx::new();
                connected(
                    &mut f.book,
                    f.actor,
                    f.co,
                    &json!({"mode":mode,"model_choices":[]}),
                )
                .unwrap();
                needs_signin(
                    &mut f.book,
                    f.member,
                    f.co,
                    SigninFailure::new(action, code).unwrap(),
                )
                .unwrap();
                let row = connection_for(&mut f.book, f.member, f.co)
                    .unwrap()
                    .unwrap();
                let expected = if code == "plan_not_granted" {
                    PLAN_SCOPE_LOST
                } else if mode == "api_key" {
                    KEY_REJECTED
                } else {
                    SIGNIN_EXPIRED
                };
                assert_eq!(row["last_error"], expected);
                let saved:String=f.book.connection().query_row("SELECT json_extract(detail,'$.code') FROM ledger_auditevent WHERE action=?", [action], |r|r.get(0)).unwrap();
                assert_eq!(saved, code);
                assert_eq!(row["credential_present"], mode == "siwc");
            }
        }
    }
}

#[test]
fn fix1_member_401_marks_connection() {
    let mut f = Fx::new();
    f.connect();
    let r = f.begin();
    fail_read(
        &mut f.book,
        f.member,
        f.co,
        r["id"].as_str().unwrap(),
        1,
        "unauthorized",
        1001,
    )
    .unwrap();
    needs_signin(&mut f.book, f.member, f.co, SigninFailure::new("ai.call_failed", "unauthorized").unwrap()).unwrap();
    assert_eq!(
        connection_for(&mut f.book, f.member, f.co)
            .unwrap()
            .unwrap()["status"],
        "needs_signin"
    );
}

#[test]
fn fix1_key_rejection_has_key_message() {
    let mut f = Fx::new();
    f.connect();
    needs_signin(&mut f.book, f.actor, f.co, SigninFailure::new("ai.call_failed", "unauthorized").unwrap()).unwrap();
    assert_eq!(
        connection_for(&mut f.book, f.actor, f.co).unwrap().unwrap()["last_error"],
        "OpenAI did not accept this API key. Check it on the OpenAI platform and paste it again."
    );
}

#[test]
fn fix1_disconnected_cannot_need_signin() {
    let mut f = Fx::new();
    set_read_attachments(&mut f.book, f.actor, f.co, true).unwrap();
    let before = f.snapshot();
    assert_eq!(
        needs_signin(&mut f.book, f.actor, f.co, SigninFailure::new("ai.call_failed", "unauthorized").unwrap())
            .unwrap_err()
            .message(),
        "Nothing is connected."
    );
    assert_eq!(f.snapshot(), before);
}

#[test]
fn fix1_fixed_client_failure_finishes_with_failed_audit() {
    for (code, message) in [
        ("unreachable", "OpenAI could not be reached."),
        ("timeout", "OpenAI did not answer in time. Try again later."),
        (
            "response_unreadable",
            "The OpenAI response could not be read.",
        ),
        (
            "response_oversized",
            "The OpenAI response was larger than allowed.",
        ),
        (
            "certificate",
            "The OpenAI server certificate could not be verified.",
        ),
    ] {
        let mut f = Fx::new();
        f.connect();
        let r = f.begin();
        let result = fail_read(
            &mut f.book,
            f.member,
            f.co,
            r["id"].as_str().unwrap(),
            1,
            code,
            1001,
        )
        .unwrap();
        assert_eq!(result["status"], "failed");
        assert_eq!(result["error"], message);
        let audit: String = f.book.connection().query_row("SELECT json_extract(detail,'$.code') FROM ledger_auditevent WHERE action='ai.receipt_failed'", [], |r|r.get(0)).unwrap();
        assert_eq!(audit, "failed");
    }
}

#[test]
fn fix1_draft_refusal_precedes_connection() {
    let mut f = Fx::new();
    let mut file = Fx::file();
    file["bill_id"] = json!(fb_ids::new_v7().to_string());
    assert_eq!(
        begin_read(
            &mut f.book,
            f.member,
            f.co,
            &fb_ids::new_v7().to_string(),
            &file,
            1000
        )
        .unwrap_err()
        .message(),
        "Only an attachment of a Draft bill can be read into that bill."
    );
}

#[test]
fn fix1_email_trims_unicode_characters() {
    let mut f = Fx::new();
    let row = connected(
        &mut f.book,
        f.actor,
        f.co,
        &json!({"mode":"siwc","model_choices":[],"account_email":"é".repeat(260)}),
    )
    .unwrap();
    assert_eq!(row["account_email"], "é".repeat(254));
}

#[test]
fn fix1_connect_preserves_closed_models_error() {
    let mut f = Fx::new();
    let row = connected(
        &mut f.book,
        f.actor,
        f.co,
        &json!({"mode":"siwc","model_choices":[],"models_error":"timeout"}),
    )
    .unwrap();
    assert_eq!(
        row["last_error"],
        "OpenAI did not answer in time. Try again later."
    );
}

#[test]
fn fix1_generation_exhaustion_is_controlled() {
    for needs in [false, true] {
        let mut f = Fx::new();
        f.connect();
        f.book
            .connection()
            .execute("UPDATE assistant_connection SET generation=?", [i64::MAX])
            .unwrap();
        let before = f.snapshot();
        let error = if needs {
            needs_signin(&mut f.book, f.actor, f.co, SigninFailure::new("ai.call_failed", "unauthorized").unwrap()).unwrap_err()
        } else {
            disconnect(&mut f.book, f.actor, f.co, None).unwrap_err()
        };
        assert_eq!(error.kind(), "ValidationError");
        assert_eq!(error.message(), "Assistant generation exhausted.");
        assert_eq!(f.snapshot(), before);
    }
}

#[test]
fn fix1_attempt_exhaustion_is_controlled() {
    let mut f = Fx::new();
    f.connect();
    let file = Fx::file();
    let key = fb_ids::new_v7().to_string();
    begin_read(&mut f.book, f.member, f.co, &key, &file, 1000).unwrap();
    f.book
        .connection()
        .execute(
            "UPDATE assistant_reading SET status='failed',error='expired',attempt=?",
            [i64::MAX],
        )
        .unwrap();
    let before = f.snapshot();
    let error = begin_read(&mut f.book, f.member, f.co, &key, &file, 1601).unwrap_err();
    assert_eq!(error.kind(), "ValidationError");
    assert_eq!(error.message(), "Receipt attempt exhausted.");
    assert_eq!(f.snapshot(), before);
}

#[test]
fn fix1_expired_reading_cannot_finish_in_sql_without_claim() {
    let mut f = Fx::new();
    f.connect();
    let r = f.begin();
    let id = r["id"].as_str().unwrap();
    f.book
        .connection()
        .execute(
            "UPDATE assistant_reading SET status='failed',error='expired' WHERE id=?",
            [id],
        )
        .unwrap();
    let before = f.snapshot();
    assert!(f.book.connection().execute("UPDATE assistant_reading SET status='ok',error='',result=?,finished_at=1001 WHERE id=?", params![parsed(false).to_string(), id]).is_err());
    assert!(f.book.connection().execute("UPDATE assistant_reading SET status='failed',error='finished',finished_at=1001 WHERE id=?", [id]).is_err());
    assert_eq!(f.snapshot(), before);
}
use crate::{services::AccountInput, users};
use std::sync::atomic::{AtomicUsize, Ordering};
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fx {
    book: Book,
    actor: UserId,
    co: CompanyId,
    member: UserId,
    vendor: String,
    expense: crate::AccountId,
    path: std::path::PathBuf,
}
impl Fx {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "fb-assistant-{}-{}.fbbook",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut book = Book::create(&path, &[81; 32]).unwrap();
        let actor = users::create_user(&mut book, "assistant-staff", true).unwrap();
        let member = users::create_user(&mut book, "assistant-member", false).unwrap();
        let co = services::create_company(&mut book, actor, "Synthetic receipts", "USD", false)
            .unwrap()
            .id;
        users::add_member(&mut book, co, member).unwrap();
        let expense = services::save_account(
            &mut book,
            actor,
            co,
            AccountInput::new("500", "Supplies").kind("EXPENSE"),
        )
        .unwrap()
        .id;
        let vendor=directory::save_record(&mut book,actor,co,directory::Model::Vendor,&json!({"display_name":"Northwind Supply","default_terms":"30","default_expense_account":expense.to_db()}),None).unwrap()["id"].as_str().unwrap().to_owned();
        Self {
            book,
            actor,
            member,
            co,
            vendor,
            expense,
            path,
        }
    }
    fn connect(&mut self) {
        connected(&mut self.book,self.actor,self.co,&json!({"mode":"api_key","key_last4":"demo","model_choices":[{"slug":"demo-model","display_name":"Demo"}]})).unwrap();
        set_read_attachments(&mut self.book, self.actor, self.co, true).unwrap();
    }
    fn file() -> Value {
        json!({"source_file_id":fb_ids::new_v7().to_string(),"original_name":"synthetic.png","content_type":"image/png","size":100,"sha256":"a".repeat(64)})
    }
    fn begin(&mut self) -> Value {
        begin_read(
            &mut self.book,
            self.member,
            self.co,
            &fb_ids::new_v7().to_string(),
            &Self::file(),
            1000,
        )
        .unwrap()
    }
    fn finish(&mut self, mismatch: bool) -> Value {
        let b = self.begin();
        finish_read(
            &mut self.book,
            self.member,
            self.co,
            b["id"].as_str().unwrap(),
            1,
            &parsed(mismatch),
            1001,
        )
        .unwrap()
    }
    fn payload(&self) -> Value {
        json!({"vendor_id":self.vendor,"date":"2026-10-15","due_date":"2026-11-14","reference":"receipt-ref","memo":"","request_key":fb_ids::new_v7().to_string(),"lines":[{"description":"Widgets","quantity":"3","rate":"2.00","expense_account_id":self.expense.to_db(),"ai_line":1}]})
    }
    fn count(&self, table: &str) -> i64 {
        self.book
            .connection()
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }
    fn snapshot(&self) -> fb_db::test_support::Snapshot {
        fb_db::test_support::snapshot(self.book.connection()).unwrap()
    }
}
fn raw(mismatch: bool) -> Value {
    json!({"vendor_name":"Northwind Supply","vendor_address":["Synthetic address"],"document_number":"receipt-ref","document_date":"2026-10-15","due_date":"","currency":"USD","subtotal":if mismatch{"7.00"}else{"6.00"},"tax":"0","total":if mismatch{"7.00"}else{"6.00"},"lines":[{"description":"Widgets","quantity":"3","unit_price":"2.00","amount":if mismatch{"7.00"}else{"6.00"}}]})
}
fn parsed(mismatch: bool) -> Value {
    receipts::parse_reply(&raw(mismatch).to_string(), "USD", "2026-10-30").unwrap()
}

#[test]
fn settings_staff_scope_disconnect_and_notice() {
    let mut f = Fx::new();
    assert!(set_read_attachments(&mut f.book, f.member, f.co, true).is_err());
    assert_eq!(connection_for(&mut f.book, f.actor, f.co).unwrap(), None);
    set_read_attachments(&mut f.book, f.actor, f.co, true).unwrap();
    let state = json!({"mode":"siwc","account_email":"synthetic@example.invalid","registration":true,"model_choices":[{"slug":"a","display_name":"A"},{"slug":"b","display_name":"B"}]});
    let c = connected(&mut f.book, f.actor, f.co, &state).unwrap();
    assert_eq!(c["model"], "a");
    assert_eq!(c["notice_pending"], true);
    set_model(&mut f.book, f.actor, f.co, " b ").unwrap();
    assert!(set_model(&mut f.book, f.actor, f.co, "unknown").is_err());
    dismiss_notice(&mut f.book, f.actor, f.co).unwrap();
    assert_eq!(
        connected(&mut f.book, f.actor, f.co, &state).unwrap()["notice_pending"],
        false
    );
    refresh_models(
        &mut f.book,
        f.actor,
        f.co,
        &json!([{"slug":"c","display_name":"C"}]),
    )
    .unwrap();
    let d = disconnect(&mut f.book, f.actor, f.co, Some(false)).unwrap();
    assert_eq!(d["read_attachments"], true);
    assert_eq!(d["notice_shown"], true);
    assert_eq!(d["credential_present"], false);
    assert_eq!(d["model_choices"], json!([]));
    assert!(disconnect(&mut f.book, f.actor, f.co, None).is_err());
    assert!(f.book.check().unwrap().is_empty());
}
#[test]
fn availability_and_cross_company_refusals_leave_no_readings() {
    let mut f = Fx::new();
    assert_eq!(
        begin_read(
            &mut f.book,
            f.member,
            f.co,
            &fb_ids::new_v7().to_string(),
            &Fx::file(),
            1000
        )
        .unwrap_err()
        .message(),
        NOT_CONNECTED
    );
    connected(
        &mut f.book,
        f.actor,
        f.co,
        &json!({"mode":"api_key","model_choices":[]}),
    )
    .unwrap();
    assert_eq!(
        begin_read(
            &mut f.book,
            f.member,
            f.co,
            &fb_ids::new_v7().to_string(),
            &Fx::file(),
            1000
        )
        .unwrap_err()
        .message(),
        NOT_AVAILABLE
    );
    set_read_attachments(&mut f.book, f.actor, f.co, true).unwrap();
    needs_signin(&mut f.book, f.actor, f.co, SigninFailure::new("ai.call_failed", "unauthorized").unwrap()).unwrap();
    assert_eq!(
        begin_read(
            &mut f.book,
            f.member,
            f.co,
            &fb_ids::new_v7().to_string(),
            &Fx::file(),
            1000
        )
        .unwrap_err()
        .message(),
        NEEDS_SIGNIN
    );
    assert_eq!(f.count("assistant_reading"), 0);
    let other = services::create_company(&mut f.book, f.actor, "Other synthetic", "USD", false)
        .unwrap()
        .id;
    assert!(review_queue(&mut f.book, f.member, other).is_err());
}
#[test]
fn idempotency_stale_retry_and_late_callback() {
    let mut f = Fx::new();
    f.connect();
    let file = Fx::file();
    let key = fb_ids::new_v7().to_string();
    let r = begin_read(&mut f.book, f.member, f.co, &key, &file, 1000).unwrap();
    assert_eq!(
        begin_read(&mut f.book, f.member, f.co, &key, &file, 1600).unwrap()["attempt"],
        1
    );
    let retry = begin_read(&mut f.book, f.member, f.co, &key, &file, 1601).unwrap();
    assert_eq!(retry["id"], r["id"]);
    assert_eq!(retry["attempt"], 2);
    assert!(
        finish_read(
            &mut f.book,
            f.member,
            f.co,
            r["id"].as_str().unwrap(),
            1,
            &parsed(false),
            1602
        )
        .is_err()
    );
    let done = finish_read(
        &mut f.book,
        f.member,
        f.co,
        r["id"].as_str().unwrap(),
        2,
        &parsed(false),
        1602,
    )
    .unwrap();
    assert_eq!(
        begin_read(&mut f.book, f.member, f.co, &key, &file, 1700).unwrap(),
        done
    );
    let mut different = file.clone();
    different["sha256"] = json!("b".repeat(64));
    assert!(begin_read(&mut f.book, f.member, f.co, &key, &different, 1700).is_err());
    assert_eq!(f.count("assistant_reading"), 1);
    assert_eq!(f.count("payables_bill"), 0);
    assert_eq!(f.count("ledger_journal"), 0);
}
#[test]
fn callback_rechecks_consent_generation_and_status() {
    let mut f = Fx::new();
    f.connect();
    let b = f.begin();
    let id = b["id"].as_str().unwrap();
    set_read_attachments(&mut f.book, f.actor, f.co, false).unwrap();
    assert!(finish_read(&mut f.book, f.member, f.co, id, 1, &parsed(false), 1001).is_err());
    set_read_attachments(&mut f.book, f.actor, f.co, true).unwrap();
    disconnect(&mut f.book, f.actor, f.co, None).unwrap();
    f.connect();
    assert!(finish_read(&mut f.book, f.member, f.co, id, 1, &parsed(false), 1002).is_err());
    assert_eq!(
        get_reading(&mut f.book, f.member, f.co, id).unwrap()["status"],
        "pending"
    );
}
#[test]
fn rate_limit_is_per_company_operator_and_counts_pending() {
    let mut f = Fx::new();
    f.connect();
    for _ in 0..30 {
        f.begin();
    }
    assert!(
        begin_read(
            &mut f.book,
            f.member,
            f.co,
            &fb_ids::new_v7().to_string(),
            &Fx::file(),
            1000
        )
        .unwrap_err()
        .message()
        .starts_with("Too many receipts")
    );
    assert!(
        begin_read(
            &mut f.book,
            f.actor,
            f.co,
            &fb_ids::new_v7().to_string(),
            &Fx::file(),
            1000
        )
        .is_ok()
    );
    assert!(
        begin_read(
            &mut f.book,
            f.member,
            f.co,
            &fb_ids::new_v7().to_string(),
            &Fx::file(),
            1601
        )
        .is_ok()
    );
}
#[test]
fn mismatches_block_save_corrections_and_explicit_keep_are_draft_only() {
    let mut f = Fx::new();
    f.connect();
    let r = f.finish(true);
    let id = r["id"].as_str().unwrap();
    let p = f.payload();
    let before = f.snapshot();
    let error = save_review(&mut f.book, f.member, f.co, id, &p).unwrap_err();
    assert!(
        error
            .message()
            .contains("3 × 2.00 = 6.00, the receipt shows 7.00")
    );
    assert_eq!(f.snapshot(), before);
    let mut forged = p.clone();
    forged["lines"][0]["amount"] = json!("6.00");
    assert!(save_review(&mut f.book, f.member, f.co, id, &forged).is_err());
    let mut keep = p;
    keep["lines"][0]["keep_as_entered"] = json!(true);
    let bill = save_review(&mut f.book, f.member, f.co, id, &keep).unwrap();
    assert_eq!(bill["current"]["state"], "DRAFT");
    assert_eq!(bill["current"]["total"], "6.00");
    assert_eq!(f.count("ledger_journal"), 0);
    assert!(
        review_queue(&mut f.book, f.member, f.co)
            .unwrap()
            .is_empty()
    );
    let detail: String = f
        .book
        .connection()
        .query_row(
            "SELECT detail FROM ledger_auditevent WHERE action='bill.ai_lines_kept'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(detail.contains("receipt shows 7.00"));
    let mut g = Fx::new();
    g.connect();
    let r = g.finish(true);
    let mut fixed = g.payload();
    fixed["lines"][0]["quantity"] = json!("1");
    fixed["lines"][0]["rate"] = json!("7.00");
    assert!(
        save_review(
            &mut g.book,
            g.member,
            g.co,
            r["id"].as_str().unwrap(),
            &fixed
        )
        .is_ok()
    );
    assert_eq!(g.count("ledger_journal"), 0);
}
#[test]
fn defaults_are_local_normalized_and_reloaded() {
    let mut f = Fx::new();
    f.connect();
    let r = f.finish(false);
    let id = r["id"].as_str().unwrap();
    let ai = form_initial(&mut f.book, f.member, f.co, id).unwrap();
    assert_eq!(ai["initial"]["vendor"], f.vendor);
    assert_eq!(ai["initial"]["due_date"], "2026-11-14");
    assert_eq!(ai["lines"][0]["expense_account"], f.expense.to_db());
    for name in [
        "NÓRTHWIND SUPPLY, LLC",
        "northwind supply ltd.",
        "Northwind Supply Co Inc",
    ] {
        assert_eq!(receipts::normalize_name(name), "northwind supply");
    }
    directory::save_record(
        &mut f.book,
        f.actor,
        f.co,
        directory::Model::Vendor,
        &json!({"display_name":"Northwind Supply LLC"}),
        None,
    )
    .unwrap();
    assert_eq!(
        form_initial(&mut f.book, f.member, f.co, id).unwrap()["initial"]["vendor"],
        Value::Null
    );
}
#[test]
fn bridge_requires_persisted_complete_matching_artifact() {
    let mut f = Fx::new();
    f.connect();
    let r = f.finish(false);
    let p = json!({"vendor":"Northwind Supply","date":"2026-10-15","lines":[{"description":"Widgets","quantity":30000,"unit_price_cents":200,"tax_collected_cents":0,"currency":"USD"}]});
    let id = create_draft_bill(
        &mut f.book,
        f.member,
        f.co,
        r["request_key"].as_str().unwrap(),
        &p,
    )
    .unwrap();
    assert_eq!(id, r["id"]);
    let mut bad = p.clone();
    bad["lines"][0]["unit_price_cents"] = json!(201);
    assert!(
        create_draft_bill(
            &mut f.book,
            f.member,
            f.co,
            r["request_key"].as_str().unwrap(),
            &bad
        )
        .is_err()
    );
    assert!(
        create_draft_bill(
            &mut f.book,
            f.member,
            f.co,
            &fb_ids::new_v7().to_string(),
            &p
        )
        .is_err()
    );
    assert_eq!(f.count("payables_bill"), 0);
}
#[test]
fn parser_preserves_mismatch_currency_tax_and_half_even() {
    let mut v = raw(true);
    let p = receipts::parse_reply(&v.to_string(), "USD", "2026-10-30").unwrap();
    assert_eq!(p["lines"][0]["rate"], "2.00");
    assert_eq!(p["lines"][0]["mismatch"], true);
    v["currency"] = json!("CAD");
    assert!(receipts::parse_reply(&v.to_string(), "USD", "2026-10-30").is_err());
    v["currency"] = json!("");
    v["tax"] = json!("1.005");
    v["total"] = json!("8");
    assert_eq!(
        receipts::parse_reply(&v.to_string(), "USD", "2026-10-30").unwrap()["tax"],
        "1.00"
    );
    v["total"] = json!("-1");
    assert!(receipts::parse_reply(&v.to_string(), "USD", "2026-10-30").is_err());
    assert!(receipts::parse_reply("[]", "USD", "2026-10-30").is_err());
    for bad in ["NaN", "Infinity", "1e999999999", "99999999999999.991"] {
        v["total"] = json!(bad);
        assert!(
            receipts::parse_reply(&v.to_string(), "USD", "2026-10-30").is_err(),
            "{bad}"
        );
    }
}
#[test]
fn sql_replacement_identity_delete_rowid_and_parent_matrix() {
    let mut f = Fx::new();
    f.connect();
    let r = f.finish(false);
    let rid = r["id"].as_str().unwrap();
    for recursion in [0, 1] {
        f.book
            .connection()
            .execute_batch(&format!("PRAGMA recursive_triggers={recursion}"))
            .unwrap();
        let before = f.snapshot();
        for table in ["assistant_connection", "assistant_reading"] {
            for mode in ["UPDATE", "UPDATE OR REPLACE"] {
                assert!(
                    f.book
                        .connection()
                        .execute(
                            &format!("{mode} {table} SET id=?"),
                            [fb_ids::new_v7().to_string()]
                        )
                        .is_err()
                );
            }
            assert!(
                f.book
                    .connection()
                    .execute(&format!("DELETE FROM {table}"), [])
                    .is_err()
            );
            for alias in ["rowid", "_rowid_", "oid"] {
                assert!(
                    f.book
                        .connection()
                        .prepare(&format!("SELECT {alias} FROM {table}"))
                        .is_err()
                );
            }
            let cols = f
                .book
                .connection()
                .prepare(&format!("SELECT * FROM {table}"))
                .unwrap()
                .column_names()
                .map(str::to_owned)
                .collect::<Vec<_>>();
            assert!(
                f.book
                    .connection()
                    .execute(
                        &format!(
                            "INSERT OR REPLACE INTO {table}({}) SELECT {} FROM {table}",
                            cols.join(","),
                            cols.join(",")
                        ),
                        []
                    )
                    .is_err()
            );
        }
        for change in [
            "result='{}'",
            "status='failed'",
            "request_key='00000000-0000-4000-8000-000000000001'",
            "source_file_id='00000000-0000-4000-8000-000000000001'",
            "created_by_id='00000000-0000-4000-8000-000000000001'",
        ] {
            assert!(
                f.book
                    .connection()
                    .execute(
                        &format!("UPDATE assistant_reading SET {change} WHERE id=?"),
                        [rid]
                    )
                    .is_err(),
                "{change}"
            );
        }
        assert_eq!(f.snapshot(), before);
    }
    f.book
        .connection()
        .execute_batch("PRAGMA recursive_triggers=ON")
        .unwrap();
    assert!(f.book.check().unwrap().is_empty());
    drop(f.book);
    assert!(Book::open(&f.path, &[81; 32]).is_ok());
}
#[test]
fn malformed_and_duplicate_stored_evidence_refused_without_panics() {
    let mut f = Fx::new();
    f.connect();
    let r = f.begin();
    let id = r["id"].as_str().unwrap();
    for bad in [
        json!(null),
        json!([]),
        json!({}),
        json!({"lines":[{"amount":false}]}),
    ] {
        let before = f.snapshot();
        assert!(finish_read(&mut f.book, f.member, f.co, id, 1, &bad, 1001).is_err());
        assert_eq!(f.snapshot(), before);
    }
    let duplicate = "[{\"slug\":\"a\",\"slug\":\"b\",\"display_name\":\"A\"}]";
    assert!(
        f.book
            .connection()
            .execute(
                "UPDATE assistant_connection SET model_choices=?",
                [duplicate]
            )
            .is_err()
    );
    assert!(
        f.book
            .connection()
            .execute(
                "UPDATE assistant_reading SET result=?,status='ok',finished_at=1001 WHERE id=?",
                params!["{\"lines\":[],\"lines\":[]}", id]
            )
            .is_err()
    );
}
#[test]
fn every_public_write_rolls_back_after_late_audit_failure() {
    let mut f = Fx::new();
    f.connect();
    let r = f.begin();
    let id = r["id"].as_str().unwrap().to_owned();
    f.book.connection().execute_batch("CREATE TRIGGER test_assistant_audit BEFORE INSERT ON ledger_auditevent BEGIN SELECT RAISE(ABORT,'test audit failure'); END").unwrap();
    let before = f.snapshot();
    assert!(set_read_attachments(&mut f.book, f.actor, f.co, false).is_err());
    assert_eq!(f.snapshot(), before);
    assert!(
        connected(
            &mut f.book,
            f.actor,
            f.co,
            &json!({"mode":"api_key","model_choices":[]})
        )
        .is_err()
    );
    assert_eq!(f.snapshot(), before);
    assert!(set_model(&mut f.book, f.actor, f.co, "demo-model").is_err());
    assert_eq!(f.snapshot(), before);
    assert!(disconnect(&mut f.book, f.actor, f.co, None).is_err());
    assert_eq!(f.snapshot(), before);
    assert!(needs_signin(&mut f.book, f.actor, f.co, SigninFailure::new("ai.call_failed", "unauthorized").unwrap()).is_err());
    assert_eq!(f.snapshot(), before);
    assert!(finish_read(&mut f.book, f.member, f.co, &id, 1, &parsed(false), 1001).is_err());
    assert_eq!(f.snapshot(), before);
    assert!(fail_read(&mut f.book, f.member, f.co, &id, 1, "schema", 1001).is_err());
    assert_eq!(f.snapshot(), before);
    assert!(
        begin_read(
            &mut f.book,
            f.member,
            f.co,
            &fb_ids::new_v7().to_string(),
            &Fx::file(),
            1601
        )
        .is_err()
    );
    assert_eq!(f.snapshot(), before);
    f.book
        .connection()
        .execute_batch("DROP TRIGGER test_assistant_audit")
        .unwrap();
    finish_read(&mut f.book, f.member, f.co, &id, 1, &parsed(true), 1001).unwrap();
    let p = f.payload();
    let mut p = p;
    p["lines"][0]["keep_as_entered"] = json!(true);
    f.book.connection().execute_batch("CREATE TRIGGER test_assistant_audit BEFORE INSERT ON ledger_auditevent WHEN NEW.action='bill.ai_lines_kept' BEGIN SELECT RAISE(ABORT,'test audit failure'); END").unwrap();
    let before = f.snapshot();
    assert!(save_review(&mut f.book, f.member, f.co, &id, &p).is_err());
    assert_eq!(f.snapshot(), before);
}
#[test]
fn populated_50c3f74_copy_upgrades_without_changing_any_prior_rows() {
    let mut f = Fx::new();
    let equity = services::save_account(
        &mut f.book,
        f.actor,
        f.co,
        AccountInput::new("300", "Synthetic equity").kind("EQUITY"),
    )
    .unwrap()
    .id;
    let j = services::save_draft(
        &mut f.book,
        f.actor,
        f.co,
        services::DraftInput::new(
            "2026-10-01",
            "Synthetic upgrade",
            vec![
                services::line(f.expense).debit("10.00").build(),
                services::line(equity).credit("10.00").build(),
            ],
        ),
    )
    .unwrap();
    services::post_journal(&mut f.book, f.actor, f.co, j.id, None).unwrap();
    services::reverse_journal(
        &mut f.book,
        f.actor,
        f.co,
        j.id,
        "2026-10-02",
        "Synthetic reversal",
    )
    .unwrap();
    let mut payload = f.payload();
    payload["lines"][0]
        .as_object_mut()
        .unwrap()
        .remove("ai_line");
    payables::save_bill(&mut f.book, f.actor, f.co, &payload).unwrap();
    let path = f.path.with_extension("prior.fbbook");
    let copy = fb_db::test_support::populated_copy(
        f.book.connection(),
        &path,
        &[81; 32],
        "0078_shared_unique_json",
        true,
    )
    .unwrap();
    for _ in 0..2 {
        let upgraded = Book::open(&path, &[81; 32]).unwrap();
        assert_eq!(
            fb_db::test_support::snapshot(upgraded.connection()).unwrap(),
            copy.expected
        );
        assert!(upgraded.check().unwrap().is_empty());
    }
}

#[test]
fn malformed_optional_inputs_are_refused_without_changes() {
    let mut f = Fx::new();
    f.connect();
    for bad in [Value::Null, json!(false), json!(42), json!([]), json!({})] {
        for field in ["account_email", "key_last4", "registration"] {
            if field == "registration" && bad.is_boolean() {
                continue;
            }
            let mut state = json!({"mode":"siwc","model_choices":[]});
            state[field] = bad.clone();
            let before = f.snapshot();
            assert!(
                connected(&mut f.book, f.actor, f.co, &state).is_err(),
                "{field}={bad}"
            );
            assert_eq!(f.snapshot(), before);
        }
        let mut file = Fx::file();
        file["bill_id"] = bad.clone();
        if !bad.is_null() {
            let before = f.snapshot();
            assert!(
                begin_read(
                    &mut f.book,
                    f.actor,
                    f.co,
                    &fb_ids::new_v7().to_string(),
                    &file,
                    1000
                )
                .is_err(),
                "bill_id={bad}"
            );
            assert_eq!(f.snapshot(), before);
        }
    }
}

#[test]
fn expired_upload_can_retry_with_a_fresh_verified_file() {
    let mut f = Fx::new();
    f.connect();
    let key = fb_ids::new_v7().to_string();
    let file = Fx::file();
    let old = begin_read(&mut f.book, f.member, f.co, &key, &file, 1000).unwrap();
    let mut fresh = file.clone();
    fresh["source_file_id"] = json!(fb_ids::new_v7().to_string());
    fresh["original_name"] = json!("retry.png");
    let retry = begin_read(&mut f.book, f.member, f.co, &key, &fresh, 1601).unwrap();
    assert_eq!(retry["id"], old["id"]);
    assert_eq!(retry["attempt"], 2);
    assert_eq!(retry["source_file_id"], fresh["source_file_id"]);
    finish_read(
        &mut f.book,
        f.member,
        f.co,
        old["id"].as_str().unwrap(),
        2,
        &parsed(false),
        1602,
    )
    .unwrap();
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn sql_distinct_key_victims_and_all_parent_identities_are_protected() {
    let mut f = Fx::new();
    f.connect();
    let r = f.finish(false);
    let r2 = f.begin();
    let id = r["id"].as_str().unwrap();
    let other = r2["id"].as_str().unwrap();
    let otherco = services::create_company(&mut f.book, f.actor, "Second synthetic", "USD", false)
        .unwrap()
        .id;
    set_read_attachments(&mut f.book, f.actor, otherco, true).unwrap();
    let c1 = connection_for(&mut f.book, f.actor, f.co).unwrap().unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let c2 = connection_for(&mut f.book, f.actor, otherco)
        .unwrap()
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    f.book.connection().execute_batch("CREATE VIEW assistant_attack AS SELECT id,request_key FROM assistant_reading; CREATE TRIGGER assistant_attack_update INSTEAD OF UPDATE ON assistant_attack BEGIN UPDATE assistant_reading SET request_key=NEW.request_key WHERE id=OLD.id; END;").unwrap();
    for recursion in [0, 1] {
        f.book
            .connection()
            .execute_batch(&format!("PRAGMA recursive_triggers={recursion}"))
            .unwrap();
        let before = f.snapshot();
        for mode in ["UPDATE", "UPDATE OR REPLACE"] {
            for (sql, args) in [
                (
                    format!("{mode} assistant_reading SET id=? WHERE id=?"),
                    vec![id, other],
                ),
                (
                    format!("{mode} assistant_reading SET request_key=? WHERE id=?"),
                    vec![r["request_key"].as_str().unwrap(), other],
                ),
                (
                    format!("{mode} assistant_attack SET request_key=? WHERE id=?"),
                    vec![r["request_key"].as_str().unwrap(), other],
                ),
                (
                    format!("{mode} assistant_connection SET company_id=? WHERE id=?"),
                    vec![&f.co.to_db(), &c2],
                ),
                (
                    format!("{mode} assistant_connection SET id=? WHERE id=?"),
                    vec![&c1, &c2],
                ),
            ] {
                assert!(
                    f.book
                        .connection()
                        .execute(&sql, fb_db::rusqlite::params_from_iter(args))
                        .is_err(),
                    "{sql}"
                );
                assert_eq!(f.snapshot(), before);
            }
        }
        for parent in ["fb_user", "ledger_company"] {
            let key = if parent == "fb_user" {
                f.actor.to_db()
            } else {
                f.co.to_db()
            };
            assert!(
                f.book
                    .connection()
                    .execute(&format!("DELETE FROM {parent} WHERE id=?"), [&key])
                    .is_err()
            );
            assert!(
                f.book
                    .connection()
                    .execute(
                        &format!("UPDATE {parent} SET id=? WHERE id=?"),
                        params![fb_ids::new_v7().to_string(), key]
                    )
                    .is_err()
            );
            assert_eq!(f.snapshot(), before);
        }
        for table in ["assistant_connection", "assistant_reading"] {
            let cols = f
                .book
                .connection()
                .prepare(&format!("SELECT * FROM {table}"))
                .unwrap()
                .column_names()
                .map(str::to_owned)
                .collect::<Vec<_>>();
            let source = if table == "assistant_connection" {
                &c1
            } else {
                id
            };
            let select = cols
                .iter()
                .map(|c| {
                    if c == "id" {
                        format!("'{}'", fb_ids::new_v7())
                    } else {
                        c.clone()
                    }
                })
                .collect::<Vec<_>>()
                .join(",");
            assert!(f.book.connection().execute(&format!("INSERT OR REPLACE INTO {table}({}) SELECT {select} FROM {table} WHERE id=?",cols.join(",")),[source]).is_err());
            assert_eq!(f.snapshot(), before);
            assert!(f.book.connection().execute(&format!("INSERT INTO {table}({}) SELECT {} FROM {table} WHERE id=? ON CONFLICT(id) DO UPDATE SET id=excluded.id",cols.join(","),cols.join(",")),[source]).is_err());
            assert_eq!(f.snapshot(), before);
        }
    }
    f.book.connection().execute_batch("PRAGMA recursive_triggers=ON; DROP TRIGGER assistant_attack_update; DROP VIEW assistant_attack;").unwrap();
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn native_assistant_predicate_is_pure_in_both_capabilities() {
    let mut f = Fx::new();
    let good = parsed(false).to_string();
    f.book
        .read(|c| -> FbResult<()> {
            assert!(
                c.query_row("SELECT fb_assistant_json('result',?)", [&good], |r| r
                    .get::<_, bool>(0))?
            );
            assert!(c.query_row("SELECT fb_assistant_model('','[]')", [], |r| {
                r.get::<_, bool>(0)
            })?);
            assert!(
                !c.query_row("SELECT fb_assistant_model('unknown','[]')", [], |r| r
                    .get::<_, bool>(0))?
            );
            assert!(
                !c.query_row("SELECT fb_assistant_json('choices','{}')", [], |r| r
                    .get::<_, bool>(0))?
            );
            Ok(())
        })
        .unwrap();
    f.book
        .write(|c| -> FbResult<()> {
            assert!(
                c.query_row("SELECT fb_assistant_json('result',?)", [&good], |r| r
                    .get::<_, bool>(0))?
            );
            assert!(c.query_row("SELECT fb_assistant_model('','[]')", [], |r| {
                r.get::<_, bool>(0)
            })?);
            assert!(
                !c.query_row("SELECT fb_assistant_model('unknown','[]')", [], |r| r
                    .get::<_, bool>(0))?
            );
            assert!(c.execute_batch("PRAGMA recursive_triggers=OFF").is_err());
            Ok(())
        })
        .unwrap();
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn every_added_schema_definition_is_checked_and_open_refuses_tampered_evidence() {
    let mut f = Fx::new();
    f.connect();
    let r = f.finish(false);
    let objects=f.book.connection().prepare("SELECT type,name,sql FROM sqlite_master WHERE name LIKE '%assistant%' AND sql IS NOT NULL ORDER BY name").unwrap().query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?))).unwrap().collect::<Result<Vec<_>,_>>().unwrap();
    for (kind, name, sql) in objects {
        if kind == "table" {
            continue;
        }
        f.book
            .connection()
            .execute_batch(&format!("DROP {kind} {name}"))
            .unwrap();
        let checked = f.book.check();
        assert!(checked.is_err() || !checked.unwrap().is_empty(), "{name}");
        assert!(Book::open(&f.path, &[81; 32]).is_err(), "{name}");
        f.book.connection().execute_batch(&sql).unwrap();
    }
    let raw = parsed(false).to_string().replace(
        "\"quantity\":\"3\"",
        "\"quantity\":\"3\",\"quantity\":\"3\"",
    );
    f.book.connection().execute_batch("DROP TRIGGER fb_json_assistant_reading_update; DROP TRIGGER fb_assistant_reading_update; PRAGMA ignore_check_constraints=ON").unwrap();
    f.book
        .connection()
        .execute(
            "UPDATE assistant_reading SET result=? WHERE id=?",
            params![raw, r["id"].as_str().unwrap()],
        )
        .unwrap();
    f.book
        .connection()
        .execute_batch("PRAGMA ignore_check_constraints=OFF")
        .unwrap();
    for name in [
        "fb_json_assistant_reading_update",
        "fb_assistant_reading_update",
    ] {
        let sql = fb_db::MIGRATIONS
            .iter()
            .flat_map(|(_, s)| s.split("CREATE TRIGGER ").skip(1))
            .find(|s| s.starts_with(&format!("{name} ")))
            .unwrap()
            .split("END;")
            .next()
            .unwrap();
        f.book
            .connection()
            .execute_batch(&format!("CREATE TRIGGER {sql}END;"))
            .unwrap();
    }
    assert!(get_reading(&mut f.book, f.member, f.co, r["id"].as_str().unwrap()).is_err());
    assert!(!f.book.check().unwrap().is_empty());
    assert!(Book::open(&f.path, &[81; 32]).is_err());
}

#[test]
fn parser_precision_aggregates_signed_zero_and_extreme_products() {
    let mut v = raw(false);
    v["total"] = json!("6.".to_owned() + &"0".repeat(100));
    assert_eq!(
        receipts::parse_reply(&v.to_string(), "USD", "2026-10-30").unwrap()["total"],
        "6.00"
    );
    v["total"] = json!("1e-1000");
    assert_eq!(
        receipts::parse_reply(&v.to_string(), "USD", "2026-10-30").unwrap()["total"],
        "0.00"
    );
    v["total"] = json!("-1e1000");
    assert!(
        receipts::parse_reply(&v.to_string(), "USD", "2026-10-30")
            .unwrap_err()
            .message()
            .contains("negative amount")
    );
    v = raw(false);
    v["subtotal"] = json!("");
    v["lines"] = json!([{"description":"Large computed","quantity":"2","unit_price":"99999999999999","amount":""}]);
    let p = receipts::parse_reply(&v.to_string(), "USD", "2026-10-30").unwrap();
    assert_eq!(p["subtotal"], "199999999999998.00");
    v["lines"][0]["quantity"] = json!("99999999999999");
    assert!(receipts::parse_reply(&v.to_string(), "USD", "2026-10-30").is_err()); // Python raises decimal.InvalidOperation; controlled refusal here.
    v = raw(false);
    for k in ["subtotal", "tax", "total"] {
        v[k] = json!("-0");
    }
    v["lines"] = json!([{"description":"Zero","quantity":"1","unit_price":"-0","amount":"-0"}]);
    let p = receipts::parse_reply(&v.to_string(), "USD", "2026-10-30").unwrap();
    for k in ["subtotal", "tax", "total"] {
        assert_eq!(p[k], "-0.00");
    }
    assert_eq!(p["lines"][0]["rate"], "-0.00");
    assert_eq!(receipts::normalize_name("A\u{034f}B"), "a b");
    v["vendor_name"] = json!("A\u{0897}B\u{2028}C");
    assert_eq!(
        receipts::parse_reply(&v.to_string(), "USD", "2026-10-30").unwrap()["vendor_name"],
        "AB C"
    );
}

#[test]
fn deleted_added_unbound_and_tampered_printed_lines_follow_python_save_rules() {
    for ai_index in [json!(99), json!("tampered"), Value::Null] {
        let mut f = Fx::new();
        f.connect();
        let r = f.finish(true);
        let mut p = f.payload();
        p["lines"][0]["ai_line"] = ai_index;
        let bill = save_review(&mut f.book, f.member, f.co, r["id"].as_str().unwrap(), &p).unwrap();
        assert_eq!(bill["current"]["total"], "6.00");
        assert_eq!(f.count("ledger_journal"), 0);
    }
    let mut f = Fx::new();
    f.connect();
    let r = f.finish(true);
    let mut p = f.payload();
    let mut added = p["lines"][0].clone();
    added.as_object_mut().unwrap().remove("ai_line");
    p["lines"][0]["DELETE"] = json!(true);
    p["lines"].as_array_mut().unwrap().push(added);
    assert!(save_review(&mut f.book, f.member, f.co, r["id"].as_str().unwrap(), &p).is_ok());
}

#[test]
fn provider_timestamps_are_plain_scoped_and_generation_bound() {
    let mut f = Fx::new();
    f.connect();
    let row = connection_for(&mut f.book, f.member, f.co)
        .unwrap()
        .unwrap();
    let generation = row["generation"].as_i64().unwrap();
    provider_metadata(
        &mut f.book,
        f.member,
        f.co,
        generation,
        Some(2000),
        Some(1000),
    )
    .unwrap();
    let row = connection_for(&mut f.book, f.member, f.co)
        .unwrap()
        .unwrap();
    assert_eq!(row["token_expires_at"], 2000);
    assert_eq!(row["last_used_at"], 1000);
    let before = f.snapshot();
    assert!(provider_metadata(&mut f.book, f.member, f.co, generation, Some(-1), None).is_err());
    assert_eq!(f.snapshot(), before);
    disconnect(&mut f.book, f.actor, f.co, None).unwrap();
    f.connect();
    let before = f.snapshot();
    assert!(provider_metadata(&mut f.book, f.member, f.co, generation, Some(3000), None).is_err());
    assert_eq!(f.snapshot(), before);
}

#[test]
fn a_new_receipt_cannot_be_spliced_into_an_unrelated_existing_bill() {
    let mut f = Fx::new();
    f.connect();
    let row = f.finish(false);
    let mut p = f.payload();
    p["lines"][0].as_object_mut().unwrap().remove("ai_line");
    let bill = payables::save_bill(&mut f.book, f.actor, f.co, &p).unwrap();
    p["bill_id"] = bill["id"].clone();
    p["expected_revision"] = json!(1);
    let before = f.snapshot();
    assert_eq!(
        save_review(&mut f.book, f.member, f.co, row["id"].as_str().unwrap(), &p)
            .unwrap_err()
            .message(),
        OUT_OF_DATE
    );
    assert_eq!(f.snapshot(), before);
}

#[test]
fn refresh_models_audit_failure_rolls_back_and_all_missing_parents_are_immediate() {
    let mut f = Fx::new();
    connected(
        &mut f.book,
        f.actor,
        f.co,
        &json!({"mode":"siwc","model_choices":[]}),
    )
    .unwrap();
    set_read_attachments(&mut f.book, f.actor, f.co, true).unwrap();
    f.book.connection().execute_batch("CREATE TRIGGER test_assistant_audit BEFORE INSERT ON ledger_auditevent BEGIN SELECT RAISE(ABORT,'test audit failure'); END").unwrap();
    let before = f.snapshot();
    assert!(
        refresh_models(
            &mut f.book,
            f.actor,
            f.co,
            &json!([{"slug":"new","display_name":"New"}])
        )
        .is_err()
    );
    assert_eq!(f.snapshot(), before);
    f.book
        .connection()
        .execute_batch("DROP TRIGGER test_assistant_audit")
        .unwrap();
    let id = fb_ids::new_v7().to_string();
    let nonexistent = fb_ids::new_v7().to_string();
    let newco =
        services::create_company(&mut f.book, f.actor, "No assistant parents", "USD", false)
            .unwrap()
            .id
            .to_db();
    for (co, actor) in [
        (nonexistent.as_str(), f.actor.to_db()),
        (newco.as_str(), nonexistent.clone()),
    ] {
        assert!(
            f.book
                .connection()
                .execute(
                    "INSERT INTO assistant_connection(id,company_id,created_by_id) VALUES(?,?,?)",
                    params![id, co, actor]
                )
                .is_err()
        );
    }
    let pending = f.begin();
    let rid = pending["id"].as_str().unwrap();
    let before = f.snapshot();
    for (assignment, value) in [
        ("created_by_id", nonexistent.clone()),
        ("company_id", nonexistent.clone()),
        ("source_file_id", nonexistent.clone()),
    ] {
        assert!(
            f.book
                .connection()
                .execute(
                    &format!("UPDATE assistant_reading SET {assignment}=? WHERE id=?"),
                    params![value, rid]
                )
                .is_err()
        );
        assert_eq!(f.snapshot(), before);
    }
    let mut bad = parsed(false);
    bad["currency"] = json!("CAD");
    assert!(f.book.connection().execute("UPDATE assistant_reading SET result=?,status='ok',finished_at=1001,mode='siwc' WHERE id=?",params![bad.to_string(),rid]).is_err());
    assert_eq!(f.snapshot(), before);
}

#[test]
fn a_host_catching_the_composed_save_error_cannot_commit_bill_or_link_changes() {
    let mut f = Fx::new();
    f.connect();
    let r = f.finish(true);
    let mut p = f.payload();
    p["lines"][0]["keep_as_entered"] = json!(true);
    f.book.connection().execute_batch("CREATE TRIGGER test_assistant_audit BEFORE INSERT ON ledger_auditevent WHEN NEW.action='bill.ai_lines_kept' BEGIN SELECT RAISE(ABORT,'test audit failure'); END").unwrap();
    let before = f.snapshot();
    f.book
        .write(|tx| -> FbResult<()> {
            assert!(save_review_tx(tx, f.member, f.co, r["id"].as_str().unwrap(), &p).is_err());
            Ok(())
        })
        .unwrap();
    assert_eq!(f.snapshot(), before);
}

#[test]
fn every_audited_composable_lifecycle_and_receipt_operation_rolls_back_when_caught() {
    let mut f = Fx::new();
    f.connect();
    let r = f.begin();
    let actor = f.actor;
    let member = f.member;
    let co = f.co;
    let id = r["id"].as_str().unwrap();
    f.book.connection().execute_batch("CREATE TRIGGER test_assistant_audit BEFORE INSERT ON ledger_auditevent BEGIN SELECT RAISE(ABORT,'test audit failure'); END").unwrap();
    macro_rules! caught {
        ($call:expr) => {{
            let before = f.snapshot();
            f.book
                .write(|tx| -> FbResult<()> {
                    assert!($call(tx).is_err());
                    Ok(())
                })
                .unwrap();
            assert_eq!(f.snapshot(), before);
        }};
    }
    caught!(|tx| connected_tx(tx, actor, co, &json!({"mode":"api_key","model_choices":[]})));
    caught!(|tx| disconnect_tx(tx, actor, co, None));
    caught!(|tx| needs_signin_tx(tx, actor, co, SigninFailure::new("ai.call_failed", "unauthorized")?));
    caught!(|tx| finish_read_tx(tx, member, co, id, 1, &parsed(false), 1001));
    caught!(|tx| fail_read_tx(tx, member, co, id, 1, "schema", 1001));
    caught!(|tx| begin_read_tx(
        tx,
        member,
        co,
        &fb_ids::new_v7().to_string(),
        &Fx::file(),
        1601
    ));
}

#[test]
fn a_saved_upload_replay_returns_the_same_reading_without_resaving_the_bill() {
    let mut f = Fx::new();
    f.connect();
    let file = Fx::file();
    let key = fb_ids::new_v7().to_string();
    let pending = begin_read(&mut f.book, f.member, f.co, &key, &file, 1000).unwrap();
    let id = pending["id"].as_str().unwrap();
    finish_read(&mut f.book, f.member, f.co, id, 1, &parsed(false), 1001).unwrap();
    let payload = f.payload();
    let bill = save_review(&mut f.book, f.member, f.co, id, &payload).unwrap();
    let before = f.snapshot();
    let replay = begin_read(&mut f.book, f.member, f.co, &key, &file, 1002).unwrap();
    assert_eq!(replay["id"], id);
    assert_eq!(replay["bill_id"], bill["id"]);
    assert_eq!(f.snapshot(), before);
}

#[test]
fn raw_reply_validation_finishes_failure_with_exact_message_and_closed_audit_code() {
    let mut f = Fx::new();
    f.connect();
    for (raw, code) in [
        ("not JSON".to_owned(), "bad_json"),
        (json!({"wrong":true}).to_string(), "schema"),
        (
            {
                let mut v = raw(false);
                v["currency"] = json!("CAD");
                v.to_string()
            },
            "currency",
        ),
        (
            {
                let mut v = raw(false);
                v["total"] = json!("-1");
                v.to_string()
            },
            "negative",
        ),
    ] {
        let expected = receipts::parse_reply(&raw, "USD", "2026-10-30")
            .unwrap_err()
            .message();
        let pending = f.begin();
        let id = pending["id"].as_str().unwrap();
        let done = complete_reply(
            &mut f.book,
            f.member,
            f.co,
            id,
            1,
            Reply {
                text: &raw,
                today: "2026-10-30",
            },
            1001,
        )
        .unwrap();
        assert_eq!(done["status"], "failed");
        assert_eq!(done["error"], expected);
        let saved:String=f.book.connection().query_row("SELECT json_extract(detail,'$.code') FROM ledger_auditevent WHERE action='ai.receipt_failed' AND json_extract(detail,'$.reading')=?",[id],|r|r.get(0)).unwrap();
        assert_eq!(saved, code);
    }
    assert_eq!(f.count("payables_bill"), 0);
    assert_eq!(f.count("ledger_journal"), 0);
    let pending = f.begin();
    let id = pending["id"].as_str().unwrap();
    f.book.connection().execute_batch("CREATE TRIGGER test_assistant_audit BEFORE INSERT ON ledger_auditevent BEGIN SELECT RAISE(ABORT,'test audit failure'); END").unwrap();
    let before = f.snapshot();
    f.book
        .write(|tx| -> FbResult<()> {
            assert!(
                complete_reply_tx(
                    tx,
                    f.member,
                    f.co,
                    id,
                    1,
                    Reply {
                        text: "not JSON",
                        today: "2026-10-30"
                    },
                    1001
                )
                .is_err()
            );
            Ok(())
        })
        .unwrap();
    assert_eq!(f.snapshot(), before);
}

#[test]
fn python_integer_hidden_indexes_still_bind_authoritative_printed_amounts() {
    let mut f = Fx::new();
    f.connect();
    let r = f.finish(true);
    let id = r["id"].as_str().unwrap();
    for index in [json!(" +１ "), json!("0_1"), json!("\u{2003}1\u{2003}")] {
        let mut p = f.payload();
        p["lines"][0]["ai_line"] = index;
        let before = f.snapshot();
        assert!(
            save_review(&mut f.book, f.member, f.co, id, &p)
                .unwrap_err()
                .message()
                .contains("This line doesn't add up")
        );
        assert_eq!(f.snapshot(), before);
    }
    for (text, expected) in [
        ("+1", Some(1)),
        ("1_0", Some(10)),
        ("_1", None),
        ("1__0", None),
        ("1_", None),
        ("-1", None),
        ("\u{1c}1", None),
    ] {
        assert_eq!(ai_index(&json!(text), 50), expected);
    }
}

#[test]
fn malformed_keep_checkbox_is_refused_even_when_the_line_already_matches() {
    let mut f = Fx::new();
    f.connect();
    let r = f.finish(false);
    for bad in [Value::Null, json!(1), json!("true"), json!([]), json!({})] {
        let mut p = f.payload();
        p["lines"][0]["keep_as_entered"] = bad;
        let before = f.snapshot();
        assert!(save_review(&mut f.book, f.member, f.co, r["id"].as_str().unwrap(), &p).is_err());
        assert_eq!(f.snapshot(), before);
    }
}

#[test]
fn sql_model_selection_cannot_bypass_the_service_picker() {
    let mut f = Fx::new();
    f.connect();
    let before = f.snapshot();
    assert!(
        f.book
            .connection()
            .execute(
                "UPDATE assistant_connection SET model='not-in-the-list'",
                []
            )
            .is_err()
    );
    assert_eq!(f.snapshot(), before);
    assert!(
        f.book
            .connection()
            .execute("UPDATE assistant_connection SET model_choices='[]'", [])
            .is_err()
    );
    assert_eq!(f.snapshot(), before);
    assert!(f.book.check().unwrap().is_empty());
    drop(f.book);
    assert!(Book::open(&f.path, &[81; 32]).is_ok());
}
