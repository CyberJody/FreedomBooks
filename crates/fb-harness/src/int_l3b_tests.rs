//! Prior released and integrated layouts, with complete row/key/report comparisons.
use super::*;

#[test]
fn populated_main_62edf22_and_int_l3a_copies_preserve_every_row_and_reopen() {
    // 0079_restore_sales_receipt_snapshot is main (pre-L4, 9ce9933): every L1-L3 module populated, upgraded through
    // 0100-0130 (assistant, attachments, backups owner, shared value guards) without changing a stored row.
    for (boundary, limit) in [("0009_l2a_review_fixes", 12), ("a2be4d8", 53), ("0078_shared_unique_json", 108), ("0079_restore_sales_receipt_snapshot", 108)] {
        let dir=tempfile::tempdir().unwrap();
        let mut scenario:Value=serde_json::from_str(include_str!("../scenarios/int-l3-combined.json")).unwrap();
        scenario["steps"].as_array_mut().unwrap().truncate(limit);
        scenario["capture_steps"]=json!(false);
        if limit==12 {
            scenario["steps"].as_array_mut().unwrap().extend([
                json!({"op":"save_draft","company":"c","ref":"opening","date":"2026-10-01","memo":"Synthetic opening","lines":[{"account":"bank","debit":"12.34","credit":"0"},{"account":"income","debit":"0","credit":"12.34"}]}),
                json!({"op":"post_journal","company":"c","journal":"opening","expected_revision":1}),
            ]);
        }
        let mut runner=Runner::new(&scenario,&dir.path().join("source.fbbook"));
        runner.run();
        assert!(runner.outcomes.iter().zip(scenario["steps"].as_array().unwrap()).all(|(r,s)| r["ok"].as_bool()==Some(s["expect_error"]!=true)),"{:?}",runner.outcomes);
        let dump=runner.dump();
        let path=dir.path().join("prior.fbbook");
        let copy=fb_db::test_support::populated_copy(runner.book.connection(),&path,&[42;32],boundary,true).unwrap();
        assert!(!copy.before["ledger_company"].is_empty());
        assert!(!copy.before["ledger_journal"].is_empty());
        assert!(!copy.before["ledger_auditevent"].is_empty());
        for _ in 0..2 {
            runner.book=backups::SelectedBook::open(&path,&[42;32]).unwrap();
            assert_eq!(fb_db::test_support::snapshot(runner.book.connection()).unwrap(),copy.expected);
            assert!(runner.book.check().unwrap().is_empty());
            assert_eq!(runner.dump(),dump);
        }
    }
}

#[test]
fn credit_refund_collisions_with_all_other_families_fail_review_check_and_open() {
    let original:Value=serde_json::from_str(include_str!("../scenarios/int-l3-combined.json")).unwrap();
    for (number,holder) in [("1001","bill payment"),("1002","check"),("1003","deposit refund")] {
        let mut scenario=original.clone();
        scenario["capture_steps"]=json!(false);
        let source=original["steps"].as_array().unwrap();
        let end=source.iter().position(|s|s["op"]=="pp_record_refund").unwrap()+1;
        let mut steps=source[..end].to_vec();
        steps.extend(source.iter().filter(|s|s["op"]=="credits_review_memo" || s["op"]=="credits_record_memo" || (s["review"]=="credit-refund" && (s["op"]=="credits_review_refund" || s["op"]=="credits_refund"))).cloned());
        let mut refusal=source.iter().find(|s|s["op"]=="credits_review_refund" && s["review"]=="credit-refund").unwrap().clone();
        refusal["values"]["request_key"]=json!(fb_ids::new_v7().to_string());
        refusal["values"]["amount"]=json!("0.01");refusal["values"]["check_number"]=json!(number);
        refusal["expect_error"]=json!(true);steps.push(refusal);
        scenario["steps"]=json!(steps);
        let dir=tempfile::tempdir().unwrap();let path=dir.path().join("collisions.fbbook");
        let key = [42; 32];
        let mut runner=Runner::with_key(&scenario,&path,&key);runner.run();
        // A clean same-key control must succeed before testing damaged evidence.
        let clean = Book::open(&path, &key).expect("clean duplicate-number fixture reopens");
        assert_eq!(fb_db::test_support::snapshot(clean.connection()).unwrap(),fb_db::test_support::snapshot(runner.book.connection()).unwrap());
        drop(clean);
        assert!(runner.outcomes.last().unwrap()["message"].as_str().unwrap().contains(&format!("already used by {holder}")));
        assert!(runner.book.check().unwrap().is_empty());
        let q=runner.book.connection();
        let definitions:Vec<(String,String)>=q.prepare("SELECT name,sql FROM sqlite_master WHERE type='trigger'").unwrap().query_map([],|r|Ok((r.get(0)?,r.get(1)?))).unwrap().collect::<Result<_,_>>().unwrap();
        for (name,_) in &definitions {q.execute_batch(&format!("DROP TRIGGER {name}")).unwrap();}
        q.execute("UPDATE credits_creditrefund SET check_number=?1,data=json_set(data,'$.check_number',?1)",[number]).unwrap();
        for (_,sql) in definitions {q.execute_batch(&sql).unwrap();}
        assert!(runner.book.check().unwrap().iter().any(|p|p.contains("duplicate live shared check number")));
        drop(runner);
        match Book::open(&path, &key) {
            Err(fb_db::DbError::Integrity(problems)) => assert!(problems.iter().any(|p| p.contains("duplicate live shared check number")), "{problems:?}"),
            Err(other) => panic!("expected duplicate-number integrity refusal, got {other}"),
            Ok(_) => panic!("duplicate number reopened"),
        }
    }
}


