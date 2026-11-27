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
fn trusted_approval_context_requires_named_signin_and_does_not_create_agent_authority() {
    let mut f=Fixture::new();
    let before=f.app.ai.generation();
    let context=f.app.ai_context(f.company).unwrap();
    assert_eq!(context["company_id"],f.company.to_db());
    assert_eq!(context["company_name"],"Synthetic local AI");
    assert_eq!(context["actor_name"],"Synthetic operator");
    assert_eq!(context["role"],"administrator");
    assert_eq!(context["book_path"],f.path.canonicalize().unwrap().to_str().unwrap());
    assert_eq!(context["restored"],false);
    assert_eq!(f.app.ai.generation(),before);
    assert_eq!(f.app.ai_connections(),json!([]));
    f.app.sign_out();
    assert!(f.app.ai_context(f.company).is_err());
    f.app.sign_in("Synthetic operator",ai_support::PASSWORD).unwrap();
    let next=f.app.ai_context(f.company).unwrap();
    assert_eq!(next["actor_id"],context["actor_id"]);
    assert_ne!(next["session_generation"],context["session_generation"]);
    assert_eq!(f.app.ai_connections(),json!([]));
}

#[test]
fn directory_api_round_trips_multiline_notes_and_keeps_control_rejections_atomic() {
    let mut f = Fixture::new();
    let grant = f.app.ai_grant(f.company, "Synthetic notes agent", Permission::AccountingWrite).unwrap();
    let token = grant["token"].as_str().unwrap();
    let mut values = json!({"kind":"customer","display_name":"Synthetic multiline customer",
        "business_name":"","contact_name":"","email":"","phone":"","billing_address":"",
        "notes":"First context\n\nSecond context\r\nFinal context"});
    let saved = call(&mut f.app, token, "command_SaveDirectoryRecord", json!({"values":values}));
    let id = saved["data"]["id"].as_str().unwrap();
    let directory = call(&mut f.app, token, "app_Directory", json!({"kind":"customer"}));
    assert!(directory.to_string().contains("First context\\n\\nSecond context\\r\\nFinal context"));
    values["notes"] = json!("Updated context\n\nUseful details");
    let updated = call(&mut f.app, token, "command_SaveDirectoryRecord", json!({"record_id":id,"values":values}));
    assert_eq!(updated["data"]["notes"], values["notes"]);
    let audits = f.count("ledger_auditevent");
    values["notes"] = json!("Context\n\u{202e}Hidden direction");
    bad(&mut f.app, token, "command_SaveDirectoryRecord", json!({"record_id":id,"values":values}));
    assert_eq!(f.count("ledger_auditevent"), audits);
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
    assert_eq!(names.len(), 101, "{names:?}");
    let feed_accept = catalog["operations"].as_array().unwrap().iter().find(|op| op["name"] == "app_AcceptFeed").unwrap();
    let preconditions = &feed_accept["inputSchema"]["properties"]["preconditions"];
    assert_eq!(preconditions["properties"]["require_no_candidates"]["type"], "boolean");
    assert!(!preconditions["required"].as_array().unwrap().contains(&json!("require_no_candidates")));
    for name in [
        "command_SaveInvoice",
        "command_PostInvoice",
        "app_PendingFeed",
        "app_DecidedFeed",
        "app_FeedAccounts",
        "app_FeedReview",
        "app_AcceptFeed",
        "app_ImportPreview",
        "app_ImportApply",
        "app_ReviewInvoice",
        "app_TaxSettings",
        "app_InvoiceSettings",
        "command_SaveCompanyDetails",
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
    let settings=call(&mut f.app,t,"app_TaxSettings",json!({}));
    assert_eq!(settings["charge_tax"],false);
    assert_eq!(settings["revision"],0);
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
        &fb_core::bankfeeds::SecretSet::from_access_url(ai_support::BANK_ACCESS, Vec::<String>::new()).unwrap(),
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

#[test]
fn historical_invoice_api_keeps_adjustments_and_binds_preview_to_writer_connection() {
    let mut f=Fixture::new();
    let writer=f.app.ai_grant(f.company,"Synthetic invoice importer",Permission::AccountingWrite).unwrap();let token=writer["token"].as_str().unwrap();
    let customer=call(&mut f.app,token,"command_SaveCustomer",json!({"display_name":"Imported synthetic customer"}))["data"]["id"].clone();
    let id=uuid::Uuid::new_v4().to_string();
    let rows=json!([{"invoice":{"request_key":id,"customer_id":customer,"date":"2026-10-01","due_date":"2026-10-02","memo":"Original memo","tax_mode":"NONE","tax_rate":"0","lines":[{"description":"Service","quantity":"1","rate":"10","taxable":false,"income_account_id":f.income},{"line_kind":"DISCOUNT","description":"Credit","quantity":"1","rate":"2","taxable":false,"income_account_id":f.income}]},"source":{"id":id,"number":42,"display_number":"SOURCE-42","status":"draft","issue_date":"2026-10-01","due_date":"2026-10-02","created_at":"2026-10-01T10:00:00Z","updated_at":"2026-10-01T10:00:00Z","notes":"Original memo","bill_to":"Original billing address","tax_rate":0,"tax_amount":0,"subtotal":8,"gc_discount":0,"total":8,"amount_paid":0,"lines":[{"description":"Service","quantity":1,"unit_price":10,"amount":10,"taxable":0},{"description":"Credit","quantity":1,"unit_price":-2,"amount":-2,"taxable":0}],"payments":[]}}]);
    let before=f.count("sales_invoice");let audits=f.count("ledger_auditevent");
    let preview=call(&mut f.app,token,"app_InvoiceImportPreview",json!({"invoices":rows}));
    assert_eq!(f.count("sales_invoice"),before);assert_eq!(f.count("ledger_auditevent"),audits);
    let reader=f.app.ai_grant(f.company,"Synthetic reader",Permission::ReadOnly).unwrap();
    bad(&mut f.app,reader["token"].as_str().unwrap(),"app_InvoiceImportApply",json!({"preview_token":preview["preview_token"]}));
    let second=f.app.ai_grant(f.company,"Other writer",Permission::AccountingWrite).unwrap();
    bad(&mut f.app,second["token"].as_str().unwrap(),"app_InvoiceImportApply",json!({"preview_token":preview["preview_token"]}));
    let saved=call(&mut f.app,token,"app_InvoiceImportApply",json!({"preview_token":preview["preview_token"]}));
    assert_eq!(saved["invoices"][0]["total"],"8.00");assert_eq!(f.count("sales_invoice"),before+1);
    let invoice=call(&mut f.app,token,"app_Invoice",json!({"invoice_id":saved["invoices"][0]["invoice_id"]}));
    assert_eq!(invoice["lines"][1]["line_kind"],"DISCOUNT");
    assert_eq!(call(&mut f.app,token,"app_InvoiceImportApply",json!({"preview_token":preview["preview_token"]}))["already_imported"],true);
    for route in [format!("/c/{}/invoices/?q=SOURCE-42",f.company),format!("/c/{}/invoices/{}/",f.company,saved["invoices"][0]["invoice_id"].as_str().unwrap()),format!("/c/{}/invoices/{}/print/",f.company,saved["invoices"][0]["invoice_id"].as_str().unwrap())] {
        let page=fb_app::screens::render(&mut f.app,&route).unwrap();
        assert!(page.html.contains("SOURCE-42"),"original number missing from {route}");
    }
    f.app.ai_revoke(writer["id"].as_str().unwrap());
    bad(&mut f.app,token,"app_InvoiceImportApply",json!({"preview_token":preview["preview_token"]}));
}

#[test]
fn company_presentation_api_is_scoped_atomic_and_does_not_expose_connections() {
    let mut f=Fixture::new();let grant=f.app.ai_grant(f.company,"Presentation migration",Permission::AccountingWrite).unwrap();let t=grant["token"].as_str().unwrap();
    let before=call(&mut f.app,t,"app_InvoiceSettings",json!({}));
    assert_eq!(before["preferences"]["revision"],0);
    assert_eq!(before["google_connected"],false);assert_eq!(before["stripe_connected"],false);
    let mut preferences=before["preferences"]["settings"].clone();preferences["expected_revision"]=json!(0);preferences["zelle_enabled"]=json!(true);preferences["zelle_recipient"]=json!("seller@example.test");
    call(&mut f.app,t,"command_SaveCompanyDetails",json!({"name":"Synthetic presentation","currency":"USD","values":{"email":"seller@example.test","phone":"555-555-0100","invoice_preferences":preferences}}));
    let saved=call(&mut f.app,t,"app_InvoiceSettings",json!({}));assert_eq!(saved["details"]["email"],"seller@example.test");assert_eq!(saved["preferences"]["settings"]["zelle_enabled"],true);
    let snapshot=fb_db::test_support::snapshot(f.app.open_mut().unwrap().book.connection()).unwrap();
    bad(&mut f.app,t,"command_SaveCompanyDetails",json!({"name":"Stale change","currency":"USD","values":{"email":"other@example.test","invoice_preferences":preferences}}));
    assert_eq!(fb_db::test_support::snapshot(f.app.open_mut().unwrap().book.connection()).unwrap(),snapshot);
    bad(&mut f.app,t,"app_InvoiceSettings",json!({"company_id":f.other.to_db()}));
}

#[test]
fn agents_list_job_tickets_and_record_time_only_with_accounting_write() {
    let mut f = Fixture::new();
    let write = f.app.ai_grant(f.company, "Synthetic agent", Permission::AccountingWrite).unwrap();
    let token = write["token"].as_str().unwrap().to_owned();
    let customer = call(&mut f.app, &token, "command_SaveCustomer", json!({"display_name":"Harbor Lane Tavern"}))["data"]["id"].as_str().unwrap().to_owned();
    let item = call(&mut f.app, &token, "command_SaveItem", json!({"code":"On-Site Support","description":"","kind":"SERVICE","sale_rate":"125.00","income_account_id":f.income}))["data"]["id"].as_str().unwrap().to_owned();
    // Tickets are opened on the desktop; agents list them and record time.
    let ticket = {
        let o = f.app.open_mut().unwrap();
        fb_core::jobs::save_ticket(&mut o.book, o.owner, f.company, &json!({"request_key":uuid::Uuid::new_v4().to_string(),"customer_id":customer,"title":"Network outage","opened_date":"2026-11-01"})).unwrap()
    };
    let listed = call(&mut f.app, &token, "app_JobTickets", json!({"status":"OPEN"}));
    assert_eq!(listed[0]["number"], "JT-000001", "{listed}");
    let key = uuid::Uuid::new_v4().to_string();
    let values = json!({"request_key":key,"customer_id":customer,"ticket_id":ticket["id"],"item_id":item,"date":"2026-11-03","hours":"3.5h","notes":"Agent-recorded"});
    let saved = call(&mut f.app, &token, "command_SaveTimeEntry", json!({"values":values}));
    assert_eq!((saved["data"]["hours"].as_str(), saved["data"]["amount"].as_str()), (Some("3.50"), Some("437.50")), "{saved}");
    // A retry with the same request key returns the same entry.
    let again = call(&mut f.app, &token, "command_SaveTimeEntry", json!({"values":values}));
    assert_eq!(again["data"]["id"], saved["data"]["id"]);
    let listed = call(&mut f.app, &token, "app_JobTickets", json!({"customer_id":customer}));
    assert_eq!(listed[0]["unbilled"], "437.50");
    // Each time event carries its agent provenance in the same transaction.
    let provenance: i64 = f.app.open_mut().unwrap().book.read(|q| q.query_row(
        "SELECT count(*) FROM ledger_auditevent p JOIN ledger_auditevent e ON p.target='audit:'||e.id WHERE p.action='agent.provenance' AND e.action='time_entry.created'", [], |r| r.get(0))).unwrap();
    assert_eq!(provenance, 1);
    // Read-only connections list tickets but cannot record time; the company is fixed by the host.
    let read = f.app.ai_grant(f.company, "Synthetic reader", Permission::ReadOnly).unwrap();
    let reader = read["token"].as_str().unwrap().to_owned();
    let names = f.app.ai_catalogue(&reader).unwrap()["operations"].as_array().unwrap().iter().map(|o| o["name"].as_str().unwrap().to_owned()).collect::<Vec<_>>();
    assert!(names.contains(&"app_JobTickets".into()) && !names.contains(&"command_SaveTimeEntry".into()));
    assert_eq!(names.len(), 29, "read-only catalogue: 4 queries and 25 read adapters");
    let mut fresh = values.clone();
    fresh["request_key"] = json!(uuid::Uuid::new_v4().to_string());
    bad(&mut f.app, &reader, "command_SaveTimeEntry", json!({"values":fresh}));
    bad(&mut f.app, &token, "command_SaveTimeEntry", json!({"company_id":f.company.to_db(),"values":fresh}));
}

#[test]
fn decided_feed_is_read_only_scoped_paginated_and_reports_the_recorded_account() {
    use fb_core::bankfeeds;

    let mut f = Fixture::new();
    let open = f.app.open_mut().unwrap();
    let connection: String = open.book.read(|q| {
        q.query_row("SELECT connection_id FROM bankfeeds_account LIMIT 1", [], |r| r.get(0))
    }).unwrap();
    let ignored = bankfeeds::stage_bank_transactions(
        &mut open.book,
        open.owner,
        f.company,
        &connection,
        &[("synthetic-checking".into(), bankfeeds::StagedTransaction {
            remote_id: "synthetic-ignore".into(), posted_on: Some("2026-11-02".into()),
            transacted_on: None, amount: -77, description: "Synthetic ignore".into(), pending: false,
        })],
        &bankfeeds::SecretSet::none(),
    ).unwrap()[0].clone();

    let writer = f.app.ai_grant(f.company, "Synthetic reviewer", Permission::AccountingWrite).unwrap();
    let writer = writer["token"].as_str().unwrap();
    let pending = call(&mut f.app, writer, "app_PendingFeed", json!({}));
    let first = &pending["items"][1];
    assert_eq!(first["mapped_account_id"], f.bank);
    let accepted = call(&mut f.app, writer, "app_AcceptFeed", json!({
        "transaction_id":first["id"],"category":{"kind":"JOURNAL","account_id":f.expense}
    }));
    call(&mut f.app, writer, "app_IgnoreFeed", json!({"transaction_id":ignored}));

    let reader = f.app.ai_grant(f.company, "Synthetic reader", Permission::ReadOnly).unwrap();
    let reader = reader["token"].as_str().unwrap();
    let audit_count = f.count("ledger_auditevent");
    let accounts = call(&mut f.app, reader, "app_FeedAccounts", json!({}));
    assert_eq!(accounts[0]["provider_account_id"], "synthetic-checking");
    assert_eq!(accounts[0]["mapped_account_id"], f.bank);
    assert!(accounts[0].get("connection_id").is_none());
    let page = call(&mut f.app, reader, "app_DecidedFeed", json!({"limit":1}));
    assert_eq!(page["total"], 2);
    assert_eq!(page["next"], 1);
    assert_eq!(page["items"][0]["id"], ignored);
    assert_eq!(page["items"][0]["provider_transaction_id"], "synthetic-ignore");
    assert_eq!(page["items"][0]["decision"]["kind"], "IGNORED");
    assert_eq!(page["items"][0]["decision"]["counterpart_account_ids"], json!([]));
    let page = call(&mut f.app, reader, "app_DecidedFeed", json!({"limit":1,"after":1}));
    assert!(page["next"].is_null());
    assert_eq!(page["items"][0]["decision"], accepted["decision"]);
    let decision = &page["items"][0]["decision"];
    assert_eq!(decision["kind"], "JOURNAL");
    assert_eq!(decision["bank_account_id"], f.bank);
    assert_eq!(decision["counterpart_account_ids"], json!([f.expense]));
    assert_eq!(decision["stale"], false);
    assert_eq!(decision["journal_id"], accepted["journal_id"]);
    assert_eq!(f.count("ledger_auditevent"), audit_count);

    bad(&mut f.app, reader, "app_DecidedFeed", json!({"limit":0}));
    bad(&mut f.app, reader, "app_DecidedFeed", json!({"limit":101}));
    bad(&mut f.app, reader, "app_DecidedFeed", json!({"company_id":f.other.to_db()}));
    bad(&mut f.app, reader, "app_ReturnFeed", json!({"transaction_id":ignored}));
    call(&mut f.app, writer, "app_ReturnFeed", json!({"transaction_id":ignored}));
    assert_eq!(call(&mut f.app, reader, "app_DecidedFeed", json!({}))["total"], 1);
    f.app.ai_select_company(f.other).unwrap();
    let other = f.app.ai_grant(f.other, "Other company reader", Permission::ReadOnly).unwrap();
    assert_eq!(call(&mut f.app, other["token"].as_str().unwrap(), "app_DecidedFeed", json!({}))["total"], 0);
    assert_eq!(call(&mut f.app, other["token"].as_str().unwrap(), "app_FeedAccounts", json!({})), json!([]));
}

#[test]
fn feed_accept_preconditions_refuse_each_stale_field_and_a_concurrent_remap() {
    let mut f = Fixture::new();
    let grant = f.app.ai_grant(f.company, "Synthetic guarded writer", Permission::AccountingWrite).unwrap();
    let token = grant["token"].as_str().unwrap();
    let snapshot = call(&mut f.app, token, "app_FeedReview", json!({}));
    let row = &snapshot["items"][0];
    let transaction_id = row["id"].as_str().unwrap();
    let preconditions = json!({
        "id":row["id"], "status":row["status"], "return_count":row["return_count"], "amount_cents":row["amount_cents"],
        "date":row["date"], "feed_account_id":row["feed_account_id"], "mapped_account_id":row["mapped_account_id"],
        "currency":row["currency"], "mapped_account_detail_type":"BANK", "category_detail_type":"EXPENSE", "description":row["description"],
        "provider_transaction_id":row["provider_transaction_id"], "provider_host":row["provider_host"],
        "provider_account_id":snapshot["accounts"][0]["provider_account_id"], "require_exact_account_id":true,
        "require_exact_transaction_id":true, "require_no_candidates":true,
    });
    let expense = f.expense.clone();
    let accept = |app: &mut App, expected: &Value| app.ai_call(token, &json!({
        "operation":"app_AcceptFeed","arguments":{"transaction_id":transaction_id,
            "category":{"kind":"JOURNAL","account_id":expense},"preconditions":expected}
    }).to_string());
    let journals = f.count("ledger_journal");
    let audits = f.count("ledger_auditevent");
    for (field, different) in [
        ("id", json!("other")), ("status", json!("MATCHED")), ("return_count", json!(1)),
        ("amount_cents", json!("-1200")), ("date", json!("2026-11-02")),
        ("feed_account_id", json!("another feed")), ("mapped_account_id", Value::Null),
        ("currency", json!("CAD")), ("category_detail_type", json!("INCOME")), ("mapped_account_detail_type", json!("CREDIT_CARD")),
        ("description", json!("different bank memo")), ("provider_transaction_id", json!("other provider row")),
        ("provider_host", json!("other.example")), ("provider_account_id", json!("different provider account")),
    ] {
        let mut expected = preconditions.clone();
        expected[field] = different;
        let error = accept(&mut f.app, &expected).unwrap_err();
        assert!(error.to_string().contains("Bank review precondition failed"), "{field}: {error}");
        assert_eq!(f.count("ledger_journal"), journals, "{field}");
        assert_eq!(f.count("ledger_auditevent"), audits, "{field}");
    }
    let feed_id = row["feed_account_id"].as_str().unwrap();
    let open = f.app.open_mut().unwrap();
    let another_bank = services::save_account(&mut open.book, open.owner, f.company,
        services::AccountInput::new("1710", "Synthetic alternate bank").detail("BANK")).unwrap().id.to_db();
    let secret = fb_core::bankfeeds::SecretSet::none();
    fb_core::bankfeeds::map_account(&mut open.book, open.owner, f.company, feed_id, &json!(another_bank), &secret).unwrap();
    assert!(accept(&mut f.app, &preconditions).unwrap_err().to_string().contains("Bank review precondition failed"));
    assert_eq!(f.count("ledger_journal"), journals);
    assert_eq!(call(&mut f.app, token, "app_FeedReview", json!({}))["items"][0]["status"], "NEW");

    let mut current = preconditions;
    current["mapped_account_id"] = json!(another_bank);
    let accepted = accept(&mut f.app, &current).unwrap();
    assert_eq!(accepted["decision"]["bank_account_id"], another_bank);
    assert!(accept(&mut f.app, &current).unwrap_err().to_string().contains("Bank review precondition failed"));
    assert_eq!(f.count("ledger_journal"), journals + 1);
}

#[test]
fn feed_accept_rechecks_candidates_posted_after_review() {
    let mut fixture = Fixture::new();
    let grant = fixture.app.ai_grant(fixture.company, "Synthetic guarded writer", Permission::AccountingWrite).unwrap();
    let token = grant["token"].as_str().unwrap();
    let snapshot = call(&mut fixture.app, token, "app_FeedReview", json!({}));
    let row = &snapshot["items"][0];
    assert_eq!(row["candidates"], json!({"match":0,"receipt":0,"recorded":false}));
    let lines = fixture.journal("12.34")["lines"].as_array().unwrap().clone();
    let open = fixture.app.open_mut().unwrap();
    let draft = services::save_draft(&mut open.book, open.owner, fixture.company,
        services::DraftInput::new("2026-11-01", "Synthetic office supplies", lines)).unwrap();
    services::post_journal(&mut open.book, open.owner, fixture.company, draft.id, None).unwrap();
    let number = format!("{} · {}", draft.entry_number, draft.display());
    let mut args = json!({"transaction_id":row["id"], "category":{"kind":"JOURNAL","account_id":fixture.expense},
        "preconditions":{
            "id":row["id"], "status":row["status"], "return_count":row["return_count"], "amount_cents":row["amount_cents"],
            "date":row["date"], "feed_account_id":row["feed_account_id"], "mapped_account_id":row["mapped_account_id"],
            "require_no_candidates":true,
        }});
    let journals = fixture.count("ledger_journal");
    let audits = fixture.count("ledger_auditevent");
    let error = fixture.app.ai_call(token, &json!({"operation":"app_AcceptFeed","arguments":args}).to_string()).unwrap_err();
    assert!(error.to_string().contains("Bank review precondition failed"), "{error}");
    assert_eq!(fixture.count("ledger_journal"), journals);
    assert_eq!(fixture.count("ledger_auditevent"), audits);
    let current = call(&mut fixture.app, token, "app_FeedReview", json!({}));
    let mut expected = row.clone();
    expected["candidates"] = json!({"match":1,"receipt":0,"recorded":false});
    assert_eq!(current["items"][0], expected);

    // Without the guard, a new entry next to the entry it could match is still refused (worker item 40):
    // the connection matches it instead, which posts nothing.
    args["preconditions"]["require_no_candidates"] = json!(false);
    let error = fixture.app.ai_call(token, &json!({"operation":"app_AcceptFeed","arguments":args}).to_string()).unwrap_err();
    // Named by its number and date, the memo after them (worker review M2).
    assert!(error.to_string().contains(&format!("looks already recorded as {number} (2026-11-01, “Synthetic office supplies”)")), "{error}");
    assert_eq!(fixture.count("ledger_journal"), journals);
    let suggestion = call(&mut fixture.app, token, "app_SuggestFeed", json!({"transaction_id":row["id"]}));
    args["category"] = json!({"kind":"MATCH","journal_line_id":suggestion["matches"][0]["line_id"]});
    assert_eq!(call(&mut fixture.app, token, "app_AcceptFeed", args)["status"], "MATCHED");
    assert_eq!(fixture.count("ledger_journal"), journals, "a match posts nothing");
}

#[test]
fn feed_review_reports_candidates_on_all_500_undecided_rows() {
    use fb_core::bankfeeds;
    let mut fixture = Fixture::new();
    let lines = fixture.journal("12.34")["lines"].as_array().unwrap().clone();
    let open = fixture.app.open_mut().unwrap();
    let connection: String = open.book.read(|q| {
        q.query_row("SELECT connection_id FROM bankfeeds_account LIMIT 1", [], |row| row.get(0))
    }).unwrap();
    let transactions: Vec<_> = (2..=500).map(|index| json!({
        "remote_id":format!("synthetic-{index}"), "posted_on":"2026-11-01", "transacted_on":null,
        "amount":-1234, "description":"Synthetic office supplies", "pending":false
    })).collect();
    let pull: bankfeeds::StagedPull = serde_json::from_value(json!({"accounts":[{
        "conn_key":"synthetic", "remote_id":"synthetic-checking", "name":"Synthetic Checking", "org_name":"Synthetic bank",
        "currency":"USD", "balance":10000, "available":null, "balance_epoch":1793534400, "transactions":transactions
    }]})).unwrap();
    bankfeeds::store_pull(&mut open.book, open.owner, fixture.company, &connection, &pull, "2026-11-02T12:00:00Z", false, None,
        &bankfeeds::SecretSet::none()).unwrap();
    for _ in 0..4 {
        let draft = services::save_draft(&mut open.book, open.owner, fixture.company,
            services::DraftInput::new("2026-11-01", "Synthetic office supplies", lines.clone())).unwrap();
        services::post_journal(&mut open.book, open.owner, fixture.company, draft.id, None).unwrap();
    }
    let grant = fixture.app.ai_grant(fixture.company, "Synthetic reader", Permission::ReadOnly).unwrap();
    let start = std::time::Instant::now();
    let review = call(&mut fixture.app, grant["token"].as_str().unwrap(), "app_FeedReview", json!({}));
    eprintln!("app_FeedReview with 500 undecided rows: {:?}", start.elapsed());
    let rows = review["items"].as_array().unwrap();
    assert_eq!(rows.len(), 500);
    assert!(rows.iter().all(|row| row["status"] == "NEW" && row["pending"] == false
        && row["candidates"] == json!({"match":3,"receipt":0,"recorded":false})));
}

#[test]
fn feed_review_proves_unchanged_ids_and_lists_each_state_once() {
    use fb_core::bankfeeds;
    let mut f = Fixture::new();
    let open = f.app.open_mut().unwrap();
    let connection: String = open.book.read(|q| {
        q.query_row("SELECT connection_id FROM bankfeeds_account LIMIT 1", [], |r| r.get(0))
    }).unwrap();
    let pull: bankfeeds::StagedPull = serde_json::from_value(json!({"accounts":[
        {"conn_key":"source","remote_id":"AC\u{200b}CT","name":"Synthetic altered account","org_name":"",
         "currency":"CAD","balance":100,"available":null,"balance_epoch":null,"transactions":[
             {"remote_id":"ABC\u{200b}123","posted_on":"2026-11-02","transacted_on":null,"amount":-4200,"description":"Synthetic altered line","pending":false}]},
        {"conn_key":"source","remote_id":"wire-normalized","name":"Synthetic normalized at parser","org_name":"",
         "currency":"USD","balance":100,"available":null,"balance_epoch":null,"transactions":[
             {"remote_id":"wire-row","posted_on":null,"transacted_on":"2026-11-02","amount":42,"description":"Synthetic wire row","pending":true}]}
    ]})).unwrap();
    let evidence = [
        bankfeeds::ProviderIdEvidence { account_exact: true, transactions_exact: vec![true] },
        bankfeeds::ProviderIdEvidence { account_exact: false, transactions_exact: vec![false] },
    ];
    bankfeeds::store_pull_for_desktop(&mut open.book, open.owner, f.company, &connection, &pull, "2026-11-02T12:00:00Z", false, None,
        &bankfeeds::SecretSet::none(), &evidence).unwrap();
    let pending_ids: Vec<String> = open.book.read(|q| -> Result<Vec<String>, fb_db::DbError> {
        let mut st = q.prepare("SELECT id FROM bankfeeds_account WHERE connection_id=?1 AND import_state='PENDING' ORDER BY id")?;
        let rows = st.query_map([&connection], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }).unwrap();
    assert_eq!(pending_ids.len(), 2);
    let settings: Vec<_> = pending_ids.into_iter().map(|feed_account_id| bankfeeds::ImportSettings {
        feed_account_id,
        expected_state: "PENDING".into(),
        expected_date: None,
        import_from_date: None,
    }).collect();
    bankfeeds::save_mappings_and_import_settings(&mut open.book, open.owner, f.company, &connection, &[], &settings, &bankfeeds::SecretSet::none()).unwrap();
    bankfeeds::store_pull_for_desktop(&mut open.book, open.owner, f.company, &connection, &pull, "2026-11-02T12:00:00Z", false, None,
        &bankfeeds::SecretSet::none(), &evidence).unwrap();
    let writer = f.app.ai_grant(f.company, "Synthetic writer", Permission::AccountingWrite).unwrap();
    let reader = f.app.ai_grant(f.company, "Synthetic reader", Permission::ReadOnly).unwrap();
    let read = reader["token"].as_str().unwrap();
    let before = call(&mut f.app, read, "app_FeedReview", json!({}));
    let items = before["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    let exact = items.iter().find(|row| row["provider_transaction_id"] == "synthetic-1").unwrap();
    assert_eq!(exact["provider_id_exact"], true);
    assert_eq!(exact["provider_host"], "fixture.invalid");
    assert_eq!(exact["candidates"], json!({"match":0,"receipt":0,"recorded":false}));
    let other = items.iter().find(|row| row["provider_transaction_id"] == "ABC123").unwrap();
    assert_eq!(other["provider_id_exact"], false);
    assert_eq!(other["currency"], "CAD");
    let pending = items.iter().find(|row| row["provider_transaction_id"] == "wire-row").unwrap();
    assert_eq!(pending["provider_id_exact"], false);
    assert_eq!(pending["pending"], true);
    assert!(pending.get("candidates").is_none());
    let accounts = before["accounts"].as_array().unwrap();
    assert_eq!(accounts.iter().find(|a| a["provider_account_id"] == "ACCT").unwrap()["provider_id_exact"], false);
    assert_eq!(accounts.iter().find(|a| a["provider_account_id"] == "wire-normalized").unwrap()["provider_id_exact"], false);
    assert!(!before.to_string().contains("remote_identity"));
    assert!(!before.to_string().contains("secret"));
    call(&mut f.app, writer["token"].as_str().unwrap(), "app_IgnoreFeed", json!({"transaction_id":exact["id"]}));
    let after = call(&mut f.app, read, "app_FeedReview", json!({}));
    let after_rows = after["items"].as_array().unwrap();
    assert_eq!(after_rows.len(), items.len());
    let ignored = after_rows.iter().find(|row| row["id"] == exact["id"]).unwrap();
    assert_eq!(ignored["decision"]["kind"], "IGNORED");
    assert!(ignored.get("candidates").is_none());
    assert_eq!(after_rows.iter().filter(|row| row["status"] == "NEW").count(), 2);
    bad(&mut f.app, read, "app_FeedReview", json!({"company_id":f.other.to_db()}));
}

fn unattended(f: &mut Fixture, permission: Permission) -> (String, String) {
    let grant = f.app.ai_grant(f.company, "Synthetic unattended connection", permission).unwrap();
    let id = grant["id"].as_str().unwrap().to_owned();
    let proof = f.app.ai_set_unattended(&id, &uuid::Uuid::new_v4().to_string(), true).unwrap();
    (id, proof.to_string())
}

#[test]
fn unattended_uses_the_approver_across_signout_switch_company_lock_and_restart() {
    use fb_app::identity::Role;
    let mut f = Fixture::new();
    f.app.backups.configure(f.path.parent().unwrap()).unwrap();
    let approver = f.app.authenticated_session().unwrap().actor;
    let other = f.app.save_desktop_user(None, "Synthetic other administrator", Role::Administrator,
        true, Some(ai_support::PASSWORD), &[f.other]).unwrap();
    f.app.sign_in("Synthetic operator", ai_support::PASSWORD).unwrap();
    let (id, proof) = unattended(&mut f, Permission::AccountingWrite);
    let (_, reader_proof) = unattended(&mut f, Permission::ReadOnly);
    let worker = f.app.ai_prepare_unattended(&proof).unwrap();
    let original_authority = f.app.ai_session(&worker).unwrap();
    f.app.sign_out();
    assert!(f.app.authenticated_session().is_err());
    assert_eq!(f.app.ai_session(&worker).unwrap(), original_authority);
    let info = call(&mut f.app, &worker, "app_ConnectionInfo", json!({}));
    assert_eq!(info["actor_id"], approver.to_db());
    assert_eq!(info["company_id"], f.company.to_db());
    call(&mut f.app, &worker, "command_SaveCustomer", json!({"display_name":"Locked synthetic customer"}));
    assert!(f.app.query(fb_app::Query::Companies).is_err());
    let reader = f.app.ai_prepare_unattended(&reader_proof).unwrap();
    bad(&mut f.app, &reader, "command_SaveCustomer", json!({"display_name":"Forbidden read-only write"}));

    f.app.sign_in("Synthetic other administrator", ai_support::PASSWORD).unwrap();
    f.app.ai_select_company(f.other).unwrap();
    assert_eq!(f.app.authenticated_session().unwrap().actor, other);
    let audit = call(&mut f.app, &worker, "app_AuditHistory", json!({"limit":100})).to_string();
    assert!(audit.contains(&id) && audit.contains(&approver.to_db()) && audit.contains("\"unattended\":true"), "{audit}");
    assert_eq!(f.app.authenticated_session().unwrap().actor, other);
    let mut widened: Value = serde_json::from_str(&reader_proof).unwrap();
    widened["permission"] = json!("accounting_write");
    assert_eq!(f.app.ai_prepare_unattended(&widened.to_string()).unwrap_err().to_string(), "approval_gone");
    for (field, value) in [("actor",json!(other.to_db())), ("company",json!(f.other.to_db())), ("epoch",json!(uuid::Uuid::new_v4().to_string())),("secret",json!("1".repeat(64)))] {
        let mut altered: Value = serde_json::from_str(&proof).unwrap();
        altered[field] = value;
        assert_eq!(f.app.ai_prepare_unattended(&altered.to_string()).unwrap_err().to_string(), "approval_gone");
    }

    f.app.lock();
    assert_eq!(f.app.ai_prepare_unattended(&proof).unwrap_err().to_string(), "books_closed");
    let copy = f.path.with_file_name("copied.fbbook");
    std::fs::copy(&f.path, &copy).unwrap();
    let root = f.path.parent().unwrap();
    let key = keys::MasterKey::new([42; 32]);
    let mut restarted = App::new();
    restarted.backups.configure(root).unwrap();
    restarted.open_book(&copy, &key).unwrap();
    assert_eq!(restarted.ai_prepare_unattended(&proof).unwrap_err().to_string(), "approval_gone");
    restarted.open_book(&f.path, &key).unwrap();
    let resumed = restarted.ai_prepare_unattended(&proof).unwrap();
    assert_eq!(restarted.ai_catalogue(&resumed).unwrap()["unattended"], true);
    call(&mut restarted, &resumed, "command_SaveCustomer", json!({"display_name":"Synthetic restart customer"}));
    assert!(restarted.authenticated_session().is_err());
    let anchor_dir = root.join("ai-connections/unattended");
    let anchors: Vec<_> = std::fs::read_dir(anchor_dir).unwrap().map(|entry| {
        let path = entry.unwrap().path();
        let bytes = std::fs::read(&path).unwrap();
        (path, bytes)
    }).collect();
    assert_eq!(anchors.len(), 2);
    restarted.ai_forget_all().unwrap();
    for (path, bytes) in anchors { std::fs::write(path, bytes).unwrap(); }
    assert_eq!(restarted.ai_prepare_unattended(&proof).unwrap_err().to_string(), "approval_gone");
    restarted.sign_in("Synthetic operator", ai_support::PASSWORD).unwrap();
    assert_eq!(restarted.ai_prepare_unattended(&proof).unwrap_err().to_string(), "approval_gone");
}

#[test]
fn signed_in_desk_cannot_spend_unattended_transfer_reviews_or_clear_them() {
    let mut f = Fixture::new();
    f.app.backups.configure(f.path.parent().unwrap()).unwrap();
    let (_, proof) = unattended(&mut f, Permission::AccountingWrite);
    f.app.sign_out();
    let worker = f.app.ai_prepare_unattended(&proof).unwrap();
    let savings = call(&mut f.app, &worker, "command_SaveAccount", json!({"code":"101","name":"Savings","detail_type":"BANK"}))["data"]["id"].as_str().unwrap().to_owned();
    for sign_in_before_review in [false, true] {
        if sign_in_before_review { f.app.sign_in("Synthetic operator", ai_support::PASSWORD).unwrap(); }
        let values = json!({"request_key":uuid::Uuid::new_v4().to_string(),"date":"2026-11-02","from_account_id":f.bank,"to_account_id":savings,"amount":"20.00","memo":"Synthetic reserve"});
        let review = call(&mut f.app, &worker, "command_ReviewTransfer", json!({"values":values}));
        if !sign_in_before_review { f.app.sign_in("Synthetic operator", ai_support::PASSWORD).unwrap(); }
        let record = json!({"type":"RecordTransfer","company_id":f.company.to_db(),"values":values,"review_token":review["data"]["token"]});
        let error = f.app.run(fb_app::commands::parse_command(&record.to_string()).unwrap()).unwrap_err();
        assert!(error.to_string().contains("review"), "{error}");
        if sign_in_before_review { f.app.sign_out(); }
        call(&mut f.app, &worker, "command_RecordTransfer", json!({"values":values,"review_token":review["data"]["token"]}));
        f.app.sign_out();
    }
}

#[test]
fn history_and_csv_show_unattended_connection() {
    let mut f = Fixture::new();
    f.app.backups.configure(f.path.parent().unwrap()).unwrap();
    let (id, proof) = unattended(&mut f, Permission::AccountingWrite);
    f.app.sign_out();
    let worker = f.app.ai_prepare_unattended(&proof).unwrap();
    call(&mut f.app, &worker, "command_SaveCustomer", json!({"display_name":"Synthetic History customer"}));
    f.app.sign_in("Synthetic operator", ai_support::PASSWORD).unwrap();
    for query in ["", "?format=csv"] {
        let text = fb_app::screens::render(&mut f.app, &format!("/c/{}/history/{query}", f.company.to_db())).unwrap().html;
        assert!(text.contains(&format!("Unattended AI connection: Synthetic unattended connection ({id})")), "{query}");
    }
}

#[test]
fn sign_in_prunes_missing_anchors_but_book_and_anchor_rollback_can_restore_consent() {
    let mut f = Fixture::new();
    f.app.backups.configure(f.path.parent().unwrap()).unwrap();
    let (_, proof) = unattended(&mut f, Permission::AccountingWrite);
    let anchor = std::fs::read_dir(f.path.parent().unwrap().join("ai-connections/unattended")).unwrap().next().unwrap().unwrap().path();
    let anchor_bytes = std::fs::read(&anchor).unwrap();
    f.app.lock();
    let old_book = std::fs::read(&f.path).unwrap();
    f.app.ai_forget_all().unwrap();
    let key = keys::MasterKey::new([42; 32]);
    f.app.open_book(&f.path, &key).unwrap();
    assert_eq!(f.app.ai_prepare_unattended(&proof).unwrap_err().to_string(), "approval_gone");
    assert_eq!(f.app.open.as_mut().unwrap().book.read(|tx| {
        tx.query_row("SELECT COUNT(*) FROM desktop_ai_unattended", [], |row| row.get::<_, i64>(0))
    }).unwrap(), 1);
    f.app.sign_in("Synthetic operator", ai_support::PASSWORD).unwrap();
    assert_eq!(f.count("desktop_ai_unattended"), 0);
    std::fs::write(&anchor, anchor_bytes).unwrap();
    assert_eq!(f.app.ai_prepare_unattended(&proof).unwrap_err().to_string(), "approval_gone");
    f.app.lock();
    std::fs::write(&f.path, old_book).unwrap();
    f.app.open_book(&f.path, &key).unwrap();
    f.app.ai_prepare_unattended(&proof).unwrap();
}

#[test]
fn sign_in_survives_failed_ai_prune() {
    let mut f = Fixture::new();
    f.app.backups.configure(f.path.parent().unwrap()).unwrap();
    let (_, proof) = unattended(&mut f, Permission::ReadOnly);
    let directory = f.path.parent().unwrap().join("ai-connections/unattended");
    let anchor = std::fs::read_dir(directory).unwrap().next().unwrap().unwrap().path();
    std::fs::remove_file(anchor).unwrap();
    f.app.open_mut().unwrap().book.connection().execute_batch(
        "CREATE TEMP TRIGGER synthetic_prune_failure BEFORE DELETE ON main.desktop_ai_unattended
         BEGIN SELECT RAISE(ABORT,'synthetic cleanup failure'); END;",
    ).unwrap();
    f.app.sign_out();
    f.app.sign_in("Synthetic operator", ai_support::PASSWORD).unwrap();
    assert!(f.app.authenticated_session().is_ok());
    assert_eq!(f.count("desktop_ai_unattended"), 1);
    assert_eq!(f.app.ai_prepare_unattended(&proof).unwrap_err().to_string(), "approval_gone");
}

#[test]
fn hosted_client_sign_in_prunes_missing_anchor_without_closing_books() {
    use fb_app::multiuser::{ClientInfo, Policy, SessionHost};
    let mut f = Fixture::new();
    f.app.backups.configure(f.path.parent().unwrap()).unwrap();
    let (_, proof) = unattended(&mut f, Permission::ReadOnly);
    f.app.lock();
    f.app.ai_forget_all().unwrap();
    f.app.open_book(&f.path, &keys::MasterKey::new([42; 32])).unwrap();
    let (mut host, local) = SessionHost::host_with_local(f.app, Policy::default(), "Synthetic host");
    let remote = host.open_session(ClientInfo::remote("Synthetic laptop", None)).unwrap();
    host.with_session(&remote, |app| {
        app.sign_in("Synthetic operator", ai_support::PASSWORD).unwrap();
        assert!(app.authenticated_session().is_ok());
        assert_eq!(app.ai_prepare_unattended(&proof).unwrap_err().to_string(), "approval_gone");
    }).unwrap();
    host.with_session(&local, |app| {
        assert!(app.is_open());
        let count: i64 = app.open.as_mut().unwrap().book.read(|tx| {
            tx.query_row("SELECT COUNT(*) FROM desktop_ai_unattended", [], |row| row.get(0))
        }).unwrap();
        assert_eq!(count, 0);
    }).unwrap();
}

#[test]
fn sign_in_prunes_a_superseded_enablement_from_older_book_contents() {
    let mut f = Fixture::new();
    let root = f.path.parent().unwrap();
    f.app.backups.configure(root).unwrap();
    let (id, old_proof) = unattended(&mut f, Permission::ReadOnly);
    let epoch = serde_json::from_str::<Value>(&old_proof).unwrap()["epoch"].as_str().unwrap().to_owned();
    let key = keys::MasterKey::new([42; 32]);
    f.app.lock();
    let old_book = std::fs::read(&f.path).unwrap();
    f.app.open_book(&f.path, &key).unwrap();
    f.app.sign_in("Synthetic operator", ai_support::PASSWORD).unwrap();
    f.app.ai_select_company(f.company).unwrap();
    f.app.ai_grant_remembered(f.company, "Synthetic unattended connection", Permission::ReadOnly, Some(&id)).unwrap();
    f.app.ai_set_unattended(&id, &epoch, true).unwrap();
    f.app.lock();
    std::fs::write(&f.path, old_book).unwrap();
    f.app.open_book(&f.path, &key).unwrap();
    f.app.sign_in("Synthetic operator", ai_support::PASSWORD).unwrap();
    assert_eq!(f.count("desktop_ai_unattended"), 0);
    assert_eq!(f.app.ai_prepare_unattended(&old_proof).unwrap_err().to_string(), "approval_gone");
}

#[cfg(unix)]
#[test]
fn temporarily_unreadable_anchor_does_not_look_forgotten() {
    let mut f = Fixture::new();
    let root = f.path.parent().unwrap().to_path_buf();
    f.app.backups.configure(&root).unwrap();
    let (_, proof) = unattended(&mut f, Permission::ReadOnly);
    let anchors = root.join("ai-connections/unattended");
    let held = anchors.with_extension("held");
    std::fs::rename(&anchors, &held).unwrap();
    std::os::unix::fs::symlink(&anchors, &anchors).unwrap();
    assert_eq!(f.app.ai_unattended_status(&proof).unwrap_err().to_string(), "anchor_unavailable");
    std::fs::remove_file(&anchors).unwrap();
    std::fs::rename(held, anchors).unwrap();
    f.app.ai_prepare_unattended(&proof).unwrap();
}

#[test]
fn unattended_first_write_after_restart_masks_saved_bank_sign_in() {
    let mut f = Fixture::new();
    let root = f.path.parent().unwrap();
    f.app.backups.configure(root).unwrap();
    let (_, proof) = unattended(&mut f, Permission::AccountingWrite);
    f.app.lock();
    f.app.open_book(&f.path, &keys::MasterKey::new([42; 32])).unwrap();
    let worker = f.app.ai_prepare_unattended(&proof).unwrap();
    call(&mut f.app, &worker, "command_SaveCustomer", json!({
        "display_name": format!("Synthetic {}", ai_support::BANK_SECRET)
    }));
    let stored: String = f.app.open.as_mut().unwrap().book.read(|tx| {
        tx.query_row("SELECT display_name FROM directory_customer", [], |row| row.get(0))
    }).unwrap();
    assert_eq!(stored, "Synthetic ***");
}

#[test]
fn roster_revocation_cannot_be_undone_by_restoring_an_older_book_at_the_same_path() {
    use fb_app::identity::Role;
    for change in ["deactivate", "demote", "remove company", "reset password"] {
        let mut f = Fixture::new();
        let root = f.path.parent().unwrap().to_path_buf();
        f.app.backups.configure(&root).unwrap();
        let approver = f.app.save_desktop_user(None, "Synthetic approving administrator", Role::Administrator,
            true, Some(ai_support::PASSWORD), &[f.company, f.other]).unwrap();
        f.app.sign_in("Synthetic approving administrator", ai_support::PASSWORD).unwrap();
        let (_, proof) = unattended(&mut f, Permission::AccountingWrite);
        let anchor = std::fs::read_dir(root.join("ai-connections/unattended")).unwrap().next().unwrap().unwrap().path();
        let older_anchor = std::fs::read(&anchor).unwrap();
        f.app.lock();
        let older_book = std::fs::read(&f.path).unwrap();
        let key = keys::MasterKey::new([42; 32]);
        f.app.open_book(&f.path, &key).unwrap();
        f.app.sign_in("Synthetic operator", ai_support::PASSWORD).unwrap();
        let worker = f.app.ai_prepare_unattended(&proof).unwrap();
        let all = [f.company,f.other];
        f.app.save_desktop_user(Some(approver), "Synthetic approving administrator",
            if change == "demote" { Role::Bookkeeper } else { Role::Administrator },
            change != "deactivate",
            (change == "reset password").then_some("Synthetic replacement approval password"),
            if change == "remove company" { &all[1..] } else { &all }).unwrap();
        assert_eq!(f.app.ai_prepare_unattended(&proof).unwrap_err().to_string(), "paused_signed_out", "{change}");
        assert_eq!(f.app.ai_session(&worker).unwrap_err().to_string(), "paused_signed_out", "{change} after acceptance");
        assert_eq!(f.app.ai_unattended_status(&proof).unwrap().unwrap()["reason"], "account_changed", "{change}");
        let receipt = std::fs::read(&anchor).unwrap();
        std::fs::write(&anchor, &older_anchor).unwrap();
        assert!(f.app.ai_unattended_status(&proof).is_err(), "replaying only the active anchor: {change}");
        std::fs::write(&anchor, &receipt).unwrap();
        f.app.sign_in("Synthetic operator", ai_support::PASSWORD).unwrap();
        f.app.save_desktop_user(Some(approver), "Synthetic approving administrator", Role::Administrator, true,
            Some(ai_support::PASSWORD), &all).unwrap();
        assert!(f.app.ai_prepare_unattended(&proof).is_err(), "{change} and restoration of roster");
        assert_eq!(f.app.ai_unattended_status(&proof).unwrap().unwrap()["reason"], "account_changed");
        f.app.ai_forget_all().unwrap();
        std::fs::write(&anchor, receipt).unwrap();
        assert!(f.app.ai_unattended_status(&proof).is_err(), "replaying the receipt after Forget");
        f.app.lock();
        std::fs::write(&f.path, older_book).unwrap();
        f.app.open_book(&f.path, &key).unwrap();
        // The older contents hold the active row again, but the file on disk is the receipt: locked
        // access stays off (it is reported as switched off, like a change that did not commit).
        assert_eq!(f.app.ai_prepare_unattended(&proof).unwrap_err().to_string(), "paused_signed_out", "{change} and restoration of older book");
        if change == "demote" {
            std::fs::write(&anchor, older_anchor).unwrap();
            f.app.ai_prepare_unattended(&proof).unwrap();
        }
    }
}

fn stored_verifier(f: &mut Fixture, id: &str) -> Vec<u8> {
    f.app.open.as_mut().unwrap().book.read(|tx| {
        tx.query_row("SELECT verifier FROM desktop_ai_unattended WHERE id=?1", [id], |row| row.get(0))
    }).unwrap()
}

fn anchor_files(f: &Fixture) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(f.path.parent().unwrap().join("ai-connections/unattended")).unwrap()
        .map(|entry| entry.unwrap().path()).collect()
}

const FAIL_CONSENT_WRITES: &str =
    "CREATE TEMP TRIGGER synthetic_consent_insert BEFORE INSERT ON main.desktop_ai_unattended
     BEGIN SELECT RAISE(ABORT,'synthetic consent failure'); END;
     CREATE TEMP TRIGGER synthetic_consent_update BEFORE UPDATE ON main.desktop_ai_unattended
     BEGIN SELECT RAISE(ABORT,'synthetic consent failure'); END;";
const ALLOW_CONSENT_WRITES: &str = "DROP TRIGGER synthetic_consent_insert; DROP TRIGGER synthetic_consent_update;";
const FAIL_CONSENT_DELETES: &str =
    "CREATE TEMP TRIGGER synthetic_consent_delete BEFORE DELETE ON main.desktop_ai_unattended
     BEGIN SELECT RAISE(ABORT,'synthetic consent failure'); END;";
const ALLOW_CONSENT_DELETES: &str = "DROP TRIGGER synthetic_consent_delete;";

// Third review, F8. What an account change makes of the consent row must not depend on the local
// file: with a receipt-shaped file already in the anchor's place, the row was left active, and the
// anchor put back afterwards turned locked access on again with nobody signed in.
#[test]
fn account_change_switches_locked_access_off_whatever_the_local_file_holds() {
    use fb_app::identity::Role;
    use sha2::{Digest, Sha256};
    for planted in ["filler", "names the real anchor"] {
        for change in ["reset password", "demote, then restore", "deactivate, then restore"] {
            let case = format!("{planted}; {change}");
            let mut f = Fixture::new();
            f.app.backups.configure(f.path.parent().unwrap()).unwrap();
            let all = [f.company, f.other];
            let approver = f.app.save_desktop_user(None, "Synthetic approving administrator", Role::Administrator,
                true, Some(ai_support::PASSWORD), &all).unwrap();
            f.app.sign_in("Synthetic approving administrator", ai_support::PASSWORD).unwrap();
            let (id, proof) = unattended(&mut f, Permission::AccountingWrite);
            let anchor = anchor_files(&f).pop().unwrap();
            let active = std::fs::read(&anchor).unwrap();
            let before = stored_verifier(&mut f, &id);
            let mut forged = vec![2u8; 74];
            if planted == "filler" {
                forged[1..].fill(0xAA);
            } else {
                forged[1..33].copy_from_slice(&Sha256::digest(&active[1..]));
                forged[33..].fill(0x11);
            }
            std::fs::write(&anchor, &forged).unwrap();
            f.app.sign_out();
            f.app.sign_in("Synthetic operator", ai_support::PASSWORD).unwrap();
            f.app.save_desktop_user(Some(approver), "Synthetic approving administrator",
                if change.starts_with("demote") { Role::Bookkeeper } else { Role::Administrator },
                !change.starts_with("deactivate"),
                (change == "reset password").then_some("Synthetic replacement approval password"), &all).unwrap();
            assert_ne!(stored_verifier(&mut f, &id), before, "the row no longer holds the active verifier: {case}");
            assert_eq!(f.app.ai_unattended_status(&proof).unwrap().unwrap()["reason"], "account_changed", "{case}");
            assert_eq!(f.app.ai_prepare_unattended(&proof).unwrap_err().to_string(), "paused_signed_out", "{case}");
            if change.ends_with("then restore") {
                // A roster change signs the person who made it out.
                f.app.sign_in("Synthetic operator", ai_support::PASSWORD).unwrap();
                f.app.save_desktop_user(Some(approver), "Synthetic approving administrator", Role::Administrator,
                    true, None, &all).unwrap();
            }
            f.app.sign_out();
            // The anchor that was in place before the change comes back.
            std::fs::write(&anchor, &active).unwrap();
            assert_eq!(f.app.ai_prepare_unattended(&proof).unwrap_err().to_string(), "approval_gone", "{case}");
            assert!(f.app.ai_unattended_status(&proof).is_err(), "{case}");
        }
    }
}

// F8, second sequence, and N14: a change that wrote the receipt and then did not commit leaves the
// approval switched off but remembered, and the same change tried again completes the switch-off.
#[test]
fn account_change_that_fails_part_way_stays_recoverable_and_a_retry_completes_it() {
    use fb_app::identity::Role;
    for change in ["reset password", "demote"] {
        let mut f = Fixture::new();
        f.app.backups.configure(f.path.parent().unwrap()).unwrap();
        let all = [f.company, f.other];
        let approver = f.app.save_desktop_user(None, "Synthetic approving administrator", Role::Administrator,
            true, Some(ai_support::PASSWORD), &all).unwrap();
        f.app.sign_in("Synthetic approving administrator", ai_support::PASSWORD).unwrap();
        let (id, proof) = unattended(&mut f, Permission::AccountingWrite);
        let anchor = anchor_files(&f).pop().unwrap();
        let active = std::fs::read(&anchor).unwrap();
        let before = stored_verifier(&mut f, &id);
        f.app.sign_out();
        f.app.sign_in("Synthetic operator", ai_support::PASSWORD).unwrap();
        let attempt = |f: &mut Fixture| f.app.save_desktop_user(Some(approver), "Synthetic approving administrator",
            if change == "demote" { Role::Bookkeeper } else { Role::Administrator }, true,
            (change == "reset password").then_some("Synthetic replacement approval password"), &all).map(drop);
        f.app.open_mut().unwrap().book.connection().execute_batch(FAIL_CONSENT_WRITES).unwrap();
        assert!(attempt(&mut f).is_err(), "{change}");
        f.app.open_mut().unwrap().book.connection().execute_batch(ALLOW_CONSENT_WRITES).unwrap();
        assert_eq!(stored_verifier(&mut f, &id), before, "the change did not happen: {change}");
        assert_eq!(std::fs::read(&anchor).unwrap().len(), 74, "its receipt was written: {change}");
        assert_eq!(f.app.ai_prepare_unattended(&proof).unwrap_err().to_string(), "paused_signed_out", "{change}");
        assert_eq!(f.app.ai_unattended_status(&proof).unwrap().unwrap()["reason"], "account_changed", "{change}");
        attempt(&mut f).unwrap();
        assert_ne!(stored_verifier(&mut f, &id), before, "{change}");
        assert_eq!(f.app.ai_unattended_status(&proof).unwrap().unwrap()["reason"], "account_changed", "{change}");
        f.app.sign_out();
        std::fs::write(&anchor, &active).unwrap();
        assert_eq!(f.app.ai_prepare_unattended(&proof).unwrap_err().to_string(), "approval_gone", "{change}");
    }
}

// N14: turning locked access on again keeps the receipt until the new row and anchor exist.
#[test]
fn a_failed_turn_on_again_leaves_the_switched_off_approval_to_retry() {
    use fb_app::identity::Role;
    let mut f = Fixture::new();
    f.app.backups.configure(f.path.parent().unwrap()).unwrap();
    let all = [f.company, f.other];
    let approver = f.app.save_desktop_user(None, "Synthetic approving administrator", Role::Administrator,
        true, Some(ai_support::PASSWORD), &all).unwrap();
    f.app.sign_in("Synthetic approving administrator", ai_support::PASSWORD).unwrap();
    let (id, proof) = unattended(&mut f, Permission::AccountingWrite);
    let epoch = serde_json::from_str::<Value>(&proof).unwrap()["epoch"].as_str().unwrap().to_owned();
    f.app.sign_out();
    f.app.sign_in("Synthetic operator", ai_support::PASSWORD).unwrap();
    f.app.save_desktop_user(Some(approver), "Synthetic approving administrator", Role::Administrator, true,
        Some("Synthetic replacement approval password"), &all).unwrap();
    f.app.sign_out();
    f.app.sign_in("Synthetic approving administrator", "Synthetic replacement approval password").unwrap();
    f.app.ai_grant_remembered(f.company, "Synthetic unattended connection", Permission::AccountingWrite, Some(&id)).unwrap();
    let receipt = anchor_files(&f);
    assert_eq!(receipt.len(), 1);
    f.app.open_mut().unwrap().book.connection().execute_batch(FAIL_CONSENT_WRITES).unwrap();
    assert!(f.app.ai_set_unattended(&id, &epoch, true).is_err());
    f.app.open_mut().unwrap().book.connection().execute_batch(ALLOW_CONSENT_WRITES).unwrap();
    assert_eq!(anchor_files(&f), receipt, "the receipt is still there and the new anchor is not");
    assert_eq!(f.app.ai_unattended_status(&proof).unwrap().unwrap()["reason"], "account_changed");
    f.app.ai_grant_remembered(f.company, "Synthetic unattended connection", Permission::AccountingWrite, Some(&id)).unwrap();
    let again = f.app.ai_set_unattended(&id, &epoch, true).unwrap().to_string();
    let files = anchor_files(&f);
    assert_eq!(files.len(), 1);
    assert_ne!(files, receipt, "the old receipt went once the new anchor was in place");
    f.app.sign_out();
    f.app.ai_prepare_unattended(&again).unwrap();
    assert!(f.app.ai_unattended_status(&proof).is_err(), "the earlier proof matches nothing");
}

// Fourth review, F9. Removing the consent row must not depend on the profile folder: with a folder in
// the anchor's place, Forget's book cleanup, the approver's own switch and an account change all
// stopped before the row was removed, and the row could then be matched again after Forget.
#[test]
fn nothing_in_the_profile_folder_keeps_the_consent_row_in_the_book() {
    use fb_app::identity::Role;
    use std::os::unix::fs::PermissionsExt;
    for obstacle in ["folder in the anchor's place", "read-only anchors folder", "unreadable anchor"] {
        for switch in ["forget", "approver switches off", "account change", "sign-in"] {
            let case = format!("{obstacle}; {switch}");
            let mut f = Fixture::new();
            f.app.backups.configure(f.path.parent().unwrap()).unwrap();
            let all = [f.company, f.other];
            let approver = f.app.save_desktop_user(None, "Synthetic approving administrator", Role::Administrator,
                true, Some(ai_support::PASSWORD), &all).unwrap();
            f.app.sign_in("Synthetic approving administrator", ai_support::PASSWORD).unwrap();
            let (id, proof) = unattended(&mut f, Permission::AccountingWrite);
            let epoch = serde_json::from_str::<Value>(&proof).unwrap()["epoch"].as_str().unwrap().to_owned();
            let anchor = anchor_files(&f).pop().unwrap();
            let folder = anchor.parent().unwrap().to_path_buf();
            let active = std::fs::read(&anchor).unwrap();
            let restore = |f: &Fixture| {
                let _ = std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700));
                let _ = std::fs::remove_dir_all(&anchor);
                let _ = std::fs::remove_file(&anchor);
                std::fs::write(&anchor, &active).unwrap();
                let _ = f;
            };
            let plant = || match obstacle {
                "folder in the anchor's place" => {
                    std::fs::remove_file(&anchor).unwrap();
                    std::fs::create_dir(&anchor).unwrap();
                    std::fs::write(anchor.join("kept"), b"x").unwrap();
                }
                "read-only anchors folder" => std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o500)).unwrap(),
                _ => std::fs::set_permissions(&anchor, std::fs::Permissions::from_mode(0o000)).unwrap(),
            };
            let expect_row_gone = match switch {
                "forget" => {
                    plant();
                    // A regular file that could not be removed is reported; a folder in its place is no anchor.
                    assert_eq!(f.app.ai_forget_all().unwrap(), obstacle == "read-only anchors folder", "{case}");
                    true
                }
                "approver switches off" => {
                    plant();
                    assert_eq!(f.app.ai_set_unattended(&id, &epoch, false).unwrap(), Value::Null, "{case}");
                    true
                }
                "account change" => {
                    // The other Administrator is signed in first, so the change itself meets the
                    // obstacle and not the cleanup at sign-in.
                    f.app.sign_out();
                    f.app.sign_in("Synthetic operator", ai_support::PASSWORD).unwrap();
                    plant();
                    f.app.save_desktop_user(Some(approver), "Synthetic approving administrator", Role::Administrator,
                        false, None, &all).unwrap();
                    // Gone, or replaced by a value no anchor matches when the anchor could still be read.
                    obstacle != "read-only anchors folder"
                }
                _ => {
                    plant();
                    f.app.sign_out();
                    f.app.sign_in("Synthetic operator", ai_support::PASSWORD).unwrap();
                    // Only an anchor that is not a regular file counts as missing at sign-in.
                    obstacle == "folder in the anchor's place"
                }
            };
            if obstacle == "read-only anchors folder" && switch != "sign-in" {
                assert_eq!(std::fs::metadata(&anchor).unwrap().len(), 0, "{case}: a file that cannot be removed is emptied");
            }
            // Read straight from the book: some of these switches sign the desk person out.
            let rows: i64 = f.app.open.as_mut().unwrap().book.read(|tx| {
                tx.query_row("SELECT COUNT(*) FROM desktop_ai_unattended", [], |row| row.get(0))
            }).unwrap();
            if switch != "sign-in" || expect_row_gone {
                assert_eq!(rows, i64::from(!expect_row_gone), "{case}");
            }
            // The folder is put right and the anchor that was in place comes back.
            f.app.sign_out();
            restore(&f);
            let after = f.app.ai_prepare_unattended(&proof);
            if switch == "sign-in" && !expect_row_gone {
                // Nothing was switched off in this case: a readable anchor is simply back.
                after.unwrap();
            } else {
                assert_eq!(after.unwrap_err().to_string(), "approval_gone", "{case}");
            }
        }
    }
}

