//! `require_same_settlement`: a receipt is not recorded beside a receipt, credit, customer deposit
//! or reversal its review never listed. The settlement is read only for a review that asks for it,
//! up to `SETTLEMENT_ENTRY_LIMIT` entries, and only such a review can satisfy the key. The unkeyed
//! behaviour is pinned in `receipt_settlement_unkeyed.rs`.
#[path = "common/receipt_books.rs"]
mod receipt_books;
use fb_core::{FbError, sales};
use receipt_books::{Books, REFUSAL, id};
use serde_json::{Value, json};

fn refused(result: fb_core::FbResult<Value>) {
    match result {
        Err(FbError::Validation(messages)) => assert_eq!(messages, [REFUSAL]),
        other => panic!("expected the receipt precondition refusal, got {other:?}"),
    }
}
/// A receipt review that asks for the settlement, as `include_settlement` does: the review and
/// what it lists, `None` above the limit.
fn review_asking(f: &mut Books, input: &Value) -> (sales::Review, Option<Value>) {
    let (review, _, settlement) =
        sales::review_receipt_form_with_settlement(&mut f.book, f.actor, f.company, input, false).unwrap();
    (review, settlement)
}
/// The same for a settlement under the limit.
fn asked(f: &mut Books, input: &Value) -> (sales::Review, Value) {
    let (review, settlement) = review_asking(f, input);
    (review, settlement.expect("a settlement under the limit"))
}
/// Something entered on the invoice and taken back again; returns the entry.
type Undone = fn(&mut Books, &str) -> Value;
fn keyed(f: &mut Books, input: &Value, review: &sales::Review) -> fb_core::FbResult<Value> {
    sales::record_receipt_same_settlement(&mut f.book, f.actor, f.company, input, Some(review), false)
}