#[test]
fn receipt_insert_immediately_refuses_another_company_snapshot() {
    use fb_db::rusqlite::{params_from_iter, types::Value as SqlValue};
    let mut scenario: Value = serde_json::from_str(include_str!("../scenarios/int-l3-combined.json")).unwrap();
    let end = scenario["steps"].as_array().unwrap().iter().position(|s| s["op"] == "sales_record_receipt").unwrap() + 1;
    scenario["steps"].as_array_mut().unwrap().truncate(end);
    scenario["capture_steps"] = json!(false);
    let dir = tempfile::tempdir().unwrap();
    let mut runner = Runner::new(&scenario, &dir.path().join("receipt.fbbook"));
    runner.run();
    assert!(runner.outcomes.iter().all(|o| o["ok"] == true || scenario["steps"][o["step"].as_u64().unwrap() as usize]["expect_error"] == true));
    let before = fb_db::test_support::snapshot(runner.book.connection()).unwrap();
    let q = runner.book.connection();
    let mut statement = q.prepare("SELECT * FROM sales_receipt WHERE kind='RCP' LIMIT 1").unwrap();
    let columns = statement.column_count();
    let index = statement.column_names().position(|n| n == "data").unwrap();
    let mut row: Vec<SqlValue> = statement.query_row([], |r| (0..columns).map(|i| r.get(i)).collect()).unwrap();
    drop(statement);
    let SqlValue::Text(raw) = &row[index] else { panic!("receipt data") };
    let mut data: Value = serde_json::from_str(raw).unwrap();
    let id = data["id"].as_str().unwrap().to_owned();
    data["company_snapshot"]["id"] = json!(fb_ids::new_v7().to_string());
    row[index] = SqlValue::Text(data.to_string());
    let definitions: Vec<(String, String)> = q.prepare("SELECT name,sql FROM sqlite_master WHERE type='trigger'").unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap();
    for recursive in [false, true] {
        q.execute_batch(&format!("PRAGMA recursive_triggers={}; BEGIN IMMEDIATE; PRAGMA defer_foreign_keys=ON", i32::from(recursive))).unwrap();
        for (name, _) in &definitions { q.execute_batch(&format!("DROP TRIGGER {name}")).unwrap(); }
        q.execute("DELETE FROM sales_seal_due WHERE kind='sales_receipt' AND doc_id=?", [&id]).unwrap();
        q.execute("DELETE FROM sales_seal WHERE kind='sales_receipt' AND doc_id=?", [&id]).unwrap();
        q.execute("DELETE FROM sales_receipt WHERE id=?", [&id]).unwrap();
        for (_, sql) in &definitions { q.execute_batch(sql).unwrap(); }
        let error = q.execute(&format!("INSERT INTO sales_receipt VALUES({})", vec!["?"; columns].join(",")), params_from_iter(&row)).unwrap_err();
        assert_eq!(fb_db::constraint_of(&error).as_deref(), Some("receipt_fields"), "{error}");
        q.execute_batch("ROLLBACK; PRAGMA recursive_triggers=ON").unwrap();
        assert_eq!(fb_db::test_support::snapshot(q).unwrap(), before);
    }
    assert!(runner.book.check().unwrap().is_empty());
}

#[test]
fn omitted_expectation_means_success_and_explicit_refusal_checks_reason() {
    for capture in [false, true] {
        for expectation in [
            json!({}),
            json!({"expect_error":false}),
            json!({"expect_error":true,"expect_kind":"DoesNotExist"}),
            json!({"expect_error":true,"expect_kind":"ValidationError","expect_message":"the intended reason"}),
        ] {
            let mut scenario = json!({"users":[{"username":"expectation tester"}], "steps":[{"op":"create_company","company":"c","name":""}],"capture_steps":capture});
            for (key, value) in expectation.as_object().unwrap() { scenario["steps"][0][key] = value.clone(); }
            let dir = tempfile::tempdir().unwrap();
            let mut runner = Runner::new(&scenario, &dir.path().join("expectations.fbbook"));
            runner.run();
            assert_eq!(runner.outcomes[0]["ok"], false);
            assert_eq!(runner.outcomes[0]["unexpected"], true, "{:?}", runner.outcomes);
            let dump = runner.dump();
            assert!(!compare(&dump, &dump).ok());
        }
    }
}

#[test]
fn combined_sale_void_steps_pin_the_banking_refusals_and_final_success() {
    let scenario: Value = serde_json::from_str(include_str!("../scenarios/int-l3-combined.json")).unwrap();
    for (index, message) in [
        (75, Some("Reverse deposit DEP-000001 before reversing this receipt.")),
        (97, Some("SR-000001 was released from deposit DEP-000001 on 2026-11-01; the receipt reversal cannot be dated earlier.")),
        (98, None),
    ] {
        let step = &scenario["steps"][index];
        assert_eq!(step["op"], "sales_void_sale");
        assert_eq!(step["record"], "undeposited-sale");
        assert_eq!(step["expect_error"], message.is_some());
        if let Some(message) = message {
            assert_eq!(step["expect_kind"], "ValidationError");
            assert_eq!(step["expect_message"], message);
        }
    }
    assert!(scenario["steps"].as_array().unwrap().iter().all(|s| s["expect_error"].is_boolean()));
}
