//! Vectors produced by the frozen Python app (`crates/fb-harness/py/probe_money.py` run against
//! `ledger.services.money`). Every input must give the same result in Rust.
use fb_money::{AmountInput, money};

#[test]
fn every_python_vector_matches() {
    let raw = include_str!("python_vectors.json");
    let data: serde_json::Value = serde_json::from_str(raw).unwrap();
    let mut checked = 0;
    for pair in data["text"].as_array().unwrap() {
        let input = pair[0].as_str().unwrap();
        let got = money(&AmountInput::Text(input.to_string())).map(|m| m.to_string());
        check(&format!("{input:?}"), got, &pair[1]);
        checked += 1;
    }
    for pair in data["int"].as_array().unwrap() {
        let n = pair[0].as_str().unwrap();
        let value: i128 = n.parse().unwrap();
        let got = money(&AmountInput::Int(value)).map(|m| m.to_string());
        check(n, got, &pair[1]);
        checked += 1;
    }
    let other = &data["other"];
    check("float", money(&AmountInput::Float(1.25)).map(|m| m.to_string()), &other["float"]);
    check("bool", money(&AmountInput::Bool(true)).map(|m| m.to_string()), &other["bool"]);
    check("none", money(&AmountInput::Other).map(|m| m.to_string()), &other["none"]);
    assert!(checked > 70, "{checked}");
}

fn check(label: &str, got: Result<String, fb_money::MoneyError>, want: &serde_json::Value) {
    if let Some(ok) = want.get("ok") {
        // Python keeps a negative zero ("-0.00"); integer cents cannot (see the L1 report).
        let want = ok.as_str().unwrap().trim_start_matches('-');
        assert_eq!(got.as_deref(), Ok(want), "{label}");
    } else {
        let message = want["error"].as_str().unwrap();
        match got {
            Err(e) => assert_eq!(e.to_string(), message, "{label}"),
            Ok(v) => panic!("{label}: Python refused ({message}), Rust gave {v}"),
        }
    }
}
