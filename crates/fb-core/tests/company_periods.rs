mod common;
#[path = "common/receipt_books.rs"]
mod receipt_books;

use common::{Fx, err_text};
use fb_core::{
    Book, CompanyId, FbError, UserId, credits,
    directory::{self, Model},
    periods::{self, PeriodChange},
    reports::financial::{self, Basis},
    sales, services,
};
use fb_db::rusqlite::params;
use receipt_books::{Books, id, key};
use serde_json::{Value, json};

fn enroll(book: &mut Book, actor: UserId, role: &str, enrolled: bool) {
    let hash = enrolled.then(|| format!("$argon2id$v=19${}", "synthetic".repeat(10)));
    book.write(|w| {
        w.execute(
            "INSERT INTO desktop_user(user_id,role,password_hash) VALUES(?1,?2,?3)",
            params![actor.to_db(), role, hash],
        )
    })
    .unwrap();
}

fn change(month: u32, closing_date: Option<&str>) -> PeriodChange {
    PeriodChange {
        fiscal_year_start_month: month,
        closing_date: closing_date.map(str::to_owned),
    }
}

fn close(book: &mut Book, actor: UserId, c: CompanyId, date: &str) {
    enroll(book, actor, "administrator", true);
    periods::set_policy(book, actor, c, 0, change(1, Some(date)), None).unwrap();
}

#[test]
fn policy_requires_an_enrolled_member_and_a_reason_and_audits_only_committed_changes() {
    let mut f = Fx::new("period-policy");
    let co = f.company("First");
    let other = f.company("Second");
    let admin = f.user;
    let other_admin = fb_core::users::create_user(&mut f.book, "other-admin", true).unwrap();
    f.book
        .write(|w| {
            w.execute(
                "INSERT INTO ledger_company_members VALUES(?1,?2)",
                params![other.to_db(), other_admin.to_db()],
            )?;
            w.execute(
                "DELETE FROM ledger_company_members WHERE company_id=?1 AND user_id=?2",
                params![other.to_db(), admin.to_db()],
            )
        })
        .unwrap();
    enroll(&mut f.book, other_admin, "administrator", true);
    enroll(&mut f.book, admin, "administrator", true);
    for (role, enrolled) in [
        ("administrator", false),
        ("bookkeeper", true),
        ("readonly", true),
    ] {
        let user = fb_core::users::create_user(&mut f.book, role, false).unwrap();
        f.book
            .write(|w| {
                w.execute(
                    "INSERT INTO ledger_company_members VALUES(?1,?2)",
                    params![co.to_db(), user.to_db()],
                )
            })
            .unwrap();
        enroll(&mut f.book, user, role, enrolled);
        assert!(matches!(
            periods::set_policy(&mut f.book, user, co, 0, change(4, None), None),
            Err(FbError::PermissionDenied(_))
        ));
    }
    assert!(matches!(
        periods::set_policy(&mut f.book, admin, other, 0, change(4, None), None),
        Err(FbError::PermissionDenied(_))
    ));
    let ai = fb_core::agent_provenance::Context {
        connection_id: "synthetic".into(),
        agent_name: "Synthetic assistant".into(),
        operation: "periods".into(),
        request_id: key(),
        unattended: false,
    };
    assert!(matches!(
        fb_core::agent_provenance::with(ai, || periods::set_policy(
            &mut f.book,
            admin,
            co,
            0,
            change(4, None),
            None
        )),
        Err(FbError::PermissionDenied(_))
    ));

    assert_eq!(
        err_text(periods::set_policy(
            &mut f.book,
            admin,
            co,
            0,
            change(13, None),
            None
        )),
        periods::MONTH
    );
    assert_eq!(
        err_text(periods::set_policy(
            &mut f.book,
            admin,
            co,
            0,
            change(4, Some("2026-02-29")),
            None
        )),
        periods::DATE
    );
    let close = change(4, Some("2028-02-29"));
    assert_eq!(
        periods::set_policy(&mut f.book, admin, co, 0, close.clone(), None)
            .unwrap()
            .revision,
        1
    );
    assert_eq!(
        periods::set_policy(&mut f.book, admin, co, 1, close.clone(), None)
            .unwrap()
            .revision,
        1
    );
    assert_eq!(
        err_text(periods::set_policy(&mut f.book, admin, co, 0, close, None)),
        periods::STALE
    );
    for reason in [None, Some("   "), Some(&"x".repeat(501))] {
        assert_eq!(
            err_text(periods::set_policy(
                &mut f.book,
                admin,
                co,
                1,
                change(4, None),
                reason
            )),
            periods::REASON
        );
    }
    assert_eq!(
        periods::set_policy(
            &mut f.book,
            admin,
            co,
            1,
            change(4, Some("2028-02-27")),
            Some("  Enter leap day adjustment  ")
        )
        .unwrap()
        .revision,
        2
    );
    assert_eq!(
        periods::set_policy(
            &mut f.book,
            admin,
            co,
            2,
            change(4, None),
            Some("Reopen the fiscal ledger")
        )
        .unwrap()
        .revision,
        3
    );
    assert_eq!(
        periods::set_policy(
            &mut f.book,
            admin,
            co,
            3,
            change(4, Some("2028-02-29")),
            None
        )
        .unwrap()
        .revision,
        4
    );
    let rows = f.book.read(|q| {
        let mut statement = q.prepare("SELECT action,actor_name,target,detail FROM ledger_auditevent WHERE action GLOB 'company.period_*' OR action='company.fiscal_year_changed' ORDER BY id")?;
        statement.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?)))?.collect::<fb_db::rusqlite::Result<Vec<_>>>()
    }).unwrap();
    assert_eq!(
        rows.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(),
        [
            "company.fiscal_year_changed",
            "company.period_closed",
            "company.period_reopened",
            "company.period_reopened",
            "company.period_closed"
        ]
    );
    assert!(
        rows.iter()
            .all(|r| r.1 == "tester" && r.2 == format!("company:{co}"))
    );
    assert_eq!(
        serde_json::from_str::<Value>(&rows[2].3).unwrap(),
        json!({
            "before":{"fiscal_year_start_month":4,"closing_date":"2028-02-29"},
            "after":{"fiscal_year_start_month":4,"closing_date":"2028-02-27"},
            "reason":"Enter leap day adjustment","revision":2
        })
    );
    let problems = f.book.check().unwrap();
    assert!(problems.is_empty(), "{problems:?}");
}

