//! Raw-SQL attacks, including every payables guard family and the Python deferred-parent hole.
use super::*;
// Absent ids: canonical UUIDs that exist nowhere, so 0130 passes and the intended guard decides.
const MISSING_JOURNAL: &str = "00000000-0000-7000-8000-0000000b0001"; // absent bill/payment/reversal journal
const MISSING_IDENTITY: &str = "00000000-0000-7000-8000-0000000b0002"; // absent payment identity
const MISSING_REVISION: &str = "00000000-0000-7000-8000-0000000b0003"; // future line/allocation revision
const MISSING_BILL: &str = "00000000-0000-7000-8000-0000000b0004"; // future allocation bill
const MISSING_PAYMENT: &str = "00000000-0000-7000-8000-0000000b0005"; // future allocation payment
const MISSING_COMPANY: &str = "00000000-0000-7000-8000-0000000b0006"; // company a parent tries to move to
const FRESH_KEY: &str = "00000000-0000-7000-8000-0000000b0007"; // attacker's alternate/changed primary key
thread_local! {static GUARDS:std::cell::RefCell<BTreeSet<String>>=std::cell::RefCell::default();}
pub(super) fn observed(message: String) {
    GUARDS.with(|g| {
        g.borrow_mut().insert(message);
    });
}
/// Records the refusal and returns the guard that raised it; 0130's `fb_valid` never counts.
fn sql_refused(result: Result<usize, fb_db::rusqlite::Error>) -> String {
    let e = result.unwrap_err();
    let constraint = fb_db::constraint_of(&e).unwrap_or_default();
    assert_ne!(constraint, "fb_valid", "masked by fb_valid: {e}");
    observed(e.to_string());
    constraint
}

