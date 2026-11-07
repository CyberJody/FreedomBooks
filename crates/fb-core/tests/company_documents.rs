use base64::{Engine, engine::general_purpose::STANDARD};
use fb_core::{
    company_documents::{self as docs, Details},
    services, users,
};
use fb_db::Book;
use std::io::Cursor;

fn image(format: image::ImageFormat, width: u32) -> String {
    let mut out = Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(width, 10)
        .write_to(&mut out, format)
        .unwrap();
    STANDARD.encode(out.into_inner())
}

#[test]
fn company_document_logo_checks_real_type_dimensions_and_reencodes_pixels() {
    for format in [image::ImageFormat::Png, image::ImageFormat::Jpeg] {
        let encoded = image(format, 24);
        let (_, bytes) = docs::clean_logo(&encoded).unwrap();
        assert_eq!(image::guess_format(&bytes).unwrap(), format);
        assert_eq!(image::load_from_memory(&bytes).unwrap().width(), 24);
        let mut tailed = STANDARD.decode(encoded).unwrap();
        tailed.extend_from_slice(b"<script>trailing payload</script>");
        let (_, cleaned) = docs::clean_logo(&STANDARD.encode(tailed)).unwrap();
        assert!(!String::from_utf8_lossy(&cleaned).contains("trailing payload"));
    }
    for encoded in [
        STANDARD.encode(b"<svg xmlns='http://www.w3.org/2000/svg'></svg>"),
        STANDARD.encode(b"GIF89a\x01\x00\x01\x00"),
        image(image::ImageFormat::Png, 1201),
        STANDARD.encode(vec![1; docs::LOGO_MAX_BYTES + 1]),
        STANDARD.encode(b"\x89PNG\r\n\x1a\n"),
    ] {
        assert!(docs::clean_logo(&encoded).is_err());
    }
}

#[test]
fn company_document_text_limits_email_and_plain_text_paragraphs() {
    let mut details = Details {
        street1: "  42 Sample Lane  ".into(),
        payment_instructions: "Pay <sample> & co.\r\nBy check".into(),
        ..Default::default()
    };
    let clean = docs::clean(details.clone()).unwrap();
    assert_eq!(clean.street1, "42 Sample Lane");
    assert_eq!(clean.payment_instructions, "Pay <sample> & co.\nBy check");
    details.email = "not-an-email".into();
    assert!(docs::clean(details.clone()).is_err());
    details.email = "sample@example.test".into();
    details.street1 = "Line\nOther line".into();
    assert!(docs::clean(details.clone()).is_err());
    details.street1.clear();
    details.payment_instructions = "x".repeat(2001);
    assert!(docs::clean(details.clone()).is_err());
    details.payment_instructions = "spoof\u{202e}text".into();
    assert!(docs::clean(details).is_err());
    assert!(serde_json::from_value::<Details>(serde_json::json!({"unknown":"x"})).is_err());
}

#[test]
fn company_document_reader_rejects_decoded_corruption_without_normalizing() {
    let dir=tempfile::tempdir().unwrap();
    let mut book=Book::create(&dir.path().join("reader-corruption.fbbook"),&[61;32]).unwrap();
    let actor=users::create_user(&mut book,"Synthetic owner",true).unwrap();
    let company=services::create_company(&mut book,actor,"Synthetic seller","USD",false).unwrap().id;
    docs::save(&mut book,actor,company,"Synthetic seller","USD",docs::Changes{details:Details{street1:"Original".into(),..Default::default()},logo_upload:None,remove_logo:false}).unwrap();
    let guards:Vec<(String,String)>=book.connection().prepare("SELECT name,sql FROM sqlite_schema WHERE type='trigger' AND tbl_name='company_document_details'").unwrap().query_map([],|r|Ok((r.get(0)?,r.get(1)?))).unwrap().collect::<Result<_,_>>().unwrap();
    let mut data=serde_json::to_value(Details::default()).unwrap();
    data["street1"]=serde_json::json!(format!("A\0{}","X".repeat(201)));
    for (name,_) in &guards {book.connection().execute_batch(&format!("DROP TRIGGER {name}")).unwrap();}
    book.connection().execute_batch("PRAGMA ignore_check_constraints=ON").unwrap();
    book.connection().execute("UPDATE company_document_details SET data=?2 WHERE company_id=?1",[company.to_db(),data.to_string()]).unwrap();
    book.connection().execute_batch("PRAGMA ignore_check_constraints=OFF").unwrap();
    for (_,sql) in guards {book.connection().execute_batch(&sql).unwrap();}
    let error=book.read(|q|docs::get(q,company)).unwrap_err();
    assert!(error.to_string().contains("Invalid stored business details"),"{error}");
    let stored:String=book.connection().query_row("SELECT data FROM company_document_details WHERE company_id=?1",[company.to_db()],|r|r.get(0)).unwrap();
    assert_eq!(stored,data.to_string(),"reader never repairs corruption silently");
}

