use super::*;
#[test]
fn competing_payments_serialize_without_overpayment() {
    let mut f = Fx::new();
    let p = f.payload();
    let b = f.save(&p);
    let b = f.post(&b);
    let mut p = f.payment(&b, "0.75");
    p["method"] = json!("OTHER");
    let r = review_payment(&mut f.book, f.actor, f.co, &p).unwrap();
    let mut p2 = p.clone();
    p2["request_key"] = json!(fb_ids::new_v7().to_string());
    let r2 = review_payment(&mut f.book, f.actor, f.co, &p2).unwrap();
    let path = std::path::PathBuf::from(f.book.connection().path().unwrap());
    let a = Book::open(&path, &[42; 32]).unwrap();
    let b2 = Book::open(&path, &[42; 32]).unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let mut threads = vec![];
    for (mut book, p, review) in [(a, p, r), (b2, p2, r2)] {
        let barrier = barrier.clone();
        let (actor, co) = (f.actor, f.co);
        threads.push(std::thread::spawn(move || {
            barrier.wait();
            record_payment(&mut book, actor, co, &p, &review)
        }));
    }
    let results = threads
        .into_iter()
        .map(|t| t.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(results.iter().filter(|r| r.is_err()).count(), 1);
    let rows = current_bills(&mut f.book, f.actor, f.co).unwrap();
    assert_eq!(rows[0]["allocated"], "0.75");
    assert_eq!(rows[0]["outstanding"], "0.26");
    assert_eq!(f.counts()["payables_billpayment"], 1);
}
#[test]
fn simultaneous_same_create_is_exactly_once() {
    let mut f = Fx::new();
    let p = f.payload();
    let path = std::path::PathBuf::from(f.book.connection().path().unwrap());
    let a = Book::open(&path, &[42; 32]).unwrap();
    let b = Book::open(&path, &[42; 32]).unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let mut ts = vec![];
    for mut book in [a, b] {
        let barrier = barrier.clone();
        let p = p.clone();
        let (actor, co) = (f.actor, f.co);
        ts.push(std::thread::spawn(move || {
            barrier.wait();
            save_bill(&mut book, actor, co, &p)
        }));
    }
    let results = ts
        .into_iter()
        .map(|t| t.join().unwrap().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results[0]["id"], results[1]["id"]);
    assert_eq!(current_bills(&mut f.book, f.actor, f.co).unwrap().len(), 1);
    assert_eq!(f.counts()["payables_billrevision"], 1);
}
#[test]
fn tracker_and_calendar_share_balances_not_gross_totals() {
    let mut f = Fx::new();
    let p = f.payload();
    let b = f.save(&p);
    let b = f.post(&b);
    let p = f.payment(&b, "0.50");
    let pay = f.pay(&p);
    let p = f.payload();
    let mut draft = p.clone();
    draft["reference"] = json!("DRAFT");
    f.save(&draft);
    let now = json!("2026-11-23");
    let t = bill_tracker(
        &mut f.book,
        f.actor,
        f.co,
        &json!({"state":"OUTSTANDING"}),
        &now,
    )
    .unwrap();
    assert_eq!(t["totals"]["open_money"], "0.51");
    assert_eq!(t["totals"]["overdue_money"], "0.00");
    assert_eq!(t["totals"]["draft_count"], 1);
    assert_eq!(t["rows"].as_array().unwrap().len(), 1);
    let t = bill_tracker(
        &mut f.book,
        f.actor,
        f.co,
        &json!({"state":"OVERDUE"}),
        &json!("2026-11-24"),
    )
    .unwrap();
    assert_eq!(t["totals"]["overdue_money"], "0.51");
    assert_eq!(t["rows"].as_array().unwrap().len(), 1);
    let cal = bill_calendar(&mut f.book, f.actor, f.co, 2026, 11, &now, false, None).unwrap();
    assert_eq!(cal["month_total"], "0.51");
    assert_eq!(cal["entries"][0]["status"], "today");
    assert_eq!(cal["days"].as_array().unwrap().len(), 30);
    assert_eq!(cal["weeks"][0][0], Value::Null);
    reverse_payment(
        &mut f.book,
        f.actor,
        f.co,
        s(&pay, "id"),
        Some(1),
        &json!("2026-10-24"),
        &json!("Correction"),
    )
    .unwrap();
    let p = f.payment(&b, "1.01");
    f.pay(&p);
    let cal = bill_calendar(&mut f.book, f.actor, f.co, 2026, 11, &now, false, None).unwrap();
    assert_eq!(cal["entries"], json!([]));
    let cal = bill_calendar(&mut f.book, f.actor, f.co, 2026, 11, &now, true, None).unwrap();
    assert_eq!(cal["entries"][0]["status"], "paid");
    assert_eq!(cal["month_total"], "0.00");
    let t = bill_tracker(
        &mut f.book,
        f.actor,
        f.co,
        &json!({"state":"PAID_RECENT","q":"synthetic"}),
        &now,
    )
    .unwrap();
    assert_eq!(t["totals"]["paid_recent_money"], "1.01");
    assert_eq!(t["rows"].as_array().unwrap().len(), 1);
    let t = bill_tracker(&mut f.book, f.actor, f.co, &json!({}), &json!("2026-11-24")).unwrap();
    assert_eq!(t["totals"]["paid_recent_money"], "0.00");
}
#[test]
fn company_scope_permissions_and_read_only_queries() {
    let mut f = Fx::new();
    let p = f.payload();
    let b = f.save(&p);
    let co = services::create_company(
        &mut f.book,
        f.actor,
        "Other synthetic company",
        "USD",
        false,
    )
    .unwrap()
    .id;
    assert!(get_bill(&mut f.book, f.actor, co, s(&b, "id")).is_err());
    assert!(current_bills(&mut f.book, f.actor, co).unwrap().is_empty());
    let user = users::create_user(&mut f.book, "outsider", false).unwrap();
    let before = f.counts();
    assert!(save_bill(&mut f.book, user, f.co, &p).is_err());
    assert!(review_bill(&mut f.book, user, f.co, s(&b, "id"), Some(1)).is_err());
    assert!(current_bills(&mut f.book, user, f.co).is_err());
    assert!(bill_tracker(&mut f.book, user, f.co, &json!({}), &json!("2026-11-23")).is_err());
    assert!(
        bill_calendar(
            &mut f.book,
            user,
            f.co,
            2026,
            11,
            &json!("2026-11-23"),
            false,
            None
        )
        .is_err()
    );
    assert_eq!(before, f.counts());
}

#[test]
fn receipt_mismatch_is_editable_but_not_silently_saved() {
    let mut f = Fx::new();
    let key = fb_ids::new_v7().to_string();
    let mut fields = json!({"document_date":"2026-10-23","lines":[{"description":"Printed mismatch","quantity":"2","rate":"3","amount":"7"}]});
    let p = receipt_draft_input(&mut f.book, f.actor, f.co, &f.vendor, &fields, &key).unwrap();
    assert_eq!(p["lines"][0]["rate"], "3");
    let before = f.counts();
    assert!(save_receipt_draft(&mut f.book, f.actor, f.co, &f.vendor, &fields, &key).is_err());
    assert_eq!(before, f.counts());
    fields["lines"][0]["rate"] = json!("3.50");
    let b = save_receipt_draft(&mut f.book, f.actor, f.co, &f.vendor, &fields, &key).unwrap();
    assert_eq!(b["state"], "DRAFT");
    assert_eq!(b["current"]["total"], "7.00");
}