#[test]
fn posted_history_stays_valid_and_the_effective_date_controls_drafts_and_reversals() {
    let mut f = Fx::new("closed-posting");
    let (co, bank, equity) = f.chart();
    let old = f.draft(co, bank, equity, "10", "2028-02-28");
    let posted = f.post(co, old.id);
    let locked = f.draft(co, bank, equity, "1", "2028-02-29");
    close(&mut f.book, f.user, co, "2028-02-29");
    let before = fb_db::test_support::snapshot(f.book.connection()).unwrap();
    assert!(
        err_text(services::post_journal(
            &mut f.book,
            f.user,
            co,
            locked.id,
            Some(locked.revision)
        ))
        .contains("closed through 2028-02-29")
    );
    assert!(
        err_text(services::reverse_journal(
            &mut f.book,
            f.user,
            co,
            posted.id,
            "2028-02-29",
            "Correction"
        ))
        .contains("Date 2028-02-29")
    );
    assert_eq!(
        fb_db::test_support::snapshot(f.book.connection()).unwrap(),
        before
    );
    let draft = f.draft(co, bank, equity, "5", "2028-02-28");
    assert!(
        err_text(services::post_journal(
            &mut f.book,
            f.user,
            co,
            draft.id,
            Some(draft.revision)
        ))
        .contains("Date 2028-02-28")
    );
    let tomorrow = f.draft(co, bank, equity, "5", "2028-03-01");
    f.post(co, tomorrow.id);
    let reversed = services::reverse_journal(
        &mut f.book,
        f.user,
        co,
        posted.id,
        "2028-03-01",
        "Forward-dated correction",
    )
    .unwrap();
    assert_eq!(reversed.date, "2028-03-01");
    let problems = f.book.check().unwrap();
    assert!(problems.is_empty(), "{problems:?}");
}

