//! The report queries against documents recorded through their ordinary book services.
#[path = "common/receipt_books.rs"]
mod receipt_books;

use chrono::{Days, NaiveDate};
use fb_core::directory::{self, Model};
use fb_core::reports::subledgers::{AgingSummary, ap_aging, ar_aging, vendor_1099};
use fb_core::{
    Book, CompanyId, Money, UserId, checks, documents, payables, sales, services, users,
};
use receipt_books::{Books, id, key};
use serde_json::{Value, json};

fn day(value: &str) -> NaiveDate {
    value.parse().unwrap()
}

fn cents(value: i64) -> Money {
    Money::from_db(value)
}

fn outstanding(summary: &AgingSummary, document: &str) -> Option<Money> {
    summary
        .parties
        .iter()
        .flat_map(|party| &party.documents)
        .find(|row| row.id == document)
        .map(|row| row.balance)
}

fn invoice(f: &mut Books, date: &str, due: &str, posted: bool) -> String {
    let saved = sales::save_invoice(&mut f.book, f.actor, f.company, &json!({
        "request_key":key(),"customer_id":f.customer,"date":date,"due_date":due,"tax_mode":"NONE",
        "lines":[{"description":"Work","quantity":"1","rate":"12.34","taxable":false,"income_account_id":f.income}]
    })).unwrap();
    if posted {
        let review =
            sales::review_invoice(&mut f.book, f.actor, f.company, id(&saved), Some(1)).unwrap();
        sales::post_invoice(
            &mut f.book,
            f.actor,
            f.company,
            id(&saved),
            Some(1),
            Some(&review),
        )
        .unwrap();
    }
    id(&saved).to_owned()
}

#[test]
fn ar_ages_every_boundary_and_groups_customers() {
    let mut f = Books::new();
    let as_of = day("2026-11-30");
    for days in [0, 1, 30, 31, 60, 61, 90, 91] {
        let due = (as_of - Days::new(days)).to_string();
        invoice(&mut f, "2026-08-01", &due, true);
    }
    let original_customer = f.customer.clone();
    f.customer = id(&directory::save_record(
        &mut f.book,
        f.actor,
        f.company,
        Model::Customer,
        &json!({"display_name":"Another customer"}),
        None,
    )
    .unwrap())
    .to_owned();
    invoice(&mut f, "2026-11-30", "2026-12-01", true);
    invoice(&mut f, "2026-11-30", "2026-12-01", false);
    invoice(&mut f, "2026-12-01", "2026-12-01", true);
    f.other_company_invoice();
    let before = f.everything();
    let report = ar_aging(&mut f.book, f.actor, f.company, as_of).unwrap();
    let [another, original] = report.parties.as_slice() else {
        panic!("{report:?}")
    };
    assert_eq!(another.id, f.customer);
    assert_eq!(original.id, original_customer);
    assert_eq!(
        original.amounts.buckets.map(Money::cents),
        [1234, 2468, 2468, 2468, 1234]
    );
    assert_eq!(original.amounts.total, cents(9872));
    assert_eq!(
        report.amounts.buckets.map(Money::cents),
        [2468, 2468, 2468, 2468, 1234]
    );
    assert_eq!(report.amounts.total, cents(11106));
    assert_eq!(before, f.everything());
}

#[test]
fn ar_uses_effective_dates_for_payments_reversals_credits_deposits_and_voids() {
    let mut f = Books::new();
    let (invoice, [receipt, credit, deposit]) = f.settled_invoice();
    f.reverse(&receipt);
    f.unapply_credit(&credit);
    f.unapply_deposit(&deposit);
    let paid = f.invoice("11.01");
    f.receive(&paid, "11.01");
    let void = f.invoice("12.34");
    sales::void_invoice(
        &mut f.book,
        f.actor,
        f.company,
        &void,
        Some(2),
        &json!("2026-11-06"),
        &json!("Synthetic void"),
    )
    .unwrap();
    for (date, balance, paid_balance, void_balance) in [
        ("2026-10-31", None, None, None),
        ("2026-11-01", Some(20000), Some(1101), Some(1234)),
        ("2026-11-02", Some(17500), Some(1101), Some(1234)),
        ("2026-11-04", Some(13500), Some(1101), Some(1234)),
        ("2026-11-05", Some(10500), None, Some(1234)),
        ("2026-11-06", Some(20000), None, None),
    ] {
        let report = ar_aging(&mut f.book, f.actor, f.company, day(date)).unwrap();
        assert_eq!(outstanding(&report, &invoice), balance.map(cents), "{date}");
        assert_eq!(
            outstanding(&report, &paid),
            paid_balance.map(cents),
            "{date}"
        );
        assert_eq!(
            outstanding(&report, &void),
            void_balance.map(cents),
            "{date}"
        );
    }
    let outsider = users::create_user(&mut f.book, "No company access", false).unwrap();
    assert!(ar_aging(&mut f.book, outsider, f.company, day("2026-11-30")).is_err());
}

