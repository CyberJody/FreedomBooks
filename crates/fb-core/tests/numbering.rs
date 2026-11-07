use fb_core::{
    AccountId, Book, CompanyId, UserId, checks, documents,
    numbering::{self, Change},
    services::{self, AccountInput},
    users,
};
use fb_db::rusqlite::params;
use serde_json::{Value, json};

struct Fx {
    _dir: tempfile::TempDir,
    book: Book,
    actor: UserId,
    co: CompanyId,
    bank: AccountId,
    other_bank: AccountId,
    expense: AccountId,
}
impl Fx {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut book = Book::create(&dir.path().join("numbers.fbbook"), &[42; 32]).unwrap();
        let actor = users::create_user(&mut book, "Numbering owner", true).unwrap();
        let co = services::create_company(&mut book, actor, "Synthetic numbering", "USD", false)
            .unwrap()
            .id;
        let bank = services::save_account(
            &mut book,
            actor,
            co,
            AccountInput::new("100", "Checking").kind("ASSET"),
        )
        .unwrap()
        .id;
        let other_bank = services::save_account(
            &mut book,
            actor,
            co,
            AccountInput::new("101", "Savings").kind("ASSET"),
        )
        .unwrap()
        .id;
        let expense = services::save_account(
            &mut book,
            actor,
            co,
            AccountInput::new("500", "Expenses").kind("EXPENSE"),
        )
        .unwrap()
        .id;
        documents::save_setup(
            &mut book,
            actor,
            co,
            &json!({"banks":[bank.to_db(),other_bank.to_db()]}),
            Some(0),
        )
        .unwrap();
        Self {
            _dir: dir,
            book,
            actor,
            co,
            bank,
            other_bank,
            expense,
        }
    }
    fn row(&mut self, kind: &str, bank: Option<AccountId>) -> Value {
        numbering::settings(&mut self.book, self.actor, self.co)
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["kind"] == kind && r["bank_id"] == json!(bank.map(|b| b.to_db())))
            .unwrap()
            .clone()
    }
    fn set(&mut self, kind: &str, bank: Option<AccountId>, next: &str) -> fb_core::FbResult<Value> {
        let before = self.row(kind, bank);
        numbering::set_next(
            &mut self.book,
            self.actor,
            self.co,
            Change {
                kind,
                bank,
                next,
                expected_next: before["next"].as_str().unwrap(),
                expected_revision: before["revision"].as_i64().unwrap(),
            },
        )
    }
    fn identity(&mut self, kind: &str) -> Value {
        self.book
            .write(|tx| {
                documents::create_identity(
                    tx,
                    self.actor,
                    self.co,
                    kind,
                    &fb_ids::new_v7().to_string(),
                )
            })
            .unwrap()
    }
    fn check(&mut self, number: &str) -> Value {
        let p = json!({"bank_id":self.bank.to_db(),"payee_kind":"OTHER","payee_name":"Synthetic supplier","date":"2026-11-05","check_number":number,"memo":"Synthetic numbering","request_key":fb_ids::new_v7().to_string(),"lines":[{"tab":"EXPENSE","description":"Supplies","rate":"10.00","account_id":self.expense.to_db()}]});
        checks::save_check(&mut self.book, self.actor, self.co, &p).unwrap()
    }
    fn audit_count(&mut self) -> i64 {
        self.book
            .read(|q| {
                q.query_row(
                    "SELECT count(*) FROM ledger_auditevent WHERE action='numbering.changed'",
                    [],
                    |r| r.get(0),
                )
            })
            .unwrap()
    }
}

