use super::*;
use std::collections::BTreeSet;

const COMPANY: &str = "00000000-0000-7000-8000-000000000001";
const DETAILS: &str = r#"{"street1":"Prior street","street2":"","city":"","region":"","postal_code":"","country":"","phone":"","email":"","website":"","tax_id":"","payment_instructions":"Prior payment instructions"}"#;

fn decoded_attacks() -> Vec<(String, String)> {
    let mut cases = Vec::new();
    for (field, max) in [("street1",200),("street2",200),("city",100),("region",100),
        ("postal_code",32),("country",100),("phone",50),("email",254),("website",500),
        ("tax_id",100),("payment_instructions",2000)] {
        let mut value: serde_json::Value = serde_json::from_str(DETAILS).unwrap();
        value[field] = serde_json::json!(format!("A\0{}", "X".repeat(max+1)));
        cases.push((format!("escaped NUL and long suffix in {field}"),value.to_string()));
    }
    let mut unknown: serde_json::Value=serde_json::from_str(DETAILS).unwrap();
    unknown["probe"]=serde_json::json!({});
    cases.push(("unknown key".into(),unknown.to_string()));
    cases
}

#[test]
fn company_document_decoded_json_write_guards_refuse_escaped_nul_and_unknown_keys() {
    let dir=tempfile::tempdir().unwrap();
    let book=Book::create(&dir.path().join("decoded.fbbook"),&[58;32]).unwrap();
    let q=&book.conn;
    q.execute("INSERT INTO ledger_company(id,name,currency) VALUES(?1,'Synthetic seller','USD')",[COMPANY]).unwrap();
    let mut failures=Vec::new();
    for (label,data) in decoded_attacks() {
        for update in [false,true] {
            q.execute_batch("SAVEPOINT decoded_case").unwrap();
            if update { q.execute("INSERT INTO company_document_details(company_id,data) VALUES(?1,?2)",[COMPANY,DETAILS]).unwrap(); }
            let result=if update { q.execute("UPDATE company_document_details SET data=?2 WHERE company_id=?1",[COMPANY,&data]) }
                else { q.execute("INSERT INTO company_document_details(company_id,data) VALUES(?1,?2)",[COMPANY,&data]) };
            q.execute_batch("ROLLBACK TO decoded_case; RELEASE decoded_case").unwrap();
            if !result.as_ref().is_err_and(|e|constraint_of(e).as_deref()==Some("company_documents_data")) {
                failures.push(format!("{label} update={update}: {result:?}"));
            }
        }
    }
    assert!(failures.is_empty(),"{}",failures.join("\n"));
}

