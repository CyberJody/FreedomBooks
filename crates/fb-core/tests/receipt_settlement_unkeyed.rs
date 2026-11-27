//! Without `require_same_settlement` a receipt records exactly as it did before the key existed
//! (commit ec7e2af4). This file uses only what that commit has, so it runs there unchanged.
#[path = "common/receipt_books.rs"]
mod receipt_books;
use fb_core::sales;
use receipt_books::Books;
use serde_json::{Value, json};

/// The virtual-machine steps SQLite runs on the book's connection during `work`: a measure of
/// every read and write, the same for the same statements over the same rows.
fn vm_steps<T>(f: &mut Books, work: impl FnOnce(&mut Books) -> T) -> (u64, T) {
    use std::sync::{Arc, atomic::{AtomicU64, Ordering}};
    let steps = Arc::new(AtomicU64::new(0));
    let counter = steps.clone();
    f.book.connection_mut().progress_handler(1, Some(move || { counter.fetch_add(1, Ordering::Relaxed); false })).unwrap();
    let result = work(f);
    f.book.connection_mut().progress_handler(0, None::<fn() -> bool>).unwrap();
    (steps.load(Ordering::Relaxed), result)
}
/// The value with each random identifier replaced by the order it first appears in, wherever in a
/// string it stands, and each digest by a mark.
fn without_random_parts(value: &Value, seen: &mut Vec<String>) -> Value {
    let is_id = |text: &str| text.len() == 36 && text.char_indices().all(|(i, c)| if [8, 13, 18, 23].contains(&i) { c == '-' } else { c.is_ascii_hexdigit() });
    match value {
        Value::String(text) if text.len() == 64 && text.chars().all(|c| c.is_ascii_hexdigit()) => json!("<digest>"),
        Value::String(text) => {
            let (mut plain, mut rest) = (String::new(), text.as_str());
            while !rest.is_empty() {
                if let Some(id) = rest.get(..36).filter(|start| is_id(start)) {
                    let position = seen.iter().position(|known| known == id).unwrap_or_else(|| { seen.push(id.to_owned()); seen.len() - 1 });
                    plain.push_str(&format!("<id {position}>"));
                    rest = &rest[36..];
                } else {
                    let first = rest.chars().next().unwrap();
                    plain.push(first);
                    rest = &rest[first.len_utf8()..];
                }
            }
            json!(plain)
        }
        Value::Array(items) => Value::Array(items.iter().map(|item| without_random_parts(item, seen)).collect()),
        Value::Object(fields) => Value::Object(fields.iter().map(|(name, field)| (name.clone(), without_random_parts(field, seen))).collect()),
        other => other.clone(),
    }
}

// The ordinary path as the desktop's payment screens, Stripe and the import use it: the review, its
// form projection and the recording. The line this prints is compared between this commit and
// ec7e2af4 (`PIN`): what each returned, random identifiers aside, and the SQLite steps each took,
// on an invoice that has every kind of entry on it, counted and reversed.
#[test]
fn without_the_key_a_review_and_a_recording_do_the_work_they_did_before() {
    let mut f = Books::new();
    let (invoice, _) = f.settled_invoice();
    let reversed = f.receive(&invoice, "20.00");
    f.reverse(&reversed);
    let credit = f.credit(&invoice, "10.00");
    f.unapply_credit(&credit);
    let deposit = f.deposit(&invoice, "5.00");
    f.unapply_deposit(&deposit);
    let other = f.invoice("60.00");
    let input = f.payment(&[(&invoice, "75.00"), (&other, "60.00")]);

    let (review_steps, review) = vm_steps(&mut f, |f| sales::review_receipt(&mut f.book, f.actor, f.company, &input).unwrap());
    let (again_steps, again) = vm_steps(&mut f, |f| sales::review_receipt(&mut f.book, f.actor, f.company, &input).unwrap());
    assert_eq!((review_steps, &review.display), (again_steps, &again.display), "the count belongs to the statements and the rows, not to the run");
    let (form_steps, (_, form)) = vm_steps(&mut f, |f| sales::review_receipt_form(&mut f.book, f.actor, f.company, &input, false).unwrap());
    let (record_steps, recorded) = vm_steps(&mut f, |f| sales::record_receipt(&mut f.book, f.actor, f.company, &input, Some(&review)).unwrap());
    assert_eq!(f.outstanding(&invoice), "30.00");
    let mut seen = Vec::new();
    println!("PIN {}", json!({"review_steps":review_steps,"form_steps":form_steps,"record_steps":record_steps,
        "review":without_random_parts(&review.display, &mut seen),"form":without_random_parts(&form, &mut seen),
        "recorded":without_random_parts(&recorded, &mut seen)}));
}

