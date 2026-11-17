//! Multi-user step 1: two (or more) sessions against one synthetic book in one process, through the
//! session host. Every test drives the sessions with the same requests a transport would send.
use fb_app::App;
use fb_app::commands::{Command, Outcome, Query};
use fb_app::identity::{Permission, Role, SIGN_IN};
use fb_app::keys::MasterKey;
use fb_app::multiuser::{ClientInfo, EndReason, HostError, Policy, QueueProgress, Request, Response, SessionHost, SessionId, SharedHost};
use fb_core::{CompanyId, UserId};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const OWNER: &str = "Synthetic owner";
const OWNER_PASSWORD: &str = "Synthetic owner user password";
const BOOK_PASSWORD: &str = "Synthetic book password";
const PASSWORD: &str = "Synthetic individual password";
const KEEPER: &str = "Synthetic bookkeeper";
const READER: &str = "Synthetic reader";

/// The harness: one host, one book with two companies, an administrator who belongs to both, a
/// bookkeeper and a reader who belong to the first only.
struct Office {
    _dir: tempfile::TempDir,
    host: SessionHost,
    path: PathBuf,
    first: CompanyId,
    second: CompanyId,
    owner: UserId,
    keeper: UserId,
}

fn office() -> Office {
    office_with(Policy::default())
}

fn office_with(policy: Policy) -> Office {
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
    let keeper = app.save_desktop_user(None, KEEPER, Role::Bookkeeper, true, Some(PASSWORD), &[first]).unwrap();
    app.sign_in(OWNER, OWNER_PASSWORD).unwrap();
    app.save_desktop_user(None, READER, Role::Readonly, true, Some(PASSWORD), &[first]).unwrap();
    Office { _dir: dir, host: SessionHost::new(app, policy), path, first, second, owner, keeper }
}

impl Office {
    fn session(&mut self, local: bool) -> SessionId {
        let client = if local { ClientInfo::local("Host desktop") } else { ClientInfo::remote("Synthetic laptop", Some("device-1")) };
        self.host.open_session(client).unwrap()
    }
    fn signed_in(&mut self, name: &str, local: bool) -> SessionId {
        let id = self.session(local);
        let password = if name == OWNER { OWNER_PASSWORD } else { PASSWORD };
        self.host.dispatch(&id, Request::SignIn { username: name.into(), password: password.into() }).unwrap();
        assert!(self.status(&id).signed_in, "{name} signed in");
        id
    }
    fn status(&mut self, id: &SessionId) -> fb_app::multiuser::SessionStatus {
        match self.host.dispatch(id, Request::Status).unwrap() {
            Response::Status(s) => s,
            other => panic!("not a status: {other:?}"),
        }
    }
    /// What rendering the form's page gives the client: a live token for `path`.
    fn form(&mut self, id: &SessionId, path: &str) -> String {
        self.host.with_session(id, |app| app.tokens.mint(path)).unwrap()
    }
    fn post_with(&mut self, id: &SessionId, path: &str, token: String, fields: &[(&str, &str)]) -> Result<(Outcome, bool), HostError> {
        let command = Command::SubmitForm { path: path.into(), token, fields: fields.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect() };
        match self.host.dispatch(id, Request::Run { command })? {
            Response::Outcome { outcome, replayed } => Ok((outcome, replayed)),
            other => panic!("not an outcome: {other:?}"),
        }
    }
    fn post(&mut self, id: &SessionId, path: &str, fields: &[(&str, &str)]) -> Result<Outcome, HostError> {
        let token = self.form(id, path);
        self.post_with(id, path, token, fields).map(|(o, _)| o)
    }
    fn render(&mut self, id: &SessionId, target: &str) -> Result<(String, u16), HostError> {
        match self.host.dispatch(id, Request::Render { target: target.into() })? {
            Response::Page { html, status, .. } => Ok((html, status)),
            other => panic!("not a page: {other:?}"),
        }
    }
    fn new_customer(&mut self, id: &SessionId, company: CompanyId, name: &str) -> Result<String, HostError> {
        let path = format!("/c/{}/lists/customers/new/", company.to_db());
        match self.post(id, &path, &[("display_name", name)])? {
            Outcome::Redirect { location } => Ok(location),
            other => panic!("customer not saved: {other:?}"),
        }
    }
    fn sql<T: fb_db::rusqlite::types::FromSql>(&mut self, sql: &str, params: &[&str]) -> T {
        let open = self.host.host_app().open.as_mut().unwrap();
        open.book.read(|q| q.query_row(sql, fb_db::rusqlite::params_from_iter(params), |r| r.get(0))).unwrap()
    }
    fn customer_name(&mut self, location: &str) -> String {
        let id = location.trim_end_matches('/').rsplit('/').next().unwrap().to_owned();
        self.sql("SELECT display_name FROM directory_customer WHERE id=?1", &[&id])
    }
}

fn notices(s: &fb_app::multiuser::SessionStatus) -> String {
    s.notices.join(" ")
}

#[test]
fn sign_out_lock_and_navigation_of_one_session_leave_the_other_alone() {
    let mut o = office();
    let a = o.signed_in(OWNER, true);
    let b = o.signed_in(KEEPER, false);
    // Navigation and company context are per session.
    let (html, status) = o.render(&a, &format!("/c/{}/home/", o.second.to_db())).unwrap();
    assert_eq!(status, 200, "{html}");
    o.render(&b, &format!("/c/{}/home/", o.first.to_db())).unwrap();
    o.render(&a, "/desktop/session/").unwrap();
    o.render(&b, "/desktop/session/").unwrap();
    assert_eq!(o.status(&a).company, Some(o.second));
    assert_eq!(o.status(&b).company, Some(o.first));
    // Pages render under each person's own name.
    let (a_home, _) = o.render(&a, &format!("/c/{}/home/", o.first.to_db())).unwrap();
    let (b_home, _) = o.render(&b, &format!("/c/{}/home/", o.first.to_db())).unwrap();
    assert!(a_home.contains(OWNER) && !a_home.contains(KEEPER));
    assert!(b_home.contains(KEEPER) && !b_home.contains(OWNER));
    // A's open form survives B's sign-out; B's does not.
    let path = format!("/c/{}/lists/customers/new/", o.first.to_db());
    let a_token = o.form(&a, &path);
    let b_token = o.form(&b, &path);
    o.host.dispatch(&b, Request::SignOut).unwrap();
    assert!(!o.status(&b).signed_in);
    assert!(o.status(&a).signed_in);
    assert!(o.app_book_open());
    let (saved, _) = o.post_with(&a, &path, a_token, &[("display_name", "Saved after the other sign-out")]).unwrap();
    assert!(matches!(saved, Outcome::Redirect { .. }), "{saved:?}");
    // B's form expired with B's sign-out, and signing in again does not revive it.
    o.host.dispatch(&b, Request::SignIn { username: KEEPER.into(), password: PASSWORD.into() }).unwrap();
    let refused = o.post_with(&b, &path, b_token, &[("display_name", "Must not save")]);
    assert!(matches!(refused, Err(HostError::App(_))), "{refused:?}");
    // Lock signs only that session out; the book stays open for the other.
    o.host.dispatch(&a, Request::Lock).unwrap();
    assert!(!o.status(&a).signed_in);
    assert!(o.status(&b).signed_in);
    assert!(o.app_book_open());
    assert!(o.new_customer(&b, o.first, "Saved after the other lock").is_ok());
    // A signed-out session sees the sign-in page, never the other's pages.
    let (html, status) = o.render(&a, &format!("/c/{}/home/", o.first.to_db())).unwrap();
    assert_eq!((html.as_str(), status), (SIGN_IN, 303));
}

impl Office {
    fn app_book_open(&self) -> bool {
        self.host.app().is_open()
    }
}