struct Payables {
    book: Book,
    actor: UserId,
    company: CompanyId,
    vendor: String,
    bank: String,
    expense: String,
    _file: fb_db::test_support::TempBookPath,
}

impl Payables {
    fn new() -> Self {
        let file = fb_db::test_support::TempBookPath::new("subledger-payables");
        let mut book = file.book(&[17; 32]);
        let actor = users::create_user(&mut book, "Payables tester", false).unwrap();
        let company =
            services::create_company(&mut book, actor, "Synthetic payables", "USD", false)
                .unwrap()
                .id;
        let mut account = |code, kind, detail| {
            services::save_account(
                &mut book,
                actor,
                company,
                services::AccountInput::new(code, code)
                    .kind(kind)
                    .detail(detail),
            )
            .unwrap()
            .id
            .to_db()
        };
        let bank = account("100", "ASSET", "BANK");
        let payable = account("200", "LIABILITY", "PAYABLE");
        let expense = account("600", "EXPENSE", "EXPENSE");
        documents::save_setup(
            &mut book,
            actor,
            company,
            &json!({"payables":payable,"banks":[bank]}),
            Some(0),
        )
        .unwrap();
        let vendor = id(&directory::save_record(
            &mut book,
            actor,
            company,
            Model::Vendor,
            &json!({"display_name":"Synthetic supplier","billing_address":"1 Example Road"}),
            None,
        )
        .unwrap())
        .to_owned();
        Self {
            book,
            actor,
            company,
            vendor,
            bank,
            expense,
            _file: file,
        }
    }

    fn bill(&mut self, date: &str, due: &str, total: &str, posted: bool) -> String {
        let saved = payables::save_bill(&mut self.book, self.actor, self.company, &json!({
            "request_key":key(),"vendor_id":self.vendor,"date":date,"due_date":due,
            "lines":[{"description":"Supplies","quantity":"1","rate":total,"expense_account_id":self.expense}]
        })).unwrap();
        if posted {
            let review = payables::review_bill(
                &mut self.book,
                self.actor,
                self.company,
                id(&saved),
                Some(1),
            )
            .unwrap();
            payables::post_bill(
                &mut self.book,
                self.actor,
                self.company,
                id(&saved),
                Some(1),
                &review,
            )
            .unwrap();
        }
        id(&saved).to_owned()
    }

    fn payment(&mut self, date: &str, total: &str, allocations: &[(&str, &str)]) -> Value {
        let allocations = allocations
            .iter()
            .map(|(bill, amount)| json!({"bill_id":bill,"amount":amount}))
            .collect::<Vec<_>>();
        let input = json!({"request_key":key(),"vendor_id":self.vendor,"date":date,"method":"CASH","amount":total,
            "bank_id":self.bank,"allocations":allocations});
        let review =
            payables::review_payment(&mut self.book, self.actor, self.company, &input).unwrap();
        payables::record_payment(&mut self.book, self.actor, self.company, &input, &review).unwrap()
    }

    fn check(&mut self, date: &str, amount: &str, vendor: bool, posted: bool) -> String {
        let mut input = json!({"request_key":key(),"bank_id":self.bank,"date":date,
            "lines":[{"tab":"EXPENSE","description":"Cost","rate":amount,"account_id":self.expense}]});
        if vendor {
            input["payee_kind"] = json!("VENDOR");
            input["vendor_id"] = json!(self.vendor);
        } else {
            input["payee_kind"] = json!("OTHER");
            input["payee_name"] = json!("Unlinked payee");
        }
        let saved = checks::save_check(&mut self.book, self.actor, self.company, &input).unwrap();
        if posted {
            let review = checks::review_check(
                &mut self.book,
                self.actor,
                self.company,
                id(&saved),
                Some(1),
            )
            .unwrap();
            checks::record_check(
                &mut self.book,
                self.actor,
                self.company,
                id(&saved),
                Some(1),
                &review,
            )
            .unwrap();
        }
        id(&saved).to_owned()
    }
}

