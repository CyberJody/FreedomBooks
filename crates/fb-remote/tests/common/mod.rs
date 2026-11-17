//! A synthetic book behind the remote boundary, and helpers that start the real `fb-remote`
//! gateway process on loopback. No real book, key or user data.
#![allow(dead_code)]
use fb_remote::client::{self, Client, Link, Paired};
use fb_remote_proto::audit::{Event, Memory};
use fb_remote_proto::link::{HostLink, Start};
use fb_remote_proto::{Backend, BackendError, Config, DeviceRef, Dispatcher, Remote, Rendered, SessionRef, SignInState, SignedIn};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const COMPANY: &str = "0190b6f4-6b3e-7c4f-9a0e-1f2d3c4b5a69";
pub const USERS: &[(&str, &str)] = &[("dana", "correct horse battery"), ("lee", "staple orbit lantern")];

#[derive(Default)]
pub struct Inner {
    pub next: u64,
    pub sessions: HashMap<u64, Option<String>>,
    pub ledger: Vec<(String, String)>,
    pub runs: usize,
}

#[derive(Default)]
pub struct Book {
    pub inner: Mutex<Inner>,
    /// Called after a "cut" entry is committed and before its answer leaves (network loss).
    pub after_commit: Mutex<Option<Box<dyn Fn() + Send>>>,
}

#[derive(Clone)]
pub struct Synthetic(pub Arc<Book>);

fn redirect(location: &str) -> String {
    serde_json::json!({"outcome": "redirect", "location": location}).to_string()
}