#[test]
fn company_membership_is_enforced_per_session() {
    let mut o = office();
    let a = o.signed_in(OWNER, true);
    let b = o.signed_in(KEEPER, false);
    let second = o.second;
    // A works in the second company in between B's attempts: B never gets A's company context.
    o.render(&a, &format!("/c/{}/home/", second.to_db())).unwrap();
    assert!(o.render(&b, &format!("/c/{}/home/", second.to_db())).is_err());
    assert!(o.host.dispatch(&b, Request::Query { query: Query::Accounts { company_id: second.to_db() } }).is_err());
    let path = format!("/c/{}/lists/customers/new/", second.to_db());
    assert!(o.post(&b, &path, &[("display_name", "Cross-company attempt")]).is_err());
    let Response::Value(companies) = o.host.dispatch(&b, Request::Query { query: Query::Companies }).unwrap() else { panic!() };
    assert_eq!(companies.as_array().unwrap().len(), 1);
    let Response::Value(companies) = o.host.dispatch(&a, Request::Query { query: Query::Companies }).unwrap() else { panic!() };
    assert_eq!(companies.as_array().unwrap().len(), 2);
    assert!(o.new_customer(&a, second, "Owner's second-company customer").is_ok());
    let foreign: i64 = o.sql("SELECT count(*) FROM ledger_auditevent WHERE company_id=?1 AND actor_id=?2", &[&second.to_db(), &o.keeper.to_db()]);
    assert_eq!(foreign, 0);
}

#[test]
fn audit_records_each_sessions_own_actor_when_interleaved() {
    let mut o = office();
    let a = o.signed_in(OWNER, true);
    let b = o.signed_in(KEEPER, false);
    let first = o.first;
    for round in 0..3 {
        let ca = o.new_customer(&a, first, &format!("Owner customer {round}")).unwrap();
        let cb = o.new_customer(&b, first, &format!("Bookkeeper customer {round}")).unwrap();
        for (location, actor, name) in [(ca, o.owner, OWNER), (cb, o.keeper, KEEPER)] {
            let id = location.trim_end_matches('/').rsplit('/').next().unwrap().to_owned();
            let target = format!("customer:{id}");
            let (by, by_name): (String, String) = {
                let open = o.host.host_app().open.as_mut().unwrap();
                open.book
                    .read(|q| q.query_row("SELECT actor_id,actor_name FROM ledger_auditevent WHERE action='customer.created' AND target=?1", [&target], |r| Ok((r.get(0)?, r.get(1)?))))
                    .unwrap()
            };
            assert_eq!(by, actor.to_db());
            assert_eq!(by_name, name);
        }
    }
    // A read-only session's write is refused while the bookkeeper's next write still succeeds.
    let r = o.signed_in(READER, false);
    assert!(o.new_customer(&r, first, "Reader attempt").is_err());
    assert!(o.new_customer(&b, first, "Bookkeeper after the reader").is_ok());
}

#[test]
fn concurrent_edits_of_one_record_refuse_the_stale_one() {
    let mut o = office();
    let a = o.signed_in(OWNER, true);
    let b = o.signed_in(KEEPER, false);
    let location = o.new_customer(&a, o.first, "Original name").unwrap();
    let edit = format!("{location}edit/");
    // Both open the edit form; A saves first.
    let a_token = o.form(&a, &edit);
    let b_token = o.form(&b, &edit);
    let (saved, _) = o.post_with(&a, &edit, a_token, &[("display_name", "Owner's change")]).unwrap();
    assert!(matches!(saved, Outcome::Redirect { .. }), "{saved:?}");
    let audits: i64 = o.sql("SELECT count(*) FROM ledger_auditevent", &[]);
    let (stale, replayed) = o.post_with(&b, &edit, b_token.clone(), &[("display_name", "Bookkeeper's change")]).unwrap();
    assert!(!replayed);
    let Outcome::Retry { message, token: None } = stale else { panic!("stale edit was not refused: {stale:?}") };
    assert!(message.contains(OWNER) && message.contains("nothing was saved"), "{message}");
    assert_eq!(o.customer_name(&location), "Owner's change");
    assert_eq!(o.sql::<i64>("SELECT count(*) FROM ledger_auditevent", &[]), audits);
    // The refused form cannot be sent again; reopening shows the change and then saves.
    assert!(o.post_with(&b, &edit, b_token, &[("display_name", "Bookkeeper's change")]).is_err());
    let early = o.form(&a, &edit);
    let fresh = o.form(&b, &edit);
    let (saved, _) = o.post_with(&b, &edit, fresh, &[("display_name", "Bookkeeper's change")]).unwrap();
    assert!(matches!(saved, Outcome::Redirect { .. }), "{saved:?}");
    assert_eq!(o.customer_name(&location), "Bookkeeper's change");
    // A's form opened before B's save is stale too.
    let (stale, _) = o.post_with(&a, &edit, early, &[("display_name", "Owner, stale")]).unwrap();
    assert!(matches!(stale, Outcome::Retry { token: None, .. }), "{stale:?}");
    assert_eq!(o.customer_name(&location), "Bookkeeper's change");
    // A person's own earlier forms are never stale because of their own saves.
    let one = o.form(&a, &edit);
    let two = o.form(&a, &edit);
    assert!(matches!(o.post_with(&a, &edit, one, &[("display_name", "Owner once")]).unwrap().0, Outcome::Redirect { .. }));
    assert!(matches!(o.post_with(&a, &edit, two, &[("display_name", "Owner twice")]).unwrap().0, Outcome::Redirect { .. }));
    assert_eq!(o.customer_name(&location), "Owner twice");
    // Different records do not interfere.
    let other = o.new_customer(&b, o.first, "Another record").unwrap();
    let other_edit = format!("{other}edit/");
    let b_other = o.form(&b, &other_edit);
    let a_edit = o.form(&a, &edit);
    assert!(matches!(o.post_with(&a, &edit, a_edit, &[("display_name", "Owner thrice")]).unwrap().0, Outcome::Redirect { .. }));
    assert!(matches!(o.post_with(&b, &other_edit, b_other, &[("display_name", "Other renamed")]).unwrap().0, Outcome::Redirect { .. }));
}

#[test]
fn concurrent_journal_edits_keep_the_cores_revision_check() {
    let mut o = office();
    let a = o.signed_in(OWNER, true);
    let b = o.signed_in(KEEPER, false);
    let c = o.first.to_db();
    // Typed journal saves carry an expected revision: the second writer of revision 0 is refused by
    // the core even outside the host's own record check.
    let accounts: Vec<String> = {
        let Response::Value(v) = o.host.dispatch(&a, Request::Query { query: Query::Accounts { company_id: c.clone() } }).unwrap() else { panic!() };
        v.as_array().unwrap().iter().map(|a| a["id"].as_str().unwrap().to_owned()).collect()
    };
    if accounts.len() < 2 {
        return; // empty starter chart: nothing to post against
    }
    let save = |journal: Option<String>, memo: &str, revision: i64| {
        serde_json::from_value::<Command>(serde_json::json!({"type":"SaveJournal","company_id":c,"journal_id":journal,"date":"2026-01-05","memo":memo,"entry_number":"","adjusting":false,"request_key":null,"expected_revision":revision,
            "lines":[{"account_id":accounts[0],"debit":"10.00","credit":"0","memo":""},{"account_id":accounts[1],"debit":"0","credit":"10.00","memo":""}]})).unwrap()
    };
    let created = o.host.with_session(&a, |app| app.run(save(None, "Draft", 0))).unwrap().unwrap();
    let Outcome::Done { data } = created else { panic!() };
    let journal = data["id"].as_str().unwrap().to_owned();
    o.host.with_session(&a, |app| app.run(save(Some(journal.clone()), "Owner edit", 0))).unwrap().unwrap();
    let stale = o.host.with_session(&b, |app| app.run(save(Some(journal.clone()), "Bookkeeper edit", 0))).unwrap();
    assert!(stale.is_err(), "{stale:?}");
}

