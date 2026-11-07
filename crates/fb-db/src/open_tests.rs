//! Item 4/5: key-free file checks, typed open failures and the open-before-upgrade split.
use super::*;

const KEY: Key = [51; 32];

#[test]
fn protection_lease_refuses_additional_connections_until_released() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("leased.fbbook");
    let holder = Book::create(&path,&KEY).unwrap();
    let lease = holder.protection_lease().unwrap();
    let result = Book::open(&path,&KEY).err().expect("maintenance must refuse a new connection");
    assert!(result.to_string().contains("already open"));
    drop(lease);
    let second = Book::open(&path,&KEY).unwrap();
    assert!(holder.protection_lease().is_err());
    drop(second);
    drop(holder.protection_lease().unwrap());
}

#[test]
fn replacing_a_path_after_guard_acquisition_is_refused_before_connecting() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("original.fbbook");
    drop(Book::create(&path, &KEY).unwrap());
    let guard = process_book_lock(&path, false, false).unwrap();
    std::fs::rename(&path, dir.path().join("retained-original.fbbook")).unwrap();
    std::fs::write(&path, b"synthetic replacement must remain untouched").unwrap();
    let error = Book::connect(&path, &KEY, false, guard).err().expect("changed inode must be refused");
    assert!(error.to_string().contains("file changed"), "{error}");
    assert_eq!(std::fs::read(path).unwrap(), b"synthetic replacement must remain untouched");
}

fn book(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("book.fbbook");
    let book = Book::create(&path, &KEY).unwrap();
    book.conn.execute_batch("INSERT INTO ledger_company(id,name,currency) VALUES('00000000-0000-7000-8000-000000000051','Synthetic open company','USD')").unwrap();
    drop(book);
    path
}

/// The same book one migration behind this app (the last migration not yet applied).
fn prior(dir: &Path) -> std::path::PathBuf {
    let source = Book::open(&book(dir), &KEY).unwrap();
    let path = dir.join("prior.fbbook");
    test_support::populated_copy(&source.conn, &path, &KEY, MIGRATIONS.last().unwrap().0, false).unwrap();
    path
}

fn bytes(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap()
}

#[test]
fn damaged_files_are_named_before_any_key_is_tried() {
    let dir = tempfile::tempdir().unwrap();
    let path = book(dir.path());
    let len = std::fs::metadata(&path).unwrap().len();
    type DamageCase = (&'static str, Box<dyn Fn(&Path)>);
    let cases: [DamageCase; 4] = [
        ("empty", Box::new(|p: &Path| std::fs::write(p, b"").unwrap())),
        ("cut short", Box::new(move |p: &Path| std::fs::OpenOptions::new().write(true).open(p).unwrap().set_len(len - 100).unwrap())),
        ("not encrypted", Box::new(|p: &Path| {
            let c = Connection::open(p).unwrap();
            c.execute_batch("CREATE TABLE t(x); INSERT INTO t VALUES(1);").unwrap();
        })),
        ("not a file", Box::new(|p: &Path| std::fs::create_dir(p).unwrap())),
    ];
    for (what, damage) in cases {
        let p = dir.path().join(format!("{}.fbbook", what.replace(' ', "-")));
        if what == "cut short" { std::fs::copy(&path, &p).unwrap(); }
        damage(&p);
        for key in [KEY, [52; 32]] {
            let e = Book::open(&p, &key).err().expect("refused");
            assert!(matches!(e, DbError::Damaged(w) if w == what), "{what}: {e:?}");
            assert!(matches!(Book::open_staged(&p, &key), Err(DbError::Damaged(_))));
        }
    }
    assert!(matches!(check_file(&dir.path().join("missing.fbbook")), Err(DbError::Io(_))));
    // Whole pages of ciphertext pass the key-free checks; only the key can tell the rest.
    assert!(check_file(&path).is_ok());
    assert!(matches!(Book::open(&path, &[52; 32]), Err(DbError::WrongKeyOrNotABook)));
}

#[test]
fn a_damaged_page_with_the_right_key_is_damage() {
    let dir = tempfile::tempdir().unwrap();
    let path = book(dir.path());
    let mut raw = bytes(&path);
    // Page 1 fails its HMAC exactly as a wrong key would: only a proven key tells them apart.
    raw[100] ^= 0xff;
    let first = dir.path().join("first.fbbook");
    std::fs::write(&first, &raw).unwrap();
    assert!(matches!(Book::open(&first, &KEY), Err(DbError::WrongKeyOrNotABook)));
    // A later page: the key read page 1, so the failure is the file's.
    let mut raw = bytes(&path);
    let at = raw.len() - 2000;
    raw[at] ^= 0xff;
    let later = dir.path().join("later.fbbook");
    std::fs::write(&later, &raw).unwrap();
    let e = Book::open(&later, &KEY).err().expect("refused");
    assert!(e.is_damage(), "{e:?}");
    // Cut at a page boundary: whole pages, but the book is incomplete.
    let cut = dir.path().join("cut.fbbook");
    std::fs::write(&cut, &bytes(&path)[..8192]).unwrap();
    let e = Book::open(&cut, &KEY).err().expect("refused");
    assert!(e.is_damage(), "{e:?}");
}

#[test]
fn a_newer_book_is_too_new_and_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let path = book(dir.path());
    {
        let c = Connection::open(&path).unwrap();
        c.execute_batch(&format!("PRAGMA key=\"x'{}'\"; PRAGMA journal_mode=DELETE; PRAGMA user_version={}", hex(&KEY), MIGRATIONS.len() + 3)).unwrap();
    }
    let before = bytes(&path);
    let known = MIGRATIONS.len() as i64;
    assert!(matches!(Book::open(&path, &KEY), Err(DbError::TooNew { found, known: k }) if found == known + 3 && k == known));
    assert!(matches!(Book::open_staged(&path, &KEY), Err(DbError::TooNew { .. })));
    assert!(bytes(&path) == before, "a too-new book must remain byte-identical, including its journal mode");
}

