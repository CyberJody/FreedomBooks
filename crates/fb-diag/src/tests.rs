use super::*;
use std::sync::Mutex;

// The log is process-wide; these tests take turns.
static TURN: Mutex<()> = Mutex::new(());

fn turn() -> std::sync::MutexGuard<'static, ()> {
    TURN.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn lines(dir: &std::path::Path, file: &str) -> Vec<serde_json::Value> {
    std::fs::read_to_string(dir.join(file)).unwrap_or_default().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

fn all_text(dir: &std::path::Path) -> String {
    let mut text = String::new();
    for e in std::fs::read_dir(dir).unwrap() {
        text.push_str(&std::fs::read_to_string(e.unwrap().path()).unwrap_or_default());
    }
    text
}

/// Book words, amounts, a memo, secrets of every shape, a URL with credentials, a home-folder
/// path and SQL with values: none of it may ever reach the log.
pub(crate) const BOOK_TEXT: &[&str] = &[
    "Synthetic Carol Hardware",
    "Alicé",
    "1234.56",
    "98765.43",
    "rent for March unit 4B",
    "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6",
    "JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP",
    "hunter2-synthetic",
    "https://synthuser:synthpass@bank.example/feed",
    "/home/synthetic-owner/Books/main.fbbook",
    "INSERT INTO sales_receipt VALUES ('Synthetic Dave', 777.77)",
];
/// Fragments of [`BOOK_TEXT`] that must not survive in any form (masked, cut or quoted).
pub(crate) const FORBIDDEN: &[&str] = &[
    "Carol", "Alic", "1234.56", "98765", "rent for", "unit 4B", "a1b2c3d4", "a#b#", "JBSWY", "PXPJ", "hunter", "synthuser", "synthpass", "bank.example",
    "synthetic-owner", "Books/main", "INSERT", "sales_receipt VALUES", "Dave", "777.77",
];

pub(crate) fn assert_clean(text: &str) {
    for gone in FORBIDDEN {
        assert!(!text.contains(gone), "the log holds {gone:?}:\n{text}");
    }
    if let Some(home) = std::env::var_os("HOME").map(|h| h.to_string_lossy().into_owned()).filter(|h| h.len() > 1) {
        assert!(!text.contains(&home), "the log holds the home folder:\n{text}");
    }
}

#[test]
fn entries_wait_for_the_folder_then_follow_a_header_with_the_build() {
    let _turn = turn();
    log::reset_for_tests();
    record(Entry { source: "rust", op: "render".into(), route: Some("company_home".into()), kind: "panic".into(), detail: Some("unwrap on Err"),
        location: Some("crates/fb-app/src/screens.rs:1:2".into()), book_open: Some(true), ..Default::default() });
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("logs");
    configure(&logs).unwrap();
    let written = lines(&logs, log::FILE);
    assert_eq!(written[0]["header"], "FreedomBooks problem log");
    assert_eq!(written[0]["build"]["migrations"], fb_db::MIGRATIONS.len());
    assert!(written[0].get("pid").is_none());
    let entry = &written[1];
    for field in ["t", "build", "schema", "source", "op", "route", "kind", "detail", "location", "book"] {
        assert!(entry.get(field).is_some(), "{field}: {entry}");
    }
    assert!(entry.get("message").is_none(), "there is no message field: {entry}");
    assert_eq!(entry["detail"], "unwrap on Err");
    assert_eq!(entry["location"], "crates/fb-app/src/screens.rs:1:2");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&logs).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(std::fs::metadata(logs.join(log::FILE)).unwrap().permissions().mode() & 0o777, 0o600);
    }
    build::set_os("Synthetic OS 1 <b>");
    let summary = summary();
    assert!(summary.contains("Operating system: Synthetic OS 1 b"), "{summary}");
    assert!(summary.contains("Book format:") && summary.contains("panic  render"), "{summary}");
    assert!(!summary.contains("unwrap") && !summary.contains("crates/"), "the summary holds only times, kinds and steps: {summary}");
}

