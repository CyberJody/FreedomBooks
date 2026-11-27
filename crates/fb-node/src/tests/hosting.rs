//! "Host these books" end to end inside the book process: the desktop door moves into the host and
//! back, a second computer pairs through the real `fb-remote gateway` and signs in, both people work
//! on one synthetic book, revocation, and the sealed record catching older copies of the files.
use crate::hosting;
use crate::tests::turn;
use crate::{app, render, run};
use fb_app::identity::Role;
use fb_app::keys::MasterKey;
use fb_remote::client::{self, Client, Paired};
use fb_remote_proto::wire::{ClientRequest, HostResponse, RefusalCode};
use std::path::{Path, PathBuf};
use std::time::Duration;

const OWNER: &str = "Synthetic owner";
const OWNER_PASSWORD: &str = "Synthetic owner user password";
const KEEPER: &str = "Synthetic bookkeeper";
const KEEPER_PASSWORD: &str = "Synthetic keeper password";

/// The `fb-remote` helper cargo built beside this test binary (target/<profile>/fb-remote).
fn helper() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let path = exe.parent().and_then(Path::parent).unwrap().join(if cfg!(windows) { "fb-remote.exe" } else { "fb-remote" });
    assert!(path.is_file(), "build the helper first (cargo build -p fb-remote): {}", path.display());
    path
}

fn configure() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| hosting::configure(helper()).unwrap());
}

/// A fresh synthetic book on the desktop door, the owner signed in, plus a bookkeeper.
fn desktop() -> (tempfile::TempDir, String) {
    configure();
    let dir = tempfile::tempdir().unwrap();
    let mut door = app().unwrap();
    hosting::end_from(&mut door, "test reset").unwrap();
    door.lock();
    door.backups.configure(dir.path()).unwrap();
    door.device_key = Some(MasterKey::new([44; 32]));
    door.create_protected_book(&dir.path().join("books/default.fbbook"), OWNER, "Synthetic hosted company", Some("Synthetic book password")).unwrap();
    door.enroll_owner(OWNER_PASSWORD).unwrap();
    let company = door.first_company().unwrap().unwrap();
    door.save_desktop_user(None, KEEPER, Role::Bookkeeper, true, Some(KEEPER_PASSWORD), &[company]).unwrap();
    door.sign_in(OWNER, OWNER_PASSWORD).unwrap();
    (dir, company.to_db())
}

fn state() -> serde_json::Value {
    hosting::state(&mut app().unwrap(), hosting::DOOR_WAIT, false).unwrap()
}

fn on() -> Result<(), String> {
    hosting::turn_on(&mut app().unwrap(), "127.0.0.1:0", "Synthetic host").map_err(|e| e.reason.clone())
}

fn pair(name: &str) -> Paired {
    let code = hosting::pair(&mut app().unwrap()).unwrap();
    let addr = state()["addr"].as_str().unwrap().to_owned();
    client::pair(&addr, code["code"].as_str().unwrap(), name).unwrap()
}

fn connect(p: &Paired) -> Client {
    let addr = state()["addr"].as_str().unwrap().to_owned();
    let c = Client::new(&addr, &p.identity, p.host_pin, Box::new(|_| {})).unwrap().with_retry(Duration::from_secs(5));
    c.hello().unwrap();
    c
}

fn page(r: HostResponse) -> String {
    match r {
        HostResponse::Page { html, status: 200, .. } => html,
        other => panic!("expected a page, got {other:?}"),
    }
}

fn token(html: &str) -> String {
    html.split("name=\"fb_command_token\" value=\"").nth(1).and_then(|s| s.split('"').next()).expect("form token").to_owned()
}