#[test]
fn an_older_book_opens_unmigrated_and_copies_before_its_upgrade() {
    let dir = tempfile::tempdir().unwrap();
    let path = prior(dir.path());
    let before = bytes(&path);
    let Opened::Behind(old) = Book::open_staged(&path, &KEY).unwrap() else { panic!("expected an upgrade") };
    assert_eq!((old.from(), old.to()), (MIGRATIONS.len() - 1, MIGRATIONS.len()));
    assert_eq!(SnapshotSource::migrations(&old).len(), MIGRATIONS.len() - 1);
    let copy = dir.path().join("copy.fbbook");
    SnapshotSource::snapshot(&old, &copy, &KEY).unwrap();
    let version = Book::inspect_snapshot(&copy, &KEY, |q| -> Result<i64, DbError> { Ok(q.query_row("PRAGMA user_version", [], |r| r.get(0))?) }).unwrap();
    assert_eq!(version, MIGRATIONS.len() as i64 - 1, "the copy is the book before its upgrade");
    drop(old);
    assert_eq!(bytes(&path), before, "opening and copying changed nothing");
    let Opened::Behind(old) = Book::open_staged(&path, &KEY).unwrap() else { panic!() };
    let book = old.upgrade().unwrap();
    let v: i64 = book.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    assert_eq!(v, MIGRATIONS.len() as i64);
    assert!(book.check().unwrap().is_empty());
    drop(book);
    assert!(matches!(Book::open_staged(&path, &KEY).unwrap(), Opened::Current(_)));
}

#[test]
fn an_unversioned_encrypted_file_is_not_migrated_into_a_book() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("not-a-book.fbbook");
    {
        let c = Connection::open(&path).unwrap();
        c.execute_batch(&format!("PRAGMA key=\"x'{}'\"; CREATE TABLE unrelated(value TEXT); INSERT INTO unrelated VALUES('synthetic');", hex(&KEY))).unwrap();
    }
    let before = bytes(&path);
    assert!(matches!(Book::open_staged(&path, &KEY), Err(DbError::Damaged(_))), "an unversioned file must be refused before migration");
    assert_eq!(bytes(&path), before, "a non-book must remain byte-identical");
    assert!(matches!(Book::open(&path, &KEY), Err(DbError::Damaged(_))));
    assert_eq!(bytes(&path), before, "ordinary open must refuse the non-book too");
}
