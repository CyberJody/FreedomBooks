use super::*;
use sha2::{Digest, Sha256};
#[test]
fn installed_format_90_history_is_unchanged_and_recurring_is_one_additive_migration() {
    let pin: serde_json::Value = serde_json::from_str(include_str!(
        "../tests/recurring_bills_base_migrations.json"
    ))
    .unwrap();
    assert_eq!(pin["source"], "5a8c4fd9c9b8fc06ee2ee7df810f20efd5e86148");
    let old = pin["migrations"].as_array().unwrap();
    assert_eq!(old.len(), 90);
    for ((name, sql), entry) in MIGRATIONS.iter().zip(old) {
        assert_eq!(*name, entry["name"], "historical migration order");
        assert_eq!(
            format!("{:x}", Sha256::digest(sql.as_bytes())),
            entry["sha256"],
            "{name} changed after installation"
        );
    }
    assert_eq!(MIGRATIONS[90].0, "0204_recurring_bills");
    assert_eq!(MIGRATIONS[89].0, "0203_salestax_payment_evidence");
    assert_eq!(MIGRATIONS[91].0, "0205_job_tickets");
    assert_eq!(MIGRATIONS[92].0, "0206_remote_host");
    assert_eq!(MIGRATIONS.len(), 93);
    let dir = tempfile::tempdir().unwrap();
    let book = Book::create(&dir.path().join("format92.fbbook"), &[92; 32]).unwrap();
    assert!(book.check().unwrap().is_empty());
}
