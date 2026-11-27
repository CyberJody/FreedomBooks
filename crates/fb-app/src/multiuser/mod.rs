//! Multi-user step 1: several independent sessions against one open book, in one process, without
//! networking. See `docs/multi-user.md` for the design and the lifecycle table.
//!
//! The [`SessionHost`] owns today's [`App`] (the book, its protection and keys, the network process,
//! templates) and keeps each connected person's per-session state parked ([`state::SessionState`]).
//! A request swaps that person's state into the `App`, runs on exactly the desktop's code paths, and
//! swaps it back out, so every authorization, accounting and audit rule is the one the desktop uses
//! and the actor is always the session's verified user. Requests never overlap: [`SharedHost`] admits
//! one at a time through a bounded FIFO queue, so accounting writes stay serialized on the one
//! connection exactly as on the desktop.
//!
//! The standalone desktop does not use this module; fb-node keeps its one `App`.
mod queue;
pub(crate) mod state;

pub use queue::{Lease, QueueProgress, SharedHost};

use crate::commands::{Command, Outcome, Query};
use crate::identity::{Permission, Role, SIGN_IN};
use crate::session::{App, AppError};
use fb_core::{CompanyId, UserId};
use serde_json::Value;
use state::SessionState;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

/// An opaque session handle: 128 random bits as 32 lowercase hex characters. It names a session; it
/// grants nothing by itself (the transport authenticates the device, the session its user).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(String);

impl SessionId {
    fn random() -> SessionId {
        SessionId(crate::keys::to_hex(&crate::keys::random_bytes::<16>().expect("system random source")))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn parse(s: &str) -> Option<SessionId> {
        (s.len() == 32 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))).then(|| SessionId(s.to_owned()))
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Who is on the other end of a session, as the transport knows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientInfo {
    /// Shown in the list of sessions ("Dana's laptop"); one line of plain text, at most 100 characters.
    pub label: String,
    /// The transport's paired-device id; opaque here, kept for listing and later audit.
    pub device: Option<String>,
    /// True only for the host's own desktop window. Remote sessions cannot run maintenance unless the
    /// policy allows it.
    pub local: bool,
}

impl ClientInfo {
    pub fn local(label: &str) -> ClientInfo {
        ClientInfo { label: label.into(), device: None, local: true }
    }
    pub fn remote(label: &str, device: Option<&str>) -> ClientInfo {
        ClientInfo { label: label.into(), device: device.map(str::to_owned), local: false }
    }
    fn cleaned(mut self) -> ClientInfo {
        self.label = crate::session::Host::new(&self.label, "").version;
        self
    }
}

#[derive(Clone, Debug)]
pub struct Policy {
    /// A signed-in session with no request for this long is signed out (that session only).
    pub idle_sign_out: Duration,
    /// A signed-out session slot with no request for this long is removed.
    pub abandon_after: Duration,
    pub max_sessions: usize,
    /// Longest a request waits for its turn ([`SharedHost`]).
    pub queue_wait: Duration,
    /// Requests already waiting beyond which a new one is refused at once.
    pub queue_depth: usize,
    /// Completed form posts remembered per session, to answer an exact resubmit (retry after a
    /// dropped connection) with the first answer instead of running it again.
    pub replay_window: usize,
    /// Whether sessions that are not `local` may open book protection, recovery and restore surfaces
    /// and run backups. Tests only: the owner decided (2026-11-16) that maintenance runs only at the
    /// host computer, so every product configuration keeps the default `false`.
    pub remote_maintenance: bool,
}

impl Default for Policy {
    fn default() -> Policy {
        Policy {
            idle_sign_out: Duration::from_secs(15 * 60),
            abandon_after: Duration::from_secs(60 * 60),
            max_sessions: 32,
            queue_wait: Duration::from_secs(20),
            queue_depth: 64,
            replay_window: 32,
            remote_maintenance: false,
        }
    }
}

/// One request from a session's client.
#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)] // One request is built, moved once and dispatched; boxing buys nothing.
pub enum Request {
    /// A page (`screens::render`).
    Render { target: String },
    /// A renderer command: a form post or an export (the same allow-list as `App::run_renderer`),
    /// with the desktop's resubmit handling (`App::run_desktop`).
    Run { command: Command },
    Query { query: Query },
    /// This session's status, including notices; never touches the book.
    Status,
    /// The sign-in form's password step, without rendering the form first.
    SignIn { username: String, password: String },
    /// "Switch user / sign out" for this session.
    SignOut,
    /// The client locked (button, OS lock, suspend, client-side idle): signs this session out. It never
    /// closes the book; that is [`SessionHost::close_book`].
    Lock,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Response {
    Page { html: String, status: u16, route: String },
    /// `replayed`: an exact repeat of a completed post, answered from the first run.
    Outcome { outcome: Outcome, replayed: bool },
    Value(Value),
    Status(SessionStatus),
    Done,
}

/// Why a session slot was removed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EndReason {
    /// The transport or an administrator ended it (disconnect, "log off", device revoked).
    Closed,
    /// The book was closed on the host.
    HostClosed,
    /// Signed out and unused for `Policy::abandon_after`.
    Abandoned,
    /// Ended to recover from an internal failure during its request.
    Evicted,
}

/// Why a session was signed out (its slot stays for a new sign-in).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignOutReason {
    Requested,
    Locked,
    Idle,
    /// An administrator changed this user (disabled, role, password, memberships).
    AccountChanged,
    /// The book was restored, replaced, reopened or its protection changed.
    Maintenance(String),
}

impl SignOutReason {
    fn notice(&self) -> Option<String> {
        Some(match self {
            SignOutReason::Requested | SignOutReason::Locked => return None,
            SignOutReason::Idle => "You were signed out after a period of inactivity. Sign in again; reopen any unsaved work.".into(),
            SignOutReason::AccountChanged => "An administrator changed your user account, so you were signed out. Sign in again.".into(),
            SignOutReason::Maintenance(what) => format!("{what} Everyone was signed out. Sign in again; reopen any unsaved work."),
        })
    }
}

#[derive(Debug)]
pub enum HostError {
    UnknownSession,
    /// The session ended; reported once, then the id is unknown.
    Ended(EndReason),
    /// The request did not get its turn: `position` in the queue, what holds the books, how long it waited.
    Busy { position: usize, holder: Option<String>, waited: Duration },
    /// No book is open on the host.
    NotOpen,
    /// No user of this book has a password yet: the first owner sets one up at the host's desktop.
    NotEnrolled,
    TooManySessions,
    /// A maintenance surface this session may not use; the words say why.
    Maintenance(String),
    /// The request ran and the application refused or failed it, exactly as on the desktop.
    App(AppError),
}

impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HostError::UnknownSession => f.write_str("This session is not known. Sign in again."),
            HostError::Ended(EndReason::HostClosed) => f.write_str("The books were closed on the host computer."),
            HostError::Ended(_) => f.write_str("This session has ended. Sign in again."),
            HostError::Busy { holder: Some(h), .. } => write!(f, "The books are busy ({h}). Try again in a moment."),
            HostError::Busy { .. } => f.write_str("The books are busy. Try again in a moment."),
            HostError::NotOpen => f.write_str("The books are not open on the host computer."),
            HostError::NotEnrolled => f.write_str("Set up the owner's user password at the host computer first."),
            HostError::TooManySessions => f.write_str("Too many people are connected. Try again later."),
            HostError::Maintenance(m) => f.write_str(m),
            HostError::App(e) => write!(f, "{e}"),
        }
    }
}