#[test]
fn duplicate_submits_are_answered_once() {
    let mut o = office();
    let b = o.signed_in(KEEPER, false);
    let path = format!("/c/{}/lists/customers/new/", o.first.to_db());
    let token = o.form(&b, &path);
    let fields = [("display_name", "Exactly once")];
    let (first, replayed) = o.post_with(&b, &path, token.clone(), &fields).unwrap();
    assert!(!replayed && matches!(first, Outcome::Redirect { .. }));
    // The client lost the answer and sends the identical post again.
    let (again, replayed) = o.post_with(&b, &path, token.clone(), &fields).unwrap();
    assert!(replayed);
    assert_eq!(again, first);
    let n: i64 = o.sql("SELECT count(*) FROM directory_customer WHERE display_name='Exactly once'", &[]);
    assert_eq!(n, 1);
    // A different post on the spent token is not a repeat: it is refused.
    assert!(o.post_with(&b, &path, token.clone(), &[("display_name", "Changed on resend")]).is_err());
    // Another session cannot replay this session's post.
    let a = o.signed_in(OWNER, true);
    assert!(o.post_with(&a, &path, token.clone(), &fields).is_err());
    // A sign-out forgets the remembered answers.
    o.host.dispatch(&b, Request::SignOut).unwrap();
    o.host.dispatch(&b, Request::SignIn { username: KEEPER.into(), password: PASSWORD.into() }).unwrap();
    assert!(o.post_with(&b, &path, token, &fields).is_err());
    let n: i64 = o.sql("SELECT count(*) FROM directory_customer WHERE display_name='Exactly once'", &[]);
    assert_eq!(n, 1);
}

#[test]
fn revocation_mid_session_signs_the_user_out_and_permissions_are_live() {
    let mut o = office();
    let a = o.signed_in(OWNER, true);
    let b = o.signed_in(KEEPER, false);
    let b2 = o.signed_in(KEEPER, false);
    let r = o.signed_in(READER, false);
    let path = format!("/c/{}/lists/customers/new/", o.first.to_db());
    let pending = o.form(&b, &path);
    // Permission is checked against the book on every request: a role changed underneath (no roster
    // hook at all) takes effect on the next request.
    o.host.with_session(&a, |app| {
        app.open_mut().unwrap().book.write(|tx| tx.execute("UPDATE desktop_user SET role='readonly' WHERE user_id=?1", [o.keeper.to_db()])).unwrap();
    }).unwrap();
    let refused = o.post(&b, &path, &[("display_name", "Written as read-only")]);
    assert!(refused.is_err(), "{refused:?}");
    o.host.with_session(&a, |app| {
        app.open_mut().unwrap().book.write(|tx| tx.execute("UPDATE desktop_user SET role='bookkeeper' WHERE user_id=?1", [o.keeper.to_db()])).unwrap();
    }).unwrap();
    // The administrator disables the bookkeeper through the Users page.
    let (first, keeper) = (o.first, o.keeper);
    o.host.with_session(&a, |app| app.save_desktop_user(Some(keeper), KEEPER, Role::Bookkeeper, false, None, &[first])).unwrap().unwrap();
    for id in [&b, &b2] {
        let s = o.status(id);
        assert!(!s.signed_in);
        assert!(notices(&s).contains("An administrator changed your user account"), "{s:?}");
    }
    // The reader, a different user, keeps working; the acting administrator is signed out as on the desktop.
    assert!(o.status(&r).signed_in);
    assert!(!o.status(&a).signed_in);
    // The bookkeeper's pending form is void and the account cannot sign in again.
    assert!(o.post_with(&b, &path, pending, &[("display_name", "After revocation")]).is_err());
    assert!(o.host.dispatch(&b, Request::SignIn { username: KEEPER.into(), password: PASSWORD.into() }).is_err());
    assert!(!o.status(&b).signed_in);
    let n: i64 = o.sql("SELECT count(*) FROM directory_customer WHERE display_name IN ('After revocation','Written as read-only')", &[]);
    assert_eq!(n, 0);
}

#[test]
fn queued_work_runs_with_the_permission_in_force_when_it_executes() {
    let o = office();
    let (first, keeper) = (o.first, o.keeper);
    let mut host = o.host;
    let a = host.open_session(ClientInfo::local("Host desktop")).unwrap();
    let b = host.open_session(ClientInfo::remote("Synthetic laptop", None)).unwrap();
    host.dispatch(&a, Request::SignIn { username: OWNER.into(), password: OWNER_PASSWORD.into() }).unwrap();
    host.dispatch(&b, Request::SignIn { username: KEEPER.into(), password: PASSWORD.into() }).unwrap();
    let path = format!("/c/{}/lists/customers/new/", first.to_db());
    let token = host.with_session(&b, |app| app.tokens.mint(&path)).unwrap();
    let shared = SharedHost::new(host);
    // A holds the books (an administrator on the Users page) while B's post is queued behind it.
    let mut lease = shared.lease("Synthetic owner: users", &mut |_| {}).unwrap();
    let queued = {
        let shared = shared.clone();
        let (b, path) = (b.clone(), path.clone());
        std::thread::spawn(move || {
            let mut heard = vec![];
            let command = Command::SubmitForm { path, token, fields: vec![("display_name".into(), "Queued before demotion".into())] };
            let result = shared.dispatch(&b, Request::Run { command }, &mut |p: QueueProgress| heard.push(p));
            (result, heard)
        })
    };
    std::thread::sleep(Duration::from_millis(400));
    lease.with_session(&a, |app| app.save_desktop_user(Some(keeper), KEEPER, Role::Readonly, true, None, &[first])).unwrap().unwrap();
    drop(lease);
    let (result, heard) = queued.join().unwrap();
    assert!(result.is_err(), "queued write ran with the old role: {result:?}");
    assert!(heard.iter().any(|p| p.position == 1 && p.holder.as_deref() == Some("Synthetic owner: users")), "{heard:?}");
    let mut lease = shared.lease("check", &mut |_| {}).unwrap();
    let n: i64 = lease.host_app().open.as_mut().unwrap().book.read(|q| q.query_row("SELECT count(*) FROM directory_customer WHERE display_name='Queued before demotion'", [], |r| r.get(0))).unwrap();
    assert_eq!(n, 0);
}

#[test]
fn idle_timeout_signs_out_only_the_idle_session() {
    let policy = Policy { idle_sign_out: Duration::from_secs(15 * 60), abandon_after: Duration::from_secs(60 * 60), ..Policy::default() };
    let mut o = office_with(policy);
    let t0 = Instant::now();
    let a = o.host.open_session_at(ClientInfo::local("Host desktop"), t0).unwrap();
    let b = o.host.open_session_at(ClientInfo::remote("Synthetic laptop", None), t0).unwrap();
    o.host.dispatch_at(&a, Request::SignIn { username: OWNER.into(), password: OWNER_PASSWORD.into() }, t0).unwrap();
    o.host.dispatch_at(&b, Request::SignIn { username: KEEPER.into(), password: PASSWORD.into() }, t0).unwrap();
    let home = format!("/c/{}/home/", o.first.to_db());
    o.host.dispatch_at(&b, Request::Render { target: home.clone() }, t0 + Duration::from_secs(10 * 60)).unwrap();
    let expired = o.host.expire_idle(t0 + Duration::from_secs(16 * 60));
    assert_eq!(expired, vec![a.clone()]);
    let sa = o.status(&a);
    assert!(!sa.signed_in && notices(&sa).contains("inactivity"), "{sa:?}");
    assert!(o.status(&b).signed_in);
    assert!(o.app_book_open());
    let Response::Page { status, .. } = o.host.dispatch_at(&b, Request::Render { target: home.clone() }, t0 + Duration::from_secs(17 * 60)).unwrap() else { panic!() };
    assert_eq!(status, 200);
    // Idle is also judged lazily: a request arriving after the idle time finds the session signed out.
    let Response::Page { html, status, .. } = o.host.dispatch_at(&b, Request::Render { target: home }, t0 + Duration::from_secs(40 * 60)).unwrap() else { panic!() };
    assert_eq!((html.as_str(), status), (SIGN_IN, 303));
    // A signed-out slot left alone is eventually removed; the client hears that once.
    o.host.expire_idle(t0 + Duration::from_secs(3 * 60 * 60));
    assert!(matches!(o.host.dispatch(&a, Request::Status), Err(HostError::Ended(EndReason::Abandoned))));
    assert!(matches!(o.host.dispatch(&a, Request::Status), Err(HostError::UnknownSession)));
}