#[test]
fn company_document_save_is_atomic_scoped_audited_portable_and_removable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("synthetic.fbbook");
    let key = [51; 32];
    let mut book = Book::create(&path, &key).unwrap();
    let actor = users::create_user(&mut book, "Synthetic owner", true).unwrap();
    let other = users::create_user(&mut book, "Other owner", true).unwrap();
    let c = services::create_company(&mut book, actor, "Sample seller", "USD", false)
        .unwrap()
        .id;
    assert_eq!(
        book.read(|tx| docs::get(tx, c)).unwrap(),
        Details::default()
    );
    let details = Details {
        street1: "42 Sample Lane".into(),
        payment_instructions: "By check\nThank you".into(),
        ..Default::default()
    };
    let png = image(image::ImageFormat::Png, 20);
    docs::save(
        &mut book,
        actor,
        c,
        "Sample seller",
        "USD",
        docs::Changes {
            details: details.clone(),
            logo_upload: Some(png.clone()),
            remove_logo: false,
        },
    )
    .unwrap();
    let logo = book.read(|tx| docs::logo(tx, c)).unwrap();
    assert!(logo.is_some());
    assert!(book.read(|tx| docs::logo_json(tx, other, c)).is_err());
    assert!(
        docs::save(
            &mut book,
            other,
            c,
            "Other seller",
            "USD",
            docs::Changes {
                details: Details::default(),
                logo_upload: None,
                remove_logo: true
            }
        )
        .is_err()
    );
    // A company-name failure must roll back the details and logo too.
    assert!(
        docs::save(
            &mut book,
            actor,
            c,
            "",
            "USD",
            docs::Changes {
                details: Details::default(),
                logo_upload: None,
                remove_logo: true
            }
        )
        .is_err()
    );
    assert_eq!(book.read(|tx| docs::get(tx, c)).unwrap(), details);
    assert_eq!(book.read(|tx| docs::logo(tx, c)).unwrap(), logo);
    let before = fb_db::test_support::snapshot(book.connection()).unwrap();
    book.connection().execute_batch("CREATE TRIGGER fail_document_audit BEFORE INSERT ON ledger_auditevent WHEN NEW.action='company.document_details_updated' BEGIN SELECT RAISE(ABORT,'injected final audit failure'); END").unwrap();
    assert!(
        docs::save(
            &mut book,
            actor,
            c,
            "Must roll back",
            "USD",
            docs::Changes {
                details: Details::default(),
                logo_upload: None,
                remove_logo: true
            }
        )
        .is_err()
    );
    assert_eq!(
        fb_db::test_support::snapshot(book.connection()).unwrap(),
        before
    );
    book.connection()
        .execute_batch("DROP TRIGGER fail_document_audit")
        .unwrap();
    docs::save(
        &mut book,
        actor,
        c,
        "Renamed seller",
        "USD",
        docs::Changes {
            details: details.clone(),
            logo_upload: None,
            remove_logo: false,
        },
    )
    .unwrap();
    assert_eq!(book.read(|tx| docs::logo(tx, c)).unwrap(), logo);
    let audit:String=book.read(|tx| tx.query_row("SELECT detail FROM ledger_auditevent WHERE action='company.document_details_updated'",[],|r|r.get(0))).unwrap();
    assert!(audit.contains("sha256") && !audit.contains(&png));
    drop(book);
    let mut reopened = Book::open(&path, &key).unwrap();
    assert_eq!(reopened.read(|tx| docs::get(tx, c)).unwrap(), details);
    assert_eq!(reopened.read(|tx| docs::logo(tx, c)).unwrap(), logo);
    docs::save(
        &mut reopened,
        actor,
        c,
        "Renamed seller",
        "USD",
        docs::Changes {
            details: Details::default(),
            logo_upload: None,
            remove_logo: true,
        },
    )
    .unwrap();
    assert_eq!(
        reopened.read(|tx| docs::get(tx, c)).unwrap(),
        Details::default()
    );
    assert!(reopened.read(|tx| docs::logo(tx, c)).unwrap().is_none());
    assert!(reopened.check().unwrap().is_empty());
}