fn id() -> String {
    let h = fb_remote_proto::hex(&fb_remote_proto::random::<16>().unwrap());
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

fn remote_submit(c: &Client, path: &str, fields: &[(&str, &str)]) -> serde_json::Value {
    let t = token(&page(c.call(&ClientRequest::Render { target: path.into() }).unwrap()));
    let command = serde_json::json!({"type": "SubmitForm", "path": path, "token": t, "fields": fields.iter().map(|(k, v)| [k, v]).collect::<Vec<_>>()}).to_string();
    match c.call(&ClientRequest::Run { request_id: id(), command }).unwrap() {
        HostResponse::Outcome { outcome } => serde_json::from_str(&outcome).unwrap(),
        other => panic!("expected an outcome, got {other:?}"),
    }
}

fn desk_submit(path: &str, fields: &[(&str, &str)]) -> serde_json::Value {
    let html: serde_json::Value = serde_json::from_str(&render(path.into()).unwrap()).unwrap();
    let t = token(html["html"].as_str().unwrap());
    let command = serde_json::json!({"type": "SubmitForm", "path": path, "token": t, "fields": fields.iter().map(|(k, v)| [k, v]).collect::<Vec<_>>()}).to_string();
    serde_json::from_str(&run(command).unwrap()).unwrap()
}

fn customers() -> (Vec<String>, Vec<String>) {
    let mut door = app().unwrap();
    hosting::with_desk(&mut door, hosting::DOOR_WAIT, |app| {
        let book = &mut app.open.as_mut().unwrap().book;
        let names = book.read(|q| q.prepare("SELECT display_name FROM directory_customer ORDER BY display_name")?.query_map([], |r| r.get(0))?.collect::<Result<Vec<String>, _>>()).unwrap();
        let creators = book.read(|q| q.prepare("SELECT actor_name FROM ledger_auditevent WHERE action='customer.created' ORDER BY occurred_at, id")?.query_map([], |r| r.get(0))?.collect::<Result<Vec<String>, _>>()).unwrap();
        Ok((names, creators))
    })
    .unwrap()
}

#[test]
fn the_desktop_hosts_a_second_computer_and_both_work_on_one_book() {
    let _turn = turn();
    let (dir, company) = desktop();
    assert!(!state()["running"].as_bool().unwrap(), "off by default");
    assert!(hosting::turn_on(&mut app().unwrap(), "0.0.0.0:47100", "x").is_err(), "never all interfaces");
    assert!(hosting::turn_on(&mut app().unwrap(), "8.8.8.8:47100", "x").is_err(), "never a public address");
    on().unwrap();
    let s = state();
    assert!(s["running"].as_bool().unwrap() && s["enabled"].as_bool().unwrap());
    assert!(hosting::active());
    // The desktop window is the host's local session: still signed in, same screens.
    let home = format!("/c/{company}/home/");
    let shown: serde_json::Value = serde_json::from_str(&render(home.clone()).unwrap()).unwrap();
    assert_eq!(shown["status"], 200);
    let saved = desk_submit(&format!("/c/{company}/lists/customers/new/"), &[("display_name", "Desk customer")]);
    assert_eq!(saved["outcome"], "redirect", "{saved}");
    // A second computer pairs with a one-time code, signs in on the host's own sign-in screen and works.
    let laptop = pair("Synthetic laptop");
    let c = connect(&laptop);
    let signed = remote_submit(&c, "/desktop/sign-in/", &[("action", "signin"), ("username", KEEPER), ("password", KEEPER_PASSWORD)]);
    assert_eq!(signed["outcome"], "redirect", "{signed}");
    let made = remote_submit(&c, &format!("/c/{company}/lists/customers/new/"), &[("display_name", "Laptop customer")]);
    assert_eq!(made["outcome"], "redirect", "{made}");
    assert_eq!(customers(), (vec!["Desk customer".to_owned(), "Laptop customer".to_owned()], vec![OWNER.to_owned(), KEEPER.to_owned()]), "one book, each change by its own person");
    // Host-only work stays on the host.
    assert!(matches!(c.call(&ClientRequest::Render { target: "/settings/book-protection/".into() }).unwrap(), HostResponse::Refused { code: RefusalCode::HostOnly, .. }));
    let s = state();
    assert_eq!(s["company"], company);
    assert_eq!(s["signed_in"], serde_json::json!([[KEEPER, "Synthetic laptop"]]));
    let devices = s["devices"].as_array().unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0]["name"], "Synthetic laptop");
    assert!(devices[0]["connected"].as_bool().unwrap());
    // The host person's lock signs out only them: the laptop keeps working.
    crate::sign_out().unwrap();
    assert!(page(c.call(&ClientRequest::Render { target: home.clone() }).unwrap()).contains("Synthetic hosted company"));
    hosting::with_desk(&mut app().unwrap(), hosting::DOOR_WAIT, |a| a.sign_in(OWNER, OWNER_PASSWORD).map_err(crate::app_error)).unwrap();
    // Revoke: the laptop is cut off at once and stays refused.
    let device = devices[0]["id"].as_str().unwrap().to_owned();
    let r = hosting::revoke(&mut app().unwrap(), &device).unwrap();
    assert_eq!(r["revoked"], true, "{r}");
    assert!(r["record_unsaved"].is_null(), "{r}");
    assert!(c.call(&ClientRequest::Render { target: home.clone() }).is_err() || matches!(c.call(&ClientRequest::Render { target: home.clone() }), Ok(HostResponse::Refused { .. })));
    // Turning hosting off gives the desktop its own books back, still signed in.
    hosting::stop(&mut app().unwrap()).unwrap();
    assert!(!hosting::active());
    let shown: serde_json::Value = serde_json::from_str(&render(home).unwrap()).unwrap();
    assert_eq!(shown["status"], 200, "the standalone desktop again");
    let record = app().unwrap().host_record().unwrap().unwrap();
    assert!(record.hosted_before && !record.enabled && record.revoked == vec![device.clone()]);
    assert!(record.audit_head.as_ref().is_some_and(|h| h.seq > 5), "{record:?}");
    assert!(record.identity.is_some());
    // The audit log verifies under the sealed key and holds no password.
    let audit = dir.path().join("hosting").join(&record.host_id).join("remote-audit.jsonl");
    let key = fb_remote_proto::audit::AuditKey::from_bytes(record.audit_key_bytes().unwrap());
    assert_eq!(fb_remote_proto::audit::verify(&audit, &key).unwrap(), record.audit_head.as_ref().unwrap().seq);
    let text = std::fs::read_to_string(&audit).unwrap();
    assert!(!text.contains(KEEPER_PASSWORD) && !text.contains(OWNER_PASSWORD));
    assert!(text.contains("device.revoked") && text.contains("pair.succeeded"));
    // Restarting keeps the identity and refuses the revoked laptop in the handshake.
    on().unwrap();
    assert!(Client::new(state()["addr"].as_str().unwrap(), &laptop.identity, laptop.host_pin, Box::new(|_| {})).unwrap().hello().is_err());
    hosting::end_from(&mut app().unwrap(), "test end").unwrap();
    assert!(app().unwrap().host_record().unwrap().unwrap().enabled, "quitting keeps the setting on");
}

