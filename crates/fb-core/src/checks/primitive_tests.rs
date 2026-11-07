//! Direct DML regressions for each lifecycle boundary, with complete book snapshots.
use super::guards::snapshot;
use super::*;
// Absent-parent ids: canonical UUIDs that exist nowhere, so 0130 passes and the parent guard decides.
const MISSING_BANK: &str = "00000000-0000-7000-8000-0000000ab001"; // future bank account
const MISSING_VENDOR: &str = "00000000-0000-7000-8000-0000000ab002"; // future vendor
const MISSING_JOURNAL: &str = "00000000-0000-7000-8000-0000000ab003"; // absent posting journal
const MISSING_ACTOR: &str = "00000000-0000-7000-8000-0000000ab004"; // absent revision actor
const MISSING_REVISION: &str = "00000000-0000-7000-8000-0000000ab005"; // future line parent revision
const MISSING_TRANSFER_ACTOR: &str = "00000000-0000-7000-8000-0000000ab006"; // absent transfer actor
thread_local! {static OBSERVED:std::cell::RefCell<BTreeSet<String>>=std::cell::RefCell::default();}
fn observe(text: String) {
    OBSERVED.with(|o| {
        o.borrow_mut().insert(text);
    });
}
/// Records the refusal and returns the guard that raised it; 0130's `fb_valid` never counts.
pub(super) fn sql_refused<T: std::fmt::Debug>(r: Result<T, fb_db::rusqlite::Error>) -> String {
    let e = r.unwrap_err();
    let constraint = fb_db::constraint_of(&e).unwrap_or_default();
    assert_ne!(constraint, "fb_valid", "masked by fb_valid: {e}");
    observe(e.to_string());
    constraint
}
/// Asserts the collected `<label>: <guard>` set equals the explicit expected set; on mismatch it prints
/// the actual set as Rust string literals.
pub(super) fn assert_guard_set(hit: &BTreeSet<String>, expected: &[&str]) {
    let expected: BTreeSet<String> = expected.iter().map(|s| s.to_string()).collect();
    assert!(
        *hit == expected,
        "guard set differs\nACTUAL:\n{}",
        hit.iter().map(|h| format!("    \"{h}\",")).collect::<Vec<_>>().join("\n")
    );
}