#[test]
fn the_log_rotates_and_keeps_three_owner_only_files() {
    let _turn = turn();
    log::reset_for_tests();
    let dir = tempfile::tempdir().unwrap();
    configure(dir.path()).unwrap();
    let filler = "x".repeat(120);
    let per = log::TEST_MAX_BYTES as usize / 200;
    for _ in 0..per * 4 {
        record(Entry { source: "main", op: filler.clone(), kind: "uncaughtException".into(), ..Default::default() });
    }
    let mut names: Vec<String> = std::fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    names.sort();
    assert_eq!(names, vec![log::FILE.to_owned(), format!("{}.1", log::FILE), format!("{}.2", log::FILE)]);
    for name in &names {
        let path = dir.path().join(name);
        assert!(std::fs::metadata(&path).unwrap().len() <= log::TEST_MAX_BYTES, "{name}");
        assert_eq!(lines(dir.path(), name)[0]["header"], "FreedomBooks problem log", "{name} starts with a header");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600, "{name}");
        }
    }
    // A later launch sees the earlier problems for the summary.
    log::reset_for_tests();
    configure(dir.path()).unwrap();
    assert_eq!(recent(5).len(), 5);
}

/// Every kind of panic payload, through the catch the bridge uses: only the location, a fixed
/// category and code frames come back, and the log written from them holds none of the text.
#[test]
fn panics_keep_where_and_a_fixed_category_and_nothing_of_their_message() {
    let _turn = turn();
    log::reset_for_tests();
    install_hook();
    let dir = tempfile::tempdir().unwrap();
    configure(dir.path()).unwrap();
    let joined = BOOK_TEXT.join(" | ");
    let stored = |ok: bool| -> Result<(), String> { if ok { Ok(()) } else { Err(joined.clone()) } };
    let mut caught = vec![];
    // A String payload (format!), a &str payload, an Err whose Debug quotes the input, a
    // non-string payload, and std's own slice message, which quotes the sliced text.
    caught.push(catch(|| panic!("row for {joined} has no id")).unwrap_err());
    caught.push(catch(|| std::panic::panic_any("Synthetic Carol Hardware owes 1234.56: hunter2-synthetic")).unwrap_err());
    caught.push(catch(|| stored(false).expect("stored row shape")).unwrap_err());
    caught.push(catch(|| stored(false).unwrap()).unwrap_err());
    caught.push(catch(|| std::panic::panic_any(BOOK_TEXT.iter().map(|s| s.to_string()).collect::<Vec<_>>())).unwrap_err());
    let memo = String::from("Synthetic Alicé rent for March unit 4B");
    caught.push(catch(|| memo[..std::hint::black_box(15)].len()).unwrap_err());
    let categories: Vec<&str> = caught.iter().map(|c| c.category).collect();
    assert_eq!(categories, ["panic with a message", "panic with a message", "panic with a message", "unwrap on Err", "panic with a non-text payload", "string index not on a character boundary"]);
    for c in &caught {
        assert!(c.location.starts_with("crates/fb-diag/src/tests.rs:") || c.location.starts_with("library/core/src/"), "{}", c.location);
        assert!(c.backtrace.iter().any(|f| f.contains("fb_diag::tests")), "{:?}", c.backtrace);
        assert!(!c.backtrace.iter().any(|f| f.contains("fb_diag::panic::") || f.contains("panic.rs")), "no hook frames: {:?}", c.backtrace);
        record(Entry { op: "render".into(), kind: "panic".into(), detail: Some(c.category), location: Some(c.location.clone()), backtrace: c.backtrace.clone(), ..Default::default() });
    }
    assert!(catch(|| 7).is_ok());
    // A panic on a thread no catch guards waits in the mailbox and is written by the next call.
    let joined_err = std::thread::Builder::new().name("synthetic-worker".into()).spawn(|| {
        let none: Option<&str> = None;
        let _ = none.map(str::len).unwrap_or_else(|| panic!("worker lost its row: {}", BOOK_TEXT.join(" ")));
    }).unwrap().join();
    assert!(joined_err.is_err());
    drain_other_threads();
    let written = lines(dir.path(), log::FILE);
    let thread = written.iter().find(|l| l["op"] == "thread").unwrap();
    assert_eq!(thread["kind"], "panic");
    assert_eq!(thread["detail"], "panic with a message");
    assert_eq!(thread["outcome"], "the thread stopped");
    assert!(thread["location"].as_str().unwrap().starts_with("crates/fb-diag/src/tests.rs:"), "{thread}");
    assert_clean(&all_text(dir.path()));
}