impl From<AppError> for HostError {
    fn from(e: AppError) -> HostError {
        HostError::App(e)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SessionStatus {
    pub id: SessionId,
    pub client: ClientInfo,
    pub signed_in: bool,
    pub user: Option<(UserId, String)>,
    pub role: Option<Role>,
    pub generation: u64,
    /// The company whose pages this session shows (its frame).
    pub company: Option<CompanyId>,
    pub opened: SystemTime,
    pub last_active: SystemTime,
    pub notices: Vec<String>,
    /// The password was accepted and the second factor is still owed (not signed in yet). A
    /// transport should not count this as a failed sign-in.
    pub mfa_pending: bool,
}

/// A completed post, kept to answer an exact resend. Bound to the person and identity generation that
/// made it: any sign-out, sign-in or other identity change makes it unusable.
struct Replay {
    token: String,
    path: String,
    fields: [u8; 32],
    actor: UserId,
    generation: u64,
    /// What the route itself requires for this post (see [`replayable`]), rechecked before reuse.
    permission: Permission,
    outcome: Outcome,
}

struct Slot {
    state: SessionState,
    client: ClientInfo,
    transport_managed: bool,
    opened: SystemTime,
    last_active: Instant,
    last_active_wall: SystemTime,
    notices: Vec<String>,
    replays: VecDeque<Replay>,
    /// The end of this session's block of identity generations (see [`allocate_block`]).
    generation_limit: u64,
}

/// A committed write through a record's route: who, and at which host write sequence.
struct Write {
    session: SessionId,
    seq: u64,
    by: String,
}

/// What identifies "the same book" between requests.
#[derive(Clone, PartialEq, Eq)]
struct BookMark {
    connection: Option<u64>,
    protection: Option<String>,
}

const TOMBSTONES: usize = 256;
const WRITE_KEYS: usize = 4096;
const SIGNIN_FAILURES: u32 = 5;
const SIGNIN_DELAY: Duration = Duration::from_secs(60);
/// Each session's identity generations start in their own block of 2^40.
const GENERATION_BLOCK: u64 = 1 << 40;
/// Block 0 is the desktop `App`'s own; blocks run below 2^24 so no generation can reach `u64::MAX`.
const MAX_BLOCKS: u64 = (1 << 24) - 1;
/// A session this close to the end of its block is ended rather than allowed to cross into the next.
const GENERATION_MARGIN: u64 = 1 << 16;

/// Generation blocks are handed out once per process, never per host: stopping and starting hosting
/// must not give a new session generations an ended session already used.
static NEXT_BLOCK: AtomicU64 = AtomicU64::new(1);
/// The write sequence form tokens are stamped with, process-wide and monotonic across hosting
/// lifetimes, so a form kept through stop/start hosting is still compared against later writes.
static WRITE_SEQ: AtomicU64 = AtomicU64::new(0);

/// What a session or handoff is told once this process has used up its identities or write
/// stamps. Practically unreachable; it is refused rather than reused.
const RESTART: &str = "FreedomBooks must be restarted before anyone else can sign in here. Quit and reopen FreedomBooks on the host computer.";

#[cfg(test)]
thread_local! {
    /// Tests only: behave as if the process-wide allocators were at their ceilings (this thread).
    static AT_CEILING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn at_ceiling() -> bool {
    #[cfg(test)]
    if AT_CEILING.with(|c| c.get()) {
        return true;
    }
    false
}

/// The first generation of a fresh block, or None when the process has used every block.
fn allocate_block() -> Option<u64> {
    if at_ceiling() {
        return None;
    }
    NEXT_BLOCK
        .try_update(Ordering::SeqCst, Ordering::SeqCst, |b| (b < MAX_BLOCKS).then_some(b + 1))
        .ok()
        .map(|b| b * GENERATION_BLOCK)
}

/// True once the write sequence cannot advance: record forms are then refused, not compared.
fn write_seq_exhausted() -> bool {
    at_ceiling() || WRITE_SEQ.load(Ordering::SeqCst) == u64::MAX
}

/// The next write stamp. At the (unreachable) ceiling it stays at `u64::MAX`, which every older
/// form sees as newer; [`write_seq_exhausted`] refuses record forms from then on.
fn next_write_seq() -> u64 {
    WRITE_SEQ.try_update(Ordering::SeqCst, Ordering::SeqCst, |s| s.checked_add(1)).map_or(u64::MAX, |s| s + 1)
}

/// Company routes whose posts may be answered again from their first run: ordinary accounting
/// record forms only. A route is cached only if it is named here; every other route (identity,
/// users and roles, MFA, desktop settings, book protection and recovery, backups and restore, the
/// assistant and its keys, company settings, payment, email, bank connections, attachments, setup,
/// reports) is never remembered, because its answer can carry secrets or depend on proofs the cache
/// cannot track. `replay_routes_are_classified` fails until a new company route is put in this list
/// or in `NEVER_REPLAYED`.
const REPLAYED: &[&str] = &[
    "account_edit", "account_new", "accounts", "bills:calendar", "bills:detail", "bills:edit",
    "bills:list", "bills:new", "bills:post", "bills:review", "bills:revision", "bills:void",
    "checks:detail", "checks:edit", "checks:list", "checks:new", "checks:number", "checks:record",
    "checks:review", "checks:revision", "checks:void", "credits:apply", "credits:detail",
    "credits:list", "credits:new", "credits:refund", "credits:refund_detail",
    "credits:refund_void", "credits:unapply", "credits:void", "deposits:detail", "deposits:list",
    "deposits:new", "deposits:reverse", "directory:detail", "directory:edit", "directory:list",
    "directory:new", "directory:status", "invoices:convert", "invoices:detail", "invoices:edit",
    "invoices:list", "invoices:new", "invoices:post", "invoices:print", "invoices:print_revision",
    "invoices:review", "invoices:revision", "invoices:void", "jobs:detail", "jobs:edit",
    "jobs:invoice", "jobs:list", "jobs:new", "journal", "journal_edit",
    "journal_new", "journal_post", "journal_reverse", "payments:detail", "payments:list",
    "payments:new", "payments:reverse", "prepayments:apply", "prepayments:detail",
    "prepayments:invoice_apply", "prepayments:list", "prepayments:new", "prepayments:print",
    "prepayments:refund", "prepayments:refund_detail", "prepayments:refund_void",
    "prepayments:unapply", "prepayments:void", "receipts:detail", "receipts:list", "receipts:new",
    "receipts:reverse", "reconcile:begin", "reconcile:history", "reconcile:list",
    "reconcile:report", "reconcile:screen", "reconcile:undo", "recurring:detail",
    "recurring:edit", "recurring:list", "recurring:new", "register", "registers:cleared",
    "registers:list", "sales:detail", "sales:edit", "sales:list", "sales:new", "sales:print",
    "sales:print_revision", "sales:revision", "sales:state", "sales_receipts:detail",
    "sales_receipts:list", "sales_receipts:new", "sales_receipts:print", "sales_receipts:void",
    "salestax:bulk_step", "salestax:group_edit", "salestax:group_new", "salestax:list",
    "salestax:rate_edit", "salestax:rate_new", "salestax:record_payment", "salestax:status",
    "salestax:tax_payment", "salestax:void_payment", "salestax:what_you_owe", "time:edit",
    "time:list", "time:new", "transfers:detail",
    "transfers:list", "transfers:new", "transfers:void",
];

/// Company routes deliberately never answered from the replay cache (see [`REPLAYED`]). Only the
/// classification test reads it: anything not in `REPLAYED` is already refused.
#[cfg(test)]
const NEVER_REPLAYED: &[&str] = &[
    "assistant:disconnect", "assistant:key", "assistant:model", "assistant:models_refresh",
    "assistant:notice", "assistant:page", "assistant:read_attachment",
    "assistant:read_attachments", "assistant:read_upload", "attachments:add_document",
    "attachments:add_journal", "attachments:download", "attachments:remove", "backups:create",
    "backups:download", "backups:page", "balance_sheet", "bankfeeds:accounts", "bankfeeds:connect",
    "bankfeeds:forget", "bankfeeds:list", "bankfeeds:review", "bankfeeds:rule_edit",
    "bankfeeds:rule_new", "bankfeeds:rules", "bankfeeds:sync", "company", "company_edit",
    "company_home", "documents:setup", "expenses", "find", "general_ledger", "history",
    "invoices:email_preview", "profit_loss", "reminders:page", "reminders:settings", "reports",
    "salestax:settings", "salestax:setup", "settings", "setup_done", "setup_fill", "setup_start",
    "stripe:payments", "trial_balance",
];

/// Which posts may be answered again from their first run, and the permission their route
/// requires. Deny by default: only routes named in [`REPLAYED`] qualify.
fn replayable(path: &str, fields: &[(String, String)]) -> Option<Permission> {
    if is_maintenance(path) {
        return None;
    }
    let route = crate::screens::resolve(path)?;
    route.args.get("company_id")?;
    if !REPLAYED.contains(&route.name.as_str()) {
        return None;
    }
    let action = field(fields, "action").unwrap_or("");
    if action.starts_with("stripe_") || action.starts_with("gmail_") {
        return None;
    }
    // As `screens::submit` authorizes the post itself.
    Some(if action == "desktop_print" { Permission::Read } else { Permission::Write })
}

/// Retire the record forms a person kept through a hosting change: writes recorded by the host
/// they were opened under are gone, so they could not be checked for newer edits.
fn retire_record_forms(state: &mut SessionState) {
    state.tokens.retire_where(|path| record_key(path).is_some());
}

/// The end of the block `generation` lies in.
fn generation_limit(generation: u64) -> u64 {
    (generation / GENERATION_BLOCK).saturating_add(1).saturating_mul(GENERATION_BLOCK)
}

pub struct SessionHost {
    app: App,
    policy: Policy,
    slots: HashMap<SessionId, Slot>,
    tombstones: VecDeque<(SessionId, EndReason)>,
    writes: HashMap<String, Vec<Write>>,
    mark: BookMark,
    /// Sign-in failures by user name, across every session: (count, refused until).
    limiter: HashMap<String, (u32, Option<Instant>)>,
    /// Second-factor failures by verified user, across every session: (count, refused until).
    factor_limiter: HashMap<UserId, (u32, Option<Instant>)>,
}

/// Paths that change or reveal the book's protection, or replace the book: maintenance.
const MAINTENANCE_PATHS: [&str; 5] = [
    crate::protection::SETTINGS,
    crate::protection::RECOVERY,
    crate::protection::KIT,
    "/settings/restore-backup/",
    "/settings/restored-book/",
];

fn is_maintenance(target: &str) -> bool {
    let path = target.split('?').next().unwrap_or(target);
    MAINTENANCE_PATHS.contains(&path) || crate::screens::resolve(path).is_some_and(|r| r.name.starts_with("backups:"))
}

/// What a route argument names, for stale-edit detection. Every argument in the route table must be
/// one of these (`route_arguments_are_classified` walks the table).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Argument {
    /// The company the page belongs to.
    Company,
    /// An existing record's id (`invoice_id`, `record_id`, ...): its name says what kind of record.
    Record,
    /// Narrows what kind of record a generic route handles (`lists/<kind>/`); part of the key.
    Kind,
    /// Not a record: a revision or sequence number, a file name.
    Other,
}

fn argument(name: &str) -> Option<Argument> {
    Some(match name {
        "company_id" => Argument::Company,
        "kind" => Argument::Kind,
        "number" | "filename" => Argument::Other,
        n if n.ends_with("_id") => Argument::Record,
        _ => return None,
    })
}

/// The existing record a form path writes: its company, the kind of record and its id, for example
/// `<company>|customers|record_id=<id>`. Detail, edit and status routes of one record share the key.
/// None for pages that create a record or are company-level or global, which keep relying on the
/// core's own revision checks.
pub(crate) fn record_key(path: &str) -> Option<String> {
    let route = crate::screens::resolve(path.split('?').next().unwrap_or(path))?;
    let company = route.args.get("company_id")?;
    let mut ids: Vec<String> = vec![];
    let mut kind = "";
    for (name, value) in &route.args {
        match argument(name) {
            Some(Argument::Record) => ids.push(format!("{name}={value}")),
            Some(Argument::Kind) => kind = value,
            Some(Argument::Company | Argument::Other) => {}
            // An unclassified argument might be a record: treat it as one rather than miss a conflict.
            None => ids.push(format!("{name}={value}")),
        }
    }
    if ids.is_empty() {
        return None;
    }
    Some(format!("{company}|{kind}|{}", ids.join("|")))
}

fn digest(fields: &[(String, String)]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    for (k, v) in fields {
        h.update((k.len() as u64).to_le_bytes());
        h.update(k.as_bytes());
        h.update((v.len() as u64).to_le_bytes());
        h.update(v.as_bytes());
    }
    h.finalize().into()
}

fn field<'a>(fields: &'a [(String, String)], key: &str) -> Option<&'a str> {
    fields.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

fn is_form(request: &Request, path: &str, action: &str) -> bool {
    matches!(request, Request::Run { command: Command::SubmitForm { path: p, fields, .. } } if p == path && field(fields, "action") == Some(action))
}

/// The user name of a sign-in attempt in this request, if it is one.
fn signin_name(request: &Request) -> Option<String> {
    match request {
        Request::SignIn { username, .. } => Some(username.clone()),
        Request::Run { command: Command::SubmitForm { path, fields, .. } } if path == SIGN_IN && field(fields, "action") == Some("signin") => {
            Some(field(fields, "username").unwrap_or("").to_owned())
        }
        _ => None,
    }
}

impl SessionHost {
    /// Host `app`. Whatever session state it held becomes the vacant state between requests and is
    /// signed out now: nobody is signed in on the host itself.
    pub fn new(mut app: App, policy: Policy) -> SessionHost {
        app.sign_out();
        app.first_run_owner = false;
        app.roster_changed.clear();
        let mark = Self::mark_of(&app);
        SessionHost::hosting(app, policy, mark)
    }

