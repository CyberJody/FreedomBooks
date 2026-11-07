use super::*;
mod guards;
mod hardening;
use crate::{services::AccountInput, users};
struct Fixture {
    book: Book,
    actor: UserId,
    company: CompanyId,
    customer: String,
    income: AccountId,
    ar: AccountId,
    bank: AccountId,
    tax: AccountId,
    path: std::path::PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("fb-credits-{}.fbbook", new_id()));
        let mut book = Book::create(&path, &[24; 32]).unwrap();
        let actor = users::create_user(&mut book, "Credits tester", false).unwrap();
        let company = services::create_company(&mut book, actor, "Synthetic credits", "USD", false)
            .unwrap()
            .id;
        fn acct(b: &mut Book, u: UserId, c: CompanyId, code: &str, kind: &str) -> AccountId {
            let mut p = AccountInput::new(code, code);
            p.kind = Some(kind.into());
            services::save_account(b, u, c, p).unwrap().id
        }
        let income = acct(&mut book, actor, company, "400", "INCOME");
        let ar = acct(&mut book, actor, company, "110", "ASSET");
        let bank = acct(&mut book, actor, company, "100", "ASSET");
        let tax = acct(&mut book, actor, company, "210", "LIABILITY");
        documents::save_setup(
            &mut book,
            actor,
            company,
            &json!({"receivables":ar.to_db(),"collected_tax":tax.to_db(),"banks":[bank.to_db()]}),
            Some(0),
        )
        .unwrap();
        let customer = directory::save_record(
            &mut book,
            actor,
            company,
            Model::Customer,
            &json!({"display_name":"Synthetic customer"}),
            None,
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        Self {
            book,
            actor,
            company,
            customer,
            income,
            ar,
            bank,
            tax,
            path,
        }
    }
    fn input(&self) -> Value {
        json!({"request_key":new_id(),"customer_id":self.customer,"date":"2026-10-08","memo":"Returned service","tax_mode":"MANUAL","tax_rate":"8","tax_label":"Tax","lines":[{"item_id":null,"description":"Work","quantity":"1","rate":"10.00","taxable":true,"income_account_id":self.income.to_db()},{"item_id":null,"description":"Untaxed","quantity":"1","rate":"2.50","taxable":false,"income_account_id":self.income.to_db()}]})
    }
    fn memo(&mut self) -> Value {
        let p = self.input();
        let r = review_credit_memo(&mut self.book, self.actor, self.company, &p).unwrap();
        record_credit_memo(&mut self.book, self.actor, self.company, &p, Some(&r)).unwrap()
    }
    fn invoice(&mut self) -> Value {
        let mut p = self.input();
        p["date"] = json!("2026-10-01");
        p["due_date"] = json!("2026-11-01");
        p["lines"][0]["rate"] = json!("30.00");
        let v = crate::sales::save_invoice(&mut self.book, self.actor, self.company, &p).unwrap();
        let k = v["id"].as_str().unwrap();
        let r = crate::sales::review_invoice(&mut self.book, self.actor, self.company, k, Some(1))
            .unwrap();
        crate::sales::post_invoice(
            &mut self.book,
            self.actor,
            self.company,
            k,
            Some(1),
            Some(&r),
        )
        .unwrap()
    }
    fn application(&self, invoice: &Value, amount: &str) -> Value {
        json!({"request_key":new_id(),"date":"2026-10-09","allocations":[{"invoice_id":invoice["id"],"amount":amount}]})
    }
    fn refund_input(&self, amount: &str) -> Value {
        json!({"request_key":new_id(),"date":"2026-10-11","amount":amount,"bank_id":self.bank.to_db(),"method":"CHECK","check_number":"1001","memo":"Synthetic refund"})
    }
    fn available(&mut self) -> Value {
        memo_balances(&mut self.book, self.actor, self.company).unwrap()[0]["available"].clone()
    }
    fn clean(&self) {
        assert!(self.book.check().unwrap().is_empty());
        let reopened = Book::open(&self.path, &[24; 32]).unwrap();
        assert!(reopened.check().unwrap().is_empty());
        assert_eq!(self.book.connection().path(), self.path.to_str());
        assert_eq!(fb_db::test_support::snapshot(reopened.connection()).unwrap(), fb_db::test_support::snapshot(self.book.connection()).unwrap());
    }
}
#[test]
fn record_exact_posting_and_retry() {
    let mut f = Fixture::new();
    let p = f.input();
    let r = review_credit_memo(&mut f.book, f.actor, f.company, &p).unwrap();
    let m = record_credit_memo(&mut f.book, f.actor, f.company, &p, Some(&r)).unwrap();
    assert_eq!(m["total"], "13.30");
    assert_eq!(m["tax"], "0.80");
    assert_eq!(f.available(), "13.30");
    assert_eq!(
        record_credit_memo(&mut f.book, f.actor, f.company, &p, Some(&r)).unwrap(),
        m
    );
    let mut changed = p.clone();
    changed["memo"] = json!("Changed");
    assert!(record_credit_memo(&mut f.book, f.actor, f.company, &changed, Some(&r)).is_err());
    let rows=f.book.read(|tx|->FbResult<Vec<(String,i64,i64)>>{let mut st=tx.prepare("SELECT account_id,debit,credit FROM ledger_journalline WHERE journal_id=?1 ORDER BY account_id")?;Ok(st.query_map([m["journal_id"].as_str().unwrap()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?.collect::<Result<Vec<_>,_>>()?)}).unwrap();
    assert!(rows.contains(&(f.income.to_db(), 1250, 0)));
    assert!(rows.contains(&(f.tax.to_db(), 80, 0)));
    assert!(rows.contains(&(f.ar.to_db(), 0, 1330)));
    f.clean();
}
#[test]
fn application_nonposting_receipt_balance_unapply_and_void_dates() {
    let mut f = Fixture::new();
    let inv = f.invoice();
    let m = f.memo();
    let p = f.application(&inv, "5.00");
    let k = m["id"].as_str().unwrap();
    let r = review_application(&mut f.book, f.actor, f.company, k, &p).unwrap();
    let a = apply_credit(&mut f.book, f.actor, f.company, k, &p, Some(&r)).unwrap();
    assert_eq!(f.available(), "8.30");
    assert!(
        crate::sales::void_invoice(
            &mut f.book,
            f.actor,
            f.company,
            inv["id"].as_str().unwrap(),
            Some(2),
            &json!("2026-10-20"),
            &json!("Void")
        )
        .is_err()
    );
    assert!(
        void_credit_memo(
            &mut f.book,
            f.actor,
            f.company,
            k,
            Some(1),
            &json!("2026-10-20"),
            &json!("Void")
        )
        .is_err()
    );
    let u = unapply_credit(
        &mut f.book,
        f.actor,
        f.company,
        a["id"].as_str().unwrap(),
        Some(1),
        &json!("2026-10-19"),
        &json!("Release"),
    )
    .unwrap();
    assert_eq!(
        unapply_credit(
            &mut f.book,
            f.actor,
            f.company,
            a["id"].as_str().unwrap(),
            Some(1),
            &json!("2026-10-19"),
            &json!("Release")
        )
        .unwrap(),
        u
    );
    assert_eq!(
        apply_credit(&mut f.book, f.actor, f.company, k, &p, Some(&r)).unwrap(),
        a
    );
    assert_eq!(f.available(), "13.30");
    assert!(
        void_credit_memo(
            &mut f.book,
            f.actor,
            f.company,
            k,
            Some(1),
            &json!("2026-10-18"),
            &json!("Void")
        )
        .is_err()
    );
    assert!(
        crate::sales::void_invoice(
            &mut f.book,
            f.actor,
            f.company,
            inv["id"].as_str().unwrap(),
            Some(2),
            &json!("2026-10-18"),
            &json!("Void")
        )
        .is_err()
    );
    void_credit_memo(
        &mut f.book,
        f.actor,
        f.company,
        k,
        Some(1),
        &json!("2026-10-20"),
        &json!("Void"),
    )
    .unwrap();
    assert_eq!(f.available(), "0.00");
    f.clean();
}
#[test]
fn refund_void_exact_retry_and_released_number() {
    let mut f = Fixture::new();
    let m = f.memo();
    let k = m["id"].as_str().unwrap();
    let p = f.refund_input("5.00");
    let r = review_refund(&mut f.book, f.actor, f.company, k, &p).unwrap();
    let refund = record_refund(&mut f.book, f.actor, f.company, k, &p, Some(&r)).unwrap();
    assert_eq!(f.available(), "8.30");
    assert_eq!(
        record_refund(&mut f.book, f.actor, f.company, k, &p, Some(&r)).unwrap(),
        refund
    );
    let mut second = p.clone();
    second["request_key"] = json!(new_id());
    assert!(review_refund(&mut f.book, f.actor, f.company, k, &second).is_err());
    let v = void_refund(
        &mut f.book,
        f.actor,
        f.company,
        refund["id"].as_str().unwrap(),
        Some(1),
        &json!("2026-10-20"),
        &json!("Void refund"),
    )
    .unwrap();
    assert_eq!(
        void_refund(
            &mut f.book,
            f.actor,
            f.company,
            refund["id"].as_str().unwrap(),
            Some(1),
            &json!("2026-10-20"),
            &json!("Void refund")
        )
        .unwrap(),
        v
    );
    assert_eq!(
        record_refund(&mut f.book, f.actor, f.company, k, &p, Some(&r)).unwrap(),
        refund
    );
    assert_eq!(f.available(), "13.30");
    let r = review_refund(&mut f.book, f.actor, f.company, k, &second).unwrap();
    record_refund(&mut f.book, f.actor, f.company, k, &second, Some(&r)).unwrap();
    f.clean();
}

fn snapshot(f: &Fixture) -> Vec<(String, Vec<Vec<fb_db::rusqlite::types::Value>>)> {
    let c = f.book.connection();
    let mut st=c.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT GLOB 'sqlite_*' ORDER BY name").unwrap();
    let tables = st
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    tables
        .into_iter()
        .map(|t| {
            let mut st = c
                .prepare(&format!("SELECT * FROM {t} ORDER BY 1,2"))
                .unwrap();
            let n = st.column_count();
            let rows = st
                .query_map([], |r| {
                    (0..n).map(|i| r.get(i)).collect::<Result<Vec<_>, _>>()
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            (t, rows)
        })
        .collect()
}
fn populated() -> Fixture {
    let mut f = Fixture::new();
    let inv = f.invoice();
    let m = f.memo();
    let k = m["id"].as_str().unwrap();
    let p = f.application(&inv, "1.00");
    let r = review_application(&mut f.book, f.actor, f.company, k, &p).unwrap();
    let a = apply_credit(&mut f.book, f.actor, f.company, k, &p, Some(&r)).unwrap();
    unapply_credit(
        &mut f.book,
        f.actor,
        f.company,
        a["id"].as_str().unwrap(),
        Some(1),
        &json!("2026-10-20"),
        &json!("Release"),
    )
    .unwrap();
    let p = f.refund_input("1.00");
    let r = review_refund(&mut f.book, f.actor, f.company, k, &p).unwrap();
    let rf = record_refund(&mut f.book, f.actor, f.company, k, &p, Some(&r)).unwrap();
    void_refund(
        &mut f.book,
        f.actor,
        f.company,
        rf["id"].as_str().unwrap(),
        Some(1),
        &json!("2026-10-20"),
        &json!("Release"),
    )
    .unwrap();
    void_credit_memo(
        &mut f.book,
        f.actor,
        f.company,
        k,
        Some(1),
        &json!("2026-10-21"),
        &json!("Release"),
    )
    .unwrap();
    f
}
#[test]
fn immutable_replacement_primary_and_all_alternate_key_matrix() {
    let f = populated();
    let c = f.book.connection();
    for recursive in [false, true] {
        c.pragma_update(None, "recursive_triggers", recursive)
            .unwrap();
        for (table, alternate) in [
            ("creditmemo", vec![vec!["identity_id"], vec!["journal_id"]]),
            ("creditmemoline", vec![vec!["credit_memo_id", "position"]]),
            (
                "creditmemovoid",
                vec![vec!["credit_memo_id"], vec!["journal_id"]],
            ),
            ("creditapplication", vec![vec!["company_id", "request_key"]]),
            (
                "creditapplicationline",
                vec![vec!["application_id", "invoice_id"]],
            ),
            ("creditunapplication", vec![vec!["application_id"]]),
            (
                "creditrefund",
                vec![vec!["identity_id"], vec!["journal_id"]],
            ),
            (
                "creditrefundvoid",
                vec![vec!["refund_id"], vec!["journal_id"]],
            ),
        ] {
            let table = format!("credits_{table}");
            let before = snapshot(&f);
            let mut st = c
                .prepare(&format!("SELECT * FROM {table} LIMIT 1"))
                .unwrap();
            let cols = st.column_names().map(str::to_owned).collect::<Vec<_>>();
            let vals = st
                .query_row([], |r| {
                    (0..cols.len())
                        .map(|i| r.get::<_, fb_db::rusqlite::types::Value>(i))
                        .collect::<Result<Vec<_>, _>>()
                })
                .unwrap();
            drop(st);
            let mut unique = vec![vec!["id"]];
            unique.extend(alternate);
            for collision in &unique {
                let mut v = vals.clone();
                for other in &unique {
                    if other != collision {
                        for col in other {
                            if collision.contains(col) {
                                continue;
                            }
                            let i = cols.iter().position(|c| c == col).unwrap();
                            if matches!(v[i], fb_db::rusqlite::types::Value::Text(_)) {
                                v[i] = fb_db::rusqlite::types::Value::Text(new_id());
                            }
                        }
                    }
                }
                // All collisions should be refused before parent/field validation, regardless
                // of which unique key would have deleted the historical victim.
                for op in ["INSERT OR REPLACE", "REPLACE", "INSERT OR IGNORE"] {
                    let sql = format!(
                        "{op} INTO {table}({}) VALUES({})",
                        cols.join(","),
                        vec!["?"; cols.len()].join(",")
                    );
                    let e = c
                        .execute(&sql, fb_db::rusqlite::params_from_iter(&v))
                        .unwrap_err();
                    assert_eq!(
                        fb_db::constraint_of(&e).as_deref(),
                        Some("credit_immutable"),
                        "{table} {collision:?} {e}"
                    );
                    assert_eq!(snapshot(&f), before);
                }
            }
            for op in ["UPDATE", "UPDATE OR REPLACE"] {
                for set in [format!("id='{}'", new_id()), "id=id".into()] {
                    let e = c
                        .execute(&format!("{op} {table} SET {set}"), [])
                        .unwrap_err();
                    assert_eq!(
                        fb_db::constraint_of(&e).as_deref(),
                        Some("credit_immutable")
                    );
                    assert_eq!(snapshot(&f), before);
                }
            }
            for hidden in ["rowid", "_rowid_", "oid"] {
                assert!(
                    c.execute(
                        &format!("INSERT OR REPLACE INTO {table}({hidden}) VALUES(1)"),
                        []
                    )
                    .is_err()
                );
                assert!(
                    c.execute(&format!("UPDATE OR REPLACE {table} SET {hidden}=1"), [])
                        .is_err()
                );
            }
            assert!(c.execute(&format!("DELETE FROM {table}"), []).is_err());
            assert_eq!(snapshot(&f), before);
            c.execute_batch(&format!("CREATE TEMP VIEW cr_v AS SELECT * FROM {table}; CREATE TEMP TRIGGER cr_t INSTEAD OF UPDATE ON cr_v BEGIN UPDATE OR REPLACE {table} SET id=NEW.id WHERE id=OLD.id; END;")).unwrap();
            assert!(
                c.execute("UPDATE OR REPLACE cr_v SET id=?1", [new_id()])
                    .is_err()
            );
            c.execute_batch("DROP TRIGGER cr_t; DROP VIEW cr_v;")
                .unwrap();
            assert_eq!(snapshot(&f), before);
            let sql = format!(
                "INSERT INTO {table}({}) VALUES({}) ON CONFLICT(id) DO UPDATE SET id=excluded.id",
                cols.join(","),
                vec!["?"; cols.len()].join(",")
            );
            assert!(
                c.execute(&sql, fb_db::rusqlite::params_from_iter(&vals))
                    .is_err()
            );
            assert_eq!(snapshot(&f), before);
        }
    }
    c.pragma_update(None, "recursive_triggers", true).unwrap();
    f.clean();
}

#[test]
fn review_credit_dates_must_not_coerce_malformed_iso_strings() {
    let mut accepted = vec![];
    for value in [
        "2026W-01",
        "2026-W0--1",
        "+026-01-01",
        "2026-+1-01",
        "2026-01-+1",
        "2026-W+1-1",
    ] {
        if let Ok(parsed) = date(&json!(value)) {
            accepted.push((value, parsed));
        }
    }
    println!("Malformed ISO dates accepted and normalized: {accepted:?}");
    assert!(
        accepted.is_empty(),
        "malformed dates must be rejected, not coerced"
    );
}
