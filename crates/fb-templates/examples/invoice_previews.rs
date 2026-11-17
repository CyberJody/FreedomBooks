//! Synthetic visual fixtures only. Does not open a book, connect providers or send mail.
use fb_templates::Screens;
use serde_json::json;

fn main() {
    let destination=std::path::PathBuf::from(std::env::args().nth(1).expect("isolated output directory"));
    std::fs::create_dir_all(&destination).unwrap();
    let screens=Screens::new();
    let base=json!({
        "company":{"pk":"00000000-0000-7000-8000-000000000001"},
        "root":{"pk":"00000000-0000-7000-8000-000000000002"},"fb_print_synthetic":true,
        "doc":{"kind":"invoice","title":"Invoice","number":"INV-1042","currency":"USD",
            "company":{"name":"Sample Technical Services"},"date":"2026-11-08","second_date":"2026-12-08","terms":"Net 30",
            "revision_number":2,"revision_total":2,"state_label":"Posted · Open","printed_at":"2026-11-08T18:00:00Z",
            "party":{"display_name":"Sample Client & Co.","contact_name":"Alex Example","billing_address":"42 Sample Lane\nExample City, NY 12801","email":"alex@example.test"},
            "business":{"city":"Example City","region":"NY","phone":"(518) 555-0123","email":"billing@example.test","website":"www.example.test","payment_instructions":"Prefer a paper check? Email billing@example.test for more information."},
            "preferences":{"zelle_enabled":true,"zelle_name":"Sample Technical Services","zelle_recipient":"payments@example.test","zelle_instructions":"","footer":"Thank you for your business.","email_mode":"DRAFT","email_subject":"Invoice INV-1042 from Sample Technical Services","email_greeting":"Hi Alex,\n\nThank you for your business! Here is your invoice summary and how to pay.","email_signature":"Thanks,\nThe Sample Technical Services team"},
            "can_pay":true,"balance":{"due":"240.00","paid":"0.00","as_of":"2026-11-08"},"memo":"Completed remote support and network setup.","posting_note":"",
            "lines":[{"description":"Remote Support: resolved mail synchronization and reviewed backups","quantity":"1","rate":"110.00","amount":"110.00","taxable":false},{"description":"Network Support: updated access points and verified connectivity","quantity":"1","rate":"130.00","amount":"130.00","taxable":false}]},
        "current":{"subtotal":"240.00","tax_mode":"NONE","tax":"0.00","total":"240.00"}
    });
    for name in ["invoice","email","long-invoice","historical","paid","void","draft"] {
        let mut vm=base.clone();
        if matches!(name,"long-invoice"|"historical") {
            vm["doc"]["lines"]=json!((1..=75).map(|n|json!({"description":format!("Service line {n:02}: synthetic description with enough detail to wrap and exercise the repeated table heading and page boundaries."),"quantity":"1","rate":"25.00","amount":"25.00"})).collect::<Vec<_>>());
            vm["current"]["subtotal"]=json!("1875.00");vm["current"]["total"]=json!("1875.00");vm["doc"]["balance"]["due"]=json!("1875.00");
        }
        if name=="historical" {
            vm["doc"]["superseded"]=json!(true);vm["doc"]["revision_total"]=json!(3);
            vm["doc"]["posting_note"]=json!("Earlier version — kept for the record. See the current version for any balance.");
        }
        if matches!(name,"historical"|"void"|"draft") {vm["doc"]["balance"]=serde_json::Value::Null;vm["doc"]["can_pay"]=json!(false);}
        if name=="paid" {vm["doc"]["balance"]["due"]=json!("0.00");vm["doc"]["balance"]["paid"]=json!("240.00");vm["doc"]["can_pay"]=json!(false);}
        if matches!(name,"paid"|"void"|"draft") {vm["doc"]["mark"]=json!(name.to_uppercase());vm["doc"]["state_label"]=json!(name);}
        let template=if name=="email" {"sales/email/preview.html"} else {"sales/print/document.html"};
        std::fs::write(destination.join(format!("{name}.html")),screens.render(template,&vm).unwrap()).unwrap();
    }
    println!("Created seven synthetic invoice/email visual fixtures.");
}