    fn hosting(app: App, policy: Policy, mark: BookMark) -> SessionHost {
        SessionHost {
            app,
            policy,
            slots: HashMap::new(),
            tombstones: VecDeque::new(),
            writes: HashMap::new(),
            mark,
            limiter: HashMap::new(),
            factor_limiter: HashMap::new(),
        }
    }

    /// Start hosting a desktop that is already in use ("Host these books"): the person signed in at
    /// this desktop becomes the `local` session, keeping their sign-in, open forms and reviews. Others
    /// join with [`SessionHost::open_session`]. [`SessionHost::into_app`] reverses it.
    pub fn host_with_local(mut app: App, policy: Policy, label: &str) -> (SessionHost, SessionId) {
        let mut state = SessionState::default();
        state::swap_out(&mut app, &mut state);
        retire_record_forms(&mut state);
        // The vacant state now in the app was never signed in; give it nothing of the person's.
        app.first_run_owner = false;
        app.roster_changed.clear();
        let mark = Self::mark_of(&app);
        let mut host = SessionHost::hosting(app, policy, mark);
        let id = SessionId::random();
        let now = Instant::now();
        // The local person keeps their generation (and the capabilities issued under it).
        let generation_limit = generation_limit(state.identity_generation());
        host.slots.insert(
            id.clone(),
            Slot {
                state,
                client: ClientInfo::local(label).cleaned(),
                transport_managed: false,
                opened: SystemTime::now(),
                last_active: now,
                last_active_wall: SystemTime::now(),
                notices: vec![],
                replays: VecDeque::new(),
                generation_limit,
            },
        );
        (host, id)
    }