// Fifth review, N22 and N23. With no book open Forget reports a regular file it could not remove,
// and only that: an empty folder at an anchor's name is removed, and a full one is no anchor.
#[test]
fn forget_with_no_book_open_reports_only_a_file_it_could_not_remove() {
    use std::os::unix::fs::PermissionsExt;
    for obstacle in ["nothing", "empty folder", "full folder", "read-only anchors folder"] {
        let mut f = Fixture::new();
        f.app.backups.configure(f.path.parent().unwrap()).unwrap();
        let (_, proof) = unattended(&mut f, Permission::ReadOnly);
        let anchor = anchor_files(&f).pop().unwrap();
        let folder = anchor.parent().unwrap().to_path_buf();
        let active = std::fs::read(&anchor).unwrap();
        let other = folder.join(uuid::Uuid::new_v4().to_string());
        match obstacle {
            "empty folder" => std::fs::create_dir(&other).unwrap(),
            "full folder" => {
                std::fs::create_dir(&other).unwrap();
                std::fs::write(other.join("kept"), b"x").unwrap();
            }
            "read-only anchors folder" => std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o500)).unwrap(),
            _ => (),
        }
        f.app.lock();
        assert_eq!(f.app.ai_forget_all().unwrap(), obstacle == "read-only anchors folder", "{obstacle}");
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700)).unwrap();
        match obstacle {
            "read-only anchors folder" => assert_eq!(std::fs::metadata(&anchor).unwrap().len(), 0, "emptied in place"),
            _ => assert!(!anchor.exists(), "{obstacle}"),
        }
        assert_eq!(other.exists(), obstacle == "full folder", "{obstacle}");
        // The books are opened again and someone signs in: the row whose file is gone or empty goes too,
        // so the anchor that was in place matches nothing when it comes back.
        let root = f.path.parent().unwrap();
        let key = keys::MasterKey::new([42; 32]);
        let mut reopened = App::new();
        reopened.backups.configure(root).unwrap();
        reopened.open_book(&f.path, &key).unwrap();
        reopened.sign_in("Synthetic operator", ai_support::PASSWORD).unwrap();
        reopened.sign_out();
        std::fs::write(&anchor, &active).unwrap();
        assert_eq!(reopened.ai_prepare_unattended(&proof).unwrap_err().to_string(), "approval_gone", "{obstacle}");
    }
}

