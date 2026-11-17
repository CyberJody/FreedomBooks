//! Two paired computers and the host's own desktop session against one synthetic book, through the
//! real gateway process, the dispatcher, fb-host's session backend and fb-app's multi-user session
//! host (`SharedHost`). Real sign-in and customer forms; synthetic data only.
mod common;
use common::{Dropper, links, request_id};
use fb_app::App;
use fb_app::identity::Role;
use fb_app::keys::MasterKey;
use fb_app::multiuser::{ClientInfo, Policy, Request, Response, SessionHost, SharedHost};
use fb_core::CompanyId;
use fb_host::SessionBackend;
use fb_remote::client::{self, Client, Paired};
use fb_remote_proto::audit::{AuditKey, AuditLog, Expect};
use fb_remote_proto::link::{HostLink, Start};
use fb_remote_proto::wire::{ClientRequest, HostResponse, RefusalCode};
use fb_remote_proto::{Config, Dispatcher};
use std::sync::Arc;
use std::time::Duration;

const OWNER: &str = "Synthetic owner";
const OWNER_PASSWORD: &str = "Synthetic owner user password";
const BOOK_PASSWORD: &str = "Synthetic book password";
const PASSWORD: &str = "Synthetic individual password";
const KEEPER: &str = "Synthetic bookkeeper";
const READER: &str = "Synthetic reader";

struct Office {
    dir: tempfile::TempDir,
    audit_key: AuditKey,
    shared: SharedHost,
    link: HostLink<SessionBackend>,
    first: CompanyId,
    second: CompanyId,
}

fn office() -> Office {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("books/default.fbbook");
    let mut app = App::new();
    app.backups.configure(dir.path()).unwrap();
    app.device_key = Some(MasterKey::new([41; 32]));
    app.create_protected_book(&path, OWNER, "First synthetic company", Some(BOOK_PASSWORD)).unwrap();
    app.enroll_owner(OWNER_PASSWORD).unwrap();
    let owner = app.authenticated_session().unwrap().actor;
    let first = app.first_company().unwrap().unwrap();
    let second = fb_core::services::create_company(&mut app.open_mut().unwrap().book, owner, "Second synthetic company", "USD", false).unwrap().id;
    app.save_desktop_user(None, KEEPER, Role::Bookkeeper, true, Some(PASSWORD), &[first]).unwrap();
    app.sign_in(OWNER, OWNER_PASSWORD).unwrap();
    app.save_desktop_user(None, READER, Role::Readonly, true, Some(PASSWORD), &[first]).unwrap();
    let shared = SharedHost::new(SessionHost::new(app, Policy::default()));
    let audit_key = AuditKey::generate().unwrap();
    let audit = AuditLog::open(&dir.path().join("remote-audit.jsonl"), &audit_key, Expect::New).unwrap();
    let dispatcher = Arc::new(Dispatcher::new(SessionBackend::new(shared.clone()), Config { host_name: "Office PC".into(), ..Config::default() }, Box::new(audit)));
    let state = dir.path().join("remote");
    std::fs::create_dir_all(&state).unwrap();
    let link = HostLink::start(dispatcher, Start::new(common::exe(), "127.0.0.1:0", None, state, "Office PC").unwrap()).unwrap();
    Office { dir, audit_key, shared, link, first, second }
}

impl Office {
    fn pair(&self, name: &str) -> Paired {
        let code = self.link.arm_pairing(OWNER).unwrap();
        client::pair(&self.link.addr, &code, name).unwrap()
    }
    /// Who the book's own audit says created each customer, oldest first.
    fn creators(&self) -> Vec<String> {
        let mut host = self.shared.lease("test", &mut |_| {}).unwrap();
        let open = host.host_app().open.as_mut().unwrap();
        open.book.read(|q| q.prepare("SELECT actor_name FROM ledger_auditevent WHERE action='customer.created' ORDER BY id")?.query_map([], |r| r.get(0))?.collect::<Result<Vec<String>, _>>()).unwrap()
    }
    fn customers(&self) -> i64 {
        let mut host = self.shared.lease("test", &mut |_| {}).unwrap();
        let open = host.host_app().open.as_mut().unwrap();
        open.book.read(|q| q.query_row("SELECT count(*) FROM directory_customer", [], |r| r.get(0))).unwrap()
    }
}

