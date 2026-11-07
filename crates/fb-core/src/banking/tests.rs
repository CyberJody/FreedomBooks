use super::*;
#[path = "hardening_tests.rs"]
mod hardening;
#[path = "matrix_tests.rs"]
mod matrix;
#[path = "dfix3_tests.rs"]
mod dfix3;
#[path = "dfix3b_validator_tests.rs"]
mod dfix3b_validators;
use crate::{services::AccountInput, users};
use std::sync::atomic::{AtomicUsize, Ordering};
static NEXT: AtomicUsize = AtomicUsize::new(0);
#[test]
fn statement_amounts_preserve_exact_python_decimal_text_and_reject_fractional_cents() {
    assert_eq!(cents(&json!("１２_３.４０")).unwrap(), 12340);
    assert_eq!(cents(&json!("-0E+99999999999999999")).unwrap(), 0);
    assert_eq!(cents(&json!("1.2300E2")).unwrap(), 12300);
    assert!(cents(&json!("1e-99999999999999999")).is_err());
    assert!(cents(&json!("0.001")).is_err());
    assert!(cents(&json!("100000000000000.00")).is_err());
    assert!(cents(&json!(true)).is_err());
}
struct Fx {
    book: Book,
    path: std::path::PathBuf,
    u: UserId,
    c: CompanyId,
    bank: AccountId,
    uf: AccountId,
    income: AccountId,
    expense: AccountId,
    customer: String,
}
impl Fx {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "banking-{}-{}.fbbook",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        let mut book = Book::create(&path, &[87; 32]).unwrap();
        let u = users::create_user(&mut book, "banking-test", true).unwrap();
        let c = services::create_company(&mut book, u, "Synthetic banking", "USD", false)
            .unwrap()
            .id;
        let mut make = |code, name, kind| {
            services::save_account(&mut book, u, c, AccountInput::new(code, name).kind(kind))
                .unwrap()
                .id
        };
        let bank = make("100", "Bank", "ASSET");
        let uf = make("110", "Undeposited", "ASSET");
        let income = make("400", "Income", "INCOME");
        let expense = make("500", "Expense", "EXPENSE");
        documents::save_setup(
            &mut book,
            u,
            c,
            &json!({"undeposited":uf.to_db(),"banks":[bank.to_db()]}),
            Some(0),
        )
        .unwrap();
        let customer = crate::directory::save_record(
            &mut book,
            u,
            c,
            crate::directory::Model::Customer,
            &json!({"display_name":"Synthetic customer"}),
            None,
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .into();
        Self {
            book,
            path,
            u,
            c,
            bank,
            uf,
            income,
            expense,
            customer,
        }
    }
    fn sale(&mut self, n: &str) -> Value {
        let p = json!({"request_key":new_id(),"customer_id":self.customer,"date":"2026-10-01","destination":"UNDEPOSITED","method":"CASH","tax_mode":"NONE","lines":[{"item_id":null,"taxable":false,"description":"Test sale","quantity":"1","rate":n,"income_account_id":self.income.to_db()}]});
        let r = sales::review_sales_receipt(&mut self.book, self.u, self.c, &p).unwrap();
        sales::record_sales_receipt(&mut self.book, self.u, self.c, &p, Some(&r)).unwrap()
    }
    fn payload(&self, ids: Vec<Value>) -> Value {
        json!({"request_key":new_id(),"bank_id":self.bank.to_db(),"date":"2026-10-02","receipt_ids":ids,"memo":"Synthetic deposit"})
    }
    fn record(&mut self, p: &Value) -> Value {
        let r = review_deposit(&mut self.book, self.u, self.c, p).unwrap();
        record_deposit(&mut self.book, self.u, self.c, p, &r).unwrap()
    }
    fn journal(&mut self, d: &str, debit: bool, n: &str) -> String {
        let lines = if debit {
            vec![
                json!({"account":self.bank.to_db(),"debit":n}),
                json!({"account":self.income.to_db(),"credit":n}),
            ]
        } else {
            vec![
                json!({"account":self.expense.to_db(),"debit":n}),
                json!({"account":self.bank.to_db(),"credit":n}),
            ]
        };
        let j = services::save_draft(
            &mut self.book,
            self.u,
            self.c,
            services::DraftInput::new(d, "Synthetic bank entry", lines),
        )
        .unwrap();
        services::post_journal(&mut self.book, self.u, self.c, j.id, None).unwrap();
        self.book
            .connection()
            .query_row(
                "SELECT id FROM ledger_journalline WHERE journal_id=?1 AND account_id=?2",
                params![j.id.to_db(), self.bank.to_db()],
                |r| r.get(0),
            )
            .unwrap()
    }
    fn begin(&mut self, n: &str) -> Value {
        begin(
            &mut self.book,
            self.u,
            self.c,
            &self.bank.to_db(),
            &json!({"request_key":new_id(),"statement_date":"2026-10-31","ending_balance":n}),
        )
        .unwrap()
    }
    fn clear(&mut self, key: &str) {
        set_cleared(&mut self.book, self.u, self.c, key, "C", Some(0), None).unwrap();
    }
    fn finish(&mut self, r: &Value) -> Value {
        reconcile_now(
            &mut self.book,
            self.u,
            self.c,
            r["id"].as_str().unwrap(),
            &self.bank.to_db(),
            r["revision"].as_i64(),
            &json!(new_id()),
        )
        .unwrap()
    }
    fn snap(&self) -> Vec<(String, Vec<Vec<fb_db::rusqlite::types::Value>>)> {
        let q = self.book.connection();
        let mut st=q.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT GLOB 'sqlite_*' ORDER BY name").unwrap();
        let names = st
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        names
            .into_iter()
            .map(|t| {
                let mut st = q
                    .prepare(&format!("SELECT * FROM {t} ORDER BY 1,2"))
                    .unwrap();
                let n = st.column_count();
                let rows = st
                    .query_map([], |r| (0..n).map(|i| r.get(i)).collect())
                    .unwrap()
                    .collect::<Result<Vec<Vec<_>>, _>>()
                    .unwrap();
                (t, rows)
            })
            .collect()
    }
}
#[test]
fn deposit_groups_original_funds_and_never_reposts_income() {
    let mut f = Fx::new();
    let a = f.sale("10.25");
    let b = f.sale("30.00");
    let p = f.payload(vec![a["id"].clone(), b["id"].clone()]);
    let d = f.record(&p);
    assert_eq!(d["amount"], "40.25");
    let lines = store::lines(
        f.book.connection(),
        d["journal_id"].as_str().unwrap().parse().unwrap(),
    )
    .unwrap();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].account_id, f.bank);
    assert_eq!(lines[1].account_id, f.uf);
    assert_eq!(lines[1].credit.to_string(), "40.25");
    assert!(f.book.check().unwrap().is_empty());
}
#[test]
fn exact_deposit_retry_survives_reversal() {
    let mut f = Fx::new();
    let a = f.sale("10");
    let p = f.payload(vec![a["id"].clone()]);
    let r = review_deposit(&mut f.book, f.u, f.c, &p).unwrap();
    let d = record_deposit(&mut f.book, f.u, f.c, &p, &r).unwrap();
    let key = d["id"].as_str().unwrap();
    reverse_deposit(
        &mut f.book,
        f.u,
        f.c,
        key,
        Some(1),
        &json!("2026-10-03"),
        &json!("Correction"),
    )
    .unwrap();
    let snap = f.snap();
    assert_eq!(record_deposit(&mut f.book, f.u, f.c, &p, &r).unwrap(), d);
    assert_eq!(f.snap(), snap);
}
#[test]
fn live_deposit_and_release_dates_lock_both_sales_kinds() {
    let mut f = Fx::new();
    let a = f.sale("10");
    let p = f.payload(vec![a["id"].clone()]);
    let d = f.record(&p);
    let before = f.snap();
    assert!(
        sales::void_sales_receipt(
            &mut f.book,
            f.u,
            f.c,
            a["id"].as_str().unwrap(),
            Some(1),
            &json!("2026-10-03"),
            &json!("Correction")
        )
        .is_err()
    );
    assert_eq!(f.snap(), before);
    reverse_deposit(
        &mut f.book,
        f.u,
        f.c,
        d["id"].as_str().unwrap(),
        Some(1),
        &json!("2026-10-05"),
        &json!("Correction"),
    )
    .unwrap();
    let mut p = f.payload(vec![a["id"].clone()]);
    assert!(review_deposit(&mut f.book, f.u, f.c, &p).is_err());
    p["date"] = json!("2026-10-05");
    f.record(&p);
    assert!(f.book.check().unwrap().is_empty());
}
#[test]
fn selection_duplicate_direct_bank_reversed_and_malformed_refusals_are_atomic() {
    let mut f = Fx::new();
    let a = f.sale("10");
    for ids in [
        json!([]),
        json!([a["id"], a["id"]]),
        json!([true]),
        json!([new_id()]),
        json!(null),
    ] {
        let mut p = f.payload(vec![]);
        p["receipt_ids"] = ids;
        let before = f.snap();
        assert!(review_deposit(&mut f.book, f.u, f.c, &p).is_err());
        assert_eq!(f.snap(), before);
    }
    let mut p = f.payload(vec![a["id"].clone()]);
    p["date"] = json!("2026-09-01");
    assert!(review_deposit(&mut f.book, f.u, f.c, &p).is_err());
}
#[test]
fn stale_review_cannot_post_changed_selection_or_mapping() {
    let mut f = Fx::new();
    let a = f.sale("10");
    let mut p = f.payload(vec![a["id"].clone()]);
    let r = review_deposit(&mut f.book, f.u, f.c, &p).unwrap();
    p["memo"] = json!("Changed");
    let before = f.snap();
    assert!(record_deposit(&mut f.book, f.u, f.c, &p, &r).is_err());
    assert_eq!(f.snap(), before);
}
#[test]
fn statement_example_reconciles_exactly_and_one_cent_refuses() {
    let mut f = Fx::new();
    let a = f.journal("2026-10-01", true, "6200.27");
    let b = f.journal("2026-10-02", false, "266.85");
    f.clear(&a);
    f.clear(&b);
    let mut r = f.begin("5933.43");
    let before = f.snap();
    let err = reconcile_now(
        &mut f.book,
        f.u,
        f.c,
        r["id"].as_str().unwrap(),
        &f.bank.to_db(),
        Some(1),
        &json!(new_id()),
    )
    .unwrap_err();
    assert!(err.message().contains("0.01"));
    assert_eq!(f.snap(), before);
    r=begin(&mut f.book,f.u,f.c,&f.bank.to_db(),&json!({"request_key":new_id(),"expected_revision":1,"statement_date":"2026-10-31","ending_balance":"5933.42"})).unwrap();
    let totals = f.book.read(|q| figures(q, f.c, &r)).unwrap();
    assert_eq!(totals["difference"], "0.00");
    let r = f.finish(&r);
    assert_eq!(r["cleared_count"], 2);
    assert_eq!(r["beginning_balance"], "0.00");
    assert!(f.book.check().unwrap().is_empty());
}
#[test]
fn beginning_chain_undo_returns_cleared_and_retains_membership() {
    let mut f = Fx::new();
    let a = f.journal("2026-10-01", true, "10");
    f.clear(&a);
    let r = f.begin("10");
    let r = f.finish(&r);
    let next = f.begin("10");
    let next = f.finish(&next);
    assert!(
        undo_last(
            &mut f.book,
            f.u,
            f.c,
            r["id"].as_str().unwrap(),
            &f.bank.to_db(),
            r["revision"].as_i64(),
            &json!("Correction"),
            &json!(new_id())
        )
        .is_err()
    );
    undo_last(
        &mut f.book,
        f.u,
        f.c,
        next["id"].as_str().unwrap(),
        &f.bank.to_db(),
        next["revision"].as_i64(),
        &json!("Correction"),
        &json!(new_id()),
    )
    .unwrap();
    undo_last(
        &mut f.book,
        f.u,
        f.c,
        r["id"].as_str().unwrap(),
        &f.bank.to_db(),
        r["revision"].as_i64(),
        &json!("Correction"),
        &json!(new_id()),
    )
    .unwrap();
    let row = f.book.read(|q| cleared::line(q, f.c, &a)).unwrap();
    assert_eq!(row["state"], "C");
    assert_eq!(
        f.book
            .connection()
            .query_row("SELECT count(*) FROM banking_reconciliationline", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
}
#[test]
fn clearing_is_idempotent_and_revision_checked() {
    let mut f = Fx::new();
    let key = f.journal("2026-10-01", true, "10");
    f.clear(&key);
    let before = f.snap();
    set_cleared(&mut f.book, f.u, f.c, &key, "C", Some(0), None).unwrap();
    assert_eq!(f.snap(), before);
    assert!(set_cleared(&mut f.book, f.u, f.c, &key, "U", Some(0), None).is_err());
    assert_eq!(f.snap(), before);
    set_cleared(&mut f.book, f.u, f.c, &key, "U", Some(1), None).unwrap();
}
#[test]
fn cleared_journal_reversal_is_blocked_by_sql_guard() {
    let mut f = Fx::new();
    let key = f.journal("2026-10-01", true, "10");
    f.clear(&key);
    let row = f.book.read(|q| cleared::line(q, f.c, &key)).unwrap();
    let before = f.snap();
    assert!(
        services::reverse_journal(
            &mut f.book,
            f.u,
            f.c,
            row["journal_id"].as_str().unwrap().parse().unwrap(),
            "2026-10-02",
            "Correction"
        )
        .is_err()
    );
    assert_eq!(f.snap(), before);
}
#[test]
fn statement_cutoff_and_hide_later_preserve_late_cleared_lines() {
    let mut f = Fx::new();
    let a = f.journal("2026-10-01", true, "10");
    let b = f.journal("2026-11-01", true, "5");
    f.clear(&a);
    f.clear(&b);
    let mut r = f.begin("10");
    assert_eq!(
        f.book.read(|q| figures(q, f.c, &r)).unwrap()["cleared"],
        "10.00"
    );
    r = save_ticks(
        &mut f.book,
        f.u,
        f.c,
        r["id"].as_str().unwrap(),
        &f.bank.to_db(),
        &json!({"expected_revision":1,"shown":{},"ticked":[],"hide_later":false}),
    )
    .unwrap();
    assert_eq!(
        f.book.read(|q| figures(q, f.c, &r)).unwrap()["cleared"],
        "15.00"
    );
}
#[test]
fn service_charge_and_interest_are_posted_once_and_kept_after_undo() {
    let mut f = Fx::new();
    let p = json!({"request_key":new_id(),"statement_date":"2026-10-31","ending_balance":"-1.00","service_charge":"2.25","service_account_id":f.expense.to_db(),"interest":"1.25","interest_account_id":f.income.to_db()});
    let r = begin(&mut f.book, f.u, f.c, &f.bank.to_db(), &p).unwrap();
    let before = f.snap();
    begin(&mut f.book, f.u, f.c, &f.bank.to_db(), &p).unwrap();
    assert_eq!(f.snap(), before);
    let r = f.finish(&r);
    undo_last(
        &mut f.book,
        f.u,
        f.c,
        r["id"].as_str().unwrap(),
        &f.bank.to_db(),
        r["revision"].as_i64(),
        &json!("Correction"),
        &json!(new_id()),
    )
    .unwrap();
    assert_eq!(
        f.book
            .connection()
            .query_row(
                "SELECT count(*) FROM ledger_journal WHERE status='POSTED'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        2
    );
}
#[test]
fn register_filters_hide_rows_without_changing_running_balances() {
    let mut f = Fx::new();
    let a = f.journal("2026-10-01", true, "10");
    f.journal("2026-10-02", false, "3");
    f.clear(&a);
    let r = build(
        &mut f.book,
        f.u,
        f.c,
        &f.bank.to_db(),
        &json!({"state":"U"}),
        1,
        50,
    )
    .unwrap();
    assert_eq!(r["total"], 1);
    assert_eq!(r["rows"][0]["balance"], "7.00");
    assert_eq!(r["totals"]["cleared"], "10.00");
    assert_eq!(r["closing"], "7.00");
}
#[test]
fn save_ticks_checks_each_revision_and_rolls_back_prior_ticks() {
    let mut f = Fx::new();
    let a = f.journal("2026-10-01", true, "10");
    let b = f.journal("2026-10-02", true, "5");
    f.clear(&b);
    let r = f.begin("15");
    let before = f.snap();
    let input = json!({"expected_revision":1,"shown":{a.clone():0,b.clone():0},"ticked":[a]});
    assert!(
        save_ticks(
            &mut f.book,
            f.u,
            f.c,
            r["id"].as_str().unwrap(),
            &f.bank.to_db(),
            &input
        )
        .is_err()
    );
    assert_eq!(f.snap(), before);
}

fn history() -> Fx {
    let mut f = Fx::new();
    let a = f.sale("10");
    let p = f.payload(vec![a["id"].clone()]);
    let d = f.record(&p);
    reverse_deposit(
        &mut f.book,
        f.u,
        f.c,
        d["id"].as_str().unwrap(),
        Some(1),
        &json!("2026-10-03"),
        &json!("Correction"),
    )
    .unwrap();
    let a = f.journal("2026-10-04", true, "5");
    f.clear(&a);
    let r = f.begin("5");
    f.finish(&r);
    f
}
const TABLES: &[&str] = &[
    "banking_deposit",
    "banking_depositline",
    "banking_depositreversal",
    "banking_reconciliation",
    "banking_banklinestatus",
    "banking_reconciliationline",
    "banking_seal",
    "banking_seal_due",
    "banking_reconciliationactor",
];
// Attacker's re-keyed primary key: canonical, so 0130 passes and the table's own guard decides.
const FRESH_KEY: &str = "00000000-0000-7000-8000-0000000d0001";
/// The guard that refused `result`; 0130's `fb_valid` never counts.
fn refused(result: Result<(), fb_db::rusqlite::Error>, what: &str) -> String {
    let e = result.expect_err(what);
    let constraint = fb_db::constraint_of(&e).unwrap_or_else(|| e.to_string());
    assert_ne!(constraint, "fb_valid", "masked by fb_valid: {what}: {e}");
    constraint
}
/// Every attack in the matrix below and the guard that must refuse it (sorted).
const EXPECTED_MATRIX_GUARDS: &[&str] = &[
    "banking_banklinestatus delete: banking_kept",
    "banking_banklinestatus rekey: primary_key_immutable",
    "banking_banklinestatus replace: banking_key_collision",
    "banking_banklinestatus upsert: banking_key_collision",
    "banking_deposit delete: banking_kept",
    "banking_deposit rekey: banking_immutable",
    "banking_deposit replace: banking_key_collision",
    "banking_deposit upsert: banking_key_collision",
    "banking_depositline delete: banking_kept",
    "banking_depositline rekey: banking_immutable",
    "banking_depositline replace: banking_key_collision",
    "banking_depositline upsert: banking_key_collision",
    "banking_depositreversal delete: banking_kept",
    "banking_depositreversal rekey: banking_immutable",
    "banking_depositreversal replace: banking_key_collision",
    "banking_depositreversal upsert: banking_key_collision",
    "banking_reconciliation delete: banking_kept",
    "banking_reconciliation rekey: reconciliation_revision",
    "banking_reconciliation replace: banking_key_collision",
    "banking_reconciliation upsert: banking_key_collision",
    "banking_reconciliationactor delete: banking_actor_immutable",
    "banking_reconciliationactor rekey: banking_actor_immutable",
    "banking_reconciliationactor replace: banking_actor_immutable",
    "banking_reconciliationactor upsert: banking_actor_immutable",
    "banking_reconciliationline delete: banking_kept",
    "banking_reconciliationline rekey: banking_immutable",
    "banking_reconciliationline replace: banking_key_collision",
    "banking_reconciliationline upsert: banking_key_collision",
    "banking_seal delete: banking_kept",
    "banking_seal rekey: banking_immutable",
    "banking_seal replace: banking_key_collision",
    "banking_seal upsert: banking_key_collision",
    "banking_seal_due delete: banking_kept",
    "banking_seal_due rekey: banking_immutable",
    "banking_seal_due replace: banking_key_collision",
    "banking_seal_due upsert: banking_key_collision",
];
#[test]
fn full_replacement_rekey_hidden_identity_and_retention_matrix() {
    let f = history();
    let q = f.book.connection();
    for recursive in [0, 1] {
        q.execute_batch(&format!("PRAGMA recursive_triggers={recursive}"))
            .unwrap();
        let mut hit = std::collections::BTreeSet::new();
        for t in TABLES {
            let ddl: String = q
                .query_row("SELECT sql FROM sqlite_master WHERE name=?1", [t], |r| {
                    r.get(0)
                })
                .unwrap();
            assert!(ddl.contains("WITHOUT ROWID"));
            let key = if t.starts_with("banking_seal") {
                "doc_id"
            } else if *t == "banking_reconciliationactor" {
                "phase"
            } else {
                "id"
            };
            // An `id` re-key gets a canonical UUID so 0130 passes; doc_id/phase are free text to 0130.
            let fresh = if key == "id" { FRESH_KEY } else { "fresh-key" };
            let before = f.snap();
            for (label, sql) in [
                (
                    "replace",
                    format!("INSERT OR REPLACE INTO {t} SELECT * FROM {t}"),
                ),
                (
                    "rekey",
                    format!("UPDATE OR REPLACE {t} SET {key}='{fresh}'"),
                ),
                ("delete", format!("DELETE FROM {t}")),
                (
                    "upsert",
                    format!(
                        "INSERT INTO {t} SELECT * FROM {t} WHERE true ON CONFLICT DO UPDATE SET {key}=excluded.{key}"
                    ),
                ),
            ] {
                hit.insert(format!(
                    "{t} {label}: {}",
                    refused(q.execute_batch(&sql), &sql)
                ));
                assert_eq!(f.snap(), before);
            }
            for alias in ["rowid", "_rowid_", "oid"] {
                for sql in [
                    format!("INSERT OR REPLACE INTO {t}({alias}) VALUES(1)"),
                    format!("UPDATE OR REPLACE {t} SET {alias}=1"),
                ] {
                    // Hidden-rowid attacks fail at prepare (no such column), never at a value guard.
                    let e = q.execute_batch(&sql).expect_err(&sql);
                    assert_eq!(fb_db::constraint_of(&e), None, "{sql}: {e}");
                    assert!(e.to_string().contains(alias), "{sql}: {e}");
                }
                assert_eq!(f.snap(), before);
            }
        }
        let hit = hit.iter().map(String::as_str).collect::<Vec<_>>();
        assert_eq!(
            hit, EXPECTED_MATRIX_GUARDS,
            "recursive_triggers={recursive}"
        );
    }
    q.execute_batch("PRAGMA recursive_triggers=ON").unwrap();
    assert!(f.book.check().unwrap().is_empty());
}
#[test]
fn late_membership_and_orphan_source_journals_cannot_commit() {
    let mut f = history();
    let before = f.snap();
    let d: String = f
        .book
        .connection()
        .query_row("SELECT id FROM banking_deposit LIMIT 1", [], |r| r.get(0))
        .unwrap();
    let result:FbResult<()>=f.book.write(|tx|{tx.execute("INSERT INTO banking_depositline SELECT ?1,deposit_id,receipt_id,undeposited_id,amount FROM banking_depositline WHERE deposit_id=?2",params![new_id(),d])?;Ok(())});
    assert!(result.is_err());
    assert_eq!(f.snap(), before);
    let result: FbResult<()> = f.book.write(|tx| {
        let source = documents::create_identity(tx, f.u, f.c, "DEP", &new_id())?;
        documents::post_source(
            tx,
            f.u,
            f.c,
            source["id"].as_str().unwrap(),
            "2026-10-05",
            "Orphan",
            vec![
                json!({"account":f.bank.to_db(),"debit":"1"}),
                json!({"account":f.income.to_db(),"credit":"1"}),
            ],
        )?;
        Ok(())
    });
    assert!(result.is_err());
    assert_eq!(f.snap(), before);
}
#[test]
fn database_completion_rejects_malformed_or_one_cent_totals() {
    let mut f = Fx::new();
    let a = f.journal("2026-10-01", true, "10");
    f.clear(&a);
    let r = f.begin("10");
    let before = f.snap();
    let result:FbResult<()>=f.book.write(|tx|{tx.execute("UPDATE banking_reconciliation SET status='DONE',revision=2,data=json_set(data,'$.status','DONE','$.revision',2,'$.beginning_balance','0.00','$.cleared_debit','10.00','$.cleared_credit','0.00','$.cleared_count',0,'$.completion_key',?1,'$.completed_by','banking-test') WHERE id=?2",params![new_id(),s(&r,"id")?])?;seal(tx,"reconciliation",&format!("{}:2",s(&r,"id")?))?;Ok(())});
    assert!(result.is_err());
    assert_eq!(f.snap(), before);
}
#[test]
fn native_banking_functions_are_pure_in_read_and_write_callbacks() {
    let mut f = Fx::new();
    for read in [false, true] {
        fn run(q: &(impl Query + ?Sized)) -> FbResult<()> {
            assert_eq!(
                q.query_row("SELECT fb_banking_sum('[620027,-26685]')", [], |r| r
                    .get::<_, i64>(0))?,
                593342
            );
            assert!(
                q.query_row("SELECT fb_banking_sum('[true]')", [], |r| r
                    .get::<_, i64>(0))
                    .is_err()
            );
            Ok(())
        }
        if read {
            f.book.read(|q| run(q)).unwrap();
        } else {
            f.book.write(|q| run(q)).unwrap();
        }
    }
    assert!(f.book.check().unwrap().is_empty());
}

#[path = "review_regressions.rs"]
mod independent_review;
#[path = "review_boundaries.rs"]
mod review_boundaries;

#[path = "dfix3_undo_tests.rs"]
mod dfix3_undo;