    /// Stop hosting: every session except `local` ends (`Closed`), and the app comes back with the
    /// local person's state in it, as the standalone desktop uses it. Unknown `local`: the app comes
    /// back signed out.
    pub fn into_app(mut self, local: &SessionId) -> App {
        self.settle_book_changes();
        let others: Vec<SessionId> = self.slots.keys().filter(|id| *id != local).cloned().collect();
        for id in others {
            self.end_session(&id, EndReason::Closed);
        }
        if let Some(mut slot) = self.slots.remove(local) {
            // This host's record of other people's writes ends here.
            retire_record_forms(&mut slot.state);
            state::swap_in(&mut self.app, &mut slot.state);
        } else if let Some(generation) = allocate_block() {
            // Nobody to hand back: the desktop starts signed out in generations nobody has used.
            state::swap_in(&mut self.app, &mut SessionState::starting_at(generation));
        } else {
            // No unused generations are left: the desktop comes back signed out and refuses every
            // sign-in until the app is restarted, rather than reuse generations.
            state::swap_in(&mut self.app, &mut SessionState::restart_required(RESTART));
        }
        self.app.tokens.stamp = 0;
        self.app
    }

    /// An administrator ends another person's session ("log off"). The target hears `Closed`.
    pub fn log_off(&mut self, by: &SessionId, target: &SessionId) -> Result<(), HostError> {
        self.with_session(by, |app| app.authorize(None, crate::identity::Permission::Administer))??;
        self.lookup(target)?;
        self.end_session(target, EndReason::Closed);
        Ok(())
    }

    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    /// The shared app, with no session in it (book path, open state, configuration).
    pub fn app(&self) -> &App {
        &self.app
    }

    /// Host-level setup with no session swapped in: configure backups and keys, open the book. A book
    /// opened, closed or replaced here signs every session out (or ends them, if it is now closed)
    /// when the next request arrives. Never sign anyone in through this.
    pub fn host_app(&mut self) -> &mut App {
        &mut self.app
    }

    fn mark_of(app: &App) -> BookMark {
        BookMark {
            connection: app.open.as_ref().map(|o| o.book.connection_id()),
            protection: app.protection.as_ref().and_then(|r| serde_json::to_string(&r.keys).ok()),
        }
    }

    fn enrolled(&mut self) -> Result<bool, HostError> {
        let open = self.app.open.as_mut().ok_or(HostError::NotOpen)?;
        let n: i64 = open
            .book
            .read(|q| q.query_row("SELECT count(*) FROM desktop_user d JOIN fb_user u ON u.id=d.user_id WHERE d.password_hash IS NOT NULL AND u.is_active=1", [], |r| r.get(0)))
            .map_err(AppError::from)?;
        Ok(n > 0)
    }

    /// A new, signed-out session for `client`.
    pub fn open_session(&mut self, client: ClientInfo) -> Result<SessionId, HostError> {
        self.open_session_at(client, Instant::now())
    }

    pub fn open_session_at(&mut self, client: ClientInfo, now: Instant) -> Result<SessionId, HostError> {
        self.open_slot(client, now, false)
    }

    pub fn open_transport_session(&mut self, client: ClientInfo) -> Result<SessionId, HostError> {
        self.open_slot(client, Instant::now(), true)
    }

    fn open_slot(&mut self, client: ClientInfo, now: Instant, transport_managed: bool) -> Result<SessionId, HostError> {
        self.settle_book_changes();
        if !self.app.is_open() {
            return Err(HostError::NotOpen);
        }
        if !self.enrolled()? {
            return Err(HostError::NotEnrolled);
        }
        self.sweep(now);
        if self.slots.len() >= self.policy.max_sessions {
            return Err(HostError::TooManySessions);
        }
        // Practically unreachable (2^24 sessions in one process); refused rather than reused.
        let generation = allocate_block().ok_or_else(|| HostError::Maintenance(RESTART.into()))?;
        let id = SessionId::random();
        self.slots.insert(
            id.clone(),
            Slot {
                state: SessionState::starting_at(generation),
                client: client.cleaned(),
                transport_managed,
                opened: SystemTime::now(),
                last_active: now,
                last_active_wall: SystemTime::now(),
                notices: vec![],
                replays: VecDeque::new(),
                generation_limit: generation_limit(generation),
            },
        );
        Ok(id)
    }

    fn lookup(&mut self, id: &SessionId) -> Result<(), HostError> {
        if self.slots.contains_key(id) {
            return Ok(());
        }
        if let Some(i) = self.tombstones.iter().position(|(t, _)| t == id) {
            let (_, reason) = self.tombstones.remove(i).expect("position");
            return Err(HostError::Ended(reason));
        }
        Err(HostError::UnknownSession)
    }

    fn bury(&mut self, id: SessionId, reason: EndReason) {
        if self.tombstones.len() >= TOMBSTONES {
            self.tombstones.pop_front();
        }
        self.tombstones.push_back((id, reason));
    }