#[test]
fn ap_ages_boundary_days_and_tracks_partial_payments_and_voids_as_of() {
    let mut f = Payables::new();
    let as_of = day("2026-11-30");
    let bills = [0, 1, 30, 31, 60, 61, 90, 91].map(|days| {
        let due = (as_of - Days::new(days)).to_string();
        f.bill("2026-08-01", &due, "12.34", true)
    });
    let paid = f.bill("2026-11-29", "2026-11-29", "3.01", true);
    let draft = f.bill("2026-11-29", "2026-11-29", "99.00", false);
    let future = f.bill("2026-12-01", "2026-12-01", "99.00", true);
    let payment = f.payment(
        "2026-11-30",
        "5.02",
        &[(&bills[2], "2.01"), (&paid, "3.01")],
    );
    payables::reverse_payment(
        &mut f.book,
        f.actor,
        f.company,
        id(&payment),
        Some(1),
        &json!("2026-12-01"),
        &json!("Synthetic reversal"),
    )
    .unwrap();
    payables::void_bill(
        &mut f.book,
        f.actor,
        f.company,
        &bills[6],
        Some(2),
        &json!("2026-12-01"),
        &json!("Synthetic void"),
    )
    .unwrap();
    let original_vendor = f.vendor.clone();
    f.vendor = id(&directory::save_record(
        &mut f.book,
        f.actor,
        f.company,
        Model::Vendor,
        &json!({"display_name":"Another vendor"}),
        None,
    )
    .unwrap())
    .to_owned();
    f.bill("2026-11-28", "2026-12-01", "4.56", true);
    let on = ap_aging(&mut f.book, f.actor, f.company, as_of).unwrap();
    let [other, original] = on.parties.as_slice() else {
        panic!("{on:?}")
    };
    assert_eq!(other.id, f.vendor);
    assert_eq!(original.id, original_vendor);
    assert_eq!(
        original.amounts.buckets.map(Money::cents),
        [1234, 2267, 2468, 2468, 1234]
    );
    assert_eq!(
        on.amounts.buckets.map(Money::cents),
        [1690, 2267, 2468, 2468, 1234]
    );
    assert_eq!(on.amounts.total, cents(10127));
    for missing in [&paid, &draft, &future] {
        assert_eq!(outstanding(&on, missing), None);
    }
    let before = ap_aging(&mut f.book, f.actor, f.company, day("2026-11-29")).unwrap();
    assert_eq!(outstanding(&before, &bills[2]), Some(cents(1234)));
    assert_eq!(outstanding(&before, &paid), Some(cents(301)));
    let after = ap_aging(&mut f.book, f.actor, f.company, day("2026-12-01")).unwrap();
    assert_eq!(outstanding(&after, &bills[2]), Some(cents(1234)));
    assert_eq!(outstanding(&after, &paid), Some(cents(301)));
    assert_eq!(outstanding(&after, &future), Some(cents(9900)));
    assert_eq!(outstanding(&after, &bills[6]), None);
}