fn refuse(f: &mut Fx, want: &str, attack: impl FnOnce(&Writer<'_>) -> FbResult<()>) {
    let before = f.counts();
    let e = f.book.write(attack).unwrap_err();
    observed(format!("{}: {}", e.constraint().unwrap_or(""), e.message()));
    assert_ne!(e.constraint(), Some("fb_valid"), "masked by fb_valid: {e}");
    assert!(e.message().contains(want), "wanted {want}, got {e:?}");
    assert_eq!(before, f.counts());
}
pub(super) fn new_revision(tx: &Writer<'_>, b: &Value, state: &str) -> FbResult<Value> {
    let n = b["revision"].as_i64().unwrap() + 1;
    tx.execute(
        "UPDATE payables_bill SET revision=?,state=? WHERE id=?",
        params![n, state, s(b, "id")],
    )?;
    let mut r = b["current"].clone();
    r.as_object_mut().unwrap().remove("lines");
    r["id"] = json!(fb_ids::new_v7().to_string());
    r["number"] = json!(n);
    r["state"] = json!(state);
    Ok(r)
}
fn posting_revision(
    tx: &Writer<'_>,
    actor: UserId,
    co: CompanyId,
    b: &Value,
    duplicate: bool,
) -> FbResult<Value> {
    let (mapping, mut entries, review) = posting(tx, actor, co, b)?;
    documents::snapshot_source_accounts(tx, actor, co, s(b, "identity_id"), &mapping)?;
    if duplicate {
        entries.extend(entries.clone());
    }
    let j = documents::post_source(
        tx,
        actor,
        co,
        s(b, "identity_id"),
        s(&b["current"], "date"),
        "Probe",
        entries,
    )?;
    let mut r = new_revision(tx, b, "OPEN")?;
    r["journal_id"] = json!(j.id.to_db());
    r["review_digest"] = json!(review.digest);
    r["action"] = json!("posted");
    Ok(r)
}
fn revision_lines(tx: &Writer<'_>, r: &Value, b: &Value, change: bool) -> FbResult<()> {
    insert(tx, "payables_billrevision", r)?;
    for mut l in b["current"]["lines"].as_array().unwrap().clone() {
        l["id"] = json!(fb_ids::new_v7().to_string());
        l["revision_id"] = r["id"].clone();
        if change {
            l["description"] = json!("Changed financial evidence");
        }
        insert(tx, "payables_billline", &l)?;
    }
    tx.execute(
        "INSERT INTO fb_seal VALUES('bill_revision',?)",
        [format!("{}:{}", s(b, "id"), r["number"])],
    )?;
    Ok(())
}
#[allow(clippy::too_many_arguments)] // Raw adversarial fixture, not a public API.
pub(super) fn raw_payment_data(
    tx: &Writer<'_>,
    actor: UserId,
    co: CompanyId,
    b: &Value,
    bank: AccountId,
    ap: AccountId,
    duplicate: bool,
    extra: bool,
) -> FbResult<Value> {
    let identity =
        documents::create_identity(tx, actor, co, "BPAY", &fb_ids::new_v7().to_string())?;
    let mut map = json!({"bank":bank.to_db(),format!("payables:{}",ap.to_db()):ap.to_db()});
    if extra {
        map["extra"] = json!(ap.to_db());
    }
    documents::snapshot_source_accounts(tx, actor, co, s(&identity, "id"), &map)?;
    let mut entries = vec![
        json!({"account":bank.to_db(),"debit":"0","credit":"1.01"}),
        json!({"account":ap.to_db(),"debit":"1.01","credit":"0"}),
    ];
    if duplicate {
        entries.extend(entries.clone());
    }
    let j = documents::post_source(
        tx,
        actor,
        co,
        s(&identity, "id"),
        "2026-10-24",
        "Probe",
        entries,
    )?;
    let u = services::access(tx, actor, co)?;
    let p = json!({"id":fb_ids::new_v7().to_string(),"company_id":co.to_db(),"identity_id":identity["id"],"creation_digest":"a".repeat(64),"review_digest":"b".repeat(64),"vendor_id":b["vendor_id"],"vendor_snapshot":b["current"]["vendor_snapshot"],"company_snapshot":b["current"]["company_snapshot"],"date":"2026-10-24","amount":101,"method":"OTHER","reference":"","memo":"","bank_id":bank.to_db(),"journal_id":j.id.to_db(),"actor_id":actor.to_db(),"actor_name":u.username,"created_at":py::now_utc()});
    Ok(p)
}
#[allow(clippy::too_many_arguments)]
fn raw_payment(
    tx: &Writer<'_>,
    actor: UserId,
    co: CompanyId,
    b: &Value,
    bank: AccountId,
    ap: AccountId,
    duplicate: bool,
    extra: bool,
) -> FbResult<Value> {
    let p = raw_payment_data(tx, actor, co, b, bank, ap, duplicate, extra)?;
    insert(tx, "payables_billpayment", &p)?;
    Ok(p)
}
fn alloc(p: &Value, b: &Value, ap: AccountId) -> Value {
    json!({"id":fb_ids::new_v7().to_string(),"payment_id":p["id"],"bill_id":b["id"],"bill_revision_id":b["current"]["id"],"payables_id":ap.to_db(),"amount":101})
}
#[test]
fn root_revision_and_line_guards() {
    let mut f = Fx::new();
    let p = f.payload();
    let out = f.save(&p);
    let b = bill(f.book.connection(), f.co, s(&out, "id")).unwrap();
    let co = f.co;
    let actor = f.actor;
    refuse(&mut f, "Bill ownership is immutable", |tx| {
        tx.execute(
            "UPDATE payables_bill SET revision=revision+2 WHERE id=?",
            [s(&b, "id")],
        )?;
        Ok(())
    });
    refuse(&mut f, "Invalid bill identity", |tx| {
        let i = documents::create_identity(tx, actor, co, "BILL", &fb_ids::new_v7().to_string())?;
        let mut v = json!({"id":fb_ids::new_v7().to_string(),"company_id":co.to_db(),"identity_id":i["id"],"creation_digest":"a".repeat(64),"revision":2,"vendor_id":b["vendor_id"],"reference_key":"","state":"DRAFT"});
        v["revision"] = json!(2);
        insert(tx, "payables_bill", &v)
    });
    for (field, value, message) in [
        ("number", json!(3), "Invalid revision ownership"),
        ("state", json!("VOID"), "Invalid bill state transition"),
        (
            "reference",
            json!("different"),
            "Bill reference index does not match revision",
        ),
        (
            "supersedes_id",
            b["id"].clone(),
            "Reference reuse requires latest voided bill and reason",
        ),
        (
            "journal_id",
            json!(MISSING_JOURNAL),
            "Invalid bill posting linkage",
        ),
    ] {
        refuse(&mut f, message, |tx| {
            let mut r = new_revision(tx, &b, "DRAFT")?;
            r[field] = value;
            insert(tx, "payables_billrevision", &r)
        });
    }
    refuse(
        &mut f,
        "Invalid bill line scope or committed history",
        |tx| {
            let mut l = b["current"]["lines"][0].clone();
            l["id"] = json!(fb_ids::new_v7().to_string());
            l["position"] = json!(2);
            insert(tx, "payables_billline", &l)
        },
    );
    refuse(
        &mut f,
        "Invalid bill line scope or committed history",
        |tx| {
            let mut l = b["current"]["lines"][0].clone();
            l["id"] = json!(fb_ids::new_v7().to_string());
            l["revision_id"] = json!(MISSING_REVISION);
            insert(tx, "payables_billline", &l)
        },
    );
    refuse(&mut f, "Incomplete or inconsistent bill totals", |tx| {
        let r = new_revision(tx, &b, "DRAFT")?;
        insert(tx, "payables_billrevision", &r)?;
        tx.execute(
            "INSERT INTO fb_seal VALUES('bill_revision',?)",
            [format!("{}:2", s(&b, "id"))],
        )?;
        Ok(())
    });
    refuse(
        &mut f,
        "Posting and void preserve financial evidence",
        |tx| {
            let mut r = posting_revision(tx, actor, co, &b, false)?;
            r["memo"] = json!("Tampered");
            insert(tx, "payables_billrevision", &r)
        },
    );
    refuse(&mut f, "Posting and void must copy exact lines", |tx| {
        let r = posting_revision(tx, actor, co, &b, false)?;
        revision_lines(tx, &r, &b, true)
    });
    refuse(
        &mut f,
        "Bill must match exact expense/payable posting and frozen accounts",
        |tx| {
            let r = posting_revision(tx, actor, co, &b, true)?;
            revision_lines(tx, &r, &b, false)
        },
    );
    let expense = f.expense;
    refuse(
        &mut f,
        "Invalid purchase posting account or inactive parent",
        |tx| {
            let r = posting_revision(tx, actor, co, &b, false)?;
            tx.execute(
                "UPDATE ledger_account SET active=0 WHERE id=?",
                [expense.to_db()],
            )?;
            revision_lines(tx, &r, &b, false)
        },
    );
}
#[test]
fn payment_guards_and_exact_multiset() {
    let mut f = Fx::new();
    let p = f.payload();
    let b = f.save(&p);
    let b = f.post(&b);
    let b = bill(f.book.connection(), f.co, s(&b, "id")).unwrap();
    let (actor, co, bank, ap) = (f.actor, f.co, f.bank, f.ap);
    refuse(&mut f, "Invalid payment ownership/posting", |tx| {
        let p = raw_payment(tx, actor, co, &b, bank, ap, false, false)?;
        let mut p = p.clone();
        p["id"] = json!(fb_ids::new_v7().to_string());
        p["identity_id"] = json!(MISSING_IDENTITY);
        p["journal_id"] = json!(MISSING_JOURNAL);
        insert(tx, "payables_billpayment", &p)
    });
    refuse(&mut f, "Payment allocations must equal amount", |tx| {
        let p = raw_payment(tx, actor, co, &b, bank, ap, false, false)?;
        tx.execute(
            "INSERT INTO fb_seal VALUES('bill_payment',?)",
            [s(&p, "id")],
        )?;
        Ok(())
    });
    for (field, value, msg) in [
        (
            "bill_revision_id",
            json!(MISSING_REVISION),
            "Invalid allocation target or original payable",
        ),
        (
            "payables_id",
            json!(bank.to_db()),
            "Invalid allocation target or original payable",
        ),
        (
            "amount",
            json!(102),
            "Live allocations cannot overpay or target void bills",
        ),
    ] {
        refuse(&mut f, msg, |tx| {
            let p = raw_payment(tx, actor, co, &b, bank, ap, false, false)?;
            let mut a = alloc(&p, &b, ap);
            a[field] = value;
            insert(tx, "payables_paymentallocation", &a)
        });
    }
    refuse(
        &mut f,
        "Payment journal must exactly clear original payables",
        |tx| {
            let p = raw_payment(tx, actor, co, &b, bank, ap, true, false)?;
            insert(tx, "payables_paymentallocation", &alloc(&p, &b, ap))?;
            tx.execute(
                "INSERT INTO fb_seal VALUES('bill_payment',?)",
                [s(&p, "id")],
            )?;
            Ok(())
        },
    );
    refuse(
        &mut f,
        "Payment account evidence must match original split",
        |tx| {
            let p = raw_payment(tx, actor, co, &b, bank, ap, false, true)?;
            insert(tx, "payables_paymentallocation", &alloc(&p, &b, ap))?;
            tx.execute(
                "INSERT INTO fb_seal VALUES('bill_payment',?)",
                [s(&p, "id")],
            )?;
            Ok(())
        },
    );
    refuse(&mut f, "Journal requires complete owning lifecycle", |tx| {
        let p = raw_payment(tx, actor, co, &b, bank, ap, false, false)?;
        seal_journal(tx, s(&p, "journal_id"))
    });
    let payload = f.payment(&b, "1.01");
    let payment = f.pay(&payload);
    refuse(&mut f, "Cannot append to committed allocations", |tx| {
        let mut a = alloc(&payment, &b, ap);
        a["bill_id"] = json!(MISSING_BILL);
        insert(tx, "payables_paymentallocation", &a)
    });
    refuse(&mut f, "Cannot append to committed allocations", |tx| {
        let mut a = alloc(&payment, &b, ap);
        a["payment_id"] = json!(MISSING_PAYMENT);
        insert(tx, "payables_paymentallocation", &a)
    });
    refuse(
        &mut f,
        "Reverse live payments before voiding a bill",
        |tx| {
            let mut r = new_revision(tx, &b, "VOID")?;
            r["reason"] = json!("Bypass");
            insert(tx, "payables_billrevision", &r)
        },
    );
    refuse(&mut f, "Invalid payment reversal", |tx| {
        insert(
            tx,
            "payables_paymentreversal",
            &json!({"id":fb_ids::new_v7().to_string(),"payment_id":payment["id"],"journal_id":MISSING_JOURNAL,"date":"2026-10-24","reason":"Bad","actor_id":actor.to_db(),"actor_name":"bill-test","created_at":py::now_utc()}),
        )
    });
    refuse(&mut f, "Cannot append posted account evidence", |tx| {
        tx.execute(
            "INSERT INTO documents_documentaccount VALUES(?,?,?,?,?,?,?)",
            params![
                fb_ids::new_v7().to_string(),
                s(&payment, "identity_id"),
                "extra",
                bank.to_db(),
                "100",
                "Bank",
                "ASSET"
            ],
        )?;
        Ok(())
    });
}
#[test]
fn protected_parents_all_unique_keys_and_source_lines() {
    let mut f = Fx::new();
    let income = services::save_account(
        &mut f.book,
        f.actor,
        f.co,
        AccountInput::new("400", "Income").kind("INCOME"),
    )
    .unwrap()
    .id;
    let item=directory::save_record(&mut f.book,f.actor,f.co,Model::Item,&json!({"code":"thing","kind":"NONSTOCK","sale_rate":"0","purchase_cost":"3","income_account":income.to_db()}),None).unwrap();
    let mut p = f.payload();
    p["lines"][0]["item_id"] = item["id"].clone();
    let b = f.save(&p);
    let b = f.post(&b);
    let mut hit = BTreeSet::new();
    for mode in ["ON", "OFF"] {
        f.book
            .connection()
            .execute_batch(&format!("PRAGMA recursive_triggers={mode}"))
            .unwrap();
        for (table, condition) in [
            ("directory_item", format!("id='{}'", s(&item, "id"))),
            ("directory_vendor", format!("id='{}'", f.vendor)),
            ("ledger_account", format!("id='{}'", f.expense.to_db())),
            (
                "documents_documentidentity",
                format!("id='{}'", s(&b, "identity_id")),
            ),
            (
                "ledger_journal",
                format!("id='{}'", s(&b["current"], "journal_id")),
            ),
            (
                "documents_documentaccount",
                format!("source_id='{}'", s(&b, "identity_id")),
            ),
            (
                "ledger_journalline",
                format!("journal_id='{}'", s(&b["current"], "journal_id")),
            ),
        ] {
            let c = f.book.connection();
            let cols = c
                .prepare(&format!("SELECT * FROM {table}"))
                .unwrap()
                .column_names()
                .map(str::to_string)
                .collect::<Vec<_>>();
            hit.insert(format!("replace {table}: {}", sql_refused(c.execute(&format!("INSERT OR REPLACE INTO {table} SELECT * FROM {table} WHERE {condition} LIMIT 1"),[]))));
            hit.insert(format!(
                "delete {table}: {}",
                sql_refused(c.execute(&format!("DELETE FROM {table} WHERE {condition}"), []))
            ));
            if [
                "directory_item",
                "ledger_account",
                "documents_documentaccount",
                "ledger_journalline",
            ]
            .contains(&table)
            {
                let projection = cols
                    .iter()
                    .map(|k| if k == "id" { format!("'{FRESH_KEY}'") } else { k.clone() })
                    .collect::<Vec<_>>()
                    .join(",");
                hit.insert(format!("alternate {table}: {}", sql_refused(c.execute(&format!("INSERT OR REPLACE INTO {table} SELECT {projection} FROM {table} WHERE {condition} LIMIT 1"),[]))));
            }
        }
    }
    let expected: BTreeSet<String> = PROTECTED_PARENT_GUARDS.iter().map(|s| s.to_string()).collect();
    assert_eq!(hit, expected);
    f.book
        .connection()
        .execute_batch("PRAGMA recursive_triggers=ON")
        .unwrap();
    for (table, id) in [
        ("directory_item", s(&item, "id").to_string()),
        ("directory_vendor", f.vendor.clone()),
        ("ledger_account", income.to_db()),
    ] {
        refuse(&mut f, "Historical parents cannot move company", |tx| {
            tx.execute(
                &format!("UPDATE {table} SET company_id=? WHERE id=?"),
                params![MISSING_COMPANY, id],
            )?;
            Ok(())
        });
    }
}
// Pre-0130 behaviour: the deferred foreign key (not 0130) refuses an owner-less balanced reversal.
const SOURCE_REVERSAL_GUARD: &str = "foreign_key";
/// `<attack> <table>: <guard>` for every protected-parent statement (both recursion modes agree).
const PROTECTED_PARENT_GUARDS: &[&str] = &[
    "alternate directory_item: payables_parent",
    "alternate documents_documentaccount: payables_frozen_account",
    "alternate ledger_account: payables_parent",
    "alternate ledger_journalline: payables_posted_line",
    "delete directory_item: payables_parent",
    "delete directory_vendor: payables_parent",
    "delete documents_documentaccount: document_account_immutable",
    "delete documents_documentidentity: payables_parent",
    "delete ledger_account: payables_parent",
    "delete ledger_journal: payables_parent",
    "delete ledger_journalline: fb_line_guard",
    "replace directory_item: payables_parent",
    "replace directory_vendor: payables_parent",
    "replace documents_documentaccount: payables_frozen_account",
    "replace documents_documentidentity: payables_parent",
    "replace ledger_account: payables_parent",
    "replace ledger_journal: payables_parent",
    "replace ledger_journalline: payables_posted_line",
];
/// `<statement>: <guard>` for every raw statement in the every-guard test (`{FRESH_KEY}` is substituted).
const EVERY_GUARD_RAW_SQL: &[&str] = &[
    "INSERT OR REPLACE INTO payables_bill SELECT * FROM payables_bill LIMIT 1: bill_immutable_ownership",
    "DELETE FROM payables_bill: bill_immutable_ownership",
    "UPDATE payables_bill SET id='{FRESH_KEY}': payables_permanent_id",
    "INSERT OR REPLACE INTO payables_billrevision SELECT * FROM payables_billrevision LIMIT 1: bill_history_immutable",
    "DELETE FROM payables_billrevision: bill_history_immutable",
    "UPDATE payables_billrevision SET id='{FRESH_KEY}': payables_permanent_id",
    "INSERT OR REPLACE INTO payables_billline SELECT * FROM payables_billline LIMIT 1: bill_history_immutable",
    "DELETE FROM payables_billline: bill_history_immutable",
    "UPDATE payables_billline SET id='{FRESH_KEY}': payables_permanent_id",
    "INSERT OR REPLACE INTO payables_billpayment SELECT * FROM payables_billpayment LIMIT 1: payment_immutable",
    "DELETE FROM payables_billpayment: payment_immutable",
    "UPDATE payables_billpayment SET id='{FRESH_KEY}': payables_permanent_id",
    "INSERT OR REPLACE INTO payables_paymentallocation SELECT * FROM payables_paymentallocation LIMIT 1: payment_immutable",
    "DELETE FROM payables_paymentallocation: payment_immutable",
    "UPDATE payables_paymentallocation SET id='{FRESH_KEY}': payables_permanent_id",
    "INSERT OR REPLACE INTO payables_paymentreversal SELECT * FROM payables_paymentreversal LIMIT 1: payment_immutable",
    "DELETE FROM payables_paymentreversal: payment_immutable",
    "UPDATE payables_paymentreversal SET id='{FRESH_KEY}': payables_permanent_id",
    "DELETE FROM directory_vendor: payables_parent",
    "DELETE FROM ledger_account: payables_parent",
    "DELETE FROM documents_documentidentity: payables_parent",
    "DELETE FROM ledger_journal: payables_parent",
    "REPLACE fb_seal: payables_seal",
    "REPLACE fb_seal_due: payables_seal",
    "REPLACE ledger_journalline: payables_posted_line",
];
#[test]
fn source_reversal_requires_owner_even_when_balanced() {
    let mut f = Fx::new();
    let p = f.payload();
    let b = f.save(&p);
    let b = f.post(&b);
    let p = f.payment(&b, "0.50");
    let pay = f.pay(&p);
    for src in [s(&b, "identity_id"), s(&pay, "identity_id")] {
        let before = f.counts();
        let (actor, co) = (f.actor, f.co);
        let r = f
            .book
            .write(|tx| documents::reverse_source(tx, actor, co, src, "2026-10-24", "Bypass"));
        let e = r.unwrap_err();
        assert_ne!(e.constraint(), Some("fb_valid"), "masked by fb_valid: {e}");
        assert_eq!(e.constraint(), Some(SOURCE_REVERSAL_GUARD), "{e}");
        assert_eq!(before, f.counts());
    }
}

#[test]
fn every_guard_message_has_an_executed_attack() {
    GUARDS.with(|g| g.borrow_mut().clear());
    root_revision_and_line_guards();
    payment_guards_and_exact_multiset();
    protected_parents_all_unique_keys_and_source_lines();
    super::rebase_tests::new_draft_lines_require_eligible_accounts_before_they_can_be_sealed();
    let mut f = Fx::new();
    let p = f.payload();
    let b = f.save(&p);
    let b = f.post(&b);
    let p = f.payment(&b, "1.01");
    let pay = f.pay(&p);
    let (actor, co, bank, ap) = (f.actor, f.co, f.bank, f.ap);
    let raw = bill(f.book.connection(), co, s(&b, "id")).unwrap();
    refuse(&mut f, "Check number already used on this bank", |tx| {
        let mut p = raw_payment_data(tx, actor, co, &raw, bank, ap, false, false)?;
        p["method"] = json!("CHECK");
        p["reference"] = json!("100");
        insert(tx, "payables_billpayment", &p)
    });
    reverse_payment(
        &mut f.book,
        actor,
        co,
        s(&pay, "id"),
        Some(1),
        &json!("2026-10-24"),
        &json!("Correction"),
    )
    .unwrap();
    let mut hit = BTreeSet::new();
    for table in [
        "payables_bill",
        "payables_billrevision",
        "payables_billline",
        "payables_billpayment",
        "payables_paymentallocation",
        "payables_paymentreversal",
    ] {
        for statement in [
            format!("INSERT OR REPLACE INTO {table} SELECT * FROM {table} LIMIT 1"),
            format!("DELETE FROM {table}"),
            format!("UPDATE {table} SET id='{FRESH_KEY}'"),
        ] {
            let guard = sql_refused(f.book.connection().execute(&statement, []));
            hit.insert(format!("{statement}: {guard}"));
        }
    }
    for table in [
        "directory_vendor",
        "ledger_account",
        "documents_documentidentity",
        "ledger_journal",
    ] {
        let guard = sql_refused(
            f.book
                .connection()
                .execute(&format!("DELETE FROM {table}"), []),
        );
        hit.insert(format!("DELETE FROM {table}: {guard}"));
    }
    for table in ["fb_seal", "fb_seal_due"] {
        let guard = sql_refused(f.book.connection().execute(&format!("INSERT OR REPLACE INTO {table} SELECT * FROM {table} WHERE kind='bill_revision' LIMIT 1"),[]));
        hit.insert(format!("REPLACE {table}: {guard}"));
    }
    let guard = sql_refused(f.book.connection().execute(
        "INSERT OR REPLACE INTO ledger_journalline SELECT * FROM ledger_journalline LIMIT 1",
        [],
    ));
    hit.insert(format!("REPLACE ledger_journalline: {guard}"));
    let expected: BTreeSet<String> = EVERY_GUARD_RAW_SQL.iter().map(|s| s.replace("{FRESH_KEY}", FRESH_KEY)).collect();
    assert_eq!(hit, expected);
    // Evidence exists, but its journal has not posted yet: replacement guard, not late-append guard.
    refuse(&mut f, "Frozen source accounts cannot be replaced", |tx| {
        let i = documents::create_identity(tx, actor, co, "BILL", &fb_ids::new_v7().to_string())?;
        documents::snapshot_source_accounts(
            tx,
            actor,
            co,
            s(&i, "id"),
            &json!({"payables":ap.to_db()}),
        )?;
        tx.execute("INSERT OR REPLACE INTO documents_documentaccount SELECT * FROM documents_documentaccount WHERE source_id=?",[s(&i,"id")])?;
        Ok(())
    });
    // Parent becomes inactive between service validation and the actual posting UPDATE.
    let expense = f.expense;
    f.book.connection().execute_batch(&format!("CREATE TEMP TRIGGER deactivate_at_post BEFORE UPDATE ON ledger_journal WHEN NEW.status='POSTED' BEGIN UPDATE ledger_account SET active=0 WHERE id='{}'; END;",expense.to_db())).unwrap();
    refuse(
        &mut f,
        "Existing eligible accounts required at posting",
        |tx| {
            let i =
                documents::create_identity(tx, actor, co, "BILL", &fb_ids::new_v7().to_string())?;
            documents::post_source(
                tx,
                actor,
                co,
                s(&i, "id"),
                "2026-10-23",
                "Probe",
                vec![
                    json!({"account":expense.to_db(),"debit":"1","credit":"0"}),
                    json!({"account":ap.to_db(),"debit":"0","credit":"1"}),
                ],
            )?;
            Ok(())
        },
    );
    f.book
        .connection()
        .execute_batch("DROP TRIGGER deactivate_at_post")
        .unwrap();
    let mut draft = f.payload();
    draft["reference"] = json!("Calculation-only fixture");
    let draft = f.save(&draft);
    let draft = bill(f.book.connection(), co, s(&draft, "id")).unwrap();
    refuse(&mut f, "Invalid bill line calculation", |tx| {
        let r = new_revision(tx, &draft, "DRAFT")?;
        insert(tx, "payables_billrevision", &r)?;
        let mut l = draft["current"]["lines"][0].clone();
        l["revision_id"] = r["id"].clone();
        l["id"] = json!(fb_ids::new_v7().to_string());
        l["amount"] = json!(999);
        insert(tx, "payables_billline", &l)
    });
    let sql = concat!(
        include_str!("../../../fb-db/sql/0020_payables.sql"),
        include_str!("../../../fb-db/sql/0021_payables_hardening.sql"),
        include_str!("../../../fb-db/sql/0022_payables_source_hardening.sql"),
        include_str!("../../../fb-db/sql/0023_payables_parent_unique_keys.sql"),
        include_str!("../../../fb-db/sql/0024_payables_line_calculation.sql"),
        include_str!("../../../fb-db/sql/0025_payables_core_hardening.sql")
    );
    let expected = sql
        .split("RAISE(ABORT,'")
        .skip(1)
        .map(|s| s.split('\'').next().unwrap())
        .collect::<BTreeSet<_>>();
    let missing = GUARDS.with(|g| {
        expected
            .iter()
            .filter(|message| !g.borrow().iter().any(|actual| actual.contains(**message)))
            .copied()
            .collect::<Vec<_>>()
    });
    assert!(
        missing.is_empty(),
        "Guards without executed attack: {missing:?}"
    );
    println!("All {} payables guard messages exercised", expected.len());
}

#[test]
fn alternate_parent_keys_fail_before_deferred_foreign_keys() {
    let mut f = Fx::new();
    let p = f.payload();
    let b = f.save(&p);
    f.post(&b);
    for mode in ["ON", "OFF"] {
        let c = f.book.connection();
        c.execute_batch(&format!(
            "PRAGMA recursive_triggers={mode};BEGIN IMMEDIATE;PRAGMA defer_foreign_keys=ON;"
        ))
        .unwrap();
        for table in ["documents_documentidentity", "ledger_journal"] {
            let cols = c
                .prepare(&format!("SELECT * FROM {table}"))
                .unwrap()
                .column_names()
                .map(|k| {
                    if k == "id" {
                        format!("'{FRESH_KEY}'")
                    } else {
                        k.into()
                    }
                })
                .collect::<Vec<_>>()
                .join(",");
            let e = c
                .execute(
                    &format!("INSERT OR REPLACE INTO {table} SELECT {cols} FROM {table} LIMIT 1"),
                    [],
                )
                .unwrap_err();
            assert_eq!(fb_db::constraint_of(&e).as_deref(), Some("payables_parent"), "{e}");
        }
        c.execute_batch("ROLLBACK;PRAGMA recursive_triggers=ON;")
            .unwrap();
    }
}
#[test]
fn opening_book_refuses_missing_payables_guards() {
    let f = Fx::new();
    let path = std::path::PathBuf::from(f.book.connection().path().unwrap());
    f.book
        .connection()
        .execute_batch("DROP TRIGGER fb_payment_insert")
        .unwrap();
    drop(f);
    assert!(Book::open(&path, &[42; 32]).is_err());
}