/// Like `sql_refused`, but the refusal must come from the named guard, never from 0130's `fb_valid`.
fn sql_refused_by<T: std::fmt::Debug>(r: Result<T, fb_db::rusqlite::Error>, want: &str) {
    let e = r.unwrap_err();
    let constraint = fb_db::constraint_of(&e);
    assert_ne!(constraint.as_deref(), Some("fb_valid"), "masked by fb_valid: {e}");
    assert_eq!(constraint.as_deref(), Some(want), "{e}");
    observe(e.to_string());
}
pub(super) fn refuse(f: &mut Fx, want: &str, attack: impl FnOnce(&Writer<'_>) -> FbResult<()>) {
    let before = snapshot(f.book.connection());
    let e = f.book.write(attack).unwrap_err();
    observe(format!("{}: {}", e.constraint().unwrap_or(""), e.message()));
    assert_ne!(e.constraint(), Some("fb_valid"), "masked by fb_valid: {e}");
    assert!(e.message().contains(want), "wanted {want}; got {e}");
    assert_eq!(before, snapshot(f.book.connection()));
}
#[test]
fn new_draft_directory_and_bank_eligibility_are_guarded() {
    let mut f = Fx::new();
    let p = f.payload();
    let b = f.save(&p);
    let b = check(f.book.connection(), f.co, s(&b, "id")).unwrap();
    let bank = f.bank;
    refuse(&mut f, "Choose an active designated bank", |tx| {
        tx.execute(
            "UPDATE ledger_account SET active=0 WHERE id=?",
            [bank.to_db()],
        )?;
        let r = revision(tx, &b, "DRAFT")?;
        insert(tx, "checks_checkrevision", &r)
    });
    let vendor = directory::save_record(
        &mut f.book,
        f.actor,
        f.co,
        Model::Vendor,
        &json!({"display_name":"Archived"}),
        None,
    )
    .unwrap();
    directory::set_active(
        &mut f.book,
        f.actor,
        f.co,
        Model::Vendor,
        s(&vendor, "id"),
        false,
    )
    .unwrap();
    refuse(&mut f, "New draft vendors must be active", |tx| {
        tx.execute(
            "UPDATE checks_check SET revision=revision+1,vendor_id=? WHERE id=?",
            params![s(&vendor, "id"), s(&b, "id")],
        )?;
        let mut r = b["current"].clone();
        r.as_object_mut().unwrap().remove("lines");
        r["id"] = json!(fb_ids::new_v7().to_string());
        r["number"] = json!(2);
        r["action"] = json!("revised");
        r["vendor_id"] = vendor["id"].clone();
        r["payee_snapshot"] = party(&vendor);
        insert(tx, "checks_checkrevision", &r)
    });
    let item = directory::save_record(
        &mut f.book,
        f.actor,
        f.co,
        Model::Item,
        &json!({"code":"Archived item","kind":"NONSTOCK","sale_rate":"1"}),
        None,
    )
    .unwrap();
    directory::set_active(
        &mut f.book,
        f.actor,
        f.co,
        Model::Item,
        s(&item, "id"),
        false,
    )
    .unwrap();
    refuse(&mut f, "New draft items must be active", |tx| {
        let r = revision(tx, &b, "DRAFT")?;
        insert(tx, "checks_checkrevision", &r)?;
        let mut l = b["current"]["lines"][0].clone();
        l["id"] = json!(fb_ids::new_v7().to_string());
        l["revision_id"] = r["id"].clone();
        l["tab"] = json!("ITEM");
        l["item_id"] = item["id"].clone();
        l["item_snapshot"] = json!({"id":item["id"],"code":item["code"],"description":item["description"],"kind":item["kind"],"purchase_cost":item["purchase_cost"],"expense_account_id":item["expense_account"]});
        insert(tx, "checks_checkline", &l)
    });
    let (actor, co, bank2) = (f.actor, f.co, f.bank2);
    refuse(&mut f, "Transfers cannot use a control account", |tx| {
        let i = documents::create_identity(tx, actor, co, "EST", &fb_ids::new_v7().to_string())?;
        documents::snapshot_source_accounts(
            tx,
            actor,
            co,
            s(&i, "id"),
            &json!({"undeposited":bank2.to_db()}),
        )?;
        let t = transfer_row(tx, actor, co, bank, bank2, false)?;
        insert(tx, "transfers_transfer", &t)
    });
}
#[test]
fn number_collision_guards_connect_both_directions_to_payables() {
    use crate::payables;
    let mut f = Fx::new();
    let (actor, co) = (f.actor, f.co);
    let mut cp = f.payload();
    cp["check_number"] = json!("50");
    let check = f.save(&cp);
    let bp = json!({"vendor_id":f.vendor,"date":"2026-10-23","due_date":"2026-11-23","request_key":fb_ids::new_v7().to_string(),"lines":[{"description":"Payable","quantity":"1","rate":"100","expense_account_id":f.expense.to_db()}]});
    let b = payables::save_bill(&mut f.book, actor, co, &bp).unwrap();
    let r = payables::review_bill(&mut f.book, actor, co, s(&b, "id"), Some(1)).unwrap();
    payables::post_bill(&mut f.book, actor, co, s(&b, "id"), Some(1), &r).unwrap();
    let p = json!({"vendor_id":f.vendor,"bank_id":f.bank.to_db(),"date":"2026-10-23","amount":"10","method":"CHECK","reference":"101","request_key":fb_ids::new_v7().to_string(),"allocations":[{"bill_id":b["id"],"amount":"10"}]});
    let r = payables::review_payment(&mut f.book, actor, co, &p).unwrap();
    let payment = payables::record_payment(&mut f.book, actor, co, &p, &r).unwrap();
    let before_retry = snapshot(f.book.connection());
    let mut alias = p.clone();
    alias["request_key"] = json!(s(&p, "request_key").to_uppercase());
    assert_eq!(
        record_bill_payment(&mut f.book, actor, co, &alias, &r).unwrap()["id"],
        payment["id"]
    );
    assert_eq!(before_retry, snapshot(f.book.connection()));
    refuse(&mut f, "already used by a live bill payment", |tx| {
        let i = documents::create_identity(tx, actor, co, "CHK", &fb_ids::new_v7().to_string())?;
        let mut root = check.clone();
        for k in ["current", "number"] {
            root.as_object_mut().unwrap().remove(k);
        }
        root["id"] = json!(fb_ids::new_v7().to_string());
        root["identity_id"] = i["id"].clone();
        root["number_key"] = json!("101");
        insert(tx, "checks_check", &root)
    });
    payables::reverse_payment(
        &mut f.book,
        actor,
        co,
        s(&payment, "id"),
        Some(1),
        &json!("2026-10-24"),
        &json!("Release"),
    )
    .unwrap();
    let mut p = p;
    p["request_key"] = json!(fb_ids::new_v7().to_string());
    let review = payables::review_payment(&mut f.book, actor, co, &p).unwrap();
    let cp = f.payload();
    f.save(&cp);
    let before = snapshot(f.book.connection());
    let e = payables::record_payment(&mut f.book, actor, co, &p, &review).unwrap_err();
    assert_eq!(e.kind(), "ValidationError");
    assert!(e.message().contains("already used by check"));
    let raw=f.book.connection().execute("INSERT INTO payables_billpayment SELECT * FROM payables_billpayment WHERE id=?",[s(&payment,"id")]).unwrap_err();
    observe(raw.to_string());
    assert_eq!(fb_db::constraint_of(&raw).as_deref(),Some("check_payment_number"));
    assert_eq!(before, snapshot(f.book.connection()));
}
#[test]
fn root_updates_require_existing_same_company_parents_immediately() {
    let mut f = Fx::new();
    let p = f.payload();
    let b = f.save(&p);
    let other = services::create_company(
        &mut f.book,
        f.actor,
        "Other synthetic company",
        "USD",
        false,
    )
    .unwrap()
    .id;
    let other_bank = services::save_account(
        &mut f.book,
        f.actor,
        other,
        AccountInput::new("100", "Other bank").kind("ASSET"),
    )
    .unwrap()
    .id
    .to_db();
    let other_vendor = directory::save_record(
        &mut f.book,
        f.actor,
        other,
        Model::Vendor,
        &json!({"display_name":"Other supplier"}),
        None,
    )
    .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let c = f.book.connection();
    let before = snapshot(c);
    for recursion in [0, 1] {
        c.execute_batch(&format!("PRAGMA recursive_triggers={recursion}"))
            .unwrap();
        for (column, value) in [
            ("bank_id", MISSING_BANK),
            ("vendor_id", MISSING_VENDOR),
            ("bank_id", other_bank.as_str()),
            ("vendor_id", other_vendor.as_str()),
        ] {
            c.execute_batch("BEGIN IMMEDIATE; PRAGMA defer_foreign_keys=ON")
                .unwrap();
            let result = c.execute(
                &format!("UPDATE checks_check SET revision=revision+1,{column}=? WHERE id=?"),
                params![value, s(&b, "id")],
            );
            c.execute_batch("ROLLBACK").unwrap();
            let error = result.expect_err(
                "root update accepted a missing/cross-company parent before the next revision",
            );
            assert_eq!(
                fb_db::constraint_of(&error).as_deref(),
                Some("checks_existing_parent"),
                "{error}"
            );
            assert_eq!(before, snapshot(c));
        }
    }
    c.execute_batch("PRAGMA recursive_triggers=ON").unwrap();
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn every_account_detail_type_matches_service_and_raw_line_eligibility() {
    let mut f = Fx::new();
    let base = f.payload();
    let b = f.save(&base);
    let raw = check(f.book.connection(), f.co, s(&b, "id")).unwrap();
    for (index, detail) in crate::model::DetailType::ALL.into_iter().enumerate() {
        let a = services::save_account(
            &mut f.book,
            f.actor,
            f.co,
            AccountInput::new(&format!("type-{index}"), "Synthetic detail").detail(detail.as_str()),
        )
        .unwrap();
        let expected = directory::purchase_line_allowed(&a);
        let mut p = f.payload();
        p["check_number"] = json!(format!("type-{index}"));
        p["lines"][0]["account_id"] = json!(a.id.to_db());
        let before = snapshot(f.book.connection());
        let result = save_check(&mut f.book, f.actor, f.co, &p);
        assert_eq!(result.is_ok(), expected, "service {detail:?}");
        if !expected {
            assert_eq!(snapshot(f.book.connection()), before);
        }
        let before = snapshot(f.book.connection());
        f.book
            .write(|tx| -> FbResult<()> {
                let r = revision(tx, &raw, "DRAFT")?;
                insert(tx, "checks_checkrevision", &r)?;
                let mut line = raw["current"]["lines"][0].clone();
                line["id"] = json!(fb_ids::new_v7().to_string());
                line["revision_id"] = r["id"].clone();
                line["account_id"] = json!(a.id.to_db());
                line["account_snapshot"] = asnap(&a);
                let result = insert(tx, "checks_checkline", &line);
                assert_eq!(result.is_ok(), expected, "SQL {detail:?}: {result:?}");
                if let Err(e) = &result {
                    assert_eq!(e.message(), "Invalid draft purchase account", "SQL {detail:?}: {e}");
                }
                Err(err("rollback eligibility fixture"))
            })
            .unwrap_err();
        assert_eq!(snapshot(f.book.connection()), before);
    }
    assert!(f.book.check().unwrap().is_empty());
}
#[test]
fn every_checks_guard_message_has_an_executed_refusal() {
    OBSERVED.with(|o| o.borrow_mut().clear());
    roots_revisions_lines_and_posting_guards_refuse_inconsistent_evidence();
    transfer_split_scope_accounts_void_and_owner_have_independent_backstops();
    new_draft_directory_and_bank_eligibility_are_guarded();
    number_collision_guards_connect_both_directions_to_payables();
    super::guards::all_unique_keys_refuse_replacement_and_delete_with_recursion_on_or_off();
    super::guards::editable_check_cannot_displace_live_reference_victim();
    super::guards::primary_keys_reject_fresh_rekeys();
    let mut f = Fx::new();
    let p = f.payload();
    f.save(&p);
    for sql in [
        "DELETE FROM directory_vendor",
        "INSERT OR REPLACE INTO directory_vendor SELECT * FROM directory_vendor",
    ] {
        sql_refused_by(f.book.connection().execute_batch(sql), "checks_parent_retained");
    }
    super::fix1::new_guards_have_independent_refusals();
    let mut messages = BTreeSet::new();
    for (_, sql) in fb_db::MIGRATIONS
        .iter()
        .filter(|(n, _)| n.starts_with("003"))
    {
        for fragment in sql.split("RAISE(ABORT,'").skip(1) {
            messages.insert(fragment.split('\'').next().unwrap().to_owned());
        }
    }
    let missing = OBSERVED.with(|o| {
        messages
            .iter()
            .filter(|m| !o.borrow().iter().any(|text| text.contains(m.as_str())))
            .cloned()
            .collect::<Vec<_>>()
    });
    assert!(
        missing.is_empty(),
        "Unexercised guard messages: {missing:#?}"
    );
    println!(
        "Observed all {} checks/transfers guard messages",
        messages.len()
    );
}
pub(super) fn revision(tx: &Writer<'_>, b: &Value, state: &str) -> FbResult<Value> {
    let n = b["revision"].as_i64().unwrap() + 1;
    tx.execute(
        "UPDATE checks_check SET revision=?,state=? WHERE id=?",
        params![n, state, s(b, "id")],
    )?;
    let mut r = b["current"].clone();
    r.as_object_mut().unwrap().remove("lines");
    r["id"] = json!(fb_ids::new_v7().to_string());
    r["number"] = json!(n);
    r["state"] = json!(state);
    r["action"] = json!(if state == "DRAFT" {
        "revised"
    } else if state == "VOID" {
        "voided"
    } else {
        "recorded"
    });
    Ok(r)
}
fn posted_revision(
    tx: &Writer<'_>,
    actor: UserId,
    co: CompanyId,
    b: &Value,
    duplicate: bool,
) -> FbResult<Value> {
    let (map, mut entries, review) = super::super::lifecycle::posting(tx, actor, co, b, true)?;
    documents::snapshot_source_accounts(tx, actor, co, s(b, "identity_id"), &map)?;
    if duplicate {
        entries.extend(entries.clone());
    }
    let j = documents::post_source(
        tx,
        actor,
        co,
        s(b, "identity_id"),
        s(&b["current"], "date"),
        "Synthetic guard test",
        entries,
    )?;
    let mut r = revision(tx, b, "RECORDED")?;
    r["journal_id"] = json!(j.id.to_db());
    r["review_digest"] = json!(review.digest);
    Ok(r)
}
fn lines_and_seal(tx: &Writer<'_>, r: &Value, b: &Value, changed: bool) -> FbResult<()> {
    insert(tx, "checks_checkrevision", r)?;
    for mut l in b["current"]["lines"].as_array().unwrap().clone() {
        l["id"] = json!(fb_ids::new_v7().to_string());
        l["revision_id"] = r["id"].clone();
        if changed {
            l["description"] = json!("Changed evidence");
        }
        insert(tx, "checks_checkline", &l)?;
    }
    tx.execute(
        "INSERT INTO fb_seal VALUES('check_revision',?)",
        [format!("{}:{}", s(b, "id"), r["number"])],
    )?;
    Ok(())
}
#[test]
fn roots_revisions_lines_and_posting_guards_refuse_inconsistent_evidence() {
    let mut f = Fx::new();
    let p = f.payload();
    let b = f.save(&p);
    let b = check(f.book.connection(), f.co, s(&b, "id")).unwrap();
    let (actor, co) = (f.actor, f.co);
    refuse(&mut f, "Check ownership is immutable", |tx| {
        tx.execute(
            "UPDATE checks_check SET revision=revision+2 WHERE id=?",
            [s(&b, "id")],
        )?;
        Ok(())
    });
    refuse(&mut f, "Invalid check identity", |tx| {
        let i = documents::create_identity(tx, actor, co, "CHK", &fb_ids::new_v7().to_string())?;
        let mut root = b.clone();
        root.as_object_mut().unwrap().remove("current");
        root.as_object_mut().unwrap().remove("number");
        root["id"] = json!(fb_ids::new_v7().to_string());
        root["identity_id"] = i["id"].clone();
        root["revision"] = json!(2);
        root["number_key"] = json!("different");
        insert(tx, "checks_check", &root)
    });
    for (field, value, want) in [
        ("number", json!(3), "Invalid check revision ownership"),
        ("action", json!("voided"), "Invalid check state transition"),
        ("payee_name", json!(" "), "Invalid check payee"),
        ("check_number", json!("other"), "Check root index"),
        (
            "journal_id",
            json!(MISSING_JOURNAL),
            "Invalid check posting linkage",
        ),
        (
            "actor_id",
            json!(MISSING_ACTOR),
            "Referenced parent must already exist",
        ),
    ] {
        refuse(&mut f, want, |tx| {
            let mut r = revision(tx, &b, "DRAFT")?;
            r[field] = value;
            insert(tx, "checks_checkrevision", &r)
        });
    }
    refuse(&mut f, "committed history", |tx| {
        let mut l = b["current"]["lines"][0].clone();
        l["id"] = json!(fb_ids::new_v7().to_string());
        l["position"] = json!(2);
        insert(tx, "checks_checkline", &l)
    });
    for (field, value, want) in [
        (
            "revision_id",
            json!(MISSING_REVISION),
            "Invalid check line scope",
        ),
        ("amount", json!(1), "Invalid check line calculation"),
        (
            "account_id",
            json!(f.ap.to_db()),
            "Invalid draft purchase account",
        ),
    ] {
        refuse(&mut f, want, |tx| {
            let r = revision(tx, &b, "DRAFT")?;
            insert(tx, "checks_checkrevision", &r)?;
            let mut l = b["current"]["lines"][0].clone();
            l["id"] = json!(fb_ids::new_v7().to_string());
            l["revision_id"] = r["id"].clone();
            l[field] = value;
            if field == "account_id" {
                l["account_snapshot"] = asnap(&account(tx, co, s(&l, "account_id"))?);
            }
            insert(tx, "checks_checkline", &l)
        });
    }
    refuse(&mut f, "Incomplete or inconsistent check totals", |tx| {
        let r = revision(tx, &b, "DRAFT")?;
        insert(tx, "checks_checkrevision", &r)?;
        tx.execute(
            "INSERT INTO fb_seal VALUES('check_revision',?)",
            [format!("{}:2", s(&b, "id"))],
        )?;
        Ok(())
    });
    refuse(&mut f, "preserve financial evidence", |tx| {
        let mut r = posted_revision(tx, actor, co, &b, false)?;
        r["memo"] = json!("Tampered");
        insert(tx, "checks_checkrevision", &r)
    });
    refuse(&mut f, "copy exact lines", |tx| {
        let r = posted_revision(tx, actor, co, &b, false)?;
        lines_and_seal(tx, &r, &b, true)
    });
    refuse(&mut f, "exact bank credit", |tx| {
        let r = posted_revision(tx, actor, co, &b, true)?;
        lines_and_seal(tx, &r, &b, false)
    });
    let expense = f.expense;
    refuse(&mut f, "Active eligible posting parents", |tx| {
        let r = posted_revision(tx, actor, co, &b, false)?;
        tx.execute(
            "UPDATE ledger_account SET active=0 WHERE id=?",
            [expense.to_db()],
        )?;
        lines_and_seal(tx, &r, &b, false)
    });
    let b = f.post(&b);
    let b = check(f.book.connection(), co, s(&b, "id")).unwrap();
    refuse(&mut f, "Void requires reason", |tx| {
        let mut r = revision(tx, &b, "VOID")?;
        let j =
            documents::reverse_source(tx, actor, co, s(&b, "identity_id"), "2026-10-24", "Guard")?;
        r["reversal_id"] = json!(j.id.to_db());
        insert(tx, "checks_checkrevision", &r)
    });
    refuse(&mut f, "Cannot append posted account evidence", |tx| {
        tx.execute("INSERT INTO documents_documentaccount(id,source_id,role,account_id,account_code,account_name,account_kind) SELECT ?,source_id,'extra',account_id,account_code,account_name,account_kind FROM documents_documentaccount WHERE source_id=? LIMIT 1",params![fb_ids::new_v7().to_string(),s(&b,"identity_id")])?;
        Ok(())
    });
    refuse(&mut f, "owning check lifecycle", |tx| {
        let j =
            documents::reverse_source(tx, actor, co, s(&b, "identity_id"), "2026-10-24", "Guard")?;
        tx.execute(
            "INSERT INTO fb_seal VALUES('checks_journal',?)",
            [j.id.to_db()],
        )?;
        Ok(())
    });
}
pub(super) fn transfer_row(
    tx: &Writer<'_>,
    actor: UserId,
    co: CompanyId,
    from: AccountId,
    to: AccountId,
    extra: bool,
) -> FbResult<Value> {
    let i = documents::create_identity(tx, actor, co, "TRF", &fb_ids::new_v7().to_string())?;
    let mut mapping = json!({"from":from.to_db(),"to":to.to_db()});
    if extra {
        mapping["extra"] = json!(to.to_db());
    }
    documents::snapshot_source_accounts(tx, actor, co, s(&i, "id"), &mapping)?;
    let j = documents::post_source(
        tx,
        actor,
        co,
        s(&i, "id"),
        "2026-10-23",
        "Guard",
        vec![
            json!({"account":from.to_db(),"debit":"0","credit":"5"}),
            json!({"account":to.to_db(),"debit":"5","credit":"0"}),
        ],
    )?;
    Ok(
        json!({"id":fb_ids::new_v7().to_string(),"company_id":co.to_db(),"identity_id":i["id"],"creation_digest":"a".repeat(64),"review_digest":"b".repeat(64),"from_account_id":from.to_db(),"to_account_id":to.to_db(),"from_snapshot":asnap(&account(tx,co,&from.to_db())?),"to_snapshot":asnap(&account(tx,co,&to.to_db())?),"company_snapshot":company_snap(tx,co)?,"date":"2026-10-23","amount":500,"memo":"","journal_id":j.id.to_db(),"actor_id":actor.to_db(),"actor_name":"guard","created_at":py::now_utc()}),
    )
}
#[test]
fn transfer_split_scope_accounts_void_and_owner_have_independent_backstops() {
    let mut f = Fx::new();
    let (actor, co, bank, bank2) = (f.actor, f.co, f.bank, f.bank2);
    for (field, value, want) in [
        ("amount", json!(600), "exactly one credit"),
        ("date", json!("2026-10-24"), "Invalid transfer ownership"),
        (
            "actor_id",
            json!(MISSING_TRANSFER_ACTOR),
            "Referenced parent must already exist",
        ),
    ] {
        refuse(&mut f, want, |tx| {
            let mut t = transfer_row(tx, actor, co, bank, bank2, false)?;
            t[field] = value;
            insert(tx, "transfers_transfer", &t)
        });
    }
    refuse(&mut f, "exactly from and to", |tx| {
        let t = transfer_row(tx, actor, co, bank, bank2, true)?;
        insert(tx, "transfers_transfer", &t)
    });
    refuse(&mut f, "owning transfer lifecycle", |tx| {
        let t = transfer_row(tx, actor, co, bank, bank2, false)?;
        tx.execute(
            "INSERT INTO fb_seal VALUES('transfer_journal',?)",
            [s(&t, "journal_id")],
        )?;
        Ok(())
    });
    let tp = f.transfer();
    let r = crate::transfers::review_transfer(&mut f.book, actor, co, &tp).unwrap();
    let t = crate::transfers::record_transfer(&mut f.book, actor, co, &tp, &r).unwrap();
    refuse(&mut f, "Invalid transfer void", |tx| {
        let j =
            documents::reverse_source(tx, actor, co, s(&t, "identity_id"), "2026-10-24", "Guard")?;
        insert(
            tx,
            "transfers_transfervoid",
            &json!({"id":fb_ids::new_v7().to_string(),"transfer_id":t["id"],"journal_id":j.id.to_db(),"date":"2026-10-25","reason":"Guard","actor_id":actor.to_db(),"actor_name":"guard","created_at":py::now_utc()}),
        )
    });
    let before = snapshot(f.book.connection());
    let e = f
        .book
        .write(|tx| documents::reverse_source(
            tx,
            actor,
            co,
            s(&t, "identity_id"),
            "2026-10-24",
            "Bypass"
        ))
        .unwrap_err();
    // Pre-0130 behaviour: the deferred foreign key (not 0130) refuses the lifecycle-less reversal.
    assert_eq!(e.constraint(), Some("foreign_key"), "{e}");
    assert_eq!(before, snapshot(f.book.connection()));
}