#[test]
fn maintenance_is_exclusive_and_signs_everyone_else_out_only_when_it_changes_the_book() {
    let mut o = office();
    let a = o.signed_in(OWNER, true);
    let b = o.signed_in(KEEPER, false);
    let r = o.signed_in(READER, false);
    // Remote sessions cannot reach maintenance surfaces.
    let o_first = o.first.to_db();
    assert!(matches!(o.render(&b, fb_app::protection::SETTINGS), Err(HostError::Maintenance(_))));
    assert!(matches!(o.post(&b, &format!("/c/{o_first}/backups/create/"), &[]), Err(HostError::Maintenance(_))));
    // A backup made at the host leaves everyone signed in.
    let made = o.post(&a, &format!("/c/{o_first}/backups/create/"), &[]).unwrap();
    assert!(matches!(made, Outcome::Redirect { .. }), "{made:?}");
    assert!(o.status(&b).signed_in && o.status(&r).signed_in);
    // A failed protection change (wrong password) changes nothing for anyone.
    let failed = o.host.with_session(&a, |app| app.change_protection("not the book password", Some("Replacement book password"))).unwrap();
    assert!(failed.is_err());
    assert!(o.status(&b).signed_in);
    // A real key change signs every other session out with the reason; the administrator stays.
    let pending = o.form(&b, &format!("/c/{o_first}/lists/customers/new/"));
    o.host.with_session(&a, |app| app.change_protection(BOOK_PASSWORD, Some("Replacement book password"))).unwrap().unwrap();
    for id in [&b, &r] {
        let s = o.status(id);
        assert!(!s.signed_in);
        assert!(notices(&s).contains("protection was changed"), "{s:?}");
    }
    assert!(o.status(&a).signed_in);
    assert!(o.app_book_open());
    o.host.dispatch(&b, Request::SignIn { username: KEEPER.into(), password: PASSWORD.into() }).unwrap();
    assert!(o.post_with(&b, &format!("/c/{o_first}/lists/customers/new/"), pending, &[("display_name", "Old form")]).is_err());
    assert!(o.new_customer(&b, o.first, "After the key change").is_ok());
}

#[test]
fn reopening_the_book_on_the_host_signs_everyone_out_and_closing_ends_every_session() {
    let mut o = office();
    let a = o.signed_in(OWNER, true);
    let b = o.signed_in(KEEPER, false);
    // Restore-like reopen by the host administrator's own request.
    let path = o.path.clone();
    o.host
        .with_session(&a, |app| {
            app.lock();
            app.open_with_secret(&path, "Replacement book password").or_else(|_| app.open_with_secret(&path, BOOK_PASSWORD))
        })
        .unwrap()
        .unwrap();
    let s = o.status(&b);
    assert!(!s.signed_in && notices(&s).contains("reopened or replaced"), "{s:?}");
    assert!(o.app_book_open());
    o.host.dispatch(&b, Request::SignIn { username: KEEPER.into(), password: PASSWORD.into() }).unwrap();
    assert!(o.status(&b).signed_in);
    // The host closes the book: every session ends, and hears why once.
    o.host.close_book();
    assert!(!o.app_book_open());
    assert!(matches!(o.host.dispatch(&b, Request::Status), Err(HostError::Ended(EndReason::HostClosed))));
    assert!(matches!(o.host.dispatch(&b, Request::Status), Err(HostError::UnknownSession)));
    assert!(matches!(o.host.dispatch(&a, Request::Status), Err(HostError::Ended(EndReason::HostClosed))));
    assert!(matches!(o.host.open_session(ClientInfo::local("Host desktop")), Err(HostError::NotOpen)));
}

#[test]
fn maintenance_holds_the_queue_and_waiters_get_progress_then_busy() {
    let o = office();
    let first = o.first;
    let mut host = o.host;
    let policy_wait = Duration::from_millis(900);
    let a = host.open_session(ClientInfo::local("Host desktop")).unwrap();
    let b = host.open_session(ClientInfo::remote("Synthetic laptop", None)).unwrap();
    host.dispatch(&a, Request::SignIn { username: OWNER.into(), password: OWNER_PASSWORD.into() }).unwrap();
    host.dispatch(&b, Request::SignIn { username: KEEPER.into(), password: PASSWORD.into() }).unwrap();
    let shared = SharedHost::new(host);
    let lease = shared.lease("Synthetic owner: maintenance", &mut |_| {}).unwrap();
    let waiter = {
        let shared = shared.clone();
        std::thread::spawn(move || {
            let mut heard = vec![];
            let r = shared.lease_within("Bookkeeper: reading", policy_wait, &mut |p| heard.push(p)).map(|_| ());
            (r, heard)
        })
    };
    let (result, heard) = waiter.join().unwrap();
    let Err(HostError::Busy { position, holder, waited }) = result else { panic!("did not time out: {result:?}") };
    assert_eq!(position, 1);
    assert_eq!(holder.as_deref(), Some("Synthetic owner: maintenance"));
    assert!(waited >= policy_wait);
    assert!(heard.len() >= 3 && heard.iter().all(|p| p.holder.as_deref() == Some("Synthetic owner: maintenance")), "{heard:?}");
    drop(lease);
    // Once the maintenance lease is gone the same request runs.
    let page = shared.dispatch(&b, Request::Render { target: format!("/c/{}/home/", first.to_db()) }, &mut |_| {}).unwrap();
    assert!(matches!(page, Response::Page { status: 200, .. }));
}

#[test]
fn the_queue_is_fifo_and_bounded() {
    let policy = Policy { queue_depth: 2, queue_wait: Duration::from_secs(10), ..Policy::default() };
    let o = office_with(policy);
    let shared = SharedHost::new(o.host);
    let order = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
    let lease = shared.lease("holder", &mut |_| {}).unwrap();
    let mut threads = vec![];
    for n in 0..2 {
        let (shared, order) = (shared.clone(), order.clone());
        threads.push(std::thread::spawn(move || {
            let _l = shared.lease(&format!("waiter {n}"), &mut |_| {}).unwrap();
            order.lock().unwrap().push(n);
            std::thread::sleep(Duration::from_millis(20));
        }));
        std::thread::sleep(Duration::from_millis(150));
    }
    // A third waiter beyond the depth is refused at once.
    let started = Instant::now();
    let refused = shared.lease("too many", &mut |_| {});
    assert!(matches!(refused, Err(HostError::Busy { position: 3, .. })), "{:?}", refused.err());
    assert!(started.elapsed() < Duration::from_millis(100));
    drop(lease);
    for t in threads {
        t.join().unwrap();
    }
    assert_eq!(*order.lock().unwrap(), vec![0, 1]);
}

#[test]
fn generations_are_unique_per_session_and_never_validate_elsewhere() {
    let mut o = office();
    let a = o.signed_in(OWNER, true);
    let b = o.signed_in(OWNER, false);
    let (ga, gb) = (o.status(&a).generation, o.status(&b).generation);
    assert_ne!(ga, gb);
    let first = o.first;
    assert!(o.host.with_session(&a, |app| app.require_session(ga, Some(first), Permission::Read).is_ok()).unwrap());
    assert!(o.host.with_session(&b, |app| app.require_session(ga, Some(first), Permission::Read).is_err()).unwrap());
    // The same person in two sessions: signing out one leaves the other.
    o.host.dispatch(&a, Request::SignOut).unwrap();
    assert!(o.status(&b).signed_in);
    let sessions = o.host.sessions();
    assert_eq!(sessions.len(), 2);
    let listed = sessions.iter().find(|s| s.id == b).unwrap();
    assert_eq!(listed.user.as_ref().map(|(_, n)| n.as_str()), Some(OWNER));
    assert_eq!(listed.role, Some(Role::Administrator));
    assert_eq!(listed.client.device.as_deref(), Some("device-1"));
}

