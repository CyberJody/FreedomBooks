//! Exact report reconciliation against synthetic documents and journals.
#[path = "common/receipt_books.rs"]
mod receipt_books;

use chrono::NaiveDate;
use fb_core::{
    AccountId, FbError, Money, banking, credits,
    directory::{self, Model},
    documents,
    periods::{self, PeriodChange},
    prepayments,
    reports::{
        asset_depreciation_schedule::{self as asset_report, AssetInput, Method, Values},
        cash_flow::{Activity, cash_flow},
        comparative_profit_and_loss::{Periods, comparative_profit_and_loss, variance_percent},
        customer_statement::customer_statement,
        financial::{Basis, profit_and_loss},
        sales_by_customer_item::{Group, sales_by_customer_item},
        schedule_c_activity::{self as sc, Activity as TaxActivity, Category, Mapping, Share, Split, Target, Totals},
        subledgers::ar_aging,
    },
    sales, salestax, services, transfers, users,
};
use fb_db::rusqlite::params;
use receipt_books::{Books, id, key};
use serde_json::{Value, json};

fn day(date: &str) -> NaiveDate {
    date.parse().unwrap()
}

fn money(cents: i64) -> Money {
    Money::from_db(cents)
}

fn posted_invoice(f: &mut Books, date: &str, lines: Value, tax: Option<&str>) -> String {
    let saved = sales::save_invoice(
        &mut f.book,
        f.actor,
        f.company,
        &json!({
            "request_key":key(),"customer_id":f.customer,"date":date,
            "tax_mode":if tax.is_some() {"ITEM"} else {"NONE"},"tax_item_id":tax,"lines":lines
        }),
    )
    .unwrap();
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
    id(&saved).into()
}

fn account(f: &mut Books, code: &str, kind: &str, detail: &str) -> AccountId {
    services::save_account(
        &mut f.book,
        f.actor,
        f.company,
        services::AccountInput::new(code, code)
            .kind(kind)
            .detail(detail),
    )
    .unwrap()
    .id
}

fn journal(f: &mut Books, date: &str, memo: &str, lines: Vec<Value>) -> fb_core::JournalId {
    let draft = services::save_draft(
        &mut f.book,
        f.actor,
        f.company,
        services::DraftInput::new(date, memo, lines),
    )
    .unwrap();
    services::post_journal(&mut f.book, f.actor, f.company, draft.id, Some(1))
        .unwrap()
        .id
}

fn unapplied_deposit(f: &mut Books) {
    let deposit = json!({"request_key":key(),"customer_id":f.customer,"date":"2026-11-05","amount":"17.03","method":"CASH","destination":"BANK","bank_id":f.bank});
    let review = prepayments::review_deposit(&mut f.book, f.actor, f.company, &deposit).unwrap();
    prepayments::record_deposit(&mut f.book, f.actor, f.company, Some(&review), &deposit).unwrap();
}

