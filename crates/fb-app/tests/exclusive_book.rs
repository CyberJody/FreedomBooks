//! JBACKUP-R02: real independent processes, no posting or shared live data.
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use fb_db::Book;

const KEY: [u8; 32] = [37; 32];

#[test]
fn maintenance_refuses_same_process_connections() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("maintenance.fbbook");
    drop(Book::create(&path, &KEY).unwrap());
    let guard = fb_db::MaintenanceGuard::acquire(&path).unwrap();
    assert!(Book::open(&path, &KEY).is_err(), "maintenance must exclude same-process openers too");
    assert!(Book::open_staged(&path, &KEY).is_err());
    drop(guard);
    drop(Book::open(&path, &KEY).unwrap());
}

#[cfg(unix)]
#[test]
fn a_hardlink_cannot_hide_an_interrupted_protection_change() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("original.fbbook");
    let alias = dir.path().join("alias.fbbook");
    let book = Book::create(&path, &KEY).unwrap();
    std::fs::hard_link(&path, &alias).unwrap();
    assert!(book.protection_lease().is_err(), "a path-based journal cannot protect multiple names");
    drop(book);
    std::fs::write(fb_app::protection::sidecar(&path, ".rekey.json"), b"pending").unwrap();
    assert!(Book::open(&alias, &KEY).is_err(), "the alias must not bypass the original path's recovery marker");
    assert!(Book::open_staged(&alias, &KEY).is_err());
    assert!(fb_app::protection::recover_key_change(&path).is_err());
}

#[cfg(feature = "key-test-hooks")]
#[test]
fn delayed_opener_child() {
    let Some(path) = std::env::var_os("FB_DELAYED_BOOK") else { return };
    let path = std::path::Path::new(&path);
    let result = if std::env::var_os("FB_DELAYED_STAGED").is_some() {
        Book::open_staged(path, &KEY).map(|_| ())
    } else { Book::open(path, &KEY).map(|_| ()) };
    let error = result.expect_err("an opener paused before locking bypassed the recovery journal");
    assert!(error.to_string().contains("interrupted protection"), "{error}");
}

#[cfg(feature = "key-test-hooks")]
#[test]
fn delayed_openers_recheck_recovery_after_locking() {
    use std::time::{Duration, Instant};
    for staged in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("delayed.fbbook");
        drop(Book::create(&path, &KEY).unwrap());
        let before = std::fs::read(&path).unwrap();
        let barrier = dir.path().join("barrier");
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.args(["--exact", "delayed_opener_child", "--nocapture"])
            .env("FB_DELAYED_BOOK", &path).env("FB_TEST_OPEN_BARRIER", &barrier);
        if staged { command.env("FB_DELAYED_STAGED", "1"); }
        let mut child = command.spawn().unwrap();
        let start = Instant::now();
        while !barrier.with_extension("ready").exists() {
            assert!(child.try_wait().unwrap().is_none());
            if start.elapsed() > Duration::from_secs(30) {
                child.kill().unwrap(); child.wait().unwrap(); panic!("opener did not reach barrier");
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        // Another process could prepare and crash here. Publish under the same guard it uses,
        // then let the earlier opener acquire that guard only after the journal exists.
        let maintenance = fb_db::MaintenanceGuard::acquire(&path).unwrap();
        std::fs::write(fb_app::protection::sidecar(&path, ".rekey.json"), b"pending").unwrap();
        drop(maintenance);
        std::fs::write(barrier.with_extension("resume"), b"resume").unwrap();
        assert!(child.wait().unwrap().success(), "staged={staged}");
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}

#[cfg(unix)]
#[test]
fn recovery_metadata_errors_are_not_treated_as_absence() {
    let dir = tempfile::tempdir().unwrap();
    // The book and lock names fit NAME_MAX; the recovery marker does not.
    let path = dir.path().join("b".repeat(250));
    assert!(Book::create(&path, &KEY).is_err(), "ENAMETOOLONG must not admit a book");
}

#[test]
fn exclusive_book_child() {
    let Some(path) = std::env::var_os("FB_EXCLUSIVE_TEST_BOOK") else { return };
    let book = Book::open(std::path::Path::new(&path), &KEY);
    match book {
        Ok(_book) => {
            println!("BOOK_OPEN");
            std::io::stdout().flush().unwrap();
            let mut line = String::new();
            std::io::stdin().read_line(&mut line).unwrap();
        }
        Err(error) => {
            assert!(error.to_string().contains("already open"), "{error}");
            println!("BOOK_REFUSED");
        }
    }
}

fn child(path: &std::path::Path) -> (std::process::Child, String) {
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "exclusive_book_child", "--nocapture"])
        .env("FB_EXCLUSIVE_TEST_BOOK", path)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    loop {
        line.clear();
        assert_ne!(output.read_line(&mut line).unwrap(), 0, "child exited without a book result");
        if line.contains("BOOK_OPEN") || line.contains("BOOK_REFUSED") { break; }
    }
    child.stdout = Some(output.into_inner());
    (child, line)
}

#[test]
fn exclusive_book_refuses_both_process_orders_and_releases_after_close_or_crash() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("synthetic.fbbook");
    let first = Book::create(&path, &KEY).unwrap();
    let second_connection = Book::open(&path, &KEY).unwrap();
    drop(first);
    let (mut blocked, result) = child(&path);
    let refused = result.contains("BOOK_REFUSED");
    // Always release the child before asserting, including the failing-first run.
    blocked.stdin.take().unwrap().write_all(b"close\n").ok();
    assert!(blocked.wait().unwrap().success());
    assert!(refused, "a second process opened the live book: {result}");
    drop(second_connection);
    let (mut holder, result) = child(&path);
    assert!(result.contains("BOOK_OPEN"));
    let refusal = Book::open(&path, &KEY).err().expect("child holds the book");
    assert!(refusal.to_string().contains("already open"));
    holder.stdin.take().unwrap().write_all(b"close\n").unwrap();
    assert!(holder.wait().unwrap().success());
    drop(Book::open(&path, &KEY).unwrap());
    let (mut crashed, result) = child(&path);
    assert!(result.contains("BOOK_OPEN"));
    crashed.kill().unwrap();
    crashed.wait().unwrap();
    drop(Book::open(&path, &KEY).unwrap());
}

