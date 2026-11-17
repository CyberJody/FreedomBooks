//! `fb-remote client`: the desktop main process's door to a host. Frames in ([`In`]) and out
//! ([`Out`]) over stdio. The parent keeps the device identity and host pin sealed by the OS store
//! and hands them over in `Connect`; this process keeps them only in memory. Calls run on their
//! own threads so a slow page does not hold up the next one; answers carry the caller's id.
use crate::client::{self, Client, Link};
use crate::tls::{self, Identity};
use fb_remote_proto::frame::{self, MAX_PIPE};
use fb_remote_proto::wire::{ClientRequest, HostResponse, IdentityHex};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

pub const MAX_CALLS: usize = 8;

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum In {
    Pair { id: u64, addr: String, code: String, device_name: String },
    Connect { id: u64, addr: String, identity: IdentityHex, host_pin: String },
    Call { id: u64, request: ClientRequest },
    Close { id: u64 },
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Out {
    Paired { id: u64, identity: IdentityHex, host_pin: String, device_id: String, host_name: String, fingerprint: String },
    Answer { id: u64, response: HostResponse },
    Error { id: u64, code: &'static str, message: String },
    Link { state: &'static str },
    Closed { id: u64 },
}

type Writer = Arc<Mutex<Box<dyn Write + Send>>>;

fn say(out: &Writer, message: &Out) {
    let _ = frame::send(&mut *out.lock().unwrap_or_else(PoisonError::into_inner), message, MAX_PIPE);
}

fn error(id: u64, e: &client::ClientError) -> Out {
    Out::Error { id, code: e.code(), message: e.to_string() }
}

pub fn run(mut input: impl Read, output: impl Write + Send + 'static) -> i32 {
    let out: Writer = Arc::new(Mutex::new(Box::new(output)));
    let current: Arc<Mutex<Option<Arc<Client>>>> = Arc::new(Mutex::new(None));
    let running = Arc::new(AtomicUsize::new(0));
    loop {
        let message = match frame::recv::<_, In>(&mut input, MAX_PIPE) {
            Ok(Some(m)) => m,
            Ok(None) => return 0,
            Err(_) => return 2,
        };
        match message {
            In::Pair { id, addr, code, device_name } => {
                let out = out.clone();
                std::thread::spawn(move || {
                    let reply = match client::pair(&addr, &code, &device_name) {
                        Ok(p) => Out::Paired { id, identity: p.identity.to_hex(), host_pin: fb_remote_proto::hex(&p.host_pin), device_id: p.device_id, host_name: p.host_name, fingerprint: tls::fingerprint(&p.host_pin) },
                        Err(e) => error(id, &e),
                    };
                    say(&out, &reply);
                });
            }
            In::Connect { id, addr, identity, host_pin } => {
                let (out, current) = (out.clone(), current.clone());
                std::thread::spawn(move || {
                    let reply = (|| {
                        let identity = Identity::from_hex(&identity).map_err(client::ClientError::Protocol)?;
                        let pin: tls::Pin = fb_remote_proto::unhex(&host_pin).and_then(|p| p.try_into().ok()).ok_or_else(|| client::ClientError::Protocol("invalid host pin".into()))?;
                        let notify_out = out.clone();
                        let notify = Box::new(move |link: Link| {
                            let state = match link {
                                Link::Connected => "connected",
                                Link::Reconnecting => "reconnecting",
                                Link::Lost => "lost",
                            };
                            say(&notify_out, &Out::Link { state });
                        });
                        let c = Arc::new(Client::new(&addr, &identity, pin, notify)?);
                        let response = c.hello()?;
                        if let Some(old) = current.lock().unwrap_or_else(PoisonError::into_inner).replace(c) {
                            old.close();
                        }
                        Ok::<_, client::ClientError>(Out::Answer { id, response })
                    })();
                    say(&out, &reply.unwrap_or_else(|e| error(id, &e)));
                });
            }
            In::Call { id, request } => {
                let Some(c) = current.lock().unwrap_or_else(PoisonError::into_inner).clone() else {
                    say(&out, &Out::Error { id, code: "not_connected", message: "Not connected to a host.".into() });
                    continue;
                };
                if running.fetch_add(1, Ordering::SeqCst) >= MAX_CALLS {
                    running.fetch_sub(1, Ordering::SeqCst);
                    say(&out, &Out::Error { id, code: "busy", message: "Too many requests at once. Try again.".into() });
                    continue;
                }
                let (out, running) = (out.clone(), running.clone());
                std::thread::spawn(move || {
                    let reply = match c.call(&request) {
                        Ok(response) => Out::Answer { id, response },
                        Err(e) => error(id, &e),
                    };
                    say(&out, &reply);
                    running.fetch_sub(1, Ordering::SeqCst);
                });
            }
            In::Close { id } => {
                if let Some(c) = current.lock().unwrap_or_else(PoisonError::into_inner).take() {
                    c.close();
                }
                say(&out, &Out::Closed { id });
            }
        }
    }
}