fn connect(addr: &str, p: &Paired) -> Client {
    let (notify, _) = links();
    let c = Client::new(addr, &p.identity, p.host_pin, notify).unwrap().with_retry(Duration::from_secs(10));
    c.hello().unwrap();
    c
}

fn render(c: &Client, target: &str) -> HostResponse {
    c.call(&ClientRequest::Render { target: target.into() }).unwrap()
}

fn token(html: &str) -> String {
    html.split("name=\"fb_command_token\" value=\"").nth(1).and_then(|s| s.split('"').next()).expect("form token").to_owned()
}

fn page(r: &HostResponse) -> &str {
    match r {
        HostResponse::Page { html, status: 200, .. } => html,
        other => panic!("expected a page, got {other:?}"),
    }
}

/// Render the form page, then post it with the page's own token, as the thin client does.
fn submit(c: &Client, path: &str, fields: &[(&str, &str)], id: &str) -> HostResponse {
    let t = token(page(&render(c, path)));
    submit_with(c, path, &t, fields, id)
}

fn submit_with(c: &Client, path: &str, token: &str, fields: &[(&str, &str)], id: &str) -> HostResponse {
    let command = serde_json::json!({"type": "SubmitForm", "path": path, "token": token, "fields": fields.iter().map(|(k, v)| [k, v]).collect::<Vec<_>>()}).to_string();
    c.call(&ClientRequest::Run { request_id: id.into(), command }).unwrap()
}

fn sign_in(c: &Client, user: &str, password: &str) -> HostResponse {
    submit(c, "/desktop/sign-in/", &[("action", "signin"), ("username", user), ("password", password)], &request_id())
}

fn outcome(r: &HostResponse) -> serde_json::Value {
    match r {
        HostResponse::Outcome { outcome } => serde_json::from_str(outcome).unwrap(),
        other => panic!("expected an outcome, got {other:?}"),
    }
}

