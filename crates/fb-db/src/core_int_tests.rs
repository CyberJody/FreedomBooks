use super::*;

fn book(label: &str) -> Book {
    Book::create(&tests::temp(label), &[31; 32]).unwrap()
}

#[test]
fn portable_functions_and_collation_are_ready_at_open_under_both_callback_policies() {
    let path = tests::temp("core-int-registration");
    let key = [32; 32];
    let mut book = Book::create(&path, &key).unwrap();
    for reopened in [false, true] {
        if reopened {
            drop(book);
            book = Book::open(&path, &key).unwrap();
        }
        let query = "SELECT fb_upper('ıKé'),fb_lower('IKÉ'),group_concat(value,'|') FROM (SELECT 'B' AS value UNION ALL SELECT 'a' UNION ALL SELECT 'A' ORDER BY value COLLATE fb_text)";
        let check = |actual: (String, String, String)| {
            assert_eq!(actual, ("IKÉ".into(), "iké".into(), "a|A|B".into()));
        };
        book.read(|reader| -> rusqlite::Result<()> {
            check(reader.query_row(query, [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?);
            assert!(reader.execute_batch("PRAGMA query_only=OFF").is_err());
            Ok(())
        })
        .unwrap();
        book.write(|writer| -> rusqlite::Result<()> {
            check(writer.query_row(query, [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?);
            // Exercise expression indexes, triggers and UDFs through an authorized Writer.
            if !reopened { return Ok(()); }
            writer.execute_batch("INSERT INTO ledger_company(id,name,currency) VALUES('00000000-0000-7000-8000-000000000001','Company','USD');
              INSERT INTO directory_item(id,company_id,code,kind,sale_rate) VALUES('00000000-0000-7000-8000-0000000000a1','00000000-0000-7000-8000-000000000001','ı','SERVICE',1);
              INSERT INTO salestax_taxitem(id,company_id,kind,name,description) VALUES('00000000-0000-7000-8000-0000000000b1','00000000-0000-7000-8000-000000000001','GROUP','K','Tax');")?;
            assert!(writer.execute_batch("INSERT INTO directory_item(id,company_id,code,kind,sale_rate) VALUES('00000000-0000-7000-8000-0000000000a2','00000000-0000-7000-8000-000000000001','I','SERVICE',1)").is_ok());
            assert!(writer.execute_batch("INSERT INTO directory_item(id,company_id,code,kind,sale_rate) VALUES('00000000-0000-7000-8000-0000000000a3','00000000-0000-7000-8000-000000000001','i','SERVICE',1)").is_err());
            assert!(writer.execute_batch("INSERT INTO salestax_taxitem(id,company_id,kind,name,description) VALUES('00000000-0000-7000-8000-0000000000b2','00000000-0000-7000-8000-000000000001','GROUP','K','Duplicate')").is_err());
            Ok(())
        }).unwrap();
    }
}

#[test]
fn library_owned_nested_atomics_keep_authorization_and_cleanup_on_errors_and_panics() {
    let mut book = book("core-int-atomic");
    book.write(|writer| -> rusqlite::Result<()> {
        writer.execute_batch("INSERT INTO fb_seal VALUES('outer','kept')")?;
        writer.atomic(|| -> rusqlite::Result<()> {
            for sql in [
                "COMMIT",
                "ROLLBACK",
                "BEGIN",
                "SAVEPOINT x",
                "RELEASE fb_library_primitive",
                "ROLLBACK TO fb_library_primitive",
                "PRAGMA recursive_triggers=OFF",
                "PRAGMA writable_schema=ON",
                "PRAGMA query_only=OFF",
                "CREATE TEMP TABLE escape(x)",
                "ATTACH ':memory:' AS escape",
            ] {
                assert!(writer.execute_batch(sql).is_err(), "allowed {sql}");
            }
            writer.execute_batch("INSERT INTO fb_seal VALUES('nested','kept')")?;
            let error = writer.atomic(|| -> rusqlite::Result<()> {
                writer.execute_batch("INSERT INTO fb_seal VALUES('error','discarded')")?;
                Err(rusqlite::Error::InvalidQuery)
            });
            assert!(error.is_err());
            let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _: rusqlite::Result<()> = writer.atomic(|| {
                    writer.execute_batch("INSERT INTO fb_seal VALUES('panic','discarded')")?;
                    panic!("injected primitive panic");
                });
            }));
            assert!(panic.is_err());
            assert!(writer.execute_batch("COMMIT").is_err());
            Ok(())
        })?;
        writer.execute_batch("INSERT INTO fb_seal VALUES('after','kept')")?;
        Ok(())
    })
    .unwrap();
    book.read(|reader| -> rusqlite::Result<()> {
        let rows = reader
            .prepare("SELECT kind FROM fb_seal ORDER BY kind")?
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(rows, ["after", "nested", "outer"]);
        Ok(())
    })
    .unwrap();
}

#[test]
fn l2a_tables_indexes_views_and_exact_prefix_objects_are_hashed_on_check_and_reopen() {
    for name in [
        "directory_customer",
        "directory_vendor",
        "directory_item",
        "salestax_taxitem",
        "salestax_taxgroupmember",
        "salestax_salestaxsettings",
        "documents_documentsequence",
        "documents_documentidentity",
        "documents_documentaccount",
        "documents_accountingsetup",
        "documents_designatedbank",
        "item_code_ci_per_company",
        "salestax_item_name_ci_per_company",
        "journal_original_per_source",
        "fb_control_accounts",
    ] {
        let path = tests::temp(&format!("l2a-definition-{name}"));
        let key = [33; 32];
        let book = Book::create(&path, &key).unwrap();
        // Simulate an external same-name definition edit. Even whitespace must
        // not be blessed as the expected migration definition.
        book.conn
            .execute_batch("PRAGMA writable_schema=ON")
            .unwrap();
        assert_eq!(
            book.conn
                .execute(
                    "UPDATE sqlite_master SET sql=sql||' ' WHERE name=?1",
                    [name]
                )
                .unwrap(),
            1
        );
        book.conn
            .execute_batch("PRAGMA writable_schema=OFF")
            .unwrap();
        assert!(
            book.check().unwrap().iter().any(|p| p.contains(name)),
            "{name}"
        );
        drop(book);
        assert!(
            matches!(Book::open(&path, &key), Err(DbError::Integrity(_))),
            "{name}"
        );
    }
    for name in ["sqliteXl2a", "sqlite-l2a", "SQLiteXl2a", "SQLITE-l2a"] {
        let path = tests::temp(name);
        let key = [34; 32];
        let book = Book::create(&path, &key).unwrap();
        book.conn.execute_batch(&format!("CREATE TRIGGER \"{name}\" BEFORE INSERT ON directory_customer BEGIN SELECT RAISE(IGNORE); END")).unwrap();
        assert!(
            book.check().unwrap().iter().any(|p| p.contains(name)),
            "{name}"
        );
        drop(book);
        assert!(
            matches!(Book::open(&path, &key), Err(DbError::Integrity(_))),
            "{name}"
        );
    }
}