#[test]
fn company_document_corrupt_decoded_json_is_detected_before_reopen() {
    for (_,data) in decoded_attacks().into_iter().filter(|(label,_)|label.contains("street1")||label=="unknown key") {
        let dir=tempfile::tempdir().unwrap();let path=dir.path().join("corrupt.fbbook");let key=[59;32];
        let book=Book::create(&path,&key).unwrap();let q=&book.conn;
        q.execute("INSERT INTO ledger_company(id,name,currency) VALUES(?1,'Synthetic seller','USD')",[COMPANY]).unwrap();
        q.execute("INSERT INTO company_document_details(company_id,data) VALUES(?1,?2)",[COMPANY,DETAILS]).unwrap();
        let guards:Vec<(String,String)>=collect(q,"SELECT name,sql FROM sqlite_schema WHERE type='trigger' AND tbl_name='company_document_details'",|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
        for (name,_) in &guards {q.execute_batch(&format!("DROP TRIGGER {name}")).unwrap();}
        q.execute_batch("PRAGMA ignore_check_constraints=ON").unwrap();
        q.execute("UPDATE company_document_details SET data=?2 WHERE company_id=?1",[COMPANY,&data]).unwrap();
        q.execute_batch("PRAGMA ignore_check_constraints=OFF").unwrap();
        for (_,sql) in guards {q.execute_batch(&sql).unwrap();}
        let invalid:i64=q.query_row("SELECT count(*) FROM fb_value_invalid WHERE problem LIKE '%company_document_details%'",[],|r|r.get(0)).unwrap();
        assert_eq!(invalid,1,"decoded corruption must be visible to the value view");
        let invalid:i64=q.query_row("SELECT count(*) FROM fb_json_invalid WHERE problem LIKE 'company_document_details.data:%'",[],|r|r.get(0)).unwrap();
        assert_eq!(invalid,1,"decoded corruption must be visible to the JSON view");
        drop(book);
        assert!(matches!(Book::open(&path,&key),Err(DbError::Integrity(_))));
    }
}

fn upgrade_at(boundary_name: &str) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("prior.fbbook");
    let key = [55; 32];
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(&format!("PRAGMA key=\"x'{}'\"", hex(&key)))
        .unwrap();
    Book::configure(&conn).unwrap();
    let boundary = MIGRATIONS
        .iter()
        .position(|m| m.0 == boundary_name)
        .unwrap();
    for (_, sql) in &MIGRATIONS[..boundary] {
        conn.execute_batch(sql).unwrap();
    }
    conn.execute_batch(&format!("PRAGMA user_version={boundary}; INSERT INTO ledger_company(id,name,currency) VALUES('00000000-0000-7000-8000-000000000001','Synthetic prior seller','USD')")).unwrap();
    if boundary_name != "0180_company_documents" {
        conn.execute(
            "INSERT INTO company_document_details(company_id,data) VALUES(?1,?2)",
            [COMPANY, DETAILS],
        )
        .unwrap();
    }
    let before = test_support::snapshot(&conn).unwrap();
    let schema = schema_hashes(&conn).unwrap();
    let views = ["fb_value_invalid", "fb_json_invalid"].map(|name| {
        let sql: String = conn
            .query_row("SELECT sql FROM sqlite_schema WHERE name=?1", [name], |r| {
                r.get(0)
            })
            .unwrap();
        (name, sql)
    });
    drop(conn);
    let book = Book::open(&path, &key).unwrap();
    let after = test_support::snapshot(&book.conn).unwrap();
    for (table, rows) in &before {
        assert_eq!(after[table], *rows, "changed prior table {table}");
    }
    assert_eq!(
        after.len(),
        before.len() + usize::from(boundary_name == "0180_company_documents")
    );
    if boundary_name == "0180_company_documents" {
        assert!(after["company_document_details"].is_empty());
    }
    let upgraded = schema_hashes(&book.conn).unwrap();
    for (object, hash) in schema {
        if !["view:fb_value_invalid", "view:fb_json_invalid"].contains(&object.as_str()) {
            assert_eq!(upgraded[&object], hash, "changed prior schema {object}");
        }
    }
    // Extended views retain every prior arm; all prior triggers above remain byte-exact.
    for (name, prior) in views {
        let current: String = book
            .conn
            .query_row("SELECT sql FROM sqlite_schema WHERE name=?1", [name], |r| {
                r.get(0)
            })
            .unwrap();
        let arms = |sql: &str| -> BTreeSet<String> {
            sql.split_once(" AS\n")
                .unwrap()
                .1
                .split("UNION ALL")
                .map(|s| s.trim().trim_end_matches(';').to_string())
                .collect()
        };
        let prior = arms(&prior);
        let current = arms(&current);
        // The owned decoded-data suffix is appended at 0182, then gated at 0183
        // to leave malformed/duplicate JSON with its original error owner. Compare
        // all original predicates and every unrelated arm byte-for-byte.
        let original_predicates = |arms: &BTreeSet<String>| -> BTreeSet<String> {
            arms.iter().map(|arm| {
                // The two reviewed generators emitted this identical three-predicate
                // vault arm in different OR order. Translate only that exact arm;
                // no predicate or other table is normalized away.
                let numbering_vault = "SELECT 'invalid stored value in plugin_credential' AS problem FROM plugin_credential WHERE\n NOT fb_valid_uuid(company_id) OR\n NOT fb_valid_uuid(connection_id) OR\n NOT fb_valid_text(plugin_id)";
                let document_vault = "SELECT 'invalid stored value in plugin_credential' AS problem FROM plugin_credential WHERE\n NOT fb_valid_uuid(company_id) OR\n NOT fb_valid_text(plugin_id) OR\n NOT fb_valid_uuid(connection_id)";
                if arm == numbering_vault { return document_vault.to_owned(); }
                if arm.contains("FROM company_document_details WHERE") {
                    arm.strip_suffix(" OR (fb_json_unique_keys(data) AND NOT fb_company_document_valid(data))")
                        .or_else(|| arm.strip_suffix(" OR NOT fb_company_document_valid(data)"))
                        .unwrap_or(arm).to_owned()
                } else { arm.clone() }
            }).collect()
        };
        let prior_predicates = original_predicates(&prior);
        let current_predicates = original_predicates(&current);
        assert!(
            prior_predicates.is_subset(&current_predicates),
            "lost {name} arms: {:?}",
            prior_predicates.difference(&current_predicates).collect::<Vec<_>>()
        );
        assert_eq!(current.len(), prior.len() + usize::from(
            name == "fb_value_invalid" || ["0180_company_documents", "0181_company_document_guards"].contains(&boundary_name)));
    }
    let fresh = Book::create(&dir.path().join("fresh.fbbook"), &key).unwrap();
    assert_eq!(
        schema_hashes(&book.conn).unwrap(),
        schema_hashes(&fresh.conn).unwrap()
    );
    assert!(book.check().unwrap().is_empty());
    drop(book);
    let reopened = Book::open(&path, &key).unwrap();
    assert_eq!(test_support::snapshot(&reopened.conn).unwrap(), after);
    assert!(reopened.check().unwrap().is_empty());
    assert!(reopened.conn.execute("INSERT INTO company_document_details(company_id,data) VALUES('00000000-0000-7000-8000-000000000002','{}')",[]).is_err(),"required string fields cannot be missing");
}