#[test]
fn the_review_lists_everything_counted_toward_each_invoice() {
    let mut f = Books::new();
    let (invoice, seen) = f.settled_invoice();
    let reversed = f.receive(&invoice, "20.00");
    f.reverse(&reversed);
    let unapplied_credit = f.credit(&invoice, "10.00");
    f.unapply_credit(&unapplied_credit);
    let unapplied_deposit = f.deposit(&invoice, "5.00");
    f.unapply_deposit(&unapplied_deposit);
    let untouched = f.invoice("60.00");

    let input = f.payment(&[(&invoice, "75.00"), (&untouched, "60.00")]);
    let (_, settlement) = asked(&mut f, &input);
    assert_eq!(settlement.as_array().unwrap().len(), 2, "one entry for each allocated invoice");
    let entry = |settlement: &Value, wanted: &str| settlement.as_array().unwrap().iter().find(|row| row["invoice_id"] == wanted).unwrap().clone();
    assert_eq!(entry(&settlement, &untouched),
        json!({"invoice_id":untouched,"total_cents":"6000","outstanding_cents":"6000","entries":[]}));
    let first = entry(&settlement, &invoice);
    assert_eq!((&first["total_cents"], &first["outstanding_cents"]), (&json!("20000"), &json!("10500")));
    // Oldest first; a receipt carries the receipt's id, a credit and a deposit their application's.
    assert_eq!(first["entries"], json!([
        {"id":id(&seen[2]),"kind":"deposit","amount_cents":"2500","date":"2026-11-02","reversed":false},
        {"id":id(&unapplied_deposit),"kind":"deposit","amount_cents":"500","date":"2026-11-02","reversed":true},
        {"id":id(&seen[1]),"kind":"credit","amount_cents":"4000","date":"2026-11-04","reversed":false},
        {"id":id(&unapplied_credit),"kind":"credit","amount_cents":"1000","date":"2026-11-04","reversed":true},
        {"id":id(&seen[0]),"kind":"receipt","amount_cents":"3000","date":"2026-11-05","reversed":false},
        {"id":id(&reversed),"kind":"receipt","amount_cents":"2000","date":"2026-11-05","reversed":true},
    ]));
    // The unreversed entries are exactly what the book counts: its own shared view of live
    // settlements, and the invoice list's outstanding amount.
    let live: i64 = first["entries"].as_array().unwrap().iter().filter(|e| e["reversed"] == false)
        .map(|e| e["amount_cents"].as_str().unwrap().parse::<i64>().unwrap()).sum();
    let counted: i64 = f.book.read(|q| q.query_row(
        "SELECT sum(amount) FROM prepayments_live_settlements WHERE invoice_id=?1", [&invoice], |row| row.get(0))).unwrap();
    assert_eq!((live, counted), (9500, 9500));
    assert_eq!(f.outstanding(&invoice), "105.00");

    // A sales receipt is allocated to no invoice.
    let sale = json!({"request_key":receipt_books::key(),"customer_id":f.customer,"date":"2026-11-05","method":"CASH","reference":"","memo":"",
        "destination":"BANK","bank_id":f.bank,"tax_mode":"NONE","lines":[{"item_id":null,"description":"Synthetic counter sale","quantity":"1","rate":"12.00","taxable":false,"income_account_id":f.income}]});
    let (review, _, settlement) = sales::review_receipt_form_with_settlement(&mut f.book, f.actor, f.company, &sale, true).unwrap();
    assert_eq!(settlement, Some(json!([])));
    let recorded = sales::record_receipt_same_settlement(&mut f.book, f.actor, f.company, &sale, Some(&review), true).unwrap();
    assert_eq!((&recorded["kind"], &recorded["amount"]), (&json!("SR"), &json!("12.00")));
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn with_the_key_every_change_after_the_review_refuses_and_records_nothing() {
    for (what, change) in Books::changes() {
        let mut f = Books::new();
        let (invoice, seen) = f.settled_invoice();
        let input = f.payment(&[(&invoice, "75.00")]);
        let (review, _) = asked(&mut f, &input);
        change(&mut f, &invoice, &seen);
        let before = f.everything();
        refused(keyed(&mut f, &input, &review));
        assert!(f.everything() == before, "{what}: no journal, receipt, allocation, audit event or number was written");

        // A fresh review of the changed invoice records it.
        let (review, _) = asked(&mut f, &input);
        let counts = f.counts();
        let recorded = keyed(&mut f, &input, &review).unwrap();
        assert_eq!(recorded["amount"], "75.00", "{what}");
        assert_eq!(f.counts(), [counts[0] + 1, counts[1] + 1, counts[2] + 1, counts[3] + 1], "{what}");
        assert!(f.book.check().unwrap().is_empty(), "{what}");
    }
}

#[test]
fn with_the_key_and_nothing_changed_the_receipt_is_recorded_as_before() {
    let mut f = Books::new();
    let (invoice, _) = f.settled_invoice();
    let input = f.payment(&[(&invoice, "75.00")]);
    let (review, _) = asked(&mut f, &input);
    let plain_review = sales::review_receipt(&mut f.book, f.actor, f.company, &input).unwrap();
    let counts = f.counts();
    let recorded = keyed(&mut f, &input, &review).unwrap();
    assert_eq!(f.counts(), [counts[0] + 1, counts[1] + 1, counts[2] + 1, counts[3] + 1]);
    assert_eq!(f.outstanding(&invoice), "30.00");
    let stored = f.book.read(|q| sales::receipt(q, f.company, id(&recorded))).unwrap();
    assert_eq!(stored, recorded);
    assert_eq!((&recorded["kind"], &recorded["amount"], &recorded["reference"]), (&json!("RCP"), &json!("75.00"), &json!("Check 5521")));

    // The same receipt recorded without the key in an identical book differs only in what is
    // random or timed: the key changes nothing about the receipt itself.
    let mut plain = Books::new();
    let (other_invoice, _) = plain.settled_invoice();
    let other = plain.payment(&[(&other_invoice, "75.00")]);
    let other_review = sales::review_receipt(&mut plain.book, plain.actor, plain.company, &other).unwrap();
    let unkeyed = sales::record_receipt(&mut plain.book, plain.actor, plain.company, &other, Some(&other_review)).unwrap();
    let fields = |receipt: &Value| receipt.as_object().unwrap().iter()
        .filter(|(name, _)| !name.ends_with("id") && !name.ends_with("digest") && !name.ends_with("snapshot"))
        .map(|(name, value)| (name.clone(), value.clone())).collect::<Vec<_>>();
    assert_eq!(fields(&recorded), fields(&unkeyed));
    assert_eq!(recorded.as_object().unwrap().keys().collect::<Vec<_>>(), unkeyed.as_object().unwrap().keys().collect::<Vec<_>>());
    assert_eq!(plain.counts(), f.counts());
    assert_eq!(plain.count("ledger_auditevent"), f.count("ledger_auditevent"));

    // A repeat of the recorded request returns the same receipt and writes nothing, as without the
    // key: the refusal is reserved for a request that recorded nothing.
    let before = f.everything();
    let again = keyed(&mut f, &input, &review).unwrap();
    assert_eq!(again, recorded);
    // So does a repeat that names a review of the same plan made without asking: it matches the
    // recorded request, so it is answered with that receipt before the key is looked at.
    assert_eq!(keyed(&mut f, &input, &plain_review).unwrap(), recorded);
    assert!(f.everything() == before);
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn a_second_invoice_in_the_same_receipt_that_changed_refuses_it_too() {
    let mut f = Books::new();
    let first = f.invoice("200.00");
    let second = f.invoice("80.00");
    let input = f.payment(&[(&first, "75.00"), (&second, "20.00")]);
    let (review, _) = asked(&mut f, &input);
    f.receive(&second, "20.00");
    let before = f.everything();
    refused(keyed(&mut f, &input, &review));
    assert!(f.everything() == before);
    assert_eq!((f.outstanding(&first), f.outstanding(&second)), ("200.00".to_owned(), "60.00".to_owned()));
}

// Review finding 1: a 200.00 invoice, a 75.00 receipt reviewed, the same 75.00 entered natively,
// then the reviewed one recorded.
#[test]
fn the_reviewed_sequence_leaves_one_receipt_and_125_outstanding() {
    let mut f = Books::new();
    let invoice = f.invoice("200.00");
    let input = f.payment(&[(&invoice, "75.00")]);
    let (review, settlement) = asked(&mut f, &input);
    assert_eq!(settlement, json!([{"invoice_id":invoice,"total_cents":"20000","outstanding_cents":"20000","entries":[]}]));
    let native = f.receive(&invoice, "75.00");
    let before = f.everything();
    refused(keyed(&mut f, &input, &review));
    assert!(f.everything() == before);
    assert_eq!(f.count("sales_receipt"), 1);
    assert_eq!(f.outstanding(&invoice), "125.00");
    // The next review shows the receipt that stopped it.
    let (_, settlement) = asked(&mut f, &input);
    assert_eq!(settlement[0]["outstanding_cents"], "12500");
    assert_eq!(settlement[0]["entries"],
        json!([{"id":id(&native),"kind":"receipt","amount_cents":"7500","date":"2026-11-05","reversed":false}]));
}

// The outstanding amount alone is not the evidence: something entered and taken back again leaves
// the amount where it was, and the recording is still refused because the entries differ.
#[test]
fn an_entry_added_and_reversed_leaves_the_same_outstanding_and_still_refuses() {
    let undone: [(&str, Undone, &str); 3] = [
        ("a receipt added and reversed", |f, invoice| { let receipt = f.receive(invoice, "40.00"); f.reverse(&receipt); receipt }, "receipt"),
        ("a credit applied and unapplied", |f, invoice| { let credit = f.credit(invoice, "40.00"); f.unapply_credit(&credit); credit }, "credit"),
        ("a deposit applied and unapplied", |f, invoice| { let deposit = f.deposit(invoice, "40.00"); f.unapply_deposit(&deposit); deposit }, "deposit"),
    ];
    for (what, change, kind) in undone {
        let mut f = Books::new();
        let invoice = f.invoice("200.00");
        let input = f.payment(&[(&invoice, "75.00")]);
        let (review, listed) = asked(&mut f, &input);
        assert_eq!(listed, json!([{"invoice_id":invoice,"total_cents":"20000","outstanding_cents":"20000","entries":[]}]), "{what}");
        let entry = change(&mut f, &invoice);
        assert_eq!(f.outstanding(&invoice), "200.00", "{what}: the outstanding amount is where it was");
        let before = f.everything();
        refused(keyed(&mut f, &input, &review));
        assert!(f.everything() == before, "{what}");
        // The next review lists what happened, with the same outstanding amount, and records.
        let (review, listed) = asked(&mut f, &input);
        assert_eq!(listed[0]["outstanding_cents"], "20000", "{what}");
        assert_eq!(listed[0]["entries"].as_array().unwrap().len(), 1, "{what}");
        assert_eq!((&listed[0]["entries"][0]["id"], &listed[0]["entries"][0]["kind"], &listed[0]["entries"][0]["reversed"]),
            (&json!(id(&entry)), &json!(kind), &json!(true)), "{what}");
        assert_eq!(keyed(&mut f, &input, &review).unwrap()["amount"], "75.00", "{what}");
        assert_eq!(f.outstanding(&invoice), "125.00", "{what}");
        assert!(f.book.check().unwrap().is_empty(), "{what}");
    }
}

#[test]
fn the_key_cannot_be_satisfied_by_another_review_or_request() {
    let mut f = Books::new();
    let invoice = f.invoice("200.00");
    let other = f.invoice("200.00");
    let input = f.payment(&[(&invoice, "75.00")]);
    // A review of a different invoice lists a different settlement.
    let elsewhere = f.payment(&[(&other, "75.00")]);
    let (wrong, _) = asked(&mut f, &elsewhere);
    let before = f.everything();
    refused(keyed(&mut f, &input, &wrong));
    // No review at all is refused as it always was.
    let error = sales::record_receipt_same_settlement(&mut f.book, f.actor, f.company, &input, None, false).unwrap_err();
    assert_eq!(error.to_string(), FbError::validation("Invalid review. Open a fresh receipt review.").to_string());
    assert!(f.everything() == before);
    // An invoice of another company in the same book is not read: the refusal is the one a review
    // gives, whether the foreign invoice is the only allocation or sits beside one of this company.
    let foreign = f.other_company_invoice();
    let before = f.everything();
    let foreign_refusal = FbError::validation("Choose invoices belonging to this company.").to_string();
    for abroad in [f.payment(&[(&foreign, "75.00")]), f.payment(&[(&invoice, "75.00"), (&foreign, "75.00")])] {
        assert_eq!(sales::review_receipt(&mut f.book, f.actor, f.company, &abroad).unwrap_err().to_string(), foreign_refusal);
        assert_eq!(sales::review_receipt_form_with_settlement(&mut f.book, f.actor, f.company, &abroad, false).unwrap_err().to_string(), foreign_refusal);
        for review in [&wrong, &asked(&mut f, &input).0] {
            let error = keyed(&mut f, &abroad, review).unwrap_err();
            assert_eq!(error.to_string(), foreign_refusal);
        }
    }
    assert!(f.everything() == before);
    // A voided invoice is a changed one.
    let (review, _) = asked(&mut f, &input);
    sales::void_invoice(&mut f.book, f.actor, f.company, &invoice, Some(2), &json!("2026-11-06"), &json!("Synthetic correction")).unwrap();
    let before = f.everything();
    refused(keyed(&mut f, &input, &review));
    assert!(f.everything() == before);
}

// The key is honoured only for a review that asked for the settlement. A review made any other way
// carries none, so the key refuses it even though nothing changed: the guard is never skipped.
#[test]
fn a_review_that_did_not_ask_for_the_settlement_cannot_satisfy_the_key() {
    let mut f = Books::new();
    let (invoice, _) = f.settled_invoice();
    let input = f.payment(&[(&invoice, "75.00")]);
    let plain = sales::review_receipt(&mut f.book, f.actor, f.company, &input).unwrap();
    let (form, _) = sales::review_receipt_form(&mut f.book, f.actor, f.company, &input, false).unwrap();
    let before = f.everything();
    for review in [&plain, &form] {
        refused(keyed(&mut f, &input, review));
    }
    assert!(f.everything() == before, "nothing was written, with nothing changed on the invoice");
    // The same goes for a sales receipt, which has no invoice to compare.
    let sale = json!({"request_key":receipt_books::key(),"customer_id":f.customer,"date":"2026-11-05","method":"CASH","reference":"","memo":"",
        "destination":"BANK","bank_id":f.bank,"tax_mode":"NONE","lines":[{"item_id":null,"description":"Synthetic counter sale","quantity":"1","rate":"12.00","taxable":false,"income_account_id":f.income}]});
    let plain_sale = sales::review_sales_receipt(&mut f.book, f.actor, f.company, &sale).unwrap();
    refused(sales::record_receipt_same_settlement(&mut f.book, f.actor, f.company, &sale, Some(&plain_sale), true));
    assert!(f.everything() == before);
    // Without the key the very same reviews record, as they always did.
    let counts = f.counts();
    assert_eq!(sales::record_receipt(&mut f.book, f.actor, f.company, &input, Some(&plain)).unwrap()["amount"], "75.00");
    assert_eq!(sales::record_sales_receipt(&mut f.book, f.actor, f.company, &sale, Some(&plain_sale)).unwrap()["amount"], "12.00");
    assert_eq!(f.counts(), [counts[0] + 2, counts[1] + 2, counts[2] + 1, counts[3] + 1]);
    assert!(f.book.check().unwrap().is_empty());
}

// The stated limit. Fifty invoices, each with a share of ten receipts, are 500 entries: listed.
// One more entry on any of them is over the limit: the review still succeeds, lists nothing, and
// cannot satisfy the key.
#[test]
fn the_settlement_is_listed_up_to_the_limit_and_above_it_the_key_refuses() {
    assert_eq!(sales::SETTLEMENT_ENTRY_LIMIT, 500);
    let mut f = Books::new();
    let invoices: Vec<String> = (0..50).map(|_| f.invoice("100.00")).collect();
    let across = |f: &Books| f.payment(&invoices.iter().map(|invoice| (invoice.as_str(), "1.00")).collect::<Vec<_>>());
    for _ in 0..10 {
        let input = across(&f);
        let review = sales::review_receipt(&mut f.book, f.actor, f.company, &input).unwrap();
        sales::record_receipt(&mut f.book, f.actor, f.company, &input, Some(&review)).unwrap();
    }
    let entries = |settlement: &Value| settlement.as_array().unwrap().iter().map(|invoice| invoice["entries"].as_array().unwrap().len()).sum::<usize>();

    // Exactly at the limit: listed whole, and the key is honoured.
    let at_limit = across(&f);
    let (review_at_limit, listed) = asked(&mut f, &at_limit);
    assert_eq!((listed.as_array().unwrap().len(), entries(&listed)), (50, 500));
    assert!(listed.as_array().unwrap().iter().all(|invoice| invoice["outstanding_cents"] == "9000"));
    let second = across(&f);
    let (review, _) = asked(&mut f, &second);
    assert_eq!(keyed(&mut f, &second, &review).unwrap()["amount"], "50.00");

    // That receipt made 550 entries. Over the limit the review succeeds without a settlement.
    let over = across(&f);
    let (review, listed) = review_asking(&mut f, &over);
    assert_eq!(listed, None);
    let before = f.everything();
    refused(keyed(&mut f, &over, &review));
    // A review made at the limit does not pass once the invoices are over it.
    refused(keyed(&mut f, &at_limit, &review_at_limit));
    assert!(f.everything() == before, "nothing was written by either refusal");
    // The review above the limit is a whole review all the same: without the key it records.
    assert_eq!(sales::record_receipt(&mut f.book, f.actor, f.company, &over, Some(&review)).unwrap()["amount"], "50.00");

    // The limit is on the receipt's invoices together, and a smaller receipt is under it again:
    // one invoice of the fifty has 12 entries.
    let one = f.payment(&[(&invoices[0], "1.00")]);
    let (review, listed) = asked(&mut f, &one);
    assert_eq!(entries(&listed), 12);
    assert_eq!(keyed(&mut f, &one, &review).unwrap()["amount"], "1.00");
    assert!(f.book.check().unwrap().is_empty());
}

// 501 entries is the first count over the limit, on one invoice as on many.
#[test]
fn one_entry_over_the_limit_is_over() {
    let mut f = Books::new();
    let invoices: Vec<String> = (0..50).map(|_| f.invoice("100.00")).collect();
    for _ in 0..10 {
        let input = f.payment(&invoices.iter().map(|invoice| (invoice.as_str(), "1.00")).collect::<Vec<_>>());
        let review = sales::review_receipt(&mut f.book, f.actor, f.company, &input).unwrap();
        sales::record_receipt(&mut f.book, f.actor, f.company, &input, Some(&review)).unwrap();
    }
    let input = f.payment(&invoices.iter().map(|invoice| (invoice.as_str(), "1.00")).collect::<Vec<_>>());
    let (review, listed) = review_asking(&mut f, &input);
    assert!(listed.is_some(), "500 entries are listed");
    // One reversed receipt on the last invoice is the 501st entry: reversed entries are entries.
    let extra = f.receive(&invoices[49], "1.00");
    f.reverse(&extra);
    let before = f.everything();
    refused(keyed(&mut f, &input, &review));
    assert!(f.everything() == before);
    assert_eq!(review_asking(&mut f, &input).1, None, "501 entries are not");
}