#[test]
fn sign_in_failures_are_limited_per_user_name_across_sessions() {
    let mut o = office();
    for _ in 0..5 {
        let s = o.session(false);
        let _ = o.host.dispatch(&s, Request::SignIn { username: KEEPER.into(), password: "wrong password".into() });
        o.host.end_session(&s, EndReason::Closed);
    }
    // A fresh session does not reset the limit for that name, even with the right password...
    let s = o.session(false);
    let refused = o.host.dispatch(&s, Request::SignIn { username: KEEPER.into(), password: PASSWORD.into() });
    assert!(matches!(&refused, Err(HostError::App(e)) if e.to_string().contains("Too many")), "{refused:?}");
    assert!(!o.status(&s).signed_in);
    // ...and other people can still sign in.
    o.host.dispatch(&s, Request::SignIn { username: READER.into(), password: PASSWORD.into() }).unwrap();
    assert!(o.status(&s).signed_in);
}

#[test]
fn sessions_need_an_open_enrolled_book() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("books/default.fbbook");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut app = App::new();
    app.backups.configure(dir.path()).unwrap();
    app.create_book(&path, &MasterKey::new([7; 32]), OWNER, "Synthetic company").unwrap();
    let mut host = SessionHost::new(app, Policy::default());
    assert!(matches!(host.open_session(ClientInfo::remote("Laptop", None)), Err(HostError::NotEnrolled)));
    host.close_book();
    assert!(matches!(host.open_session(ClientInfo::remote("Laptop", None)), Err(HostError::NotOpen)));
}

#[test]
fn sign_in_through_the_rendered_form_works_per_session() {
    let mut o = office();
    let a = o.session(true);
    let b = o.session(false);
    let (page_a, _) = o.render(&a, SIGN_IN).unwrap();
    let (page_b, _) = o.render(&b, SIGN_IN).unwrap();
    let token = |html: &str| html.split("name=\"fb_command_token\" value=\"").nth(1).map(|s| s.split('"').next().unwrap().to_owned());
    let (ta, tb) = (token(&page_a).expect("sign-in form token"), token(&page_b).expect("sign-in form token"));
    // One session's token does not work in the other.
    let fields = |n: &str, p: &str| vec![("action".into(), "signin".into()), ("username".into(), n.to_owned()), ("password".into(), p.to_owned())];
    let cross = o.host.dispatch(&b, Request::Run { command: Command::SubmitForm { path: SIGN_IN.into(), token: ta.clone(), fields: fields(KEEPER, PASSWORD) } });
    assert!(cross.is_err() || !o.status(&b).signed_in);
    o.host.dispatch(&a, Request::Run { command: Command::SubmitForm { path: SIGN_IN.into(), token: ta, fields: fields(OWNER, OWNER_PASSWORD) } }).unwrap();
    o.host.dispatch(&b, Request::Run { command: Command::SubmitForm { path: SIGN_IN.into(), token: tb, fields: fields(KEEPER, PASSWORD) } }).unwrap();
    assert_eq!(o.status(&a).user.map(|(_, n)| n), Some(OWNER.to_owned()));
    assert_eq!(o.status(&b).user.map(|(_, n)| n), Some(KEEPER.to_owned()));
}

#[test]
fn a_desktop_in_use_starts_and_stops_hosting_without_losing_its_session() {
    let o = office();
    let (first, path_book) = (o.first, o.path.clone());
    let mut app = o.host.into_app(&SessionId::parse(&"0".repeat(32)).unwrap());
    assert!(app.authenticated_session().is_err());
    app.sign_in(OWNER, OWNER_PASSWORD).unwrap();
    let generation = app.session_generation();
    let path = format!("/c/{}/lists/customers/new/", first.to_db());
    let token = app.tokens.mint(&path);
    // Host these books: the desktop's person is the local session, still signed in, form still live.
    let (mut host, local) = SessionHost::host_with_local(app, Policy::default(), "Host desktop");
    let command = |token: String, name: &str| Command::SubmitForm { path: path.clone(), token, fields: vec![("display_name".into(), name.into())] };
    let Response::Status(s) = host.dispatch(&local, Request::Status).unwrap() else { panic!() };
    assert!(s.signed_in && s.client.local && s.generation == generation);
    let Response::Outcome { outcome, .. } = host.dispatch(&local, Request::Run { command: command(token, "Saved while hosting") }).unwrap() else { panic!() };
    assert!(matches!(outcome, Outcome::Redirect { .. }), "{outcome:?}");
    let remote = host.open_session(ClientInfo::remote("Synthetic laptop", None)).unwrap();
    host.dispatch(&remote, Request::SignIn { username: KEEPER.into(), password: PASSWORD.into() }).unwrap();
    // Only administrators log others off.
    assert!(host.log_off(&remote, &local).is_err());
    let other = host.open_session(ClientInfo::remote("Second laptop", None)).unwrap();
    host.log_off(&local, &other).unwrap();
    assert!(matches!(host.dispatch(&other, Request::Status), Err(HostError::Ended(EndReason::Closed))));
    // Stop hosting: the remote session ends; the desktop keeps its person and the book.
    let token = host.with_session(&local, |app| app.tokens.mint(&path)).unwrap();
    let mut app = host.into_app(&local);
    assert_eq!(app.authenticated_session().unwrap().role, Role::Administrator);
    assert!(matches!(app.run_desktop(command(token, "Saved after hosting")).unwrap(), Outcome::Redirect { .. }));
    let n: i64 = app.open_mut().unwrap().book.read(|q| q.query_row("SELECT count(*) FROM directory_customer WHERE display_name LIKE 'Saved %hosting'", [], |r| r.get(0))).unwrap();
    assert_eq!(n, 2);
    let _ = path_book;
}

// --- Review fixes (review-sol.md, 2026-11-16): one regression per finding --------------------------

/// B1: a remembered answer is bound to the person and identity generation that made it.
#[test]
fn replayed_answers_never_cross_a_form_sign_out_or_a_different_user() {
    let mut o = office();
    let a = o.signed_in(OWNER, true);
    let path = format!("/c/{}/lists/customers/new/", o.first.to_db());
    let fields = [("display_name", "Replayed once")];
    // Within one unchanged session an exact resend is still answered from the first run.
    let token = o.form(&a, &path);
    let (first, replayed) = o.post_with(&a, &path, token.clone(), &fields).unwrap();
    assert!(!replayed && matches!(first, Outcome::Redirect { .. }), "{first:?}");
    let (again, replayed) = o.post_with(&a, &path, token.clone(), &fields).unwrap();
    assert!(replayed && again == first);
    // Signing out through the rendered form (not `Request::SignOut`) voids it.
    o.post(&a, fb_app::identity::SESSION, &[("action", "signout")]).unwrap();
    assert!(!o.status(&a).signed_in);
    let replayed = |r: &Result<(Outcome, bool), HostError>| matches!(r, Ok((_, true)));
    let signed_out = o.post_with(&a, &path, token.clone(), &fields);
    assert!(!replayed(&signed_out), "signed-out client got the remembered answer: {signed_out:?}");
    // Another person signing in on the same slot does not inherit it either.
    o.host.dispatch(&a, Request::SignIn { username: KEEPER.into(), password: PASSWORD.into() }).unwrap();
    let other = o.post_with(&a, &path, token.clone(), &fields);
    assert!(!replayed(&other), "bookkeeper got the administrator's answer: {other:?}");
    // Nor the same administrator signing in again: the answer was for an earlier sign-in.
    o.host.dispatch(&a, Request::SignOut).unwrap();
    o.host.dispatch(&a, Request::SignIn { username: OWNER.into(), password: OWNER_PASSWORD.into() }).unwrap();
    let again = o.post_with(&a, &path, token, &fields);
    assert!(!replayed(&again), "answer replayed across sign-ins: {again:?}");
    assert_eq!(o.sql::<i64>("SELECT count(*) FROM directory_customer WHERE display_name='Replayed once'", &[]), 1);
}

