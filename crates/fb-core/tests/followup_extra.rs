mod common;
use common::*;
use fb_core::{Book, users};
use fb_db::rusqlite;

#[test]
fn schema_hash_like_pattern_ignores_non_internal_trigger() {
    let path = book_path("schema-like");
    let mut book = Book::create(&path, &KEY).unwrap();
    let user = users::create_user(&mut book, "tester", false).unwrap();
    let mut fx = Fx { book, user };
    fx.chart();
    fx.book.connection().execute_batch("CREATE TRIGGER sqliteXskip_audit BEFORE INSERT ON ledger_auditevent BEGIN SELECT RAISE(IGNORE); END").unwrap();
    assert!(!fx.book.check().unwrap().is_empty());
    drop(fx);
    assert!(Book::open(&path, &KEY).is_err());
}

#[test]
fn defer_foreign_keys_toggle_can_discharge_seal_obligation() {
    let path = book_path("deferred-toggle");
    let mut book = Book::create(&path, &KEY).unwrap();
    let result:rusqlite::Result<()>=book.write(|tx| {
        tx.execute_batch("PRAGMA defer_foreign_keys=ON; INSERT INTO fb_seal_due VALUES('unsealed','x'); PRAGMA defer_foreign_keys=OFF")?;
        Ok(())
    });
    assert!(result.is_err());
    assert_eq!(
        book.connection()
            .query_row("SELECT count(*) FROM fb_seal_due", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert!(book.check().unwrap().is_empty());
    drop(book);
    assert!(Book::open(&path, &KEY).is_ok());
}

#[test]
fn read_callback_can_disable_query_only_and_write() {
    let path = book_path("read-write");
    let mut book = Book::create(&path, &KEY).unwrap();
    let result: rusqlite::Result<()> = book.read(|tx| {
        tx.execute_batch(
            "PRAGMA query_only=OFF; INSERT INTO fb_seal VALUES('read','write'); COMMIT",
        )?;
        Ok(())
    });
    assert!(result.is_err());
    assert_eq!(
        book.connection()
            .query_row("SELECT count(*) FROM fb_seal WHERE kind='read'", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap(),
        0
    );
}
