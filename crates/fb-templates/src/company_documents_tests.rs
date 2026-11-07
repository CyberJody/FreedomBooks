use super::*;
use serde_json::{Value, json};

#[test]
fn company_document_empty_fields_are_byte_identical_to_the_prior_print_template() {
    let mut screens = Screens::new();
    screens
        .env
        .add_template(
            "before.html",
            include_str!("../tests/fixtures/company_document_before.html"),
        )
        .unwrap();
    for kind in ["invoice", "estimate", "sales_receipt"] {
        for mark in ["", "DRAFT", "VOID", "Earlier version"] {
            let mut vm = fixture();
            vm["doc"]["kind"] = json!(kind);
            vm["doc"]["mark"] = json!(mark);
            let before = screens.render("before.html", &vm).unwrap();
            vm["doc"]["business"] = json!({
                "street1":"", "street2":"", "city":"", "region":"", "postal_code":"",
                "country":"", "phone":"", "email":"", "website":"", "tax_id":"",
                "payment_instructions":""
            });
            let after = screens.render("sales/print/document.html", &vm).unwrap();
            assert_eq!(before, after, "empty details changed {kind} / {mark}");
        }
    }
}

#[test]
fn company_document_filled_fields_escape_plain_text_and_omit_blank_labels() {
    let screens = Screens::new();
    let mut vm = fixture();
    vm["doc"]["business"] = json!({"street1":"<street> & company","tax_id":"<tax>","payment_instructions":"Pay <check>\nThank you & co."});
    vm["doc"]["company_logo_url"] = json!("/c/synthetic/document-logo/");
    let html = screens.render("sales/print/document.html", &vm).unwrap();
    assert!(html.contains("&lt;street&gt; &amp; company"));
    assert!(html.contains("Tax ID: &lt;tax&gt;"));
    assert!(html.contains("Pay &lt;check&gt;<br>Thank you &amp; co."));
    assert_eq!(html.matches("class=\"business-contact\"").count(), 2);
    assert!(!html.contains("<check>"));
}

fn fixture() -> Value {
    json!({"company":{"pk":"00000000-0000-7000-8000-000000000001"},"root":{"pk":"00000000-0000-7000-8000-000000000002"},"doc":{"kind":"invoice","title":"Invoice","company":{"name":"Synthetic seller"},"number":"INV-1","currency":"USD","revision_number":1,"revision_total":1,"date":"2026-11-05","printed_at":"2026-11-05T12:00:00Z","party":{"display_name":"Synthetic buyer"},"lines":[{"description":"Sample service","quantity":"1","rate":"10.00","amount":"10.00"}],"balance":null},"current":{"subtotal":"10.00","tax":"0.00","total":"10.00","tax_mode":"NONE"}})
}