#[test]
fn company_document_identity_cannot_move_delete_or_be_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let mut book = Book::create(&dir.path().join("identity.fbbook"), &[52; 32]).unwrap();
    let actor = users::create_user(&mut book, "Synthetic owner", true).unwrap();
    let c = services::create_company(&mut book, actor, "Seller", "USD", false)
        .unwrap()
        .id;
    let other = services::create_company(&mut book, actor, "Other", "USD", false)
        .unwrap()
        .id;
    docs::save(
        &mut book,
        actor,
        c,
        "Seller",
        "USD",
        docs::Changes {
            details: Details {
                street1: "Original".into(),
                ..Default::default()
            },
            logo_upload: None,
            remove_logo: false,
        },
    )
    .unwrap();
    let before = fb_db::test_support::snapshot(book.connection()).unwrap();
    for recursion in [false, true] {
        book.connection()
            .execute_batch(&format!(
                "PRAGMA recursive_triggers={}",
                i32::from(recursion)
            ))
            .unwrap();
        for sql in [
            format!(
                "UPDATE company_document_details SET company_id='{other}' WHERE company_id='{c}'"
            ),
            format!(
                "UPDATE OR REPLACE company_document_details SET company_id='{other}' WHERE company_id='{c}'"
            ),
            format!("DELETE FROM company_document_details WHERE company_id='{c}'"),
            format!(
                "INSERT OR REPLACE INTO company_document_details SELECT company_id,data,logo_mime,logo FROM company_document_details WHERE company_id='{c}'"
            ),
        ] {
            book.connection()
                .execute_batch("SAVEPOINT identity_probe")
                .unwrap();
            let result = book.connection().execute_batch(&sql);
            book.connection()
                .execute_batch("ROLLBACK TO identity_probe; RELEASE identity_probe")
                .unwrap();
            let error = result.expect_err(&sql);
            assert_eq!(
                fb_db::constraint_of(&error).as_deref(),
                Some("company_documents_identity"),
                "{sql}: {error}"
            );
            assert_eq!(
                fb_db::test_support::snapshot(book.connection()).unwrap(),
                before
            );
        }
    }
    book.connection()
        .execute_batch("PRAGMA recursive_triggers=ON")
        .unwrap();
    // Ordinary updates and clearing fields keep the same permanent company row.
    docs::save(
        &mut book,
        actor,
        c,
        "Seller",
        "USD",
        docs::Changes {
            details: Details::default(),
            logo_upload: None,
            remove_logo: true,
        },
    )
    .unwrap();
    assert_eq!(book.read(|q| docs::get(q, c)).unwrap(), Details::default());
    assert!(book.check().unwrap().is_empty());
}

#[test]
fn company_document_writer_masks_every_field_and_actor_audit_after_composition() {
    let dir = tempfile::tempdir().unwrap();
    let mut book = Book::create(&dir.path().join("mask.fbbook"), &[53; 32]).unwrap();
    let actor = users::create_user(&mut book, "Synthetic owner", true).unwrap();
    let c = services::create_company(&mut book, actor, "Seller", "USD", false)
        .unwrap()
        .id;
    let secret = "SyntheticPassword42";
    let secrets = fb_db::credential_mask::SecretSet::new([secret]).unwrap();
    book.retain_bankfeed_mask(&secrets).unwrap();
    let mut value = serde_json::to_value(Details::default()).unwrap();
    for &(field, _, _) in docs::FIELDS {
        value[field] = serde_json::json!(if field == "email" {
            format!("{secret}@example.test")
        } else {
            secret.into()
        });
    }
    docs::save(
        &mut book,
        actor,
        c,
        secret,
        "USD",
        docs::Changes {
            details: serde_json::from_value(value).unwrap(),
            logo_upload: None,
            remove_logo: false,
        },
    )
    .unwrap();
    let stored = serde_json::to_value(book.read(|q| docs::get(q, c)).unwrap()).unwrap();
    for &(field, _, _) in docs::FIELDS {
        assert!(
            stored[field]
                .as_str()
                .unwrap()
                .contains(fb_db::credential_mask::MASK),
            "{field}"
        );
    }
    let (audit_actor, audit): (String,String) = book.connection().query_row(
        "SELECT actor_id,detail FROM ledger_auditevent WHERE action='company.document_details_updated'", [], |r| Ok((r.get(0)?,r.get(1)?))).unwrap();
    assert_eq!(audit_actor, actor.to_db());
    assert!(!audit.contains(secret));
    assert!(audit.contains(fb_db::credential_mask::MASK));
    let snapshot = format!(
        "{:?}",
        fb_db::test_support::snapshot(book.connection()).unwrap()
    );
    assert!(
        !snapshot.contains(secret),
        "unmasked company detail, composed audit or name persisted"
    );
    assert!(book.check().unwrap().is_empty());
}

#[test]
fn company_document_reader_refuses_same_value_escaped_duplicate_keys() {
    let dir = tempfile::tempdir().unwrap();
    let mut book = Book::create(&dir.path().join("duplicate.fbbook"), &[54; 32]).unwrap();
    let actor = users::create_user(&mut book, "Synthetic owner", true).unwrap();
    let c = services::create_company(&mut book, actor, "Seller", "USD", false)
        .unwrap()
        .id;
    let details = Details {
        street1: "Original".into(),
        ..Default::default()
    };
    docs::save(
        &mut book,
        actor,
        c,
        "Seller",
        "USD",
        docs::Changes {
            details: details.clone(),
            logo_upload: None,
            remove_logo: false,
        },
    )
    .unwrap();
    book.connection()
        .execute_batch("DROP TRIGGER IF EXISTS fb_json_company_document_details_update")
        .unwrap();
    let raw =
        serde_json::to_string(&details)
            .unwrap()
            .replacen("{", r#"{"\u0073treet1":"Original","#, 1);
    book.connection()
        .execute(
            "UPDATE company_document_details SET data=?1 WHERE company_id=?2",
            [&raw, c.to_db().as_str()],
        )
        .unwrap();
    assert!(
        book.read(|q| docs::get(q, c)).is_err(),
        "typed read discarded an escaped duplicate key"
    );
}