/// B2: existing remote sessions do not survive a switch to books nobody has enrolled in, and the
/// enrollment action itself is refused to remote sessions.
#[test]
fn a_remote_session_cannot_set_up_the_owner_of_replacement_books() {
    let mut o = office();
    let local = o.signed_in(OWNER, true);
    let remote = o.signed_in(KEEPER, false);
    // On enrolled books the action is refused before it runs.
    let refused = o.post(&remote, SIGN_IN, &[("action", "enroll"), ("confirm_owner", "on"), ("password", PASSWORD), ("password_repeat", PASSWORD)]);
    assert!(matches!(refused, Err(HostError::Maintenance(_))), "{refused:?}");
    // The host switches to an unenrolled book (for example, a retained book from before named users).
    let new_path = o._dir.path().join("books/unenrolled.fbbook");
    let app = o.host.host_app();
    app.lock();
    app.create_book(&new_path, &MasterKey::new([71; 32]), "Replacement owner", "Replacement synthetic books").unwrap();
    assert!(matches!(o.host.open_session(ClientInfo::remote("New remote", None)), Err(HostError::NotEnrolled)));
    let rendered = o.render(&remote, SIGN_IN);
    assert!(matches!(rendered, Err(HostError::Ended(EndReason::Closed))), "{:?}", rendered.map(|(_, s)| s));
    let enroll = o.post_with(&remote, SIGN_IN, "x".repeat(64), &[("action", "enroll"), ("confirm_owner", "on"), ("password", PASSWORD), ("password_repeat", PASSWORD)]);
    assert!(matches!(enroll, Err(HostError::UnknownSession)), "{enroll:?}");
    assert_eq!(o.sql::<i64>("SELECT count(*) FROM desktop_user WHERE password_hash IS NOT NULL", &[]), 0);
    // The host computer's own session stays, signed out, for the host-only setup.
    assert!(!o.status(&local).signed_in);
}

/// S1: a waiter that panics in its progress callback leaves the line.
#[test]
fn a_waiter_that_panics_in_its_progress_callback_leaves_the_queue() {
    let o = office();
    let shared = SharedHost::new(o.host);
    let holder = shared.lease("holder", &mut |_| {}).unwrap();
    let copy = shared.clone();
    let panicked = std::thread::spawn(move || {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = copy.lease("abandoned waiter", &mut |_| panic!("synthetic progress panic"));
        }))
        .is_err()
    })
    .join()
    .unwrap();
    assert!(panicked);
    // The next waiter gets the books once the holder lets go.
    let copy = shared.clone();
    let next = std::thread::spawn(move || copy.lease_within("next", Duration::from_secs(10), &mut |_| {}).map(|_| ()).is_ok());
    std::thread::sleep(Duration::from_millis(100));
    drop(holder);
    assert!(next.join().unwrap(), "the queue stayed blocked behind the abandoned ticket");
    assert!(shared.lease_within("after", Duration::from_millis(500), &mut |_| {}).is_ok());
}

/// S2: a form kept through stop/start hosting is still compared with writes made afterwards.
#[test]
fn forms_kept_through_stopping_and_starting_hosting_still_detect_newer_edits() {
    let mut o = office();
    let mut local = o.signed_in(OWNER, true);
    let location = o.new_customer(&local, o.first, "Original").unwrap();
    let edit = format!("{location}edit/");
    o.post(&local, &edit, &[("display_name", "First epoch")]).unwrap();
    let stale = o.form(&local, &edit);
    for round in 0..2 {
        let app = std::mem::replace(&mut o.host, SessionHost::new(App::new(), Policy::default())).into_app(&local);
        let (host, id) = SessionHost::host_with_local(app, Policy::default(), &format!("Host again {round}"));
        o.host = host;
        local = id;
    }
    let remote = o.signed_in(KEEPER, false);
    assert!(matches!(o.post(&remote, &edit, &[("display_name", "Remote's fresh edit")]).unwrap(), Outcome::Redirect { .. }));
    // The record form did not survive the hosting change at all (re-review R2): it must be reopened.
    let result = o.post_with(&local, &edit, stale, &[("display_name", "Local's stale overwrite")]);
    assert!(matches!(&result, Err(HostError::App(e)) if e.to_string().contains("Hosting changed")), "{result:?}");
    assert_eq!(o.customer_name(&location), "Remote's fresh edit");
    // A form opened after the change is compared with later writes as usual.
    let fresh = o.form(&local, &edit);
    assert!(matches!(o.post(&remote, &edit, &[("display_name", "Remote again")]).unwrap(), Outcome::Redirect { .. }));
    let (result, _) = o.post_with(&local, &edit, fresh, &[("display_name", "Local's stale overwrite")]).unwrap();
    assert!(matches!(&result, Outcome::Retry { token: None, message } if message.contains(KEEPER)), "{result:?}");
    assert_eq!(o.customer_name(&location), "Remote again");
}

/// S3: generations are never handed out twice in one process, across hosting lifetimes; the local
/// person keeps theirs.
#[test]
fn generations_are_not_reused_when_hosting_restarts() {
    let mut o = office();
    let a = o.signed_in(OWNER, true);
    let app = std::mem::replace(&mut o.host, SessionHost::new(App::new(), Policy::default())).into_app(&a);
    let (host, local) = SessionHost::host_with_local(app, Policy::default(), "Epoch one");
    o.host = host;
    let remote = o.signed_in(KEEPER, false);
    let before = o.status(&remote).generation;
    let local_generation = o.status(&local).generation;
    let app = std::mem::replace(&mut o.host, SessionHost::new(App::new(), Policy::default())).into_app(&local);
    let (host, local) = SessionHost::host_with_local(app, Policy::default(), "Epoch two");
    o.host = host;
    assert_eq!(o.status(&local).generation, local_generation, "the local person's capabilities are kept");
    let next = o.signed_in(KEEPER, false);
    assert_ne!(o.status(&next).generation, before);
    assert!(o.host.with_session(&next, |app| app.require_session(before, None, Permission::Read).is_err()).unwrap());
    assert!(o.host.with_session(&next, |app| app.require_session(app.session_generation(), None, Permission::Read).is_ok()).unwrap());
}

/// S4: forms that create records have no record to conflict on.
#[test]
fn independent_new_record_forms_do_not_conflict() {
    let mut o = office();
    let a = o.signed_in(OWNER, true);
    let b = o.signed_in(KEEPER, false);
    let path = format!("/c/{}/lists/customers/new/", o.first.to_db());
    let first = o.form(&a, &path);
    let second = o.form(&b, &path);
    assert!(matches!(o.post_with(&a, &path, first, &[("display_name", "Unrelated A")]).unwrap().0, Outcome::Redirect { .. }));
    let result = o.post_with(&b, &path, second, &[("display_name", "Unrelated B")]).unwrap().0;
    assert!(matches!(result, Outcome::Redirect { .. }), "independent new record refused: {result:?}");
    assert_eq!(o.sql::<i64>("SELECT count(*) FROM directory_customer WHERE display_name IN ('Unrelated A','Unrelated B')", &[]), 2);
}

fn totp(secret: &str, offset: i64) -> String {
    use ring::hmac;
    let bytes = data_encoding::BASE32_NOPAD.decode(secret.as_bytes()).unwrap();
    let step = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() / 30) as i64 + offset;
    let key = hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, &bytes);
    let tag = hmac::sign(&key, &(step as u64).to_be_bytes());
    let mac = tag.as_ref();
    let i = (mac[19] & 15) as usize;
    format!("{:06}", (u32::from_be_bytes(mac[i..i + 4].try_into().unwrap()) & 0x7fff_ffff) % 1_000_000)
}