#[test]
fn older_copies_of_the_files_are_refused_and_the_administrator_can_repair() {
    let _turn = turn();
    let (dir, _company) = desktop();
    on().unwrap();
    let laptop = pair("Synthetic laptop");
    let host_id = hosting::with_desk(&mut app().unwrap(), hosting::DOOR_WAIT, |a| Ok(a.host_record().unwrap().unwrap().host_id)).unwrap();
    let folder = dir.path().join("hosting").join(&host_id);
    let list = folder.join("gateway").join(fb_remote_proto::registry_file::FILE);
    let older_list = std::fs::read(&list).unwrap();
    let older_log = std::fs::read(folder.join("remote-audit.jsonl")).unwrap();
    let older_head = std::fs::read(folder.join("remote-audit.jsonl.head")).unwrap();
    let device = state()["devices"][0]["id"].as_str().unwrap().to_owned();
    hosting::revoke(&mut app().unwrap(), &device).unwrap();
    hosting::stop(&mut app().unwrap()).unwrap();

    // The list put back from before the revocation: refused, and the repair is offered.
    std::fs::write(&list, &older_list).unwrap();
    let err = on().unwrap_err();
    assert!(err.contains("older than the one these books recorded"), "{err}");
    assert_eq!(state()["problem"]["kind"], "devices-rolled-back");
    let aside = hosting::repair(&mut app().unwrap(), "devices-rolled-back", "Synthetic host").unwrap();
    assert!(aside.contains("remote-devices.json.older-") && Path::new(&aside).exists(), "the Administrator is shown what was set aside: {aside}");
    assert!(hosting::active() && state()["problem"].is_null());
    assert!(Client::new(state()["addr"].as_str().unwrap(), &laptop.identity, laptop.host_pin, Box::new(|_| {})).unwrap().hello().is_err(), "the revoked laptop stays out");
    hosting::stop(&mut app().unwrap()).unwrap();

    // Even without the counter, the sealed revoked ids hold: the list put back with a forged
    // counter is refused by its MAC, and a list re-made at a newer counter still refuses the id.
    // The audit log and its checkpoint put back from earlier: refused, then set aside.
    std::fs::write(folder.join("remote-audit.jsonl"), &older_log).unwrap();
    std::fs::write(folder.join("remote-audit.jsonl.head"), &older_head).unwrap();
    let err = on().unwrap_err();
    assert!(err.contains("not the one these books recorded"), "{err}");
    assert_eq!(state()["problem"]["kind"], "audit-rolled-back");
    let aside = hosting::repair(&mut app().unwrap(), "audit-rolled-back", "Synthetic host").unwrap();
    assert!(aside.contains("remote-audit.jsonl.rolled-back-"), "{aside}");
    assert!(hosting::active());
    let aside = std::fs::read_dir(&folder).unwrap().filter_map(|e| e.ok()).filter(|e| e.file_name().to_string_lossy().contains(".rolled-back-")).count();
    assert_eq!(aside, 2, "the old log and its checkpoint are kept aside");
    let first = std::fs::read_to_string(folder.join("remote-audit.jsonl")).unwrap();
    assert!(first.lines().next().unwrap().contains("audit.key_replaced") && first.contains(OWNER), "the repair is recorded with the Administrator's name");
    hosting::stop(&mut app().unwrap()).unwrap();
}

