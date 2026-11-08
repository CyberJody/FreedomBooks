use fb_app::{App, ai::Permission, keys};
use fb_core::services;
use serde_json::{Value, json};
mod ai_support;
use ai_support::Fixture;
fn call(app: &mut App, token: &str, name: &str, args: Value) -> Value {
    app.ai_call(
        token,
        &json!({"operation":name,"arguments":args}).to_string(),
    )
    .unwrap()
}
fn bad(app: &mut App, token: &str, name: &str, args: Value) {
    assert!(
        app.ai_call(
            token,
            &json!({"operation":name,"arguments":args}).to_string()
        )
        .is_err(),
        "{name}"
    );
}

#[test]
fn catalogue_is_exact_scoped_and_dispatches_invoice_feed_import_and_reports() {
    let mut f = Fixture::new();
    let grant = f
        .app
        .ai_grant(f.company, "Synthetic agent", Permission::AccountingWrite)
        .unwrap();
    let token = grant["token"].as_str().unwrap();
    let catalog = f.app.ai_catalogue(token).unwrap();
    let names = catalog["operations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(names.len(), 91, "{names:?}");
    for name in [
        "command_SaveInvoice",
        "command_PostInvoice",
        "app_PendingFeed",
        "app_AcceptFeed",
        "app_ImportPreview",
        "app_ImportApply",
        "app_ReviewInvoice",
    ] {
        assert!(names.contains(&name), "{name}");
    }
    for op in catalog["operations"].as_array().unwrap() {
        assert!(
            !op["inputSchema"]["properties"]
                .as_object()
                .unwrap()
                .contains_key("company_id")
        );
        assert!(
            !op["inputSchema"]["properties"]
                .as_object()
                .unwrap()
                .contains_key("type")
        );
        assert_eq!(op["inputSchema"]["additionalProperties"], false);
    }
    assert!(!names.contains(&"command_SubmitForm") && !names.contains(&"command_ExportBackup"));
    let customer = call(
        &mut f.app,
        token,
        "command_SaveCustomer",
        json!({"display_name":"Agent customer"}),
    )["data"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let values = json!({"request_key":uuid::Uuid::new_v4().to_string(),"customer_id":customer,"date":"2026-11-01","due_date":"2026-11-30","memo":"Synthetic API invoice","tax_mode":"NONE","lines":[{"description":"Service","quantity":"3","rate":"12.34","income_account_id":f.income}]});
    let saved = call(
        &mut f.app,
        token,
        "command_SaveInvoice",
        json!({"values":values}),
    );
    let invoice = saved["data"]["id"].as_str().unwrap();
    let again = call(
        &mut f.app,
        token,
        "command_SaveInvoice",
        json!({"values":values}),
    );
    assert_eq!(again["data"]["id"], invoice);
    let inv = call(
        &mut f.app,
        token,
        "app_Invoice",
        json!({"invoice_id":invoice}),
    );
    let rev = inv["revision"].as_i64().unwrap();
    bad(
        &mut f.app,
        token,
        "command_PostInvoice",
        json!({"invoice_id":invoice,"expected_revision":rev+1}),
    );
    let review = call(
        &mut f.app,
        token,
        "app_ReviewInvoice",
        json!({"invoice_id":invoice,"expected_revision":rev}),
    );
    call(
        &mut f.app,
        token,
        "command_PostReviewedInvoice",
        json!({"invoice_id":invoice,"expected_revision":rev,"review_token":review["token"]}),
    );
    let invoices = call(&mut f.app, token, "query_Invoices", json!({}));
    assert!(invoices.to_string().contains("37.02"), "{invoices}");
    let pending = call(&mut f.app, token, "app_PendingFeed", json!({}));
    assert_eq!(pending["total"], 1, "{pending}");
    let id = pending["items"][0]["id"].as_str().unwrap();
    call(
        &mut f.app,
        token,
        "app_SuggestFeed",
        json!({"transaction_id":id}),
    );
    bad(
        &mut f.app,
        token,
        "app_AcceptFeed",
        json!({"transaction_id":id,"category":{"kind":"JOURNAL","account_id":f.other_account}}),
    );
    let accepted = call(
        &mut f.app,
        token,
        "app_AcceptFeed",
        json!({"transaction_id":id,"category":{"kind":"JOURNAL","account_id":f.expense}}),
    );
    assert_eq!(accepted["status"], "ACCEPTED");
    assert_eq!(
        call(
            &mut f.app,
            token,
            "app_AcceptFeed",
            json!({"transaction_id":id,"category":{"kind":"JOURNAL","account_id":f.expense}})
        )["journal_id"],
        accepted["journal_id"]
    );
    let before = f.count("ledger_journal");
    let journal = f.journal("10.01");
    let preview = call(
        &mut f.app,
        token,
        "app_ImportPreview",
        json!({"journals":[journal]}),
    );
    assert_eq!(f.count("ledger_journal"), before, "preview rolls back");
    let applied = call(
        &mut f.app,
        token,
        "app_ImportApply",
        json!({"preview_token":preview["preview_token"]}),
    );
    assert_eq!(f.count("ledger_journal"), before + 1);
    assert_eq!(
        applied,
        call(
            &mut f.app,
            token,
            "app_ImportApply",
            json!({"preview_token":preview["preview_token"]})
        )
    );
    let balance = call(
        &mut f.app,
        token,
        "app_TrialBalance",
        json!({"as_of":"2026-12-31"}),
    );
    assert_eq!(balance["debit"], balance["credit"]);
    call(
        &mut f.app,
        token,
        "app_ProfitAndLoss",
        json!({"start":null,"end":"2026-12-31","basis":"ACCRUAL"}),
    );
    call(
        &mut f.app,
        token,
        "app_BalanceSheet",
        json!({"as_of":"2026-12-31","basis":"ACCRUAL"}),
    );
    let second = f
        .app
        .ai_grant(f.company, "Another writer", Permission::AccountingWrite)
        .unwrap();
    bad(
        &mut f.app,
        second["token"].as_str().unwrap(),
        "app_ImportApply",
        json!({"preview_token":preview["preview_token"]}),
    );
    let actor = f.app.open_mut().unwrap().owner.to_db();
    f.app.open_mut().unwrap().book.read(|q| {
        let count:i64=q.query_row("SELECT COUNT(*) FROM ledger_auditevent p JOIN ledger_auditevent e ON p.target='audit:'||e.id WHERE p.action='agent.provenance' AND p.actor_id=?1 AND e.actor_id=p.actor_id AND json_extract(p.detail,'$.connection_id')=?2 AND json_extract(p.detail,'$.agent_name')='Synthetic agent'",[actor,grant["id"].as_str().unwrap().to_owned()],|r|r.get(0))?;
        assert!(count>=8,"{count}");
        let leaks:i64=q.query_row("SELECT COUNT(*) FROM ledger_auditevent WHERE instr(detail,?1)>0",[token],|r|r.get(0))?;
        assert_eq!(leaks,0);
        Ok::<_,fb_db::rusqlite::Error>(())
    }).unwrap();
    let conn = f.app.open_mut().unwrap().book.connection();
    assert!(
        conn.execute(
            "DELETE FROM ledger_auditevent WHERE action='agent.provenance'",
            []
        )
        .is_err()
    );
    assert!(
        conn.execute(
            "UPDATE ledger_auditevent SET actor_name='forged' WHERE action='agent.provenance'",
            []
        )
        .is_err()
    );
}

#[test]
fn permission_scope_duplicate_keys_actor_switch_lock_and_revocation_fail_closed() {
    let mut f = Fixture::new();
    let g = f
        .app
        .ai_grant(f.company, "Read agent", Permission::ReadOnly)
        .unwrap();
    let t = g["token"].as_str().unwrap();
    assert!(
        !f.app.ai_catalogue(t).unwrap()["operations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["name"].as_str().unwrap().starts_with("command_"))
    );
    call(&mut f.app, t, "query_Accounts", json!({}));
    bad(
        &mut f.app,
        t,
        "command_SaveCustomer",
        json!({"display_name":"No"}),
    );
    bad(
        &mut f.app,
        t,
        "app_ImportApply",
        json!({"preview_token":"No"}),
    );
    for args in [
        json!({"company_id":f.other.to_db()}),
        json!({"actor_id":f.outsider.to_db()}),
        json!({"type":"Companies"}),
        json!({"raw_sql":"select 1"}),
    ] {
        bad(&mut f.app, t, "query_Accounts", args);
    }
    assert!(
        f.app
            .ai_call(
                t,
                r#"{"operation":"query_Accounts","arguments":{},"arguments":{}}"#
            )
            .is_err()
    );
    assert!(f.app.ai_catalogue(&"f".repeat(64)).is_err());
    let owner = f.app.open_mut().unwrap().owner;
    f.app.open_mut().unwrap().owner = f.outsider;
    assert!(f.app.ai_catalogue(t).is_err());
    f.app.open.as_mut().unwrap().owner = owner;
    f.app.ai_revoke(g["id"].as_str().unwrap());
    assert!(f.app.ai_catalogue(t).is_err());
    let g = f
        .app
        .ai_grant(f.company, "Write agent", Permission::AccountingWrite)
        .unwrap();
    let t = g["token"].as_str().unwrap();
    f.app.ai_select_company(f.other).unwrap();
    assert!(f.app.ai_catalogue(t).is_err());
    let g = f
        .app
        .ai_grant(f.company, "Write agent", Permission::AccountingWrite)
        .unwrap();
    f.app.lock();
    f.app
        .open_book(&f.path, &keys::MasterKey::new([42; 32]))
        .unwrap();
    assert!(f.app.ai_catalogue(g["token"].as_str().unwrap()).is_err());
}

#[test]
fn named_sessions_revoke_ai_even_when_the_same_user_returns() {
    use fb_app::identity::Role;
    let mut f = Fixture::new();
    f.app.save_desktop_user(None,"Synthetic bookkeeper",Role::Bookkeeper,true,Some(ai_support::PASSWORD),&[f.company]).unwrap();
    f.app.sign_in("Synthetic operator",ai_support::PASSWORD).unwrap();
    for through_another_user in [false,true] {
        let grant=f.app.ai_grant(f.company,"Session agent",Permission::AccountingWrite).unwrap();
        let token=grant["token"].as_str().unwrap();
        f.app.sign_out();
        assert!(f.app.ai_catalogue(token).is_err());
        if through_another_user {
            f.app.sign_in("Synthetic bookkeeper",ai_support::PASSWORD).unwrap();
            assert!(f.app.ai_catalogue(token).is_err());
            f.app.sign_out();
        }
        f.app.sign_in("Synthetic operator",ai_support::PASSWORD).unwrap();
        assert!(f.app.ai_catalogue(token).is_err(),"old token revived after returning to the same user");
    }
}

#[test]
fn named_permissions_limit_grants_and_live_adapter_writes() {
    use fb_app::identity::Role;
    let mut f=Fixture::new();
    let user=f.app.save_desktop_user(None,"Synthetic limited user",Role::Readonly,true,Some(ai_support::PASSWORD),&[f.company]).unwrap();
    f.app.sign_in("Synthetic limited user",ai_support::PASSWORD).unwrap();
    assert!(f.app.ai_grant(f.company,"Forbidden writer",Permission::AccountingWrite).is_err());
    assert!(f.app.ai_grant(f.other,"Foreign reader",Permission::ReadOnly).is_err());
    let reader=f.app.ai_grant(f.company,"Reader",Permission::ReadOnly).unwrap();
    call(&mut f.app,reader["token"].as_str().unwrap(),"query_Accounts",json!({}));
    f.app.sign_out();f.app.sign_in("Synthetic operator",ai_support::PASSWORD).unwrap();
    f.app.save_desktop_user(Some(user),"Synthetic limited user",Role::Bookkeeper,true,None,&[f.company]).unwrap();
    f.app.sign_in("Synthetic limited user",ai_support::PASSWORD).unwrap();
    let grant=f.app.ai_grant(f.company,"Bookkeeper agent",Permission::AccountingWrite).unwrap();
    let token=grant["token"].as_str().unwrap();
    let pending=call(&mut f.app,token,"app_PendingFeed",json!({}));
    let id=pending["items"][0]["id"].as_str().unwrap();
    // Simulate a live role change without clearing the grant to prove dispatch also checks
    // current human authority, rather than trusting the original connection permission.
    f.app.open_mut().unwrap().book.write(|tx|tx.execute("UPDATE desktop_user SET role='readonly' WHERE user_id=?1",[user.to_db()])).unwrap();
    bad(&mut f.app,token,"app_AcceptFeed",json!({"transaction_id":id,"category":{"kind":"JOURNAL","account_id":f.expense}}));
    assert_eq!(f.count("ledger_journal"),0);
    f.app.open_mut().unwrap().book.write(|tx|tx.execute("UPDATE desktop_user SET role='bookkeeper' WHERE user_id=?1",[user.to_db()])).unwrap();
    call(&mut f.app,token,"app_AcceptFeed",json!({"transaction_id":id,"category":{"kind":"JOURNAL","account_id":f.expense}}));
    let actor=user.to_db();
    f.app.open_mut().unwrap().book.read(|tx|{
        let count:i64=tx.query_row("SELECT count(*) FROM ledger_auditevent WHERE action='agent.provenance' AND actor_id=?1 AND actor_name='Synthetic limited user'",[&actor],|r|r.get(0))?;
        assert!(count>0);
        Ok::<_,fb_db::rusqlite::Error>(())
    }).unwrap();
}

#[test]
fn import_batch_and_provenance_failure_roll_back_and_apply_revalidates() {
    let mut f = Fixture::new();
    let g = f
        .app
        .ai_grant(f.company, "Atomic agent", Permission::AccountingWrite)
        .unwrap();
    let t = g["token"].as_str().unwrap();
    let mut invalid = f.journal("9.99");
    invalid["lines"][1]["credit"] = json!("9.98");
    let before = f.count("ledger_journal");
    let audits = f.count("ledger_auditevent");
    let journal = f.journal("10.00");
    bad(
        &mut f.app,
        t,
        "app_ImportPreview",
        json!({"journals":[journal,invalid]}),
    );
    assert_eq!(f.count("ledger_journal"), before);
    assert_eq!(f.count("ledger_auditevent"), audits);
    let journal = f.journal("20.00");
    let p = call(
        &mut f.app,
        t,
        "app_ImportPreview",
        json!({"journals":[journal]}),
    );
    let audits = f.count("ledger_auditevent");
    f.app.open_mut().unwrap().book.connection().execute_batch("CREATE TRIGGER ai_fail_provenance BEFORE INSERT ON ledger_auditevent WHEN NEW.action='agent.provenance' BEGIN SELECT RAISE(ABORT,'synthetic_provenance_failure'); END;").unwrap();
    bad(
        &mut f.app,
        t,
        "app_ImportApply",
        json!({"preview_token":p["preview_token"]}),
    );
    assert_eq!(f.count("ledger_journal"), before);
    assert_eq!(
        f.count("ledger_auditevent"),
        audits,
        "provenance failure rolls back original audit and writes"
    );
    f.app
        .open_mut()
        .unwrap()
        .book
        .connection()
        .execute_batch("DROP TRIGGER ai_fail_provenance")
        .unwrap();
    // Inactivate after preview: apply must repeat eligibility checks and leave no partial batch.
    let o = f.app.open_mut().unwrap();
    services::save_account(
        &mut o.book,
        o.owner,
        f.company,
        services::AccountInput::new("500", "Office")
            .detail("EXPENSE")
            .active(false)
            .edit(f.expense.parse().unwrap()),
    )
    .unwrap();
    bad(
        &mut f.app,
        t,
        "app_ImportApply",
        json!({"preview_token":p["preview_token"]}),
    );
    assert_eq!(f.count("ledger_journal"), before);
}

#[test]
fn import_never_posts_a_different_current_draft_resolved_by_a_creation_key() {
    let mut f = Fixture::new();
    let g = f
        .app
        .ai_grant(f.company, "Import reviewer", Permission::AccountingWrite)
        .unwrap();
    let token = g["token"].as_str().unwrap();
    let journal = f.journal("12.00");
    let p = call(
        &mut f.app,
        token,
        "app_ImportPreview",
        json!({"journals":[journal]}),
    );
    let input = services::DraftInput {
        date: "2026-11-02".into(),
        memo: "Synthetic import".into(),
        request_key: Some(journal["request_key"].as_str().unwrap().into()),
        lines: journal["lines"].as_array().unwrap().clone(),
        ..Default::default()
    };
    let o = f.app.open_mut().unwrap();
    let draft = services::save_draft(&mut o.book, o.owner, f.company, input.clone()).unwrap();
    let mut changed = input;
    changed.journal_id = Some(draft.id);
    changed.expected_revision = Some(draft.revision);
    changed.lines[0]["debit"] = json!("99.00");
    changed.lines[1]["credit"] = json!("99.00");
    services::save_draft(&mut o.book, o.owner, f.company, changed).unwrap();
    bad(
        &mut f.app,
        token,
        "app_ImportApply",
        json!({"preview_token":p["preview_token"]}),
    );
    assert_eq!(f.count("ledger_journal"), 1);
    let current = call(
        &mut f.app,
        token,
        "app_Journal",
        json!({"journal_id":draft.id.to_db()}),
    );
    assert_eq!(current["lines"][0]["debit"], "99.00");
    let o = f.app.open_mut().unwrap();
    assert_eq!(
        o.book
            .read(|q| fb_core::store::journal_in(q, draft.id, f.company))
            .unwrap()
            .status
            .as_str(),
        "DRAFT"
    );
}

#[test]
fn import_uuid_aliases_are_rejected_without_journals_or_audit() {
    let mut f = Fixture::new();
    let mut journal = f.journal("10.01");
    let key = "211ce6fc-4559-46d4-91ea-c81b0b41ff93";
    journal["request_key"] = json!(key);
    let before = f.count("ledger_journal");
    let audits = f.count("ledger_auditevent");
    for alias in [
        key.to_owned(),
        key.to_uppercase(),
        key.replace('-', ""),
        format!("{{{key}}}"),
        format!("urn:uuid:{key}"),
    ] {
        let mut duplicate = journal.clone();
        duplicate["request_key"] = json!(alias);
        let journals: Vec<fb_app::ai::import::Journal> =
            serde_json::from_value(json!([journal, duplicate])).unwrap();
        // The shared service must refuse aliases in both preview and apply, before any Writer.
        for preview in [true, false] {
            let open = f.app.open_mut().unwrap();
            let result = fb_app::ai::import::run(
                &mut open.book,
                open.owner,
                f.company,
                &journals,
                preview,
            );
            assert!(
                result.is_err(),
                "alias {alias}, preview {preview}: {result:?}"
            );
            assert_eq!(f.count("ledger_journal"), before);
            assert_eq!(f.count("ledger_auditevent"), audits);
        }
    }
}

#[test]
fn import_distinct_keys_post_distinct_journals_and_retry_once() {
    let mut f = Fixture::new();
    let grant = f
        .app
        .ai_grant(f.company, "Batch importer", Permission::AccountingWrite)
        .unwrap();
    let token = grant["token"].as_str().unwrap();
    let mut first = f.journal("10.01");
    first["request_key"] = json!("{211CE6FC-4559-46D4-91EA-C81B0B41FF93}");
    let second = f.journal("10.01");
    let before = f.count("ledger_journal");
    let audits = f.count("ledger_auditevent");
    let preview = call(
        &mut f.app,
        token,
        "app_ImportPreview",
        json!({"journals":[first,second]}),
    );
    assert_eq!(preview["count"], 2);
    assert_ne!(preview["journals"][0]["id"], preview["journals"][1]["id"]);
    assert_eq!(f.count("ledger_journal"), before);
    assert_eq!(f.count("ledger_auditevent"), audits);
    let applied = call(
        &mut f.app,
        token,
        "app_ImportApply",
        json!({"preview_token":preview["preview_token"]}),
    );
    assert_eq!(applied["count"], 2);
    assert_ne!(applied["journals"][0]["id"], applied["journals"][1]["id"]);
    assert!(
        applied["journals"]
            .as_array()
            .unwrap()
            .iter()
            .all(|j| j["status"] == "POSTED")
    );
    assert_eq!(f.count("ledger_journal"), before + 2);
    let posted_audits = f.count("ledger_auditevent");
    assert_eq!(
        applied,
        call(
            &mut f.app,
            token,
            "app_ImportApply",
            json!({"preview_token":preview["preview_token"]}),
        ),
    );
    assert_eq!(f.count("ledger_journal"), before + 2);
    assert_eq!(f.count("ledger_auditevent"), posted_audits);
}

#[test]
fn feed_rows_still_pending_at_the_bank_cannot_post() {
    let mut f = Fixture::new();
    let open = f.app.open_mut().unwrap();
    let connection: String = open
        .book
        .read(|q| {
            q.query_row(
                "SELECT connection_id FROM bankfeeds_account LIMIT 1",
                [],
                |r| r.get(0),
            )
        })
        .unwrap();
    let pending = fb_core::bankfeeds::StagedTransaction {
        remote_id: "synthetic-bank-pending".into(),
        posted_on: Some("2026-11-02".into()),
        transacted_on: None,
        amount: -555,
        description: "Still pending at bank".into(),
        pending: true,
    };
    let staged = fb_core::bankfeeds::stage_bank_transactions(
        &mut open.book,
        open.owner,
        f.company,
        &connection,
        &[("synthetic-checking".into(), pending)],
        &fb_core::bankfeeds::SecretSet::none(),
    )
    .unwrap();
    let g = f
        .app
        .ai_grant(f.company, "Pending reviewer", Permission::AccountingWrite)
        .unwrap();
    let token = g["token"].as_str().unwrap();
    let page = call(&mut f.app, token, "app_PendingFeed", json!({}));
    assert_eq!(page["total"], 2);
    assert!(
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["id"] == staged[0] && v["pending"] == true)
    );
    bad(
        &mut f.app,
        token,
        "app_AcceptFeed",
        json!({"transaction_id":staged[0],"category":{"kind":"JOURNAL","account_id":f.expense}}),
    );
    assert_eq!(f.count("ledger_journal"), 0);
}