#[test]
fn without_the_key_the_same_changes_after_the_review_still_record() {
    for (what, change) in Books::changes() {
        let mut f = Books::new();
        let (invoice, seen) = f.settled_invoice();
        let input = f.payment(&[(&invoice, "75.00")]);
        let review = sales::review_receipt(&mut f.book, f.actor, f.company, &input).unwrap();
        change(&mut f, &invoice, &seen);
        let counts = f.counts();
        let open = f.outstanding(&invoice);
        let recorded = sales::record_receipt(&mut f.book, f.actor, f.company, &input, Some(&review)).unwrap();
        assert_eq!((&recorded["kind"], &recorded["amount"]), (&json!("RCP"), &json!("75.00")), "{what}");
        assert_eq!(f.counts(), [counts[0] + 1, counts[1] + 1, counts[2] + 1, counts[3] + 1], "{what}");
        let cents = |text: &str| text.replace('.', "").parse::<i64>().unwrap();
        assert_eq!(cents(&f.outstanding(&invoice)), cents(&open) - 7500, "{what}");
        // The repeat returns the same receipt.
        assert_eq!(sales::record_receipt(&mut f.book, f.actor, f.company, &input, Some(&review)).unwrap(), recorded, "{what}");
        assert!(f.book.check().unwrap().is_empty(), "{what}");
    }
}

// Review finding 1 as the books behave without the key: both receipts are recorded.
#[test]
fn without_the_key_the_reviewed_sequence_records_both_receipts() {
    let mut f = Books::new();
    let invoice = f.invoice("200.00");
    let input = f.payment(&[(&invoice, "75.00")]);
    let review = sales::review_receipt(&mut f.book, f.actor, f.company, &input).unwrap();
    f.receive(&invoice, "75.00");
    sales::record_receipt(&mut f.book, f.actor, f.company, &input, Some(&review)).unwrap();
    assert_eq!(f.count("sales_receipt"), 2);
    assert_eq!(f.outstanding(&invoice), "50.00");
}

#[test]
fn without_the_key_only_the_outstanding_amount_and_the_invoice_revision_stop_a_receipt() {
    let mut f = Books::new();
    let invoice = f.invoice("200.00");
    let input = f.payment(&[(&invoice, "75.00")]);
    let review = sales::review_receipt(&mut f.book, f.actor, f.company, &input).unwrap();
    f.receive(&invoice, "150.00");
    let counts = f.counts();
    let error = sales::record_receipt(&mut f.book, f.actor, f.company, &input, Some(&review)).unwrap_err();
    assert_eq!(error.to_string(), fb_core::FbError::validation("Allocation exceeds the actual outstanding invoice amount. Review again.").to_string());
    assert_eq!(f.counts(), counts);

    let other = f.invoice("200.00");
    let input = f.payment(&[(&other, "75.00")]);
    let review = sales::review_receipt(&mut f.book, f.actor, f.company, &input).unwrap();
    sales::void_invoice(&mut f.book, f.actor, f.company, &other, Some(2), &json!("2026-11-06"), &json!("Synthetic correction")).unwrap();
    let counts = f.counts();
    let error = sales::record_receipt(&mut f.book, f.actor, f.company, &input, Some(&review)).unwrap_err();
    assert_eq!(error.to_string(), fb_core::FbError::validation("Allocate only to this customer’s posted, unreversed invoices.").to_string());
    assert_eq!(f.counts(), counts);
}