#[test]
fn two_computers_and_the_host_user_share_one_book_with_separate_sessions() {
    let o = office();
    let first = o.first.to_db();
    let new_customer = format!("/c/{first}/lists/customers/new/");
    let home = format!("/c/{first}/home/");
    // The host's own desktop session, signed in as the owner.
    let local = {
        let mut host = o.shared.lease("test", &mut |_| {}).unwrap();
        let id = host.open_session(ClientInfo::local("Host desktop")).unwrap();
        host.dispatch(&id, Request::SignIn { username: OWNER.into(), password: OWNER_PASSWORD.into() }).unwrap();
        id
    };
    let (a, b) = (o.pair("Front desk"), o.pair("Back office"));
    let ca = connect(&o.link.addr, &a);
    let cb = connect(&o.link.addr, &b);
    // Not signed in yet: every page is the sign-in screen.
    assert!(matches!(render(&ca, &home), HostResponse::Page { status: 303, ref html, .. } if html == "/desktop/sign-in/"));
    assert_eq!(outcome(&sign_in(&ca, KEEPER, "not the password"))["outcome"], "invalid");
    assert_eq!(outcome(&sign_in(&ca, KEEPER, PASSWORD))["outcome"], "redirect");
    assert_eq!(outcome(&sign_in(&cb, READER, PASSWORD))["outcome"], "redirect");
    assert!(page(&render(&ca, &home)).contains("First synthetic company"));

    // The bookkeeper saves a customer through the real form; a retry of the same request is
    // answered from the first run and saves nothing more.
    let before = o.customers();
    let id = request_id();
    let t = token(page(&render(&ca, &new_customer)));
    let saved = submit_with(&ca, &new_customer, &t, &[("display_name", "Remote customer")], &id);
    assert_eq!(outcome(&saved)["outcome"], "redirect", "{saved:?}");
    assert_eq!(submit_with(&ca, &new_customer, &t, &[("display_name", "Remote customer")], &id), saved);
    assert_eq!(o.customers(), before + 1);

    // The reader cannot write; the host's own user can, at the same time.
    let refused = submit(&cb, &new_customer, &[("display_name", "Reader attempt")], &request_id());
    assert!(matches!(refused, HostResponse::Refused { code: RefusalCode::Refused, .. }) || outcome(&refused)["outcome"] == "retry", "{refused:?}");
    {
        let mut host = o.shared.lease("test", &mut |_| {}).unwrap();
        let t = host.with_session(&local, |app| app.tokens.mint(&new_customer)).unwrap();
        let command = fb_app::commands::Command::SubmitForm { path: new_customer.clone(), token: t, fields: vec![("display_name".into(), "Host customer".into())] };
        assert!(matches!(host.dispatch(&local, Request::Run { command }).unwrap(), Response::Outcome { .. }));
    }
    assert_eq!(o.customers(), before + 2);
    assert_eq!(o.creators(), vec![KEEPER.to_owned(), OWNER.to_owned()], "each change is attributed to its own session's user");

    // Company boundaries and host-only surfaces hold for remote sessions.
    let second = render(&ca, &format!("/c/{}/home/", o.second.to_db()));
    assert!(matches!(second, HostResponse::Refused { .. }), "{second:?}");
    assert!(matches!(render(&ca, "/settings/book-protection/"), HostResponse::Refused { code: RefusalCode::HostOnly, .. }));
    assert!(matches!(render(&ca, &format!("/c/{first}/backups/")), HostResponse::Refused { code: RefusalCode::HostOnly, .. }));

    // Signing out one remote computer leaves the other and the host user signed in.
    assert!(matches!(ca.call(&ClientRequest::SignOut).unwrap(), HostResponse::SignedOut));
    assert!(matches!(render(&ca, &home), HostResponse::Page { status: 303, .. }));
    assert!(page(&render(&cb, &home)).contains("First synthetic company"));
    let mut host = o.shared.lease("test", &mut |_| {}).unwrap();
    let Response::Status(status) = host.dispatch(&local, Request::Status).unwrap() else { panic!() };
    assert!(status.signed_in, "the host's own user is still signed in");
    drop(host);
    o.link.stop();
    let lines = fb_remote_proto::audit::verify(&o.dir.path().join("remote-audit.jsonl"), &o.audit_key).unwrap();
    assert!(lines > 6);
    let audit = std::fs::read_to_string(o.dir.path().join("remote-audit.jsonl")).unwrap();
    for kind in ["pair.armed", "pair.succeeded", "connect", "sign_in.failed", "sign_in.succeeded", "request.replayed", "sign_out"] {
        assert!(audit.contains(&format!("\"kind\":\"{kind}\"")), "{kind} audited");
    }
    assert!(!audit.contains(PASSWORD) && !audit.contains(OWNER_PASSWORD), "no password in the audit");
}

#[test]
fn a_dropped_answer_after_commit_saves_the_customer_once() {
    let o = office();
    let first = o.first.to_db();
    let new_customer = format!("/c/{first}/lists/customers/new/");
    let a = o.pair("Front desk");
    let relay = Dropper::start(o.link.addr.clone());
    let ca = connect(&relay.addr, &a);
    assert_eq!(outcome(&sign_in(&ca, KEEPER, PASSWORD))["outcome"], "redirect");
    let before = o.customers();
    let t = token(page(&render(&ca, &new_customer)));
    // The host commits and answers; the answer is lost; the client reconnects and asks again with
    // the same request id.
    relay.arm();
    let answer = submit_with(&ca, &new_customer, &t, &[("display_name", "Exactly once")], &request_id());
    assert_eq!(outcome(&answer)["outcome"], "redirect", "{answer:?}");
    assert_eq!(o.customers(), before + 1, "saved once");
    assert!(relay.connections() >= 2, "it reconnected");
    // Still signed in on the resumed session.
    assert!(page(&render(&ca, &format!("/c/{first}/home/"))).contains("First synthetic company"));
}

