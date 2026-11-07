//! Independent, schema-inventoried ambiguity probes for every stored JSON column.
use super::*;
use rusqlite::{params, params_from_iter, types::Value};

#[test]
fn every_stored_json_column_refuses_duplicate_keys_at_write_read_seal_and_open() {
    let dir = tempfile::tempdir().unwrap();
    let probe = Book::create(&dir.path().join("inventory.fbbook"), &[93; 32]).unwrap();
    let mut actual = Vec::new();
    let tables: Vec<(String,String)> = probe.connection().prepare("SELECT name,sql FROM sqlite_master WHERE type='table'").unwrap()
        .query_map([], |r|Ok((r.get(0)?,r.get(1)?))).unwrap().collect::<Result<_,_>>().unwrap();
    for (table,sql) in tables {
        // Every JSON CHECK in the real schema must have a shared boundary.
        let mut rest=sql.as_str();
        while let Some(i)=rest.find("json_valid(") {
            rest=&rest[i+11..];
            let column=rest.split(')').next().unwrap().trim();
            actual.push((table.clone(),column.to_owned()));
        }
    }
    actual.sort(); actual.dedup();
    assert_eq!(actual, json_evidence::STORED_JSON_COLUMNS.iter().map(|(t,c)|(t.to_string(),c.to_string())).collect::<Vec<_>>());
    drop(probe);
    for (index, &(table,column)) in json_evidence::STORED_JSON_COLUMNS.iter().enumerate() {
        let path=dir.path().join(format!("column-{index}.fbbook"));
        let book=Book::create(&path,&[93;32]).unwrap();
        let q=book.connection();
        let duplicate=r#"{"probe":{"value":1,"\u0076alue":1}}"#;
        let guard=format!("fb_json_{table}_insert");
        // Isolate our boundary from all module logic, NOT NULLs and foreign keys.
        let definitions:Vec<(String,String)>=q.prepare("SELECT name,sql FROM sqlite_master WHERE type='trigger'").unwrap()
            .query_map([],|r|Ok((r.get(0)?,r.get(1)?))).unwrap().collect::<Result<_,_>>().unwrap();
        for (name,_) in &definitions { if name!=&guard {q.execute_batch(&format!("DROP TRIGGER {name}")).unwrap();} }
        let json_columns:Vec<_>=json_evidence::STORED_JSON_COLUMNS.iter().filter(|(t,_)| *t==table).map(|(_,c)|*c).collect();
        let json_values:Vec<_>=json_columns.iter().map(|c|if *c==column {duplicate}else{"{}"}).collect();
        let error=q.execute(&format!("INSERT INTO {table}({}) VALUES({})",json_columns.join(","),vec!["?";json_columns.len()].join(",")),params_from_iter(json_values)).unwrap_err();
        assert_eq!(constraint_of(&error).as_deref(),Some("duplicate_json"),"{table}.{column}: {error}");
        q.execute_batch(&format!("DROP TRIGGER {guard}; PRAGMA foreign_keys=OFF; PRAGMA ignore_check_constraints=ON;")).unwrap();
        let columns:Vec<(String,String)>=q.prepare(&format!("PRAGMA table_info({table})")).unwrap().query_map([],|r|Ok((r.get(1)?,r.get(2)?))).unwrap().collect::<Result<_,_>>().unwrap();
        // Other columns get values of their 0130 value class (read from this table's own fb_valid guard), so the
        // only stored defect is the duplicate JSON key and open is refused for that alone.
        let value_guard=definitions.iter().find(|(name,_)| *name==format!("fb_valid_{table}_insert")).map(|(_,sql)|sql.as_str()).unwrap_or("");
        let class=|name:&str,f:&str| value_guard.contains(&format!("{f}(NEW.{name})"));
        let values:Vec<Value>=columns.iter().enumerate().map(|(i,(name,kind))| if name==column {Value::Text(duplicate.into())} else if kind=="INTEGER" {Value::Integer(1)} else if json_evidence::STORED_JSON_COLUMNS.contains(&(table,name.as_str())) {Value::Text("{}".into())}
            else if class(name,"fb_valid_uuid") {Value::Text(format!("00000000-0000-7000-8000-{:012x}",0x0f00_0000+i))} // synthetic row id / reference
            else if class(name,"fb_valid_date") {Value::Text("2026-01-01".into())}
            else if class(name,"fb_valid_timestamp") {Value::Text("2026-01-01T00:00:00.000000Z".into())}
            else {Value::Text("synthetic".into())}).collect();
        q.execute(&format!("INSERT INTO {table} VALUES({})",vec!["?";values.len()].join(",")),params_from_iter(values)).unwrap();
        // The UPDATE boundary must independently reject the same recursive escaped duplicate.
        let update_guard = format!("fb_json_{table}_update");
        let update_sql = &definitions.iter().find(|(name,_)| name == &update_guard).unwrap().1;
        q.execute_batch(update_sql).unwrap();
        let error = q.execute(&format!("UPDATE {table} SET {column}=?1"), [duplicate]).unwrap_err();
        assert_eq!(constraint_of(&error).as_deref(),Some("duplicate_json"),"update {table}.{column}: {error}");
        q.execute_batch(&format!("DROP TRIGGER {update_guard}")).unwrap();
        for (_,sql) in &definitions {q.execute_batch(sql).unwrap();}
        q.execute_batch("PRAGMA ignore_check_constraints=OFF; PRAGMA foreign_keys=ON").unwrap();
        let raw:String=q.query_row(&format!("SELECT {column} FROM {table}"),[],|r|r.get(0)).unwrap();
        assert!(parse_unique_json(&raw).is_err(),"{table}.{column}");
        let problem:String=q.query_row("SELECT problem FROM fb_json_invalid WHERE problem LIKE ?",[format!("{table}.{column}:%")],|r|r.get(0)).unwrap();
        assert!(problem.starts_with(&format!("{table}.{column}:")));
        for seal in ["fb_seal","sales_seal","credits_seal","prepayments_seal","banking_seal","estimate_seal"] {
            let error=q.execute(&format!("INSERT INTO {seal}(kind,doc_id) VALUES(?,?)"),params!["probe","probe"]).unwrap_err();
            assert_eq!(constraint_of(&error).as_deref(),Some("duplicate_json"),"{table}.{column}, {seal}: {error}");
        }
        drop(book);
        match Book::open(&path,&[93;32]) {
            Err(DbError::Integrity(problems)) => {
                assert!(problems.iter().any(|p|p.starts_with("ambiguous stored JSON: ")),"{table}.{column}: {problems:?}");
                assert!(!problems.iter().any(|p|p.starts_with("invalid stored text, identifier, date or timestamp")),"{table}.{column}: {problems:?}");
            }
            other => panic!("{table}.{column}: {:?}",other.err()),
        }
        eprintln!("verified {table}.{column}");
    }
}
