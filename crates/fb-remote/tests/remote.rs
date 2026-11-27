//! End to end through the real `fb-remote gateway` process on loopback: pairing, mutual TLS,
//! revocation, refusals, sign-in, retry deduplication and network loss, against a synthetic book.
mod common;
use common::*;
use fb_remote::client::{self, Client, ClientError, Link};
use fb_remote::tls::{self, DeviceGate, Identity, Pin};
use fb_remote_proto::frame::{self, MAX_REQUEST, MAX_RESPONSE};
use fb_remote_proto::wire::{ClientRequest, HostResponse, RefusalCode};
use fb_remote_proto::Config;
use rustls::{ClientConnection, ServerConnection, StreamOwned};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn run(c: &Client, id: &str, command: &str) -> HostResponse {
    c.call(&ClientRequest::Run { request_id: id.into(), command: command.into() }).unwrap()
}

fn render(c: &Client, target: &str) -> HostResponse {
    c.call(&ClientRequest::Render { target: target.into() }).unwrap()
}

fn sign_in(c: &Client, user: &str, password: &str) -> HostResponse {
    run(c, &request_id(), &form("/desktop/sign-in/", &[("action", "signin"), ("username", user), ("password", password)]))
}

fn post(c: &Client, id: &str, memo: &str) -> HostResponse {
    run(c, id, &form(&journal(), &[("memo", memo)]))
}

