//! Comparer self-tests: it must catch a one-cent difference, a register order change and an outcome
//! change, and only warn on wording.
use serde_json::json;

mod common;
use common::dump;

#[test]
fn identical_dumps_match() {
    assert!(fb_harness::compare(&dump(), &dump()).ok());
}

#[test]
fn matching_audit_counts_cannot_replace_full_payloads() {
    let mut bad=dump();bad["audit"]["c1"]=json!({"journal.posted":1});
    assert!(!fb_harness::compare(&bad,&bad).ok());
}

#[test]
fn one_cent_is_a_failure() {
    let mut other = dump();
    other["journals"][0]["lines"][0]["debit"] = json!(100);
    let report = fb_harness::compare(&dump(), &other);
    assert!(!report.ok());
    assert!(report.failures[0].contains("debit"), "{:?}", report.failures);
}

#[test]
fn register_and_non_tie_tb_order_are_failures() {
    let mut other = dump();
    other["registers"]["a"]["rows"] = json!([["j2", 1, 202], ["j1", 1, 101]]);
    assert!(!fb_harness::compare(&dump(), &other).ok());
    let mut other = dump();
    other["trial_balances"]["c1"]["2026-12-31"]["rows"] = json!([["b", 0, 101], ["a", 101, 0]]);
    let report = fb_harness::compare(&dump(), &other);
    assert!(!report.ok());
    assert_eq!(report.warnings.len(), 0);
}

#[test]
fn outcome_kind_fails_wording_warns_and_j_numbers_normalize() {
    let mut other = dump();
    other["outcomes"][0]["message"] = json!("Different words.");
    let report = fb_harness::compare(&dump(), &other);
    assert!(report.ok() && report.warnings.len() == 1);
    other["outcomes"][0]["ok"] = json!(true);
    assert!(!fb_harness::compare(&dump(), &other).ok());
    let mut renumbered = dump();
    renumbered["journals"][0]["memo"] = json!("Reversal of J000003: x");
    renumbered["display"]["j0"] = json!("J000003");
    assert!(fb_harness::compare(&dump(), &renumbered).ok());
}

#[test]
fn review_extra_trial_balance_row_is_not_an_order_warning() {
    let mut other=dump();
    other["trial_balances"]["c1"]["2026-12-31"]["rows"].as_array_mut().unwrap().insert(0,json!(["a",999,0]));
    assert!(!fb_harness::compare(&dump(),&other).ok());
}
#[test]
fn review_extra_company_or_date_report_is_rejected() {
    for (company,date) in [("extra","2026-12-31"),("c1","2027-01-01")] {
        let mut other=dump();
        if other["trial_balances"].get(company).is_none() {other["trial_balances"][company]=json!({});}
        other["trial_balances"][company][date]=json!({"rows":[["x",100,0]],"debit":100,"credit":0});
        assert!(!fb_harness::compare(&dump(),&other).ok());
    }
}
#[test]
fn review_ordinary_memo_display_numbers_are_not_normalized() {
    let mut py=dump(); let mut rs=dump();
    py["journals"][0]["memo"]=json!("Customer said J000007");
    rs["journals"][0]["memo"]=json!("Customer said J000003");
    rs["display"]["j0"]=json!("J000003");
    assert!(!fb_harness::compare(&py,&rs).ok());
}
#[test]
fn review_duplicate_natural_keys_fail_even_when_both_sides_identical() {
    for collection in ["companies","accounts","journals"] {
        let mut bad=dump(); let duplicate=bad[collection][0].clone();
        bad[collection].as_array_mut().unwrap().push(duplicate);
        assert!(!fb_harness::compare(&bad,&bad).ok(),"{collection}");
    }
}
#[test]
fn review_outcome_identity_is_compared() {
    let mut other=dump(); other["outcomes"][0]["op"]=json!("post_journal");
    assert!(!fb_harness::compare(&dump(),&other).ok());
}
#[test]
fn review_audit_actor_target_and_details_are_compared() {
    let mut py=dump();
    py["audit"]["c1"]=json!([{"action":"journal.posted","actor":"owner","actor_name":"owner","target":"journal:j1","detail":{"memo":"Ordinary J000007","debit":"1.01"}}]);
    for (field,value) in [("actor",json!("intruder")),("actor_name",json!("renamed")),("target",json!("journal:wrong")),("detail",json!({"memo":"changed","debit":"1.01"}))] {
        let mut other=py.clone(); other["audit"]["c1"][0][field]=value;
        assert!(!fb_harness::compare(&py,&other).ok());
    }
}

