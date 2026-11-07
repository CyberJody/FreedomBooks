from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
p=ROOT/'crates/fb-app/tests/d5_flow.rs'
p.write_text(p.read_text()+'''

#[test]
fn credit_memo_has_no_print_route_like_the_frozen_reference() {
    let company = "0195b4a4-55af-719e-96cb-a2f946b067bf";
    let memo = "0195b4a4-55af-719e-96cb-a2f946b067c0";
    assert!(screens::resolve(&format!("/c/{company}/credit-memos/{memo}/")).is_some());
    assert!(screens::resolve(&format!("/c/{company}/credit-memos/{memo}/print/")).is_none());
    assert!(screens::resolve(&format!("/c/{company}/invoices/{memo}/print/")).is_some());
}
''')