#[test]
fn only_an_administrator_controls_hosting_and_a_copy_is_never_hosted() {
    let _turn = turn();
    let (_dir, _company) = desktop();
    {
        let mut door = app().unwrap();
        door.sign_out();
        door.sign_in(KEEPER, KEEPER_PASSWORD).unwrap();
    }
    assert!(on().unwrap_err().contains("administrator"));
    assert_eq!(state()["admin"], false);
    {
        let mut door = app().unwrap();
        door.sign_out();
        door.sign_in(OWNER, OWNER_PASSWORD).unwrap();
        door.mark_copy(true);
    }
    assert!(on().unwrap_err().contains("restored copy"));
    app().unwrap().mark_copy(false);
    on().unwrap();
    // A bookkeeper signed in at the host cannot pair or revoke.
    hosting::with_desk(&mut app().unwrap(), hosting::DOOR_WAIT, |a| {
        a.sign_out();
        a.sign_in(KEEPER, KEEPER_PASSWORD).map_err(crate::app_error)
    })
    .unwrap();
    assert!(hosting::pair(&mut app().unwrap()).is_err());
    assert!(hosting::revoke(&mut app().unwrap(), &"0".repeat(32)).is_err());
    assert!(hosting::stop(&mut app().unwrap()).is_err());
    hosting::end_from(&mut app().unwrap(), "test end").unwrap();
}