// Sixth review, F11. Emptying in place is only for a file that could be an anchor. A hard link at
// an anchor's name is the same file as something elsewhere, and a longer file is not an anchor.
#[test]
fn only_a_file_that_could_be_an_anchor_is_ever_emptied() {
    use fb_app::identity::Role;
    use std::os::unix::fs::PermissionsExt;
    for switch in ["forget", "approver switches off", "account change"] {
        let mut f = Fixture::new();
        f.app.backups.configure(f.path.parent().unwrap()).unwrap();
        let all = [f.company, f.other];
        let approver = f.app.save_desktop_user(None, "Synthetic approving administrator", Role::Administrator,
            true, Some(ai_support::PASSWORD), &all).unwrap();
        f.app.sign_in("Synthetic approving administrator", ai_support::PASSWORD).unwrap();
        let (id, proof) = unattended(&mut f, Permission::AccountingWrite);
        let epoch = serde_json::from_str::<Value>(&proof).unwrap()["epoch"].as_str().unwrap().to_owned();
        let anchor = anchor_files(&f).pop().unwrap();
        let folder = anchor.parent().unwrap().to_path_buf();
        let root = f.path.parent().unwrap().to_path_buf();
        // The anchor's own name is a second name of a file kept elsewhere; two more names in the
        // folder are a second name of a larger file, and a larger file with no other name.
        let kept = root.join("synthetic-kept-anchor");
        std::fs::hard_link(&anchor, &kept).unwrap();
        let elsewhere = root.join("synthetic-other-file");
        std::fs::write(&elsewhere, vec![7; 5000]).unwrap();
        std::fs::hard_link(&elsewhere, folder.join(uuid::Uuid::new_v4().to_string())).unwrap();
        let long = folder.join(uuid::Uuid::new_v4().to_string());
        std::fs::write(&long, vec![9; 75]).unwrap();
        let book_bytes = std::fs::metadata(&f.path).unwrap().len();
        std::fs::hard_link(&f.path, folder.join(uuid::Uuid::new_v4().to_string())).unwrap();
        if switch == "account change" {
            f.app.sign_out();
            f.app.sign_in("Synthetic operator", ai_support::PASSWORD).unwrap();
        }
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o500)).unwrap();
        match switch {
            "forget" => assert!(f.app.ai_forget_all().unwrap(), "the files that stayed are reported"),
            "approver switches off" => assert_eq!(f.app.ai_set_unattended(&id, &epoch, false).unwrap(), Value::Null),
            _ => f.app.save_desktop_user(Some(approver), "Synthetic approving administrator", Role::Administrator,
                false, None, &all).map(drop).unwrap(),
        }
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(std::fs::metadata(&kept).unwrap().len(), 33, "{switch}: a file with another name is left alone");
        assert_eq!(std::fs::read(&elsewhere).unwrap(), vec![7; 5000], "{switch}");
        assert_eq!(std::fs::read(&long).unwrap(), vec![9; 75], "{switch}: longer than any anchor");
        assert_eq!(std::fs::metadata(&f.path).unwrap().len(), book_bytes, "{switch}: the book");
        // Locked access is off all the same: the book's record went or changed.
        f.app.sign_out();
        assert_eq!(f.app.ai_prepare_unattended(&proof).unwrap_err().to_string(), "approval_gone", "{switch}");
    }
}