#[test]
fn closing_the_book_on_the_host_ends_remote_sessions() {
    let o = office();
    let a = o.pair("Front desk");
    let ca = connect(&o.link.addr, &a);
    assert_eq!(outcome(&sign_in(&ca, KEEPER, PASSWORD))["outcome"], "redirect");
    o.shared.lease("test", &mut |_| {}).unwrap().close_book();
    let after = render(&ca, &format!("/c/{}/home/", o.first.to_db()));
    assert!(matches!(after, HostResponse::Refused { code: RefusalCode::Unauthenticated | RefusalCode::HostLocked, .. }), "{after:?}");
}

/// The reviewer's [E] probe (S1), end to end: an Administrator's stored answer must not be
/// replayed to the Bookkeeper who signs in next on the same computer and session.
#[test]
fn an_administrators_answer_is_never_replayed_to_the_next_user_on_the_same_computer() {
    let o = office();
    let p = o.pair("Shared front desk");
    let c = connect(&o.link.addr, &p);
    assert_eq!(outcome(&sign_in(&c, OWNER, OWNER_PASSWORD))["outcome"], "redirect");
    let users = "/desktop/users/";
    let t = token(page(&render(&c, users)));
    let id = request_id();
    let first = submit_with(&c, users, &t, &[("action", "edit")], &id);
    assert!(matches!(first, HostResponse::Outcome { .. }), "the administrator's request: {first:?}");
    assert!(matches!(c.call(&ClientRequest::SignOut).unwrap(), HostResponse::SignedOut));
    assert_eq!(outcome(&sign_in(&c, KEEPER, PASSWORD))["outcome"], "redirect");
    assert!(matches!(render(&c, users), HostResponse::Refused { .. }), "the bookkeeper cannot open the user editor");
    let replay = submit_with(&c, users, &t, &[("action", "edit")], &id);
    assert_ne!(replay, first, "the administrator's answer was replayed to the bookkeeper");
    assert!(matches!(replay, HostResponse::Refused { .. }), "{replay:?}");
    o.link.stop();
    let audit = std::fs::read_to_string(o.dir.path().join("remote-audit.jsonl")).unwrap();
    assert!(!audit.contains("\"kind\":\"request.replayed\""), "nothing was replayed");
}

/// S4 and S5 against the real router and book: an unclassified path and a host-only form action
/// are refused before anything reaches the book.
#[test]
fn unclassified_screens_and_host_only_actions_never_reach_the_book() {
    let o = office();
    let first = o.first.to_db();
    let p = o.pair("Front desk");
    let c = connect(&o.link.addr, &p);
    assert_eq!(outcome(&sign_in(&c, KEEPER, PASSWORD))["outcome"], "redirect");
    let events = || {
        let mut host = o.shared.lease("test", &mut |_| {}).unwrap();
        let open = host.host_app().open.as_mut().unwrap();
        open.book.read(|q| q.query_row("SELECT count(*) FROM ledger_auditevent", [], |r| r.get::<_, i64>(0))).unwrap()
    };
    let before = events();
    for target in [format!("/c/{first}/no-such-screen/"), format!("/c/{first}/backups/"), format!("/c/{first}/attachments/{first}/download/"), "/help/report/x/".into()] {
        assert!(matches!(render(&c, &target), HostResponse::Refused { code: RefusalCode::HostOnly, .. }), "{target}");
    }
    let invoice = "0195b4a4-55af-719e-96cb-a2f946b067bf";
    for action in ["email_send", "email_draft"] {
        let r = submit_with(&c, &format!("/c/{first}/invoices/{invoice}/email-preview/"), "t", &[("action", action)], &request_id());
        assert!(matches!(r, HostResponse::Refused { code: RefusalCode::HostOnly, .. }), "{action}: {r:?}");
    }
    assert_eq!(events(), before, "nothing was written to the book");
    assert!(page(&render(&c, &format!("/c/{first}/home/"))).contains("First synthetic company"));
}