#[test]
fn review_runner_emits_full_audit_payloads_not_counts() {
    let scenario=json!({"users":[{"username":"owner"}],"steps":[{"op":"create_company","ref":"c1","name":"C"}]});
    let path=std::env::temp_dir().join(format!("audit-payload-{}.fbbook",std::process::id()));
    let _=std::fs::remove_file(&path);
    let mut runner=fb_harness::Runner::new(&scenario,&path); runner.run();
    let dump=runner.dump();
    assert_eq!(dump["audit"]["c1"][0]["actor"],json!("owner"));
    assert_eq!(dump["audit"]["c1"][0]["target"],json!("company:c1"));
    assert_eq!(dump["audit"]["c1"][0]["detail"],json!({"name":"C","currency":"USD"}));
}

#[test]
fn only_punctuation_tie_inversions_are_accepted_and_counted() {
    for (a,b,accepted) in [("1â","1,â",true),("a","A",false),("e","é",false),("a","b",false)] {
        let mut py=dump();
        py["accounts"][0]["code"]=json!(a); py["accounts"][1]["code"]=json!(b);
        let mut rs=py.clone();
        rs["accounts"].as_array_mut().unwrap().reverse();
        rs["trial_balances"]["c1"]["2026-12-31"]["rows"].as_array_mut().unwrap().reverse();
        let report=fb_harness::compare(&py,&rs);
        assert_eq!(report.ok(),accepted,"{a:?}/{b:?}: {report:?}");
        assert!(report.warnings.is_empty());
        assert_eq!(report.collation_ties,if accepted {2}else{0});
    }
}

#[test]
fn l2_order_ties_do_not_mask_changed_values_duplicates_or_company_moves() {
    let mut py=dump();
    py["l2a"]["directory"]=json!([
        {"id":"x","company_id":"c1","model":"customer","display_name":"1â","notes":"Keep 123","active":true,"business_name":"","contact_name":"","email":"","phone":"","billing_address":"","tax_status":"TAXABLE","exemption_number":""},
        {"id":"y","company_id":"c1","model":"customer","display_name":"1,â","notes":"Keep 456","active":true,"business_name":"","contact_name":"","email":"","phone":"","billing_address":"","tax_status":"TAXABLE","exemption_number":""}
    ]);
    let mut rs=py.clone();rs["l2a"]["directory"].as_array_mut().unwrap().reverse();
    let report=fb_harness::compare(&py,&rs);
    assert!(report.ok() && report.warnings.is_empty());assert_eq!(report.collation_ties,1);
    rs["l2a"]["directory"][0]["notes"]=json!("Changed 456");
    assert!(!fb_harness::compare(&py,&rs).ok());
    let mut duplicate=py.clone(); let row=duplicate["l2a"]["directory"][0].clone();
    duplicate["l2a"]["directory"].as_array_mut().unwrap().push(row);
    assert!(!fb_harness::compare(&duplicate,&duplicate).ok());
    py["l2a"]["directory"][0]["company_id"]=json!("other");
    rs=py.clone();rs["l2a"]["directory"].as_array_mut().unwrap().reverse();
    assert!(!fb_harness::compare(&py,&rs).ok());
}

#[test]
fn every_l2_audit_payload_leaf_remains_significant() {
    let scenario:serde_json::Value=serde_json::from_str(include_str!("../scenarios/l2a-directory.json")).unwrap();
    let path=std::env::temp_dir().join(format!("l2-audit-payload-{}.fbbook",std::process::id()));
    let _=std::fs::remove_file(&path);
    let mut runner=fb_harness::Runner::new(&scenario,&path);runner.run();let py=runner.dump();
    fn paths(v:&serde_json::Value,path:String,out:&mut Vec<String>) {
        match v {
            serde_json::Value::Object(m)=>for (k,v) in m {paths(v,format!("{path}/{k}"),out);},
            serde_json::Value::Array(a)=>for (i,v) in a.iter().enumerate() {paths(v,format!("{path}/{i}"),out);},
            _=>out.push(path)
        }
    }
    let mut leaves=vec![];paths(&py["audit"],"/audit".into(),&mut leaves);
    assert!(leaves.len()>100);
    for path in leaves {
        let mut rs=py.clone();*rs.pointer_mut(&path).unwrap()=json!("changed business value 123");
        assert!(!fb_harness::compare(&py,&rs).ok(),"ignored {path}");
    }
}
