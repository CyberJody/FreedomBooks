use super::*;
use std::io::Write;
fn record(bytes: &mut Vec<u8>, code: u16, data: &[u8]) {
    bytes.extend(code.to_le_bytes());
    bytes.extend((data.len() as u16).to_le_bytes());
    bytes.extend(data);
}
fn workbook(strings: &[&str]) -> Vec<u8> {
    let mut b = vec![];
    record(&mut b, 0x809, &[0, 6, 5, 0]);
    record(&mut b, 0x85, &[0, 0, 0, 0, 0, 0]);
    let mut sst = vec![];
    sst.extend((strings.len() as u32).to_le_bytes());
    sst.extend((strings.len() as u32).to_le_bytes());
    for s in strings {
        sst.extend((s.len() as u16).to_le_bytes());
        sst.push(0);
        sst.extend(s.as_bytes());
    }
    record(&mut b, 0xfc, &sst);
    record(&mut b, 0xa, &[]);
    record(&mut b, 0x809, &[0, 6, 0x10, 0]);
    for i in 0..strings.len() {
        let mut d = vec![];
        d.extend(((i / 3) as u16).to_le_bytes());
        d.extend(((i % 3) as u16).to_le_bytes());
        d.extend([0, 0]);
        d.extend((i as u32).to_le_bytes());
        record(&mut b, 0xfd, &d);
    }
    record(&mut b, 0xa, &[]);
    let mut file =
        cfb::CompoundFile::create_with_version(cfb::Version::V3, Cursor::new(Vec::new())).unwrap();
    file.create_stream("Workbook")
        .unwrap()
        .write_all(&b)
        .unwrap();
    file.into_inner().into_inner()
}
const CONDITIONS: &str = r#"{"ruleConditions":[{"ruleType":1,"value":"SYNTHETIC"},{"ruleType":2,"value":"-258.54"},{"ruleType":10,"value":"-1"}],"isAndRule":true}"#;
const OUTPUTS: &str =
    r#"{"ruleActions":[{"actionType":0,"value":"Office"},{"actionType":8,"value":true}]}"#;
#[test]
fn bounded_biff_reads_original_format_and_preserves_flags() {
    let b = workbook(&[
        "Rule Name",
        "Rule Conditions",
        "Rule Outputs",
        "Synthetic",
        CONDITIONS,
        OUTPUTS,
    ]);
    let import = parse_xls(&b).unwrap();
    assert_eq!(import.rows.len(), 1);
    let row = &import.rows[0];
    assert!(row.diagnostics.is_empty(), "{:?}", row.diagnostics);
    assert!(row.auto_post);
    assert_eq!(row.direction, "OUT");
    assert_eq!(row.conditions[1].value, "-258.54");
    assert_eq!(row.row, 2);
    assert!(parse_xls(&vec![0; MAX_FILE + 1]).is_err());
    assert!(parse_xls(&b[..500]).is_err());
}
#[test]
fn blank_unknown_duplicate_and_deep_payloads_are_blocked() {
    assert!(
        !parse_row(2, "Synthetic", CONDITIONS, "")
            .diagnostics
            .is_empty()
    );
    assert!(
        !parse_row(
            2,
            "Synthetic",
            &CONDITIONS.replace("\"ruleType\":1,", "\"ruleType\":6,"),
            OUTPUTS
        )
        .diagnostics
        .is_empty()
    );
    assert!(
        !parse_row(
            2,
            "Synthetic",
            CONDITIONS,
            &OUTPUTS.replace(
                "\"actionType\":8,\"value\":true",
                "\"actionType\":11,\"value\":[]"
            )
        )
        .diagnostics
        .is_empty()
    );
    assert!(json_cell("{\"a\":1,\"a\":2}").is_err());
    assert!(json_cell("[[[[[[[[1]]]]]]]]").is_err());
}
#[test]
fn signed_amount_is_exact_without_rounding() {
    assert_eq!(signed_cents("-258.54").unwrap(), -25854);
    assert_eq!(signed_cents("-10").unwrap(), -1000);
    assert!(signed_cents("-10.001").is_err());
    assert!(signed_cents("999999999999999999").is_err());
}
#[test]
fn sst_continuation_keeps_encoding_boundaries() {
    let mut strings = Strings {
        chunks: vec![&[4, 0, 0, b'A', b'B'], &[1, b'C', 0, b'D', 0]],
        chunk: 0,
        pos: 0,
    };
    assert_eq!(strings.string().unwrap(), "ABCD");
    let mut bad = Strings {
        chunks: vec![&[4, 0, 0, b'A'], &[2, b'B']],
        chunk: 0,
        pos: 0,
    };
    assert!(bad.string().is_err());
}
// Read-only, opt-in validation of private export: never part of committed fixtures.
#[test]
#[ignore]
fn supplied_private_export_read_only() {
    let path = std::env::var("FB_QBO_PRIVATE_EXPORT").unwrap();
    let b = std::fs::read(path).unwrap();
    let parsed = parse_xls(&b).unwrap();
    assert_eq!(
        parsed.hash,
        "b82594ec49f81b536e611918950bf08c6bdb5de1c5217735f4c9417b9fa0f60a"
    );
    assert_eq!(parsed.rows.len(), 135);
    assert_eq!(parsed.rows.iter().filter(|r| r.auto_post).count(), 125);
    assert!(!parsed.rows[51].diagnostics.is_empty());
}

#[test]
fn hostile_compound_headers_are_rejected_before_fat_expansion() {
    let b = workbook(&[
        "Rule Name",
        "Rule Conditions",
        "Rule Outputs",
        "Synthetic",
        CONDITIONS,
        OUTPUTS,
    ]);
    for (at, value) in [(44, 100000u32), (68, 0), (72, 100000), (76, 0xffffffff)] {
        let mut bad = b.clone();
        bad[at..at + 4].copy_from_slice(&value.to_le_bytes());
        assert!(parse_xls(&bad).is_err());
    }
    // Bounded deterministic corruption exercises truncation and malformed record handling.
    for i in (0..b.len()).step_by(127) {
        let mut bad = b.clone();
        bad[i] ^= 0xff;
        let _ = parse_xls(&bad);
    }
}

#[test]
fn nested_storage_and_macro_streams_are_refused() {
    let b = workbook(&[
        "Rule Name",
        "Rule Conditions",
        "Rule Outputs",
        "Synthetic",
        CONDITIONS,
        OUTPUTS,
    ]);
    let mut file = cfb::CompoundFile::open(Cursor::new(b)).unwrap();
    let mut path = String::new();
    for _ in 0..32 {
        path.push_str("/Nested");
        file.create_storage(&path).unwrap();
    }
    assert!(parse_xls(&file.into_inner().into_inner()).is_err());
}