#[cfg(unix)]
#[test]
fn the_log_never_follows_a_link_never_chmods_and_refuses_files_that_are_not_its_own() {
    use std::os::unix::fs::PermissionsExt;
    let _turn = turn();
    log::reset_for_tests();
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("logs");
    std::fs::create_dir(&logs).unwrap();
    // A link where the log goes: the target is neither written nor chmodded.
    let target = dir.path().join("elsewhere.txt");
    std::fs::write(&target, "someone's file\n").unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();
    std::os::unix::fs::symlink(&target, logs.join(log::FILE)).unwrap();
    configure(&logs).unwrap();
    record(Entry { op: "render".into(), kind: "panic".into(), ..Default::default() });
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "someone's file\n");
    assert_eq!(std::fs::metadata(&target).unwrap().permissions().mode() & 0o777, 0o644);
    std::fs::remove_file(logs.join(log::FILE)).unwrap();
    // A hard link to another file: refused (more than one link), never written.
    std::fs::hard_link(&target, logs.join(log::FILE)).unwrap();
    record(Entry { op: "render".into(), kind: "panic".into(), ..Default::default() });
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "someone's file\n");
    std::fs::remove_file(logs.join(log::FILE)).unwrap();
    // A FIFO: opened without blocking, refused.
    let fifo = std::ffi::CString::new(logs.join(log::FILE).into_os_string().into_encoded_bytes()).unwrap();
    // SAFETY: a valid NUL-terminated path.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    log::reset_for_tests();
    configure(&logs).unwrap();
    record(Entry { op: "render".into(), kind: "panic".into(), ..Default::default() });
    std::fs::remove_file(logs.join(log::FILE)).unwrap();
    // An existing log that others can read is set aside as it is, and a private one started.
    std::fs::write(logs.join(log::FILE), "{\"t\":\"2026-01-01T00:00:00.000Z\",\"kind\":\"panic\",\"op\":\"old\"}\n").unwrap();
    std::fs::set_permissions(logs.join(log::FILE), std::fs::Permissions::from_mode(0o644)).unwrap();
    record(Entry { op: "render".into(), kind: "panic".into(), ..Default::default() });
    assert_eq!(std::fs::metadata(logs.join(format!("{}.1", log::FILE))).unwrap().permissions().mode() & 0o777, 0o644, "set aside, not chmodded");
    assert_eq!(std::fs::metadata(logs.join(log::FILE)).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(lines(&logs, log::FILE).iter().filter(|l| l["op"] == "render").count(), 1);
    // A folder that is a link is refused; an existing folder's permissions are left alone.
    let linked = dir.path().join("linked-logs");
    std::os::unix::fs::symlink(&logs, &linked).unwrap();
    assert!(configure(&linked).is_err());
    let open = dir.path().join("open-logs");
    std::fs::create_dir(&open).unwrap();
    std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o755)).unwrap();
    configure(&open).unwrap();
    assert_eq!(std::fs::metadata(&open).unwrap().permissions().mode() & 0o777, 0o755);
    assert_eq!(std::fs::metadata(open.join(log::FILE)).unwrap().permissions().mode() & 0o777, 0o600);
}

