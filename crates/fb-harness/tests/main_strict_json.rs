//! The `fb-harness` binary reads scenarios and dumps with the shared duplicate-key-rejecting parser: a repeated
//! key (serde keeps the last, SQLite the first) is a failure, never silently resolved.
use std::process::Command;

fn harness(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_fb-harness")).args(args).output().unwrap()
}

#[test]
fn compare_rejects_duplicate_keys() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dup.json");
    for raw in [r#"{"financial_reports": false, "financial_reports": null}"#, r#"{"a": {"b": 1, "b": 2}}"#] {
        std::fs::write(&path, raw).unwrap();
        let p = path.to_str().unwrap();
        let out = harness(&["compare", p, p]);
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert_eq!(out.status.code(), Some(1), "{raw}: {stdout}");
        assert!(stdout.contains("FAIL") && stdout.to_lowercase().contains("duplicate") && !stdout.lines().any(|l| l.starts_with("MATCH")), "{raw}: {stdout}");
    }
}

#[test]
fn run_rejects_duplicate_scenario_keys() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dup-scenario.json");
    std::fs::write(&path, r#"{"users": [{"username": "u", "is_staff": true}], "steps": [], "steps": []}"#).unwrap();
    let out = harness(&["run", path.to_str().unwrap(), dir.path().join("out.json").to_str().unwrap(), dir.path().to_str().unwrap()]);
    assert_ne!(out.status.code(), Some(0));
    assert!(!dir.path().join("out.json").exists());
}
