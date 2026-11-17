use super::*;

/// The open-time check keeps no list of required trigger or index names. It compares every schema
/// object with what the migrations create, so removing or adding any one of them is a problem.
#[test]
fn removing_any_trigger_or_index_or_adding_one_is_a_problem_at_open() {
    let dir = tempfile::tempdir().unwrap();
    let book = Book::create(&dir.path().join("schema.fbbook"), &[77; 32]).unwrap();
    assert!(book.check().unwrap().is_empty());
    let objects: Vec<(String, String)> = book
        .conn
        .prepare("SELECT type, name FROM sqlite_master WHERE type IN ('trigger','index') AND sql IS NOT NULL AND name NOT GLOB 'sqlite_*' ORDER BY type, name")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(objects.iter().filter(|(kind, _)| kind == "trigger").count() > 500, "the schema's guards are all examined");
    for (kind, name) in &objects {
        book.conn.execute_batch(&format!("SAVEPOINT one; DROP {kind} \"{name}\";")).unwrap();
        let problems = schema_problems(&book.conn, MIGRATIONS.len()).unwrap();
        book.conn.execute_batch("ROLLBACK TO one; RELEASE one;").unwrap();
        assert!(problems.contains(&format!("schema definition mismatch: {kind}:{name}")), "{kind} {name}: {problems:?}");
    }
    // The whole open-time check reports the same thing, not only the schema comparison on its own.
    let (kind, name) = objects.iter().find(|(kind, _)| kind == "trigger").unwrap();
    book.conn.execute_batch(&format!("SAVEPOINT one; DROP {kind} \"{name}\";")).unwrap();
    let problems = book.check().unwrap();
    book.conn.execute_batch("ROLLBACK TO one; RELEASE one;").unwrap();
    assert!(problems.contains(&format!("schema definition mismatch: {kind}:{name}")), "{problems:?}");
    // An object the migrations do not create is refused as well.
    book.conn.execute_batch("SAVEPOINT one; CREATE TRIGGER fb_test_extra AFTER INSERT ON ledger_company BEGIN SELECT 1; END;").unwrap();
    let problems = schema_problems(&book.conn, MIGRATIONS.len()).unwrap();
    book.conn.execute_batch("ROLLBACK TO one; RELEASE one;").unwrap();
    assert!(problems.contains(&"unexpected schema object: trigger:fb_test_extra".to_string()), "{problems:?}");
    assert!(book.check().unwrap().is_empty());
}