#[test]
fn vendor_1099_counts_payments_once_and_filters_dates_drafts_voids_and_payee_kind() {
    let mut f = Payables::new();
    let a = f.bill("2025-12-30", "2025-12-30", "100.00", true);
    let b = f.bill("2025-12-30", "2025-12-30", "100.00", true);
    f.payment("2025-12-31", "2.22", &[(&a, "2.22")]);
    f.payment("2026-01-01", "30.03", &[(&a, "10.01"), (&b, "20.02")]);
    f.check("2026-12-31", "3.33", true, true);
    f.check("2027-01-01", "99.00", true, true);
    f.check("2026-05-01", "99.00", true, false);
    f.check("2026-05-01", "99.00", false, true);
    let check_same_year = f.check("2026-04-01", "11.00", true, true);
    checks::void_check(
        &mut f.book,
        f.actor,
        f.company,
        &check_same_year,
        Some(2),
        &json!("2026-04-05"),
        &json!("Same-year void"),
    )
    .unwrap();
    let payment_same_year = f.payment("2026-04-02", "12.00", &[(&a, "12.00")]);
    payables::reverse_payment(
        &mut f.book,
        f.actor,
        f.company,
        id(&payment_same_year),
        Some(1),
        &json!("2026-04-06"),
        &json!("Same-year reversal"),
    )
    .unwrap();
    // Cross-year void/reversal: stays in 2026 on its payment date and offsets in 2027 on its effective date.
    let check = f.check("2026-05-01", "99.00", true, true);
    checks::void_check(
        &mut f.book,
        f.actor,
        f.company,
        &check,
        Some(2),
        &json!("2027-01-01"),
        &json!("Synthetic void"),
    )
    .unwrap();
    let payment = f.payment("2026-05-01", "50.00", &[(&a, "50.00")]);
    payables::reverse_payment(
        &mut f.book,
        f.actor,
        f.company,
        id(&payment),
        Some(1),
        &json!("2027-01-01"),
        &json!("Synthetic reversal"),
    )
    .unwrap();
    // A print-later check that is later numbered has two RECORDED revisions but must still count once.
    let draft_numbered = checks::save_check(
        &mut f.book,
        f.actor,
        f.company,
        &json!({
            "request_key": key(),
            "bank_id": f.bank,
            "payee_kind": "VENDOR",
            "vendor_id": f.vendor,
            "date": "2026-07-01",
            "print_later": true,
            "lines": [{"tab": "EXPENSE", "account_id": f.expense, "description": "Numbered check", "rate": "7.00"}]
        }),
    )
    .unwrap();
    let rev = checks::review_check(&mut f.book, f.actor, f.company, id(&draft_numbered), Some(1))
        .unwrap();
    let rec = checks::record_check(
        &mut f.book,
        f.actor,
        f.company,
        id(&draft_numbered),
        Some(1),
        &rev,
    )
    .unwrap();
    checks::assign_number(
        &mut f.book,
        f.actor,
        f.company,
        id(&rec),
        Some(2),
        &json!("1042"),
    )
    .unwrap();
    let vendor = f.vendor.clone();
    f.vendor = id(&directory::save_record(
        &mut f.book,
        f.actor,
        f.company,
        Model::Vendor,
        &json!({"display_name":"Different vendor"}),
        None,
    )
    .unwrap())
    .to_owned();
    f.check("2026-06-01", "0.04", true, true);
    let report = vendor_1099(
        &mut f.book,
        f.actor,
        f.company,
        day("2026-01-01"),
        day("2026-12-31"),
    )
    .unwrap();
    let [other, original] = report.vendors.as_slice() else {
        panic!("{report:?}")
    };
    assert_eq!(
        (&other.id, other.count, other.total),
        (&f.vendor, 1, cents(4))
    );
    // 2026 original vendor: 30.03 + 3.33 + 99.00 + 50.00 + 7.00 = 189.36 (5 net payments; same-year void/reversal netted to 0).
    assert_eq!(
        (&original.id, original.count, original.total),
        (&vendor, 5, cents(18936))
    );
    assert_eq!(original.address, "1 Example Road");
    assert_eq!((report.count, report.total), (6, cents(18940)));
    // 2027 carries the 99.00 new check plus -99.00 void and -50.00 reversal = -50.00 (-1 net count).
    let next_year = vendor_1099(
        &mut f.book,
        f.actor,
        f.company,
        day("2027-01-01"),
        day("2027-12-31"),
    )
    .unwrap();
    let [v2027] = next_year.vendors.as_slice() else {
        panic!("{next_year:?}")
    };
    assert_eq!((&v2027.id, v2027.count, v2027.total), (&vendor, -1, cents(-5000)));
    // Spanning 2026..=2027 equals the exact sum of the two annual periods.
    let combined = vendor_1099(
        &mut f.book,
        f.actor,
        f.company,
        day("2026-01-01"),
        day("2027-12-31"),
    )
    .unwrap();
    assert_eq!(combined.count, report.count + next_year.count);
    assert_eq!(combined.total, report.total + next_year.total);
    assert_eq!(
        vendor_1099(
            &mut f.book,
            f.actor,
            f.company,
            day("2025-12-31"),
            day("2025-12-31")
        )
        .unwrap()
        .total,
        cents(222)
    );
    // A vendor whose net dollars in the period equal $0.00 (e.g. -$100 prior-year reversal + two $50 checks,
    // net count +1) must be omitted from the 1099 summary.
    let zero_net_vendor = id(&directory::save_record(
        &mut f.book,
        f.actor,
        f.company,
        Model::Vendor,
        &json!({"display_name":"Zero net vendor"}),
        None,
    )
    .unwrap())
    .to_owned();
    let saved_vendor = f.vendor.clone();
    f.vendor = zero_net_vendor.clone();
    let old_chk = f.check("2026-12-20", "100.00", true, true);
    checks::void_check(
        &mut f.book,
        f.actor,
        f.company,
        &old_chk,
        Some(2),
        &json!("2027-01-05"),
        &json!("Reversal in next year"),
    )
    .unwrap();
    f.check("2027-02-01", "50.00", true, true);
    f.check("2027-02-02", "50.00", true, true);
    f.vendor = saved_vendor;
    let y2027_with_zero_net = vendor_1099(
        &mut f.book,
        f.actor,
        f.company,
        day("2027-01-01"),
        day("2027-12-31"),
    )
    .unwrap();
    assert!(
        !y2027_with_zero_net
            .vendors
            .iter()
            .any(|v| v.id == zero_net_vendor),
        "zero-net vendor should be omitted: {y2027_with_zero_net:?}"
    );
    let empty = services::create_company(&mut f.book, f.actor, "Empty company", "USD", false)
        .unwrap()
        .id;
    assert!(
        vendor_1099(
            &mut f.book,
            f.actor,
            empty,
            day("2026-01-01"),
            day("2026-12-31")
        )
        .unwrap()
        .vendors
        .is_empty()
    );
    let outsider = users::create_user(&mut f.book, "No payable access", false).unwrap();
    assert!(ap_aging(&mut f.book, outsider, f.company, day("2026-11-30")).is_err());
    assert!(
        vendor_1099(
            &mut f.book,
            outsider,
            f.company,
            day("2026-01-01"),
            day("2026-12-31")
        )
        .is_err()
    );
}

