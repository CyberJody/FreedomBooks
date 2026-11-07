use fb_plugin_api::Date;
use fb_plugin_openai::receipts::parse_reply;
use serde_json::Value;

#[test]
fn frozen_python_receipt_parser_parity() {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("receipt-reference-cases.json")).unwrap();
    assert_eq!(cases.len(), 97);
    for case in cases {
        let result = parse_reply(
            &case["input"].to_string(),
            "USD",
            Date::new(2023, 11, 14).unwrap(),
        );
        if case.get("error").is_some() {
            assert!(result.is_err(), "Python refused case {}", case["name"]);
        } else {
            let reading = result.unwrap_or_else(|e| panic!("case {}: {e}", case["name"]));
            assert_eq!(
                reading.notes.len(),
                case["note_count"].as_u64().unwrap() as usize,
                "notes in case {}",
                case["name"]
            );
            let mut actual = serde_json::to_value(reading).unwrap();
            actual.as_object_mut().unwrap().remove("notes");
            assert_eq!(actual, case["expected"], "case {}", case["name"]);
        }
    }
}

#[test]
fn frozen_python_precision_boundary_parity() {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("receipt-precision-cases.json")).unwrap();
    assert_eq!(cases.len(), 47);
    for case in cases {
        let result = parse_reply(
            &case["input"].to_string(),
            "USD",
            Date::new(2023, 11, 14).unwrap(),
        );
        if case.get("error").is_some() {
            assert!(result.is_err(), "Python refused case {}", case["name"]);
        } else {
            let reading = result.unwrap_or_else(|e| panic!("case {}: {e}", case["name"]));
            assert_eq!(
                reading.notes.len(),
                case["note_count"].as_u64().unwrap() as usize,
                "notes in case {}",
                case["name"]
            );
            let mut actual = serde_json::to_value(reading).unwrap();
            actual.as_object_mut().unwrap().remove("notes");
            assert_eq!(actual, case["expected"], "case {}", case["name"]);
        }
    }
}
