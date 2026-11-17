//! The bounded FIFO queue in front of the session host. One lease at a time touches the books; the
//! others wait in arrival order for at most `Policy::queue_wait`, hearing their position and what
//! holds the books about every 250 ms, and get `Busy` instead of waiting forever.
use super::{HostError, Request, Response, SessionHost, SessionId};
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Where a waiting request stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueueProgress {
    /// 1 = next in line.
    pub position: usize,
    /// What holds the books now ("Dana: saving"), if anything.
    pub holder: Option<String>,
    pub waited: Duration,
}

const TICK: Duration = Duration::from_millis(250);

#[derive(Default)]
struct Queue {
    next: u64,
    holder: Option<(u64, String)>,
    waiting: VecDeque<u64>,
}

struct Inner {
    host: Mutex<SessionHost>,
    queue: Mutex<Queue>,
    turn: Condvar,
    wait: Duration,
    depth: usize,
}

/// The session host for transport threads. Clone it freely; every clone is the same host.
#[derive(Clone)]
pub struct SharedHost(Arc<Inner>);

/// The one right to use the host. Dropping it lets the next waiter in.
pub struct Lease<'a> {
    inner: &'a Inner,
    ticket: u64,
    guard: Option<MutexGuard<'a, SessionHost>>,
}

impl std::ops::Deref for Lease<'_> {
    type Target = SessionHost;
    fn deref(&self) -> &SessionHost {
        self.guard.as_ref().expect("held until drop")
    }
}

impl std::ops::DerefMut for Lease<'_> {
    fn deref_mut(&mut self) -> &mut SessionHost {
        self.guard.as_mut().expect("held until drop")
    }
}

impl Lease<'_> {
    /// Tell waiters what this lease is doing.
    pub fn set_label(&self, label: &str) {
        let mut q = lock(&self.inner.queue);
        if let Some((ticket, held)) = q.holder.as_mut()
            && *ticket == self.ticket
        {
            *held = label.to_owned();
        }
    }
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        self.guard.take();
        let mut q = lock(&self.inner.queue);
        if q.holder.as_ref().is_some_and(|(t, _)| *t == self.ticket) {
            q.holder = None;
        }
        drop(q);
        self.inner.turn.notify_all();
    }
}

/// A place in the queue. Whatever way the waiting ends (its turn, timeout, or a panic in the caller's
/// progress callback), dropping this takes the ticket out of line and wakes the others; only a granted
/// turn hands the ticket on to its [`Lease`].
struct Waiting<'a> {
    inner: &'a Inner,
    ticket: u64,
    granted: bool,
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        if self.granted {
            return;
        }
        let mut q = lock(&self.inner.queue);
        q.waiting.retain(|t| *t != self.ticket);
        drop(q);
        self.inner.turn.notify_all();
    }
}

/// The queue's own lock guards only plain bookkeeping, so a poisoned one is still consistent.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl SharedHost {
    pub fn new(host: SessionHost) -> SharedHost {
        let (wait, depth) = (host.policy().queue_wait, host.policy().queue_depth);
        SharedHost(Arc::new(Inner { host: Mutex::new(host), queue: Mutex::new(Queue::default()), turn: Condvar::new(), wait, depth }))
    }

    /// Wait for the books, in arrival order, for at most the policy's wait. `progress` is called when
    /// the wait begins and about every 250 ms after, never while the queue is locked.
    pub fn lease(&self, label: &str, progress: &mut dyn FnMut(QueueProgress)) -> Result<Lease<'_>, HostError> {
        self.lease_within(label, self.0.wait, progress)
    }

    pub fn lease_within(&self, label: &str, wait: Duration, progress: &mut dyn FnMut(QueueProgress)) -> Result<Lease<'_>, HostError> {
        let inner = &*self.0;
        let start = Instant::now();
        let deadline = start + wait;
        let mut q = lock(&inner.queue);
        if q.waiting.len() >= inner.depth {
            return Err(HostError::Busy { position: q.waiting.len() + 1, holder: q.holder.as_ref().map(|(_, l)| l.clone()), waited: Duration::ZERO });
        }
        let ticket = q.next;
        q.next += 1;
        q.waiting.push_back(ticket);
        let mut place = Waiting { inner, ticket, granted: false };
        loop {
            if q.holder.is_none() && q.waiting.front() == Some(&ticket) {
                q.waiting.pop_front();
                q.holder = Some((ticket, label.to_owned()));
                place.granted = true;
                drop(q);
                // Wake the next in line so it sees its new position.
                inner.turn.notify_all();
                let guard = inner.host.lock().unwrap_or_else(|p| {
                    inner.host.clear_poison();
                    p.into_inner()
                });
                return Ok(Lease { inner, ticket, guard: Some(guard) });
            }
            let position = q.waiting.iter().position(|t| *t == ticket).map_or(1, |i| i + 1);
            let holder = q.holder.as_ref().map(|(_, l)| l.clone());
            let now = Instant::now();
            if now >= deadline {
                drop(q);
                drop(place); // leaves the line and wakes the others
                return Err(HostError::Busy { position, holder, waited: now - start });
            }
            drop(q);
            progress(QueueProgress { position, holder, waited: now - start });
            q = lock(&inner.queue);
            if q.holder.is_none() && q.waiting.front() == Some(&ticket) {
                continue;
            }
            let pause = TICK.min(deadline.saturating_duration_since(Instant::now()));
            q = inner.turn.wait_timeout(q, pause).unwrap_or_else(|p| p.into_inner()).0;
        }
    }

    /// One request from session `id`, after waiting for its turn.
    pub fn dispatch(&self, id: &SessionId, request: Request, progress: &mut dyn FnMut(QueueProgress)) -> Result<Response, HostError> {
        let mut lease = self.lease("a request", progress)?;
        let label = lease.describe(id, &request);
        lease.set_label(&label);
        lease.dispatch(id, request)
    }
}