    /// Run `f` with session `id` swapped into the app. Panics inside `f` are caught: the session is
    /// signed out, and the book is closed for everyone if its connection was left mid-transaction.
    fn enter<R>(&mut self, id: &SessionId, f: impl FnOnce(&mut App) -> R) -> Result<R, HostError> {
        let mut slot = self.slots.remove(id).ok_or(HostError::UnknownSession)?;
        let generation = slot.state.identity_generation();
        state::swap_in(&mut self.app, &mut slot.state);
        self.app.tokens.stamp = WRITE_SEQ.load(Ordering::SeqCst);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&mut self.app)));
        let failed = result.is_err();
        // Near the end of its generation block the session is ended here, in this one request
        // (signed out while swapped in, then not put back), never by re-entering it.
        let exhausted = self.app.session_generation() >= slot.generation_limit.saturating_sub(GENERATION_MARGIN);
        if failed || exhausted {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.app.sign_out()));
        }
        state::swap_out(&mut self.app, &mut slot.state);
        let roster: Vec<UserId> = std::mem::take(&mut self.app.roster_changed);
        // Any identity change (form sign-out, sign-in, MFA cancel, own roster change, panic) voids the
        // session's remembered answers: they were made for someone else's authority.
        if slot.state.identity_generation() != generation {
            slot.replays.clear();
        }
        if exhausted {
            drop(slot);
            self.bury(id.clone(), EndReason::Evicted);
        } else {
            self.slots.insert(id.clone(), slot);
        }
        for user in roster {
            self.sign_out_where(|other, s| other != id && s.actor() == Some(user), SignOutReason::AccountChanged);
        }
        self.drop_orphan_signin_jobs();
        match result {
            Ok(r) => Ok(r),
            Err(_) => {
                let idle = self.app.open.as_ref().map(|o| o.book.idle_after_unwind().is_ok());
                if idle == Some(false) {
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.app.lock()));
                    if self.app.is_open() {
                        self.app.open = None;
                    }
                }
                Err(HostError::App(AppError::Invalid(
                    "FreedomBooks couldn't finish this and signed you out. Sign in again and check whether it was saved.".into(),
                )))
            }
        }
    }

    /// Browser sign-in jobs whose owning generation no session holds any more (a sign-out that could
    /// not finish) are cancelled, so nobody is locked out of connecting that company.
    fn drop_orphan_signin_jobs(&mut self) {
        if self.app.signin_jobs.is_empty() {
            return;
        }
        let live: Vec<u64> = self.slots.values().map(|s| s.state.identity_generation()).collect();
        let orphans: Vec<String> = self.app.signin_jobs.iter().filter(|(_, (g, _))| !live.contains(g)).map(|(k, _)| k.clone()).collect();
        for key in orphans {
            self.app.signin_jobs.remove(&key);
            if let (Some((provider, company)), Some(net)) = (key.split_once(':'), self.app.net.as_mut()) {
                let _ = net.call(&serde_json::json!({"op": format!("{provider}_cancel"), "company": company}));
            }
        }
    }

    /// Sign out every session matching `which`, with `reason`'s notice.
    fn sign_out_where(&mut self, which: impl Fn(&SessionId, &SessionState) -> bool, reason: SignOutReason) {
        let ids: Vec<SessionId> = self.slots.iter().filter(|(id, s)| which(id, &s.state)).map(|(id, _)| id.clone()).collect();
        for id in ids {
            self.sign_out_session(&id, reason.clone());
        }
    }

    fn sign_out_session(&mut self, id: &SessionId, reason: SignOutReason) {
        let was = self.slots.get(id).is_some_and(|s| s.state.signed_in() || s.state.identity.mfa_pending());
        let _ = self.enter(id, |app| app.sign_out());
        if let Some(slot) = self.slots.get_mut(id) {
            slot.replays.clear();
            if was && let Some(notice) = reason.notice() {
                slot.notices.push(notice);
            }
        }
    }

    /// Remove a session (transport disconnect, administrator "log off", device revoked). Anything it
    /// had begun (forms, reviews, dialogs, sign-in callbacks) is cancelled first.
    pub fn end_session(&mut self, id: &SessionId, reason: EndReason) {
        if !self.slots.contains_key(id) {
            return;
        }
        let _ = self.enter(id, |app| app.sign_out());
        // `enter` may already have ended it (generation block used up): buried once, as Evicted.
        if self.slots.remove(id).is_some() {
            self.bury(id.clone(), reason);
        }
    }

    fn end_all(&mut self, reason: EndReason) {
        let ids: Vec<SessionId> = self.slots.keys().cloned().collect();
        for id in ids {
            self.end_session(&id, reason.clone());
        }
    }

    /// Host lock / stop hosting: end every session and close the book (its key leaves memory).
    pub fn close_book(&mut self) {
        self.end_all(EndReason::HostClosed);
        self.app.lock();
        self.writes.clear();
        self.mark = Self::mark_of(&self.app);
    }

    /// Apply a book change made outside a session's request (`host_app`): sessions cannot keep forms,
    /// reviews or sign-ins from a different book or key.
    fn settle_book_changes(&mut self) {
        let now = Self::mark_of(&self.app);
        if now == self.mark {
            return;
        }
        self.after_book_change(None, &now);
    }

    /// The book changed during a request by `by` (or on the host): close out every other session.
    fn after_book_change(&mut self, by: Option<&SessionId>, now: &BookMark) {
        let previous = std::mem::replace(&mut self.mark, now.clone());
        self.writes.clear();
        if now.connection.is_none() {
            let others: Vec<SessionId> = self.slots.keys().filter(|id| Some(*id) != by).cloned().collect();
            for id in others {
                self.end_session(&id, EndReason::HostClosed);
            }
            return;
        }
        let what = if now.connection != previous.connection {
            "The books were reopened or replaced on the host (for example, a backup was restored)."
        } else {
            "The book's password or key protection was changed."
        };
        self.sign_out_where(|id, _| Some(id) != by, SignOutReason::Maintenance(what.into()));
        // First-owner setup is for the host computer only: remote sessions cannot stay on books
        // nobody has enrolled in.
        if !self.enrolled().unwrap_or(false) {
            let remote: Vec<SessionId> = self.slots.iter().filter(|(_, s)| !s.client.local).map(|(id, _)| id.clone()).collect();
            for id in remote {
                self.end_session(&id, EndReason::Closed);
            }
        }
    }

    /// Sign out sessions idle for `Policy::idle_sign_out` and remove signed-out ones unused for
    /// `Policy::abandon_after`. Returns the sessions it signed out or removed. Other sessions and the
    /// book are untouched.
    pub fn expire_idle(&mut self, now: Instant) -> Vec<SessionId> {
        self.settle_book_changes();
        self.sweep(now)
    }

    fn sweep(&mut self, now: Instant) -> Vec<SessionId> {
        let idle: Vec<SessionId> = self
            .slots
            .iter()
            .filter(|(_, s)| (s.state.signed_in() || s.state.identity.mfa_pending()) && now.saturating_duration_since(s.last_active) >= self.policy.idle_sign_out)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &idle {
            self.sign_out_session(id, SignOutReason::Idle);
        }
        let abandoned: Vec<SessionId> = self
            .slots
            .iter()
            .filter(|(id, s)| !s.transport_managed && !idle.contains(id) && !s.state.signed_in() && now.saturating_duration_since(s.last_active) >= self.policy.abandon_after)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &abandoned {
            self.end_session(id, EndReason::Abandoned);
        }
        idle.into_iter().chain(abandoned).collect()
    }

    /// Every session, for a "who's connected" list. Notices are left for their own session.
    pub fn sessions(&mut self) -> Vec<SessionStatus> {
        let mut ids: Vec<SessionId> = self.slots.keys().cloned().collect();
        ids.sort();
        ids.iter().filter_map(|id| self.status(id, false)).collect()
    }

    fn status(&mut self, id: &SessionId, drain: bool) -> Option<SessionStatus> {
        let actor = self.slots.get(id)?.state.actor();
        let role = match (actor, self.app.open.as_mut()) {
            (Some(actor), Some(open)) => open
                .book
                .read(|q| q.query_row("SELECT role FROM desktop_user WHERE user_id=?1", [actor.to_db()], |r| r.get::<_, String>(0)))
                .ok()
                .and_then(|r| Role::parse(&r).ok()),
            _ => None,
        };
        let slot = self.slots.get_mut(id)?;
        let notices = if drain { std::mem::take(&mut slot.notices) } else { slot.notices.clone() };
        Some(SessionStatus {
            id: id.clone(),
            client: slot.client.clone(),
            signed_in: slot.state.signed_in(),
            user: actor.zip(slot.state.name().map(str::to_owned)),
            role,
            generation: slot.state.identity_generation(),
            company: slot.state.frame_company,
            opened: slot.opened,
            last_active: slot.last_active_wall,
            notices,
            mfa_pending: !slot.state.signed_in() && slot.state.identity.mfa_pending(),
        })
    }

    /// A short description of what a request of this session is doing, for others waiting in the queue.
    pub fn describe(&self, id: &SessionId, request: &Request) -> String {
        let who = self.slots.get(id).and_then(|s| s.state.name()).unwrap_or("Someone").to_owned();
        let what = match request {
            Request::Render { .. } | Request::Query { .. } | Request::Status => "reading",
            Request::Run { command: Command::SubmitForm { path, .. } } if is_maintenance(path) => "maintenance",
            Request::Run { .. } => "saving",
            Request::SignIn { .. } | Request::SignOut | Request::Lock => "signing in or out",
        };
        format!("{who}: {what}")
    }

    /// Run a trusted in-process closure as session `id` (tests, adapters). It gets the desktop's
    /// authorization but not `dispatch`'s stale-edit, replay and maintenance checks: a transport must
    /// send everything a client asked for through [`SessionHost::dispatch`].
    pub fn with_session<R>(&mut self, id: &SessionId, f: impl FnOnce(&mut App) -> R) -> Result<R, HostError> {
        self.settle_book_changes();
        self.lookup(id)?;
        let before = self.mark.clone();
        let r = self.enter(id, f)?;
        let after = Self::mark_of(&self.app);
        if after != before {
            self.after_book_change(Some(id), &after);
        }
        Ok(r)
    }

    pub fn dispatch(&mut self, id: &SessionId, request: Request) -> Result<Response, HostError> {
        self.dispatch_at(id, request, Instant::now())
    }

    /// [`SessionHost::dispatch`] at a given instant (idle accounting; tests).
    pub fn dispatch_at(&mut self, id: &SessionId, request: Request, now: Instant) -> Result<Response, HostError> {
        self.settle_book_changes();
        self.lookup(id)?;
        if let Request::Status = request {
            return self.status(id, true).map(Response::Status).ok_or(HostError::UnknownSession);
        }
        if !self.app.is_open() {
            return Err(HostError::NotOpen);
        }
        // Idle is judged before this request counts as activity: a request after the idle time
        // finds the session already signed out.
        let idle = {
            let slot = self.slots.get(id).expect("looked up");
            (slot.state.signed_in() || slot.state.identity.mfa_pending()) && now.saturating_duration_since(slot.last_active) >= self.policy.idle_sign_out
        };
        if idle {
            self.sign_out_session(id, SignOutReason::Idle);
            // The sign-out may have used up the session's generations and ended it.
            self.lookup(id)?;
        }
        {
            let slot = self.slots.get_mut(id).expect("looked up");
            slot.last_active = now;
            slot.last_active_wall = SystemTime::now();
        }
        let local = self.slots[id].client.local;
        if !local {
            // Checked on every request, not only when the session opened: the host may have switched
            // to books nobody has enrolled in since.
            if !self.enrolled()? {
                self.end_session(id, EndReason::Closed);
                return Err(HostError::NotEnrolled);
            }
            if is_form(&request, SIGN_IN, "enroll") {
                return Err(HostError::Maintenance("Setting up the owner's sign-in is available only at the host computer.".into()));
            }
        }
        if !local && !self.policy.remote_maintenance {
            let target = match &request {
                Request::Render { target } => Some(target.as_str()),
                Request::Run { command: Command::SubmitForm { path, .. } } => Some(path.as_str()),
                Request::Run { command: Command::ExportBackup { .. } } => Some("/backups/"),
                _ => None,
            };
            if target.is_some_and(|t| t == "/backups/" || is_maintenance(t)) {
                return Err(HostError::Maintenance(
                    "Backups, restores and book protection are available only at the host computer.".into(),
                ));
            }
        }
        // Attempts from a session that is already signed in are refused by the app, not guesses.
        let signin = signin_name(&request).filter(|_| !self.slots[id].state.signed_in());
        if let Some(name) = &signin
            && self.limiter.get(name).and_then(|(_, until)| *until).is_some_and(|until| until > now)
        {
            return Err(HostError::App(AppError::Invalid(
                "Too many unsuccessful attempts for this user name. Wait one minute, then try again.".into(),
            )));
        }
        // A second-factor attempt is limited per verified user across every session, so ending a
        // session and starting another does not renew the guesses.
        let factor = self.second_factor(id, &request);
        if let Some(user) = factor
            && self.factor_limiter.get(&user).and_then(|(_, until)| *until).is_some_and(|until| until > now)
        {
            return Err(HostError::App(AppError::Invalid(
                "Too many unsuccessful codes for this user. Wait one minute, then try again.".into(),
            )));
        }
        let before = self.mark.clone();
        let result = self.run(id, request);
        if let Some(name) = signin {
            // An evicted slot (its request used up its generations) counts as a success: the
            // password was right, and the session is gone either way.
            let succeeded = self.slots.get(id).is_none_or(|slot| slot.state.signed_in() || slot.state.identity.mfa_pending());
            self.note_signin(name, succeeded, now);
        }
        if let Some(user) = factor {
            let succeeded = self.slots.get(id).is_some_and(|s| s.state.actor() == Some(user));
            note_failure(&mut self.factor_limiter, user, succeeded, now);
        }
        let after = Self::mark_of(&self.app);
        if after != before {
            self.after_book_change(Some(id), &after);
        }
        result
    }

    fn note_signin(&mut self, name: String, succeeded: bool, now: Instant) {
        note_failure(&mut self.limiter, name, succeeded, now);
    }

    /// The user whose second factor this request tries, if it is a code for a live password step.
    fn second_factor(&self, id: &SessionId, request: &Request) -> Option<UserId> {
        if !is_form(request, SIGN_IN, "mfa") {
            return None;
        }
        let pending = self.slots.get(id)?.state.identity.mfa.pending.as_ref()?;
        (pending.expires > Instant::now()).then_some(pending.actor)
    }

    fn run(&mut self, id: &SessionId, request: Request) -> Result<Response, HostError> {
        match request {
            Request::Status => unreachable!("answered before the book is touched"),
            Request::Render { target } => {
                if target.len() > 4096 || !target.starts_with('/') {
                    return Err(HostError::App(AppError::Invalid("Invalid screen address.".into())));
                }
                let page = self.enter(id, |app| crate::screens::render(app, &target))??;
                Ok(Response::Page { html: page.html, status: page.status, route: page.route })
            }
            Request::Query { query } => Ok(Response::Value(self.enter(id, |app| app.query(query))??)),
            Request::SignIn { username, password } => {
                self.enter(id, |app| app.sign_in(&username, &password))??;
                Ok(Response::Done)
            }
            Request::SignOut => {
                self.sign_out_session(id, SignOutReason::Requested);
                Ok(Response::Done)
            }
            Request::Lock => {
                self.sign_out_session(id, SignOutReason::Locked);
                Ok(Response::Done)
            }
            Request::Run { command } => self.run_command(id, command),
        }
    }

    fn run_command(&mut self, id: &SessionId, command: Command) -> Result<Response, HostError> {
        let Command::SubmitForm { path, token, fields } = &command else {
            let outcome = self.enter(id, |app| app.run_desktop(command))??;
            return Ok(Response::Outcome { outcome, replayed: false });
        };
        let (path, token, digest) = (path.clone(), token.clone(), digest(fields));
        // An exact repeat of a completed post (the client retried after losing the answer): the
        // first answer, not a second run and not "expired".
        if let Some(outcome) = self.replay(id, &token, &path, &digest) {
            return Ok(Response::Outcome { outcome, replayed: true });
        }
        let Some(slot) = self.slots.get(id) else { return Err(HostError::UnknownSession) };
        let key = record_key(&path);
        if key.is_some() && write_seq_exhausted() {
            return Err(HostError::Maintenance(RESTART.into()));
        }
        let permission = replayable(&path, fields);
        if let Some(key) = &key
            && let Some(stamp) = slot.state.tokens.stamp_of(&token, &path)
            && let Some(w) = self.writes.get(key).and_then(|ws| ws.iter().filter(|w| &w.session != id && w.seq > stamp).max_by_key(|w| w.seq))
        {
            let message = format!(
                "{} changed this record after you opened it, so nothing was saved. Open it again to see the change, then make yours.",
                w.by
            );
            let _ = self.enter(id, |app| app.tokens.consume(&token, &path));
            return Ok(Response::Outcome { outcome: Outcome::Retry { message, token: None }, replayed: false });
        }
        let counter = |app: &App| app.open.as_ref().map(|o| (o.book.connection_id(), o.book.commits()));
        let before = counter(&self.app);
        let made_by = self.slots.get(id).map(|s| (s.state.actor(), s.state.identity_generation()));
        let (outcome, by) = self.enter(id, |app| {
            let outcome = app.run_desktop(command);
            let by = app.open.as_ref().map(|o| o.owner_name.clone()).unwrap_or_default();
            (outcome, by)
        })?;
        let after = counter(&self.app);
        if let (Some(key), Some((c0, n0)), Some((c1, n1))) = (key, before, after)
            && c0 == c1
            && n1 > n0
        {
            self.note_write(id, key, by);
        }
        let outcome = outcome?;
        let window = self.policy.replay_window;
        // Remembered only for a signed-in person whose identity the post did not change: an answer
        // to a sign-in, sign-out or roster change is never handed out again.
        // Only ordinary accounting posts (`replayable`), and never an answer that sends the client
        // elsewhere (a provider sign-in address is single-use).
        if let Some(slot) = self.slots.get_mut(id)
            && window > 0
            && let Some(permission) = permission
            && !matches!(outcome, Outcome::External { .. })
            && let Some((Some(actor), generation)) = made_by
            && slot.state.actor() == Some(actor)
            && slot.state.identity_generation() == generation
        {
            if slot.replays.len() >= window {
                slot.replays.pop_front();
            }
            slot.replays.push_back(Replay { token, path, fields: digest, actor, generation, permission, outcome: outcome.clone() });
        }
        Ok(Response::Outcome { outcome, replayed: false })
    }

    /// The remembered answer to an exact resend of a completed post, if this session's person and
    /// identity generation are still the ones that made it and they may still see that page.
    fn replay(&mut self, id: &SessionId, token: &str, path: &str, digest: &[u8; 32]) -> Option<Outcome> {
        let slot = self.slots.get(id)?;
        if slot.state.tokens.valid(token, path) {
            return None;
        }
        let r = slot.replays.iter().find(|r| r.token == token && r.path == path && &r.fields == digest)?;
        let current = (slot.state.actor(), slot.state.identity_generation());
        if current != (Some(r.actor), r.generation) {
            self.slots.get_mut(id)?.replays.clear();
            return None;
        }
        let (outcome, permission) = (r.outcome.clone(), r.permission);
        // The route's own authorization (role, company membership, active account) is rechecked
        // against the book now, exactly as the original post was authorized.
        let allowed = self.enter(id, |app| app.authorize_path(path, permission).is_ok()).unwrap_or(false);
        if !allowed {
            if let Some(slot) = self.slots.get_mut(id) {
                slot.replays.clear();
            }
            return None;
        }
        Some(outcome)
    }

    fn note_write(&mut self, id: &SessionId, key: String, by: String) {
        let seq = next_write_seq();
        let writes = self.writes.entry(key).or_default();
        writes.retain(|w| &w.session != id);
        writes.push(Write { session: id.clone(), seq, by });
        if self.writes.len() > WRITE_KEYS {
            // Writes older than every live form's page cannot make any of them stale.
            let floor = self.slots.values().filter_map(|s| s.state.tokens.oldest_stamp()).min().unwrap_or(seq);
            self.writes.retain(|_, ws| {
                ws.retain(|w| w.seq > floor);
                !ws.is_empty()
            });
        }
    }
}