#[test]
fn aging_retains_settlement_residual_when_void_predates_settlement_reversal() {
    // A/R: invoice posted Nov 1, receipt Nov 10, receipt reversed effective Nov 30, invoice voided effective Nov 15.
    let mut r = Books::new();
    let inv = r.invoice("100.00");
    let receipt_input = json!({
        "request_key": key(),
        "customer_id": r.customer,
        "date": "2026-11-10",
        "method": "CASH",
        "destination": "BANK",
        "bank_id": r.bank,
        "amount": "100.00",
        "allocations": [{"invoice_id": inv, "amount": "100.00"}]
    });
    let receipt_review = sales::review_receipt(&mut r.book, r.actor, r.company, &receipt_input).unwrap();
    let receipt = sales::record_receipt(&mut r.book, r.actor, r.company, &receipt_input, Some(&receipt_review)).unwrap();
    sales::reverse_receipt(
        &mut r.book,
        r.actor,
        r.company,
        id(&receipt),
        Some(1),
        &json!("2026-11-30"),
        &json!("Late reversal"),
    )
    .unwrap();
    sales::void_invoice(
        &mut r.book,
        r.actor,
        r.company,
        &inv,
        Some(2),
        &json!("2026-11-15"),
        &json!("Void before reversal date"),
    )
    .unwrap();
    let mid_ar = ar_aging(&mut r.book, r.actor, r.company, day("2026-11-20")).unwrap();
    assert_eq!(outstanding(&mid_ar, &inv), Some(cents(-10000)));
    assert_eq!(mid_ar.amounts.total, cents(-10000));
    let end_ar = ar_aging(&mut r.book, r.actor, r.company, day("2026-11-30")).unwrap();
    assert_eq!(outstanding(&end_ar, &inv), None);
    assert_eq!(end_ar.amounts.total, cents(0));

    // A/P: bill posted Nov 1, payment Nov 10, payment reversed effective Nov 30, bill voided effective Nov 15.
    let mut p = Payables::new();
    let bill = p.bill("2026-11-01", "2026-11-01", "100.00", true);
    let payment = p.payment("2026-11-10", "100.00", &[(&bill, "100.00")]);
    payables::reverse_payment(
        &mut p.book,
        p.actor,
        p.company,
        id(&payment),
        Some(1),
        &json!("2026-11-30"),
        &json!("Late reversal"),
    )
    .unwrap();
    payables::void_bill(
        &mut p.book,
        p.actor,
        p.company,
        &bill,
        Some(2),
        &json!("2026-11-15"),
        &json!("Void before reversal date"),
    )
    .unwrap();
    let mid_ap = ap_aging(&mut p.book, p.actor, p.company, day("2026-11-20")).unwrap();
    assert_eq!(outstanding(&mid_ap, &bill), Some(cents(-10000)));
    assert_eq!(mid_ap.amounts.total, cents(-10000));
    let end_ap = ap_aging(&mut p.book, p.actor, p.company, day("2026-11-30")).unwrap();
    assert_eq!(outstanding(&end_ap, &bill), None);
    assert_eq!(end_ap.amounts.total, cents(0));
}