#[cfg(unix)]
#[test]
fn an_unwritable_folder_or_a_read_only_log_means_logged_nothing_and_no_failure() {
    use std::os::unix::fs::PermissionsExt;
    let _turn = turn();
    log::reset_for_tests();
    install_hook();
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("logs");
    std::fs::create_dir(&logs).unwrap();
    std::fs::set_permissions(&logs, std::fs::Permissions::from_mode(0o500)).unwrap();
    configure(&logs).unwrap();
    record(Entry { op: "render".into(), kind: "panic".into(), ..Default::default() });
    assert!(catch(|| panic!("x")).is_err());
    assert_eq!(std::fs::read_dir(&logs).unwrap().count(), 0);
    std::fs::set_permissions(&logs, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(logs.join(log::FILE), "").unwrap();
    std::fs::set_permissions(logs.join(log::FILE), std::fs::Permissions::from_mode(0o400)).unwrap();
    record(Entry { op: "render".into(), kind: "panic".into(), ..Default::default() });
    assert_eq!(std::fs::read_to_string(logs.join(log::FILE)).unwrap(), "");
}

#[test]
fn build_labels_never_invent_a_commit() {
    let b = build();
    assert_eq!(b.commit, "unknown", "nothing registered a build in these tests");
    assert!(short_version().ends_with("(source build)"));
    assert!(record_label().contains("schema"));
}

/// The hook under the conditions that used to abort the process: stderr closed, stderr on a full
/// device on Linux, the log folder unwritable, and writes to the log failing as on a full disk. Each runs
/// in a child process (this test binary again); each must survive and catch its panics.
#[cfg(unix)]
#[test]
fn the_hook_survives_closed_or_full_stderr_an_unwritable_folder_and_a_full_disk() {
    for mode in [
        "stderr-closed",
        #[cfg(target_os = "linux")]
        "stderr-full",
        "folder-unwritable",
        "disk-full",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command.args(["--exact", "tests::child_process_under_test", "--nocapture", "--test-threads=1"]).env("FB_DIAG_CHILD", mode).env("FB_DIAG_CHILD_DIR", dir.path()).env_remove("FB_DIAG_STDERR");
        #[cfg(target_os = "linux")]
        if mode == "stderr-full" {
            command.stderr(std::fs::OpenOptions::new().write(true).open("/dev/full").unwrap());
        }
        let out = command.output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "{mode}: {:?}\n{stdout}", out.status);
        assert!(stdout.contains("CHILD-SURVIVED caught=3 thread=1"), "{mode}: {stdout}");
        let logs = dir.path().join("logs");
        let text = std::fs::read_dir(&logs).map(|_| all_text(&logs)).unwrap_or_default();
        match mode {
            "folder-unwritable" => assert_eq!(text, "", "{mode}: logged nothing"),
            "disk-full" => assert!(text.len() <= 600, "{mode}: writes stop at the limit: {} bytes", text.len()),
            _ => {
                assert_eq!(text.lines().filter(|l| l.contains("\"kind\":\"panic\"")).count(), 4, "{mode}: {text}");
                assert!(text.lines().all(|l| l.starts_with('{')), "{mode}: nothing else wrote into the log: {text}");
            }
        }
        assert_clean(&text);
    }
}

#[cfg(unix)]
#[test]
fn child_process_under_test() {
    let Ok(mode) = std::env::var("FB_DIAG_CHILD") else { return };
    let dir = std::path::PathBuf::from(std::env::var("FB_DIAG_CHILD_DIR").unwrap());
    let logs = dir.join("logs");
    match mode.as_str() {
        // SAFETY: closing this process's own stderr descriptor.
        "stderr-closed" => assert_eq!(unsafe { libc::close(2) }, 0),
        "folder-unwritable" => {
            use std::os::unix::fs::PermissionsExt;
            std::fs::create_dir(&logs).unwrap();
            std::fs::set_permissions(&logs, std::fs::Permissions::from_mode(0o500)).unwrap();
        }
        "disk-full" => {
            // Files may not grow past 512 bytes; the write fails with EFBIG instead of a signal.
            // SAFETY: plain libc calls with valid arguments.
            unsafe {
                libc::signal(libc::SIGXFSZ, libc::SIG_IGN);
                let limit = libc::rlimit { rlim_cur: 512, rlim_max: libc::RLIM_INFINITY };
                assert_eq!(libc::setrlimit(libc::RLIMIT_FSIZE, &limit), 0);
            }
        }
        _ => {}
    }
    install_hook();
    let _ = configure(&logs);
    let mut caught = 0;
    for _ in 0..3 {
        let c = catch(|| -> u32 { panic!("child panic for {}", BOOK_TEXT.join(" ")) }).unwrap_err();
        record(Entry { op: "render".into(), kind: "panic".into(), detail: Some(c.category), location: Some(c.location), backtrace: c.backtrace, ..Default::default() });
        caught += 1;
    }
    let thread = std::thread::spawn(|| panic!("{}", BOOK_TEXT.join(" "))).join().is_err();
    drain_other_threads();
    use std::io::Write;
    let _ = writeln!(std::io::stdout(), "CHILD-SURVIVED caught={caught} thread={}", u8::from(thread));
    let _ = std::io::stdout().flush();
}
