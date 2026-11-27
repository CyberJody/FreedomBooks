//! Development and smoke host only: a synthetic protected book in `<dir>`, served through the real
//! dispatcher and `fb-remote gateway` on 127.0.0.1. Not a production host: its book password and user
//! passwords are fixed synthetic strings, and the host identity is generated fresh each run.
//!
//! `cargo run -p fb-host --example dev_host -- <empty dir> <path to fb-remote>`
//!
//! Prints one JSON line `{"addr","code","fingerprint","company"}`. Then reads commands on stdin:
//! `pair` (prints a new `{"code"}`), `revoke <device id>`, `devices`, `end-sessions`, and stops at end of input.
use fb_app::App;
use fb_app::identity::Role;
use fb_app::keys::MasterKey;
use fb_app::multiuser::{Policy, SessionHost, SharedHost};
use fb_host::SessionBackend;
use fb_remote_proto::audit::{AuditKey, AuditLog, Expect};
use fb_remote_proto::link::{HostLink, Start};
use fb_remote_proto::{Config, Dispatcher};
use std::io::{BufRead, Write};
use std::sync::Arc;
use std::time::Duration;

pub const OWNER: &str = "Synthetic owner";
pub const OWNER_PASSWORD: &str = "Synthetic owner user password";
pub const KEEPER: &str = "Synthetic bookkeeper";
pub const PASSWORD: &str = "Synthetic individual password";

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(dir), Some(exe)) = (args.next(), args.next()) else {
        eprintln!("usage: dev_host <empty dir> <fb-remote> [--idle-seconds N] [--host-name NAME]");
        std::process::exit(2);
    };
    let mut config = Config { host_name: "Synthetic office host".into(), ..Config::default() };
    while let Some(option) = args.next() {
        let value = args.next().expect("option value");
        match option.as_str() {
            "--idle-seconds" => config.idle = Duration::from_secs(value.parse().expect("idle seconds")),
            "--host-name" => config.host_name = value,
            _ => panic!("unknown option: {option}"),
        }
    }
    let host_name = config.host_name.clone();
    let dir = std::path::PathBuf::from(dir);
    let mut app = App::new();
    app.backups.configure(&dir).expect("backups folder");
    app.device_key = Some(MasterKey::new([41; 32]));
    app.create_protected_book(&dir.join("books/default.fbbook"), OWNER, "Synthetic remote company", Some("Synthetic book password")).expect("synthetic book");
    app.enroll_owner(OWNER_PASSWORD).expect("owner password");
    let first = app.first_company().expect("company").expect("company");
    app.save_desktop_user(None, KEEPER, Role::Bookkeeper, true, Some(PASSWORD), &[first]).expect("bookkeeper");
    let shared = SharedHost::new(SessionHost::new(app, Policy::default()));
    // A development host keeps its audit key only for this run (a real host keeps it sealed).
    let audit_key = AuditKey::generate().expect("audit key");
    let audit = AuditLog::open(&dir.join("remote-audit.jsonl"), &audit_key, Expect::New).expect("audit log");
    let dispatcher = Arc::new(Dispatcher::new(SessionBackend::new(shared), config, Box::new(audit)));
    let state = dir.join("remote");
    std::fs::create_dir_all(&state).expect("state dir");
    let link = HostLink::start(dispatcher.clone(), Start::new(exe.into(), "127.0.0.1:0", None, state, &host_name).expect("start")).expect("listener");
    let code = link.arm_pairing(OWNER).expect("pairing code");
    let mut out = std::io::stdout().lock();
    let line = serde_json::json!({"addr": link.addr, "code": fb_remote_proto::code::display(&code), "fingerprint": link.fingerprint, "company": first.to_db()});
    writeln!(out, "{line}").and_then(|()| out.flush()).expect("stdout");
    for command in std::io::stdin().lock().lines() {
        let Ok(command) = command else { break };
        let answer = |r: Result<serde_json::Value, std::io::Error>| r.unwrap_or_else(|e| serde_json::json!({"error": e.to_string()}));
        let reply = match command.split_once(' ').unwrap_or((command.as_str(), "")) {
            ("pair", _) => answer(link.arm_pairing(OWNER).map(|c| serde_json::json!({"code": fb_remote_proto::code::display(&c)}))),
            ("revoke", id) => answer(link.revoke(id, OWNER).map(|r| serde_json::json!({"revoked": r.revoked, "already": r.already, "unsaved": r.unsaved}))),
            ("end-sessions", _) => {
                dispatcher.end_all("synthetic test");
                serde_json::json!({"sessions": dispatcher.session_count()})
            }
            ("devices", _) => answer(link.devices().map(|d| serde_json::json!({"devices": d.into_iter().map(|d| serde_json::json!({"id": d.id, "name": d.name, "revoked": d.revoked_at.is_some()})).collect::<Vec<_>>()}))),
            _ => serde_json::json!({"error": "unknown command"}),
        };
        writeln!(out, "{reply}").and_then(|()| out.flush()).expect("stdout");
    }
    link.stop();
}
