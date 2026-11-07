use super::*;
use std::sync::{Arc, Barrier};

#[test]
fn archive_snapshot_refresh_retry_and_reordering_follow_python() {
    let mut f = Fixture::new();
    let item = directory::save_record(
        &mut f.book,
        f.actor,
        f.company,
        Model::Item,
        &json!({"code":"SKU","kind":"SERVICE","description":"Old item","sale_rate":"9.99"}),
        None,
    )
    .unwrap();
    let mut input = f.input();
    input["lines"][0]["item_id"] = item["id"].clone();
    let old = save_invoice(&mut f.book, f.actor, f.company, &input).unwrap();
    directory::save_record(
        &mut f.book,
        f.actor,
        f.company,
        Model::Customer,
        &json!({"display_name":"Renamed"}),
        Some(&f.customer),
    )
    .unwrap();
    directory::save_record(
        &mut f.book,
        f.actor,
        f.company,
        Model::Item,
        &json!({"description":"New item","sale_rate":"20"}),
        item["id"].as_str(),
    )
    .unwrap();
    let key = old["id"].as_str().unwrap();
    let mut edit = input.clone();
    edit["invoice_id"] = json!(key);
    edit["expected_revision"] = json!(1);
    let kept = save_invoice(&mut f.book, f.actor, f.company, &edit).unwrap();
    assert_eq!(
        kept["current"]["customer_snapshot"],
        old["current"]["customer_snapshot"]
    );
    assert_eq!(
        kept["lines"][0]["item_snapshot"],
        old["lines"][0]["item_snapshot"]
    );
    edit["expected_revision"] = json!(2);
    edit["refresh_customer"] = json!(true);
    edit["lines"][0]["refresh_item"] = json!(true);
    let fresh = save_invoice(&mut f.book, f.actor, f.company, &edit).unwrap();
    assert_eq!(
        fresh["current"]["customer_snapshot"]["display_name"],
        "Renamed"
    );
    assert_eq!(
        fresh["lines"][0]["item_snapshot"]["description"],
        "New item"
    );
    assert_eq!(fresh["lines"][0]["rate"], old["lines"][0]["rate"]);
    directory::set_active(
        &mut f.book,
        f.actor,
        f.company,
        Model::Customer,
        &f.customer,
        false,
    )
    .unwrap();
    directory::set_active(
        &mut f.book,
        f.actor,
        f.company,
        Model::Item,
        item["id"].as_str().unwrap(),
        false,
    )
    .unwrap();
    assert_eq!(
        save_invoice(&mut f.book, f.actor, f.company, &input).unwrap()["id"],
        old["id"]
    );
    edit["expected_revision"] = json!(3);
    edit["refresh_customer"] = json!(false);
    edit["lines"][0]["refresh_item"] = json!(false);
    assert!(save_invoice(&mut f.book, f.actor, f.company, &edit).is_err());
    edit["keep_archived"] = json!(true);
    edit["lines"][0]["keep_archived"] = json!(true);
    let kept = save_invoice(&mut f.book, f.actor, f.company, &edit).unwrap();
    assert_eq!(kept["revision"], 4);
    assert!(review_invoice(&mut f.book, f.actor, f.company, key, Some(4)).is_err());
}
#[test]
fn original_ar_split_and_historical_undeposited_remain_control_accounts() {
    let mut f = Fixture::new();
    let one = f.posted();
    let mut ai = AccountInput::new("111", "Other AR");
    ai.kind = Some("ASSET".into());
    let ar2 = services::save_account(&mut f.book, f.actor, f.company, ai)
        .unwrap()
        .id;
    documents::save_setup(&mut f.book,f.actor,f.company,&json!({"receivables":ar2.to_db(),"undeposited":f.undeposited.to_db(),"collected_tax":f.tax.to_db(),"banks":[f.bank.to_db()]}),Some(1)).unwrap();
    let two = f.posted();
    let mut input = f.payment(one["id"].as_str().unwrap(), "2");
    input["allocations"] =
        json!([{"invoice_id":one["id"],"amount":"1"},{"invoice_id":two["id"],"amount":"1"}]);
    input["destination"] = json!("UNDEPOSITED");
    input["bank_id"] = Value::Null;
    let r = review_receipt(&mut f.book, f.actor, f.company, &input).unwrap();
    let r = record_receipt(&mut f.book, f.actor, f.company, &input, Some(&r)).unwrap();
    let lines = store::lines(
        f.book.connection(),
        r["journal_id"].as_str().unwrap().parse().unwrap(),
    )
    .unwrap();
    assert_eq!(lines.len(), 3);
    assert!(lines.iter().any(|l| l.account_id == f.ar));
    assert!(lines.iter().any(|l| l.account_id == ar2));
    documents::save_setup(
        &mut f.book,
        f.actor,
        f.company,
        &json!({"receivables":ar2.to_db(),"collected_tax":f.tax.to_db(),"banks":[f.bank.to_db()]}),
        Some(2),
    )
    .unwrap();
    assert_eq!(
        documents::control_role(f.book.connection(), f.company, f.undeposited)
            .unwrap()
            .as_deref(),
        Some("undeposited")
    );
}
#[test]
fn zero_total_decimal_overflow_dates_and_strict_types_never_panic_or_write() {
    let mut f = Fixture::new();
    let mut zero = f.input();
    zero["lines"][0]["rate"] = json!("0");
    let draft = save_invoice(&mut f.book, f.actor, f.company, &zero).unwrap();
    assert_eq!(draft["current"]["total"], "0.00");
    assert!(
        review_invoice(
            &mut f.book,
            f.actor,
            f.company,
            draft["id"].as_str().unwrap(),
            Some(1)
        )
        .is_err()
    );
    for input in [
        json!(true),
        json!("NaN"),
        json!("Infinity"),
        json!("1e999999999999"),
        json!("-1"),
        json!("0.00001"),
    ] {
        let mut v = f.input();
        v["lines"][0]["quantity"] = input;
        assert!(save_invoice(&mut f.book, f.actor, f.company, &v).is_err());
    }
    let mut v = f.input();
    v["lines"][0]["quantity"] = json!("99999999999999.9999");
    v["lines"][0]["rate"] = json!("99999999999999.99");
    assert!(save_invoice(&mut f.book, f.actor, f.company, &v).is_err());
    for v in [
        json!("2026-1-1"),
        json!(" 2026-10-01"),
        json!("2026-10-01\n"),
        json!("2026-02-30"),
        json!(false),
    ] {
        assert!(date(&v).is_err())
    }
    assert_eq!(date(&json!("2026-W44-5")).unwrap(), "2026-10-30");
    assert_eq!(date(&json!("20261030")).unwrap(), "2026-10-30");
}
#[test]
fn cross_kind_review_capabilities_refuse_and_reviews_are_read_only() {
    let mut f = Fixture::new();
    let inv = f.posted();
    let input = f.payment(inv["id"].as_str().unwrap(), "1");
    let r = review_receipt(&mut f.book, f.actor, f.company, &input).unwrap();
    let sale = f.sale_input();
    assert!(record_sales_receipt(&mut f.book, f.actor, f.company, &sale, Some(&r)).is_err());
    let sr = review_sales_receipt(&mut f.book, f.actor, f.company, &sale).unwrap();
    assert!(record_receipt(&mut f.book, f.actor, f.company, &input, Some(&sr)).is_err());
}
#[test]
fn unsafe_pragmas_dropped_guards_and_unregistered_connections_fail_closed() {
    let mut f = Fixture::new();
    let input = f.input();
    for (pragma, bad, good) in [
        ("foreign_keys", 0, 1),
        ("recursive_triggers", 0, 1),
        ("ignore_check_constraints", 1, 0),
        ("writable_schema", 1, 0),
        ("synchronous", 0, 2),
    ] {
        f.book
            .connection()
            .execute_batch(&format!("PRAGMA {pragma}={bad}"))
            .unwrap();
        let err = save_invoice(&mut f.book, f.actor, f.company, &input).unwrap_err();
        assert!(
            err.message().contains("Unsafe") || err.message().contains("PRAGMA"),
            "{}",
            err.message()
        );
        f.book
            .connection()
            .execute_batch(&format!("PRAGMA {pragma}={good}"))
            .unwrap();
    }
    f.book
        .connection()
        .execute_batch("DROP TRIGGER fb_sales_allocation_insert")
        .unwrap();
    assert!(
        initialize(&f.book)
            .unwrap_err()
            .message()
            .contains("fb_sales_allocation_insert")
    );
}
#[test]
fn two_connections_duplicate_payment_commits_exactly_once() {
    let mut f = Fixture::new();
    let inv = f.posted();
    let input = f.payment(inv["id"].as_str().unwrap(), "1");
    let review = review_receipt(&mut f.book, f.actor, f.company, &input).unwrap();
    let path: String = f
        .book
        .connection()
        .query_row(
            "SELECT file FROM pragma_database_list WHERE name='main'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let mut joins = Vec::new();
    for _ in 0..2 {
        let path = path.clone();
        let b = barrier.clone();
        let input = input.clone();
        let review = review.clone();
        let actor = f.actor;
        let c = f.company;
        joins.push(std::thread::spawn(move || {
            let mut book = Book::open(std::path::Path::new(&path), &[13; 32]).unwrap();
            b.wait();
            record_receipt(&mut book, actor, c, &input, Some(&review)).unwrap()["id"].clone()
        }));
    }
    let ids = joins
        .into_iter()
        .map(|j| j.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(ids[0], ids[1]);
    let n: i64 = f
        .book
        .connection()
        .query_row("SELECT count(*) FROM sales_receipt", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1);
}
#[test]
fn competing_payment_reviews_cannot_overpay_between_connections() {
    let mut f = Fixture::new();
    let inv = f.posted();
    let one = f.payment(inv["id"].as_str().unwrap(), "3");
    let two = f.payment(inv["id"].as_str().unwrap(), "3");
    let r1 = review_receipt(&mut f.book, f.actor, f.company, &one).unwrap();
    let r2 = review_receipt(&mut f.book, f.actor, f.company, &two).unwrap();
    let path: String = f
        .book
        .connection()
        .query_row(
            "SELECT file FROM pragma_database_list WHERE name='main'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let mut joins = Vec::new();
    for (input, review) in [(one, r1), (two, r2)] {
        let path = path.clone();
        let b = barrier.clone();
        let actor = f.actor;
        let c = f.company;
        joins.push(std::thread::spawn(move || {
            let mut book = Book::open(std::path::Path::new(&path), &[13; 32]).unwrap();
            b.wait();
            record_receipt(&mut book, actor, c, &input, Some(&review)).is_ok()
        }));
    }
    let successes = joins
        .into_iter()
        .map(|j| j.join().unwrap())
        .filter(|v| *v)
        .count();
    assert_eq!(successes, 1);
    assert_eq!(
        invoice_balances(&mut f.book, f.actor, f.company).unwrap()[0]["outstanding"],
        "1.26"
    );
}

#[test]
fn frozen_python_service_date_vectors() {
    let cases: Value = serde_json::from_str(include_str!("date_vectors.json")).unwrap();
    for row in cases.as_array().unwrap() {
        match date(&row["input"]) {
            Ok(s) => assert_eq!(json!(s), row["output"], "{row}"),
            Err(e) => assert_eq!(json!(e.message()), row["error"], "{row}"),
        }
    }
}