/// S5: second-factor guesses are limited per verified user across sessions; `mfa_pending` reports
/// the password step (remote access request (b)).
#[test]
fn second_factor_guesses_are_limited_per_user_across_sessions() {
    let mut o = office();
    let local = o.signed_in(OWNER, true);
    o.host
        .with_session(&local, |app| {
            app.mfa_authorize_management(OWNER_PASSWORD, "").unwrap();
            let secret = app.mfa_begin_totp().unwrap();
            app.mfa_confirm_totp(&totp(&secret, -1)).unwrap();
        })
        .unwrap();
    let (page, _) = o.render(&local, fb_app::mfa::SETTINGS).unwrap();
    let codes: Vec<String> = page.split("aria-label=\"Recovery codes\"").nth(1).unwrap().split_once('>').unwrap().1.split("</textarea>").next().unwrap()
        .lines().map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect();
    assert_eq!(codes.len(), 10);
    // Five wrong codes in one session, which is then ended.
    let first = o.session(false);
    o.host.dispatch(&first, Request::SignIn { username: OWNER.into(), password: OWNER_PASSWORD.into() }).unwrap();
    let status = o.status(&first);
    assert!(status.mfa_pending && !status.signed_in, "password accepted, second factor owed");
    for _ in 0..5 {
        let _ = o.post(&first, SIGN_IN, &[("action", "mfa"), ("code", "000000")]);
    }
    assert!(!o.status(&first).mfa_pending && !o.status(&first).signed_in);
    o.host.end_session(&first, EndReason::Closed);
    // A fresh session passes the password step again, but the sixth code is refused unseen, even a
    // correct one, and nothing is used up.
    let fresh = o.session(false);
    o.host.dispatch(&fresh, Request::SignIn { username: OWNER.into(), password: OWNER_PASSWORD.into() }).unwrap();
    assert!(o.status(&fresh).mfa_pending);
    let sixth = o.post(&fresh, SIGN_IN, &[("action", "mfa"), ("code", &codes[0])]);
    assert!(matches!(&sixth, Err(HostError::App(e)) if e.to_string().contains("Too many unsuccessful codes")), "{sixth:?}");
    assert!(!o.status(&fresh).signed_in);
    // After the wait it expires normally and the same (unused) code signs in.
    let token = o.form(&fresh, SIGN_IN);
    let command = Command::SubmitForm { path: SIGN_IN.into(), token, fields: vec![("action".into(), "mfa".into()), ("code".into(), codes[0].clone())] };
    o.host.dispatch_at(&fresh, Request::Run { command }, Instant::now() + Duration::from_secs(61)).unwrap();
    let status = o.status(&fresh);
    assert!(status.signed_in && !status.mfa_pending);
}

#[cfg(feature = "plugins")]
mod provider_sign_in {
    use super::*;

    /// fb-net keys sign-in jobs by provider and company only, like this fake.
    #[derive(Default)]
    struct CompanyOnlyNet {
        attempt: u32,
    }
    impl fb_app::net::NetHost for CompanyOnlyNet {
        fn call(&mut self, request: &serde_json::Value) -> Result<serde_json::Value, String> {
            use serde_json::json;
            Ok(match request["op"].as_str().unwrap() {
                "chatgpt_begin" => {
                    self.attempt += 1;
                    json!({"ok":true,"authorization_url":format!("https://synthetic.invalid/attempt/{}",self.attempt)})
                }
                "chatgpt_poll" => json!({"ok":true,"record":"synthetic OAuth record","model_choices":[],"account_email":format!("person{}@synthetic.invalid",self.attempt)}),
                "chatgpt_cancel" => json!({"ok":true}),
                _ => panic!("unexpected synthetic net operation"),
            })
        }
    }

    /// B3: one session's provider sign-in cannot be replaced, polled or claimed by another.
    #[test]
    fn a_session_cannot_take_over_or_claim_another_sessions_provider_sign_in() {
        let mut o = office();
        o.host.host_app().net = Some(Box::new(CompanyOnlyNet::default()));
        let a = o.signed_in(OWNER, true);
        let first = o.first;
        let other = o.host.with_session(&a, |app| app.save_desktop_user(None, "Second administrator", Role::Administrator, true, Some(PASSWORD), &[first])).unwrap().unwrap();
        o.host.dispatch(&a, Request::SignIn { username: OWNER.into(), password: OWNER_PASSWORD.into() }).unwrap();
        let b = o.session(false);
        o.host.dispatch(&b, Request::SignIn { username: "Second administrator".into(), password: PASSWORD.into() }).unwrap();
        let start = "/auth/openai/start";
        let company = first.to_db();
        assert!(matches!(o.post(&a, start, &[("company", &company)]).unwrap(), Outcome::External { .. }));
        let second = o.post(&b, start, &[("company", &company)]);
        assert!(!matches!(second, Ok(Outcome::External { .. })), "B replaced A's sign-in: {second:?}");
        // B's pages never poll A's job; A's page completes A's own attempt, as A.
        o.render(&b, &format!("/c/{company}/ai/")).unwrap();
        assert_eq!(o.sql::<i64>("SELECT count(*) FROM assistant_connection WHERE company_id=?1 AND account_email<>''", &[&company]), 0);
        o.render(&a, &format!("/c/{company}/ai/")).unwrap();
        assert_eq!(o.sql::<String>("SELECT account_email FROM assistant_connection WHERE company_id=?1", &[&company]), "person1@synthetic.invalid");
        assert_eq!(o.sql::<String>("SELECT actor_id FROM ledger_auditevent WHERE action='ai.connected' LIMIT 1", &[]), o.owner.to_db());
        assert_ne!(o.owner, other);
        // A's finished attempt is no one's job any more: B may start one; signing B out releases it.
        assert!(matches!(o.post(&b, start, &[("company", &company)]).unwrap(), Outcome::External { .. }));
        assert!(!matches!(o.post(&a, start, &[("company", &company)]), Ok(Outcome::External { .. })));
        o.host.dispatch(&b, Request::SignOut).unwrap();
        assert!(matches!(o.post(&a, start, &[("company", &company)]).unwrap(), Outcome::External { .. }));
    }
}

// --- Re-review fixes (review-sol-2.md, 2026-11-16) --------------------------------------------------

fn two_administrators(o: &mut Office) -> SessionId {
    // A second administrator keeps the book's last-administrator safeguard satisfied when the first
    // is demoted underneath.
    let a = o.signed_in(OWNER, true);
    let (first, second) = (o.first, o.second);
    o.host.with_session(&a, |app| app.save_desktop_user(None, "Second administrator", Role::Administrator, true, Some(PASSWORD), &[first, second])).unwrap().unwrap();
    o.host.dispatch(&a, Request::SignIn { username: OWNER.into(), password: OWNER_PASSWORD.into() }).unwrap();
    a
}

fn set_role(o: &mut Office, user: UserId, role: &str) {
    o.host.host_app().open.as_mut().unwrap().book.write(|tx| tx.execute("UPDATE desktop_user SET role=?1 WHERE user_id=?2", [role.to_owned(), user.to_db()])).unwrap();
}

/// R1 (reviewer's scenario 1): an administrator page is never answered from the replay cache, so a
/// demotion underneath cannot be bypassed by resending the post.
#[test]
fn replay_never_returns_an_administrator_page_after_a_live_demotion() {
    let mut o = office();
    let a = two_administrators(&mut o);
    let path = fb_app::identity::USERS;
    let fields = [("action", "edit")];
    let token = o.form(&a, path);
    let (first, _) = o.post_with(&a, path, token.clone(), &fields).unwrap();
    assert!(matches!(&first, Outcome::Page { html, .. } if html.contains("Second synthetic company")));
    // The stored role changes without the roster hook (as in the live-permission test).
    let owner = o.owner;
    set_role(&mut o, owner, "bookkeeper");
    assert!(o.render(&a, path).is_err(), "a fresh roster render refuses the demoted person");
    let again = o.post_with(&a, path, token, &fields);
    let leaked = matches!(&again, Ok((_, true))) || matches!(&again, Ok((Outcome::Page { html, .. }, _)) if html.contains("Second synthetic company"));
    assert!(!leaked, "cached administrator page under a weaker role: {again:?}");
}