#[test]
fn customer_statement_uses_aging_cutoffs_and_credit_allocations_are_neutral() {
    let mut f = Books::new();
    let (_, [receipt, credit, deposit]) = f.settled_invoice();
    f.reverse(&receipt);
    f.unapply_credit(&credit);
    f.unapply_deposit(&deposit);
    unapplied_deposit(&mut f);
    f.other_company_invoice();
    let before = f.everything();
    for (as_of, opening, closing, liability) in [
        ("2026-11-03", 17500, 13500, 0),
        ("2026-11-04", 17500, 13500, 0),
        ("2026-11-05", 17500, 10500, 1703),
        ("2026-11-06", 17500, 16000, 4203),
    ] {
        let report = customer_statement(
            &mut f.book,
            f.actor,
            f.company,
            &f.customer,
            day("2026-11-03"),
            day(as_of),
        )
        .unwrap();
        let aging = ar_aging(&mut f.book, f.actor, f.company, day(as_of)).unwrap();
        assert_eq!(report.opening_ar, money(opening));
        assert_eq!(report.opening_deposits, Money::ZERO);
        assert_eq!(report.closing_ar, money(closing));
        assert_eq!(report.closing_deposits, money(liability));
        assert_eq!(report.opening_receivables.open_invoices, money(17500));
        assert_eq!(report.opening_receivables.unapplied_credits, Money::ZERO);
        let unapplied = if matches!(as_of, "2026-11-03" | "2026-11-06") {
            -4000
        } else {
            0
        };
        assert_eq!(
            report.closing_receivables.unapplied_credits,
            money(unapplied)
        );
        assert_eq!(
            report.closing_receivables.open_invoices,
            money(closing - unapplied)
        );
        assert_eq!(
            aging.amounts.buckets.into_iter().sum::<Money>(),
            money(closing - unapplied)
        );
        assert_eq!(
            aging.amounts.receivables.unapplied_credits,
            money(unapplied)
        );
        assert_eq!(aging.amounts.total, report.closing_ar);
        assert_eq!(
            report.opening_ar + report.lines.iter().map(|line| line.ar_delta).sum::<Money>(),
            report.closing_ar
        );
        for line in report
            .lines
            .iter()
            .filter(|line| line.activity.starts_with("credit "))
        {
            assert!(line.ar_delta.is_zero());
            assert_eq!(
                line.credit_allocation,
                if line.activity == "credit applied" {
                    money(4000)
                } else {
                    money(-4000)
                }
            );
        }
        if as_of >= "2026-11-04" {
            assert!(
                report
                    .lines
                    .iter()
                    .any(|line| line.activity == "credit applied")
            );
        }
        if as_of == "2026-11-06" {
            assert!(
                report
                    .lines
                    .iter()
                    .any(|line| line.activity == "receipt reversal")
            );
            assert!(
                report
                    .lines
                    .iter()
                    .any(|line| line.activity == "deposit unapplied")
            );
        }
    }
    assert_eq!(f.everything(), before);
    let foreign: String = f
        .book
        .read(|tx| {
            tx.query_row(
                "SELECT id FROM directory_customer WHERE company_id<>?1",
                [f.company.to_db()],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert!(matches!(
        customer_statement(
            &mut f.book,
            f.actor,
            f.company,
            &foreign,
            day("2026-11-01"),
            day("2026-11-30")
        ),
        Err(FbError::DoesNotExist(_))
    ));
    let empty = directory::save_record(
        &mut f.book,
        f.actor,
        f.company,
        Model::Customer,
        &json!({"display_name":"No activity"}),
        None,
    )
    .unwrap();
    let report = customer_statement(
        &mut f.book,
        f.actor,
        f.company,
        id(&empty),
        day("2026-11-01"),
        day("2026-11-30"),
    )
    .unwrap();
    assert!(report.lines.is_empty());
    assert_eq!(
        (
            report.opening_ar,
            report.closing_ar,
            report.closing_deposits
        ),
        (Money::ZERO, Money::ZERO, Money::ZERO)
    );
}

#[test]
fn customer_statement_preserves_void_before_receipt_reversal_residual() {
    let mut f = Books::new();
    let invoice = f.invoice("100.01");
    let receipt = f.receive(&invoice, "42.03");
    sales::reverse_receipt(
        &mut f.book,
        f.actor,
        f.company,
        id(&receipt),
        Some(1),
        &json!("2026-11-30"),
        &json!("Returned payment"),
    )
    .unwrap();
    sales::void_invoice(
        &mut f.book,
        f.actor,
        f.company,
        &invoice,
        Some(2),
        &json!("2026-11-15"),
        &json!("Invoice cancelled"),
    )
    .unwrap();
    let mid = customer_statement(
        &mut f.book,
        f.actor,
        f.company,
        &f.customer,
        day("2026-11-10"),
        day("2026-11-20"),
    )
    .unwrap();
    assert_eq!(mid.opening_ar, money(5798));
    assert_eq!(mid.lines.len(), 1);
    assert_eq!(mid.lines[0].activity, "Invoice void");
    assert_eq!(mid.lines[0].ar_balance, money(-4203));
    let close = customer_statement(
        &mut f.book,
        f.actor,
        f.company,
        &f.customer,
        day("2026-11-20"),
        day("2026-11-30"),
    )
    .unwrap();
    assert_eq!(close.opening_ar, money(-4203));
    assert_eq!(close.lines[0].ar_delta, money(4203));
    assert_eq!(close.closing_ar, Money::ZERO);
}

#[test]
fn direct_cash_flow_reconciles_banks_cards_assets_debt_equity_splits_and_reversals() {
    let mut f = Books::new();
    let bank = AccountId::parse(&f.bank).unwrap();
    let other_bank = account(&mut f, "105", "ASSET", "BANK");
    let legacy_bank = account(&mut f, "106", "ASSET", "OTHER_CURRENT_ASSET");
    let mut setup = f
        .book
        .read(|tx| fb_core::documents::setup_snapshot(tx, f.company))
        .unwrap();
    setup["banks"]
        .as_array_mut()
        .unwrap()
        .push(json!(legacy_bank.to_db()));
    fb_core::documents::save_setup(
        &mut f.book,
        f.actor,
        f.company,
        &setup,
        setup["revision"].as_i64(),
    )
    .unwrap();
    let card = account(&mut f, "201", "LIABILITY", "CREDIT_CARD");
    let debt = account(&mut f, "250", "LIABILITY", "LONG_TERM_LIABILITY");
    let equity = account(&mut f, "300", "EQUITY", "EQUITY");
    let asset = account(&mut f, "155", "ASSET", "FIXED_ASSET");
    let expense = account(&mut f, "600", "EXPENSE", "EXPENSE");
    let entry = |f: &mut Books, date, memo, debit, credit, amount| {
        journal(
            f,
            date,
            memo,
            vec![
                services::line(debit).debit(amount).build(),
                services::line(credit).credit(amount).build(),
            ],
        )
    };
    entry(&mut f, "2026-10-31", "Opening cash", bank, equity, "100.01");
    let transfer = |f: &mut Books, date, from: AccountId, to: AccountId, amount, memo| {
        let fields = json!({"request_key":key(),"date":date,"from_account_id":from.to_db(),"to_account_id":to.to_db(),"amount":amount,"memo":memo});
        let review = transfers::review_transfer(&mut f.book, f.actor, f.company, &fields).unwrap();
        transfers::record_transfer(&mut f.book, f.actor, f.company, &fields, &review).unwrap()
    };
    transfer(&mut f, "2026-11-01", bank, other_bank, "20.00", "Internal");
    transfer(&mut f, "2026-11-01", bank, legacy_bank, "10.11", "Internal");
    let payment = transfer(&mut f, "2026-11-02", bank, card, "5.03", "Card payment");
    entry(&mut f, "2026-11-03", "Loan proceeds", bank, debt, "50.00");
    entry(&mut f, "2026-11-04", "Loan repayment", debt, bank, "10.01");
    entry(&mut f, "2026-11-05", "Contribution", bank, equity, "70.00");
    entry(&mut f, "2026-11-06", "Draw", equity, bank, "3.00");
    entry(&mut f, "2026-11-07", "Asset purchase", asset, bank, "12.34");
    entry(&mut f, "2026-11-08", "Asset sale", bank, asset, "6.01");
    entry(&mut f, "2026-11-09", "Bank fees", expense, bank, "0.69");
    entry(&mut f, "2026-11-10", "Noncash", asset, debt, "40.00");
    let split = journal(
        &mut f,
        "2026-11-11",
        "Mixed purpose",
        vec![
            services::line(bank).debit("8.55").build(),
            services::line(equity).credit("6.53").build(),
            services::line(asset).credit("2.02").build(),
        ],
    );
    transfers::void_transfer(
        &mut f.book,
        f.actor,
        f.company,
        id(&payment),
        Some(1),
        &json!("2026-11-20"),
        &json!("Reversed bank payment"),
    )
    .unwrap();
    services::save_account(
        &mut f.book,
        f.actor,
        f.company,
        services::AccountInput::new("105", "105")
            .kind("ASSET")
            .detail("BANK")
            .edit(other_bank)
            .active(false)
            .postable(false),
    )
    .unwrap();

    let start = day("2026-11-01");
    let before = cash_flow(&mut f.book, f.actor, f.company, start, day("2026-11-19")).unwrap();
    assert_eq!(before.opening, money(10001));
    assert_eq!(before.net, [-572, -633, 10699, 855].map(money));
    assert_eq!(before.closing, money(20350));
    assert_eq!(before.rows.len(), 9);
    assert!(
        before
            .rows
            .iter()
            .any(|line| line.category == "Credit card payments" && line.amount == money(-503))
    );
    assert!(before.rows.iter().any(|line| line.journal == split.to_db()
        && line.activity == Activity::Unclassified
        && line.amount == money(855)));
    let after = cash_flow(&mut f.book, f.actor, f.company, start, day("2026-11-30")).unwrap();
    assert_eq!(after.net, [-69, -633, 10699, 855].map(money));
    assert_eq!(
        after.opening + after.net.into_iter().sum::<Money>(),
        after.closing
    );
    assert_eq!(after.closing, money(20853));
    assert!(
        after
            .rows
            .iter()
            .all(|line| line.memo != "Noncash" && !line.memo.ends_with("Internal"))
    );
}

#[test]
fn cash_flow_excludes_bank_typed_receivables_after_the_control_role_changes() {
    let mut f = Books::new();
    let ar = account(&mut f, "111", "ASSET", "BANK").to_db();
    let mut setup = f
        .book
        .read(|tx| documents::setup_snapshot(tx, f.company))
        .unwrap();
    let original_ar = setup["receivables"].clone();
    setup["receivables"] = json!(ar);
    setup["banks"]
        .as_array_mut()
        .unwrap()
        .retain(|bank| *bank != ar);
    let setup = documents::save_setup(
        &mut f.book,
        f.actor,
        f.company,
        &setup,
        setup["revision"].as_i64(),
    )
    .unwrap();
    f.invoice("100.00");

    for current in [true, false] {
        if !current {
            let mut replacement = setup.clone();
            replacement["receivables"] = original_ar.clone();
            documents::save_setup(
                &mut f.book,
                f.actor,
                f.company,
                &replacement,
                setup["revision"].as_i64(),
            )
            .unwrap();
        }
        for start in ["2026-11-01", "2026-11-02"] {
            let report = cash_flow(
                &mut f.book,
                f.actor,
                f.company,
                day(start),
                day("2026-11-30"),
            )
            .unwrap();
            assert_eq!(report.opening, Money::ZERO);
            assert_eq!(report.closing, Money::ZERO);
            assert_eq!(report.net, [Money::ZERO; 4]);
            assert!(report.rows.is_empty());
        }
    }
}

#[test]
fn cash_flow_retains_posted_legacy_bank_balances_after_designation_is_removed() {
    let mut f = Books::new();
    let legacy = account(&mut f, "106", "ASSET", "OTHER_CURRENT_ASSET");
    let equity = account(&mut f, "300", "EQUITY", "EQUITY");
    let mut setup = f
        .book
        .read(|tx| documents::setup_snapshot(tx, f.company))
        .unwrap();
    setup["banks"]
        .as_array_mut()
        .unwrap()
        .push(json!(legacy.to_db()));
    let mut setup = documents::save_setup(
        &mut f.book,
        f.actor,
        f.company,
        &setup,
        setup["revision"].as_i64(),
    )
    .unwrap();
    for (date, amount) in [("2026-10-31", "10.03"), ("2026-11-02", "12.04")] {
        journal(
            &mut f,
            date,
            "Legacy bank funding",
            vec![
                services::line(legacy).debit(amount).build(),
                services::line(equity).credit(amount).build(),
            ],
        );
    }
    setup["banks"] = json!([f.bank]);
    let setup = documents::save_setup(
        &mut f.book,
        f.actor,
        f.company,
        &setup,
        setup["revision"].as_i64(),
    )
    .unwrap();
    assert_eq!(setup["banks"], json!([f.bank]));

    let report = cash_flow(
        &mut f.book,
        f.actor,
        f.company,
        day("2026-11-01"),
        day("2026-11-30"),
    )
    .unwrap();
    assert_eq!(report.opening, money(1003));
    assert_eq!(report.net, [0, 0, 1204, 0].map(money));
    assert_eq!(report.closing, money(2207));
    assert_eq!(
        report.opening + report.net.into_iter().sum::<Money>(),
        report.closing
    );
}

#[test]
fn cash_flow_splits_gross_deposit_receipts_fees_and_unclassified_adjustments() {
    let mut f = Books::new();
    let fee = account(&mut f, "600", "EXPENSE", "EXPENSE");
    let adjustment = account(&mut f, "155", "ASSET", "FIXED_ASSET");
    let adjustment2 = account(&mut f, "156", "ASSET", "FIXED_ASSET");
    let bank = f.bank.clone();
    let invoice = f.invoice("300.00");
    for (date, reversal_date, adjustments, expected_unclassified) in [
        (
            "2026-11-05",
            "2026-11-06",
            json!([{"account_id":fee.to_db(),"amount":"-3.00"}]),
            vec![],
        ),
        (
            "2026-11-07",
            "2026-11-08",
            json!([
                {"account_id":fee.to_db(),"amount":"-3.00"},
                {"account_id":adjustment.to_db(),"amount":"-1.01"}
            ]),
            vec![-101],
        ),
        (
            "2026-11-09",
            "2026-11-10",
            json!([
                {"account_id":fee.to_db(),"amount":"-3.00"},
                {"account_id":adjustment.to_db(),"amount":"5.00"},
                {"account_id":adjustment2.to_db(),"amount":"-5.00"}
            ]),
            vec![-500, 500],
        ),
    ] {
        let residual: i64 = expected_unclassified.iter().sum();
        let mut payment = f.payment(&[(&invoice, "100.00")]);
        payment["date"] = json!(date);
        let recorded = banking::record_receipt_net(
            &mut f.book,
            f.actor,
            f.company,
            &payment,
            &bank,
            &adjustments,
        )
        .unwrap();
        let deposit = &recorded["deposit"];
        let reversed = banking::reverse_deposit(
            &mut f.book,
            f.actor,
            f.company,
            id(deposit),
            Some(1),
            &json!(reversal_date),
            &json!("Synthetic deposit reversal"),
        )
        .unwrap();

        for (date, sign, opening, closing, journal) in [
            (date, 1, 0, 9700 + residual, &deposit["journal_id"]),
            (
                reversal_date,
                -1,
                9700 + residual,
                0,
                &reversed["journal_id"],
            ),
        ] {
            let report = cash_flow(&mut f.book, f.actor, f.company, day(date), day(date)).unwrap();
            assert_eq!(report.net, [sign * 9700, 0, 0, sign * residual].map(money));
            assert_eq!(
                (report.opening, report.closing),
                (money(opening), money(closing))
            );
            assert_eq!(
                report.opening + report.net.into_iter().sum::<Money>(),
                report.closing
            );
            assert!(
                report
                    .rows
                    .iter()
                    .all(|row| row.journal == journal.as_str().unwrap())
            );
            for (activity, category, amount) in [
                (Activity::Operating, "Customer receipts deposited", 10000),
                (Activity::Operating, "Processing and bank fees", -300),
            ] {
                assert_eq!(
                    report
                        .rows
                        .iter()
                        .find(|row| row.activity == activity && row.category == category)
                        .map(|row| row.amount),
                    Some(money(amount * sign)),
                );
            }
            let mut got_unclassified: Vec<Money> = report
                .rows
                .iter()
                .filter(|row| row.activity == Activity::Unclassified && row.category == "Review counterparties")
                .map(|row| row.amount)
                .collect();
            got_unclassified.sort();
            let mut want_unclassified: Vec<Money> = expected_unclassified.iter().map(|v| money(v * sign)).collect();
            want_unclassified.sort();
            assert_eq!(got_unclassified, want_unclassified);
        }
    }
}

#[test]
fn comparative_months_quarters_and_fiscal_years_match_every_standalone_column() {
    let mut f = Books::new();
    f.book
        .write(|tx| {
            tx.execute(
        "INSERT INTO desktop_user(user_id,role,password_hash) VALUES(?1,'administrator',?2)",
        params![f.actor.to_db(), format!("$argon2id$v=19${}", "synthetic".repeat(10))],
    )
        })
        .unwrap();
    periods::set_policy(
        &mut f.book,
        f.actor,
        f.company,
        0,
        PeriodChange {
            fiscal_year_start_month: 4,
            closing_date: None,
        },
        None,
    )
    .unwrap();
    let line = json!([{"description":"Work","quantity":"1","rate":"123.45","taxable":false,"income_account_id":f.income}]);
    posted_invoice(&mut f, "2026-11-03", line.clone(), None);
    let leap = posted_invoice(&mut f, "2028-02-29", line, None);
    let mut receipt = f.payment(&[(&leap, "23.45")]);
    receipt["date"] = json!("2028-03-01");
    let review = sales::review_receipt(&mut f.book, f.actor, f.company, &receipt).unwrap();
    sales::record_receipt(&mut f.book, f.actor, f.company, &receipt, Some(&review)).unwrap();
    f.other_company_invoice();

    for basis in [Basis::Accrual, Basis::Cash] {
        for periods in [Periods::Monthly, Periods::Quarterly, Periods::FiscalYears] {
            let report = comparative_profit_and_loss(
                &mut f.book,
                f.actor,
                f.company,
                basis,
                periods,
                day("2027-04-01"),
                day("2028-03-31"),
            )
            .unwrap();
            assert_eq!(
                report.columns.len(),
                match periods {
                    Periods::Monthly => 13,
                    Periods::Quarterly => 5,
                    Periods::FiscalYears => 2,
                }
            );
            for (index, column) in report.columns.iter().enumerate() {
                let expected = profit_and_loss(
                    &mut f.book,
                    f.actor,
                    f.company,
                    basis,
                    column.report.start,
                    column.report.end,
                )
                .unwrap();
                assert_eq!(column.report, expected);
                for row in report.rows.iter().filter(|row| row.account.is_some()) {
                    let account = row.account.as_ref().unwrap();
                    let found = expected
                        .sections
                        .iter()
                        .flat_map(|s| &s.rows)
                        .find(|r| r.account.id == account.id);
                    assert_eq!(
                        row.amounts[index],
                        found.map(|r| r.amount).unwrap_or_default()
                    );
                }
            }
            let period_amount = if basis == Basis::Accrual {
                money(12345)
            } else {
                money(2345)
            };
            match periods {
                Periods::Monthly => {
                    assert_eq!(report.columns[10].report.end, day("2028-02-29"));
                    assert_eq!(
                        report.columns[if basis == Basis::Accrual { 10 } else { 11 }]
                            .report
                            .net_income,
                        period_amount
                    );
                    assert_eq!(report.columns[12].report.net_income, period_amount);
                }
                Periods::Quarterly => {
                    assert!(report.columns[3].label.starts_with("Q4"));
                    assert_eq!(report.columns[3].report.start, Some(day("2028-01-01")));
                    assert_eq!(report.columns[3].report.net_income, period_amount);
                }
                Periods::FiscalYears => {
                    assert_eq!(report.columns[0].report.start, Some(day("2027-04-01")));
                    assert_eq!(report.columns[1].report.end, day("2027-03-31"));
                }
            }
        }
    }
    for (period, start) in [
        (Periods::Monthly, "2027-03-31"),
        (Periods::Quarterly, "2025-03-31"),
    ] {
        let refusal = comparative_profit_and_loss(
            &mut f.book,
            f.actor,
            f.company,
            Basis::Accrual,
            period,
            day(start),
            day("2028-03-31"),
        )
        .unwrap_err();
        assert!(refusal.to_string().contains("no more than 12 periods"));
    }
    assert_eq!(
        variance_percent(money(101), money(300)).as_deref(),
        Some("-66.33%")
    );
    assert_eq!(variance_percent(money(101), Money::ZERO), None);
}

#[test]
fn sales_use_document_lines_tax_exclusion_snapshot_groups_and_effective_reversals() {
    let mut f = Books::new();
    let tax_account = account(&mut f, "215", "LIABILITY", "OTHER_CURRENT_LIABILITY");
    let mut setup = f
        .book
        .read(|tx| fb_core::documents::setup_snapshot(tx, f.company))
        .unwrap();
    setup["collected_tax"] = json!(tax_account.to_db());
    fb_core::documents::save_setup(&mut f.book, f.actor, f.company, &setup, Some(1)).unwrap();
    let tax = salestax::save_rate(&mut f.book, f.actor, f.company, &json!({
        "name":"Example tax","description":"Example tax","rate":"6","new_agency_name":"Example jurisdiction","expected_revision":0
    })).unwrap();
    salestax::save_settings(&mut f.book, f.actor, f.company, &json!({
        "charge_tax":true,"default_item_id":tax["id"],"owed_basis":"ACCRUAL","filing_frequency":"QUARTERLY","expected_revision":0
    })).unwrap();
    let item = directory::save_record(&mut f.book, f.actor, f.company, Model::Item,
        &json!({"code":"Original item","kind":"SERVICE","sale_rate":"1.25","income_account":f.income}), None).unwrap();
    let inv_lines = json!([
        {"item_id":id(&item),"description":"Product","quantity":"2","rate":"50.05","taxable":true,"income_account_id":f.income},
        {"item_id":id(&item),"line_kind":"DISCOUNT","description":"Item discount","quantity":"1","rate":"2.01","taxable":true,"income_account_id":f.income},
        {"line_kind":"DISCOUNT","description":"Document discount","quantity":"1","rate":"8.04","taxable":true,"income_account_id":f.income},
        {"description":"Other income","quantity":"1","rate":"20.01","taxable":false,"income_account_id":f.income}
    ]);
    let inv = posted_invoice(&mut f, "2026-11-01", inv_lines.clone(), Some(id(&tax)));
    let payment = f.receive(&inv, "4.01");
    f.reverse(&payment);

    directory::save_record(
        &mut f.book,
        f.actor,
        f.company,
        Model::Item,
        &json!({"code":"Renamed item"}),
        Some(id(&item)),
    )
    .unwrap();
    directory::save_record(
        &mut f.book,
        f.actor,
        f.company,
        Model::Customer,
        &json!({"display_name":"Renamed customer"}),
        Some(&f.customer),
    )
    .unwrap();
    let sale_input = json!({"request_key":key(),"customer_id":f.customer,"date":"2026-11-07","method":"CASH","destination":"BANK","bank_id":f.bank,
        "tax_mode":"ITEM","tax_item_id":tax["id"],"lines":[{"item_id":id(&item),"description":"Receipt sale","quantity":"1.25","rate":"8.00","taxable":true,"income_account_id":f.income}]});
    let review = sales::review_sales_receipt(&mut f.book, f.actor, f.company, &sale_input).unwrap();
    let sale =
        sales::record_sales_receipt(&mut f.book, f.actor, f.company, &sale_input, Some(&review))
            .unwrap();
    let memo_input = json!({"request_key":key(),"customer_id":f.customer,"date":"2026-11-08","tax_mode":"ITEM","tax_item_id":tax["id"],
        "lines":[{"item_id":id(&item),"description":"Return","quantity":"0.5","rate":"8.00","taxable":true,"income_account_id":f.income}]});
    let review = credits::review_credit_memo(&mut f.book, f.actor, f.company, &memo_input).unwrap();
    let memo =
        credits::record_credit_memo(&mut f.book, f.actor, f.company, &memo_input, Some(&review))
            .unwrap();
    sales::save_estimate(
        &mut f.book,
        f.actor,
        f.company,
        &json!({
            "request_key":key(),"customer_id":f.customer,"date":"2026-11-12","tax_mode":"NONE",
            "lines":[{"description":"Estimate only","quantity":"1","rate":"700.00","taxable":false}]
        }),
    )
    .unwrap();
    let bank = AccountId::parse(&f.bank).unwrap();
    let income = AccountId::parse(&f.income).unwrap();
    journal(
        &mut f,
        "2026-11-09",
        "Manual income",
        vec![
            services::line(bank).debit("25.02").build(),
            services::line(income).credit("25.02").build(),
        ],
    );
    sales::void_invoice(
        &mut f.book,
        f.actor,
        f.company,
        &inv,
        Some(2),
        &json!("2026-11-20"),
        &json!("Cancelled order"),
    )
    .unwrap();
    sales::void_sales_receipt(
        &mut f.book,
        f.actor,
        f.company,
        id(&sale),
        Some(1),
        &json!("2026-11-21"),
        &json!("Voided sale"),
    )
    .unwrap();
    credits::void_credit_memo(
        &mut f.book,
        f.actor,
        f.company,
        id(&memo),
        Some(1),
        &json!("2026-11-22"),
        &json!("Credit withdrawn"),
    )
    .unwrap();
    directory::set_active(
        &mut f.book,
        f.actor,
        f.company,
        Model::Item,
        id(&item),
        false,
    )
    .unwrap();
    directory::set_active(
        &mut f.book,
        f.actor,
        f.company,
        Model::Customer,
        &f.customer,
        false,
    )
    .unwrap();
    f.other_company_invoice();

    for (group, count) in [
        (Group::Customer, 1),
        (Group::Item, 3),
        (Group::CustomerItem, 3),
    ] {
        let report = sales_by_customer_item(
            &mut f.book,
            f.actor,
            f.company,
            day("2026-11-01"),
            day("2026-11-19"),
            group,
        )
        .unwrap();
        assert_eq!(report.document_events, 3);
        assert_eq!(report.rows.len(), count);
        assert_eq!(report.totals.gross, money(13011));
        assert_eq!(report.totals.adjustments, money(-1405));
        assert_eq!(report.totals.net(), money(11606));
        assert_eq!(report.document_net, report.totals.net());
        assert_eq!(report.totals.quantity_text(), "3.75");
        assert_eq!(report.manual_income, money(2502));
        assert_eq!(report.manual_income_lines, 1);
        if group != Group::Customer {
            assert!(
                report
                    .rows
                    .iter()
                    .any(|r| r.item == "Unassigned discounts" && r.amounts.net() == money(-804))
            );
            assert!(
                report
                    .rows
                    .iter()
                    .any(|r| r.item == "Unassigned item" && r.amounts.net() == money(2001))
            );
            assert!(
                report
                    .rows
                    .iter()
                    .any(|r| r.item_id.as_deref() == Some(id(&item))
                        && r.item == "Renamed item (archived)")
            );
        }
        if group != Group::Item {
            assert!(
                report
                    .rows
                    .iter()
                    .all(|r| r.customer_id.as_deref() == Some(f.customer.as_str())
                        && r.customer == "Renamed customer (archived)")
            );
        }
    }
    let original = sales_by_customer_item(
        &mut f.book,
        f.actor,
        f.company,
        day("2026-11-01"),
        day("2026-11-01"),
        Group::CustomerItem,
    )
    .unwrap();
    assert!(
        original
            .rows
            .iter()
            .all(|row| row.customer == "Synthetic Orchard (archived)")
    );
    assert!(
        original
            .rows
            .iter()
            .any(|row| row.item == "Original item (archived)")
    );
    let reversal = sales_by_customer_item(
        &mut f.book,
        f.actor,
        f.company,
        day("2026-11-20"),
        day("2026-11-22"),
        Group::Item,
    )
    .unwrap();
    assert_eq!(reversal.document_net, money(-11606));
    assert_eq!(reversal.totals.quantity_text(), "-3.75");
    let whole = sales_by_customer_item(
        &mut f.book,
        f.actor,
        f.company,
        day("2026-11-01"),
        day("2026-11-30"),
        Group::Item,
    )
    .unwrap();
    assert_eq!(whole.document_net, Money::ZERO);
    assert_eq!(whole.document_events, 6);
    assert_eq!(whole.totals.quantity_text(), "0");

    let outsider = users::create_user(&mut f.book, "Outsider", false).unwrap();
    let (start, end) = (day("2026-11-01"), day("2026-11-30"));
    assert!(
        sales_by_customer_item(&mut f.book, outsider, f.company, start, end, Group::Item).is_err()
    );
    assert!(customer_statement(&mut f.book, outsider, f.company, &f.customer, start, end).is_err());
    assert!(cash_flow(&mut f.book, outsider, f.company, start, end).is_err());
    assert!(
        comparative_profit_and_loss(
            &mut f.book,
            outsider,
            f.company,
            Basis::Accrual,
            Periods::Monthly,
            start,
            end
        )
        .is_err()
    );
}

#[test]
fn customer_picker_search_folds_non_ascii_case() {
    let mut f = Books::new();
    let elodie = directory::save_record(
        &mut f.book,
        f.actor,
        f.company,
        directory::Model::Customer,
        &json!({"display_name":"Élodie Studio"}),
        None,
    )
    .unwrap();
    let elodie_id = id(&elodie).to_owned();
    let picker = fb_core::reports::customer_statement::customers(
        &mut f.book,
        f.actor,
        f.company,
        "élodie",
        "",
    )
    .unwrap();
    assert!(
        picker.choices.iter().any(|c| c.id == elodie_id && c.name == "Élodie Studio"),
        "expected non-ASCII case-insensitive match for Élodie Studio, got {:?}",
        picker.choices
    );
}

fn entry(
    f: &mut Books,
    date: &str,
    dr: AccountId,
    cr: AccountId,
    amount: &str,
) -> fb_core::JournalId {
    journal(
        f,
        date,
        "Synthetic activity",
        vec![
            services::line(dr).debit(amount).build(),
            services::line(cr).credit(amount).build(),
        ],
    )
}

fn split(activity: &str, category: Category, share: Share) -> Split {
    Split {
        activity: TaxActivity::Business(activity.into()),
        category: Some(category),
        share,
    }
}

fn mapping(account: AccountId, splits: Vec<Split>) -> Mapping {
    Mapping {
        target: Target::Account(account),
        splits,
    }
}

fn totals(income: i64, expenses: i64) -> Totals {
    Totals {
        income: money(income),
        expenses: money(expenses),
    }
}

#[test]
fn fy2025_schedule_c_reconciles_exact_activity_allocations_on_each_basis() {
    let mut f = Books::new();
    let bank = AccountId::parse(&f.bank).unwrap();
    let tech = AccountId::parse(&f.income).unwrap();
    let rideshare = account(&mut f, "401", "INCOME", "INCOME");
    let personal = account(&mut f, "490", "INCOME", "OTHER_INCOME");
    let office = account(&mut f, "618", "EXPENSE", "EXPENSE");
    let supplies = account(&mut f, "619", "EXPENSE", "EXPENSE");
    let misc = account(&mut f, "620", "EXPENSE", "OTHER_EXPENSE");
    let unpaid = account(&mut f, "491", "INCOME", "INCOME");
    entry(&mut f, "2025-01-01", bank, tech, "30000");
    entry(&mut f, "2025-12-31", bank, rideshare, "14968.70");
    entry(&mut f, "2025-08-12", bank, personal, "64.38");
    entry(&mut f, "2025-04-03", office, bank, "10.01");
    entry(&mut f, "2025-04-04", supplies, bank, "4.23");
    let legacy_payable = account(&mut f, "212", "LIABILITY", "PAYABLE");
    entry(&mut f, "2025-04-05", misc, legacy_payable, "2.06");
    let mut setup = f
        .book
        .read(|tx| documents::setup_snapshot(tx, f.company))
        .unwrap();
    setup["payables"] = json!(legacy_payable.to_db());
    documents::save_setup(
        &mut f.book,
        f.actor,
        f.company,
        &setup,
        setup["revision"].as_i64(),
    )
    .unwrap();
    entry(&mut f, "2026-01-01", bank, tech, "20000");
    let invoice_lines = |account: AccountId, amount| {
        json!([
            {"description":"Synthetic invoice","quantity":"1","rate":amount,"taxable":false,"income_account_id":account.to_db()}
        ])
    };
    let invoice = posted_invoice(&mut f, "2025-10-01", invoice_lines(tech, "74.50"), None);
    posted_invoice(&mut f, "2025-12-03", invoice_lines(unpaid, "11.03"), None);
    let mut receipt = f.payment(&[(&invoice, "74.50")]);
    receipt["date"] = json!("2025-10-10");
    let review = sales::review_receipt(&mut f.book, f.actor, f.company, &receipt).unwrap();
    let receipt =
        sales::record_receipt(&mut f.book, f.actor, f.company, &receipt, Some(&review)).unwrap();
    let settlement = fb_core::JournalId::parse(receipt["journal_id"].as_str().unwrap()).unwrap();
    f.other_company_invoice();

    let mappings = [
        mapping(
            tech,
            vec![split(
                "Tech consulting",
                Category::GrossReceipts,
                Share::Weight(1),
            )],
        ),
        mapping(
            rideshare,
            vec![split(
                "Rideshare",
                Category::GrossReceipts,
                Share::Weight(1),
            )],
        ),
        mapping(
            personal,
            vec![Split {
                activity: TaxActivity::Excluded,
                category: Some(Category::ExcludedPersonal),
                share: Share::Weight(1),
            }],
        ),
        mapping(
            office,
            vec![
                split("Tech consulting", Category::Office, Share::Weight(1)),
                split("Rideshare", Category::Office, Share::Weight(1)),
            ],
        ),
        mapping(
            supplies,
            vec![
                split("Tech consulting", Category::Supplies, Share::Cents(101)),
                split("Rideshare", Category::Supplies, Share::Cents(322)),
            ],
        ),
    ];
    let (start, end) = (day("2025-01-01"), day("2025-12-31"));
    let before = f.everything();
    for (basis, unassigned_income) in [(Basis::Accrual, 1103), (Basis::Cash, 0)] {
        let report = sc::schedule_c_activity(
            &mut f.book,
            f.actor,
            f.company,
            basis,
            start,
            end,
            &mappings,
        )
        .unwrap();
        assert_eq!(
            report.groups[&TaxActivity::Business("Tech consulting".into())].totals,
            totals(3007450, 602)
        );
        assert_eq!(
            report.groups[&TaxActivity::Business("Rideshare".into())].totals,
            totals(1496870, 822)
        );
        assert_eq!(
            report.groups[&TaxActivity::Excluded].totals,
            totals(6438, 0)
        );
        assert_eq!(
            report.groups[&TaxActivity::Unassigned].totals,
            totals(unassigned_income, 206)
        );
        let pl = profit_and_loss(&mut f.book, f.actor, f.company, basis, Some(start), end).unwrap();
        let sum = |keys: &[&str]| {
            keys.iter()
                .map(|key| pl.section(key).unwrap().total)
                .sum::<Money>()
        };
        assert_eq!(
            report.allocated,
            Totals {
                income: sum(&["income", "other_income"]),
                expenses: sum(&["cogs", "expense", "other_expense"]),
            }
        );
        assert_eq!(report.book, report.allocated);
        assert_eq!(
            report
                .groups
                .values()
                .map(|g| g.totals.net())
                .sum::<Money>(),
            pl.net_income
        );
        assert_eq!(report.unconverted, pl.unconverted);
        assert_eq!(report.unconverted, i64::from(basis == Basis::Cash));
        let blank =
            sc::schedule_c_activity(&mut f.book, f.actor, f.company, basis, start, end, &[])
                .unwrap();
        assert_eq!(blank.groups.len(), 1);
        assert_eq!(blank.groups[&TaxActivity::Unassigned].rows.len(), 7);
        assert_eq!(blank.groups[&TaxActivity::Unassigned].totals, report.book);
    }
    let settlement_mapping = Mapping {
        target: Target::Journal(settlement),
        splits: vec![split(
            "Settlement override",
            Category::GrossReceipts,
            Share::Cents(7450),
        )],
    };
    let mut override_mappings = mappings.to_vec();
    override_mappings.push(settlement_mapping);
    let report = sc::schedule_c_activity(
        &mut f.book,
        f.actor,
        f.company,
        Basis::Cash,
        start,
        end,
        &override_mappings,
    )
    .unwrap();
    assert_eq!(
        report.groups[&TaxActivity::Business("Settlement override".into())].totals,
        totals(7450, 0)
    );
    assert_eq!(
        report.groups[&TaxActivity::Business("Tech consulting".into())].totals,
        totals(3000000, 602)
    );
    assert_eq!(report.allocated, report.book);
    assert_eq!(f.everything(), before);
}

#[test]
fn schedule_c_overrides_use_selected_book_postings_and_validate_shares_and_ids() {
    let mut f = Books::new();
    let bank = AccountId::parse(&f.bank).unwrap();
    let income = AccountId::parse(&f.income).unwrap();
    let j = entry(&mut f, "2025-05-05", income, bank, "0.03");
    entry(&mut f, "2025-05-06", bank, income, "10");
    let line = f
        .book
        .read(|tx| {
            Ok::<_, FbError>(
                fb_core::store::lines(tx, j)?
                    .into_iter()
                    .find(|l| l.account_id == income)
                    .unwrap()
                    .id,
            )
        })
        .unwrap();
    let foreign_company =
        services::create_company(&mut f.book, f.actor, "Other worksheets", "USD", false)
            .unwrap()
            .id;
    let foreign = services::save_account(
        &mut f.book,
        f.actor,
        foreign_company,
        services::AccountInput::new("401", "Foreign income")
            .kind("INCOME")
            .detail("INCOME"),
    )
    .unwrap()
    .id;
    let (start, end) = (day("2025-01-01"), day("2025-12-31"));
    let cases = [
        (income, Share::Cents(5), "exact total"),
        (income, Share::Weight(0), "positive total"),
        (income, Share::Weight(-1), "nonnegative weights"),
        (
            AccountId::new(),
            Share::Weight(1),
            "P&L account in this company",
        ),
        (foreign, Share::Weight(1), "P&L account in this company"),
    ];
    let run = |f: &mut Books, basis, mappings: &[Mapping]| {
        sc::schedule_c_activity(&mut f.book, f.actor, f.company, basis, start, end, mappings)
    };
    for (id, share, expected) in cases {
        let error = run(
            &mut f,
            Basis::Accrual,
            &[mapping(
                id,
                vec![split("A", Category::GrossReceipts, share)],
            )],
        )
        .unwrap_err();
        assert!(
            matches!(error, FbError::Validation(_)) && error.message().contains(expected),
            "{error}"
        );
    }
    let account_mapping = mapping(
        income,
        vec![split(
            "Account",
            Category::GrossReceipts,
            Share::Cents(1000),
        )],
    );
    let shares = vec![
        split("First", Category::ReturnsAllowances, Share::Weight(1)),
        split("Second", Category::ReturnsAllowances, Share::Weight(1)),
    ];
    for target in [Target::Journal(j), Target::Line(line)] {
        let report = run(
            &mut f,
            Basis::Cash,
            &[
                account_mapping.clone(),
                Mapping {
                    target,
                    splits: shares.clone(),
                },
            ],
        )
        .unwrap();
        assert_eq!(
            report.groups[&TaxActivity::Business("Account".into())].totals,
            totals(1000, 0)
        );
        assert_eq!(
            report.groups[&TaxActivity::Business("First".into())].totals,
            totals(-2, 0)
        );
        assert_eq!(
            report.groups[&TaxActivity::Business("Second".into())].totals,
            totals(-1, 0)
        );
        assert_eq!(report.book.net(), money(997));
    }
    let exact = mapping(
        income,
        vec![
            split("Receipts", Category::GrossReceipts, Share::Cents(1000)),
            split("Returns", Category::ReturnsAllowances, Share::Cents(-3)),
        ],
    );
    let report = run(&mut f, Basis::Accrual, &[exact]).unwrap();
    assert_eq!(
        report.groups[&TaxActivity::Business("Receipts".into())].totals,
        totals(1000, 0)
    );
    assert_eq!(
        report.groups[&TaxActivity::Business("Returns".into())].totals,
        totals(-3, 0)
    );
    assert_eq!(report.book.net(), money(997));
    let outsider = users::create_user(&mut f.book, "Worksheet outsider", false).unwrap();
    assert!(matches!(
        sc::schedule_c_activity(
            &mut f.book,
            outsider,
            f.company,
            Basis::Cash,
            start,
            end,
            &[]
        ),
        Err(FbError::PermissionDenied(_))
    ));
}

#[test]
fn fy2025_asset_schedule_reconciles_assets_with_additions_disposals_and_reversals() {
    let mut f = Books::new();
    let bank = AccountId::parse(&f.bank).unwrap();
    let equipment = account(&mut f, "150", "ASSET", "FIXED_ASSET");
    let contra = services::save_account(
        &mut f.book,
        f.actor,
        f.company,
        services::AccountInput::new("159", "Contra equipment")
            .kind("ASSET")
            .detail("FIXED_ASSET")
            .contra(true),
    )
    .unwrap()
    .id;
    let depreciation = account(&mut f, "610", "EXPENSE", "EXPENSE");
    let loss = account(&mut f, "611", "EXPENSE", "OTHER_EXPENSE");
    let surviving = entry(&mut f, "2024-01-01", equipment, bank, "1800");
    entry(&mut f, "2024-02-01", equipment, bank, "1000");
    entry(&mut f, "2024-12-31", depreciation, contra, "1000");
    let addition = entry(&mut f, "2025-01-01", equipment, bank, "1699");
    journal(
        &mut f,
        "2025-07-14",
        "Dispose of equipment",
        vec![
            services::line(contra).debit("400").build(),
            services::line(loss).debit("600").build(),
            services::line(equipment).credit("1000").build(),
        ],
    );
    let correction = entry(&mut f, "2025-10-01", depreciation, contra, "10");
    services::reverse_journal(
        &mut f.book,
        f.actor,
        f.company,
        correction,
        "2025-10-02",
        "Correct posting",
    )
    .unwrap();
    entry(&mut f, "2025-12-31", depreciation, contra, "609.90");
    entry(&mut f, "2026-01-01", equipment, bank, "9000");
    let record = |label: &str, journal, in_service, salvage, life, prior| AssetInput {
        label: label.into(),
        account: Some(equipment),
        journal: Some(journal),
        line: None,
        in_service: Some(day(in_service)),
        disposal_date: None,
        cost_cents: None,
        salvage_cents: Some(salvage),
        method: Some(Method::StraightLine),
        life_months: Some(life),
        prior_cents: Some(prior),
    };
    let mut inputs = [
        record(
            "Retained equipment",
            surviving,
            "2024-01-01",
            120000,
            12,
            60000,
        ),
        record(
            "Current-year equipment",
            addition,
            "2025-01-01",
            108910,
            12,
            0,
        ),
    ];
    let addition_line = f
        .book
        .read(|tx| {
            Ok::<_, FbError>(
                fb_core::store::lines(tx, addition)?
                    .into_iter()
                    .find(|l| l.account_id == equipment)
                    .unwrap()
                    .id,
            )
        })
        .unwrap();
    inputs[1].line = Some(addition_line);
    let (start, end) = (day("2025-01-01"), day("2025-12-31"));
    let before = f.everything();
    let run = |f: &mut Books, inputs: &[AssetInput]| {
        asset_report::asset_depreciation_schedule(
            &mut f.book,
            f.actor,
            f.company,
            start,
            end,
            inputs,
        )
    };
    let report = run(&mut f, &inputs).unwrap();
    assert_eq!(
        (
            report.cost.opening,
            report.cost.additions,
            report.cost.disposals,
            report.cost.closing()
        ),
        (money(280000), money(169900), money(100000), money(349900))
    );
    assert_eq!(
        (
            report.accumulated.opening,
            report.accumulated.booked,
            report.accumulated.other_debits,
            report.accumulated.period_net(),
            report.accumulated.closing()
        ),
        (
            money(100000),
            money(60990),
            money(40000),
            money(20990),
            money(120990)
        )
    );
    assert_eq!(report.expense_accounts.len(), 1);
    assert_eq!(
        (
            report.expense_accounts[0].0.id,
            report.expense_accounts[0].1
        ),
        (depreciation, money(60990))
    );
    assert_eq!(report.known_assets, report.ledger);
    assert_eq!(
        report.known_assets.difference(report.ledger),
        Values::default()
    );
    assert_eq!(report.ledger.net_book_value, money(228910));
    assert_eq!(report.incomplete, 0);
    assert_eq!(run(&mut f, &[]).unwrap().ledger, report.ledger);
    assert_eq!(f.everything(), before);
}

#[test]
fn asset_inputs_keep_missing_values_visible_and_refuse_foreign_links() {
    let mut f = Books::new();
    let bank = AccountId::parse(&f.bank).unwrap();
    let equipment = account(&mut f, "150", "ASSET", "FIXED_ASSET");
    let j = entry(&mut f, "2025-02-01", equipment, bank, "4");
    let local_company = f.company;
    f.company = services::create_company(&mut f.book, f.actor, "Other assets", "USD", false)
        .unwrap()
        .id;
    let foreign_account = account(&mut f, "150", "ASSET", "FIXED_ASSET");
    let other_equity = account(&mut f, "300", "EQUITY", "EQUITY");
    let foreign_journal = entry(
        &mut f,
        "2025-02-01",
        foreign_account,
        other_equity,
        "999.99",
    );
    let foreign_line = f
        .book
        .read(|tx| {
            Ok::<_, FbError>(
                fb_core::store::lines(tx, foreign_journal)?
                    .into_iter()
                    .find(|l| l.account_id == foreign_account)
                    .unwrap()
                    .id,
            )
        })
        .unwrap();
    f.company = local_company;
    let input = AssetInput {
        label: "Incomplete equipment".into(),
        account: Some(equipment),
        journal: Some(j),
        line: None,
        in_service: None,
        disposal_date: None,
        cost_cents: None,
        salvage_cents: Some(0),
        method: None,
        life_months: None,
        prior_cents: Some(0),
    };
    let (start, end) = (day("2025-01-01"), day("2025-12-31"));
    let run = |f: &mut Books, inputs: &[AssetInput]| {
        asset_report::asset_depreciation_schedule(
            &mut f.book,
            f.actor,
            f.company,
            start,
            end,
            inputs,
        )
    };
    let report = run(&mut f, std::slice::from_ref(&input)).unwrap();
    assert_eq!(report.ledger.cost, money(400));
    assert_eq!(report.incomplete, 1);
    assert_eq!(report.assets[0].cost, Some(money(400)));
    assert_eq!(
        report.assets[0].missing,
        ["in-service date", "method", "useful life"]
    );
    assert!(report.assets[0].values.is_none());
    for invalid in [
        AssetInput {
            account: Some(foreign_account),
            ..input.clone()
        },
        AssetInput {
            journal: Some(foreign_journal),
            account: None,
            ..input.clone()
        },
        AssetInput {
            line: Some(foreign_line),
            journal: None,
            account: None,
            ..input.clone()
        },
    ] {
        assert!(matches!(
            run(&mut f, &[invalid]),
            Err(FbError::Validation(_))
        ));
    }
    assert!(matches!(
        run(
            &mut f,
            &[
                input.clone(),
                AssetInput {
                    label: "Duplicate source".into(),
                    ..input
                }
            ]
        ),
        Err(FbError::Validation(_))
    ));
    let outsider = users::create_user(&mut f.book, "Asset outsider", false).unwrap();
    assert!(matches!(
        asset_report::asset_depreciation_schedule(
            &mut f.book,
            outsider,
            f.company,
            start,
            end,
            &[]
        ),
        Err(FbError::PermissionDenied(_))
    ));
}

#[test]
fn cash_schedule_c_keeps_invoice_line_overrides_through_receipt_credit_and_reversal() {
    let mut f = Books::new();
    let income = f.income.clone();
    let invoice = posted_invoice(
        &mut f,
        "2026-11-01",
        json!([
            {"description":"Consulting","quantity":"1","rate":"70.00","taxable":false,"income_account_id":income},
            {"description":"Rideshare","quantity":"1","rate":"30.00","taxable":false,"income_account_id":income}
        ]),
        None,
    );
    let source_lines = f.book.read(|tx| {
        let mut stmt = tx.prepare(
            "SELECT l.id, l.amount FROM sales_invoicerevision r JOIN sales_invoiceline l ON l.revision_id=r.id
             WHERE r.invoice_id=?1 AND r.state='OPEN'",
        )?;
        let lines = stmt.query_map([&invoice], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok::<_, FbError>(lines)
    }).unwrap();
    let source = |amount| {
        let (id, _) = source_lines
            .iter()
            .find(|(_, cents)| *cents == amount)
            .unwrap();
        fb_ids::JournalLineId::parse(id).unwrap()
    };
    let receipt = f.receive(&invoice, "50.00");
    let settlement = fb_core::JournalId::parse(receipt["journal_id"].as_str().unwrap()).unwrap();
    let override_for = |target, name| Mapping {
        target,
        splits: vec![split(name, Category::GrossReceipts, Share::Weight(1))],
    };
    let consulting = override_for(Target::Line(source(7000)), "Consulting");
    let rideshare = override_for(Target::Line(source(3000)), "Rideshare");
    let settlement = override_for(Target::Journal(settlement), "Settlement override");
    let check = |f: &mut Books, date, mappings: &[Mapping], expected: &[(&str, i64)], residual| {
        let start = day(date);
        let report = sc::schedule_c_activity(
            &mut f.book,
            f.actor,
            f.company,
            Basis::Cash,
            start,
            start,
            mappings,
        )
        .unwrap();
        let pl = profit_and_loss(
            &mut f.book,
            f.actor,
            f.company,
            Basis::Cash,
            Some(start),
            start,
        )
        .unwrap();
        for (name, amount) in expected {
            assert_eq!(
                report.groups[&TaxActivity::Business((*name).into())].totals,
                totals(*amount, 0),
                "{date}"
            );
        }
        assert_eq!(
            report.groups[&TaxActivity::Unassigned].totals,
            totals(residual, 0)
        );
        assert_eq!(
            report
                .groups
                .values()
                .map(|g| g.totals.net())
                .sum::<Money>(),
            pl.net_income
        );
        assert_eq!(report.allocated, report.book);
    };
    let both = [consulting.clone(), rideshare];
    check(
        &mut f,
        "2026-11-05",
        &both,
        &[("Consulting", 3500), ("Rideshare", 1500)],
        0,
    );
    check(
        &mut f,
        "2026-11-05",
        &[consulting, settlement],
        &[("Consulting", 3500), ("Settlement override", 1500)],
        0,
    );
    let credit = f.credit(&invoice, "10.00");
    check(
        &mut f,
        "2026-11-04",
        &both,
        &[("Consulting", 700), ("Rideshare", 300)],
        -1000,
    );
    let credit_line = f
        .book
        .read(|tx| {
            let line: String = tx.query_row(
                "SELECT l.id FROM credits_creditmemoline l
             JOIN credits_creditapplication a ON a.credit_memo_id=l.credit_memo_id
             WHERE a.id=?1",
                [id(&credit)],
                |row| row.get(0),
            )?;
            Ok::<_, FbError>(fb_ids::JournalLineId::parse(&line).unwrap())
        })
        .unwrap();
    let with_credit = [
        both[0].clone(),
        both[1].clone(),
        override_for(Target::Line(credit_line), "Return"),
    ];
    check(
        &mut f,
        "2026-11-04",
        &with_credit,
        &[("Consulting", 700), ("Rideshare", 300), ("Return", -1000)],
        0,
    );
    f.reverse(&receipt);
    check(
        &mut f,
        "2026-11-06",
        &both,
        &[("Consulting", -3500), ("Rideshare", -1500)],
        0,
    );
}

#[test]
fn schedule_c_exact_cent_shares_require_remaining_source_amounts_of_each_sign() {
    let mut f = Books::new();
    let bank = AccountId::parse(&f.bank).unwrap();
    let income = AccountId::parse(&f.income).unwrap();
    let exact = mapping(
        income,
        vec![
            split("Receipts", Category::GrossReceipts, Share::Cents(10000)),
            Split {
                activity: TaxActivity::Excluded,
                category: Some(Category::ExcludedPersonal),
                share: Share::Cents(-9000),
            },
        ],
    );
    let run = |f: &mut Books, start, mappings: &[Mapping]| {
        sc::schedule_c_activity(
            &mut f.book,
            f.actor,
            f.company,
            Basis::Accrual,
            day(start),
            day("2025-04-30"),
            mappings,
        )
    };
    entry(&mut f, "2025-04-01", bank, income, "10");
    let err = run(&mut f, "2025-04-01", std::slice::from_ref(&exact)).unwrap_err();
    assert!(err.message().contains("same sign"), "{err}");

    let receipt = entry(&mut f, "2025-04-02", bank, income, "100");
    let refund = entry(&mut f, "2025-04-03", income, bank, "90");
    let report = run(&mut f, "2025-04-02", std::slice::from_ref(&exact)).unwrap();
    assert_eq!(
        report.groups[&TaxActivity::Business("Receipts".into())].totals,
        totals(10000, 0)
    );
    assert_eq!(
        report.groups[&TaxActivity::Excluded].totals,
        totals(-9000, 0)
    );
    assert_eq!(report.allocated, totals(1000, 0));
    let mut higher = [
        exact,
        Mapping {
            target: Target::Journal(receipt),
            splits: vec![split("Posted", Category::GrossReceipts, Share::Weight(1))],
        },
        Mapping {
            target: Target::Journal(refund),
            splits: vec![split(
                "Posted",
                Category::ReturnsAllowances,
                Share::Weight(1),
            )],
        },
    ];
    // After both higher-priority overrides, the remaining account net is still $10.
    let err = run(&mut f, "2025-04-01", &higher).unwrap_err();
    assert!(err.message().contains("same sign"), "{err}");
    higher[0].splits[0].share = Share::Cents(1000);
    higher[0].splits[1].share = Share::Cents(0);
    assert!(run(&mut f, "2025-04-01", &higher).is_ok());
}

fn fixed_asset_contra(f: &mut Books) -> AccountId {
    services::save_account(
        &mut f.book,
        f.actor,
        f.company,
        services::AccountInput::new("159", "Accumulated depreciation")
            .kind("ASSET")
            .detail("FIXED_ASSET")
            .contra(true),
    )
    .unwrap()
    .id
}

#[test]
fn disposed_asset_reconciles_period_depreciation_and_zero_closing_balances() {
    let mut f = Books::new();
    let bank = AccountId::parse(&f.bank).unwrap();
    let equipment = account(&mut f, "150", "ASSET", "FIXED_ASSET");
    let contra = fixed_asset_contra(&mut f);
    let depreciation = account(&mut f, "610", "EXPENSE", "EXPENSE");
    let acquisition = entry(&mut f, "2025-01-01", equipment, bank, "310");
    entry(&mut f, "2025-01-10", depreciation, contra, "50");
    journal(
        &mut f,
        "2025-01-10",
        "Dispose of equipment",
        vec![
            services::line(contra).debit("50").build(),
            services::line(depreciation).debit("260").build(),
            services::line(equipment).credit("310").build(),
        ],
    );
    let mut asset = AssetInput {
        label: "Disposed machinery".into(),
        account: Some(equipment),
        journal: Some(acquisition),
        line: None,
        in_service: Some(day("2025-01-01")),
        disposal_date: Some(day("2025-01-10")),
        cost_cents: None,
        salvage_cents: Some(15500),
        method: Some(Method::StraightLine),
        life_months: Some(1),
        prior_cents: Some(0),
    };
    let run = |f: &mut Books, start, asset: &AssetInput| {
        asset_report::asset_depreciation_schedule(
            &mut f.book,
            f.actor,
            f.company,
            day(start),
            day("2025-01-31"),
            std::slice::from_ref(asset),
        )
    };
    let report = run(&mut f, "2025-01-01", &asset).unwrap();
    let expected = Values {
        period_depreciation: money(5000),
        ..Values::default()
    };
    assert!(report.assets[0].disposed);
    assert_eq!(report.assets[0].values, Some(expected));
    assert_eq!(report.ledger, expected);
    assert_eq!(
        report.known_assets.difference(report.ledger),
        Values::default()
    );
    assert_eq!(report.accumulated.unresolved_debits, Money::ZERO);
    let later = run(&mut f, "2025-01-11", &asset).unwrap();
    assert_eq!(later.assets[0].values, Some(Values::default()));
    assert_eq!(
        later.known_assets.difference(later.ledger),
        Values::default()
    );

    asset.disposal_date = Some(day("2024-12-31"));
    let err = run(&mut f, "2025-01-01", &asset).unwrap_err();
    assert!(err.message().contains("on or after"), "{err}");
}

#[test]
fn asset_rollforward_books_cogs_and_separates_mixed_contra_movements() {
    let mut f = Books::new();
    let bank = AccountId::parse(&f.bank).unwrap();
    let contra = fixed_asset_contra(&mut f);
    let cogs = account(&mut f, "510", "EXPENSE", "COST_OF_GOODS_SOLD");
    let supplies = account(&mut f, "610", "EXPENSE", "EXPENSE");
    entry(&mut f, "2025-01-05", cogs, contra, "500");
    journal(
        &mut f,
        "2025-01-06",
        "Depreciation and supplies",
        vec![
            services::line(cogs).debit("150").build(),
            services::line(supplies).debit("20").build(),
            services::line(bank).credit("20").build(),
            services::line(contra).credit("150").build(),
        ],
    );
    let report = asset_report::asset_depreciation_schedule(
        &mut f.book,
        f.actor,
        f.company,
        day("2025-01-01"),
        day("2025-01-31"),
        &[],
    )
    .unwrap();
    assert_eq!(report.expense_accounts[0].0.id, cogs);
    assert_eq!(report.expense_accounts[0].1, money(50000));
    assert_eq!(report.accumulated.booked, money(50000));
    assert_eq!(report.accumulated.unresolved_credits, money(15000));
    assert_eq!(report.accumulated.other_credits, Money::ZERO);
    assert_eq!(report.ledger.accumulated, money(65000));
}

#[test]
fn linked_asset_cost_requires_disambiguation_and_cannot_be_claimed_twice() {
    let mut f = Books::new();
    let bank = AccountId::parse(&f.bank).unwrap();
    let equipment = account(&mut f, "150", "ASSET", "FIXED_ASSET");
    let acquisition = journal(
        &mut f,
        "2025-01-05",
        "Two assets",
        vec![
            services::line(equipment).debit("80").build(),
            services::line(equipment).debit("20").build(),
            services::line(bank).credit("100").build(),
        ],
    );
    let line = f
        .book
        .read(|tx| {
            Ok::<_, FbError>(
                fb_core::store::lines(tx, acquisition)?
                    .into_iter()
                    .find(|l| l.debit == money(8000))
                    .unwrap()
                    .id,
            )
        })
        .unwrap();
    let mut asset = AssetInput {
        label: "First asset".into(),
        account: Some(equipment),
        journal: Some(acquisition),
        line: None,
        in_service: None,
        disposal_date: None,
        cost_cents: None,
        salvage_cents: None,
        method: None,
        life_months: None,
        prior_cents: None,
    };
    let run = |f: &mut Books, assets: &[AssetInput]| {
        asset_report::asset_depreciation_schedule(
            &mut f.book,
            f.actor,
            f.company,
            day("2025-01-01"),
            day("2025-01-31"),
            assets,
        )
    };
    let err = run(&mut f, std::slice::from_ref(&asset)).unwrap_err();
    assert!(err.message().contains("specific posted line ID"), "{err}");
    asset.line = Some(line);
    asset.cost_cents = Some(8000);
    let duplicate = AssetInput {
        label: "Duplicate".into(),
        ..asset.clone()
    };
    let err = run(&mut f, &[asset.clone(), duplicate.clone()]).unwrap_err();
    assert!(err.message().contains("unclaimed amount"), "{err}");
    asset.cost_cents = Some(4000);
    let split = AssetInput {
        cost_cents: Some(4000),
        ..duplicate
    };
    let report = run(&mut f, &[asset.clone(), split.clone()]).unwrap();
    assert_eq!(report.known_assets.cost, money(8000));

    let journal_only = AssetInput {
        label: "Journal link".into(),
        line: None,
        account: None,
        cost_cents: Some(2000),
        ..asset.clone()
    };
    let report = run(&mut f, &[journal_only, asset, split]).unwrap();
    assert_eq!(report.known_assets.cost, money(10000));
}

#[test]
fn reversing_asset_acquisition_keeps_disposals_zero() {
    let mut f = Books::new();
    let bank = AccountId::parse(&f.bank).unwrap();
    let equipment = account(&mut f, "150", "ASSET", "FIXED_ASSET");
    let acquisition = entry(&mut f, "2025-01-05", equipment, bank, "1000");
    services::reverse_journal(
        &mut f.book,
        f.actor,
        f.company,
        acquisition,
        "2025-01-10",
        "Duplicate acquisition",
    )
    .unwrap();
    let report = asset_report::asset_depreciation_schedule(
        &mut f.book,
        f.actor,
        f.company,
        day("2025-01-01"),
        day("2025-01-31"),
        &[],
    )
    .unwrap();
    assert_eq!(report.cost.additions, money(100000));
    assert_eq!(report.cost.reversals, money(-100000));
    assert_eq!(report.cost.disposals, Money::ZERO);
    assert_eq!(report.cost.closing(), Money::ZERO);
}

#[test]
fn cash_schedule_c_awards_a_rounded_account_cent_to_the_lower_source_line_id() {
    let mut f = Books::new();
    let income = AccountId::parse(&f.income).unwrap();
    let other = account(&mut f, "401", "INCOME", "INCOME");
    let invoice = posted_invoice(
        &mut f,
        "2026-11-01",
        json!([
            {"description":"First penny","quantity":"1","rate":"0.01","taxable":false,"income_account_id":income.to_db()},
            {"description":"Other account penny","quantity":"1","rate":"0.01","taxable":false,"income_account_id":other.to_db()},
            {"description":"Second penny","quantity":"1","rate":"0.01","taxable":false,"income_account_id":income.to_db()}
        ]),
        None,
    );
    let line = f
        .book
        .read(|tx| {
            let line: String = tx.query_row(
                "SELECT min(l.id) FROM sales_invoicerevision r JOIN sales_invoiceline l ON l.revision_id=r.id
                 WHERE r.invoice_id=?1 AND r.state='OPEN' AND l.income_account_id=?2",
                params![invoice, income.to_db()],
                |row| row.get(0),
            )?;
            Ok::<_, FbError>(fb_ids::JournalLineId::parse(&line).unwrap())
        })
        .unwrap();
    let receipt = f.receive(&invoice, "0.01");
    f.reverse(&receipt);
    let mapping = Mapping {
        target: Target::Line(line),
        splits: vec![split(
            "First cent",
            Category::GrossReceipts,
            Share::Weight(1),
        )],
    };
    for (date, amount) in [("2026-11-05", 1), ("2026-11-06", -1)] {
        let date = day(date);
        let report = sc::schedule_c_activity(
            &mut f.book,
            f.actor,
            f.company,
            Basis::Cash,
            date,
            date,
            std::slice::from_ref(&mapping),
        )
        .unwrap();
        assert_eq!(
            report.groups[&TaxActivity::Business("First cent".into())].totals,
            totals(amount, 0)
        );
        assert_eq!(report.groups[&TaxActivity::Unassigned].totals, totals(0, 0));
        assert_eq!(report.allocated, report.book);
    }
}
