use super::*;
use crate::{services::AccountInput, users};
pub(super) struct Fixture {
    pub(super) book: Book,
    pub(super) actor: UserId,
    pub(super) c: CompanyId,
    pub(super) customer: String,
    pub(super) income: AccountId,
    pub(super) bank: AccountId,
    pub(super) ar: AccountId,
    pub(super) liability: AccountId,
    pub(super) undeposited: AccountId,
}
impl Fixture {
    pub(super) fn new() -> Self {
        let path = std::env::temp_dir().join(format!("fb-prepayments-{}.fbbook", new_id()));
        let mut book = Book::create(&path, &[72; 32]).unwrap();
        let actor = users::create_user(&mut book, "Prepayment tester", false).unwrap();
        let c = services::create_company(&mut book, actor, "Synthetic prepayments", "USD", false)
            .unwrap()
            .id;
        let mut acct = |code: &str, kind: &str, detail: &str| {
            let mut p = AccountInput::new(code, code);
            p.kind = Some(kind.into());
            p.detail_type = Some(detail.into());
            services::save_account(&mut book, actor, c, p).unwrap().id
        };
        let bank = acct("100", "ASSET", "BANK");
        let ar = acct("110", "ASSET", "RECEIVABLE");
        let undeposited = acct("120", "ASSET", "OTHER_CURRENT_ASSET");
        let income = acct("400", "INCOME", "INCOME");
        let liability = acct("230", "LIABILITY", "OTHER_CURRENT_LIABILITY");
        documents::save_setup(&mut book,actor,c,&json!({"receivables":ar.to_db(),"undeposited":undeposited.to_db(),"customer_deposits":liability.to_db(),"banks":[bank.to_db()]}),Some(0)).unwrap();
        let customer = directory::save_record(
            &mut book,
            actor,
            c,
            Model::Customer,
            &json!({"display_name":"Synthetic customer"}),
            None,
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .into();
        Self {
            book,
            actor,
            c,
            customer,
            income,
            bank,
            ar,
            liability,
            undeposited,
        }
    }
    pub(super) fn input(&self) -> Value {
        json!({"request_key":new_id(),"customer_id":self.customer,"date":"2026-10-01","amount":"500.00","method":"CHECK","destination":"UNDEPOSITED","order_reference":"SO-1"})
    }
    pub(super) fn receive(&mut self) -> Value {
        let p = self.input();
        let r = review_deposit(&mut self.book, self.actor, self.c, &p).unwrap();
        record_deposit(&mut self.book, self.actor, self.c, Some(&r), &p).unwrap()
    }
    pub(super) fn invoice(&mut self) -> Value {
        let p = json!({"request_key":new_id(),"customer_id":self.customer,"date":"2026-10-02","tax_mode":"NONE","lines":[{"description":"Work","quantity":"1","rate":"1200.00","taxable":false,"income_account_id":self.income.to_db()}]});
        let v = sales::save_invoice(&mut self.book, self.actor, self.c, &p).unwrap();
        let r = sales::review_invoice(&mut self.book, self.actor, self.c, s(&v, "id"), Some(1))
            .unwrap();
        sales::post_invoice(
            &mut self.book,
            self.actor,
            self.c,
            s(&v, "id"),
            Some(1),
            Some(&r),
        )
        .unwrap()
    }
    pub(super) fn apply(&mut self, r: &Value, i: &Value, amount: &str) -> Value {
        let p = json!({"request_key":new_id(),"invoice_id":i["id"],"date":"2026-10-03","amount":amount});
        let review =
            review_application(&mut self.book, self.actor, self.c, s(r, "id"), &p).unwrap();
        apply_deposit(
            &mut self.book,
            self.actor,
            self.c,
            s(r, "id"),
            Some(&review),
            &p,
        )
        .unwrap()
    }
    pub(super) fn refund_input(&self) -> Value {
        json!({"request_key":new_id(),"date":"2026-10-04","amount":"100.00","bank_id":self.bank.to_db(),"method":"CHECK","check_number":"4001"})
    }
    pub(super) fn refund(&mut self, r: &Value) -> Value {
        let p = self.refund_input();
        let review = review_refund(&mut self.book, self.actor, self.c, s(r, "id"), &p).unwrap();
        record_refund(
            &mut self.book,
            self.actor,
            self.c,
            s(r, "id"),
            Some(&review),
            &p,
        )
        .unwrap()
    }
    pub(super) fn balance(&mut self, r: &Value) -> i64 {
        self.book.read(|q| remaining(q, r)).unwrap()
    }
    pub(super) fn counts(&mut self) -> Vec<i64> {
        self.book
            .read::<_, FbError>(|q| {
                [
                    "sales_receipt",
                    "ledger_journal",
                    "ledger_auditevent",
                    "documents_documentidentity",
                    "prepayments_depositapplication",
                    "prepayments_depositrefund",
                ]
                .iter()
                .map(|t| Ok(q.query_row(&format!("SELECT count(*) FROM {t}"), [], |r| r.get(0))?))
                .collect()
            })
            .unwrap()
    }
}
#[test]
fn receive_apply_1200_less_500_is_700() {
    let mut f = Fixture::new();
    let r = f.receive();
    let i = f.invoice();
    f.apply(&r, &i, "500.00");
    assert_eq!(f.balance(&r), 0);
    assert_eq!(
        f.book
            .read(|q| invoice_open_balance(q, f.c, s(&i, "id")))
            .unwrap(),
        70000
    );
    let balances = sales::invoice_balances(&mut f.book, f.actor, f.c).unwrap();
    assert_eq!(balances[0]["outstanding"], "700.00");
    assert_eq!(balances[0]["deposited"], "500.00");
    assert!(f.book.check().unwrap().is_empty());
}
#[test]
fn deposit_is_liability_not_income() {
    let mut f = Fixture::new();
    let r = f.receive();
    let ls = f
        .book
        .read(|q| store::lines(q, s(&r, "journal_id").parse().unwrap()))
        .unwrap();
    assert_eq!(ls.len(), 2);
    assert_eq!(ls[0].account_id, f.undeposited);
    assert_eq!(ls[1].account_id, f.liability);
    assert_eq!(ls[0].debit.to_string(), "500.00");
    assert_eq!(ls[1].credit.to_string(), "500.00");
    assert_eq!(
        unapplied_total(&mut f.book, f.actor, f.c, None)
            .unwrap()
            .to_string(),
        "500.00"
    );
}
#[test]
fn retries_and_void_release_dates() {
    let mut f = Fixture::new();
    let p = f.input();
    let review = review_deposit(&mut f.book, f.actor, f.c, &p).unwrap();
    let r = record_deposit(&mut f.book, f.actor, f.c, Some(&review), &p).unwrap();
    assert_eq!(
        r,
        record_deposit(&mut f.book, f.actor, f.c, Some(&review), &p).unwrap()
    );
    let i = f.invoice();
    let a = f.apply(&r, &i, "300.00");
    let refund = f.refund(&r);
    assert_eq!(f.balance(&r), 10000);
    assert!(
        void_deposit(
            &mut f.book,
            f.actor,
            f.c,
            s(&r, "id"),
            Some(1),
            &json!("2026-10-10"),
            &json!("void")
        )
        .is_err()
    );
    let u = unapply_deposit(
        &mut f.book,
        f.actor,
        f.c,
        s(&a, "id"),
        Some(1),
        &json!("2026-10-10"),
        &json!("undo"),
    )
    .unwrap();
    assert_eq!(
        u,
        unapply_deposit(
            &mut f.book,
            f.actor,
            f.c,
            s(&a, "id"),
            Some(1),
            &json!("2026-10-10"),
            &json!("undo")
        )
        .unwrap()
    );
    void_refund(
        &mut f.book,
        f.actor,
        f.c,
        s(&refund, "id"),
        Some(1),
        &json!("2026-10-11"),
        &json!("undo"),
    )
    .unwrap();
    assert_eq!(f.balance(&r), 50000);
    assert!(
        void_deposit(
            &mut f.book,
            f.actor,
            f.c,
            s(&r, "id"),
            Some(1),
            &json!("2026-10-10"),
            &json!("void")
        )
        .is_err()
    );
    void_deposit(
        &mut f.book,
        f.actor,
        f.c,
        s(&r, "id"),
        Some(1),
        &json!("2026-10-11"),
        &json!("void"),
    )
    .unwrap();
    assert_eq!(f.balance(&r), 0);
    assert_eq!(
        r,
        record_deposit(&mut f.book, f.actor, f.c, Some(&review), &p).unwrap()
    );
    assert!(f.book.check().unwrap().is_empty());
}
#[test]
fn malformed_inputs_change_nothing() {
    let mut f = Fixture::new();
    let before = f.counts();
    for (k, v) in [
        ("amount", json!("0")),
        ("amount", json!("-5")),
        ("amount", json!("1.001")),
        ("method", json!("BITCOIN")),
        ("destination", json!("MOON")),
        ("request_key", json!(false)),
        ("memo", Value::Null),
        ("customer_id", json!([])),
        ("estimate_id", json!(true)),
        ("order_reference", json!("x".repeat(121))),
    ] {
        let mut p = f.input();
        p[k] = v;
        assert!(
            review_deposit(&mut f.book, f.actor, f.c, &p).is_err(),
            "{p}"
        );
        assert_eq!(before, f.counts());
    }
}
#[test]
fn stale_application_review_rolls_back_identity() {
    let mut f = Fixture::new();
    let r = f.receive();
    let i = f.invoice();
    let p =
        json!({"request_key":new_id(),"invoice_id":i["id"],"date":"2026-10-03","amount":"100.00"});
    let review = review_application(&mut f.book, f.actor, f.c, s(&r, "id"), &p).unwrap();
    f.apply(&r, &i, "100.00");
    let before = f.counts();
    assert!(
        apply_deposit(&mut f.book, f.actor, f.c, s(&r, "id"), Some(&review), &p)
            .unwrap_err()
            .to_string()
            .contains("fresh review")
    );
    assert_eq!(before, f.counts());
}
#[test]
fn invoice_receipt_sees_applied_deposit() {
    let mut f = Fixture::new();
    let r = f.receive();
    let i = f.invoice();
    f.apply(&r, &i, "500.00");
    let mut p = json!({"request_key":new_id(),"customer_id":f.customer,"date":"2026-10-04","amount":"700.01","method":"CASH","destination":"BANK","bank_id":f.bank.to_db(),"allocations":[{"invoice_id":i["id"],"amount":"700.01"}]});
    assert!(sales::review_receipt(&mut f.book, f.actor, f.c, &p).is_err());
    p["amount"] = json!("700.00");
    p["allocations"][0]["amount"] = json!("700.00");
    let review = sales::review_receipt(&mut f.book, f.actor, f.c, &p).unwrap();
    sales::record_receipt(&mut f.book, f.actor, f.c, &p, Some(&review)).unwrap();
    assert_eq!(
        sales::invoice_balances(&mut f.book, f.actor, f.c).unwrap()[0]["payment_state"],
        "PAID"
    );
}
#[test]
fn overuse_and_refund_number_reuse() {
    let mut f = Fixture::new();
    let r = f.receive();
    let i = f.invoice();
    f.apply(&r, &i, "450.00");
    let p = f.refund_input();
    assert!(review_refund(&mut f.book, f.actor, f.c, s(&r, "id"), &p).is_err());
    let mut p = p;
    p["amount"] = json!("50.00");
    let review = review_refund(&mut f.book, f.actor, f.c, s(&r, "id"), &p).unwrap();
    let refund = record_refund(&mut f.book, f.actor, f.c, s(&r, "id"), Some(&review), &p).unwrap();
    assert_eq!(f.balance(&r), 0);
    assert_eq!(
        refund,
        record_refund(&mut f.book, f.actor, f.c, s(&r, "id"), Some(&review), &p).unwrap()
    );
    void_refund(
        &mut f.book,
        f.actor,
        f.c,
        s(&refund, "id"),
        Some(1),
        &json!("2026-10-05"),
        &json!("void"),
    )
    .unwrap();
    assert_eq!(f.balance(&r), 5000);
}
#[test]
fn owner_seals_refuse_orphan_journals() {
    let mut f = Fixture::new();
    for kind in ["CDP", "CDA", "CDR"] {
        let before = f.counts();
        let result = f.book.write::<_, FbError>(|tx| {
            let i = documents::create_identity(tx, f.actor, f.c, kind, &new_id())?;
            documents::post_source(
                tx,
                f.actor,
                f.c,
                s(&i, "id"),
                "2026-10-01",
                "orphan",
                entries(&f.bank.to_db(), &f.liability.to_db(), &json!("1.00")),
            )?;
            Ok(())
        });
        assert!(result.is_err());
        assert_eq!(before, f.counts());
    }
}
#[test]
fn append_only_all_new_tables_and_hidden_rowids() {
    let mut f = Fixture::new();
    let r = f.receive();
    let i = f.invoice();
    let a = f.apply(&r, &i, "100.00");
    unapply_deposit(
        &mut f.book,
        f.actor,
        f.c,
        s(&a, "id"),
        Some(1),
        &json!("2026-10-03"),
        &json!("undo"),
    )
    .unwrap();
    let refund = f.refund(&r);
    void_refund(
        &mut f.book,
        f.actor,
        f.c,
        s(&refund, "id"),
        Some(1),
        &json!("2026-10-04"),
        &json!("undo"),
    )
    .unwrap();
    // Attacker's re-keyed primary key: canonical, so 0130 passes and the append-only guard decides.
    const FRESH: &str = "00000000-0000-7000-8000-0000000e0101";
    let mut hit = std::collections::BTreeSet::new();
    for table in [
        "prepayments_customerdeposit",
        "prepayments_depositapplication",
        "prepayments_depositunapplication",
        "prepayments_depositrefund",
        "prepayments_depositrefundvoid",
    ] {
        for (label, sql) in [
            ("update same id", format!("UPDATE {table} SET id=id")),
            (
                "update or replace id",
                format!("UPDATE OR REPLACE {table} SET id='{FRESH}'"),
            ),
            ("delete", format!("DELETE FROM {table}")),
            (
                "replace",
                format!("INSERT OR REPLACE INTO {table} SELECT * FROM {table}"),
            ),
        ] {
            let e = f
                .book
                .write::<_, FbError>(|q| {
                    q.execute(&sql, [])?;
                    Ok(())
                })
                .expect_err(&sql);
            assert_ne!(
                e.constraint(),
                Some("fb_valid"),
                "masked by fb_valid: {sql}: {e}"
            );
            hit.insert(format!(
                "{table} {label}: {}",
                e.constraint()
                    .map(str::to_owned)
                    .unwrap_or_else(|| e.to_string())
            ));
        }
        for alias in ["rowid", "_rowid_", "oid"] {
            assert!(
                f.book
                    .read::<_, FbError>(|q| {
                        q.prepare(&format!("SELECT {alias} FROM {table}"))?;
                        Ok(())
                    })
                    .is_err()
            );
        }
    }
    let hit = hit.iter().map(String::as_str).collect::<Vec<_>>();
    let expected: &[&str] = &[
        "prepayments_customerdeposit delete: customer_deposit_immutable",
        "prepayments_customerdeposit replace: customer_deposit_scope",
        "prepayments_customerdeposit update or replace id: customer_deposit_immutable",
        "prepayments_customerdeposit update same id: customer_deposit_immutable",
        "prepayments_depositapplication delete: prepayment_parent_retained",
        "prepayments_depositapplication replace: customer_deposit_immutable",
        "prepayments_depositapplication update or replace id: customer_deposit_immutable",
        "prepayments_depositapplication update same id: customer_deposit_immutable",
        "prepayments_depositrefund delete: prepayment_parent_retained",
        "prepayments_depositrefund replace: customer_deposit_immutable",
        "prepayments_depositrefund update or replace id: customer_deposit_immutable",
        "prepayments_depositrefund update same id: customer_deposit_immutable",
        "prepayments_depositrefundvoid delete: customer_deposit_immutable",
        "prepayments_depositrefundvoid replace: customer_deposit_immutable",
        "prepayments_depositrefundvoid update or replace id: customer_deposit_immutable",
        "prepayments_depositrefundvoid update same id: customer_deposit_immutable",
        "prepayments_depositunapplication delete: customer_deposit_immutable",
        "prepayments_depositunapplication replace: customer_deposit_immutable",
        "prepayments_depositunapplication update or replace id: customer_deposit_immutable",
        "prepayments_depositunapplication update same id: customer_deposit_immutable",
    ];
    assert_eq!(hit, expected);
    assert!(f.book.check().unwrap().is_empty());
}
#[test]
fn exact_refund_split_and_account_eligibility() {
    let mut f = Fixture::new();
    let r = f.receive();
    let refund = f.refund(&r);
    let ls = f
        .book
        .read(|q| store::lines(q, s(&refund, "journal_id").parse().unwrap()))
        .unwrap();
    assert_eq!(ls[0].account_id, f.liability);
    assert_eq!(ls[1].account_id, f.bank);
    let mut p = f.refund_input();
    p["bank_id"] = json!(f.ar.to_db());
    assert!(review_refund(&mut f.book, f.actor, f.c, s(&r, "id"), &p).is_err());
}

#[test]
fn selected_estimate_matches_frozen_deposit_rules() {
    let mut f = Fixture::new();
    let payload = json!({"request_key":new_id(),"customer_id":f.customer,"date":"2026-10-01","tax_mode":"NONE","lines":[{"description":"Work","quantity":"1","rate":"10.00","taxable":false}]});
    let estimate = sales::save_estimate(&mut f.book, f.actor, f.c, &payload).unwrap();
    let mut p = f.input();
    p["estimate_id"] = estimate["id"].clone();
    let review = review_deposit(&mut f.book, f.actor, f.c, &p).unwrap();
    assert_eq!(review.display["estimate_number"], "EST-000001");
    let other = directory::save_record(&mut f.book, f.actor, f.c, Model::Customer, &json!({"display_name":"Another customer"}), None).unwrap();
    p["request_key"] = json!(new_id());
    p["customer_id"] = other["id"].clone();
    let e = review_deposit(&mut f.book, f.actor, f.c, &p).unwrap_err();
    assert!(e.to_string().contains("Estimate EST-000001 belongs to another customer. Link an estimate of this customer, or none."), "{e}");
    p["customer_id"] = json!(f.customer);
    sales::transition_estimate(&mut f.book, f.actor, f.c, s(&estimate,"id"), "CANCELLED", Some(1), "Synthetic cancellation").unwrap();
    let e = review_deposit(&mut f.book, f.actor, f.c, &p).unwrap_err();
    assert!(e.to_string().contains("Estimate EST-000001 is cancelled. Link an open estimate, or none."), "{e}");
    let other_company = services::create_company(&mut f.book, f.actor, "Other books", "USD", false).unwrap().id;
    let e = review_deposit(&mut f.book, f.actor, other_company, &p).unwrap_err();
    assert!(e.to_string().contains("Choose an estimate from this company."), "{e}");
    p["estimate_id"] = json!(new_id());
    let e = review_deposit(&mut f.book, f.actor, f.c, &p).unwrap_err();
    assert!(e.to_string().contains("Choose an estimate from this company."), "{e}");
    assert!(f.book.check().unwrap().is_empty());
}