#[test]
fn untouched_books_keep_every_default_and_settings_reads_do_not_write() {
    let mut f = Fx::new();
    let before: i64 = f
        .book
        .read(|q| q.query_row("SELECT count(*) FROM ledger_auditevent", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(f.row("INV", None)["next"], "1");
    assert_eq!(f.row("CHECK", Some(f.bank))["next"], "1");
    let after: i64 = f
        .book
        .read(|q| q.query_row("SELECT count(*) FROM ledger_auditevent", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(before, after);
    assert_eq!(f.audit_count(), 0);
    for kind in documents::KINDS {
        assert_eq!(f.identity(kind)["number"], format!("{kind}-000001"));
        assert_eq!(f.identity(kind)["number"], format!("{kind}-000002"));
    }
    let count: i64 = f
        .book
        .read(|q| q.query_row("SELECT count(*) FROM fb_numbering", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(count, 0);
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn all_shared_kinds_allow_gaps_refuse_collisions_and_continue_without_renumbering() {
    let mut f = Fx::new();
    for kind in documents::KINDS {
        let existing = f.identity(kind);
        let e = f.set(kind, None, "1").unwrap_err().message();
        assert!(e.contains(existing["number"].as_str().unwrap()), "{e}");
        f.set(kind, None, "000100").unwrap();
        assert_eq!(f.identity(kind)["number"], format!("{kind}-000100"));
        assert_eq!(f.identity(kind)["number"], format!("{kind}-000101"));
        // Re-enter an unused gap below the high-water mark, then skip the existing future number.
        f.set(kind, None, "99").unwrap();
        assert_eq!(f.identity(kind)["number"], format!("{kind}-000099"));
        assert_eq!(f.identity(kind)["number"], format!("{kind}-000102"));
        let retained = f
            .book
            .read(|q| documents::identity(q, f.co, existing["id"].as_str().unwrap()))
            .unwrap();
        assert_eq!(retained, existing);
    }
    assert_eq!(f.audit_count(), (documents::KINDS.len() * 2) as i64);
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn check_starts_are_per_bank_preserve_zeroes_and_never_reuse_voided_numbers() {
    let mut f = Fx::new();
    let existing = f.check("900");
    assert_eq!(f.row("CHECK", Some(f.bank))["next"], "901");
    assert!(
        f.set("CHECK", Some(f.bank), "900")
            .unwrap_err()
            .message()
            .contains(existing["number"].as_str().unwrap())
    );
    f.set("CHECK", Some(f.bank), "0007").unwrap();
    assert_eq!(
        checks::next_number(&mut f.book, f.actor, f.co, f.bank).unwrap(),
        "0007"
    );
    let draft = f.check("0007");
    assert_eq!(f.row("CHECK", Some(f.bank))["next"], "0008");
    assert_eq!(f.row("CHECK", Some(f.other_bank))["next"], "1");
    let review = checks::review_check(
        &mut f.book,
        f.actor,
        f.co,
        draft["id"].as_str().unwrap(),
        draft["revision"].as_i64(),
    )
    .unwrap();
    let posted = checks::record_check(
        &mut f.book,
        f.actor,
        f.co,
        draft["id"].as_str().unwrap(),
        draft["revision"].as_i64(),
        &review,
    )
    .unwrap();
    checks::void_check(
        &mut f.book,
        f.actor,
        f.co,
        posted["id"].as_str().unwrap(),
        posted["revision"].as_i64(),
        &json!("2026-11-05"),
        &json!("Synthetic correction"),
    )
    .unwrap();
    assert!(
        f.set("CHECK", Some(f.bank), "0007")
            .unwrap_err()
            .message()
            .contains(draft["number"].as_str().unwrap())
    );
    assert_eq!(f.row("CHECK", Some(f.bank))["next"], "0008");
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn invalid_values_stale_forms_and_unauthorized_changes_leave_no_audit_or_setting() {
    let mut f = Fx::new();
    for text in [
        "",
        "0",
        "-1",
        "+1",
        "1.0",
        "1e3",
        " 7 ",
        "INV-7",
        "١",
        "é",
        "1\0",
        "9999999999999",
    ] {
        assert!(f.set("INV", None, text).is_err(), "accepted {text:?}");
    }
    assert_eq!(f.audit_count(), 0);
    f.identity("INV");
    let stale = numbering::set_next(
        &mut f.book,
        f.actor,
        f.co,
        Change {
            kind: "INV",
            bank: None,
            next: "100",
            expected_next: "1",
            expected_revision: 0,
        },
    )
    .unwrap_err();
    assert!(stale.message().contains("changed since"));
    let user = users::create_user(&mut f.book, "Regular operator", false).unwrap();
    users::add_member(&mut f.book, f.co, user).unwrap();
    assert!(
        numbering::set_next(
            &mut f.book,
            user,
            f.co,
            Change {
                kind: "INV",
                bank: None,
                next: "100",
                expected_next: "2",
                expected_revision: 0
            }
        )
        .is_err()
    );
    assert_eq!(f.audit_count(), 0);
}

#[test]
fn audit_records_old_new_actor_time_and_reopen_retains_the_start() {
    let mut f = Fx::new();
    f.set("INV", None, "100").unwrap();
    f.set("INV", None, "200").unwrap();
    let (actor,name,stamp,details):(String,String,String,String)=f.book.read(|q|q.query_row("SELECT actor_id,actor_name,occurred_at,detail FROM ledger_auditevent WHERE action='numbering.changed' ORDER BY id DESC LIMIT 1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))).unwrap();
    assert_eq!(actor, f.actor.to_db());
    assert_eq!(name, "Numbering owner");
    assert!(chrono::DateTime::parse_from_rfc3339(&stamp).is_ok());
    let details: Value = serde_json::from_str(&details).unwrap();
    assert_eq!(details["old_next"], "100");
    assert_eq!(details["new_next"], "200");
    assert_eq!(details["old_start"], "100");
    assert_eq!(details["revision"], 2);
    assert!(f.book.check().unwrap().is_empty());
    let Fx {
        _dir,
        book,
        actor,
        co,
        ..
    } = f;
    drop(book);
    let mut reopened = Book::open(&_dir.path().join("numbers.fbbook"), &[42; 32]).unwrap();
    let doc = reopened
        .write(|q| documents::create_identity(q, actor, co, "INV", &fb_ids::new_v7().to_string()))
        .unwrap();
    assert_eq!(doc["number"], "INV-000200");
}

#[test]
fn upper_bounds_exhaust_without_overflow_and_can_be_reset_into_a_gap() {
    let mut f = Fx::new();
    f.set("INV", None, "999999999999").unwrap();
    assert_eq!(f.identity("INV")["number"], "INV-999999999999");
    assert_eq!(f.row("INV", None)["next"], "");
    f.set("INV", None, "5").unwrap();
    assert_eq!(f.identity("INV")["number"], "INV-000005");
    f.set("CHECK", Some(f.bank), "99999999999999999999")
        .unwrap();
    f.check("99999999999999999999");
    assert_eq!(f.row("CHECK", Some(f.bank))["next"], "");
    f.set("CHECK", Some(f.bank), "8").unwrap();
    assert_eq!(f.row("CHECK", Some(f.bank))["next"], "8");
}

#[test]
fn sql_guards_require_audit_and_preserve_setting_and_bank_identities() {
    let mut f = Fx::new();
    f.set("CHECK", Some(f.bank), "100").unwrap();
    let e = f
        .book
        .write(|q| {
            q.execute(
                "UPDATE fb_numbering SET start='200',revision=revision+1",
                [],
            )
        })
        .unwrap_err();
    assert!(e.to_string().contains("numbering_revision"));
    assert!(
        f.book
            .write(|q| q.execute("DELETE FROM fb_numbering", []))
            .unwrap_err()
            .to_string()
            .contains("numbering_delete")
    );
    assert!(
        f.book
            .write(|q| q.execute("DELETE FROM ledger_account WHERE id=?1", [f.bank.to_db()]))
            .is_err()
    );
    let e = f
        .book
        .write(|q| {
            q.execute(
                "INSERT OR REPLACE INTO fb_numbering SELECT * FROM fb_numbering",
                [],
            )
        })
        .unwrap_err();
    assert!(e.to_string().contains("numbering_key"));
    let before = f.row("CHECK", Some(f.bank));
    let audits = f.audit_count();
    {
        let connection = f.book.connection();
        for recursion in [true, false] {
            connection
                .execute_batch(&format!("PRAGMA recursive_triggers={}", i32::from(recursion)))
                .unwrap();
            for operation in ["UPDATE", "UPDATE OR REPLACE"] {
                for (column, value) in [
                    ("company_id", fb_ids::new_v7().to_string()),
                    ("kind", "INV".to_owned()),
                    ("scope", f.other_bank.to_db()),
                ] {
                    let sql = format!(
                        "{operation} fb_numbering SET {column}=?1 WHERE company_id=?2 AND kind='CHECK' AND scope=?3"
                    );
                    let error = connection
                        .execute(&sql, params![value, f.co.to_db(), f.bank.to_db()])
                        .unwrap_err();
                    assert!(fb_db::is_constraint(&error), "{sql}: {error}");
                    assert!(error.to_string().contains("numbering_identity"), "{sql}: {error}");
                }
            }
            for (sql, message) in [
                ("DELETE FROM fb_numbering", "numbering_delete"),
                ("INSERT OR REPLACE INTO fb_numbering SELECT * FROM fb_numbering", "numbering_key"),
            ] {
                let error = connection.execute(sql, []).unwrap_err();
                assert!(fb_db::is_constraint(&error), "{sql}: {error}");
                assert!(error.to_string().contains(message), "{sql}: {error}");
            }
        }
        connection.execute_batch("PRAGMA recursive_triggers=ON").unwrap();
    }
    assert_eq!(f.row("CHECK", Some(f.bank)), before);
    assert_eq!(f.audit_count(), audits);
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn failed_outer_transaction_rolls_back_the_number_and_document_together_and_retry_does_not_consume()
{
    let mut f = Fx::new();
    f.set("INV", None, "500").unwrap();
    let key = fb_ids::new_v7().to_string();
    let refused: fb_core::FbResult<()> = f.book.write(|q| {
        documents::create_identity(q, f.actor, f.co, "INV", &key)?;
        Err(fb_core::FbError::validation("refused"))
    });
    assert!(refused.is_err());
    assert_eq!(f.row("INV", None)["next"], "500");
    let doc = f
        .book
        .write(|q| documents::create_identity(q, f.actor, f.co, "INV", &key))
        .unwrap();
    let retry = f
        .book
        .write(|q| documents::create_identity(q, f.actor, f.co, "INV", &key))
        .unwrap();
    assert_eq!(doc, retry);
    assert_eq!(f.row("INV", None)["next"], "501");
    let count:i64=f.book.read(|q|q.query_row("SELECT count(*) FROM documents_documentidentity WHERE company_id=?1 AND kind='INV'",params![f.co.to_db()],|r|r.get(0))).unwrap();
    assert_eq!(count, 1);
}

#[test]
fn concurrent_settings_and_allocations_serialize_and_stale_setter_is_refused() {
    use std::sync::{Arc, Barrier, Mutex};
    let f = Fx::new();
    let actor = f.actor;
    let co = f.co;
    let shared = Arc::new(Mutex::new(f));
    let barrier = Arc::new(Barrier::new(2));
    let handles: Vec<_> = ["100", "200"]
        .into_iter()
        .map(|next| {
            let shared = shared.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                let mut f = shared.lock().unwrap();
                numbering::set_next(
                    &mut f.book,
                    actor,
                    co,
                    Change {
                        kind: "INV",
                        bank: None,
                        next,
                        expected_next: "1",
                        expected_revision: 0,
                    },
                )
            })
        })
        .collect();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert!(
        results
            .iter()
            .find_map(|r| r.as_ref().err())
            .unwrap()
            .message()
            .contains("changed since")
    );
    let barrier = Arc::new(Barrier::new(2));
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let shared = shared.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                shared.lock().unwrap().identity("INV")
            })
        })
        .collect();
    let docs: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_ne!(docs[0]["number"], docs[1]["number"]);
    let mut f = shared.lock().unwrap();
    assert_eq!(f.audit_count(), 1);
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn bill_payments_and_both_refund_families_reserve_and_advance_the_same_check_start() {
    use fb_core::{
        credits,
        directory::{self, Model},
        payables, prepayments,
    };
    let mut f = Fx::new();
    let ap = services::save_account(
        &mut f.book,
        f.actor,
        f.co,
        AccountInput::new("200", "Payables").kind("LIABILITY"),
    )
    .unwrap()
    .id;
    let ar = services::save_account(
        &mut f.book,
        f.actor,
        f.co,
        AccountInput::new("110", "Receivables").kind("ASSET"),
    )
    .unwrap()
    .id;
    let income = services::save_account(
        &mut f.book,
        f.actor,
        f.co,
        AccountInput::new("400", "Income").kind("INCOME"),
    )
    .unwrap()
    .id;
    let mut account = AccountInput::new("230", "Deposits");
    account.kind = Some("LIABILITY".into());
    account.detail_type = Some("OTHER_CURRENT_LIABILITY".into());
    let liability = services::save_account(&mut f.book, f.actor, f.co, account)
        .unwrap()
        .id;
    documents::save_setup(&mut f.book,f.actor,f.co,&json!({"receivables":ar.to_db(),"payables":ap.to_db(),"customer_deposits":liability.to_db(),"banks":[f.bank.to_db(),f.other_bank.to_db()]}),Some(1)).unwrap();
    let customer = directory::save_record(
        &mut f.book,
        f.actor,
        f.co,
        Model::Customer,
        &json!({"display_name":"Synthetic customer"}),
        None,
    )
    .unwrap();
    let vendor = directory::save_record(
        &mut f.book,
        f.actor,
        f.co,
        Model::Vendor,
        &json!({"display_name":"Synthetic vendor"}),
        None,
    )
    .unwrap();
    f.set("CHECK", Some(f.bank), "1200").unwrap();
    let payload = json!({"vendor_id":vendor["id"],"date":"2026-11-05","due_date":"2026-12-05","request_key":fb_ids::new_v7().to_string(),"lines":[{"description":"Cost","quantity":"1","rate":"10","expense_account_id":f.expense.to_db()}]});
    let bill = payables::save_bill(&mut f.book, f.actor, f.co, &payload).unwrap();
    let review = payables::review_bill(
        &mut f.book,
        f.actor,
        f.co,
        bill["id"].as_str().unwrap(),
        Some(1),
    )
    .unwrap();
    let bill = payables::post_bill(
        &mut f.book,
        f.actor,
        f.co,
        bill["id"].as_str().unwrap(),
        Some(1),
        &review,
    )
    .unwrap();
    let payload = json!({"vendor_id":vendor["id"],"bank_id":f.bank.to_db(),"date":"2026-11-05","amount":"1","method":"CHECK","reference":"1200","request_key":fb_ids::new_v7().to_string(),"allocations":[{"bill_id":bill["id"],"amount":"1"}]});
    let review = checks::review_bill_payment(&mut f.book, f.actor, f.co, &payload).unwrap();
    let payment =
        checks::record_bill_payment(&mut f.book, f.actor, f.co, &payload, &review).unwrap();
    assert_eq!(f.row("CHECK", Some(f.bank))["next"], "1201");
    assert!(
        f.set("CHECK", Some(f.bank), "1200")
            .unwrap_err()
            .message()
            .contains("BPAY-")
    );
    payables::reverse_payment(
        &mut f.book,
        f.actor,
        f.co,
        payment["id"].as_str().unwrap(),
        Some(1),
        &json!("2026-11-05"),
        &json!("Synthetic correction"),
    )
    .unwrap();
    assert!(
        f.set("CHECK", Some(f.bank), "1200")
            .unwrap_err()
            .message()
            .contains("BPAY-")
    );

    let payload = json!({"request_key":fb_ids::new_v7().to_string(),"customer_id":customer["id"],"date":"2026-11-05","memo":"Returned service","tax_mode":"NONE","lines":[{"item_id":null,"description":"Work","quantity":"1","rate":"10.00","taxable":false,"income_account_id":income.to_db()}]});
    let review = credits::review_credit_memo(&mut f.book, f.actor, f.co, &payload).unwrap();
    let memo =
        credits::record_credit_memo(&mut f.book, f.actor, f.co, &payload, Some(&review)).unwrap();
    let payload = json!({"request_key":fb_ids::new_v7().to_string(),"date":"2026-11-05","amount":"1.00","bank_id":f.bank.to_db(),"method":"CHECK","check_number":"1201","memo":"Synthetic refund"});
    let review = credits::review_refund(
        &mut f.book,
        f.actor,
        f.co,
        memo["id"].as_str().unwrap(),
        &payload,
    )
    .unwrap();
    credits::record_refund(
        &mut f.book,
        f.actor,
        f.co,
        memo["id"].as_str().unwrap(),
        &payload,
        Some(&review),
    )
    .unwrap();
    assert_eq!(f.row("CHECK", Some(f.bank))["next"], "1202");
    assert!(
        f.set("CHECK", Some(f.bank), "1201")
            .unwrap_err()
            .message()
            .contains("RFD-")
    );

    let payload = json!({"request_key":fb_ids::new_v7().to_string(),"customer_id":customer["id"],"date":"2026-11-05","amount":"10.00","method":"CHECK","destination":"BANK","bank_id":f.bank.to_db()});
    let review = prepayments::review_deposit(&mut f.book, f.actor, f.co, &payload).unwrap();
    let deposit =
        prepayments::record_deposit(&mut f.book, f.actor, f.co, Some(&review), &payload).unwrap();
    let payload = json!({"request_key":fb_ids::new_v7().to_string(),"date":"2026-11-05","amount":"1.00","bank_id":f.bank.to_db(),"method":"CHECK","check_number":"1202"});
    let review = prepayments::review_refund(
        &mut f.book,
        f.actor,
        f.co,
        deposit["id"].as_str().unwrap(),
        &payload,
    )
    .unwrap();
    prepayments::record_refund(
        &mut f.book,
        f.actor,
        f.co,
        deposit["id"].as_str().unwrap(),
        Some(&review),
        &payload,
    )
    .unwrap();
    assert_eq!(f.row("CHECK", Some(f.bank))["next"], "1203");
    assert!(
        f.set("CHECK", Some(f.bank), "1202")
            .unwrap_err()
            .message()
            .contains("CDR-")
    );
    assert_eq!(f.row("CHECK", Some(f.other_bank))["next"], "1");
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn editing_a_check_number_does_not_rewind_the_configured_next_number() {
    let mut f = Fx::new();
    f.set("CHECK", Some(f.bank), "100").unwrap();
    let check = f.check("100");
    assert_eq!(f.row("CHECK", Some(f.bank))["next"], "101");
    let payload = json!({"check_id":check["id"],"expected_revision":check["revision"],"bank_id":f.bank.to_db(),"payee_kind":"OTHER","payee_name":"Synthetic supplier","date":"2026-11-05","check_number":"101","memo":"Synthetic numbering","request_key":fb_ids::new_v7().to_string(),"lines":[{"tab":"EXPENSE","description":"Supplies","rate":"10.00","account_id":f.expense.to_db()}]});
    checks::save_check(&mut f.book, f.actor, f.co, &payload).unwrap();
    assert_eq!(f.row("CHECK", Some(f.bank))["next"], "102");
    assert!(
        f.set("CHECK", Some(f.bank), "100")
            .unwrap_err()
            .message()
            .contains(check["number"].as_str().unwrap())
    );
    assert!(f.book.check().unwrap().is_empty());
}
