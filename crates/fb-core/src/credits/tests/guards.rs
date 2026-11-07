//! Each named credit guard is exercised independently through raw DML.
use super::*;
use fb_db::rusqlite::{params_from_iter, types::Value as SqlValue};

fn insert(tx: &Writer<'_>, table: &str, v: &Value) -> FbResult<()> {
    let stmt = tx.prepare(&format!("SELECT * FROM {table} LIMIT 0"))?;
    let cols = stmt.column_names().map(str::to_owned).collect::<Vec<_>>();
    drop(stmt);
    let vals = cols
        .iter()
        .map(|k| -> FbResult<SqlValue> {
            if k == "data" {
                return Ok(SqlValue::Text(v.to_string()));
            }
            if k == "total" || k == "amount" {
                return Ok(SqlValue::Integer(cents(&v[k])?));
            }
            Ok(match &v[k] {
                Value::Null => SqlValue::Null,
                Value::String(s) => SqlValue::Text(s.clone()),
                Value::Bool(b) => SqlValue::Integer(i64::from(*b)),
                Value::Number(n) => SqlValue::Integer(
                    n.as_i64()
                        .ok_or_else(|| FbError::validation("bad test value"))?,
                ),
                _ => return Err(FbError::validation("bad test value")),
            })
        })
        .collect::<FbResult<Vec<_>>>()?;
    tx.execute(
        &format!(
            "INSERT INTO {table}({}) VALUES({})",
            cols.join(","),
            vec!["?"; cols.len()].join(",")
        ),
        params_from_iter(vals),
    )?;
    Ok(())
}
fn attack(
    f: &mut Fixture,
    seen: &mut BTreeSet<String>,
    label: &str,
    run: impl FnOnce(&Writer<'_>) -> FbResult<()>,
) {
    let before = snapshot(f);
    let error = f.book.write(run).expect_err(label);
    let actual = match error {
        FbError::Integrity { constraint, .. } => constraint,
        _ => None,
    };
    assert_eq!(actual.as_deref(), Some(label));
    seen.insert(label.into());
    assert_eq!(snapshot(f), before);
}
fn memo_shell(
    tx: &Writer<'_>,
    actor: UserId,
    c: CompanyId,
    base: &Value,
    plan: &Value,
    skew: bool,
    missing_mapping: bool,
) -> FbResult<Value> {
    let identity = documents::create_identity(tx, actor, c, "CM", &new_id())?;
    let source = identity["id"].as_str().unwrap();
    let mut mapping = json!({});
    for row in documents::source_accounts(tx, base["identity_id"].as_str().unwrap())? {
        mapping[row["role"].as_str().unwrap()] = row["account_id"].clone();
    }
    if missing_mapping {
        mapping.as_object_mut().unwrap().remove("collected_tax");
    }
    documents::snapshot_source_accounts(tx, actor, c, source, &mapping)?;
    let mut entries = plan["entries"].as_array().unwrap().clone();
    if skew {
        let last = entries.len() - 1;
        entries[0]["debit"] = cash(cents(&entries[0]["debit"])? + 1);
        entries[last]["credit"] = cash(cents(&entries[last]["credit"])? + 1);
    }
    let j = documents::post_source(
        tx,
        actor,
        c,
        source,
        base["date"].as_str().unwrap(),
        "Synthetic guard fixture",
        entries,
    )?;
    let mut m = base.clone();
    m["id"] = json!(new_id());
    m["identity_id"] = identity["id"].clone();
    m["journal_id"] = json!(j.id.to_db());
    Ok(m)
}
fn lines(tx: &Writer<'_>, memo: &Value, rows: &[Value]) -> FbResult<()> {
    for row in rows {
        let mut l = row.clone();
        l["id"] = json!(new_id());
        l["credit_memo_id"] = memo["id"].clone();
        insert(tx, "credits_creditmemoline", &l)?;
    }
    Ok(())
}
fn application(actor: UserId, c: CompanyId, memo: &Value, amount: &str) -> Value {
    json!({"id":new_id(),"company_id":c.to_db(),"credit_memo_id":memo["id"],"request_key":new_id(),"date":"2026-10-09","amount":amount,"actor_id":actor.to_db(),"actor_name":"Credits tester","creation_digest":"0".repeat(64),"review_digest":"0".repeat(64)})
}
fn allocation(app: &Value, inv: &Value, ar: AccountId, amount: &str) -> Value {
    json!({"id":new_id(),"application_id":app["id"],"invoice_id":inv["id"],"invoice_revision_id":inv["current"]["id"],"receivables_id":ar.to_db(),"amount":amount})
}
fn refund_shell(
    tx: &Writer<'_>,
    actor: UserId,
    c: CompanyId,
    base: &Value,
    ar: AccountId,
    bank: AccountId,
    options: (Option<AccountId>, bool),
) -> FbResult<Value> {
    let (extra_account, missing_mapping) = options;
    let identity = documents::create_identity(tx, actor, c, "RFD", &new_id())?;
    let source = identity["id"].as_str().unwrap();
    let mut mapping = json!({"bank":bank.to_db(),"receivables":ar.to_db()});
    if missing_mapping {
        mapping.as_object_mut().unwrap().remove("receivables");
    }
    documents::snapshot_source_accounts(tx, actor, c, source, &mapping)?;
    let entries = if let Some(other) = extra_account {
        vec![
            json!({"account":ar.to_db(),"debit":"0.99","credit":"0.00"}),
            json!({"account":other.to_db(),"debit":"0.01","credit":"0.00"}),
            json!({"account":bank.to_db(),"debit":"0.00","credit":"1.00"}),
        ]
    } else {
        vec![
            json!({"account":ar.to_db(),"debit":"1.00","credit":"0.00"}),
            json!({"account":bank.to_db(),"debit":"0.00","credit":"1.00"}),
        ]
    };
    let j = documents::post_source(
        tx,
        actor,
        c,
        source,
        "2026-10-11",
        "Synthetic refund guard",
        entries,
    )?;
    Ok(
        json!({"id":new_id(),"company_id":c.to_db(),"identity_id":identity["id"],"credit_memo_id":base["id"],"customer_id":base["customer_id"],"customer_snapshot":base["customer_snapshot"],"company_snapshot":base["company_snapshot"],"creation_digest":"0".repeat(64),"review_digest":"0".repeat(64),"date":"2026-10-11","amount":"1.00","bank_id":bank.to_db(),"receivables_id":ar.to_db(),"method":"CHECK","check_number":"1001","memo":"Guard refund","journal_id":j.id.to_db(),"actor_id":actor.to_db(),"actor_name":"Credits tester"}),
    )
}
fn void_value(actor: UserId, parent: &str, key: &str, journal: &str, d: &str) -> Value {
    let mut v = json!({"id":new_id(),"journal_id":journal,"date":d,"reason":"Synthetic correction","actor_id":actor.to_db(),"actor_name":"Credits tester"});
    v[parent] = json!(key);
    v
}
fn invoice_void(
    tx: &Writer<'_>,
    actor: UserId,
    c: CompanyId,
    inv: &Value,
    d: &str,
) -> FbResult<()> {
    let j = documents::reverse_source(
        tx,
        actor,
        c,
        inv["identity_id"].as_str().unwrap(),
        d,
        "Synthetic raw void",
    )?;
    let mut h = inv["current"].clone();
    h["id"] = json!(new_id());
    h["number"] = json!(3);
    h["state"] = json!("VOID");
    h["action"] = json!("voided");
    h["reason"] = json!("Synthetic raw void");
    h["reversal_id"] = json!(j.id.to_db());
    tx.execute(
        "UPDATE sales_invoice SET revision=3 WHERE id=?1",
        [inv["id"].as_str().unwrap()],
    )?;
    tx.execute("INSERT INTO sales_invoicerevision(id,invoice_id,number,customer_id,date,due_date,state,total,journal_id,reversal_id,data,actor_id) VALUES(?1,?2,3,?3,?4,?5,'VOID',?6,?7,?8,?9,?10)",params![h["id"].as_str(),inv["id"].as_str(),h["customer_id"].as_str(),h["date"].as_str(),h["due_date"].as_str(),cents(&h["total"])?,h["journal_id"].as_str(),j.id.to_db(),h.to_string(),actor.to_db()])?;
    Ok(())
}
#[test]
fn every_credit_guard_has_an_independent_executed_raw_sql_regression() {
    let mut f = Fixture::new();
    let p = f.input();
    let plan = review_credit_memo(&mut f.book, f.actor, f.company, &p)
        .unwrap()
        .display;
    let m = f.memo();
    let base = get_credit_memo(&mut f.book, f.actor, f.company, m["id"].as_str().unwrap()).unwrap();
    let ls = base["lines"].as_array().unwrap().clone();
    let inv = f.invoice();
    let (actor, c, ar, bank, income) = (f.actor, f.company, f.ar, f.bank, f.income);
    let mut seen = BTreeSet::new();
    attack(&mut f, &mut seen, "credit_immutable", |tx| {
        insert(tx, "credits_creditmemo", &m)
    });
    attack(&mut f, &mut seen, "credit_fields", |tx| {
        let mut n = memo_shell(tx, actor, c, &m, &plan, false, false)?;
        n["memo"] = json!(12);
        insert(tx, "credits_creditmemo", &n)
    });
    attack(&mut f, &mut seen, "credit_memo_scope", |tx| {
        let mut n = memo_shell(tx, actor, c, &m, &plan, false, false)?;
        n["customer_id"] = json!(new_id());
        insert(tx, "credits_creditmemo", &n)
    });
    attack(&mut f, &mut seen, "credit_memo_tax", |tx| {
        let mut n = memo_shell(tx, actor, c, &m, &plan, false, false)?;
        n["collected_tax_id"] = Value::Null;
        insert(tx, "credits_creditmemo", &n)
    });
    attack(&mut f, &mut seen, "credit_memo_complete", |tx| {
        let n = memo_shell(tx, actor, c, &m, &plan, false, false)?;
        insert(tx, "credits_creditmemo", &n)?;
        seal(tx, "memo", n["id"].as_str().unwrap())
    });
    attack(&mut f, &mut seen, "credit_memo_journal_split", |tx| {
        let n = memo_shell(tx, actor, c, &m, &plan, true, false)?;
        insert(tx, "credits_creditmemo", &n)?;
        lines(tx, &n, &ls)?;
        seal(tx, "memo", n["id"].as_str().unwrap())
    });
    attack(&mut f, &mut seen, "credit_memo_account_evidence", |tx| {
        let n = memo_shell(tx, actor, c, &m, &plan, false, true)?;
        insert(tx, "credits_creditmemo", &n)?;
        lines(tx, &n, &ls)?;
        seal(tx, "memo", n["id"].as_str().unwrap())
    });
    attack(&mut f, &mut seen, "credit_memo_late_line", |tx| {
        let mut l = ls[0].clone();
        l["id"] = json!(new_id());
        l["position"] = json!(3);
        insert(tx, "credits_creditmemoline", &l)
    });
    attack(&mut f, &mut seen, "credit_memo_line_scope", |tx| {
        let n = memo_shell(tx, actor, c, &m, &plan, false, false)?;
        insert(tx, "credits_creditmemo", &n)?;
        let mut l = ls[0].clone();
        l["id"] = json!(new_id());
        l["credit_memo_id"] = n["id"].clone();
        l["income_account_id"] = json!(new_id());
        insert(tx, "credits_creditmemoline", &l)
    });
    attack(&mut f, &mut seen, "credit_complete", |tx| {
        seal(tx, "memo", &new_id())
    });
    attack(&mut f, &mut seen, "credit_journal_owner", |tx| {
        seal(tx, "journal", &new_id())
    });
    attack(&mut f, &mut seen, "credit_late_reference", |tx| {
        tx.execute("INSERT INTO documents_documentaccount(id,source_id,role,account_id,account_code,account_name,account_kind) VALUES(?1,?2,'extra',?3,'400','400','INCOME')",params![new_id(),m["identity_id"].as_str(),income.to_db()])?;
        Ok(())
    });
    attack(&mut f, &mut seen, "credit_application_scope", |tx| {
        let mut a = application(actor, c, &m, "1.00");
        a["credit_memo_id"] = json!(new_id());
        insert(tx, "credits_creditapplication", &a)
    });
    attack(&mut f, &mut seen, "credit_memo_overused", |tx| {
        insert(
            tx,
            "credits_creditapplication",
            &application(actor, c, &m, "13.31"),
        )
    });
    attack(&mut f, &mut seen, "credit_application_complete", |tx| {
        let a = application(actor, c, &m, "1.00");
        insert(tx, "credits_creditapplication", &a)?;
        seal(tx, "application", a["id"].as_str().unwrap())
    });
    attack(&mut f, &mut seen, "credit_application_target", |tx| {
        let a = application(actor, c, &m, "1.00");
        insert(tx, "credits_creditapplication", &a)?;
        let mut l = allocation(&a, &inv, ar, "1.00");
        l["invoice_revision_id"] = json!(new_id());
        insert(tx, "credits_creditapplicationline", &l)
    });
    attack(&mut f, &mut seen, "credit_application_cross_ar", |tx| {
        let a = application(actor, c, &m, "1.00");
        insert(tx, "credits_creditapplication", &a)?;
        let mut l = allocation(&a, &inv, bank, "1.00");
        l["receivables_id"] = json!(bank.to_db());
        insert(tx, "credits_creditapplicationline", &l)
    });
    attack(
        &mut f,
        &mut seen,
        "credit_application_open_at_commit",
        |tx| {
            let a = application(actor, c, &m, "1.00");
            insert(tx, "credits_creditapplication", &a)?;
            insert(
                tx,
                "credits_creditapplicationline",
                &allocation(&a, &inv, ar, "1.00"),
            )?;
            tx.execute(
                "UPDATE sales_invoice SET revision=3 WHERE id=?1",
                [inv["id"].as_str().unwrap()],
            )?;
            seal(tx, "application", a["id"].as_str().unwrap())
        },
    );
    attack(&mut f, &mut seen, "credit_unapplication_fields", |tx| {
        let v = json!({"id":new_id(),"application_id":new_id(),"date":"2026-10-20","reason":"Release","actor_id":actor.to_db(),"actor_name":"Credits tester"});
        insert(tx, "credits_creditunapplication", &v)
    });
    attack(&mut f, &mut seen, "credit_refund_scope", |tx| {
        let mut r = refund_shell(tx, actor, c, &m, ar, bank, (None, false))?;
        r["customer_id"] = json!(new_id());
        insert(tx, "credits_creditrefund", &r)
    });
    attack(&mut f, &mut seen, "credit_refund_bank", |tx| {
        let mut r = refund_shell(tx, actor, c, &m, ar, bank, (None, false))?;
        r["bank_id"] = json!(ar.to_db());
        insert(tx, "credits_creditrefund", &r)
    });
    attack(&mut f, &mut seen, "credit_refund_journal_split", |tx| {
        let r = refund_shell(tx, actor, c, &m, ar, bank, (Some(income), false))?;
        insert(tx, "credits_creditrefund", &r)?;
        seal(tx, "refund", r["id"].as_str().unwrap())
    });
    attack(&mut f, &mut seen, "credit_refund_account_evidence", |tx| {
        let r = refund_shell(tx, actor, c, &m, ar, bank, (None, true))?;
        insert(tx, "credits_creditrefund", &r)?;
        seal(tx, "refund", r["id"].as_str().unwrap())
    });
    attack(&mut f, &mut seen, "credit_memo_void_link", |tx| {
        insert(
            tx,
            "credits_creditmemovoid",
            &void_value(
                actor,
                "credit_memo_id",
                m["id"].as_str().unwrap(),
                &new_id(),
                "2026-10-21",
            ),
        )
    });
    attack(&mut f, &mut seen, "credit_refund_void_link", |tx| {
        insert(
            tx,
            "credits_creditrefundvoid",
            &void_value(actor, "refund_id", &new_id(), &new_id(), "2026-10-20"),
        )
    });
    let ap = f.application(&inv, "1.00");
    let r = review_application(&mut f.book, actor, c, m["id"].as_str().unwrap(), &ap).unwrap();
    let a = apply_credit(
        &mut f.book,
        actor,
        c,
        m["id"].as_str().unwrap(),
        &ap,
        Some(&r),
    )
    .unwrap();
    attack(&mut f, &mut seen, "credit_application_late_line", |tx| {
        let mut l = allocation(&a, &inv, ar, "1.00");
        l["invoice_id"] = json!(new_id());
        insert(tx, "credits_creditapplicationline", &l)
    });
    attack(&mut f, &mut seen, "credit_memo_live_dependency", |tx| {
        let j = documents::reverse_source(
            tx,
            actor,
            c,
            m["identity_id"].as_str().unwrap(),
            "2026-10-21",
            "Void",
        )?;
        insert(
            tx,
            "credits_creditmemovoid",
            &void_value(
                actor,
                "credit_memo_id",
                m["id"].as_str().unwrap(),
                &j.id.to_db(),
                "2026-10-21",
            ),
        )
    });
    // Exercise the older credits backstop independently of the shared guard.
    let shared:String=f.book.connection().query_row("SELECT sql FROM sqlite_master WHERE name='fb_invoice_shared_settlements'",[],|r|r.get(0)).unwrap();
    f.book.connection().execute_batch("DROP TRIGGER fb_invoice_shared_settlements").unwrap();
    attack(&mut f, &mut seen, "invoice_live_credits", |tx| {
        invoice_void(tx, actor, c, &inv, "2026-10-20")
    });
    f.book.connection().execute_batch(&shared).unwrap();
    // Raw receipt allocation cannot consume the balance already settled by credit.
    attack(&mut f, &mut seen, "invoice_overcredited", |tx| {
        let a = application(actor, c, &m, "1.00");
        insert(tx, "credits_creditapplication", &a)?;
        let small = inv.clone();
        let mut l = allocation(&a, &small, ar, "34.90");
        l["amount"] = inv["current"]["total"].clone();
        insert(tx, "credits_creditapplicationline", &l)
    });
    unapply_credit(
        &mut f.book,
        actor,
        c,
        a["id"].as_str().unwrap(),
        Some(1),
        &json!("2026-10-20"),
        &json!("Release"),
    )
    .unwrap();
    attack(&mut f, &mut seen, "credit_memo_release_date", |tx| {
        let j = documents::reverse_source(
            tx,
            actor,
            c,
            m["identity_id"].as_str().unwrap(),
            "2026-10-19",
            "Void",
        )?;
        insert(
            tx,
            "credits_creditmemovoid",
            &void_value(
                actor,
                "credit_memo_id",
                m["id"].as_str().unwrap(),
                &j.id.to_db(),
                "2026-10-19",
            ),
        )
    });
    attack(&mut f, &mut seen, "invoice_credit_release_date", |tx| {
        invoice_void(tx, actor, c, &inv, "2026-10-19")
    });
    let rp = f.refund_input("1.00");
    let r = review_refund(&mut f.book, actor, c, m["id"].as_str().unwrap(), &rp).unwrap();
    record_refund(
        &mut f.book,
        actor,
        c,
        m["id"].as_str().unwrap(),
        &rp,
        Some(&r),
    )
    .unwrap();
    attack(&mut f, &mut seen, "credit_refund_number_in_use", |tx| {
        let r = refund_shell(tx, actor, c, &m, ar, bank, (None, false))?;
        insert(tx, "credits_creditrefund", &r)
    });
    let shared:String=f.book.connection().query_row("SELECT sql FROM sqlite_master WHERE name='fb_number_payables_billpayment_insert'",[],|r|r.get(0)).unwrap();
    f.book.connection().execute_batch("DROP TRIGGER fb_number_payables_billpayment_insert").unwrap();
    attack(&mut f, &mut seen, "bill_payment_check_number", |tx| {
        tx.execute("INSERT INTO payables_billpayment(id,company_id,bank_id,method,reference,vendor_snapshot,company_snapshot) VALUES(?1,?2,?3,'CHECK','1001','{}','{}')",params![new_id(),c.to_db(),bank.to_db()])?;
        Ok(())
    });
    f.book.connection().execute_batch(&shared).unwrap();
    attack(&mut f, &mut seen, "credit_fields", |tx| {
        let mut r = refund_shell(tx, actor, c, &m, ar, bank, (None, false))?;
        r["check_number"] = json!("1\n2");
        insert(tx, "credits_creditrefund", &r)
    });
    let names = include_str!("../../../../fb-db/sql/0060_credits.sql")
        .split("RAISE(ABORT,'")
        .skip(1)
        .filter_map(|s| s.split_once(':').map(|(n, _)| n.to_owned()))
        .collect::<BTreeSet<_>>();
    let missing = names.difference(&seen).collect::<Vec<_>>();
    assert!(
        missing.is_empty(),
        "guards without raw regression: {missing:?}"
    );
    println!(
        "Executed {} named credit guards with exact unchanged book snapshots",
        names.len()
    );
    f.clean();
}

include!("review_cases.rs");
include!("open_cases.rs");

include!("review_boundaries.rs");