// Fifth review, F10. When the write to the book fails, Forget says so and the row is still there:
// the approval is forgotten only by a Forget that succeeds.
#[test]
fn forget_fails_when_the_book_cannot_be_written_and_a_retry_finishes() {
    let mut f = Fixture::new();
    f.app.backups.configure(f.path.parent().unwrap()).unwrap();
    let (_, proof) = unattended(&mut f, Permission::ReadOnly);
    let anchor = anchor_files(&f).pop().unwrap();
    let active = std::fs::read(&anchor).unwrap();
    f.app.open_mut().unwrap().book.connection().execute_batch(FAIL_CONSENT_DELETES).unwrap();
    assert!(f.app.ai_forget_all().is_err());
    assert!(anchor_files(&f).is_empty(), "the anchors are removed even so");
    f.app.sign_out();
    assert_eq!(f.app.ai_prepare_unattended(&proof).unwrap_err().to_string(), "approval_gone");
    // The limit the documents state: the row is still in the book, so the file brings it back.
    std::fs::write(&anchor, &active).unwrap();
    f.app.ai_prepare_unattended(&proof).unwrap();
    // Nobody is signed in now; Forget needs no sign-in.
    f.app.open.as_mut().unwrap().book.connection().execute_batch(ALLOW_CONSENT_DELETES).unwrap();
    assert!(!f.app.ai_forget_all().unwrap());
    std::fs::write(&anchor, &active).unwrap();
    assert_eq!(f.app.ai_prepare_unattended(&proof).unwrap_err().to_string(), "approval_gone");
}

// N16: a file in the earlier 32-byte form is not an anchor.
#[test]
fn a_bare_32_byte_file_is_not_an_anchor() {
    let mut f = Fixture::new();
    f.app.backups.configure(f.path.parent().unwrap()).unwrap();
    let (_, proof) = unattended(&mut f, Permission::ReadOnly);
    let anchor = anchor_files(&f).pop().unwrap();
    let active = std::fs::read(&anchor).unwrap();
    std::fs::write(&anchor, &active[1..]).unwrap();
    assert_eq!(f.app.ai_prepare_unattended(&proof).unwrap_err().to_string(), "approval_gone");
}

#[test]
fn unattended_requires_an_administrator_and_disabling_requires_the_approver() {
    use fb_app::identity::Role;
    let mut f = Fixture::new();
    f.app.backups.configure(f.path.parent().unwrap()).unwrap();
    for (name,role) in [("Synthetic another admin",Role::Administrator),("Synthetic bookkeeper consent",Role::Bookkeeper)] {
        f.app.save_desktop_user(None, name, role, true, Some(ai_support::PASSWORD), &[f.company]).unwrap();
        f.app.sign_in("Synthetic operator", ai_support::PASSWORD).unwrap();
    }
    let (id, proof) = unattended(&mut f, Permission::ReadOnly);
    let epoch = serde_json::from_str::<Value>(&proof).unwrap()["epoch"].as_str().unwrap().to_owned();
    for name in ["Synthetic another admin", "Synthetic bookkeeper consent"] {
        f.app.sign_out();
        f.app.sign_in(name, ai_support::PASSWORD).unwrap();
        f.app.ai_grant_remembered(f.company, "Synthetic unattended connection", Permission::ReadOnly, Some(&id)).unwrap();
        assert!(f.app.ai_set_unattended(&id, &epoch, false).is_err());
        assert!(f.app.ai_set_unattended(&id, &epoch, true).is_err());
        f.app.ai_unattended_status(&proof).unwrap();
    }
    f.app.sign_out();
    f.app.sign_in("Synthetic operator", ai_support::PASSWORD).unwrap();
    f.app.ai_grant_remembered(f.company, "Synthetic unattended connection", Permission::ReadOnly, Some(&id)).unwrap();
    f.app.ai_set_unattended(&id, &epoch, false).unwrap();
    assert!(f.app.ai_unattended_status(&proof).is_err());
    let new = f.app.ai_set_unattended(&id, &epoch, true).unwrap();
    assert_ne!(new.to_string(), proof);
    f.app.ai_unattended_status(&new.to_string()).unwrap();
    assert!(f.app.ai_unattended_status(&proof).is_err(), "a replayed old file cannot revive its old scope");
}

#[test]
fn hosted_unattended_worker_is_independent_of_the_local_session_slot() {
    use fb_app::multiuser::{ClientInfo, EndReason, Policy, SessionHost};
    let mut f = Fixture::new();
    f.app.backups.configure(f.path.parent().unwrap()).unwrap();
    let (_, proof) = unattended(&mut f, Permission::AccountingWrite);
    let worker = f.app.ai_prepare_unattended(&proof).unwrap();
    let binding = f.app.ai_session(&worker).unwrap();
    let (mut hosted, local) = SessionHost::host_with_local(f.app, Policy::default(), "Synthetic host");
    hosted.end_session(&local, EndReason::Abandoned);
    let replacement = hosted.open_session(ClientInfo::local("Synthetic host")).unwrap();
    hosted.with_session(&replacement, |app| {
        assert!(app.authenticated_session().is_err());
        assert_eq!(app.ai_session(&worker).unwrap(), binding);
        call(app, &worker, "command_SaveCustomer", json!({"display_name":"Synthetic hosted customer"}));
        assert!(app.authenticated_session().is_err());
    }).unwrap();
}