#[test]
fn company_document_migration_preserves_every_prior_table_and_schema_object() {
    upgrade_at("0180_company_documents");
}

#[test]
fn company_document_guard_upgrade_preserves_populated_source_book_and_reopens() {
    upgrade_at("0181_company_document_guards");
}

#[test]
fn company_document_each_text_value_guard_refuses_insert_update_and_detects_corruption_on_open() {
    for (column, value) in [
        ("company_id", format!("'{COMPANY}'||char(0)")),
        ("data", "'{}'||char(0)".into()),
        ("logo_mime", "'image/png'||char(0)".into()),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("value.fbbook");
        let key = [56; 32];
        let book = Book::create(&path, &key).unwrap();
        let q = &book.conn;
        let definitions: Vec<(String, String)> = collect(
            q,
            "SELECT name,sql FROM sqlite_schema WHERE type='trigger'",
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
        for (name, _) in &definitions {
            if ![
                "fb_valid_company_document_details_insert",
                "fb_valid_company_document_details_update",
            ]
            .contains(&name.as_str())
            {
                q.execute_batch(&format!("DROP TRIGGER {name}")).unwrap();
            }
        }
        q.execute_batch("PRAGMA foreign_keys=OFF; PRAGMA ignore_check_constraints=ON;")
            .unwrap();
        let expression = |c: &str, default: &str| {
            if c == column {
                value.clone()
            } else {
                default.into()
            }
        };
        let insert = format!(
            "INSERT INTO company_document_details(company_id,data,logo_mime) VALUES({},{},{})",
            expression("company_id", &format!("'{COMPANY}'")),
            expression("data", "'{}'"),
            expression("logo_mime", "NULL")
        );
        let error = q.execute_batch(&insert).unwrap_err();
        assert_eq!(
            constraint_of(&error).as_deref(),
            Some("fb_valid"),
            "insert {column}: {error}"
        );
        q.execute(
            "INSERT INTO company_document_details(company_id,data) VALUES(?1,?2)",
            [COMPANY, DETAILS],
        )
        .unwrap();
        let update = format!("UPDATE company_document_details SET {column}={value}");
        let error = q.execute_batch(&update).unwrap_err();
        assert_eq!(
            constraint_of(&error).as_deref(),
            Some("fb_valid"),
            "update {column}: {error}"
        );
        q.execute_batch("DROP TRIGGER fb_valid_company_document_details_insert; DROP TRIGGER fb_valid_company_document_details_update;").unwrap();
        q.execute_batch(&update).unwrap();
        for (_, sql) in definitions {
            q.execute_batch(&sql).unwrap();
        }
        let problems: Vec<String> =
            collect(q, "SELECT problem FROM fb_value_invalid", |r| r.get(0)).unwrap();
        assert_eq!(
            problems,
            ["invalid stored value in company_document_details"]
        );
        drop(book);
        match Book::open(&path, &key) {
            Err(DbError::Integrity(problems)) => assert!(
                problems
                    .iter()
                    .any(|p| p.contains("invalid stored value in company_document_details")),
                "{column}: {problems:?}"
            ),
            result => panic!("{column}: {:?}", result.err()),
        }
    }
}

#[test]
fn company_document_decoded_schema_upgrade_preserves_populated_previous_book() {
    upgrade_at("0182_company_document_data");
}

#[test]
fn company_document_json_ownership_upgrade_preserves_populated_previous_book() {
    upgrade_at("0183_company_document_json_ownership");
}

#[test]
fn company_document_malformed_and_duplicate_json_keep_their_error_owner() {
    for data in [r#"{"probe":{"value":1,"\u0076alue":1}}"#, "{"] {
        let dir=tempfile::tempdir().unwrap();let path=dir.path().join("json-owner.fbbook");let key=[60;32];
        let book=Book::create(&path,&key).unwrap();let q=&book.conn;
        q.execute("INSERT INTO ledger_company(id,name,currency) VALUES(?1,'Synthetic seller','USD')",[COMPANY]).unwrap();
        q.execute("INSERT INTO company_document_details(company_id,data) VALUES(?1,?2)",[COMPANY,DETAILS]).unwrap();
        let guards:Vec<(String,String)>=collect(q,"SELECT name,sql FROM sqlite_schema WHERE type='trigger' AND tbl_name='company_document_details'",|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
        for (name,_) in &guards {q.execute_batch(&format!("DROP TRIGGER {name}")).unwrap();}
        q.execute_batch("PRAGMA ignore_check_constraints=ON").unwrap();
        q.execute("UPDATE company_document_details SET data=?2 WHERE company_id=?1",[COMPANY,data]).unwrap();
        q.execute_batch("PRAGMA ignore_check_constraints=OFF").unwrap();
        for (_,sql) in guards {q.execute_batch(&sql).unwrap();}
        let invalid:i64=q.query_row("SELECT count(*) FROM fb_value_invalid WHERE problem LIKE '%company_document_details%'",[],|r|r.get(0)).unwrap();
        assert_eq!(invalid,0,"JSON syntax/duplicates must not acquire a stored-text error");
        let invalid:i64=q.query_row("SELECT count(*) FROM fb_json_invalid WHERE problem LIKE 'company_document_details.data:%'",[],|r|r.get(0)).unwrap();
        assert_eq!(invalid,1,"JSON syntax/duplicates remain refused by the JSON view");
        drop(book);
        match Book::open(&path,&key) {
            Err(DbError::Integrity(problems)) => {
                assert!(problems.iter().any(|p|p.starts_with("ambiguous stored JSON: ")),"{problems:?}");
                assert!(!problems.iter().any(|p|p.starts_with("invalid stored text, identifier, date or timestamp")),"{problems:?}");
            }
            other => panic!("corrupt JSON reopened: {:?}",other.err()),
        }
    }
}

#[test]
fn company_document_combined_guard_union_preserves_populated_previous_book() {
    upgrade_at("0184_release_guard_union");
}