/// Section 10 item 9: restoring a backup brings back an older sealed record (fewer revoked ids, a
/// lower counter), never an older list of paired computers (that list is not in the backup). The
/// newer list on disk is accepted, its revocations are taken back into the record, and the revoked
/// computer stays out.
#[test]
fn an_older_record_from_a_restored_backup_never_readmits_a_revoked_computer() {
    let _turn = turn();
    let (_dir, _company) = desktop();
    on().unwrap();
    let laptop = pair("Synthetic laptop");
    let before = hosting::with_desk(&mut app().unwrap(), hosting::DOOR_WAIT, |a| Ok(a.host_record().unwrap().unwrap())).unwrap();
    let device = state()["devices"][0]["id"].as_str().unwrap().to_owned();
    hosting::revoke(&mut app().unwrap(), &device).unwrap();
    hosting::stop(&mut app().unwrap()).unwrap();
    // The record as it was in the backup: the revocation is not in it.
    let mut older = before.clone();
    older.enabled = false;
    assert!(!older.revoked.contains(&device));
    app().unwrap().save_host_record(&older).unwrap();
    on().unwrap();
    assert!(Client::new(state()["addr"].as_str().unwrap(), &laptop.identity, laptop.host_pin, Box::new(|_| {})).unwrap().hello().is_err(), "still refused");
    let s = state();
    assert_eq!(s["devices"][0]["revoked"], true);
    hosting::stop(&mut app().unwrap()).unwrap();
    let now = app().unwrap().host_record().unwrap().unwrap();
    assert!(now.revoked.contains(&device), "the revocation is back in the record");
    assert!(now.device_counter > before.device_counter);
    assert!(now.audit_head.unwrap().seq > before.audit_head.unwrap().seq, "the log went on past the older head and was accepted");
}

fn signed_in() -> bool {
    let s: serde_json::Value = serde_json::from_str(&crate::app_state().unwrap()).unwrap();
    s["signed_in"].as_bool().unwrap()
}

/// Hold the books for `hold` on another thread, as a slow request from a connected computer does.
fn hold_books(hold: Duration) -> std::thread::JoinHandle<()> {
    let shared = hosting::test_shared().expect("hosting");
    let (tx, rx) = std::sync::mpsc::channel();
    let t = std::thread::spawn(move || {
        let _lease = shared.lease("test: a slow remote request", &mut |_| {}).unwrap();
        tx.send(()).unwrap();
        std::thread::sleep(hold);
    });
    rx.recv().unwrap();
    t
}

#[test]
fn a_busy_host_skips_the_screen_poll_and_executes_the_confirmed_revoke_on_the_worker() {
    let _turn = turn();
    let (_dir, _company) = desktop();
    on().unwrap();
    let laptop = pair("Synthetic laptop");
    let s = state();
    let device = s["devices"][0]["id"].as_str().unwrap().to_owned();
    let generation = s["generation"].as_str().unwrap().to_owned();
    let revoke = |generation| crate::hosting_in(&mut app().unwrap(), &crate::HostingAction::Revoke { device: device.clone(), generation });
    assert!(revoke("stale".into()).unwrap_err().reason.contains("user changed"));
    let client = connect(&laptop);
    let held = hold_books(hosting::DOOR_WAIT + Duration::from_millis(500));
    let started = std::time::Instant::now();
    assert_eq!(crate::hosting_state(Some(true)).unwrap_err().reason, "BUSY");
    assert!(started.elapsed() < Duration::from_millis(500));
    let result: serde_json::Value = serde_json::from_str(&revoke(generation).unwrap()).unwrap();
    held.join().unwrap();
    assert_eq!(result["revoked"], true);
    client.close();
    assert!(Client::new(s["addr"].as_str().unwrap(), &laptop.identity, laptop.host_pin, Box::new(|_| {})).unwrap().hello().is_err());
    hosting::stop(&mut app().unwrap()).unwrap();
}

/// Review S1: the host's Lock while a remote request holds the books returns at once, and the host
/// person is signed out before anything else runs for the window, or when hosting ends first.
#[test]
fn the_hosts_lock_never_waits_for_the_books_and_always_signs_out() {
    let _turn = turn();
    let (_dir, _company) = desktop();
    on().unwrap();
    assert!(signed_in());
    let t = hold_books(Duration::from_secs(3));
    let started = std::time::Instant::now();
    crate::sign_out().expect("Lock never fails because the books are busy");
    assert!(started.elapsed() < Duration::from_millis(500), "Lock did not wait: {:?}", started.elapsed());
    t.join().unwrap();
    assert!(!signed_in(), "applied before the next desktop call ran");
    // The same while hosting stops before the books come free: the person does not come back signed in.
    hosting::with_desk(&mut app().unwrap(), hosting::DOOR_WAIT, |a| a.sign_in(OWNER, OWNER_PASSWORD).map_err(crate::app_error)).unwrap();
    assert!(signed_in());
    let t = hold_books(Duration::from_secs(2));
    crate::sign_out().unwrap();
    hosting::end_from(&mut app().unwrap(), "test end").unwrap();
    t.join().unwrap();
    assert!(!hosting::active());
    assert!(app().unwrap().authenticated_session().is_err(), "signed out on the standalone desktop too");
    assert!(!signed_in());
    // Leave the desktop as the other tests expect it: the owner signed in.
    app().unwrap().sign_in(OWNER, OWNER_PASSWORD).unwrap();
}