/// R1: the route's own permission is rechecked before an ordinary post is replayed (a write post
/// needs write permission again, not merely read access to the company).
#[test]
fn replay_rechecks_the_routes_own_permission() {
    let mut o = office();
    let b = o.signed_in(KEEPER, false);
    let path = format!("/c/{}/lists/customers/new/", o.first.to_db());
    let fields = [("display_name", "Written before demotion")];
    let token = o.form(&b, &path);
    let (first, _) = o.post_with(&b, &path, token.clone(), &fields).unwrap();
    assert!(matches!(first, Outcome::Redirect { .. }));
    let keeper = o.keeper;
    set_role(&mut o, keeper, "readonly");
    let again = o.post_with(&b, &path, token, &fields);
    assert!(!matches!(again, Ok((_, true))), "write post replayed to a read-only user: {again:?}");
    assert_eq!(o.sql::<i64>("SELECT count(*) FROM directory_customer WHERE display_name='Written before demotion'", &[]), 1);
}

/// R1 (reviewer's scenario 2): a recovery-key page is never cached, so leaving recovery hides the
/// key for good.
#[test]
fn replay_never_restores_a_recovery_key_after_leaving_recovery() {
    let mut o = office();
    let a = o.signed_in(OWNER, true);
    let path = fb_app::protection::RECOVERY;
    o.post(&a, path, &[("action", "show"), ("current_password", BOOK_PASSWORD)]).unwrap();
    let secret = o.host.with_session(&a, |app| app.recovery_text().unwrap().as_str().to_owned()).unwrap();
    let fields = [("action", "synthetic-invalid-action")];
    let token = o.form(&a, path);
    let (first, _) = o.post_with(&a, path, token.clone(), &fields).unwrap();
    assert!(matches!(&first, Outcome::Invalid { html } if html.contains(&secret)), "the authorized error page shows the key");
    o.render(&a, &format!("/c/{}/home/", o.first.to_db())).unwrap();
    let (page, _) = o.render(&a, path).unwrap();
    assert!(!page.contains(&secret), "a fresh page hides the key");
    let again = o.post_with(&a, path, token, &fields);
    let leaked = match &again {
        Ok((_, true)) => true,
        Ok((Outcome::Invalid { html } | Outcome::Page { html, .. }, _)) => html.contains(&secret),
        Ok((Outcome::Retry { message, .. }, _)) => message.contains(&secret),
        _ => false,
    };
    assert!(!leaked, "resend restored the hidden recovery key: {again:?}");
}

/// R1: MFA recovery codes, once acknowledged and cleared, are not handed out again by a resend.
#[test]
fn replay_never_restores_cleared_mfa_recovery_codes() {
    let mut o = office();
    let a = o.signed_in(OWNER, true);
    o.host
        .with_session(&a, |app| {
            app.mfa_authorize_management(OWNER_PASSWORD, "").unwrap();
            let secret = app.mfa_begin_totp().unwrap();
            app.mfa_confirm_totp(&totp(&secret, 0)).unwrap();
        })
        .unwrap();
    let (page, _) = o.render(&a, fb_app::mfa::SETTINGS).unwrap();
    let code = page.split("aria-label=\"Recovery codes\"").nth(1).unwrap().split_once('>').unwrap().1.split("</textarea>").next().unwrap()
        .lines().map(str::trim).find(|s| !s.is_empty()).unwrap().to_owned();
    // A refused post on the settings page re-renders it with the codes still on display.
    let fields = [("action", "disable")];
    let token = o.form(&a, fb_app::mfa::SETTINGS);
    let first = o.post_with(&a, fb_app::mfa::SETTINGS, token.clone(), &fields);
    let shows = |r: &Result<(Outcome, bool), HostError>| match r {
        Ok((Outcome::Invalid { html } | Outcome::Page { html, .. }, _)) => html.contains(&code),
        Ok((Outcome::Retry { message, .. }, _)) => message.contains(&code),
        Err(e) => e.to_string().contains(&code),
        _ => false,
    };
    assert!(shows(&first), "the refused post showed the codes: {first:?}");
    // "I saved them": the display is cleared.
    o.post(&a, fb_app::mfa::SETTINGS, &[("action", "saved")]).unwrap();
    let (page, _) = o.render(&a, fb_app::mfa::SETTINGS).unwrap();
    assert!(!page.contains(&code));
    let again = o.post_with(&a, fb_app::mfa::SETTINGS, token, &fields);
    assert!(!matches!(again, Ok((_, true))) && !shows(&again), "resend restored cleared recovery codes: {again:?}");
}

/// R2 (reviewer's scenario): a remote edit committed before hosting stops still makes the local
/// person's older form unusable after stopping and starting again: the form must be reopened.
#[test]
fn a_conflict_recorded_before_stopping_hosting_still_refuses_the_stale_form() {
    let mut o = office();
    let local = o.signed_in(OWNER, true);
    let remote = o.signed_in(KEEPER, false);
    let location = o.new_customer(&local, o.first, "Original").unwrap();
    let edit = format!("{location}edit/");
    let stale = o.form(&local, &edit);
    o.post(&remote, &edit, &[("display_name", "Remote's committed edit")]).unwrap();
    let app = std::mem::replace(&mut o.host, SessionHost::new(App::new(), Policy::default())).into_app(&local);
    let (host, local) = SessionHost::host_with_local(app, Policy::default(), "Restart after remote save");
    o.host = host;
    let result = o.post_with(&local, &edit, stale, &[("display_name", "Local stale overwrite")]);
    assert!(matches!(&result, Err(HostError::App(e)) if e.to_string().contains("Hosting changed")), "{result:?}");
    assert_eq!(o.customer_name(&location), "Remote's committed edit");
    // Forms that do not edit an existing record are kept.
    let new_path = format!("/c/{}/lists/customers/new/", o.first.to_db());
    let kept = o.form(&local, &new_path);
    let app = std::mem::replace(&mut o.host, SessionHost::new(App::new(), Policy::default())).into_app(&local);
    let (host, local) = SessionHost::host_with_local(app, Policy::default(), "Again");
    o.host = host;
    assert!(matches!(o.post_with(&local, &new_path, kept, &[("display_name", "Kept new form")]).unwrap().0, Outcome::Redirect { .. }));
}

/// R2: the same, returning straight to the standalone desktop (no new host).
#[test]
fn a_stale_record_form_is_refused_right_after_stopping_hosting() {
    let mut o = office();
    let local = o.signed_in(OWNER, true);
    let remote = o.signed_in(KEEPER, false);
    let location = o.new_customer(&local, o.first, "Original").unwrap();
    let edit = format!("{location}edit/");
    let stale = o.form(&local, &edit);
    o.post(&remote, &edit, &[("display_name", "Remote's committed edit")]).unwrap();
    let mut app = std::mem::replace(&mut o.host, SessionHost::new(App::new(), Policy::default())).into_app(&local);
    let command = Command::SubmitForm { path: edit.clone(), token: stale, fields: vec![("display_name".into(), "Local stale overwrite".into())] };
    let result = app.run_desktop(command);
    assert!(matches!(&result, Err(e) if e.to_string().contains("Hosting changed")), "{result:?}");
    let id = location.trim_end_matches('/').rsplit('/').next().unwrap().to_owned();
    let name: String = app.open_mut().unwrap().book.read(|q| q.query_row("SELECT display_name FROM directory_customer WHERE id=?1", [id], |r| r.get(0))).unwrap();
    assert_eq!(name, "Remote's committed edit");
    // Reopened, the record saves normally on the desktop.
    let token = app.tokens.mint(&edit);
    let command = Command::SubmitForm { path: edit, token, fields: vec![("display_name".into(), "Local after reopening".into())] };
    assert!(matches!(app.run_desktop(command).unwrap(), Outcome::Redirect { .. }));
}