fn refusal(r: &HostResponse) -> RefusalCode {
    match r {
        HostResponse::Refused { code, .. } => *code,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn page(r: &HostResponse) -> &str {
    match r {
        HostResponse::Page { html, .. } => html,
        other => panic!("expected a page, got {other:?}"),
    }
}

#[test]
fn pair_sign_in_and_post_through_the_gateway() {
    let host = start();
    let paired = pair(&host, "Front desk");
    assert_eq!(paired.host_name, "Office PC");
    assert!(host.has("pair.armed") && host.has("pair.succeeded"));
    let devices = host.link.devices().unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!((devices[0].name.as_str(), devices[0].paired_by.as_str()), ("Front desk", "Jordan Admin"));
    assert_eq!(devices[0].pin, fb_remote_proto::hex(&paired.identity.pin()), "the host keeps only the public pin");

    let c = connect_to(&host.link.addr, &paired);
    // Before sign-in every page goes to the sign-in screen.
    assert!(matches!(render(&c, &home()), HostResponse::Page { status: 303, ref html, .. } if html == "/desktop/sign-in/"));
    assert_eq!(page(&render(&c, "/desktop/sign-in/")), "<form>sign in</form>");
    assert!(matches!(sign_in(&c, "dana", "wrong password"), HostResponse::Outcome { ref outcome } if outcome.contains("invalid")));
    assert!(host.has("sign_in.failed"));
    assert!(matches!(sign_in(&c, "dana", USERS[0].1), HostResponse::Outcome { ref outcome } if outcome.contains("redirect")));
    assert!(host.has("sign_in.succeeded"));
    assert!(matches!(post(&c, &request_id(), "rent"), HostResponse::Outcome { .. }));
    assert_eq!(page(&render(&c, &home())), "home of dana: 1 of 1 entries");
    assert_eq!(host.ledger(), vec![("dana".to_owned(), "rent".to_owned())]);
    assert!(matches!(c.call(&ClientRequest::SignOut).unwrap(), HostResponse::SignedOut));
    assert!(matches!(render(&c, &home()), HostResponse::Page { status: 303, .. }));
    let e = host.events.lock().unwrap().iter().find(|e| e.kind == "sign_in.succeeded").cloned().unwrap();
    assert_eq!((e.user.as_deref(), e.device_name.as_deref()), (Some("dana"), Some("Front desk")));
    assert_eq!(e.peer, "127.0.0.1");
}

#[test]
fn unpaired_computers_are_refused_in_the_handshake() {
    let host = start();
    let paired = pair(&host, "Front desk");
    // No pairing code is live: a certificate the host does not know never gets past TLS.
    let stranger = Identity::generate("stranger").unwrap();
    let (notify, _) = links();
    let c = Client::new(&host.link.addr, &stranger, paired.host_pin, notify).unwrap();
    assert_eq!(c.hello().unwrap_err(), ClientError::NotPaired);
    assert!(host.has("handshake.refused"));
    assert_eq!(host.sessions(), 0, "nothing reached the book");
    // Plain TCP garbage gets nothing either.
    let mut raw = TcpStream::connect(&host.link.addr).unwrap();
    raw.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
    raw.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut buf = [0u8; 64];
    let n = raw.read(&mut buf).unwrap_or(0);
    assert!(!String::from_utf8_lossy(&buf[..n]).contains("HTTP"), "not an HTTP server");
    assert_eq!(host.sessions(), 0);
}

#[test]
fn wrong_expired_and_used_codes_are_refused() {
    let host = start();
    // Wrong code, then the code is destroyed after five wrong proofs.
    let code = host.link.arm_pairing("Jordan Admin").unwrap();
    let wrong = if code.starts_with('A') { "B" } else { "A" }.to_owned() + &code[1..];
    for _ in 0..5 {
        match client::pair(&host.link.addr, &wrong, "Laptop") {
            Err(ClientError::Pairing(m)) => assert_eq!(m, fb_remote::pairing::REFUSED),
            other => panic!("{:?}", other.err()),
        }
    }
    assert!(host.has("pair.refused"));
    let after = client::pair(&host.link.addr, &code, "Laptop").err().unwrap();
    assert!(matches!(after, ClientError::NotPaired | ClientError::Pairing(_)), "a destroyed code no longer pairs: {after:?}");
    // Expired.
    let code = host.link.arm_pairing_for("Jordan Admin", 1).unwrap();
    std::thread::sleep(Duration::from_millis(1300));
    assert!(client::pair(&host.link.addr, &code, "Laptop").is_err());
    // Used: one code admits one computer.
    let code = host.link.arm_pairing("Jordan Admin").unwrap();
    assert!(client::pair(&host.link.addr, &code, "Laptop").is_ok());
    assert!(client::pair(&host.link.addr, &code, "Second laptop").is_err());
    // Mistyped forms are refused before anything is sent.
    assert!(matches!(client::pair(&host.link.addr, "ABCD", "Laptop"), Err(ClientError::Pairing(_))));
    assert_eq!(host.link.devices().unwrap().len(), 1);
    // Cancelled.
    let code = host.link.arm_pairing("Jordan Admin").unwrap();
    host.link.cancel_pairing().unwrap();
    assert!(client::pair(&host.link.addr, &code, "Laptop 3").is_err());
}

#[test]
fn revoked_computers_are_cut_off_and_refused() {
    let host = start();
    let paired = pair(&host, "Front desk");
    let other = pair(&host, "Back office");
    let c = connect_to(&host.link.addr, &paired);
    let o = connect_to(&host.link.addr, &other);
    sign_in(&c, "dana", USERS[0].1);
    sign_in(&o, "lee", USERS[1].1);
    assert_eq!(host.sessions(), 2);
    assert_eq!(host.link.revoke(&paired.device_id, "Jordan Admin").unwrap(), fb_remote_proto::link::Revocation { revoked: true, already: false, unsaved: None });
    assert_eq!(host.link.revoke(&paired.device_id, "Jordan Admin").unwrap(), fb_remote_proto::link::Revocation { revoked: true, already: true, unsaved: None }, "a repeat is answered as revoked");
    assert!(!host.link.revoke("no-such-device", "Jordan Admin").unwrap().revoked);
    assert!(host.has("device.revoked") && host.has("device.revocation_confirmed"));
    assert_eq!(host.kinds().iter().filter(|k| *k == "device.revoked").count(), 1, "recorded once");
    // Its live connection is closed and its next connection refused in the handshake.
    assert_eq!(c.call(&ClientRequest::Render { target: home() }).unwrap_err(), ClientError::NotPaired);
    assert_eq!(host.sessions(), 1, "the revoked computer's session ended; the other's did not");
    assert_eq!(page(&render(&o, &home())), "home of lee: 0 of 0 entries");
    host.link.arm_pairing("Jordan Admin").unwrap();
    assert_eq!(c.hello().unwrap_err(), ClientError::NotPaired, "a new pairing code is on screen when the revoked client tries Connect");
    let devices = host.link.devices().unwrap();
    assert!(devices.iter().find(|d| d.id == paired.device_id).unwrap().revoked_at.is_some());
}

/// A man in the middle that terminates TLS itself and relays the pairing frame to the real host.
fn relay_mitm(host_addr: String, answer_itself: bool) -> String {
    #[derive(Debug)]
    struct Open;
    impl DeviceGate for Open {
        fn known(&self, _: &Pin) -> bool {
            true
        }
        fn pairing_open(&self) -> bool {
            true
        }
    }
    let identity = Identity::generate("mitm").unwrap();
    let config = tls::server_config(&identity, Arc::new(Open)).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    std::thread::spawn(move || {
        for socket in listener.incoming() {
            let mut socket = socket.unwrap();
            let mut tls_conn = ServerConnection::new(config.clone()).unwrap();
            if tls::handshake(&mut tls_conn, &mut socket, Instant::now() + Duration::from_secs(5)).is_err() {
                continue;
            }
            let mut victim = StreamOwned::new(tls_conn, socket);
            let Ok(Some(bytes)) = frame::read_frame(&mut victim, MAX_REQUEST) else { continue };
            let reply = if answer_itself {
                serde_json::to_vec(&serde_json::json!({"type": "Paired", "device_id": "x", "proof": "00".repeat(32), "host_name": "Office PC"})).unwrap()
            } else {
                let mine = Identity::generate("mitm-client").unwrap();
                let (cfg, _) = tls::client_config(&mine, None).unwrap();
                let mut up = TcpStream::connect(&host_addr).unwrap();
                let mut c = ClientConnection::new(cfg, tls::server_name()).unwrap();
                tls::handshake(&mut c, &mut up, Instant::now() + Duration::from_secs(5)).unwrap();
                let mut up = StreamOwned::new(c, up);
                frame::write_frame(&mut up, &bytes, MAX_REQUEST).unwrap();
                frame::read_frame(&mut up, MAX_REQUEST).unwrap().unwrap()
            };
            let _ = frame::write_frame(&mut victim, &reply, MAX_REQUEST);
        }
    });
    addr
}

#[test]
fn a_man_in_the_middle_cannot_pair_or_impersonate_the_host() {
    let host = start();
    // Relaying: the proof covers the certificates each side saw, so the real host refuses it.
    let code = host.link.arm_pairing("Jordan Admin").unwrap();
    let mitm = relay_mitm(host.link.addr.clone(), false);
    match client::pair(&mitm, &code, "Laptop") {
        Err(ClientError::Pairing(m)) => assert_eq!(m, fb_remote::pairing::REFUSED),
        other => panic!("{:?}", other.err()),
    }
    assert!(host.has("pair.refused"));
    assert!(host.link.devices().unwrap().is_empty(), "nothing was paired");
    // Answering itself: it cannot prove it knows the code, so the client pins nothing.
    let fake = relay_mitm(host.link.addr.clone(), true);
    assert_eq!(client::pair(&fake, &code, "Laptop").err().unwrap(), ClientError::HostMismatch);
    // The real host still pairs with the same live code.
    let paired = client::pair(&host.link.addr, &code, "Laptop").unwrap();
    // After pairing, a different certificate at the host's address is refused before anything is sent.
    let (notify, _) = links();
    let c = Client::new(&fake, &paired.identity, paired.host_pin, notify).unwrap();
    assert_eq!(c.hello().unwrap_err(), ClientError::HostMismatch);
    let (notify, _) = links();
    let mut wrong_pin = paired.host_pin;
    wrong_pin[0] ^= 1;
    let c = Client::new(&host.link.addr, &paired.identity, wrong_pin, notify).unwrap();
    assert_eq!(c.hello().unwrap_err(), ClientError::HostMismatch);
}

fn raw(addr: &str, identity: &Identity, pin: Pin) -> StreamOwned<ClientConnection, TcpStream> {
    let (cfg, _) = tls::client_config(identity, Some(pin)).unwrap();
    let mut socket = TcpStream::connect(addr).unwrap();
    let mut c = ClientConnection::new(cfg, tls::server_name()).unwrap();
    tls::handshake(&mut c, &mut socket, Instant::now() + Duration::from_secs(5)).unwrap();
    socket.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    StreamOwned::new(c, socket)
}

fn ask(s: &mut StreamOwned<ClientConnection, TcpStream>, r: &ClientRequest) -> HostResponse {
    frame::send(s, r, MAX_REQUEST).unwrap();
    frame::recv(s, MAX_RESPONSE).unwrap().unwrap()
}

#[test]
fn unauthenticated_and_malformed_requests_are_refused() {
    let host = start();
    let paired = pair(&host, "Front desk");
    let other = pair(&host, "Back office");
    let mut s = raw(&host.link.addr, &paired.identity, paired.host_pin);
    // A paired computer that has not said Hello has no session.
    assert_eq!(refusal(&ask(&mut s, &ClientRequest::Render { target: home() })), RefusalCode::Unauthenticated);
    assert_eq!(refusal(&ask(&mut s, &ClientRequest::Run { request_id: request_id(), command: form(&journal(), &[]) })), RefusalCode::Unauthenticated);
    let HostResponse::Session { session, signed_in: None, resumed: false, .. } = ask(&mut s, &ClientRequest::Hello { resume: None }) else { panic!() };
    // Signed-out work is refused or sent to sign-in; nothing reaches the book's forms.
    assert_eq!(refusal(&ask(&mut s, &ClientRequest::Query { query: r#"{"type":"Companies"}"#.into() })), RefusalCode::SignInRequired);
    assert_eq!(refusal(&ask(&mut s, &ClientRequest::Run { request_id: request_id(), command: form(&journal(), &[("memo", "x")]) })), RefusalCode::SignInRequired);
    assert_eq!(host.book.inner.lock().unwrap().runs, 0);
    // Host-only work stays on the host even after sign-in.
    ask(&mut s, &ClientRequest::Run { request_id: request_id(), command: form("/desktop/sign-in/", &[("action", "signin"), ("username", "dana"), ("password", USERS[0].1)]) });
    assert_eq!(refusal(&ask(&mut s, &ClientRequest::Render { target: "/settings/book-protection/".into() })), RefusalCode::HostOnly);
    assert_eq!(refusal(&ask(&mut s, &ClientRequest::Run { request_id: request_id(), command: r#"{"type":"ExportBackup"}"#.into() })), RefusalCode::HostOnly);
    assert_eq!(refusal(&ask(&mut s, &ClientRequest::Run { request_id: request_id(), command: form(&format!("/c/{COMPANY}/x/dialog/"), &[]) })), RefusalCode::HostOnly, "host dialog outcomes are not forwarded");
    assert_eq!(refusal(&ask(&mut s, &ClientRequest::Run { request_id: "not-a-uuid".into(), command: form(&journal(), &[]) })), RefusalCode::Invalid);
    // Another computer cannot take over this session with its token.
    let mut o = raw(&host.link.addr, &other.identity, other.host_pin);
    let HostResponse::Session { session: theirs, resumed, signed_in, .. } = ask(&mut o, &ClientRequest::Hello { resume: Some(session.clone()) }) else { panic!() };
    assert!(!resumed && signed_in.is_none() && theirs != session);
    // An unknown request type or an extra field ends the connection with a refusal.
    frame::write_frame(&mut o, br#"{"type":"Render","target":"/","actor":"dana"}"#, MAX_REQUEST).unwrap();
    assert_eq!(refusal(&frame::recv::<_, HostResponse>(&mut o, MAX_RESPONSE).unwrap().unwrap()), RefusalCode::Invalid);
    // A frame over the limit is refused without being read.
    let mut big = raw(&host.link.addr, &other.identity, other.host_pin);
    big.write_all(&((MAX_REQUEST as u32) + 1).to_be_bytes()).unwrap();
    big.flush().unwrap();
    assert_eq!(refusal(&frame::recv::<_, HostResponse>(&mut big, MAX_RESPONSE).unwrap().unwrap()), RefusalCode::Invalid);
}

#[test]
fn a_retried_command_is_answered_once_and_a_reused_id_conflicts() {
    let host = start();
    let paired = pair(&host, "Front desk");
    let c = connect_to(&host.link.addr, &paired);
    sign_in(&c, "dana", USERS[0].1);
    let id = request_id();
    let first = post(&c, &id, "rent");
    let again = post(&c, &id, "rent");
    assert_eq!(first, again, "the same answer");
    assert_eq!(host.ledger().len(), 1, "posted once");
    assert!(host.has("request.replayed"));
    assert_eq!(refusal(&post(&c, &id, "something else")), RefusalCode::Conflict);
    assert_eq!(host.ledger().len(), 1);
}

/// A TCP relay that can cut every connection through it at once.
struct Cutter {
    addr: String,
    live: Arc<Mutex<Vec<TcpStream>>>,
}

impl Cutter {
    fn start(upstream: String) -> Cutter {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let live: Arc<Mutex<Vec<TcpStream>>> = Arc::default();
        let l = live.clone();
        std::thread::spawn(move || {
            for down in listener.incoming() {
                let down = down.unwrap();
                let up = TcpStream::connect(&upstream).unwrap();
                l.lock().unwrap().extend([down.try_clone().unwrap(), up.try_clone().unwrap()]);
                for (mut from, mut to) in [(down.try_clone().unwrap(), up.try_clone().unwrap()), (up, down)] {
                    std::thread::spawn(move || {
                        let _ = std::io::copy(&mut from, &mut to);
                        let _ = to.shutdown(Shutdown::Both);
                    });
                }
            }
        });
        Cutter { addr, live }
    }
    fn cut(&self) {
        for s in self.live.lock().unwrap().drain(..) {
            let _ = s.shutdown(Shutdown::Both);
        }
    }
}

#[test]
fn network_loss_after_commit_never_posts_twice() {
    let host = start();
    let paired = pair(&host, "Front desk");
    let cutter = Arc::new(Cutter::start(host.link.addr.clone()));
    let (notify, seen) = links();
    let c = Client::new(&cutter.addr, &paired.identity, paired.host_pin, notify).unwrap().with_retry(Duration::from_secs(10));
    c.hello().unwrap();
    sign_in(&c, "dana", USERS[0].1);
    // The host commits, then the network drops before the answer gets back.
    let k = cutter.clone();
    *host.book.after_commit.lock().unwrap() = Some(Box::new(move || k.cut()));
    let answer = post(&c, &request_id(), "cut");
    assert!(matches!(answer, HostResponse::Outcome { ref outcome } if outcome.contains("/journal/1/")), "{answer:?}");
    assert_eq!(host.ledger().len(), 1, "the retry got the first answer instead of posting again");
    let links = seen.lock().unwrap().clone();
    assert!(links.contains(&Link::Reconnecting) && links.last() == Some(&Link::Connected), "{links:?}");
    // The session survived the reconnect: still signed in.
    assert_eq!(page(&render(&c, &home())), "home of dana: 1 of 1 entries");
}

#[test]
fn a_host_that_stays_away_is_reported_lost() {
    let host = start();
    let paired = pair(&host, "Front desk");
    let (notify, seen) = links();
    let c = Client::new(&host.link.addr, &paired.identity, paired.host_pin, notify).unwrap().with_retry(Duration::from_millis(800));
    c.hello().unwrap();
    let addr = host.link.addr.clone();
    host.link.stop();
    assert_eq!(c.call(&ClientRequest::Render { target: home() }).unwrap_err(), ClientError::Lost);
    assert_eq!(seen.lock().unwrap().last(), Some(&Link::Lost));
    drop(addr);
}

#[test]
fn two_computers_have_independent_sessions() {
    let host = start();
    let a = pair(&host, "Front desk");
    let b = pair(&host, "Back office");
    let ca = connect_to(&host.link.addr, &a);
    let cb = connect_to(&host.link.addr, &b);
    sign_in(&ca, "dana", USERS[0].1);
    sign_in(&cb, "lee", USERS[1].1);
    let threads: Vec<_> = [(ca, "dana"), (cb, "lee")]
        .into_iter()
        .map(|(c, who)| {
            std::thread::spawn(move || {
                for i in 0..10 {
                    assert!(matches!(post(&c, &request_id(), &format!("{who} {i}")), HostResponse::Outcome { .. }));
                }
                c
            })
        })
        .collect();
    let mut clients: Vec<Client> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    let cb = clients.pop().unwrap();
    let ca = clients.pop().unwrap();
    let ledger = host.ledger();
    assert_eq!(ledger.len(), 20);
    assert_eq!(ledger.iter().filter(|(u, m)| u == "dana" && m.starts_with("dana")).count(), 10, "each entry is attributed to its own session's user");
    assert!(matches!(cb.call(&ClientRequest::SignOut).unwrap(), HostResponse::SignedOut));
    assert_eq!(page(&render(&ca, &home())), "home of dana: 10 of 20 entries", "one person signing out does not sign out another");
}

#[test]
fn idle_sessions_end_and_sign_in_is_throttled() {
    let host = start_with(Config { idle: Duration::from_millis(500), ..Config::default() });
    let paired = pair(&host, "Front desk");
    let c = connect_to(&host.link.addr, &paired);
    sign_in(&c, "dana", USERS[0].1);
    std::thread::sleep(Duration::from_millis(700));
    assert_eq!(refusal(&render(&c, &home())), RefusalCode::Unauthenticated);
    assert!(host.has("session.expired"));
    // A new Hello starts a fresh, signed-out session; five failures then a wait.
    c.hello().unwrap();
    for _ in 0..5 {
        assert!(matches!(sign_in(&c, "dana", "nope"), HostResponse::Outcome { .. }));
    }
    assert_eq!(refusal(&sign_in(&c, "dana", USERS[0].1)), RefusalCode::Throttled);
    assert!(host.has("sign_in.throttled"));
}

#[test]
fn the_listener_refuses_public_and_all_interface_addresses() {
    use fb_remote_proto::link::{HostLink, Start};
    for bind in ["0.0.0.0:0", "[::]:0", "8.8.8.8:47100", "example.com:1"] {
        let dir = tempfile::tempdir().unwrap();
        let d = Arc::new(fb_remote_proto::Dispatcher::new(Synthetic(Arc::default()), Config::default(), Box::new(fb_remote_proto::audit::Memory::default())));
        let r = HostLink::start(d, Start::new(exe(), bind, None, dir.path().to_owned(), "x").unwrap());
        assert!(r.is_err(), "{bind}");
    }
}

#[test]
fn the_host_identity_and_paired_computers_survive_a_restart() {
    use fb_remote_proto::link::{HostLink, Start};
    let host = start();
    let paired = pair(&host, "Front desk");
    let identity = host.link.new_identity.clone().unwrap();
    assert_eq!(host.counter.load(std::sync::atomic::Ordering::SeqCst), 1, "the pairing wrote the list once and said so");
    let Host { link, book, dir, registry_key, .. } = host;
    link.stop();
    let audit_path = dir.path().join("remote-audit.jsonl");
    let key = fb_remote_proto::audit::AuditKey::generate().unwrap();
    let audit = fb_remote_proto::audit::AuditLog::open(&audit_path, &key, fb_remote_proto::audit::Expect::New).unwrap();
    let d = Arc::new(fb_remote_proto::Dispatcher::new(Synthetic(book), Config::default(), Box::new(audit)));
    let mut again = Start::new(exe(), "127.0.0.1:0", Some(identity.clone()), dir.path().to_owned(), "Office PC").unwrap();
    again.registry_key = registry_key;
    again.min_counter = 1;
    let link = HostLink::start(d, again).unwrap();
    assert!(link.new_identity.is_none());
    assert_eq!(link.counter, 1);
    let c = connect_to(&link.addr, &paired);
    assert!(matches!(sign_in(&c, "dana", USERS[0].1), HostResponse::Outcome { .. }));
    link.stop();
    let lines = fb_remote_proto::audit::verify(&audit_path, &key).unwrap();
    assert!(lines >= 4, "host.started, connect, session.opened, sign_in.succeeded…: {lines}");
    let text = std::fs::read_to_string(&audit_path).unwrap();
    assert!(!text.contains(USERS[0].1), "no password in the audit");
    // Another list key cannot read this host's registry (the key is the sealed record's, not
    // derived from the TLS identity).
    let d = Arc::new(fb_remote_proto::Dispatcher::new(Synthetic(Arc::default()), Config::default(), Box::new(fb_remote_proto::audit::Memory::default())));
    assert!(HostLink::start(d, Start::new(exe(), "127.0.0.1:0", Some(identity.clone()), dir.path().to_owned(), "x").unwrap()).is_err());
    // A counter the sealed record has seen past is refused: an older copy of the list.
    let d = Arc::new(fb_remote_proto::Dispatcher::new(Synthetic(Arc::default()), Config::default(), Box::new(fb_remote_proto::audit::Memory::default())));
    let mut later = Start::new(exe(), "127.0.0.1:0", Some(identity), dir.path().to_owned(), "x").unwrap();
    later.registry_key = registry_key;
    later.min_counter = 2;
    let err = HostLink::start(d, later).err().unwrap();
    assert!(err.to_string().contains("older than the one this host recorded"), "{err}");
}

/// S7: a revocation the host cannot save (a full or read-only disk) still cuts the computer off,
/// says so, and is saved once the disk allows.
#[cfg(unix)]
#[test]
fn a_revocation_that_cannot_be_saved_still_cuts_the_computer_off() {
    use std::os::unix::fs::PermissionsExt;
    let host = start();
    let stolen = pair(&host, "Stolen laptop");
    let c = connect_to(&host.link.addr, &stolen);
    sign_in(&c, "dana", USERS[0].1);
    std::fs::set_permissions(host.dir.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
    if std::fs::write(host.dir.path().join("probe"), b"x").is_ok() {
        return; // running as root: permissions do not stop writes
    }
    let r = host.link.revoke(&stolen.device_id, "Jordan Admin").unwrap();
    assert!(r.revoked && r.unsaved.as_deref().is_some_and(|m| m.contains("could not be saved")), "{r:?}");
    assert!(host.events.lock().unwrap().iter().any(|e| e.kind == "device.revoked" && e.user.as_deref() == Some("Jordan Admin")));
    assert!(host.events.lock().unwrap().iter().any(|e| e.kind == "device.revocation_unsaved" && e.detail.contains("not saved")));
    assert!(host.link.dispatcher().revocation(&stolen.device_id).unwrap().is_some(), "outstanding for the host's screen");
    assert_eq!(c.call(&ClientRequest::Render { target: home() }).unwrap_err(), ClientError::NotPaired, "cut off at once");
    assert_eq!(host.sessions(), 0);
    let (notify, _) = links();
    let again = Client::new(&host.link.addr, &stolen.identity, stolen.host_pin, notify).unwrap().with_retry(Duration::from_secs(2));
    assert!(again.hello().is_err(), "and refused when it reconnects");
    std::fs::set_permissions(host.dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    host.link.devices().unwrap();
    assert!(host.has("device.revocation_saved"));
    assert_eq!(host.link.dispatcher().revocation(&stolen.device_id), Some(None), "saved: nothing outstanding");
    let saved = std::fs::read_to_string(host.dir.path().join("remote-devices.json")).unwrap();
    let saved: serde_json::Value = serde_json::from_str(&saved).unwrap();
    assert!(saved["devices"].as_array().unwrap().iter().any(|d| d["id"] == stolen.device_id.as_str() && !d["revoked_at"].is_null()), "saved once the disk allows");
}

/// S8: a connection that never starts its handshake is closed within a few seconds.
#[test]
fn a_silent_connection_is_closed_quickly() {
    let host = start();
    let mut silent = std::net::TcpStream::connect(&host.link.addr).unwrap();
    silent.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let started = std::time::Instant::now();
    let mut buf = [0u8; 16];
    let n = std::io::Read::read(&mut silent, &mut buf).unwrap_or(0);
    assert_eq!(n, 0, "closed without an answer");
    assert!(started.elapsed() < Duration::from_secs(5), "closed after {:?}", started.elapsed());
}

/// Resumes a stopped gateway when the test ends, whatever happens.
#[cfg(unix)]
struct Frozen(i32);

#[cfg(unix)]
impl Frozen {
    fn new(pid: u32) -> Frozen {
        let pid = pid as i32;
        // SAFETY: signalling our own child process.
        assert_eq!(unsafe { libc::kill(pid, libc::SIGSTOP) }, 0);
        Frozen(pid)
    }
}

#[cfg(unix)]
impl Drop for Frozen {
    fn drop(&mut self) {
        // SAFETY: as above.
        unsafe { libc::kill(self.0, libc::SIGCONT) };
    }
}

/// The re-check's A2 (`probe-host2.out` [R]): with the gateway stalled, a revocation was never
/// recorded, the computer's session stayed signed in, and a retry said "no such active device".
#[cfg(unix)]
#[test]
fn a_revocation_the_listener_does_not_answer_is_recorded_and_in_force_in_the_books() {
    use fb_remote_proto::dispatch::NOT_CONFIRMED;
    let mut host = start();
    host.link.set_control_timeout(Duration::from_secs(1));
    let laptop = pair(&host, "Laptop B");
    let shared = pair(&host, "Shared PC");
    let c = connect_to(&host.link.addr, &laptop);
    let s = connect_to(&host.link.addr, &shared);
    sign_in(&c, "dana", USERS[0].1);
    sign_in(&s, "lee", USERS[1].1);
    assert_eq!(host.sessions(), 2);
    let frozen = Frozen::new(host.link.gateway_pid());
    let r = host.link.revoke(&laptop.device_id, "Jordan Admin").unwrap();
    assert!(r.revoked && !r.already, "{r:?}");
    assert_eq!(r.unsaved.as_deref(), Some(NOT_CONFIRMED), "the host is told it is not confirmed yet");
    assert_eq!(host.sessions(), 1, "the laptop's session ended in the books at once; the other's did not");
    let revoked: Vec<_> = host.events.lock().unwrap().iter().filter(|e| e.kind == "device.revoked").cloned().collect();
    assert_eq!(revoked.len(), 1, "recorded by the book process without the gateway");
    assert_eq!((revoked[0].device_id.as_deref(), revoked[0].device_name.as_deref(), revoked[0].user.as_deref()), (Some(laptop.device_id.as_str()), Some("Laptop B"), Some("Jordan Admin")));
    assert!(host.kinds().iter().any(|k| k == "device.revocation_unconfirmed"));
    let again = host.link.revoke(&laptop.device_id, "Jordan Admin").unwrap();
    assert!(again.revoked && again.already && again.unsaved.as_deref() == Some(NOT_CONFIRMED), "a retry is answered as revoked: {again:?}");
    drop(frozen);
    // The late answer is recorded whenever it arrives and clears the warning.
    assert!(host.has("device.revocation_confirmed"));
    assert_eq!(host.link.dispatcher().revocation(&laptop.device_id), Some(None));
    assert_eq!(host.link.revoke(&laptop.device_id, "Jordan Admin").unwrap(), fb_remote_proto::link::Revocation { revoked: true, already: true, unsaved: None });
    assert_eq!(host.kinds().iter().filter(|k| *k == "device.revoked").count(), 1, "still recorded once");
    assert_eq!(c.call(&ClientRequest::Render { target: home() }).unwrap_err(), ClientError::NotPaired);
    assert_eq!(page(&render(&s, &home())), "home of lee: 0 of 0 entries", "the other computer is unaffected");
    assert!(host.link.devices().unwrap().iter().find(|d| d.id == laptop.device_id).unwrap().revoked_at.is_some());
}

/// A TCP connection to `addr` from the loopback address `src`, that never says anything.
#[cfg(target_os = "linux")]
fn silent_from(src: std::net::Ipv4Addr, addr: &str) -> TcpStream {
    use std::os::fd::FromRawFd;
    let dst: std::net::SocketAddrV4 = addr.parse().unwrap();
    let sockaddr = |ip: std::net::Ipv4Addr, port: u16| libc::sockaddr_in { sin_family: libc::AF_INET as libc::sa_family_t, sin_port: port.to_be(), sin_addr: libc::in_addr { s_addr: u32::from(ip).to_be() }, sin_zero: [0; 8] };
    let len = std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
    // SAFETY: a fresh socket, bound and connected with valid sockaddr_in values, then owned by
    // the TcpStream.
    unsafe {
        let fd = libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0);
        assert!(fd >= 0);
        let local = sockaddr(src, 0);
        assert_eq!(libc::bind(fd, (&raw const local).cast(), len), 0, "bind {src}");
        let remote = sockaddr(*dst.ip(), dst.port());
        assert_eq!(libc::connect(fd, (&raw const remote).cast(), len), 0, "connect from {src}");
        TcpStream::from_raw_fd(fd)
    }
}

/// The re-check's A3 (`probe-host2.out` [S8], `probe-limits2.out` [B]/[C]): a paired computer
/// connecting for the first time in this run (just paired) was refused while 32 silent
/// connections from 4 addresses were open, and could be kept out indefinitely from 32 addresses.
#[cfg(target_os = "linux")]
#[test]
fn silent_connections_cannot_keep_a_paired_computer_out() {
    let host = start();
    let paired = pair(&host, "Just paired");
    let mut silent = vec![];
    for a in 1..=4 {
        for _ in 0..8 {
            silent.push(silent_from(std::net::Ipv4Addr::new(127, 0, 1, a), &host.link.addr));
        }
    }
    std::thread::sleep(Duration::from_millis(200));
    let started = Instant::now();
    let c = connect_to(&host.link.addr, &paired);
    assert!(started.elapsed() < Duration::from_secs(1), "admitted at once past 32 silent connections from 4 addresses: {:?}", started.elapsed());
    assert!(matches!(sign_in(&c, "dana", USERS[0].1), HostResponse::Outcome { .. }));
    drop(c);
    // 32 more addresses, one silent connection each: the pending pool is full.
    for a in 1..=32 {
        silent.push(silent_from(std::net::Ipv4Addr::new(127, 0, 2, a), &host.link.addr));
    }
    std::thread::sleep(Duration::from_millis(200));
    let started = Instant::now();
    let c = connect_to(&host.link.addr, &paired);
    assert!(started.elapsed() < Duration::from_secs(1), "admitted at once past a full pending pool: {:?}", started.elapsed());
    assert!(matches!(sign_in(&c, "dana", USERS[0].1), HostResponse::Outcome { .. }));
    assert_eq!(page(&render(&c, &home())), "home of dana: 0 of 0 entries");
    // And a new computer can pair while the pool is full.
    for a in 1..=32 {
        silent.push(silent_from(std::net::Ipv4Addr::new(127, 0, 3, a), &host.link.addr));
    }
    std::thread::sleep(Duration::from_millis(200));
    let other = pair(&host, "Second laptop");
    drop(connect_to(&host.link.addr, &other));
    drop(silent);
}

/// Hosting stops with connected computers: while it stops they are told why (refusal code
/// host_locked with the host's words), then the connection ends.
#[test]
fn computers_connected_while_hosting_stops_are_told_why() {
    let host = start();
    let paired = pair(&host, "Front desk");
    let c = connect_to(&host.link.addr, &paired);
    assert!(matches!(sign_in(&c, "dana", USERS[0].1), HostResponse::Outcome { .. }));
    let Host { link, .. } = host;
    let stopper = std::thread::spawn(move || link.stop_with("The host stopped sharing these books.", Duration::from_secs(5)));
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let told = loop {
        match c.call(&ClientRequest::Ping) {
            Ok(HostResponse::Refused { code: RefusalCode::HostLocked, message }) => break message,
            _ if std::time::Instant::now() > deadline => panic!("never told"),
            _ => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    assert_eq!(told, "The host stopped sharing these books.");
    c.close();
    stopper.join().unwrap();
}