#[test]
fn credit_and_payment_events_obey_their_dates_and_exact_retries_survive_closing() {
    let mut f = Books::new();
    let invoice = f.invoice("200.00");
    let lines = json!([{"item_id":null,"description":"Synthetic allowance","quantity":"1","rate":"60.00","taxable":false,"income_account_id":f.income}]);
    let payload = json!({"request_key":key(),"customer_id":f.customer,"date":"2026-11-03","tax_mode":"NONE","lines":lines});
    let review = credits::review_credit_memo(&mut f.book, f.actor, f.company, &payload).unwrap();
    let memo =
        credits::record_credit_memo(&mut f.book, f.actor, f.company, &payload, Some(&review))
            .unwrap();
    let applied = json!({"request_key":key(),"date":"2026-11-04","allocations":[{"invoice_id":invoice,"amount":"10.00"}]});
    let apply_review =
        credits::review_application(&mut f.book, f.actor, f.company, id(&memo), &applied).unwrap();
    let applying = credits::apply_credit(
        &mut f.book,
        f.actor,
        f.company,
        id(&memo),
        &applied,
        Some(&apply_review),
    )
    .unwrap();
    let blocked = json!({"request_key":key(),"date":"2026-11-05","allocations":[{"invoice_id":invoice,"amount":"12.00"}]});
    let block_review =
        credits::review_application(&mut f.book, f.actor, f.company, id(&memo), &blocked).unwrap();
    let received = f.payment(&[(&invoice, "25.00")]);
    let received_review =
        sales::review_receipt(&mut f.book, f.actor, f.company, &received).unwrap();
    let payment = sales::record_receipt(
        &mut f.book,
        f.actor,
        f.company,
        &received,
        Some(&received_review),
    )
    .unwrap();
    let blocked_payment = f.payment(&[(&invoice, "30.00")]);
    let pay_review =
        sales::review_receipt(&mut f.book, f.actor, f.company, &blocked_payment).unwrap();
    close(&mut f.book, f.actor, f.company, "2026-11-05");
    let before = f.everything();
    assert_eq!(
        credits::apply_credit(
            &mut f.book,
            f.actor,
            f.company,
            id(&memo),
            &applied,
            Some(&apply_review)
        )
        .unwrap(),
        applying
    );
    assert_eq!(
        sales::record_receipt(
            &mut f.book,
            f.actor,
            f.company,
            &received,
            Some(&received_review)
        )
        .unwrap(),
        payment
    );
    assert!(
        err_text(credits::apply_credit(
            &mut f.book,
            f.actor,
            f.company,
            id(&memo),
            &blocked,
            Some(&block_review)
        ))
        .contains("closed period")
    );
    assert!(
        err_text(credits::unapply_credit(
            &mut f.book,
            f.actor,
            f.company,
            id(&applying),
            Some(1),
            &json!("2026-11-05"),
            &json!("Correction")
        ))
        .contains("closed period")
    );
    let connection = f.book.connection();
    for recursion in ["ON", "OFF"] {
        connection
            .execute_batch(&format!("PRAGMA recursive_triggers={recursion}"))
            .unwrap();
        let raw = connection.execute("INSERT INTO credits_creditunapplication(id,application_id,date,actor_id,data) VALUES(?1,?2,'2026-11-05',?3,'{}')", params![key(), id(&applying), f.actor.to_db()]).unwrap_err();
        assert_eq!(fb_db::constraint_of(&raw).as_deref(), Some("period_closed"));
        let raw = connection.execute("INSERT OR REPLACE INTO credits_creditapplication(id,company_id,credit_memo_id,request_key,date,amount,actor_id,data) VALUES(?1,?2,?3,?4,'2026-11-05',100,?5,'{}')", params![key(), f.company.to_db(), id(&memo), key(), f.actor.to_db()]).unwrap_err();
        assert_eq!(fb_db::constraint_of(&raw).as_deref(), Some("period_closed"));
    }
    connection
        .execute_batch("PRAGMA recursive_triggers=ON")
        .unwrap();
    assert!(
        err_text(sales::record_receipt(
            &mut f.book,
            f.actor,
            f.company,
            &blocked_payment,
            Some(&pay_review)
        ))
        .contains("closed period")
    );
    assert_eq!(f.everything(), before);
    let mut open = blocked_payment;
    open["date"] = json!("2026-11-06");
    let review = sales::review_receipt(&mut f.book, f.actor, f.company, &open).unwrap();
    sales::record_receipt(&mut f.book, f.actor, f.company, &open, Some(&review)).unwrap();
    let problems = f.book.check().unwrap();
    assert!(problems.is_empty(), "{problems:?}");
}