fn full_preconditions(snapshot: &Value, row: &Value) -> Value {
    json!({
        "id":row["id"], "status":row["status"], "return_count":row["return_count"], "amount_cents":row["amount_cents"],
        "date":row["date"], "feed_account_id":row["feed_account_id"], "mapped_account_id":row["mapped_account_id"],
        "currency":row["currency"], "mapped_account_detail_type":"BANK", "category_detail_type":"EXPENSE", "description":row["description"],
        "provider_transaction_id":row["provider_transaction_id"], "provider_host":row["provider_host"],
        "provider_account_id":snapshot["accounts"][0]["provider_account_id"], "require_exact_account_id":true,
        "require_exact_transaction_id":true, "require_no_candidates":true,
    })
}

/// Rows of the book read without a signed-in session, as the locked-access tests need.
fn book_rows(f: &mut Fixture, sql: &str) -> Vec<String> {
    f.app.open.as_mut().unwrap().book.read(|tx| {
        let mut statement = tx.prepare(sql)?;
        statement.query_map([], |row| row.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()
    }).unwrap()
}

// Locked access and the bank sync together, as CyberBrain uses them every day: after a restart,
// with nobody signed in at the desk, the connection reads the review and records one guarded entry.
#[test]
fn a_locked_access_connection_reads_the_review_and_records_one_guarded_bank_entry() {
    let mut f = Fixture::new();
    let root = f.path.parent().unwrap().to_path_buf();
    f.app.backups.configure(&root).unwrap();
    let approver = f.app.authenticated_session().unwrap().actor;
    // What a signed-in connection sees, to compare with.
    let attended = f.app.ai_grant(f.company, "Synthetic attended reader", Permission::ReadOnly).unwrap();
    let attended = attended["token"].as_str().unwrap().to_owned();
    let reads = ["app_FeedReview", "app_FeedAccounts", "app_DecidedFeed"];
    let seen: Vec<Value> = reads.iter().map(|name| call(&mut f.app, &attended, name, json!({}))).collect();
    let (id, proof) = unattended(&mut f, Permission::AccountingWrite);
    let (_, reader_proof) = unattended(&mut f, Permission::ReadOnly);

    // A restart: the books are closed and opened again, and nobody signs in.
    f.app.lock();
    f.app.open_book(&f.path, &keys::MasterKey::new([42; 32])).unwrap();
    assert!(f.app.authenticated_session().is_err());
    let worker = f.app.ai_prepare_unattended(&proof).unwrap();
    let reader = f.app.ai_prepare_unattended(&reader_proof).unwrap();
    for token in [&worker, &reader] {
        for (name, expected) in reads.iter().zip(&seen) {
            assert_eq!(&call(&mut f.app, token, name, json!({})), expected, "{name}");
            bad(&mut f.app, token, name, json!({"company_id":f.other.to_db()}));
        }
    }

    let snapshot = &seen[0];
    let row = snapshot["items"].as_array().unwrap().iter()
        .find(|row| row["status"] == "NEW" && row["pending"] != true).unwrap().clone();
    assert_eq!(row["provider_id_exact"], true);
    assert_eq!(row["candidates"], json!({"match":0,"receipt":0,"recorded":false}));
    // The payee text carries the saved bank sign-in's password, which must never be stored.
    let arguments = json!({"transaction_id":row["id"],
        "category":{"kind":"JOURNAL","account_id":f.expense,"payee_name":format!("Synthetic payee {}", ai_support::BANK_SECRET)},
        "preconditions":full_preconditions(snapshot, &row)});
    let journals = book_rows(&mut f, "SELECT id FROM ledger_journal");
    let events = book_rows(&mut f, "SELECT id FROM ledger_auditevent");
    let since = format!("id NOT IN ({})", events.iter().map(|id| format!("'{id}'")).collect::<Vec<_>>().join(","));

    // A read-only approval cannot record it, locked or not.
    bad(&mut f.app, &reader, "app_AcceptFeed", arguments.clone());
    assert_eq!(book_rows(&mut f, "SELECT id FROM ledger_journal"), journals);
    assert_eq!(book_rows(&mut f, "SELECT id FROM ledger_auditevent"), events);

    let accepted = call(&mut f.app, &worker, "app_AcceptFeed", arguments.clone());
    assert_eq!(accepted["status"], "ACCEPTED", "{accepted}");
    assert_eq!(accepted["decision"]["bank_account_id"], f.bank);
    assert!(f.app.authenticated_session().is_err(), "the call signed nobody in");

    // One balanced entry, with the bank sign-in text masked.
    let after = book_rows(&mut f, "SELECT id FROM ledger_journal");
    let new: Vec<_> = after.iter().filter(|id| !journals.contains(id)).collect();
    assert_eq!(new.len(), 1);
    let totals = book_rows(&mut f, &format!(
        "SELECT sum(debit)||':'||sum(credit)||':'||count(*) FROM ledger_journalline WHERE journal_id='{}'", new[0]));
    let parts: Vec<_> = totals[0].split(':').collect();
    assert!(parts[0] == parts[1] && parts[0] != "0" && parts[2] == "2", "{totals:?}");
    let names = book_rows(&mut f, &format!("SELECT name FROM ledger_journalline WHERE journal_id='{}'", new[0]));
    assert_eq!(names, ["Synthetic payee ***", "Synthetic payee ***"]);
    let everything = book_rows(&mut f, "SELECT coalesce(group_concat(detail),'') FROM ledger_auditevent").join("");
    assert!(!everything.contains(ai_support::BANK_SECRET));

    // The approver is the actor on every new event, and each has a provenance event in the same
    // book that names this connection, the operation and a request, and says nobody was signed in.
    let actors = book_rows(&mut f, &format!("SELECT DISTINCT actor_id FROM ledger_auditevent WHERE {since}"));
    assert_eq!(actors, [approver.to_db()]);
    let provenance: Vec<Value> = book_rows(&mut f, &format!(
        "SELECT detail FROM ledger_auditevent WHERE action='agent.provenance' AND {since}"))
        .iter().map(|detail| serde_json::from_str(detail).unwrap()).collect();
    let others = book_rows(&mut f, &format!("SELECT id FROM ledger_auditevent WHERE action<>'agent.provenance' AND {since}"));
    assert!(!others.is_empty());
    assert_eq!(provenance.len(), others.len(), "one provenance event for each event of the call");
    for event in &provenance {
        assert_eq!(event["connection_id"], id.as_str(), "{event}");
        assert_eq!(event["operation"], "app_AcceptFeed", "{event}");
        assert_eq!(event["unattended"], true, "{event}");
        assert!(event["request_id"].as_str().is_some_and(|request| !request.is_empty()), "{event}");
        assert!(others.contains(&event["event_id"].as_str().unwrap().to_owned()), "{event}");
    }

    // The same reviewed state cannot record it a second time, and the decision is listed once.
    let again = f.app.ai_call(&worker, &json!({"operation":"app_AcceptFeed","arguments":arguments}).to_string()).unwrap_err();
    assert!(again.to_string().contains("Bank review precondition failed"), "{again}");
    assert_eq!(book_rows(&mut f, "SELECT id FROM ledger_journal"), after);
    let decided = call(&mut f.app, &reader, "app_DecidedFeed", json!({}));
    assert_eq!(decided["items"].as_array().unwrap().iter().filter(|item| item["id"] == row["id"]).count(), 1, "{decided}");
}

// An entry posted by hand after the review was read stops the locked-access write, and nothing
// at all is recorded for the refused call.
#[test]
fn a_locked_access_write_is_refused_when_the_money_was_entered_after_the_review() {
    let mut f = Fixture::new();
    let root = f.path.parent().unwrap().to_path_buf();
    f.app.backups.configure(&root).unwrap();
    let (_, proof) = unattended(&mut f, Permission::AccountingWrite);
    f.app.sign_out();
    let worker = f.app.ai_prepare_unattended(&proof).unwrap();
    let snapshot = call(&mut f.app, &worker, "app_FeedReview", json!({}));
    let row = snapshot["items"][0].clone();
    assert_eq!(row["candidates"], json!({"match":0,"receipt":0,"recorded":false}));

    // Someone records the same purchase by hand.
    let lines = f.journal("12.34")["lines"].as_array().unwrap().clone();
    let company = f.company;
    let open = f.app.open.as_mut().unwrap();
    let draft = services::save_draft(&mut open.book, open.owner, company,
        services::DraftInput::new("2026-11-01", "Synthetic office supplies", lines)).unwrap();
    services::post_journal(&mut open.book, open.owner, company, draft.id, None).unwrap();

    let before: Vec<Vec<String>> = ["ledger_journal", "ledger_auditevent"].iter()
        .map(|table| book_rows(&mut f, &format!("SELECT id FROM {table}"))).collect();
    let arguments = json!({"transaction_id":row["id"], "category":{"kind":"JOURNAL","account_id":f.expense},
        "preconditions":full_preconditions(&snapshot, &row)});
    let refused = f.app.ai_call(&worker, &json!({"operation":"app_AcceptFeed","arguments":arguments}).to_string()).unwrap_err();
    assert!(refused.to_string().contains("Bank review precondition failed"), "{refused}");
    let after: Vec<Vec<String>> = ["ledger_journal", "ledger_auditevent"].iter()
        .map(|table| book_rows(&mut f, &format!("SELECT id FROM {table}"))).collect();
    assert_eq!(after, before, "no journal, audit or provenance event for the refused call");
    let current = call(&mut f.app, &worker, "app_FeedReview", json!({}));
    assert_eq!(current["items"][0]["status"], "NEW");
    assert_eq!(current["items"][0]["candidates"], json!({"match":1,"receipt":0,"recorded":false}));
    assert!(call(&mut f.app, &worker, "app_DecidedFeed", json!({}))["items"].as_array().unwrap().is_empty());
}

// ---- Receipts through a connection: `include_settlement` and `require_same_settlement` ----

const RECEIPT_REFUSAL: &str = "Receipt precondition failed. Nothing was recorded. Review the invoice again.";
const NO_SUCH_REVIEW: &str = "Invalid review. Open a fresh receipt review.";

/// A posted invoice of the customer dated 2026-11-01, entered at the desk.
fn posted_invoice(f: &mut Fixture, customer: &str, total: &str) -> String {
    let company = f.company.to_db();
    let values = json!({"request_key":uuid::Uuid::new_v4().to_string(),"customer_id":customer,"date":"2026-11-01","due_date":"2026-11-30",
        "memo":"Synthetic invoice","tax_mode":"NONE","lines":[{"description":"Synthetic work","quantity":"1","rate":total,"income_account_id":f.income}]});
    let invoice = ai_support::run(&mut f.app, json!({"type":"SaveInvoice","company_id":company,"values":values}))["id"].as_str().unwrap().to_owned();
    ai_support::run(&mut f.app, json!({"type":"PostInvoice","company_id":company,"invoice_id":invoice,"expected_revision":1}));
    invoice
}
/// A customer with a posted 200.00 invoice dated 2026-11-01, entered at the desk.
fn customer_with_invoice(f: &mut Fixture) -> (String, String) {
    let company = f.company.to_db();
    let customer = ai_support::run(&mut f.app, json!({"type":"SaveCustomer","company_id":company,"display_name":"Synthetic Orchard"}))["id"]
        .as_str().unwrap().to_owned();
    let invoice = posted_invoice(f, &customer, "200.00");
    (customer, invoice)
}
/// The values of a check receipt dated 2026-11-05 into the bank, all of it on one invoice.
fn receipt_values(f: &Fixture, customer: &str, invoice: &str, amount: &str) -> Value {
    json!({"request_key":uuid::Uuid::new_v4().to_string(),"customer_id":customer,"date":"2026-11-05","method":"CHECK","reference":"Check 5521",
        "memo":"","destination":"BANK","bank_id":f.bank,"amount":amount,"allocations":[{"invoice_id":invoice,"amount":amount}]})
}
/// A receipt entered in FreedomBooks itself by the person at the desk.
fn native_receipt(f: &mut Fixture, customer: &str, invoice: &str, amount: &str) -> Value {
    let owner = f.app.open.as_ref().unwrap().owner;
    native_receipt_by(f, owner, customer, invoice, amount)
}
/// The same by a named person, for a book reopened with nobody signed in.
fn native_receipt_by(f: &mut Fixture, actor: fb_core::UserId, customer: &str, invoice: &str, amount: &str) -> Value {
    let values = receipt_values(f, customer, invoice, amount);
    let company = f.company;
    let book = &mut f.app.open.as_mut().unwrap().book;
    let review = fb_core::sales::review_receipt(book, actor, company, &values).unwrap();
    fb_core::sales::record_receipt(book, actor, company, &values, Some(&review)).unwrap()
}
fn native_reversal(f: &mut Fixture, receipt: &Value) {
    let owner = f.app.open.as_ref().unwrap().owner;
    native_reversal_by(f, owner, receipt)
}
fn native_reversal_by(f: &mut Fixture, actor: fb_core::UserId, receipt: &Value) {
    let company = f.company;
    let book = &mut f.app.open.as_mut().unwrap().book;
    fb_core::sales::reverse_receipt(book, actor, company, receipt["id"].as_str().unwrap(), Some(1),
        &json!("2026-11-06"), &json!("Synthetic bounced check")).unwrap();
}
/// A new credit memo applied to the invoice in FreedomBooks itself.
fn native_credit(f: &mut Fixture, customer: &str, invoice: &str, amount: &str) -> Value {
    use fb_core::credits;
    let memo = json!({"request_key":uuid::Uuid::new_v4().to_string(),"customer_id":customer,"date":"2026-11-03","memo":"Synthetic return","tax_mode":"NONE",
        "lines":[{"item_id":null,"description":"Synthetic return","quantity":"1","rate":amount,"taxable":false,"income_account_id":f.income}]});
    let apply = json!({"request_key":uuid::Uuid::new_v4().to_string(),"date":"2026-11-04","allocations":[{"invoice_id":invoice,"amount":amount}]});
    let company = f.company;
    let open = f.app.open.as_mut().unwrap();
    let review = credits::review_credit_memo(&mut open.book, open.owner, company, &memo).unwrap();
    let memo = credits::record_credit_memo(&mut open.book, open.owner, company, &memo, Some(&review)).unwrap();
    let review = credits::review_application(&mut open.book, open.owner, company, memo["id"].as_str().unwrap(), &apply).unwrap();
    credits::apply_credit(&mut open.book, open.owner, company, memo["id"].as_str().unwrap(), &apply, Some(&review)).unwrap()
}
/// Every row of every table of the open book.
fn whole_book(f: &Fixture) -> fb_db::test_support::Snapshot {
    fb_db::test_support::snapshot(f.app.open.as_ref().unwrap().book.connection()).unwrap()
}
/// The number of receipts in the book and the invoice's outstanding amount, as `actor` reads them.
fn receipts_and_outstanding(f: &mut Fixture, actor: fb_core::UserId, invoice: &str) -> (i64, String) {
    let company = f.company;
    let receipts = book_rows(f, "SELECT id FROM sales_receipt").len() as i64;
    let open = f.app.open.as_mut().unwrap();
    let balances = fb_core::sales::invoice_balances(&mut open.book, actor, company).unwrap();
    let row = balances.iter().find(|row| row["invoice_id"] == invoice).unwrap();
    (receipts, row["outstanding"].as_str().unwrap().to_owned())
}
/// A review as every caller made it before the settlement existed: nothing asked for.
fn review_receipt(f: &mut Fixture, token: &str, values: &Value) -> Value {
    call(&mut f.app, token, "command_ReviewCustomerReceipt", json!({"sales_receipt":false,"values":values}))["data"].clone()
}
fn record_arguments(values: &Value, review: &Value, key: Option<bool>) -> Value {
    let mut arguments = json!({"sales_receipt":false,"values":values,"review_token":review["token"]});
    if let Some(key) = key {
        arguments["require_same_settlement"] = json!(key);
    }
    arguments
}
/// The names in an object, in the order the answer carries them.
fn names(value: &Value) -> Vec<&str> {
    value.as_object().unwrap().keys().map(String::as_str).collect()
}
/// The virtual-machine steps SQLite runs on the book's connection during `work`: a measure of
/// every read and write, the same for the same statements over the same rows.
fn vm_steps<T>(f: &mut Fixture, work: impl FnOnce(&mut Fixture) -> T) -> (u64, T) {
    use std::sync::{Arc, atomic::{AtomicU64, Ordering}};
    let steps = Arc::new(AtomicU64::new(0));
    let counter = steps.clone();
    f.app.open.as_mut().unwrap().book.connection_mut()
        .progress_handler(1, Some(move || { counter.fetch_add(1, Ordering::Relaxed); false })).unwrap();
    let result = work(f);
    f.app.open.as_mut().unwrap().book.connection_mut().progress_handler(0, None::<fn() -> bool>).unwrap();
    (steps.load(Ordering::Relaxed), result)
}
/// The value with each random identifier replaced by the order it first appears in, wherever in a
/// string it stands, and each digest by a mark, so two runs over books made the same way can be
/// compared word for word.
fn without_random_parts(value: &Value, seen: &mut Vec<String>) -> Value {
    let is_id = |text: &str| text.len() == 36 && text.char_indices().all(|(i, c)| if [8, 13, 18, 23].contains(&i) { c == '-' } else { c.is_ascii_hexdigit() });
    match value {
        Value::String(text) if text.len() == 64 && text.chars().all(|c| c.is_ascii_hexdigit()) => json!("<digest>"),
        Value::String(text) => {
            let (mut plain, mut rest) = (String::new(), text.as_str());
            while !rest.is_empty() {
                if let Some(id) = rest.get(..36).filter(|start| is_id(start)) {
                    let position = seen.iter().position(|known| known == id).unwrap_or_else(|| { seen.push(id.to_owned()); seen.len() - 1 });
                    plain.push_str(&format!("<id {position}>"));
                    rest = &rest[36..];
                } else {
                    let first = rest.chars().next().unwrap();
                    plain.push(first);
                    rest = &rest[first.len_utf8()..];
                }
            }
            json!(plain)
        }
        Value::Array(items) => Value::Array(items.iter().map(|item| without_random_parts(item, seen)).collect()),
        Value::Object(fields) => Value::Object(fields.iter().map(|(name, field)| (name.clone(), without_random_parts(field, seen))).collect()),
        other => other.clone(),
    }
}

// Without the key a connection's receipt records as it did before the key existed (ec7e2af4):
// whatever was entered after its review, while enough is outstanding. Only what that commit has is
// used here, so this test runs there unchanged.
#[test]
fn without_the_key_a_connection_records_beside_what_changed_after_its_review() {
    let mut f = Fixture::new();
    let operator = f.app.authenticated_session().unwrap().actor;
    let (customer, invoice) = customer_with_invoice(&mut f);
    let grant = f.app.ai_grant(f.company, "Synthetic agent", Permission::AccountingWrite).unwrap();
    let token = grant["token"].as_str().unwrap().to_owned();
    let seen = native_receipt(&mut f, &customer, &invoice, "20.00");
    assert_eq!(receipts_and_outstanding(&mut f, operator, &invoice), (1, "180.00".to_owned()));

    let record = |f: &mut Fixture, amount: &str, change: &dyn Fn(&mut Fixture)| {
        let values = receipt_values(f, &customer, &invoice, amount);
        let review = review_receipt(f, &token, &values);
        change(f);
        let recorded = call(&mut f.app, &token, "command_RecordCustomerReceipt", record_arguments(&values, &review, None));
        assert_eq!((&recorded["outcome"], &recorded["data"]["kind"], &recorded["data"]["amount"]), (&json!("done"), &json!("RCP"), &json!(amount)));
    };
    // The reviewed sequence: the same money entered natively after the review. Both are recorded.
    record(&mut f, "75.00", &|f| { native_receipt(f, &customer, &invoice, "75.00"); });
    assert_eq!(receipts_and_outstanding(&mut f, operator, &invoice), (3, "30.00".to_owned()));
    record(&mut f, "10.00", &|f| { native_credit(f, &customer, &invoice, "10.00"); });
    assert_eq!(receipts_and_outstanding(&mut f, operator, &invoice), (4, "10.00".to_owned()));
    record(&mut f, "10.00", &|f| native_reversal(f, &seen));
    assert_eq!(receipts_and_outstanding(&mut f, operator, &invoice), (5, "20.00".to_owned()));

    // And as before, a receipt larger than what is left is refused.
    let values = receipt_values(&f, &customer, &invoice, "20.00");
    let review = review_receipt(&mut f, &token, &values);
    native_receipt(&mut f, &customer, &invoice, "5.00");
    let error = f.app.ai_call(&token, &json!({"operation":"command_RecordCustomerReceipt","arguments":record_arguments(&values, &review, None)}).to_string()).unwrap_err();
    assert!(error.to_string().contains("Allocation exceeds the actual outstanding invoice amount. Review again."), "{error}");
    assert_eq!(receipts_and_outstanding(&mut f, operator, &invoice), (6, "15.00".to_owned()));
}

// A review that does not ask for the settlement is what it was before the settlement existed
// (ec7e2af4): the same result, word for word, and the same reads. Only what that commit has is
// used here, so this test runs there unchanged, and the line it prints is compared between the two
// commits (`PIN`): the review and the recording as returned, random identifiers aside, and the
// SQLite steps each took.
#[test]
fn a_review_that_does_not_ask_is_what_it_was_before_the_settlement_existed() {
    let mut f = Fixture::new();
    let operator = f.app.authenticated_session().unwrap().actor;
    let (customer, invoice) = customer_with_invoice(&mut f);
    let second = posted_invoice(&mut f, &customer, "60.00");
    // Something of each kind an app-level test can enter is already on the first invoice.
    native_receipt(&mut f, &customer, &invoice, "20.00");
    native_credit(&mut f, &customer, &invoice, "10.00");
    let bounced = native_receipt(&mut f, &customer, &invoice, "5.00");
    native_reversal(&mut f, &bounced);
    let grant = f.app.ai_grant(f.company, "Synthetic agent", Permission::AccountingWrite).unwrap();
    let token = grant["token"].as_str().unwrap().to_owned();
    let other = f.app.ai_grant(f.company, "Synthetic second agent", Permission::AccountingWrite).unwrap();
    let other = other["token"].as_str().unwrap().to_owned();

    let mut values = receipt_values(&f, &customer, &invoice, "95.00");
    values["allocations"] = json!([{"invoice_id":invoice,"amount":"75.00"},{"invoice_id":second,"amount":"20.00"}]);
    let (review_steps, review) = vm_steps(&mut f, |f| review_receipt(f, &token, &values));
    // Exactly the two things a review has always returned.
    assert_eq!(names(&review), ["review", "token"]);
    assert_eq!(names(&review["review"]), ["data", "company_snapshot", "customer_snapshot", "mapping", "entries", "allocations", "lines", "method_label"]);
    // The count is a property of the statements and the rows, not of the run.
    let (again_steps, again) = vm_steps(&mut f, |f| review_receipt(f, &token, &values));
    assert_eq!(review_steps, again_steps);
    assert_eq!(review["review"], again["review"]);
    let (record_steps, recorded) = vm_steps(&mut f, |f| call(&mut f.app, &token, "command_RecordCustomerReceipt", record_arguments(&values, &review, None)));
    assert_eq!(receipts_and_outstanding(&mut f, operator, &invoice), (3, "95.00".to_owned()));
    let mut seen = Vec::new();
    println!("PIN {}", json!({"review_steps":review_steps,"record_steps":record_steps,
        "review":without_random_parts(&review, &mut seen),"recorded":without_random_parts(&recorded, &mut seen)}));

    // And as before, such a review belongs to the signed-in session, not to the approval that made
    // it: another write approval of the same session can record with its token.
    let values = receipt_values(&f, &customer, &invoice, "10.00");
    let review = review_receipt(&mut f, &token, &values);
    let recorded = call(&mut f.app, &other, "command_RecordCustomerReceipt", record_arguments(&values, &review, None));
    assert_eq!((&recorded["outcome"], &recorded["data"]["amount"]), (&json!("done"), &json!("10.00")));
    assert_eq!(receipts_and_outstanding(&mut f, operator, &invoice), (4, "85.00".to_owned()));
    // Spent by that: the approval that made it cannot use it again.
    bad(&mut f.app, &token, "command_RecordCustomerReceipt", record_arguments(&values, &review, None));
}

/// A review that asks for the settlement.
fn review_with_settlement(f: &mut Fixture, token: &str, values: &Value) -> Value {
    call(&mut f.app, token, "command_ReviewCustomerReceipt", json!({"sales_receipt":false,"values":values,"include_settlement":true}))["data"].clone()
}
/// A call's answer as the transport returns it.
fn answer(f: &mut Fixture, token: &str, operation: &str, arguments: Value) -> Value {
    f.app.ai_response(token, Some(&json!({"operation":operation,"arguments":arguments}).to_string())).unwrap()
}
fn refusal(message: &str) -> Value {
    json!({"_agent_error":true,"error":"operation_refused","code":"operation_refused","message":message})
}
/// The refusal as the transport returns it, and proof that the call left the book as it was.
fn refused_and_nothing_recorded(f: &mut Fixture, token: &str, values: &Value, review: &Value, what: &str) {
    let before = whole_book(f);
    assert_eq!(answer(f, token, "command_RecordCustomerReceipt", record_arguments(values, review, Some(true))), refusal(RECEIPT_REFUSAL), "{what}");
    assert!(whole_book(f) == before, "{what}: no journal, receipt, allocation, audit or provenance event was written");
}
/// A recording that names a review this caller does not hold, with the key and without it:
/// refused as an unknown review, and the book is as it was.
fn no_such_review(f: &mut Fixture, token: &str, values: &Value, review: &Value, what: &str) {
    let before = whole_book(f);
    for key in [Some(true), None] {
        assert_eq!(answer(f, token, "command_RecordCustomerReceipt", record_arguments(values, review, key)), refusal(NO_SUCH_REVIEW), "{what}");
    }
    assert!(whole_book(f) == before, "{what}");
}
/// The same asked at the desk itself, outside any approval.
fn no_such_review_at_the_desk(f: &mut Fixture, values: &Value, review: &Value, what: &str) {
    let before = whole_book(f);
    for key in [true, false] {
        let record = json!({"type":"RecordCustomerReceipt","company_id":f.company.to_db(),"sales_receipt":false,"values":values,
            "review_token":review["token"],"require_same_settlement":key});
        let error = f.app.run(fb_app::commands::parse_command(&record.to_string()).unwrap()).unwrap_err();
        assert!(error.to_string().contains(NO_SUCH_REVIEW), "{what}: {error}");
    }
    assert!(whole_book(f) == before, "{what}");
}

// Review finding 1 through a signed-in connection, then each kind of change, then the desk.
#[test]
fn a_signed_in_connection_records_a_receipt_only_beside_the_settlement_it_reviewed() {
    let mut f = Fixture::new();
    let operator = f.app.authenticated_session().unwrap().actor;
    let (customer, invoice) = customer_with_invoice(&mut f);
    let grant = f.app.ai_grant(f.company, "Synthetic agent", Permission::AccountingWrite).unwrap();
    let token = grant["token"].as_str().unwrap().to_owned();
    let reader = f.app.ai_grant(f.company, "Synthetic reader", Permission::ReadOnly).unwrap();
    let reader = reader["token"].as_str().unwrap().to_owned();

    // The catalogue shows each key, optional, on its own operation alone.
    let catalogue = f.app.ai_catalogue(&token).unwrap();
    let operation = |name: &str| catalogue["operations"].as_array().unwrap().iter().find(|op| op["name"] == name).unwrap().clone();
    let (review_operation, record_operation) = (operation("command_ReviewCustomerReceipt"), operation("command_RecordCustomerReceipt"));
    for (with, key, without) in [(&record_operation, "require_same_settlement", &review_operation), (&review_operation, "include_settlement", &record_operation)] {
        assert_eq!(with["inputSchema"]["properties"][key]["type"], "boolean", "{key}");
        assert!(!with["inputSchema"]["required"].as_array().unwrap().contains(&json!(key)), "{key}");
        assert!(without["inputSchema"]["properties"].get(key).is_none(), "{key}");
    }

    // Not asked for, the review is the two things it always was; `false` is the same as absent.
    let values = receipt_values(&f, &customer, &invoice, "75.00");
    assert_eq!(names(&review_receipt(&mut f, &token, &values)), ["review", "token"]);
    let unasked = call(&mut f.app, &token, "command_ReviewCustomerReceipt", json!({"sales_receipt":false,"values":values,"include_settlement":false}));
    assert_eq!(names(&unasked["data"]), ["review", "token"]);
    // Anything but a boolean is refused.
    bad(&mut f.app, &token, "command_ReviewCustomerReceipt", json!({"sales_receipt":false,"values":values,"include_settlement":"yes"}));

    // A 200.00 invoice; a 75.00 receipt is reviewed, asking for the settlement.
    let review = review_with_settlement(&mut f, &token, &values);
    assert_eq!(names(&review), ["review", "token", "settlement"]);
    assert_eq!(review["settlement"], json!([{"invoice_id":invoice,"total_cents":"20000","outstanding_cents":"20000","entries":[]}]));
    assert!(review["token"].is_string() && review["review"]["allocations"][0]["invoice_id"] == invoice.as_str());
    assert_eq!(review["review"], unasked["data"]["review"], "asking changes nothing else in the review");
    // The same 75.00 is entered at the desk, through the payment screen's own two commands.
    let company = f.company.to_db();
    let desk = receipt_values(&f, &customer, &invoice, "75.00");
    let desk_review = ai_support::run(&mut f.app, json!({"type":"ReviewCustomerReceipt","company_id":company,"sales_receipt":false,"values":desk}));
    assert_eq!(names(&desk_review), ["review", "token"]);
    let native = ai_support::run(&mut f.app, json!({"type":"RecordCustomerReceipt","company_id":company,"sales_receipt":false,"values":desk,"review_token":desk_review["token"]}));
    // The reviewed one is refused: one receipt, 125.00 outstanding.
    refused_and_nothing_recorded(&mut f, &token, &values, &review, "a receipt was added");
    assert_eq!(receipts_and_outstanding(&mut f, operator, &invoice), (1, "125.00".to_owned()));
    // The refused call spent its review; the same token cannot record afterwards either.
    bad(&mut f.app, &token, "command_RecordCustomerReceipt", record_arguments(&values, &review, None));
    assert_eq!(receipts_and_outstanding(&mut f, operator, &invoice), (1, "125.00".to_owned()));

    // The next review shows what stopped it, and with nothing changed the receipt is recorded.
    let values = receipt_values(&f, &customer, &invoice, "25.00");
    let review = review_with_settlement(&mut f, &token, &values);
    assert_eq!(review["settlement"], json!([{"invoice_id":invoice,"total_cents":"20000","outstanding_cents":"12500",
        "entries":[{"id":native["id"],"kind":"receipt","amount_cents":"7500","date":"2026-11-05","reversed":false}]}]));
    let events = f.count("ledger_auditevent WHERE action='receipt.recorded'");
    let recorded = call(&mut f.app, &token, "command_RecordCustomerReceipt", record_arguments(&values, &review, Some(true)));
    assert_eq!((&recorded["outcome"], &recorded["data"]["kind"], &recorded["data"]["amount"]), (&json!("done"), &json!("RCP"), &json!("25.00")));
    assert_eq!(receipts_and_outstanding(&mut f, operator, &invoice), (2, "100.00".to_owned()));
    assert_eq!(f.count("ledger_auditevent WHERE action='receipt.recorded'"), events + 1);

    // A credit applied after the review, and a reversal after the review, each stop it too.
    let values = receipt_values(&f, &customer, &invoice, "10.00");
    let review = review_with_settlement(&mut f, &token, &values);
    let credit = native_credit(&mut f, &customer, &invoice, "10.00");
    refused_and_nothing_recorded(&mut f, &token, &values, &review, "a credit was applied");
    let review = review_with_settlement(&mut f, &token, &values);
    assert_eq!(review["settlement"][0]["outstanding_cents"], "9000");
    assert_eq!(review["settlement"][0]["entries"][0], json!({"id":credit["id"],"kind":"credit","amount_cents":"1000","date":"2026-11-04","reversed":false}));
    native_reversal(&mut f, &native);
    refused_and_nothing_recorded(&mut f, &token, &values, &review, "a receipt was reversed");
    let review = review_with_settlement(&mut f, &token, &values);
    assert_eq!(review["settlement"][0]["outstanding_cents"], "16500");
    assert_eq!(review["settlement"][0]["entries"][1], json!({"id":native["id"],"kind":"receipt","amount_cents":"7500","date":"2026-11-05","reversed":true}));
    assert_eq!(receipts_and_outstanding(&mut f, operator, &invoice), (2, "165.00".to_owned()));
    // A receipt added and reversed again leaves 165.00 outstanding, and still stops it: the
    // entries are not the ones reviewed.
    let undone = native_receipt(&mut f, &customer, &invoice, "40.00");
    native_reversal(&mut f, &undone);
    assert_eq!(receipts_and_outstanding(&mut f, operator, &invoice), (3, "165.00".to_owned()));
    refused_and_nothing_recorded(&mut f, &token, &values, &review, "a receipt was added and reversed");
    let review = review_with_settlement(&mut f, &token, &values);
    assert_eq!(review["settlement"][0]["outstanding_cents"], "16500");
    assert!(review["settlement"][0]["entries"].as_array().unwrap().contains(
        &json!({"id":undone["id"],"kind":"receipt","amount_cents":"4000","date":"2026-11-05","reversed":true})));

    // `false` is the same as leaving the key out: the receipt is recorded beside the change.
    native_receipt(&mut f, &customer, &invoice, "5.00");
    let recorded = call(&mut f.app, &token, "command_RecordCustomerReceipt", record_arguments(&values, &review, Some(false)));
    assert_eq!(recorded["data"]["amount"], "10.00");
    assert_eq!(receipts_and_outstanding(&mut f, operator, &invoice), (5, "150.00".to_owned()));
    // Anything but a boolean is refused before the books are touched.
    let review = review_with_settlement(&mut f, &token, &values);
    let before = whole_book(&f);
    let mut arguments = record_arguments(&values, &review, None);
    arguments["require_same_settlement"] = json!("yes");
    bad(&mut f.app, &token, "command_RecordCustomerReceipt", arguments);
    assert!(whole_book(&f) == before);

    // The key is honoured only for a review that asked. One that did not carries no settlement,
    // so with nothing changed at all the key still refuses it, and the attempt spends it.
    let values = receipt_values(&f, &customer, &invoice, "10.00");
    let plain = review_receipt(&mut f, &token, &values);
    refused_and_nothing_recorded(&mut f, &token, &values, &plain, "the review did not ask for the settlement");
    bad(&mut f.app, &token, "command_RecordCustomerReceipt", record_arguments(&values, &plain, None));
    assert_eq!(receipts_and_outstanding(&mut f, operator, &invoice), (5, "150.00".to_owned()));

    // A read-only approval can neither review nor record, with or without the keys, as before.
    let review = review_with_settlement(&mut f, &token, &values);
    let before = whole_book(&f);
    assert!(!f.app.ai_catalogue(&reader).unwrap()["operations"].as_array().unwrap().iter()
        .any(|op| op["name"].as_str().unwrap().ends_with("CustomerReceipt")));
    for arguments in [record_arguments(&values, &review, Some(true)), record_arguments(&values, &review, None)] {
        let error = f.app.ai_call(&reader, &json!({"operation":"command_RecordCustomerReceipt","arguments":arguments}).to_string()).unwrap_err();
        assert_eq!(error.to_string(), "Agent connection is unavailable or unauthorized.");
    }
    bad(&mut f.app, &reader, "command_ReviewCustomerReceipt", json!({"sales_receipt":false,"values":values}));
    bad(&mut f.app, &reader, "command_ReviewCustomerReceipt", json!({"sales_receipt":false,"values":values,"include_settlement":true}));
    assert!(whole_book(&f) == before);
    // The writer's review was not spent by the reader's attempts.
    call(&mut f.app, &token, "command_RecordCustomerReceipt", record_arguments(&values, &review, Some(true)));
    assert_eq!(receipts_and_outstanding(&mut f, operator, &invoice), (6, "140.00".to_owned()));
}

// The same guard with locked access: nobody signed in, after a restart.
#[test]
fn a_locked_access_connection_records_a_receipt_only_beside_the_settlement_it_reviewed() {
    let mut f = Fixture::new();
    let root = f.path.parent().unwrap().to_path_buf();
    f.app.backups.configure(&root).unwrap();
    let approver = f.app.authenticated_session().unwrap().actor;
    let (customer, invoice) = customer_with_invoice(&mut f);
    let (id, proof) = unattended(&mut f, Permission::AccountingWrite);
    let (_, reader_proof) = unattended(&mut f, Permission::ReadOnly);
    f.app.lock();
    f.app.open_book(&f.path, &keys::MasterKey::new([42; 32])).unwrap();
    assert!(f.app.authenticated_session().is_err());
    let worker = f.app.ai_prepare_unattended(&proof).unwrap();
    let reader = f.app.ai_prepare_unattended(&reader_proof).unwrap();

    let values = receipt_values(&f, &customer, &invoice, "75.00");
    assert_eq!(names(&review_receipt(&mut f, &worker, &values)), ["review", "token"]);
    let review = review_with_settlement(&mut f, &worker, &values);
    assert_eq!(names(&review), ["review", "token", "settlement"]);
    assert_eq!(review["settlement"], json!([{"invoice_id":invoice,"total_cents":"20000","outstanding_cents":"20000","entries":[]}]));
    let native = native_receipt_by(&mut f, approver, &customer, &invoice, "75.00");
    refused_and_nothing_recorded(&mut f, &worker, &values, &review, "a receipt was added");
    assert_eq!(receipts_and_outstanding(&mut f, approver, &invoice), (1, "125.00".to_owned()));

    // A read-only locked approval cannot ask for a review, with or without the settlement, and
    // cannot record, with or without the key. It is refused before anything is read or written.
    let review = review_with_settlement(&mut f, &worker, &values);
    assert_eq!(review["settlement"][0]["entries"], json!([{"id":native["id"],"kind":"receipt","amount_cents":"7500","date":"2026-11-05","reversed":false}]));
    let before = whole_book(&f);
    assert!(!f.app.ai_catalogue(&reader).unwrap()["operations"].as_array().unwrap().iter()
        .any(|op| op["name"].as_str().unwrap().ends_with("CustomerReceipt")));
    for arguments in [json!({"sales_receipt":false,"values":values}), json!({"sales_receipt":false,"values":values,"include_settlement":true})] {
        let (steps, error) = vm_steps(&mut f, |f| f.app.ai_call(&reader, &json!({"operation":"command_ReviewCustomerReceipt","arguments":arguments}).to_string()).unwrap_err());
        assert_eq!(error.to_string(), "Agent connection is unavailable or unauthorized.");
        let (catalogue_steps, _) = vm_steps(&mut f, |f| f.app.ai_catalogue(&reader).unwrap());
        assert_eq!(steps, catalogue_steps, "refused on the approval's permission alone, with no read of the invoice");
    }
    for arguments in [record_arguments(&values, &review, Some(true)), record_arguments(&values, &review, None)] {
        let error = f.app.ai_call(&reader, &json!({"operation":"command_RecordCustomerReceipt","arguments":arguments}).to_string()).unwrap_err();
        assert_eq!(error.to_string(), "Agent connection is unavailable or unauthorized.");
    }
    assert!(whole_book(&f) == before);

    // With nothing changed since its review the worker records it, as the approver, with provenance.
    let events = book_rows(&mut f, "SELECT id FROM ledger_auditevent");
    let recorded = call(&mut f.app, &worker, "command_RecordCustomerReceipt", record_arguments(&values, &review, Some(true)));
    assert_eq!(recorded["data"]["amount"], "75.00");
    assert_eq!(receipts_and_outstanding(&mut f, approver, &invoice), (2, "50.00".to_owned()));
    assert!(f.app.authenticated_session().is_err(), "the call signed nobody in");
    let since = format!("id NOT IN ({})", events.iter().map(|id| format!("'{id}'")).collect::<Vec<_>>().join(","));
    assert_eq!(book_rows(&mut f, &format!("SELECT DISTINCT actor_id FROM ledger_auditevent WHERE {since}")), [approver.to_db()]);
    let others = book_rows(&mut f, &format!("SELECT action FROM ledger_auditevent WHERE action<>'agent.provenance' AND {since}"));
    assert_eq!(others.iter().filter(|action| *action == "receipt.recorded").count(), 1, "{others:?}");
    let provenance: Vec<Value> = book_rows(&mut f, &format!("SELECT detail FROM ledger_auditevent WHERE action='agent.provenance' AND {since}"))
        .iter().map(|detail| serde_json::from_str(detail).unwrap()).collect();
    assert_eq!(provenance.len(), others.len(), "one provenance event for each event of the call");
    for event in &provenance {
        assert_eq!((&event["connection_id"], &event["operation"], &event["unattended"]),
            (&json!(id), &json!("command_RecordCustomerReceipt"), &json!(true)), "{event}");
    }

    // A reversal entered after the next review stops the next receipt.
    let values = receipt_values(&f, &customer, &invoice, "50.00");
    let review = review_with_settlement(&mut f, &worker, &values);
    native_reversal_by(&mut f, approver, &native);
    refused_and_nothing_recorded(&mut f, &worker, &values, &review, "a receipt was reversed");
    assert_eq!(receipts_and_outstanding(&mut f, approver, &invoice), (2, "125.00".to_owned()));
    // A review that did not ask cannot satisfy the key here either.
    let plain = review_receipt(&mut f, &worker, &values);
    refused_and_nothing_recorded(&mut f, &worker, &values, &plain, "the review did not ask for the settlement");
}

// Review finding 3. A review made with `include_settlement` belongs to the approval that made it:
// no other approval of the same signed-in session, and nobody at the desk, can record with its
// token, with the key or without, and their attempts leave it for its owner. A review that did
// not ask stays the session's, as it always was.
#[test]
fn a_review_with_a_settlement_can_be_recorded_only_by_the_approval_that_made_it() {
    let mut f = Fixture::new();
    let root = f.path.parent().unwrap().to_path_buf();
    f.app.backups.configure(&root).unwrap();
    let operator = f.app.authenticated_session().unwrap().actor;
    let (customer, invoice) = customer_with_invoice(&mut f);
    let first = f.app.ai_grant(f.company, "Synthetic first agent", Permission::AccountingWrite).unwrap();
    let (first_id, first) = (first["id"].as_str().unwrap().to_owned(), first["token"].as_str().unwrap().to_owned());
    let second = f.app.ai_grant(f.company, "Synthetic second agent", Permission::AccountingWrite).unwrap();
    let second = second["token"].as_str().unwrap().to_owned();

    // Two write approvals of one signed-in session. The first reviews, asking for the settlement.
    let values = receipt_values(&f, &customer, &invoice, "75.00");
    let review = review_with_settlement(&mut f, &first, &values);
    no_such_review(&mut f, &second, &values, &review, "another approval of the same session");
    no_such_review_at_the_desk(&mut f, &values, &review, "the person at the desk");
    // Left for its owner by all four attempts: the first approval records with it.
    let recorded = call(&mut f.app, &first, "command_RecordCustomerReceipt", record_arguments(&values, &review, Some(true)));
    assert_eq!((&recorded["outcome"], &recorded["data"]["amount"]), (&json!("done"), &json!("75.00")));
    assert_eq!(receipts_and_outstanding(&mut f, operator, &invoice), (1, "125.00".to_owned()));
    // And without the key its owner alone can record with such a review too.
    let values = receipt_values(&f, &customer, &invoice, "5.00");
    let review = review_with_settlement(&mut f, &first, &values);
    no_such_review(&mut f, &second, &values, &review, "another approval, before an unkeyed recording");
    assert_eq!(call(&mut f.app, &first, "command_RecordCustomerReceipt", record_arguments(&values, &review, None))["data"]["amount"], "5.00");
    assert_eq!(receipts_and_outstanding(&mut f, operator, &invoice), (2, "120.00".to_owned()));

    // A review that did not ask is the session's, exactly as before: the second approval records
    // with the first one's token.
    let values = receipt_values(&f, &customer, &invoice, "10.00");
    let plain = review_receipt(&mut f, &first, &values);
    assert_eq!(call(&mut f.app, &second, "command_RecordCustomerReceipt", record_arguments(&values, &plain, None))["data"]["amount"], "10.00");
    assert_eq!(receipts_and_outstanding(&mut f, operator, &invoice), (3, "110.00".to_owned()));
    // With the key such a review is refused whoever presents it, and the attempt spends it as any
    // attempt always has.
    let values = receipt_values(&f, &customer, &invoice, "10.00");
    let plain = review_receipt(&mut f, &first, &values);
    refused_and_nothing_recorded(&mut f, &second, &values, &plain, "another approval, a review that did not ask");
    bad(&mut f.app, &first, "command_RecordCustomerReceipt", record_arguments(&values, &plain, None));

    // Outside an approval there is nobody to keep such a review for: the desk cannot ask.
    let before = whole_book(&f);
    let ask = json!({"type":"ReviewCustomerReceipt","company_id":f.company.to_db(),"sales_receipt":false,"values":values,"include_settlement":true});
    let error = f.app.run(fb_app::commands::parse_command(&ask.to_string()).unwrap()).unwrap_err();
    assert_eq!(error.to_string(), "Only an AI connection can ask for the settlement.");
    assert!(whole_book(&f) == before);

    // A forgotten approval takes its reviews with it.
    let review = review_with_settlement(&mut f, &first, &values);
    f.app.ai_revoke(&first_id);
    assert!(f.app.ai_call(&first, &json!({"operation":"command_RecordCustomerReceipt","arguments":record_arguments(&values, &review, Some(true))}).to_string()).is_err());
    no_such_review(&mut f, &second, &values, &review, "after the approval that made it was forgotten");
    no_such_review_at_the_desk(&mut f, &values, &review, "the desk, after the approval was forgotten");
    assert_eq!(receipts_and_outstanding(&mut f, operator, &invoice), (3, "110.00".to_owned()));

    // Locked access: two approvals with nobody signed in, then a person signs in beside them.
    let (_, proof_a) = unattended(&mut f, Permission::AccountingWrite);
    let (_, proof_b) = unattended(&mut f, Permission::AccountingWrite);
    f.app.sign_out();
    let worker_a = f.app.ai_prepare_unattended(&proof_a).unwrap();
    let worker_b = f.app.ai_prepare_unattended(&proof_b).unwrap();
    let values = receipt_values(&f, &customer, &invoice, "30.00");
    let review = review_with_settlement(&mut f, &worker_a, &values);
    // The first write of the other worker after the sign-out, so the comparison below starts clean.
    review_receipt(&mut f, &worker_b, &values);
    no_such_review(&mut f, &worker_b, &values, &review, "another locked-access approval");
    f.app.sign_in("Synthetic operator", ai_support::PASSWORD).unwrap();
    f.app.ai_select_company(f.company).unwrap();
    let attended = f.app.ai_grant(f.company, "Synthetic third agent", Permission::AccountingWrite).unwrap();
    let attended = attended["token"].as_str().unwrap().to_owned();
    no_such_review(&mut f, &attended, &values, &review, "a signed-in approval, a locked-access review");
    no_such_review_at_the_desk(&mut f, &values, &review, "the desk, a locked-access review");
    // And the other way: a signed-in approval's review is not the worker's.
    let attended_review = review_with_settlement(&mut f, &attended, &values);
    no_such_review(&mut f, &worker_a, &values, &attended_review, "a locked-access approval, a signed-in review");
    f.app.sign_out();
    // Its owner records with it after all of that.
    let recorded = call(&mut f.app, &worker_a, "command_RecordCustomerReceipt", record_arguments(&values, &review, Some(true)));
    assert_eq!(recorded["data"]["amount"], "30.00");
    assert_eq!(receipts_and_outstanding(&mut f, operator, &invoice), (4, "80.00".to_owned()));
}

// A review token lives in memory with the session or the approval, so a lock ends it: after the
// books are opened again no token made before the lock records anything, asked or not, with the
// key or without.
#[test]
fn a_review_token_made_before_a_lock_records_nothing_after_it() {
    let mut f = Fixture::new();
    let root = f.path.parent().unwrap().to_path_buf();
    f.app.backups.configure(&root).unwrap();
    let approver = f.app.authenticated_session().unwrap().actor;
    let (customer, invoice) = customer_with_invoice(&mut f);
    let grant = f.app.ai_grant(f.company, "Synthetic agent", Permission::AccountingWrite).unwrap();
    let (grant_id, token) = (grant["id"].as_str().unwrap().to_owned(), grant["token"].as_str().unwrap().to_owned());
    let (_, proof) = unattended(&mut f, Permission::AccountingWrite);
    let worker = f.app.ai_prepare_unattended(&proof).unwrap();
    let values = receipt_values(&f, &customer, &invoice, "75.00");
    let (asked, plain) = (review_with_settlement(&mut f, &token, &values), review_receipt(&mut f, &token, &values));
    let (worker_asked, worker_plain) = (review_with_settlement(&mut f, &worker, &values), review_receipt(&mut f, &worker, &values));

    f.app.lock();
    f.app.open_book(&f.path, &keys::MasterKey::new([42; 32])).unwrap();
    // Nobody is signed in. The locked-access approval resumes with the same credential, and the
    // reviews it made before the lock are gone.
    assert_eq!(f.app.ai_prepare_unattended(&proof).unwrap(), worker);
    review_receipt(&mut f, &worker, &values);
    no_such_review(&mut f, &worker, &values, &worker_asked, "locked access, a review that asked");
    no_such_review(&mut f, &worker, &values, &worker_plain, "locked access, a review that did not ask");
    // The signed-in approval's credential ended with the lock.
    let record = json!({"operation":"command_RecordCustomerReceipt","arguments":record_arguments(&values, &asked, Some(true))}).to_string();
    assert!(f.app.ai_call(&token, &record).is_err());
    // The same person signs in and renews the same approval: a new credential, and none of the old
    // review tokens.
    f.app.sign_in("Synthetic operator", ai_support::PASSWORD).unwrap();
    f.app.ai_select_company(f.company).unwrap();
    let renewed = f.app.ai_grant_remembered(f.company, "Synthetic agent", Permission::AccountingWrite, Some(&grant_id)).unwrap();
    assert_eq!(renewed["id"], grant_id.as_str());
    let renewed = renewed["token"].as_str().unwrap().to_owned();
    assert!(f.app.ai_call(&token, &record).is_err());
    no_such_review(&mut f, &renewed, &values, &asked, "signed in, a review that asked");
    no_such_review(&mut f, &renewed, &values, &plain, "signed in, a review that did not ask");
    no_such_review(&mut f, &renewed, &values, &worker_asked, "signed in, the locked-access approval's review");
    no_such_review_at_the_desk(&mut f, &values, &asked, "the desk");
    assert_eq!(receipts_and_outstanding(&mut f, approver, &invoice), (0, "200.00".to_owned()));
    // A review made after the lock records as usual.
    let review = review_with_settlement(&mut f, &renewed, &values);
    assert_eq!(call(&mut f.app, &renewed, "command_RecordCustomerReceipt", record_arguments(&values, &review, Some(true)))["data"]["amount"], "75.00");
    assert_eq!(receipts_and_outstanding(&mut f, approver, &invoice), (1, "125.00".to_owned()));
}

// Review finding 4: what a retry can and cannot do. The review token is spent by the first
// attempt, so after an answer is lost the caller cannot simply send the recording again.
#[test]
fn after_a_lost_answer_a_retry_returns_the_original_receipt_or_nothing_at_all() {
    let mut f = Fixture::new();
    let operator = f.app.authenticated_session().unwrap().actor;
    let (customer, invoice) = customer_with_invoice(&mut f);
    let grant = f.app.ai_grant(f.company, "Synthetic agent", Permission::AccountingWrite).unwrap();
    let token = grant["token"].as_str().unwrap().to_owned();
    let entry = |receipt: &Value, amount: &str, reversed: bool| json!({"id":receipt["data"]["id"],"kind":"receipt","amount_cents":amount,"date":"2026-11-05","reversed":reversed});

    // 1. A lost answer, with money still outstanding. The 75.00 receipt is recorded; the caller
    //    never sees `first`.
    let values = receipt_values(&f, &customer, &invoice, "75.00");
    let review = review_with_settlement(&mut f, &token, &values);
    let first = call(&mut f.app, &token, "command_RecordCustomerReceipt", record_arguments(&values, &review, Some(true)));
    assert_eq!(receipts_and_outstanding(&mut f, operator, &invoice), (1, "125.00".to_owned()));
    // The old token records nothing and returns nothing.
    no_such_review(&mut f, &token, &values, &review, "the token of the lost answer");
    // A fresh review of the same request can still be made, because 125.00 is outstanding. A
    // recording with it matches the recorded request and returns the original receipt, writing
    // nothing, with the key although the settlement now holds that very receipt, and without it.
    let before = whole_book(&f);
    let again = review_with_settlement(&mut f, &token, &values);
    assert_eq!(again["settlement"], json!([{"invoice_id":invoice,"total_cents":"20000","outstanding_cents":"12500","entries":[entry(&first, "7500", false)]}]));
    assert_eq!(call(&mut f.app, &token, "command_RecordCustomerReceipt", record_arguments(&values, &again, Some(true))), first);
    let again = review_receipt(&mut f, &token, &values);
    assert_eq!(call(&mut f.app, &token, "command_RecordCustomerReceipt", record_arguments(&values, &again, None)), first);
    assert!(whole_book(&f) == before, "the repeats wrote nothing");
    // The same request key with other values is not a repeat.
    let mut other = values.clone();
    other["reference"] = json!("Check 5522");
    let again = review_with_settlement(&mut f, &token, &other);
    assert_eq!(answer(&mut f, &token, "command_RecordCustomerReceipt", record_arguments(&other, &again, Some(true))),
        refusal("This request key was recorded with a different payload or review."));
    assert!(whole_book(&f) == before);

    // 2. A lost answer on an invoice the receipt paid off. 125.00 is left and is received.
    let values = receipt_values(&f, &customer, &invoice, "125.00");
    let review = review_with_settlement(&mut f, &token, &values);
    let paid = call(&mut f.app, &token, "command_RecordCustomerReceipt", record_arguments(&values, &review, Some(true)));
    let before = whole_book(&f);
    no_such_review(&mut f, &token, &values, &review, "the token of the lost answer, invoice paid");
    // No fresh review can be made: nothing is outstanding, so the review is refused and there is
    // no token to repeat the request with.
    for arguments in [json!({"sales_receipt":false,"values":values,"include_settlement":true}), json!({"sales_receipt":false,"values":values})] {
        assert_eq!(answer(&mut f, &token, "command_ReviewCustomerReceipt", arguments),
            refusal("Allocation exceeds the actual outstanding invoice amount. Review again."));
    }
    // What happened is learned by reading the invoice.
    let invoices = call(&mut f.app, &token, "query_Invoices", json!({}));
    let row = invoices.as_array().unwrap().iter().find(|row| row["invoice_id"] == invoice.as_str()).unwrap();
    assert_eq!((&row["outstanding"], &row["allocated"], &row["payment_state"]), (&json!("0.00"), &json!("200.00"), &json!("PAID")));
    assert!(whole_book(&f) == before);

    // 3. A reversed receipt. The 125.00 receipt is reversed at the desk, so the same request can
    //    be reviewed again. The repeat returns the receipt as it was recorded, which does not say
    //    that it has been reversed, and writes nothing: the money is not recorded a second time.
    native_reversal(&mut f, &paid["data"]);
    let before = whole_book(&f);
    let again = review_with_settlement(&mut f, &token, &values);
    assert_eq!(again["settlement"], json!([{"invoice_id":invoice,"total_cents":"20000","outstanding_cents":"12500",
        "entries":[entry(&first, "7500", false), entry(&paid, "12500", true)]}]));
    let repeat = call(&mut f.app, &token, "command_RecordCustomerReceipt", record_arguments(&values, &again, Some(true)));
    assert_eq!(repeat, paid);
    assert!(!repeat.to_string().contains("revers"), "the receipt record says nothing of the reversal: {repeat}");
    assert!(whole_book(&f) == before);
    assert_eq!(receipts_and_outstanding(&mut f, operator, &invoice), (2, "125.00".to_owned()));
    // Recording that money again takes a new request key.
    let values = receipt_values(&f, &customer, &invoice, "125.00");
    let review = review_with_settlement(&mut f, &token, &values);
    assert_eq!(call(&mut f.app, &token, "command_RecordCustomerReceipt", record_arguments(&values, &review, Some(true)))["data"]["amount"], "125.00");
    assert_eq!(receipts_and_outstanding(&mut f, operator, &invoice), (3, "0.00".to_owned()));
}

/// The local API's limit on a response, read from the transport that enforces it.
fn transport_response_limit() -> usize {
    let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../electron/main/ai-transport.cjs")).unwrap();
    let limit = source.split("Buffer.byteLength(body) > ").nth(1).and_then(|rest| rest.split(')').next())
        .expect("the transport's response limit");
    limit.split('*').map(|factor| factor.trim().parse::<usize>().expect("a product of whole numbers")).product()
}

// Review finding 1 (the blocker). A settlement is listed up to 500 entries, so the largest review
// a connection can be sent is far under the transport's response limit; above 500 the review is
// still made, without the settlement, and the key refuses it.
#[test]
fn a_large_settlement_is_capped_and_the_review_stays_under_the_transport_limit() {
    let limit = transport_response_limit();
    assert_eq!(limit, 4 * 1024 * 1024);
    let mut f = Fixture::new();
    let operator = f.app.authenticated_session().unwrap().actor;
    let company = f.company.to_db();
    let customer = ai_support::run(&mut f.app, json!({"type":"SaveCustomer","company_id":company,"display_name":"Synthetic Orchard"}))["id"]
        .as_str().unwrap().to_owned();
    // The most invoices one receipt can be allocated to, each with a share of ten receipts.
    let invoices: Vec<String> = (0..50).map(|_| posted_invoice(&mut f, &customer, "100.00")).collect();
    let across = |f: &Fixture| {
        let mut values = receipt_values(f, &customer, &invoices[0], "50.00");
        values["allocations"] = invoices.iter().map(|invoice| json!({"invoice_id":invoice,"amount":"1.00"})).collect();
        values
    };
    for _ in 0..10 {
        let values = across(&f);
        let book = &mut f.app.open.as_mut().unwrap().book;
        let review = fb_core::sales::review_receipt(book, operator, f.company, &values).unwrap();
        fb_core::sales::record_receipt(book, operator, f.company, &values, Some(&review)).unwrap();
    }
    let grant = f.app.ai_grant(f.company, "Synthetic agent", Permission::AccountingWrite).unwrap();
    let token = grant["token"].as_str().unwrap().to_owned();
    let ask = |values: &Value| json!({"sales_receipt":false,"values":values,"include_settlement":true});

    // 500 entries, the most a review lists: returned whole, in a response far under the limit.
    let values = across(&f);
    let largest = answer(&mut f, &token, "command_ReviewCustomerReceipt", ask(&values));
    let data = &largest["data"];
    assert_eq!(names(data), ["review", "token", "settlement"]);
    let listed = data["settlement"].as_array().unwrap();
    assert_eq!((listed.len(), listed.iter().map(|invoice| invoice["entries"].as_array().unwrap().len()).sum::<usize>()), (50, 500));
    let (response, settlement) = (largest.to_string().len(), data["settlement"].to_string().len());
    println!("SIZE the largest review is {response} bytes, {settlement} of them the settlement; the transport allows {limit}");
    assert!(response < limit / 16, "the largest review a connection can receive is {response} bytes; the transport allows {limit}");
    // Without the limit the size grows with the history: the reviewed case, 50 invoices with 800
    // entries each, would be eighty times this settlement and over the transport's limit.
    assert!(settlement * 80 > limit, "{settlement} bytes for 500 entries");
    // An entry of the longest form (a deposit of the largest amount) is 133 bytes and an invoice's
    // own fields 138, each with a comma, so no 500 entries come near the limit whatever the amounts.
    assert!(response - settlement + 50 * 139 + 500 * 134 < limit / 16);
    // At the limit the key is honoured.
    let recorded = call(&mut f.app, &token, "command_RecordCustomerReceipt", record_arguments(&values, data, Some(true)));
    assert_eq!(recorded["data"]["amount"], "50.00");

    // 550 entries now. The review is made and returns its token; the settlement is not listed.
    let values = across(&f);
    let over = answer(&mut f, &token, "command_ReviewCustomerReceipt", ask(&values));
    let data = &over["data"];
    assert_eq!(names(data), ["review", "token", "settlement", "settlement_unavailable"]);
    assert_eq!((&data["settlement"], &data["settlement_unavailable"]), (&Value::Null, &json!("more than 500 entries")));
    assert!(data["token"].as_str().is_some_and(|token| !token.is_empty()));
    assert!(over.to_string().len() < response, "smaller than the largest listed review");
    // The key refuses it, and nothing is written.
    refused_and_nothing_recorded(&mut f, &token, &values, data, "more than 500 entries");
    // Its token is a whole review's all the same: made again, it records without the key.
    let data = answer(&mut f, &token, "command_ReviewCustomerReceipt", ask(&values))["data"].clone();
    assert_eq!(data["settlement_unavailable"], "more than 500 entries");
    assert_eq!(call(&mut f.app, &token, "command_RecordCustomerReceipt", record_arguments(&values, &data, None))["data"]["amount"], "50.00");
    // A review that does not ask is the two things it always was, whatever the history.
    let values = across(&f);
    assert_eq!(names(&review_receipt(&mut f, &token, &values)), ["review", "token"]);
    // A receipt on fewer invoices is under the limit again: one invoice has 12 entries.
    let values = receipt_values(&f, &customer, &invoices[0], "1.00");
    let review = review_with_settlement(&mut f, &token, &values);
    assert_eq!(review["settlement"][0]["entries"].as_array().unwrap().len(), 12);
    assert_eq!(call(&mut f.app, &token, "command_RecordCustomerReceipt", record_arguments(&values, &review, Some(true)))["data"]["amount"], "1.00");
}

// An unasked review does the reads it always did; asking adds the settlement's reads to it, and
// nothing is added to a recording without the key.
#[test]
fn only_a_review_that_asks_reads_the_settlement() {
    let mut f = Fixture::new();
    let (customer, invoice) = customer_with_invoice(&mut f);
    native_receipt(&mut f, &customer, &invoice, "20.00");
    native_credit(&mut f, &customer, &invoice, "10.00");
    let grant = f.app.ai_grant(f.company, "Synthetic agent", Permission::AccountingWrite).unwrap();
    let token = grant["token"].as_str().unwrap().to_owned();
    let values = receipt_values(&f, &customer, &invoice, "75.00");
    let (plain_steps, plain) = vm_steps(&mut f, |f| review_receipt(f, &token, &values));
    let (asked_steps, asked) = vm_steps(&mut f, |f| review_with_settlement(f, &token, &values));
    let (false_steps, _) = vm_steps(&mut f, |f| call(&mut f.app, &token, "command_ReviewCustomerReceipt", json!({"sales_receipt":false,"values":values,"include_settlement":false})));
    assert_eq!(plain_steps, false_steps);
    assert!(asked_steps > plain_steps, "{asked_steps} steps when asked, {plain_steps} when not");
    assert_eq!(asked["settlement"][0]["entries"].as_array().unwrap().len(), 2);
    assert_eq!(asked["review"], plain["review"]);
}

// Second review of the settlement guard (internal test notes): its five probes, kept as tests.
// The first pins a limit the documentation now states: after a lost refusal, the key only compares
// against the fresh review, so the caller must compare that review's settlement with the one it acted on.


// P1. docs/local-ai.md, "What a retry can and cannot do": after a lost answer, review again with the
// same request_key and values and record with the new token. If the lost answer was the guard's
// refusal, the fresh review already contains the change that caused it, so the guarded recording
// now succeeds beside it.
#[test]
fn after_a_lost_refusal_a_fresh_review_holds_the_change_and_the_key_no_longer_stops_the_record() {
    let mut f = Fixture::new();
    let operator = f.app.authenticated_session().unwrap().actor;
    let (customer, invoice) = customer_with_invoice(&mut f);
    let grant = f.app.ai_grant(f.company, "Synthetic agent", Permission::AccountingWrite).unwrap();
    let token = grant["token"].as_str().unwrap().to_owned();
    let values = receipt_values(&f, &customer, &invoice, "75.00");
    let shown = review_with_settlement(&mut f, &token, &values);
    assert_eq!(shown["settlement"][0]["entries"], json!([]));
    native_receipt(&mut f, &customer, &invoice, "75.00");
    // The guard refuses. Suppose this answer is lost (outcome_unknown).
    let lost = answer(&mut f, &token, "command_RecordCustomerReceipt", record_arguments(&values, &shown, Some(true)));
    assert_eq!(lost, refusal(RECEIPT_REFUSAL));
    // As documented: the old token tells nothing.
    assert_eq!(answer(&mut f, &token, "command_RecordCustomerReceipt", record_arguments(&values, &shown, Some(true))), refusal(NO_SUCH_REVIEW));
    // As documented: a fresh review with the same request_key and values, then a recording with its token.
    let fresh = review_with_settlement(&mut f, &token, &values);
    assert_eq!(fresh["settlement"][0]["entries"].as_array().unwrap().len(), 1, "the native receipt is in the fresh settlement");
    let retried = call(&mut f.app, &token, "command_RecordCustomerReceipt", record_arguments(&values, &fresh, Some(true)));
    assert_eq!(retried["data"]["amount"], "75.00");
    let state = receipts_and_outstanding(&mut f, operator, &invoice);
    println!("PROBE P1 after the documented retry: {state:?} (receipts, outstanding)");
    assert_eq!(state, (2, "50.00".to_owned()), "the same 75.00 is now in the books twice");
}

// P2. docs/local-ai.md says a recording attempt spends its token "whether the receipt is recorded,
// the recording is refused, or the answer never arrives". A refusal before the receipt command runs
// (bad arguments, a read-only approval) does not spend it.
#[test]
fn a_refusal_before_the_receipt_service_leaves_the_review_token_usable() {
    let mut f = Fixture::new();
    let (customer, invoice) = customer_with_invoice(&mut f);
    let grant = f.app.ai_grant(f.company, "Synthetic agent", Permission::AccountingWrite).unwrap();
    let token = grant["token"].as_str().unwrap().to_owned();
    let values = receipt_values(&f, &customer, &invoice, "75.00");
    let review = review_with_settlement(&mut f, &token, &values);
    let mut arguments = record_arguments(&values, &review, None);
    arguments["require_same_settlement"] = json!("yes");
    let refused = answer(&mut f, &token, "command_RecordCustomerReceipt", arguments);
    println!("PROBE P2 refused: {refused}");
    assert_eq!(refused["_agent_error"], true);
    let recorded = call(&mut f.app, &token, "command_RecordCustomerReceipt", record_arguments(&values, &review, Some(true)));
    assert_eq!(recorded["data"]["amount"], "75.00", "the token survived the refused attempt");
}

// P3. The cap counted across invoices when the first invoice uses all 500 and a later one has none
// (room 0, LIMIT 1): listed; one entry on the later invoice: over.
#[test]
fn the_settlement_cap_is_counted_across_invoices_when_the_first_uses_all_of_it() {
    let mut f = Fixture::new();
    let operator = f.app.authenticated_session().unwrap().actor;
    let (customer, invoice) = customer_with_invoice(&mut f);
    let empty = posted_invoice(&mut f, &customer, "50.00");
    // Order of the two invoices in the request is by id; make sure the full one sorts first or not,
    // either way the total is what counts.
    for _ in 0..500 {
        let values = receipt_values(&f, &customer, &invoice, "0.01");
        let book = &mut f.app.open.as_mut().unwrap().book;
        let review = fb_core::sales::review_receipt(book, operator, f.company, &values).unwrap();
        fb_core::sales::record_receipt(book, operator, f.company, &values, Some(&review)).unwrap();
    }
    let grant = f.app.ai_grant(f.company, "Synthetic agent", Permission::AccountingWrite).unwrap();
    let token = grant["token"].as_str().unwrap().to_owned();
    let mut values = receipt_values(&f, &customer, &invoice, "2.00");
    values["allocations"] = json!([{"invoice_id":invoice,"amount":"1.00"},{"invoice_id":empty,"amount":"1.00"}]);
    let listed = review_with_settlement(&mut f, &token, &values);
    let count: usize = listed["settlement"].as_array().unwrap().iter().map(|i| i["entries"].as_array().unwrap().len()).sum();
    println!("PROBE P3 at the cap: {} invoices, {count} entries, empty first: {}", listed["settlement"].as_array().unwrap().len(), empty < invoice);
    assert_eq!(count, 500);
    // One entry on the empty invoice: 501.
    let one = receipt_values(&f, &customer, &empty, "0.01");
    let book = &mut f.app.open.as_mut().unwrap().book;
    let review = fb_core::sales::review_receipt(book, operator, f.company, &one).unwrap();
    fb_core::sales::record_receipt(book, operator, f.company, &one, Some(&review)).unwrap();
    let over = review_with_settlement(&mut f, &token, &values);
    assert_eq!((&over["settlement"], &over["settlement_unavailable"]), (&Value::Null, &json!("more than 500 entries")));
    let before = whole_book(&f);
    assert_eq!(answer(&mut f, &token, "command_RecordCustomerReceipt", record_arguments(&values, &listed, Some(true))), refusal(RECEIPT_REFUSAL));
    assert!(whole_book(&f) == before);
}

// P4. A guarded review made by a signed-in approval is gone after a plain sign-out and sign-in
// (no lock), with the key and without; the desk cannot use it either.
#[test]
fn a_signed_in_connections_settlement_review_ends_with_sign_out() {
    let mut f = Fixture::new();
    let (customer, invoice) = customer_with_invoice(&mut f);
    let grant = f.app.ai_grant(f.company, "Synthetic agent", Permission::AccountingWrite).unwrap();
    let (id, token) = (grant["id"].as_str().unwrap().to_owned(), grant["token"].as_str().unwrap().to_owned());
    let values = receipt_values(&f, &customer, &invoice, "75.00");
    let review = review_with_settlement(&mut f, &token, &values);
    f.app.sign_out();
    f.app.sign_in("Synthetic operator", ai_support::PASSWORD).unwrap();
    f.app.ai_select_company(f.company).unwrap();
    assert!(f.app.ai_call(&token, &json!({"operation":"command_RecordCustomerReceipt","arguments":record_arguments(&values, &review, Some(true))}).to_string()).is_err());
    let renewed = f.app.ai_grant_remembered(f.company, "Synthetic agent", Permission::AccountingWrite, Some(&id)).unwrap();
    let renewed = renewed["token"].as_str().unwrap().to_owned();
    no_such_review(&mut f, &renewed, &values, &review, "same approval after sign-out");
    no_such_review_at_the_desk(&mut f, &values, &review, "desk after sign-out");
}

// P5. A guarded sales receipt through a connection: settlement [] and the key records it.
#[test]
fn a_sales_receipt_can_be_recorded_with_the_key_through_a_connection() {
    let mut f = Fixture::new();
    let (customer, _) = customer_with_invoice(&mut f);
    let grant = f.app.ai_grant(f.company, "Synthetic agent", Permission::AccountingWrite).unwrap();
    let token = grant["token"].as_str().unwrap().to_owned();
    let sale = json!({"request_key":uuid::Uuid::new_v4().to_string(),"customer_id":customer,"date":"2026-11-05","method":"CASH","reference":"","memo":"",
        "destination":"BANK","bank_id":f.bank,"tax_mode":"NONE","lines":[{"item_id":null,"description":"Synthetic counter sale","quantity":"1","rate":"12.00","taxable":false,"income_account_id":f.income}]});
    let review = call(&mut f.app, &token, "command_ReviewCustomerReceipt", json!({"sales_receipt":true,"values":sale,"include_settlement":true}))["data"].clone();
    assert_eq!(review["settlement"], json!([]));
    let recorded = call(&mut f.app, &token, "command_RecordCustomerReceipt", json!({"sales_receipt":true,"values":sale,"review_token":review["token"],"require_same_settlement":true}));
    assert_eq!(recorded["data"]["kind"], "SR");
}
