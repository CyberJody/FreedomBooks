use fb_db::{Book, DbError};
use std::fs;
const KEY: [u8; 32] = [42; 32];

#[test]
fn encrypted_snapshot_refuses_existing_targets_and_remains_independent() {
    let d = tempfile::tempdir().unwrap();
    let live = d.path().join("live.fbbook");
    let snap = d.path().join("snap.fbbook");
    let mut book = Book::create(&live, &KEY).unwrap();
    book.write(|q|->Result<_,DbError>{q.execute("INSERT INTO fb_user(id,username) VALUES('00000000-0000-0000-0000-000000000001','Synthetic')",[])?;Ok(())}).unwrap();
    assert!(book.snapshot_size().unwrap() > 0);
    book.snapshot(&snap, &KEY).unwrap();
    assert!(book.snapshot(&snap, &KEY).is_err());
    book.write(|q| -> Result<_, DbError> {
        q.execute("UPDATE fb_user SET username='Later'", [])?;
        Ok(())
    })
    .unwrap();
    let name = Book::inspect_snapshot(&snap, &KEY, |q| -> Result<_, DbError> {
        Ok(q.query_row("SELECT username FROM fb_user", [], |r| {
            r.get::<_, String>(0)
        })?)
    })
    .unwrap();
    assert_eq!(name, "Synthetic");
    assert_ne!(&fs::read(&snap).unwrap()[..16], b"SQLite format 3\0");
    assert!(Book::open(&snap, &[0; 32]).is_err());
    assert!(Book::open(&snap, &KEY).unwrap().check().unwrap().is_empty());
}

#[test]
fn inspection_cannot_write_or_change_configuration_and_corrupt_schema_is_refused() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("live.fbbook");
    let mut book = Book::create(&path, &KEY).unwrap();
    book.write(|q|->Result<_,DbError>{q.execute("INSERT INTO fb_user(id,username) VALUES('00000000-0000-0000-0000-000000000001','Synthetic')",[])?;Ok(())}).unwrap();
    drop(book);
    for sql in [
        "DELETE FROM fb_user",
        "PRAGMA query_only=OFF",
        "PRAGMA writable_schema=ON",
        "COMMIT",
        "ATTACH ':memory:' AS other",
        "DROP TABLE fb_user",
    ] {
        let result = Book::inspect_snapshot(&path, &KEY, |q| -> Result<_, DbError> {
            q.query_row(sql, [], |_| Ok(()))?;
            Ok(())
        });
        assert!(result.is_err(), "{sql}");
        assert!(Book::open(&path, &KEY).unwrap().check().unwrap().is_empty());
    }
    let book = Book::open(&path, &KEY).unwrap();
    book.connection()
        .execute_batch("DROP TRIGGER fb_owner_insert")
        .unwrap();
    let snap = d.path().join("bad.fbbook");
    assert!(book.snapshot(&snap, &KEY).is_err());
    assert!(!snap.exists());
    drop(book);
    assert!(Book::inspect_snapshot(&path, &KEY, |_| -> Result<_, DbError> { Ok(()) }).is_err());
}