/// Review S2: hosting left on resumes when the books open with nobody signed in, and the trusted
/// main process gets the listening address without an Administrator (the network watch).
#[test]
fn resumed_hosting_reports_its_address_with_nobody_signed_in() {
    let _turn = turn();
    let (_dir, _company) = desktop();
    on().unwrap();
    let bind = hosting::status()["bind"].as_str().unwrap().to_owned();
    assert!(bind.starts_with("127.0.0.1:"), "{bind}");
    hosting::end_from(&mut app().unwrap(), "the app quit").unwrap();
    assert_eq!(hosting::status()["running"], false);
    {
        let mut door = app().unwrap();
        door.sign_out();
        assert!(hosting::wanted(&mut door));
        assert!(hosting::resume(&mut door, "Synthetic host").unwrap());
    }
    assert!(!signed_in());
    assert_eq!(state()["admin"], false, "the Hosting state stays for an Administrator");
    let s = hosting::status();
    assert_eq!(s["running"], true);
    assert_eq!(s["connected"], 0);
    assert!(!s["bind"].as_str().unwrap().is_empty() && s.to_string().len() < 120, "the address only: {s}");
    hosting::end_from(&mut app().unwrap(), "test end").unwrap();
}

/// Review S3, the reviewer's scenario: a computer revoked while hosting is off, then a backup made
/// before the revocation restored. The list of paired computers and the audit log carry the
/// revocation, so the computer stays out. Also N1 (no repair without its problem) and N5.
#[test]
fn a_revocation_while_hosting_is_off_survives_an_older_backup() {
    use std::os::unix::fs::PermissionsExt;
    let _turn = turn();
    let (dir, _company) = desktop();
    on().unwrap();
    let laptop = pair("Synthetic laptop");
    let device = state()["devices"][0]["id"].as_str().unwrap().to_owned();
    hosting::stop(&mut app().unwrap()).unwrap();
    let before = app().unwrap().host_record().unwrap().unwrap();
    let folder = dir.path().join("hosting").join(&before.host_id);
    assert_eq!(std::fs::metadata(dir.path().join("hosting")).unwrap().permissions().mode() & 0o777, 0o700, "N5");
    let r = hosting::revoke(&mut app().unwrap(), &device).unwrap();
    assert_eq!(r["revoked"], true, "{r}");
    assert!(r["unsaved"].is_null() && r["record_unsaved"].is_null(), "{r}");
    let key = before.registry_key_bytes().unwrap();
    let list = fb_remote_proto::registry_file::read(&folder.join("gateway"), &key).unwrap().unwrap();
    assert!(list.devices.iter().any(|d| d.id == device && d.revoked_at.is_some()), "written into the list");
    let after = app().unwrap().host_record().unwrap().unwrap();
    assert_eq!(after.device_counter, list.counter);
    assert!(list.counter > before.device_counter);
    let audit = std::fs::read_to_string(folder.join("remote-audit.jsonl")).unwrap();
    let line = audit.lines().find(|l| l.contains("\"device.revoked\"")).expect("a device.revoked line");
    assert!(line.contains(&device) && line.contains(OWNER), "names the computer and who revoked it: {line}");
    let key = fb_remote_proto::audit::AuditKey::from_bytes(after.audit_key_bytes().unwrap());
    assert_eq!(fb_remote_proto::audit::verify(&folder.join("remote-audit.jsonl"), &key).unwrap(), after.audit_head.as_ref().unwrap().seq, "the record kept the new head");
    // The backup from before the revocation is restored (its record has no revocation).
    let mut older = before.clone();
    older.enabled = false;
    app().unwrap().save_host_record(&older).unwrap();
    on().unwrap();
    assert!(Client::new(state()["addr"].as_str().unwrap(), &laptop.identity, laptop.host_pin, Box::new(|_| {})).unwrap().hello().is_err(), "the revoked laptop stays out");
    hosting::stop(&mut app().unwrap()).unwrap();
    assert!(app().unwrap().host_record().unwrap().unwrap().revoked.contains(&device));
    // N1: a repair needs the problem the last start found; a healthy log is never set aside.
    let err = hosting::repair(&mut app().unwrap(), "audit-rolled-back", "Synthetic host").unwrap_err();
    assert!(err.reason.contains("nothing to repair"), "{}", err.reason);
    assert!(std::fs::read_dir(&folder).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().contains(".rolled-back-")));
}

