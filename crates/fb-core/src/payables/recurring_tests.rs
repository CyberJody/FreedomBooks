use super::*;
use crate::payables::recurring as r;
use fb_db::{SnapshotSource, rusqlite};
fn template(f: &Fx) -> Value {
    json!({"id":null,"request_key":fb_ids::new_v7().to_string(),"expected_revision":0,"name":"Synthetic monthly rent","vendor_id":f.vendor,"memo":"Recurring synthetic expense","terms_days":15,"frequency":"MONTHLY","start_date":"2024-01-31","end_date":null,"occurrence_limit":null,"active":true,"auto_post":false,"lines":[{"item_id":null,"expense_account_id":f.expense.to_db(),"description":"Rent","quantity":"2","rate":"12.34"}]})
}
fn save(f: &mut Fx, p: &Value) -> Value {
    r::save(&mut f.book, f.actor, f.co, p, "2024-01-01").unwrap()
}
fn run(f: &mut Fx, dt: &str) -> Value {
    r::generate_due(&mut f.book, f.actor, f.co, dt).unwrap()
}
fn detail(f: &mut Fx, t: &Value) -> Value {
    r::detail(&mut f.book, f.actor, f.co, s(t, "id")).unwrap()
}
#[test]
fn anchored_month_ends_and_leap_years() {
    for (start, frequency, index, want) in [
        ("2024-01-31", "MONTHLY", 1, "2024-02-29"),
        ("2024-01-31", "MONTHLY", 2, "2024-03-31"),
        ("2025-01-30", "MONTHLY", 1, "2025-02-28"),
        ("2025-01-30", "MONTHLY", 2, "2025-03-30"),
        ("2025-01-29", "MONTHLY", 1, "2025-02-28"),
        ("2025-12-15", "MONTHLY", 1, "2026-01-15"),
        ("2024-02-29", "YEARLY", 1, "2025-02-28"),
        ("2024-02-29", "YEARLY", 4, "2028-02-29"),
        ("2096-02-29", "YEARLY", 4, "2100-02-28"),
        ("2000-02-29", "MONTHLY", 0, "2000-02-29"),
        ("2026-01-01", "MONTHLY", 1, "2026-02-01"),
    ] {
        assert_eq!(
            r::occurrence(start, frequency, index).unwrap().as_deref(),
            Some(want)
        );
    }
    assert_eq!(r::occurrence("9999-12-31", "MONTHLY", 1).unwrap(), None);
    assert_eq!(
        r::occurrence("2026-01-31", "YEARLY", i64::MAX).unwrap(),
        None
    );
    assert!(r::occurrence("2026-02-30", "MONTHLY", 0).is_err());
    assert!(r::occurrence("2026-01-31", "DAILY", 1).is_err());
}
#[test]
fn catch_up_drafts_are_exact_and_idempotent_on_reopen() {
    let mut f = Fx::new();
    let p = template(&f);
    let t = save(&mut f, &p);
    assert_eq!(
        run(&mut f, "2024-01-30")["created"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    let out = run(&mut f, "2024-03-31");
    assert_eq!(out["created"].as_array().unwrap().len(), 3);
    for b in out["created"].as_array().unwrap() {
        assert_eq!(b["state"], "DRAFT");
        assert_eq!(b["current"]["total"], "24.68");
    }
    assert_eq!(out["created"][1]["current"]["date"], "2024-02-29");
    assert_eq!(out["created"][1]["current"]["due_date"], "2024-03-15");
    let counts = f.counts();
    assert_eq!(counts["ledger_journal"], 0);
    assert_eq!(run(&mut f, "2024-03-31")["created"], json!([]));
    assert_eq!(f.counts(), counts);
    let path = std::path::PathBuf::from(f.book.connection().path().unwrap());
    let mut reopened = Book::open(&path, &[42; 32]).unwrap();
    assert_eq!(
        r::generate_due(&mut reopened, f.actor, f.co, "2024-03-31").unwrap()["created"],
        json!([])
    );
    assert_eq!(detail(&mut f, &t)["next_date"], "2024-04-30");
    assert!(f.book.check().unwrap().is_empty());
}
#[test]
fn inclusive_end_and_count_stop_the_schedule() {
    let mut f = Fx::new();
    let mut p = template(&f);
    p["end_date"] = json!("2024-02-29");
    let t = save(&mut f, &p);
    assert_eq!(
        run(&mut f, "2025-12-31")["created"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(detail(&mut f, &t)["next_date"].is_null());
    p["request_key"] = json!(fb_ids::new_v7().to_string());
    p["end_date"] = Value::Null;
    p["occurrence_limit"] = json!(1);
    let t = save(&mut f, &p);
    assert_eq!(
        run(&mut f, "2025-12-31")["created"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(detail(&mut f, &t)["next_date"].is_null());
    p["request_key"] = json!(fb_ids::new_v7().to_string());
    p["frequency"] = json!("YEARLY");
    p["start_date"] = json!("2024-02-29");
    p["end_date"] = json!("2025-02-28");
    p["occurrence_limit"] = json!(9);
    save(&mut f, &p);
    let out = run(&mut f, "2030-01-01");
    assert_eq!(out["created"].as_array().unwrap().len(), 2);
    assert_eq!(out["created"][1]["current"]["date"], "2025-02-28");
}
#[test]
fn simultaneous_triggers_create_each_occurrence_once() {
    let mut f = Fx::new();
    let mut p = template(&f);
    p["occurrence_limit"] = json!(2);
    save(&mut f, &p);
    let path = std::path::PathBuf::from(f.book.connection().path().unwrap());
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let mut threads = vec![];
    for mut book in [
        Book::open(&path, &[42; 32]).unwrap(),
        Book::open(&path, &[42; 32]).unwrap(),
    ] {
        let barrier = barrier.clone();
        let (actor, co) = (f.actor, f.co);
        threads.push(std::thread::spawn(move || {
            barrier.wait();
            r::generate_due(&mut book, actor, co, "2025-01-01").unwrap()
        }));
    }
    let count: usize = threads
        .into_iter()
        .map(|t| t.join().unwrap()["created"].as_array().unwrap().len())
        .sum();
    assert_eq!(count, 2);
    assert_eq!(f.counts()["payables_bill"], 2);
    assert!(f.book.check().unwrap().is_empty());
}
#[test]
fn auto_post_uses_normal_ledger_payments_and_actor_audit() {
    let mut f = Fx::new();
    let mut p = template(&f);
    p["auto_post"] = json!(true);
    p["occurrence_limit"] = json!(1);
    let t = save(&mut f, &p);
    let out = run(&mut f, "2024-01-31");
    assert_eq!(out["errors"], json!([]));
    let b = &out["created"][0];
    assert_eq!(b["state"], "OPEN");
    assert_eq!(f.counts()["ledger_journal"], 1);
    let payment = f.payment(b, "24.68");
    f.pay(&payment);
    let balance = current_bills(&mut f.book, f.actor, f.co).unwrap();
    assert_eq!(balance[0]["outstanding"], "0.00");
    let events = rows(
        f.book.connection(),
        "SELECT actor_id,action,detail FROM ledger_auditevent WHERE action LIKE 'recurring_bill.%'",
        &[],
    )
    .unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[1]["actor_id"], f.actor.to_db());
    let audit: Value = serde_json::from_str(s(&events[1], "detail")).unwrap();
    assert_eq!(audit["template_id"], t["id"]);
    assert_eq!(audit["automatic"], true);
    assert_eq!(audit["auto_post"], true);
    assert!(f.book.check().unwrap().is_empty());
}
#[test]
fn failed_autopost_rolls_back_bill_number_audit_and_cursor_and_retry_succeeds() {
    let mut f = Fx::new();
    let mut p = template(&f);
    p["auto_post"] = json!(true);
    p["occurrence_limit"] = json!(1);
    let t = save(&mut f, &p);
    // Archive the vendor after scheduling; the normal bill engine must refuse it.
    // The directory's active control is independent of its edit payload.
    f.book
        .write(|tx| -> FbResult<()> {
            tx.execute(
                "UPDATE directory_vendor SET active=0 WHERE id=?",
                [&f.vendor],
            )?;
            Ok(())
        })
        .unwrap();
    let before = f.counts();
    let out = run(&mut f, "2024-01-31");
    assert_eq!(out["created"], json!([]));
    assert_eq!(out["errors"].as_array().unwrap().len(), 1);
    assert_eq!(f.counts(), before);
    assert_eq!(detail(&mut f, &t)["next_index"], 0);
    f.book
        .write(|tx| -> FbResult<()> {
            tx.execute(
                "UPDATE directory_vendor SET active=1 WHERE id=?",
                [&f.vendor],
            )?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        run(&mut f, "2024-01-31")["created"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(run(&mut f, "2024-01-31")["created"], json!([]));
}
#[test]
fn pause_resume_edit_and_delete_preserve_created_bills() {
    let mut f = Fx::new();
    let mut p = template(&f);
    let t = save(&mut f, &p);
    r::control(&mut f.book, f.actor, f.co, s(&t, "id"), &json!({"expected_revision":1,"action":"pause","confirm_delete":true,"confirmation":{"dates":["2024-01-31","2024-02-29"],"auto_post":false}}), "2024-02-29").unwrap();
    assert_eq!(run(&mut f, "2024-03-31")["created"], json!([]));
    let paused = detail(&mut f, &t);
    let confirmation = r::due_confirmation(&paused, "2024-02-29").unwrap()["token"].clone();
    r::control(
        &mut f.book,
        f.actor,
        f.co,
        s(&t, "id"),
        &json!({"expected_revision":2,"action":"resume_create","confirmation":confirmation}),
        "2024-02-29",
    )
    .unwrap();
    assert_eq!(
        run(&mut f, "2024-02-29")["created"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    p["id"] = t["id"].clone();
    p["expected_revision"] = json!(3);
    p["lines"][0]["rate"] = json!("20.00");
    save(&mut f, &p);
    assert_eq!(
        run(&mut f, "2024-03-31")["created"][0]["current"]["total"],
        "40.00"
    );
    assert_eq!(
        current_bills(&mut f.book, f.actor, f.co).unwrap()[0]["current"]["total"],
        "24.68"
    );
    p["expected_revision"] = json!(4);
    p["start_date"] = json!("2024-01-30");
    assert!(r::save(&mut f.book, f.actor, f.co, &p, "2024-01-01").is_err());
    assert!(r::control(&mut f.book, f.actor, f.co, s(&t, "id"), &json!({"expected_revision":3,"action":"delete","confirm_delete":true,"confirmation":{"dates":["2024-01-31","2024-02-29"],"auto_post":false}}), "2024-02-29").is_err());
    r::control(&mut f.book, f.actor, f.co, s(&t, "id"), &json!({"expected_revision":4,"action":"delete","confirm_delete":true,"confirmation":{"dates":["2024-01-31","2024-02-29"],"auto_post":false}}), "2024-02-29").unwrap();
    assert_eq!(run(&mut f, "2025-01-01")["created"], json!([]));
    assert_eq!(
        detail(&mut f, &t)["occurrences"].as_array().unwrap().len(),
        3
    );
    assert_eq!(f.counts()["payables_bill"], 3);
    assert!(r::list(&mut f.book, f.actor, f.co).unwrap().is_empty());
    assert!(f.book.check().unwrap().is_empty());
}
#[test]
fn permissions_and_company_references_are_rechecked() {
    let mut f = Fx::new();
    let p = template(&f);
    let t = save(&mut f, &p);
    let other = users::create_user(&mut f.book, "outsider", false).unwrap();
    assert!(r::save(&mut f.book, other, f.co, &p, "2024-01-01").is_err());
    assert!(r::generate_due(&mut f.book, other, f.co, "2025-01-01").is_err());
    assert!(r::detail(&mut f.book, other, f.co, s(&t, "id")).is_err());
    let co = services::create_company(&mut f.book, f.actor, "Other synthetic", "USD", false)
        .unwrap()
        .id;
    assert!(r::save(&mut f.book, f.actor, co, &p, "2024-01-01").is_err());
    assert!(r::detail(&mut f.book, f.actor, co, s(&t, "id")).is_err());
    assert!(r::control(&mut f.book, f.actor, co, s(&t, "id"), &json!({"expected_revision":1,"action":"delete","confirm_delete":true,"confirmation":{"dates":["2024-01-31","2024-02-29"],"auto_post":false}}), "2024-02-29").is_err());
    f.book.write(|tx|->FbResult<()>{tx.execute("INSERT INTO desktop_user(user_id,role,password_hash) VALUES(?,'readonly',NULL) ON CONFLICT(user_id) DO UPDATE SET role='readonly'",[f.actor.to_db()])?;Ok(())}).unwrap();
    assert!(r::save(&mut f.book, f.actor, f.co, &p, "2024-01-01").is_err());
    assert!(r::generate_due(&mut f.book, f.actor, f.co, "2025-01-01").is_err());
    assert!(r::control(&mut f.book, f.actor, f.co, s(&t, "id"), &json!({"expected_revision":1,"action":"pause","confirm_delete":true,"confirmation":{"dates":["2024-01-31","2024-02-29"],"auto_post":false}}), "2024-02-29").is_err());
    assert!(r::list(&mut f.book, f.actor, f.co).is_ok());
}
#[test]
fn make_recurring_copies_lines_but_not_supplier_reference_or_history() {
    let mut f = Fx::new();
    let p = f.payload();
    let b = f.save(&p);
    let copy = r::from_bill(&mut f.book, f.actor, f.co, s(&b, "id"), "2026-11-15").unwrap();
    assert_eq!(copy["vendor_id"], p["vendor_id"]);
    assert_eq!(copy["lines"][0]["quantity"], "1.0050");
    assert!(copy.get("reference").is_none());
    assert!(copy.get("bill_id").is_none());
}
#[test]
fn format_90_populated_book_upgrades_and_reopens_without_changing_existing_records() {
    let mut f = Fx::new();
    let p = f.payload();
    let b = f.save(&p);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("format90.fbbook");
    let copy = fb_db::test_support::populated_copy(
        f.book.connection(),
        &path,
        &[42; 32],
        "0203_salestax_payment_evidence",
        true,
    )
    .unwrap();
    let old = match fb_db::Book::open_staged(&path, &[42; 32]).unwrap() {
        fb_db::Opened::Behind(old) => old,
        _ => panic!("format 90 must require upgrade"),
    };
    assert_eq!(old.from(), 90);
    assert_eq!(old.to(), fb_db::MIGRATIONS.len());
    assert_eq!(fb_db::MIGRATIONS[90].0, "0204_recurring_bills");
    old.snapshot(&dir.path().join("verified-format90.fbbook"), &[42; 32])
        .unwrap();
    let mut upgraded = old.upgrade().unwrap();
    let now = fb_db::test_support::snapshot(upgraded.connection()).unwrap();
    for (table, rows) in copy.expected {
        assert_eq!(now[&table], rows, "{table}");
    }
    assert_eq!(
        get_bill(&mut upgraded, f.actor, f.co, s(&b, "id")).unwrap(),
        b
    );
    assert!(upgraded.check().unwrap().is_empty());
    drop(upgraded);
    assert!(
        Book::open(&path, &[42; 32])
            .unwrap()
            .check()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn interruption_after_posting_rolls_back_every_row_and_retry_creates_one_bill() {
    let mut f = Fx::new();
    let mut p = template(&f);
    p["auto_post"] = json!(true);
    p["occurrence_limit"] = json!(1);
    let t = save(&mut f, &p);
    f.book.connection().execute_batch("CREATE TEMP TRIGGER recurring_injected_failure BEFORE INSERT ON payables_recurring_occurrence BEGIN SELECT RAISE(ABORT,'synthetic interruption after posting'); END;").unwrap();
    let before = fb_db::test_support::snapshot(f.book.connection()).unwrap();
    let out = run(&mut f, "2024-01-31");
    assert_eq!(out["created"], json!([]));
    assert_eq!(out["errors"].as_array().unwrap().len(), 1);
    assert_eq!(
        fb_db::test_support::snapshot(f.book.connection()).unwrap(),
        before,
        "bill, journal, audit and number counters must all roll back"
    );
    f.book
        .connection()
        .execute_batch("DROP TRIGGER recurring_injected_failure")
        .unwrap();
    let out = run(&mut f, "2024-01-31");
    assert_eq!(out["created"].as_array().unwrap().len(), 1);
    assert_eq!(out["created"][0]["state"], "OPEN");
    assert_eq!(detail(&mut f, &t)["next_index"], 1);
    assert_eq!(f.counts()["ledger_journal"], 1);
    assert!(f.book.check().unwrap().is_empty());
}
#[test]
fn sql_guards_reject_cross_company_content_bad_arithmetic_and_occurrence_replacement() {
    let mut f = Fx::new();
    let p = template(&f);
    let t = save(&mut f, &p);
    let tid = s(&t, "id");
    for recursion in [true, false] {
        f.book
            .connection()
            .execute_batch(&format!(
                "PRAGMA recursive_triggers={}",
                i32::from(recursion)
            ))
            .unwrap();
        assert!(f.book.connection().execute("INSERT OR REPLACE INTO payables_recurring SELECT * FROM payables_recurring WHERE id=?",[tid]).is_err());
    }
    f.book
        .connection()
        .execute_batch("PRAGMA recursive_triggers=ON")
        .unwrap();
    let other = services::create_company(
        &mut f.book,
        f.actor,
        "Other SQL guard company",
        "USD",
        false,
    )
    .unwrap()
    .id;
    let expense = services::save_account(
        &mut f.book,
        f.actor,
        other,
        AccountInput::new("500", "Other expense").kind("EXPENSE"),
    )
    .unwrap()
    .id;
    let mut data = t["data"].clone();
    data["lines"][0]["expense_account_id"] = json!(expense.to_db());
    assert!(
        f.book
            .connection()
            .execute(
                "UPDATE payables_recurring SET revision=revision+1,data=? WHERE id=?",
                params![data.to_string(), tid]
            )
            .is_err()
    );
    data = t["data"].clone();
    data["lines"][0]["quantity"] = json!("not a decimal");
    assert!(
        f.book
            .connection()
            .execute(
                "UPDATE payables_recurring SET revision=revision+1,data=? WHERE id=?",
                params![data.to_string(), tid]
            )
            .is_err()
    );
    run(&mut f, "2024-01-31");
    let b = detail(&mut f, &t)["occurrences"][0]["bill_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        f.book
            .connection()
            .execute(
                "INSERT OR REPLACE INTO payables_recurring_occurrence(company_id,template_id,occurrence_index,occurrence_date,bill_id) VALUES(?,?,?,?,?)",
                params![f.co.to_db(), tid, 0, "2024-01-31", b]
            )
            .is_err()
    );
    assert!(f.book.connection().execute("UPDATE payables_recurring_occurrence SET occurrence_date='2024-02-01' WHERE bill_id=?",[&b]).is_err());
    assert!(
        f.book
            .connection()
            .execute(
                "DELETE FROM payables_recurring_occurrence WHERE bill_id=?",
                [&b]
            )
            .is_err()
    );
    let mut second = template(&f);
    second["active"] = json!(false);
    let second = save(&mut f, &second);
    let confirmation = r::due_confirmation(&second, "2024-02-29").unwrap()["token"].clone();
    r::control(
        &mut f.book,
        f.actor,
        f.co,
        s(&second, "id"),
        &json!({"expected_revision":1,"action":"resume_create","confirmation":confirmation}),
        "2024-02-29",
    )
    .unwrap();
    for recursion in [true, false] {
        f.book
            .connection()
            .execute_batch(&format!(
                "PRAGMA recursive_triggers={}",
                i32::from(recursion)
            ))
            .unwrap();
        assert!(
            f.book
                .connection()
                .execute(
                    "INSERT OR REPLACE INTO payables_recurring_occurrence(company_id,template_id,occurrence_index,occurrence_date,bill_id) VALUES(?,?,?,?,?)",
                    params![f.co.to_db(), s(&second, "id"), 0, "2024-01-31", b]
                )
                .is_err(),
            "a fresh template cannot displace an existing bill's occurrence with recursion disabled"
        );
    }
    f.book
        .connection()
        .execute_batch("PRAGMA recursive_triggers=ON")
        .unwrap();
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn save_requires_exact_confirmation_for_past_dates_including_edits_and_autopost() {
    let mut f = Fx::new();
    let mut p = template(&f);
    p["auto_post"] = json!(true);
    let before = f.counts();
    assert!(r::save(&mut f.book, f.actor, f.co, &p, "2024-03-31").is_err());
    assert_eq!(f.counts(), before);
    let preview = r::save_confirmation(&mut f.book, f.actor, f.co, &p, "2024-03-31").unwrap();
    assert_eq!(
        preview["dates"],
        json!(["2024-01-31", "2024-02-29", "2024-03-31"])
    );
    assert_eq!(preview["auto_post"], true);
    p["confirmation"] = preview["token"].clone();
    let t = r::save(&mut f.book, f.actor, f.co, &p, "2024-03-31").unwrap();
    assert_eq!(
        f.counts()["payables_bill"],
        0,
        "saving itself must not generate bills"
    );
    p["id"] = t["id"].clone();
    p["expected_revision"] = json!(1);
    p["start_date"] = json!("2023-12-31");
    assert!(
        r::save(&mut f.book, f.actor, f.co, &p, "2024-03-31").is_err(),
        "moving dates earlier invalidates confirmation"
    );
    p["confirmation"] = r::save_confirmation(&mut f.book, f.actor, f.co, &p, "2024-03-31").unwrap()
        ["token"]
        .clone();
    p["auto_post"] = json!(false);
    assert!(
        r::save(&mut f.book, f.actor, f.co, &p, "2024-03-31").is_err(),
        "posting choice is part of confirmation"
    );
    p["confirmation"] = r::save_confirmation(&mut f.book, f.actor, f.co, &p, "2024-03-31").unwrap()
        ["token"]
        .clone();
    r::save(&mut f.book, f.actor, f.co, &p, "2024-03-31").unwrap();
    assert_eq!(
        run(&mut f, "2024-03-31")["created"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
}

#[test]
fn source_bill_dates_require_confirmation_only_before_one_full_period() {
    let mut f = Fx::new();
    let mut source = f.payload();
    source["date"] = json!("2026-11-15");
    let b = f.save(&source);
    let copy = r::from_bill(&mut f.book, f.actor, f.co, s(&b, "id"), "2026-11-15").unwrap();
    assert_eq!(copy["start_date"], "2026-12-15");
    for (frequency, first, warning) in [
        ("MONTHLY", "2026-11-20", true),
        ("MONTHLY", "2026-12-01", true),
        ("MONTHLY", "2026-12-15", false),
        ("YEARLY", "2027-11-01", true),
        ("YEARLY", "2027-11-15", false),
    ] {
        let mut p = template(&f);
        p["source_bill_id"] = b["id"].clone();
        p["frequency"] = json!(frequency);
        p["start_date"] = json!(first);
        let preview = r::save_confirmation(&mut f.book, f.actor, f.co, &p, "2026-11-15").unwrap();
        assert_eq!(!preview["source_warning"].is_null(), warning);
        if warning {
            assert_eq!(
                preview["source_warning"],
                json!({"source_date":"2026-11-15","first_date":first})
            );
            assert!(r::save(&mut f.book, f.actor, f.co, &p, "2026-11-15").is_err());
            p["confirmation"] = preview["token"].clone();
        }
        let t = r::save(&mut f.book, f.actor, f.co, &p, "2026-11-15").unwrap();
        assert_eq!(run(&mut f, "2026-11-15")["created"], json!([]));
        p["id"] = t["id"].clone();
        p["expected_revision"] = json!(1);
        for invalid in ["2026-11-15", "2026-11-14"] {
            p["start_date"] = json!(invalid);
            assert!(r::save_confirmation(&mut f.book, f.actor, f.co, &p, "2026-11-15").is_err());
            assert!(r::save(&mut f.book, f.actor, f.co, &p, "2026-11-15").is_err());
            assert!(
                f.book
                    .connection()
                    .execute(
                        "UPDATE payables_recurring SET revision=revision+1,start_date=? WHERE id=?",
                        rusqlite::params![invalid, s(&t, "id")]
                    )
                    .is_err()
            );
        }
    }
    source["date"] = json!("2024-01-31");
    source["request_key"] = json!(fb_ids::new_v7().to_string());
    source["reference"] = json!("End-of-month source");
    let b = f.save(&source);
    assert_eq!(
        r::from_bill(&mut f.book, f.actor, f.co, s(&b, "id"), "2024-01-31").unwrap()["start_date"],
        "2024-02-29"
    );
    let mut p = template(&f);
    p["source_bill_id"] = b["id"].clone();
    p["start_date"] = json!("2024-02-01");
    let preview = r::save_confirmation(&mut f.book, f.actor, f.co, &p, "2024-01-31").unwrap();
    assert!(!preview["source_warning"].is_null());
    p["confirmation"] = preview["token"].clone();
    r::save(&mut f.book, f.actor, f.co, &p, "2024-01-31").unwrap();
    assert_eq!(f.counts()["payables_bill"], 2);
}

#[test]
fn resume_skip_and_create_survive_restart_and_competing_triggers() {
    for skip in [true, false] {
        let mut f = Fx::new();
        let mut p = template(&f);
        p["active"] = json!(false);
        let t = save(&mut f, &p);
        let tid = s(&t, "id").to_owned();
        let mut command = json!({"expected_revision":1,"action":"resume"});
        assert!(r::control(&mut f.book, f.actor, f.co, &tid, &command, "2024-03-31").is_err());
        assert_eq!(detail(&mut f, &t)["active"], 0);
        command["action"] = json!(if skip { "resume_skip" } else { "resume_create" });
        command["confirmation"] = r::due_confirmation(&t, "2024-02-29").unwrap()["token"].clone();
        assert!(r::control(&mut f.book, f.actor, f.co, &tid, &command, "2024-03-31").is_err());
        command["confirmation"] = r::due_confirmation(&t, "2024-03-31").unwrap()["token"].clone();
        let path = std::path::PathBuf::from(f.book.connection().path().unwrap());
        let mut competitor = Book::open(&path, &[42; 32]).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let b2 = barrier.clone();
        let (actor, co) = (f.actor, f.co);
        let trigger = std::thread::spawn(move || {
            b2.wait();
            r::generate_due(&mut competitor, actor, co, "2024-03-31").unwrap()
        });
        barrier.wait();
        r::control(&mut f.book, f.actor, f.co, &tid, &command, "2024-03-31").unwrap();
        trigger.join().unwrap();
        assert!(
            r::control(&mut f.book, f.actor, f.co, &tid, &command, "2024-03-31").is_err(),
            "a repeated decision cannot act again"
        );
        let mut reopened = Book::open(&path, &[42; 32]).unwrap();
        r::generate_due(&mut reopened, actor, co, "2024-03-31").unwrap();
        r::generate_due(&mut reopened, actor, co, "2024-03-31").unwrap();
        let history = r::detail(&mut reopened, actor, co, &tid).unwrap();
        assert_eq!(history["occurrences"].as_array().unwrap().len(), 3);
        for row in history["occurrences"].as_array().unwrap() {
            assert_eq!(row["skipped"], i64::from(skip));
            assert_eq!(row["bill_id"].is_null(), skip);
        }
        assert_eq!(f.counts()["payables_bill"], if skip { 0 } else { 3 });
        assert_eq!(
            r::generate_due(&mut reopened, actor, co, "2024-04-30").unwrap()["created"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(reopened.check().unwrap().is_empty());
        if skip {
            for recursion in ["OFF", "ON"] {
                f.book
                    .connection()
                    .execute_batch(&format!("PRAGMA recursive_triggers={recursion}"))
                    .unwrap();
                assert!(f.book.connection().execute("INSERT OR REPLACE INTO payables_recurring_occurrence SELECT * FROM payables_recurring_occurrence WHERE template_id=? AND occurrence_index=0",[&tid]).is_err());
                assert!(f.book.connection().execute("UPDATE payables_recurring_occurrence SET skipped=0 WHERE template_id=?",[&tid]).is_err());
            }
        }
    }
}

#[test]
fn no_due_work_does_not_open_a_write_transaction() {
    let mut f = Fx::new();
    let mut p = template(&f);
    p["start_date"] = json!("2027-01-01");
    save(&mut f, &p);
    p["request_key"] = json!(fb_ids::new_v7().to_string());
    p["active"] = json!(false);
    p["start_date"] = json!("2024-01-31");
    save(&mut f, &p);
    let commits = f.book.commits();
    for _ in 0..3 {
        assert_eq!(run(&mut f, "2026-11-15")["created"], json!([]));
    }
    assert_eq!(f.book.commits(), commits);
}

#[test]
fn autopost_validation_keeps_one_draft_with_reason_and_no_partial_ledger() {
    let mut f = Fx::new();
    let mut p = template(&f);
    p["auto_post"] = json!(true);
    p["occurrence_limit"] = json!(1);
    save(&mut f, &p);
    f.book
        .write(|tx| -> FbResult<()> {
            tx.execute(
                "UPDATE ledger_account SET active=0 WHERE id=?",
                [f.ap.to_db()],
            )?;
            Ok(())
        })
        .unwrap();
    let out = run(&mut f, "2024-01-31");
    assert_eq!(out["errors"], json!([]));
    assert_eq!(out["created"][0]["state"], "DRAFT");
    assert!(
        out["warnings"][0]["message"]
            .as_str()
            .unwrap()
            .contains("saved as a draft")
    );
    assert_eq!(f.counts()["ledger_journal"], 0);
    assert_eq!(run(&mut f, "2024-01-31")["created"], json!([]));
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn occurrence_open_check_uses_evidence_indexes() {
    let mut f = Fx::new();
    let mut p = template(&f);
    p["occurrence_limit"] = json!(3);
    save(&mut f, &p);
    assert_eq!(
        run(&mut f, "2024-03-31")["created"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    let plan = rows(
        f.book.connection(),
        "EXPLAIN QUERY PLAN SELECT * FROM payables_recurring_invalid",
        &[],
    )
    .unwrap();
    let plan = plan
        .iter()
        .map(|r| s(r, "detail"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        plan.contains("USING INDEX payables_recurring_generated_audit (target=?)"),
        "{plan}"
    );
    assert!(
        plan.contains("USING INDEX payables_recurring_skipped_audit (target=? AND <expr>=?)"),
        "{plan}"
    );
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn confirmation_normalizes_end_dates_and_validates_limits() {
    let mut f = Fx::new();
    let mut p = template(&f);
    p["start_date"] = json!("2026-09-30");
    p["end_date"] = json!("2026-10-30");
    let preview = r::save_confirmation(&mut f.book, f.actor, f.co, &p, "2026-12-31").unwrap();
    assert_eq!(preview["dates"], json!(["2026-09-30", "2026-10-30"]));
    for spelling in ["20261030", "2026-W44-5"] {
        p["end_date"] = json!(spelling);
        let normalized =
            r::save_confirmation(&mut f.book, f.actor, f.co, &p, "2026-12-31").unwrap();
        assert_eq!(normalized["token"], preview["token"]);
    }
    for invalid in [json!(0), json!(120001), json!("2"), json!(1.5)] {
        p["occurrence_limit"] = invalid;
        assert!(r::save_confirmation(&mut f.book, f.actor, f.co, &p, "2026-12-31").is_err());
        assert!(r::save(&mut f.book, f.actor, f.co, &p, "2026-12-31").is_err());
    }
    p["occurrence_limit"] = Value::Null;
    p["confirmation"] = preview["token"].clone();
    let t = r::save(&mut f.book, f.actor, f.co, &p, "2026-12-31").unwrap();
    assert_eq!(t["end_date"], "2026-10-30");
    assert_eq!(
        run(&mut f, "2026-12-31")["created"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn first_bill_date_over_24_months_back_is_refused_when_new_or_changed() {
    assert_eq!(r::MAX_START_MONTHS_BACK, 24);
    let mut f = Fx::new();
    let mut p = template(&f);
    p["active"] = json!(false);
    p["start_date"] = json!("2024-01-30");
    let before = f.counts();
    for refused in [
        r::save_confirmation(&mut f.book, f.actor, f.co, &p, "2026-01-31").unwrap_err(),
        r::save(&mut f.book, f.actor, f.co, &p, "2026-01-31").unwrap_err(),
    ] {
        assert!(
            refused
                .message()
                .contains("cannot be more than 24 months before today"),
            "{}",
            refused.message()
        );
    }
    assert_eq!(f.counts(), before);
    assert!(r::list(&mut f.book, f.actor, f.co).unwrap().is_empty());
    // Exactly 24 months back is the oldest accepted date; active, it lists 25 dates to confirm.
    p["start_date"] = json!("2024-01-31");
    p["active"] = json!(true);
    let preview = r::save_confirmation(&mut f.book, f.actor, f.co, &p, "2026-01-31").unwrap();
    assert_eq!(preview["count"], 25);
    assert_eq!(preview["more"], false);
    p["active"] = json!(false);
    let t = r::save(&mut f.book, f.actor, f.co, &p, "2026-01-31").unwrap();
    // An unchanged stored date stays editable as it ages; a changed one must be recent again.
    p["id"] = t["id"].clone();
    p["expected_revision"] = json!(1);
    p["name"] = json!("Renamed years later");
    r::save(&mut f.book, f.actor, f.co, &p, "2030-06-30").unwrap();
    p["expected_revision"] = json!(2);
    p["start_date"] = json!("2025-01-31");
    assert!(
        r::save(&mut f.book, f.actor, f.co, &p, "2030-06-30")
            .unwrap_err()
            .message()
            .contains("24 months")
    );
    assert_eq!(detail(&mut f, &t)["start_date"], "2024-01-31");
}

#[test]
fn a_backlog_of_exactly_36_dates_is_created_in_one_run() {
    assert_eq!(r::MAX_OCCURRENCES_PER_RUN, 36);
    let mut f = Fx::new();
    let p = template(&f);
    let t = save(&mut f, &p);
    let out = run(&mut f, "2026-12-31");
    assert_eq!(out["errors"], json!([]));
    assert_eq!(out["created"].as_array().unwrap().len(), 36);
    assert_eq!(out["created"][35]["current"]["date"], "2026-12-31");
    assert_eq!(run(&mut f, "2026-12-31")["created"], json!([]));
    assert_eq!(detail(&mut f, &t)["next_index"], 36);
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn a_backlog_over_36_dates_creates_nothing_and_only_the_excess_must_be_skipped() {
    let mut f = Fx::new();
    let p = template(&f);
    let t = save(&mut f, &p);
    let tid = s(&t, "id").to_owned();
    let before = f.counts();
    let commits = f.book.commits();
    // 37 dates are due through 2027-01-31; a wildly wrong clock is refused the same way.
    for clock in ["2027-01-31", "9999-12-31"] {
        let out = run(&mut f, clock);
        assert_eq!(out["created"], json!([]));
        assert!(
            out["errors"][0]["message"]
                .as_str()
                .unwrap()
                .contains("More than 36 bill dates are overdue, so none were created"),
            "{out}"
        );
    }
    assert_eq!(f.counts(), before);
    assert_eq!(f.book.commits(), commits, "a refused backlog writes nothing");
    let mut edit = p.clone();
    edit["id"] = t["id"].clone();
    edit["expected_revision"] = json!(1);
    assert!(
        r::save(&mut f.book, f.actor, f.co, &edit, "2027-01-31")
            .unwrap_err()
            .message()
            .contains("more than 36 missed bill dates")
    );
    let control = |f: &mut Fx, revision: i64, action: &str, confirmation: &Value| {
        r::control(
            &mut f.book,
            f.actor,
            f.co,
            &tid,
            &json!({"expected_revision":revision,"action":action,"confirmation":confirmation}),
            "2027-04-30",
        )
    };
    control(&mut f, 1, "pause", &Value::Null).unwrap();
    let before = f.counts();
    // 40 dates are now missed. Only the oldest 4 must be skipped; the newest 36 keep the choice.
    let paused = detail(&mut f, &t);
    let preview = r::due_confirmation(&paused, "2027-04-30").unwrap();
    assert_eq!(preview["total"], 40);
    assert_eq!(preview["count"], 4);
    assert_eq!(preview["more"], true);
    assert_eq!(preview["first_date"], "2024-01-31");
    assert_eq!(preview["last_date"], "2024-04-30");
    assert!(
        control(&mut f, 2, "resume_create", &preview["token"])
            .unwrap_err()
            .message()
            .contains("Skip the oldest 4 first")
    );
    assert!(control(&mut f, 2, "resume", &Value::Null).is_err());
    assert_eq!(f.counts(), before);
    control(&mut f, 2, "resume_skip", &preview["token"]).unwrap();
    let after_skip = detail(&mut f, &t);
    assert_eq!(after_skip["active"], 0, "stays paused while dates remain");
    assert_eq!(after_skip["next_index"], 4);
    assert_eq!(after_skip["occurrences"].as_array().unwrap().len(), 4);
    assert!(
        after_skip["occurrences"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["skipped"] == 1 && row["bill_id"].is_null())
    );
    let actions = rows(
        f.book.connection(),
        "SELECT action,count(*) AS n FROM ledger_auditevent WHERE action IN('recurring_bill.skip_missed','recurring_bill.skipped','recurring_bill.resume') GROUP BY action ORDER BY action",
        &[],
    )
    .unwrap();
    assert_eq!(
        actions,
        [
            json!({"action":"recurring_bill.skip_missed","n":1}),
            json!({"action":"recurring_bill.skipped","n":4})
        ]
    );
    assert_eq!(run(&mut f, "2027-04-30")["created"], json!([]));
    assert!(
        control(&mut f, 3, "resume_skip", &preview["token"]).is_err(),
        "the earlier decision cannot act again"
    );
    // The newest 36 dates fit one decision and one run, so they can all be created.
    let rest = r::due_confirmation(&after_skip, "2027-04-30").unwrap();
    assert_eq!(rest["total"], 36);
    assert_eq!(rest["count"], 36);
    assert_eq!(rest["more"], false);
    control(&mut f, 3, "resume_create", &rest["token"]).unwrap();
    let out = run(&mut f, "2027-04-30");
    assert_eq!(out["errors"], json!([]));
    assert_eq!(out["created"].as_array().unwrap().len(), 36);
    assert_eq!(out["created"][0]["current"]["date"], "2024-05-31");
    assert_eq!(out["created"][35]["current"]["date"], "2027-04-30");
    assert_eq!(f.counts()["payables_bill"], 36);
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn a_backlog_of_100_dates_is_skipped_36_then_28_before_the_newest_36_get_the_choice() {
    let mut f = Fx::new();
    let mut p = template(&f);
    p["active"] = json!(false);
    let t = save(&mut f, &p);
    let tid = s(&t, "id").to_owned();
    // Index 99 falls on 2032-04-30, so 100 dates are missed; a wildly wrong clock walks no further.
    let far = detail(&mut f, &t);
    assert_eq!(
        r::due_confirmation(&far, "9999-12-31").unwrap()["dates"],
        r::due_confirmation(&far, "2032-04-30").unwrap()["dates"]
    );
    let mut revision = 1;
    for (total, skip, last) in [
        (Value::Null, 36, "2026-12-31"),
        (json!(64), 28, "2029-04-30"),
    ] {
        let preview = r::due_confirmation(&detail(&mut f, &t), "2032-04-30").unwrap();
        assert_eq!(preview["total"], total);
        assert_eq!(preview["count"], skip);
        assert_eq!(preview["more"], true);
        assert_eq!(preview["last_date"], last);
        for (action, ok) in [("resume_create", false), ("resume_skip", true)] {
            assert_eq!(
                r::control(
                    &mut f.book,
                    f.actor,
                    f.co,
                    &tid,
                    &json!({"expected_revision":revision,"action":action,"confirmation":preview["token"]}),
                    "2032-04-30",
                )
                .is_ok(),
                ok
            );
        }
        revision += 1;
        assert_eq!(detail(&mut f, &t)["active"], 0);
    }
    let rest = r::due_confirmation(&detail(&mut f, &t), "2032-04-30").unwrap();
    assert_eq!((&rest["total"], &rest["count"], &rest["more"]), (&json!(36), &json!(36), &json!(false)));
    assert_eq!(rest["first_date"], "2029-05-31");
    r::control(
        &mut f.book,
        f.actor,
        f.co,
        &tid,
        &json!({"expected_revision":revision,"action":"resume_create","confirmation":rest["token"]}),
        "2032-04-30",
    )
    .unwrap();
    let out = run(&mut f, "2032-04-30");
    assert_eq!(out["errors"], json!([]));
    assert_eq!(out["created"].as_array().unwrap().len(), 36);
    assert_eq!(detail(&mut f, &t)["next_index"], 100);
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn make_recurring_never_suggests_a_first_date_older_than_the_limit() {
    let mut f = Fx::new();
    for (n, (source_date, want)) in [
        ("2026-10-31", "2026-11-30"),
        ("2024-10-15", "2024-11-15"),
        ("2024-01-31", "2024-11-30"),
        ("2020-03-10", "2024-12-10"),
    ]
    .into_iter()
    .enumerate()
    {
        let mut source = f.payload();
        source["date"] = json!(source_date);
        source["reference"] = json!(format!("Old source {n}"));
        let b = f.save(&source);
        let copy = r::from_bill(&mut f.book, f.actor, f.co, s(&b, "id"), "2026-11-15").unwrap();
        assert_eq!(copy["start_date"], want, "{source_date}");
        let mut p = template(&f);
        p["source_bill_id"] = b["id"].clone();
        p["start_date"] = copy["start_date"].clone();
        p["active"] = json!(false);
        r::save_confirmation(&mut f.book, f.actor, f.co, &p, "2026-11-15").unwrap();
    }
}