/// Count a failure (or clear the record on success) in a limiter: 5 failures, then 60 seconds.
fn note_failure<K: std::hash::Hash + Eq>(limiter: &mut HashMap<K, (u32, Option<Instant>)>, key: K, succeeded: bool, now: Instant) {
    if succeeded {
        limiter.remove(&key);
        return;
    }
    if limiter.len() > 4096 {
        limiter.retain(|_, (_, until)| until.is_some_and(|u| u > now));
    }
    let entry = limiter.entry(key).or_insert((0, None));
    entry.0 += 1;
    if entry.0 >= SIGNIN_FAILURES {
        *entry = (0, Some(now + SIGNIN_DELAY));
    }
}

impl SessionState {
    fn identity_generation(&self) -> u64 {
        self.identity.generation()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every argument of every route is classified for stale-edit detection: a new kind of argument
    /// fails here until `argument` says whether it names a record.
    #[test]
    fn route_arguments_are_classified() {
        let mut unclassified = vec![];
        for (name, pattern) in fb_templates::django::route_table() {
            for part in pattern.split('/') {
                if let Some(inner) = part.strip_prefix('<').and_then(|p| p.strip_suffix('>')) {
                    let arg = inner.split_once(':').map_or(inner, |(_, a)| a);
                    if argument(arg).is_none() {
                        unclassified.push(format!("{name}: {arg}"));
                    }
                }
            }
        }
        assert!(unclassified.is_empty(), "classify these route arguments in multiuser::argument: {unclassified:?}");
    }

    /// Every company route says whether its posts may be replayed: a new route fails here until it
    /// is put in `REPLAYED` or `NEVER_REPLAYED`, so a sensitive page cannot be cached by default.
    #[test]
    fn replay_routes_are_classified() {
        let mut unclassified = vec![];
        let mut both = vec![];
        for (name, pattern) in fb_templates::django::route_table() {
            if !pattern.contains("company_id") {
                continue;
            }
            let (yes, no) = (REPLAYED.contains(&name.as_str()), NEVER_REPLAYED.contains(&name.as_str()));
            if yes && no {
                both.push(name.clone());
            } else if !yes && !no {
                unclassified.push(name.clone());
            }
        }
        assert!(both.is_empty(), "routes in both replay lists: {both:?}");
        assert!(unclassified.is_empty(), "classify these company routes in REPLAYED or NEVER_REPLAYED: {unclassified:?}");
        for name in REPLAYED.iter().chain(NEVER_REPLAYED) {
            assert!(fb_templates::django::route_table().contains_key(*name), "unknown route in replay lists: {name}");
        }
    }

    /// An unlisted company route is never replayed, whatever it is.
    #[test]
    fn unlisted_company_routes_are_not_replayed() {
        assert!(replayable("/c/00000000-0000-4000-8000-000000000001/settings/", &[]).is_none());
        assert!(NEVER_REPLAYED.contains(&"settings"));
    }

    const OWNER: &str = "Synthetic owner";
    const PASSWORD: &str = "Synthetic owner user password";

    /// A host over a synthetic enrolled book (kept alive by the returned directory).
    fn host() -> (tempfile::TempDir, SessionHost) {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new();
        app.backups.configure(dir.path()).unwrap();
        app.device_key = Some(crate::keys::MasterKey::new([43; 32]));
        app.create_protected_book(&dir.path().join("books/default.fbbook"), OWNER, "Synthetic company", Some("Synthetic book password")).unwrap();
        app.enroll_owner(PASSWORD).unwrap();
        (dir, SessionHost::new(app, Policy::default()))
    }

    fn sign_in(host: &mut SessionHost, id: &SessionId) -> Result<Response, HostError> {
        host.dispatch(id, Request::SignIn { username: OWNER.into(), password: PASSWORD.into() })
    }

    /// R3: a session whose next identity change reaches its generation margin is signed out and
    /// removed in that one request, without re-entering itself, and reports `Evicted` once.
    #[test]
    fn a_session_at_the_end_of_its_generations_is_evicted_once() {
        let (_dir, mut host) = host();
        let id = host.open_session(ClientInfo::remote("Synthetic laptop", None)).unwrap();
        let other = host.open_session(ClientInfo::remote("Other laptop", None)).unwrap();
        sign_in(&mut host, &other).unwrap();
        // Boundary injection: one more identity change crosses the margin.
        let slot = host.slots.get_mut(&id).unwrap();
        slot.generation_limit = slot.state.identity_generation() + GENERATION_MARGIN + 1;
        let _ = sign_in(&mut host, &id);
        assert!(!host.slots.contains_key(&id), "slot removed");
        assert_eq!(host.tombstones.iter().filter(|(t, _)| *t == id).count(), 1, "buried exactly once");
        assert!(matches!(host.dispatch(&id, Request::Status), Err(HostError::Ended(EndReason::Evicted))));
        assert!(matches!(host.dispatch(&id, Request::Status), Err(HostError::UnknownSession)));
        // Nobody else is touched; ending an already-evicted session is a no-op.
        host.end_session(&id, EndReason::Closed);
        assert!(host.status(&other, false).unwrap().signed_in);
    }

    /// R3: the same through the idle sign-out at the start of a request.
    #[test]
    fn an_idle_sign_out_that_exhausts_a_session_ends_it_cleanly() {
        let (_dir, mut host) = host();
        let id = host.open_session(ClientInfo::remote("Synthetic laptop", None)).unwrap();
        sign_in(&mut host, &id).unwrap();
        let slot = host.slots.get_mut(&id).unwrap();
        slot.generation_limit = slot.state.identity_generation() + GENERATION_MARGIN + 1;
        let later = Instant::now() + host.policy.idle_sign_out + Duration::from_secs(1);
        let result = host.dispatch_at(&id, Request::Render { target: "/".into() }, later);
        assert!(matches!(result, Err(HostError::Ended(EndReason::Evicted))), "{result:?}");
        assert!(!host.slots.contains_key(&id));
    }

    /// R3: with the process-wide allocators at their ceiling nothing is reused: new sessions and
    /// record forms are refused, and a handoff with nobody to hand back refuses every sign-in until
    /// the app restarts (never generation zero).
    #[test]
    fn exhausted_allocators_refuse_instead_of_reusing() {
        let (_dir, mut host) = host();
        let local = host.open_session(ClientInfo::local("Host desktop")).unwrap();
        sign_in(&mut host, &local).unwrap();
        let company = host.with_session(&local, |app| app.first_company().unwrap().unwrap()).unwrap();
        let location = match host.with_session(&local, |app| {
            let path = format!("/c/{}/lists/customers/new/", company.to_db());
            let token = app.tokens.mint(&path);
            app.run_desktop(Command::SubmitForm { path, token, fields: vec![("display_name".into(), "Synthetic".into())] })
        }) {
            Ok(Ok(Outcome::Redirect { location })) => location,
            other => panic!("{other:?}"),
        };
        AT_CEILING.with(|c| c.set(true));
        assert!(matches!(host.open_session(ClientInfo::remote("Late laptop", None)), Err(HostError::Maintenance(m)) if m == RESTART));
        let edit = format!("{location}edit/");
        let token = host.with_session(&local, |app| app.tokens.mint(&edit)).unwrap();
        let fields = vec![("display_name".to_owned(), "Unstamped".to_owned())];
        let refused = host.dispatch(&local, Request::Run { command: Command::SubmitForm { path: edit, token, fields } });
        assert!(matches!(refused, Err(HostError::Maintenance(ref m)) if m == RESTART), "{refused:?}");
        // Stop hosting with nobody to hand back.
        let mut app = host.into_app(&SessionId::parse(&"0".repeat(32)).unwrap());
        AT_CEILING.with(|c| c.set(false));
        assert!(app.authenticated_session().is_err());
        let refused = app.sign_in(OWNER, PASSWORD);
        assert!(matches!(&refused, Err(e) if e.to_string() == RESTART), "{refused:?}");
        app.sign_out();
        assert!(app.sign_in(OWNER, PASSWORD).is_err(), "still refused after a sign-out");
        assert!(app.authenticated_session().is_err());
        // Hosting it again keeps refusing: the local person cannot sign in there either.
        let (mut host, local) = SessionHost::host_with_local(app, Policy::default(), "Again");
        assert!(sign_in(&mut host, &local).is_err());
    }

    #[test]
    fn record_keys_name_existing_records_only() {
        let c = CompanyId::new().to_db();
        let r = CompanyId::new().to_db();
        assert_eq!(record_key(&format!("/c/{c}/lists/customers/new/")), None);
        assert_eq!(record_key(&format!("/c/{c}/invoices/new/")), None);
        let detail = record_key(&format!("/c/{c}/lists/customers/{r}/")).unwrap();
        assert_eq!(record_key(&format!("/c/{c}/lists/customers/{r}/edit/")).unwrap(), detail);
        assert_eq!(record_key(&format!("/c/{c}/lists/customers/{r}/status/")).unwrap(), detail);
        assert_ne!(record_key(&format!("/c/{c}/lists/vendors/{r}/edit/")).unwrap(), detail);
        assert_eq!(record_key(&format!("/c/{c}/invoices/{r}/edit/")), record_key(&format!("/c/{c}/invoices/{r}/post/")));
        assert_ne!(record_key(&format!("/c/{c}/invoices/{r}/edit/")), record_key(&format!("/c/{c}/bills/{r}/edit/")));
    }
}
