//! Dedicated Expenses destination, through real screen handlers and synthetic encrypted books.
mod support;
use chrono::NaiveDate;
use fb_app::{App, Outcome, commands::parse_command, identity::Role, keys, screens};
use fb_core::{
    AccountId, CompanyId,
    services::{self, AccountInput, DraftInput, line},
};
use serde_json::{Value, json};

struct Fixture {
    _dir: tempfile::TempDir,
    app: App,
    c: CompanyId,
    bank: AccountId,
    expense: AccountId,
}
fn command(app: &mut App, value: Value) -> Value {
    match app.run(parse_command(&value.to_string()).unwrap()).unwrap() {
        Outcome::Done { data } => data,
        other => panic!("{other:?}"),
    }
}
fn account(app: &mut App, c: CompanyId, code: &str, name: &str, detail: &str) -> AccountId {
    let o = app.open_mut().unwrap();
    services::save_account(
        &mut o.book,
        o.owner,
        c,
        AccountInput::new(code, name).detail(detail),
    )
    .unwrap()
    .id
}
fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new();
    app.today = || NaiveDate::from_ymd_opt(2026, 11, 15).unwrap();
    let c = app
        .create_book(
            &dir.path().join("book.fbbook"),
            &keys::new_master_key().unwrap(),
            "owner",
            "Synthetic expense company",
        )
        .unwrap()
        .company_id;
    support::sign_in(&mut app);
    let bank = account(&mut app, c, "1000", "Bank", "BANK");
    let expense = account(&mut app, c, "6000", "Supplies <&>", "EXPENSE");
    Fixture {
        _dir: dir,
        app,
        c,
        bank,
        expense,
    }
}
fn page(f: &mut Fixture, query: &str) -> screens::Rendered {
    screens::render(&mut f.app, &format!("/c/{}/expenses/?{query}", f.c)).unwrap()
}
fn total(html: &str) -> &str {
    html.split_once("data-expense-total>")
        .unwrap()
        .1
        .split(" <small>")
        .next()
        .unwrap()
}
fn audit(app: &mut App) -> i64 {
    app.open_mut()
        .unwrap()
        .book
        .read(|tx| tx.query_row("SELECT count(*) FROM ledger_auditevent", [], |r| r.get(0)))
        .unwrap()
}
fn journal(
    f: &mut Fixture,
    account: AccountId,
    amount: &str,
    date: &str,
    memo: &str,
    posted: bool,
) -> fb_core::JournalId {
    let o = f.app.open_mut().unwrap();
    let j = services::save_draft(
        &mut o.book,
        o.owner,
        f.c,
        DraftInput::new(
            date,
            memo,
            vec![
                line(account).debit(amount).credit("0").build(),
                line(f.bank).debit("0").credit(amount).build(),
            ],
        ),
    )
    .unwrap();
    if posted {
        services::post_journal(&mut o.book, o.owner, f.c, j.id, None).unwrap();
    }
    j.id
}
#[test]
fn destination_defaults_to_this_month_and_highlights_expenses() {
    let mut f = fixture();
    let before = audit(&mut f.app);
    let p = page(&mut f, "");
    assert_eq!(p.status, 200);
    assert_eq!(total(&p.html), "0.00");
    assert!(p.html.contains("November 1 – 30, 2026"));
    assert!(p.html.contains("No posted expenses match these filters"));
    assert!(p.html.contains("Record check / expense") && p.html.contains("Add bill"));
    assert_eq!(
        p.html.contains("Review bank transactions"),
        cfg!(any(feature = "simplefin", feature = "plugins"))
    );
    let nav = p
        .html
        .split_once("<nav class=\"appnav\"")
        .unwrap()
        .1
        .split_once("</nav>")
        .unwrap()
        .0;
    let selected = nav
        .split(" aria-current=\"page\"")
        .skip(1)
        .map(|s| {
            s.split("<span>")
                .nth(1)
                .unwrap()
                .split("</span>")
                .next()
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(selected, ["Expenses"]);
    assert_eq!(audit(&mut f.app), before);
}
#[test]
fn totals_are_exact_period_activity_with_credits_and_archived_categories() {
    let mut f = fixture();
    let expense = f.expense;
    journal(
        &mut f,
        expense,
        "1000.00",
        "2026-10-31",
        "Prior period",
        true,
    );
    let j = journal(
        &mut f,
        expense,
        "10.25",
        "2026-11-01",
        "Original expense <script>",
        true,
    );
    let o = f.app.open_mut().unwrap();
    services::reverse_journal(
        &mut o.book,
        o.owner,
        f.c,
        j,
        "2026-11-02",
        "Synthetic reversal",
    )
    .unwrap();
    journal(
        &mut f,
        expense,
        "13.37",
        "2026-11-03",
        "Retained expense <script>",
        true,
    );
    journal(
        &mut f,
        expense,
        "99.00",
        "2026-11-04",
        "UNPOSTED draft",
        false,
    );
    let cogs = account(&mut f.app, f.c, "5000", "Materials", "COST_OF_GOODS_SOLD");
    let other = account(&mut f.app, f.c, "7000", "Other costs", "OTHER_EXPENSE");
    let asset = account(&mut f.app, f.c, "1500", "Equipment", "FIXED_ASSET");
    journal(&mut f, cogs, "2.11", "2026-11-05", "Materials bought", true);
    journal(&mut f, other, "1.02", "2026-11-06", "Other cost", true);
    journal(
        &mut f,
        asset,
        "555.00",
        "2026-11-07",
        "Asset purchase",
        true,
    );
    journal(
        &mut f,
        expense,
        "75.00",
        "2026-12-01",
        "Future period",
        true,
    );
    let o = f.app.open_mut().unwrap();
    services::save_account(
        &mut o.book,
        o.owner,
        f.c,
        AccountInput::new("6000", "Supplies <&>")
            .detail("EXPENSE")
            .edit(expense)
            .active(false),
    )
    .unwrap();
    let before = audit(&mut f.app);
    let p = page(&mut f, "");
    assert_eq!(total(&p.html), "16.50");
    assert_eq!(p.html.matches("data-expense-entry").count(), 5);
    for excluded in [
        "Prior period",
        "UNPOSTED draft",
        "Asset purchase",
        "Future period",
    ] {
        assert!(!p.html.contains(excluded), "{excluded}");
    }
    assert!(p.html.contains("Supplies &lt;&amp;&gt; (archived)"));
    assert!(
        p.html.contains("Retained expense &lt;script&gt;")
            && !p.html.contains("Retained expense <script>")
    );
    assert!(p.html.contains(&format!("/c/{}/journals/{j}/", f.c)));
    assert!(
        p.html.find("Other cost</td>").unwrap()
            < p.html.find("Retained expense &lt;script&gt;</td>").unwrap()
    );
    assert_eq!(audit(&mut f.app), before);
}
fn bill(f: &mut Fixture, vendor: &str, date: &str, amount: &str) -> String {
    let data = command(
        &mut f.app,
        json!({"type":"SaveBill","company_id":f.c.to_db(),"values":{"bill_id":null,"expected_revision":0,"request_key":fb_core::JournalId::new().to_db(),"vendor_id":vendor,"date":date,"due_date":date,"memo":"Synthetic bill","reference":"","supersedes_id":null,"reentry_reason":"","refresh_vendor":false,"keep_archived":false,"lines":[{"origin_position":null,"item_id":null,"expense_account_id":f.expense.to_db(),"description":"Supplies","quantity":"1","rate":amount,"refresh_item":false,"keep_archived":false}]}}),
    );
    let id = data["id"].as_str().unwrap().to_owned();
    let review = command(
        &mut f.app,
        json!({"type":"ReviewBill","company_id":f.c.to_db(),"bill_id":id,"expected_revision":1}),
    );
    command(
        &mut f.app,
        json!({"type":"PostBill","company_id":f.c.to_db(),"bill_id":id,"expected_revision":1,"review_token":review["token"]}),
    );
    id
}
fn payment(f: &mut Fixture, vendor: &str, bill: &str, date: &str, amount: &str) -> String {
    let values = json!({"request_key":fb_core::JournalId::new().to_db(),"vendor_id":vendor,"date":date,"amount":amount,"method":"OTHER","reference":"","memo":"Synthetic payment","bank_id":f.bank.to_db(),"allocations":[{"bill_id":bill,"amount":amount}]});
    let review = command(
        &mut f.app,
        json!({"type":"ReviewBillPayment","company_id":f.c.to_db(),"values":values}),
    );
    command(&mut f.app, json!({"type":"RecordBillPayment","company_id":f.c.to_db(),"values":values,"review_token":review["token"]}))["id"].as_str().unwrap().to_owned()
}
#[test]
fn bills_and_partial_payments_share_the_report_recognition_without_double_counting() {
    let mut f = fixture();
    let ap = account(&mut f.app, f.c, "2000", "Payables", "PAYABLE");
    command(
        &mut f.app,
        json!({"type":"SaveSetup","company_id":f.c.to_db(),"expected_revision":0,"receivables_id":null,"undeposited_id":null,"payables_id":ap.to_db(),"bank_ids":[f.bank.to_db()]}),
    );
    let o = f.app.open_mut().unwrap();
    let vendor = fb_core::directory::save_record(
        &mut o.book,
        o.owner,
        f.c,
        fb_core::directory::Model::Vendor,
        &json!({"display_name":"Synthetic Supplier"}),
        None,
    )
    .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let id = bill(&mut f, &vendor, "2026-10-15", "100.01");
    let pay = payment(&mut f, &vendor, &id, "2026-11-10", "60.01");
    payment(&mut f, &vendor, &id, "2026-12-10", "40.00");
    let unpaid = bill(&mut f, &vendor, "2026-11-12", "49.99");
    let p = page(&mut f, "basis=ACCRUAL");
    assert_eq!(total(&p.html), "49.99");
    assert!(p.html.contains(&format!("/bills/{unpaid}/")));
    let p = page(&mut f, "basis=CASH");
    assert_eq!(total(&p.html), "60.01");
    assert!(p.html.contains("Cash-basis search matches the displayed payment recognition memo"));
    assert!(p.html.contains("Synthetic Supplier") && p.html.contains(&format!("/payments/{pay}/")));
    for (basis, expected) in [("ACCRUAL", "150.00"), ("CASH", "100.01")] {
        let p = page(
            &mut f,
            &format!("preset=custom&start=2026-10-01&end=2026-12-31&basis={basis}"),
        );
        assert_eq!(total(&p.html), expected);
    }
    command(
        &mut f.app,
        json!({"type":"ReverseBillPayment","company_id":f.c.to_db(),"payment_id":pay,"expected_revision":1,"date":"2026-11-20","reason":"Synthetic correction"}),
    );
    assert_eq!(total(&page(&mut f, "basis=CASH").html), "0.00");
    assert_eq!(
        total(
            &page(
                &mut f,
                "preset=custom&start=2026-11-20&end=2026-11-20&basis=CASH"
            )
            .html
        ),
        "-60.01"
    );
    for basis in [
        fb_core::reports::financial::Basis::Accrual,
        fb_core::reports::financial::Basis::Cash,
    ] {
        let rendered = page(&mut f, &format!("basis={}", basis.as_str()));
        let open = f.app.open_mut().unwrap();
        let report = fb_core::reports::financial::profit_and_loss(
            &mut open.book,
            open.owner,
            f.c,
            basis,
            Some(NaiveDate::from_ymd_opt(2026, 11, 1).unwrap()),
            NaiveDate::from_ymd_opt(2026, 11, 30).unwrap(),
        )
        .unwrap();
        let expected = report.section("cogs").unwrap().total
            + report.section("expense").unwrap().total
            + report.section("other_expense").unwrap().total;
        assert_eq!(total(&rendered.html), expected.to_string());
    }
}
#[test]
fn category_search_and_paging_keep_totals_and_filters_for_all_matches() {
    let mut f = fixture();
    for count in [30, 21] {
        let mut lines = (0..count)
            .map(|_| line(f.expense).debit("0.01").credit("0").build())
            .collect::<Vec<_>>();
        lines.push(
            line(f.bank)
                .debit("0")
                .credit(if count == 30 { "0.30" } else { "0.21" })
                .build(),
        );
        let o = f.app.open_mut().unwrap();
        let j = services::save_draft(
            &mut o.book,
            o.owner,
            f.c,
            DraftInput::new("2026-11-01", "Match & cents", lines),
        )
        .unwrap();
        services::post_journal(&mut o.book, o.owner, f.c, j.id, None).unwrap();
    }
    let other = account(&mut f.app, f.c, "7000", "Other", "OTHER_EXPENSE");
    journal(&mut f, other, "200.00", "2026-11-02", "Not searched", true);
    let query = format!("account={}&q=maTch%20%26%20cents&basis=CASH", f.expense);
    let p = page(&mut f, &query);
    assert_eq!(total(&p.html), "0.51");
    assert_eq!(p.html.matches("data-expense-entry").count(), 50);
    assert!(p.html.contains("Page 1 of 2") && p.html.contains("page=2"));
    assert!(
        p.html.contains(&format!("account={}", f.expense)) && p.html.contains("q=maTch+%26+cents")
    );
    let p = page(&mut f, &format!("{query}&page=2"));
    assert_eq!(total(&p.html), "0.51");
    assert_eq!(p.html.matches("data-expense-entry").count(), 1);
    assert!(!p.html.contains("Not searched"));
    assert_eq!(total(&page(&mut f, "q=missing").html), "0.00");
}
#[test]
fn invalid_filters_and_posts_do_not_mutate_the_book_or_fall_back_to_all_accounts() {
    let mut f = fixture();
    let other_company = {
        let o = f.app.open_mut().unwrap();
        services::create_company(&mut o.book, o.owner, "Other", "USD", false)
            .unwrap()
            .id
    };
    let foreign = account(
        &mut f.app,
        other_company,
        "6000",
        "FOREIGN SECRET CATEGORY",
        "EXPENSE",
    );
    let before = audit(&mut f.app);
    for query in [
        format!("account={foreign}"),
        format!("account={}", f.bank),
        "account=garbage".into(),
        "end=not-a-date".into(),
        "basis=invalid".into(),
        "preset=custom&start=2026-12-01&end=2026-11-01".into(),
        format!("q={}", "x".repeat(201)),
    ] {
        let p = page(&mut f, &query);
        assert_eq!(p.status, 400, "{query}");
        assert!(
            !p.html.contains("data-expense-total") && !p.html.contains("FOREIGN SECRET CATEGORY")
        );
    }
    let p = screens::submit(&mut f.app, &format!("/c/{}/expenses/", f.c), &[]).unwrap();
    assert!(matches!(p, Outcome::Page { status: 405, .. }));
    assert_eq!(audit(&mut f.app), before);
}
#[test]
fn readonly_can_track_expenses_but_cannot_create_or_read_an_ungranted_company() {
    let mut f = fixture();
    let expense = f.expense;
    journal(
        &mut f,
        expense,
        "12.34",
        "2026-11-01",
        "Allowed expense",
        true,
    );
    let other = {
        let o = f.app.open_mut().unwrap();
        services::create_company(&mut o.book, o.owner, "Private company", "USD", false)
            .unwrap()
            .id
    };
    f.app
        .save_desktop_user(
            None,
            "Synthetic viewer",
            Role::Readonly,
            true,
            Some("SyntheticViewerPassword42"),
            &[f.c],
        )
        .unwrap();
    f.app
        .sign_in("Synthetic viewer", "SyntheticViewerPassword42")
        .unwrap();
    let p = page(&mut f, "");
    assert_eq!(total(&p.html), "12.34");
    assert!(!p.html.contains("Record check / expense") && !p.html.contains("Add bill"));
    assert!(screens::render(&mut f.app, &format!("/c/{other}/expenses/")).is_err());
    assert!(screens::submit(&mut f.app, &format!("/c/{}/expenses/", f.c), &[]).is_err());
    f.app.sign_out();
    assert_eq!(page(&mut f, "").html, "/desktop/sign-in/");
}