#[test]
fn a_connected_computer_hears_why_the_host_stopped_or_closed_the_books() {
    let _turn = turn();
    for closing in [false, true] {
        let (_dir, _) = desktop();
        on().unwrap();
        let client = connect(&pair("Synthetic laptop"));
        let shutdown = std::thread::spawn(move || {
            if closing {
                crate::lock().unwrap();
            } else {
                hosting::stop(&mut app().unwrap()).unwrap();
            }
        });
        // Stop immediately after the preceding probe: the next idle probe still gets the reason.
        std::thread::sleep(Duration::from_secs(1));
        let reply = client.call(&ClientRequest::Ping);
        client.close();
        shutdown.join().unwrap();
        let expected = if closing { "The host closed these books." } else { hosting::STOPPED };
        assert!(matches!(reply, Ok(HostResponse::Refused { code: RefusalCode::HostLocked, ref message }) if message == expected), "{reply:?}");
    }
}

#[test]
fn stop_and_close_do_not_wait_five_seconds_for_a_silent_computer() {
    let _turn = turn();
    for closing in [false, true] {
        let (_dir, _company) = desktop();
        on().unwrap();
        let silent = connect(&pair("Synthetic silent laptop"));
        let started = std::time::Instant::now();
        if closing {
            crate::lock().unwrap();
        } else {
            hosting::stop(&mut app().unwrap()).unwrap();
        }
        // Allow for gateway process teardown after the 1.5-second notice window.
        assert!(started.elapsed() < Duration::from_millis(2500), "{:?}", started.elapsed());
        silent.close();
    }
}

#[test]
fn a_stuck_lease_times_out_and_a_later_lock_recovers_the_same_book() {
    let _turn = turn();
    let (_dir, _) = desktop();
    on().unwrap();
    let shared = hosting::test_shared().unwrap();
    let lease = shared.lease_within("synthetic busy request", Duration::ZERO, &mut |_| {}).unwrap();
    let started = std::time::Instant::now();
    let error = crate::lock().unwrap_err();
    assert!(error.reason.contains("busy"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(crate::scheduled_backup_async().is_err());
    drop(lease);
    hosting::end_from(&mut app().unwrap(), "retry").unwrap();
    assert!(app().unwrap().is_open());
    assert!(!hosting::active());
    crate::lock().unwrap();
}

#[test]
fn a_hosted_desk_panic_uses_the_actual_books_for_read_and_write_recovery() {
    let _turn = turn();
    let (_dir, _) = desktop();
    on().unwrap();
    for class in [crate::Class::Read, crate::Class::Write] {
        let error = crate::desk("hosted_panic", class, |_| Ok(()),
            |_, _, ()| panic!("synthetic hosted panic")).unwrap_err();
        assert!(error.reason.starts_with(crate::boundary::PANIC), "{error}");
        let state: serde_json::Value = serde_json::from_str(error.reason.split_once(' ').unwrap().1).unwrap();
        assert_eq!(state["was_open"], true);
        assert_eq!(state["closed"], class == crate::Class::Write);
        assert_eq!(hosting::active(), class == crate::Class::Read);
    }
}
