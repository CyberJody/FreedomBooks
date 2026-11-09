mod common;
use common::*;
use fb_core::{directory::{self, Model}, services};
use serde_json::json;

#[test]
fn free_text_refuses_controls_before_trimming_without_writes() {
    let mut f=Fx::new("secfix1-text");
    let (c,bank,equity)=f.chart();
    let forbidden=['\0','\n','\r','\u{7f}','\u{85}','\u{ad}','\u{202a}','\u{202b}','\u{202c}','\u{202d}','\u{202e}','\u{2066}','\u{2067}','\u{2068}','\u{2069}','\u{200b}','\u{200c}','\u{200d}','\u{2060}','\u{feff}'];
    for ch in forbidden {
        let value=format!("{ch}Name{ch}");
        let before=fb_db::test_support::snapshot(f.book.connection()).unwrap();
        assert!(services::create_company(&mut f.book,f.user,&value,"USD",false).unwrap_err().message().contains("Control and formatting"));
        for model in [Model::Customer,Model::Vendor,Model::Item] {
            let field=if model==Model::Item {"code"} else {"display_name"};
            let data=json!({field:value});
            assert!(directory::save_record(&mut f.book,f.user,c,model,&data,None).unwrap_err().message().contains("Control and formatting"));
        }
        for field in ["business_name","billing_address","email","phone"] {
            let data=json!({"display_name":"Customer",field:value});
            assert!(directory::save_record(&mut f.book,f.user,c,Model::Customer,&data,None).unwrap_err().message().contains("Control and formatting"));
        }
        let a=services::AccountInput::new("999",&value).kind("ASSET");
        assert!(services::save_account(&mut f.book,f.user,c,a).is_err());
        let mut draft=services::DraftInput::new("2026-10-01",&value,vec![services::line(bank).debit("1").credit("0").build(),services::line(equity).debit("0").credit("1").build()]);
        assert!(services::save_draft(&mut f.book,f.user,c,draft.clone()).is_err());
        draft.memo="Safe".into();draft.lines[0]["description"]=json!(value);
        assert!(services::save_draft(&mut f.book,f.user,c,draft).is_err());
        assert_eq!(before,fb_db::test_support::snapshot(f.book.connection()).unwrap(),"U+{:04X}",ch as u32);
    }
    assert!(fb_core::py::clean_free_text("Café\t東京 😀").is_ok());
}

#[test]
fn csv_formula_and_delimiter_cells_use_one_encoder() {
    use fb_core::reports::params::csv_cell;
    for start in ["\t","\r"] {
        assert_eq!(csv_cell(&format!("{start}=SUM(A1)")),format!("\"'{start}=SUM(A1)\""));
    }
    assert_eq!(csv_cell("=1+1"),"'=1+1");
    assert_eq!(csv_cell("a,\"b\""),"\"a,\"\"b\"\"\"");
    assert_eq!(csv_cell("plain"),"plain");
}

#[test]
fn existing_stored_text_is_read_without_rewriting() {
    let mut f=Fx::new("secfix1-legacy");let c=f.company("Legacy company");
    let customer=directory::save_record(&mut f.book,f.user,c,Model::Customer,&json!({"display_name":"Customer"}),None).unwrap();
    let id=customer["id"].as_str().unwrap();
    // Model an existing v1/imported row, not a new input through a service.
    f.book.connection().execute("UPDATE directory_customer SET display_name=?,billing_address=? WHERE id=?",fb_db::rusqlite::params!["Legacy\u{202e}Name","Street\nTown",id]).unwrap();
    let before=fb_db::test_support::snapshot(f.book.connection()).unwrap();
    assert!(f.book.check().unwrap().is_empty());
    let loaded=directory::get(f.book.connection(),c,Model::Customer,id).unwrap();
    assert_eq!(loaded["display_name"],"Legacy\u{202e}Name");assert_eq!(loaded["billing_address"],"Street\nTown");
    assert_eq!(before,fb_db::test_support::snapshot(f.book.connection()).unwrap());
}

#[test]
fn directory_notes_preserve_line_endings_with_atomic_validation_and_audit() {
    let mut f = Fx::new("multiline-notes");
    let c = f.company("Synthetic notes company");
    for model in [Model::Customer, Model::Vendor] {
        let original = "First line\n\nCafé\t東京 😀\r\nLast line\rFinal line";
        let row = directory::save_record(&mut f.book, f.user, c, model,
            &json!({"display_name":"Synthetic contact","notes":original}), None).unwrap();
        let id = row["id"].as_str().unwrap();
        assert_eq!(row["notes"], original);
        let revised = "Revised context\n\nAdditional contact details";
        let row = directory::save_record(&mut f.book, f.user, c, model,
            &json!({"notes":revised}), Some(id)).unwrap();
        assert_eq!(row["notes"], revised);
        let audit: String = f.book.connection().query_row(
            "SELECT detail FROM ledger_auditevent WHERE target LIKE '%' || ?1 AND action LIKE '%.updated'",
            [id], |r| r.get(0)).unwrap();
        let audit: serde_json::Value = serde_json::from_str(&audit).unwrap();
        assert_eq!(audit["before"]["notes"], original);
        assert_eq!(audit["after"]["notes"], revised);
        let before = fb_db::test_support::snapshot(f.book.connection()).unwrap();
        for ch in ['\0','\u{7f}','\u{85}','\u{ad}','\u{202a}','\u{202b}','\u{202c}',
            '\u{202d}','\u{202e}','\u{2066}','\u{2067}','\u{2068}','\u{2069}',
            '\u{200b}','\u{200c}','\u{200d}','\u{2060}','\u{feff}'] {
            let invalid = format!("{ch}First\n\nLast{ch}");
            for record_id in [None, Some(id)] {
                assert!(directory::save_record(&mut f.book, f.user, c, model,
                    &json!({"display_name":"Invalid contact","notes":invalid}), record_id).is_err());
            }
        }
        assert!(directory::save_record(&mut f.book, f.user, c, model,
            &json!({"notes":"é".repeat(4001)}), Some(id)).is_err());
        assert_eq!(before, fb_db::test_support::snapshot(f.book.connection()).unwrap());
        assert!(f.book.check().unwrap().is_empty());
    }
}
