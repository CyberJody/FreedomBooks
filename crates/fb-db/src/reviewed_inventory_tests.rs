use super::*;
use std::collections::BTreeSet;

#[test]
fn every_trigger_and_required_guard_from_ten_reviewed_heads_survives_with_exact_counts() {
    let dir = tempfile::tempdir().unwrap();
    let book = Book::create(&dir.path().join("inventory.fbbook"), &[81; 32]).unwrap();
    let installed: BTreeSet<String> = book
        .conn
        .prepare("SELECT name FROM sqlite_master WHERE type='trigger'")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let required: BTreeSet<&str> = REQUIRED_TRIGGERS.iter().copied().collect();
    eprintln!(
        "integration head: {} installed triggers, {} required guards",
        installed.len(),
        required.len()
    );
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../tests/reviewed_trigger_inventory.json")).unwrap();
    let mut missing = Vec::new();
    for (module, inventory) in fixture.as_object().unwrap() {
        eprintln!(
            "reviewed {module} {}: {} triggers, {} required guards",
            inventory["head"],
            inventory["triggers"].as_array().unwrap().len(),
            inventory["required"].as_array().unwrap().len()
        );
        for name in inventory["triggers"].as_array().unwrap() {
            let name = name.as_str().unwrap();
            if !installed.contains(name) {
                missing.push(format!("{module}: missing installed trigger {name}"));
            }
        }
        for name in inventory["required"].as_array().unwrap() {
            let name = name.as_str().unwrap();
            if !required.contains(name) {
                missing.push(format!("{module}: missing required guard {name}"));
            }
        }
    }
    assert!(missing.is_empty(), "{}", missing.join("\n"));
    // INT-L4, desktop estimates and the 44 reviewed bank-feed triggers.
    // Require the full union of reviewed inventories, including D4, with exact head counts.
    assert_eq!(installed.len(), 944);
    assert_eq!(required.len(), 577);
    let value_guards: BTreeSet<&str> = value_guard_triggers().collect();
    assert_eq!(value_guards.len(), 174);
    // Both lanes add distinct guards; retain their complete reviewed union.
    assert!(value_guards.contains("fb_valid_plugin_credential_insert"));
    assert!(value_guards.contains("fb_valid_plugin_credential_update"));
    assert!(value_guards.contains("fb_valid_fb_numbering_insert"));
    assert!(value_guards.contains("fb_valid_fb_numbering_update"));
    assert!(value_guards.is_disjoint(&required));
    assert!(value_guards.iter().all(|name| installed.contains(*name)));
    assert_eq!(installed.iter().filter(|name| !value_guards.contains(name.as_str())).count(), 770);
    for name in ["company_documents_data_insert", "company_documents_data_update",
        "fb_json_company_document_details_insert", "fb_json_company_document_details_update",
        "company_documents_identity_insert", "company_documents_identity_update", "company_documents_identity_delete"] {
        assert!(installed.contains(name));
        assert!(required.contains(name));
    }
    // 0140 retains all reviewed triggers and adds exactly this required guard.
    assert!(installed.contains("pp_customerdeposit_estimate_scope"));
    assert!(required.contains("pp_customerdeposit_estimate_scope"));
    assert!(required.contains("fb_sales_receipt_snapshot"));
    let reviewed = include_str!("../sql/0016_sales_review_fixes.sql")
        .split("CREATE TRIGGER fb_sales_receipt_snapshot ")
        .nth(1)
        .unwrap()
        .split("CREATE TRIGGER fb_sales_detail_snapshot ")
        .next()
        .unwrap();
    let restored = include_str!("../sql/0079_restore_sales_receipt_snapshot.sql")
        .split("CREATE TRIGGER fb_sales_receipt_snapshot ")
        .nth(1)
        .unwrap();
    assert_eq!(restored, reviewed, "restore the reviewed trigger unchanged");
    // --nocapture preserves the complete live inventory for integration review.
    eprintln!("complete guard inventory: {}", serde_json::json!({
        "installed": installed,
        "required": required,
        "value_guards": value_guards,
        "stored_json_columns": json_evidence::STORED_JSON_COLUMNS,
    }));
}