#[test]
fn exclusive_book_keeps_an_existing_book_lock_record() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("synthetic.fbbook");
    drop(Book::create(&path, &KEY).unwrap());
    let lock = dir.path().join("synthetic.fbbook.lock");
    let record = b"FreedomBooks book lock 1\nhost=synthetic-other-host\n";
    std::fs::write(&lock, record).unwrap();
    assert!(Book::open(&path, &KEY).is_err(), "an existing host record needs the matching book-lock implementation");
    assert_eq!(std::fs::read(lock).unwrap(), record);
}

#[test]
fn staged_open_holds_exclusive_access_until_the_book_closes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("staged.fbbook");
    drop(Book::create(&path, &KEY).unwrap());
    let staged = Book::open_staged(&path, &KEY).unwrap();
    let (mut contender, result) = child(&path);
    contender.stdin.take().unwrap().write_all(b"close\n").ok();
    assert!(contender.wait().unwrap().success());
    assert!(result.contains("BOOK_REFUSED"), "staged opener allowed another process: {result}");
    drop(staged);
    drop(Book::open(&path, &KEY).unwrap());
}

#[test]
fn pending_upgrade_holds_exclusive_access_before_any_migration() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("prior.fbbook");
    {
        let source_path = dir.path().join("source.fbbook");
        let source = Book::create(&source_path, &KEY).unwrap();
        fb_db::test_support::populated_copy(source.connection(), &path, &KEY,
            fb_db::MIGRATIONS.last().unwrap().0, false).unwrap();
    }
    let before = std::fs::read(&path).unwrap();
    let staged = Book::open_staged(&path, &KEY).unwrap();
    assert!(matches!(&staged, fb_db::Opened::Behind(_)));
    let (mut contender, result) = child(&path);
    contender.stdin.take().unwrap().write_all(b"close\n").ok();
    assert!(contender.wait().unwrap().success());
    assert!(result.contains("BOOK_REFUSED"), "pending upgrade allowed another opener: {result}");
    assert_eq!(std::fs::read(&path).unwrap(), before);
    drop(staged);
    drop(Book::open(&path, &KEY).unwrap());
}

#[cfg(unix)]
#[test]
fn hardlink_alias_cannot_open_the_same_physical_book() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("original.fbbook");
    let alias = dir.path().join("alias.fbbook");
    let holder = Book::create(&path, &KEY).unwrap();
    std::fs::hard_link(&path, &alias).unwrap();
    let (mut contender, result) = child(&alias);
    contender.stdin.take().unwrap().write_all(b"close\n").ok();
    assert!(contender.wait().unwrap().success());
    assert!(result.contains("BOOK_REFUSED"), "hardlink bypass: {result}");
    drop(holder);
}

#[test]
fn recovery_refuses_an_open_book_before_interpreting_its_marker() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("recovery.fbbook");
    let holder = Book::create(&path, &KEY).unwrap();
    let marker = fb_app::protection::sidecar(&path, ".rekey.json");
    std::fs::write(&marker, b"{}").unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let error = fb_app::protection::recover_rekey(&path).unwrap_err();
    assert!(error.to_string().contains("already open"), "{error}");
    assert_eq!(std::fs::read(marker).unwrap(), b"{}");
    assert_eq!(std::fs::read(path).unwrap(), bytes);
    drop(holder);
}

#[test]
fn recovery_keeps_a_live_process_and_its_valid_journal_untouched() {
    use sha2::{Digest, Sha256};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("live-recovery.fbbook");
    let snapshot = fb_app::protection::sidecar(&path, ".rekey-before");
    let marker = fb_app::protection::sidecar(&path, ".rekey.json");
    let key = fb_app::keys::MasterKey::new(KEY);
    let old = fb_app::protection::Record::new(fb_core::backups::portable::KeyEnvelope::new(&key,&key,Some("Synthetic maintenance password")).unwrap());
    {
        let source = Book::create(&path, &KEY).unwrap();
        source.snapshot(&snapshot,&KEY).unwrap();
    }
    old.save(&path).unwrap();
    let (mut holder, result) = child(&path);
    assert!(result.contains("BOOK_OPEN"));
    std::fs::write(&marker,serde_json::to_vec(&serde_json::json!({"old":old,"snapshot_hash":format!("{:x}",Sha256::digest(std::fs::read(&snapshot).unwrap()))})).unwrap()).unwrap();
    let paths: Vec<_> = ["", "-wal", "-shm", ".keys.json", ".rekey.json", ".rekey-before"].iter().map(|suffix|fb_app::protection::sidecar(&path,suffix)).collect();
    let before: Vec<_> = paths.iter().map(std::fs::read).map(Result::ok).collect();
    let result = fb_app::protection::recover_key_change(&path);
    let after: Vec<_> = paths.iter().map(std::fs::read).map(Result::ok).collect();
    holder.stdin.take().unwrap().write_all(b"close\n").unwrap();
    assert!(holder.wait().unwrap().success());
    assert!(result.unwrap_err().to_string().contains("already open"));
    assert_eq!(before,after);
    assert!(fb_app::protection::recover_key_change(&path).unwrap());
}