#[test]
fn fiscal_balance_sheet_reclassifies_income_on_both_bases_without_changing_total_equity() {
    let mut f = Fx::new("fiscal-balance-sheet");
    let (co, bank, _) = f.chart();
    let income = f.account(co, "4000", "Sales", "INCOME");
    for (date, amount) in [
        ("2027-10-31", "12"),
        ("2027-11-01", "20"),
        ("2028-02-29", "8"),
    ] {
        let draft = f.draft(co, bank, income, amount, date);
        f.post(co, draft.id);
    }
    enroll(&mut f.book, f.user, "administrator", true);
    let as_of = chrono::NaiveDate::from_ymd_opt(2028, 2, 29).unwrap();
    let load =
        |book: &mut Book, basis| financial::balance_sheet(book, f.user, co, basis, as_of).unwrap();
    let original = [Basis::Accrual, Basis::Cash].map(|b| load(&mut f.book, b));
    periods::set_policy(&mut f.book, f.user, co, 0, change(11, None), None).unwrap();
    for (basis, baseline) in [Basis::Accrual, Basis::Cash].into_iter().zip(original) {
        let report = load(&mut f.book, basis);
        assert_eq!(report.fiscal_year_start.to_string(), "2027-11-01");
        assert_eq!(report.retained_earnings.to_string(), "12.00");
        assert_eq!(report.net_income.to_string(), "28.00");
        assert_eq!(report.total_equity, baseline.total_equity);
        assert_eq!(report.assets.total, baseline.assets.total);
        assert!(report.balanced);
    }
}

#[test]
fn customer_terms_allow_partial_edits_and_blank_clear_and_remain_company_scoped() {
    let mut f = Fx::new("customer-terms");
    let co = f.company("First");
    let other = f.company("Second");
    let customer = directory::save_record(
        &mut f.book,
        f.user,
        co,
        Model::Customer,
        &json!({"display_name":"Synthetic buyer","default_terms":"15"}),
        None,
    )
    .unwrap();
    let id = id(&customer);
    let before = directory::save_record(
        &mut f.book,
        f.user,
        co,
        Model::Customer,
        &json!({"notes":"Partial edit"}),
        Some(id),
    )
    .unwrap();
    assert_eq!(before["default_terms"], "15");
    assert!(
        directory::save_record(
            &mut f.book,
            f.user,
            co,
            Model::Customer,
            &json!({"default_terms":"90"}),
            Some(id)
        )
        .is_err()
    );
    assert!(
        directory::save_record(
            &mut f.book,
            f.user,
            other,
            Model::Customer,
            &json!({"default_terms":"0"}),
            Some(id)
        )
        .is_err()
    );
    let after = directory::save_record(
        &mut f.book,
        f.user,
        co,
        Model::Customer,
        &json!({"default_terms":""}),
        Some(id),
    )
    .unwrap();
    assert!(after["default_terms"].is_null());
}

#[test]
fn the_policy_writer_and_posting_writer_serialize_in_both_orders() {
    for policy_first in [true, false] {
        let path = fb_db::test_support::TempBookPath::new("period-concurrency");
        let mut book = path.book(&[68; 32]);
        let actor =
            fb_core::users::create_user(&mut book, "Synthetic administrator", false).unwrap();
        let mut f = Fx { book, user: actor };
        let (co, bank, equity) = f.chart();
        enroll(&mut f.book, actor, "administrator", true);
        let journal = f.draft(co, bank, equity, "1", "2028-02-29");
        let journal_id = journal.id;
        let revision = journal.revision;
        let mut competing = Book::open(path.path(), &[68; 32]).unwrap();
        let new = || change(4, Some("2028-02-29"));
        let (begin, started) = std::sync::mpsc::channel();
        let (ready, proceed) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            let challenger = scope.spawn(move || {
                started.recv().unwrap();
                ready.send(()).unwrap();
                if policy_first {
                    assert!(
                        err_text(services::post_journal(
                            &mut competing,
                            actor,
                            co,
                            journal_id,
                            Some(revision)
                        ))
                        .contains("closed period")
                    );
                } else {
                    periods::set_policy(&mut competing, actor, co, 0, new(), None).unwrap();
                }
            });
            f.book
                .write(|w| -> Result<(), FbError> {
                    begin.send(()).unwrap();
                    proceed.recv().unwrap();
                    if policy_first {
                        w.set_company_period_policy(actor, co, 0, new(), None)
                            .unwrap();
                    } else {
                        services::post_journal_tx(w, actor, co, journal_id, Some(revision))?;
                    }
                    Ok(())
                })
                .unwrap();
            challenger.join().unwrap();
        });
        assert_eq!(f.audit_count("journal.posted"), i64::from(!policy_first));
        assert_eq!(f.audit_count("company.period_closed"), 1);
        let problems = f.book.check().unwrap();
    assert!(problems.is_empty(), "{problems:?}");
    }
}