impl Backend for Synthetic {
    fn classify(&self, path: &str) -> Option<Remote> {
        // The synthetic book's own routes; anything else is unclassified, so refused.
        let company = format!("/c/{COMPANY}/");
        if path == "/desktop/sign-in/" || path == format!("{company}home/") || path == format!("{company}journal/new/") || (path.starts_with(&company) && path.ends_with("/dialog/")) {
            return Some(Remote::Allowed);
        }
        None
    }
    fn open_session(&self, _device: &DeviceRef) -> Result<SessionRef, BackendError> {
        let mut b = self.0.inner.lock().unwrap();
        b.next += 1;
        let id = b.next;
        b.sessions.insert(id, None);
        Ok(SessionRef(id))
    }
    fn close_session(&self, session: SessionRef) {
        self.0.inner.lock().unwrap().sessions.remove(&session.0);
    }
    fn sign_in_state(&self, session: SessionRef) -> Result<SignInState, BackendError> {
        match self.0.inner.lock().unwrap().sessions.get(&session.0) {
            None => Err(BackendError::NoSession),
            Some(None) => Ok(SignInState::SignedOut),
            Some(Some(u)) => Ok(SignInState::SignedIn(SignedIn { user_id: u.clone(), user: u.clone(), role: "bookkeeper".into() })),
        }
    }
    fn render(&self, session: SessionRef, target: &str) -> Result<Rendered, BackendError> {
        let b = self.0.inner.lock().unwrap();
        let user = b.sessions.get(&session.0).ok_or(BackendError::NoSession)?.clone();
        if target.starts_with("/desktop/sign-in/") {
            return Ok(Rendered { html: "<form>sign in</form>".into(), status: 200, route: "sign-in".into() });
        }
        let user = user.ok_or_else(|| BackendError::Refused("Sign in.".into()))?;
        if target == format!("/c/{COMPANY}/home/") {
            let mine = b.ledger.iter().filter(|(u, _)| *u == user).count();
            return Ok(Rendered { html: format!("home of {user}: {mine} of {} entries", b.ledger.len()), status: 200, route: "company_home".into() });
        }
        Err(BackendError::Refused(format!("No screen at {target}")))
    }
    fn run(&self, session: SessionRef, command: &str) -> Result<String, BackendError> {
        let v: Value = serde_json::from_str(command).map_err(|_| BackendError::Refused("bad".into()))?;
        let path = v["path"].as_str().unwrap_or("").to_owned();
        let field = |name: &str| v["fields"].as_array().into_iter().flatten().find(|f| f[0] == name).and_then(|f| f[1].as_str()).unwrap_or("").to_owned();
        let mut b = self.0.inner.lock().unwrap();
        b.runs += 1;
        if !b.sessions.contains_key(&session.0) {
            return Err(BackendError::NoSession);
        }
        if path == "/desktop/sign-in/" {
            let (user, pass) = (field("username"), field("password"));
            if USERS.iter().any(|(u, p)| *u == user && *p == pass) {
                b.sessions.insert(session.0, Some(user));
                return Ok(redirect(&format!("/c/{COMPANY}/home/")));
            }
            return Ok(serde_json::json!({"outcome": "invalid", "html": "The user name or password is incorrect."}).to_string());
        }
        let user = b.sessions[&session.0].clone().ok_or_else(|| BackendError::Refused("Sign in.".into()))?;
        if path.ends_with("/journal/new/") {
            let memo = field("memo");
            b.ledger.push((user, memo.clone()));
            let n = b.ledger.len();
            drop(b);
            if memo == "cut" {
                if let Some(hook) = self.0.after_commit.lock().unwrap().as_ref() {
                    hook();
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            return Ok(redirect(&format!("/c/{COMPANY}/journal/{n}/")));
        }
        if path.ends_with("/dialog/") {
            return Ok(serde_json::json!({"outcome": "done", "data": {"desktop": "pick-attachment", "ticket": "t"}}).to_string());
        }
        Err(BackendError::Refused("No such form.".into()))
    }
    fn query(&self, session: SessionRef, _query: &str) -> Result<String, BackendError> {
        self.sign_in_state(session).map(|_| "[]".into())
    }
    fn sign_out(&self, session: SessionRef) -> Result<(), BackendError> {
        let mut b = self.0.inner.lock().unwrap();
        let s = b.sessions.get_mut(&session.0).ok_or(BackendError::NoSession)?;
        *s = None;
        Ok(())
    }
}

pub struct Host {
    pub link: HostLink<Synthetic>,
    pub book: Arc<Book>,
    pub events: Arc<Mutex<Vec<Event>>>,
    pub dir: tempfile::TempDir,
    /// The list of paired computers' key (kept by the host's sealed record in the product).
    pub registry_key: [u8; 32],
    /// The list's counter, as reported after every write.
    pub counter: Arc<std::sync::atomic::AtomicU64>,
}

impl Host {
    pub fn kinds(&self) -> Vec<String> {
        self.events.lock().unwrap().iter().map(|e| e.kind.clone()).collect()
    }
    pub fn has(&self, kind: &str) -> bool {
        // Gateway events arrive asynchronously over the pipe.
        for _ in 0..50 {
            if self.kinds().iter().any(|k| k == kind) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }
    pub fn ledger(&self) -> Vec<(String, String)> {
        self.book.inner.lock().unwrap().ledger.clone()
    }
    pub fn sessions(&self) -> usize {
        self.book.inner.lock().unwrap().sessions.len()
    }
}

pub fn exe() -> std::path::PathBuf {
    env!("CARGO_BIN_EXE_fb-remote").into()
}

pub fn start_with(config: Config) -> Host {
    let dir = tempfile::tempdir().unwrap();
    let book = Arc::new(Book::default());
    let memory = Memory::default();
    let events = memory.0.clone();
    let dispatcher = Arc::new(Dispatcher::new(Synthetic(book.clone()), Config { host_name: "Office PC".into(), ..config }, Box::new(memory)));
    let mut start = Start::new(exe(), "127.0.0.1:0", None, dir.path().to_owned(), "Office PC").unwrap();
    let registry_key = start.registry_key;
    let counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let seen = counter.clone();
    start.on_registry = Some(Box::new(move |c| seen.store(c, std::sync::atomic::Ordering::SeqCst)));
    let link = HostLink::start(dispatcher, start).unwrap();
    assert!(link.new_identity.is_some(), "a first start makes the host identity");
    Host { link, book, events, dir, registry_key, counter }
}

pub fn start() -> Host {
    start_with(Config::default())
}

pub fn pair(host: &Host, name: &str) -> Paired {
    let code = host.link.arm_pairing("Jordan Admin").unwrap();
    client::pair(&host.link.addr, &fb_remote_proto::code::display(&code), name).unwrap()
}

pub type Notify = Box<dyn Fn(Link) + Send + Sync>;

pub fn links() -> (Notify, Arc<Mutex<Vec<Link>>>) {
    let seen = Arc::new(Mutex::new(vec![]));
    let s = seen.clone();
    (Box::new(move |l| s.lock().unwrap().push(l)), seen)
}

pub fn connect_to(addr: &str, paired: &Paired) -> Client {
    let (notify, _) = links();
    let c = Client::new(addr, &paired.identity, paired.host_pin, notify).unwrap().with_retry(Duration::from_secs(5));
    c.hello().unwrap();
    c
}

pub fn form(path: &str, fields: &[(&str, &str)]) -> String {
    serde_json::json!({"type": "SubmitForm", "path": path, "token": "t", "fields": fields.iter().map(|(k, v)| [k, v]).collect::<Vec<_>>()}).to_string()
}

pub fn request_id() -> String {
    let b: [u8; 16] = fb_remote_proto::random().unwrap();
    let h = fb_remote_proto::hex(&b);
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

pub fn journal() -> String {
    format!("/c/{COMPANY}/journal/new/")
}

pub fn home() -> String {
    format!("/c/{COMPANY}/home/")
}

/// A TCP relay that, once armed, cuts every connection when the host's next bytes arrive: the
/// host has finished (committed) and its answer is lost on the way back.
pub struct Dropper {
    pub addr: String,
    armed: Arc<std::sync::atomic::AtomicBool>,
    live: Arc<Mutex<Vec<std::net::TcpStream>>>,
}

impl Dropper {
    pub fn start(upstream: String) -> Dropper {
        use std::io::{Read, Write};
        use std::net::{Shutdown, TcpListener, TcpStream};
        use std::sync::atomic::Ordering;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let armed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let live: Arc<Mutex<Vec<TcpStream>>> = Arc::default();
        let (a, l) = (armed.clone(), live.clone());
        std::thread::spawn(move || {
            for down in listener.incoming() {
                let down = down.unwrap();
                let up = TcpStream::connect(&upstream).unwrap();
                l.lock().unwrap().extend([down.try_clone().unwrap(), up.try_clone().unwrap()]);
                let (mut d1, mut u1) = (down.try_clone().unwrap(), up.try_clone().unwrap());
                std::thread::spawn(move || {
                    let _ = std::io::copy(&mut d1, &mut u1);
                    let _ = u1.shutdown(Shutdown::Both);
                });
                let (mut u2, mut d2, a, l) = (up, down, a.clone(), l.clone());
                std::thread::spawn(move || {
                    let mut buf = [0u8; 16384];
                    loop {
                        let n = match u2.read(&mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => n,
                        };
                        if a.swap(false, Ordering::SeqCst) {
                            for s in l.lock().unwrap().drain(..) {
                                let _ = s.shutdown(Shutdown::Both);
                            }
                            break;
                        }
                        if d2.write_all(&buf[..n]).is_err() {
                            break;
                        }
                    }
                    let _ = d2.shutdown(Shutdown::Both);
                });
            }
        });
        Dropper { addr, armed, live }
    }

    /// Lose the host's next answer.
    pub fn arm(&self) {
        self.armed.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn connections(&self) -> usize {
        self.live.lock().unwrap().len()
    }
}
