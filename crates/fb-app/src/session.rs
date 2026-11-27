//! The application session: one open book (or none, when locked), its owner, the command and query
//! dispatch, and the command tokens that bind a form submission to the screen that rendered it.
use crate::commands::{Command, InvoiceValues, Outcome, Query};
use crate::keys::MasterKey;
use crate::screens;
use fb_core::{AccountId, CompanyId, FbError, UserId};
use fb_db::Book;
use fb_templates::Screens;
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::path::Path;

#[derive(Debug)]
pub enum AppError {
    Locked,
    Ai(crate::ai::Refusal),
    Invalid(String),
    Core(FbError),
    Render(String),
    NotFound(String),
    /// The book did not open; the words say why and what to do.
    Open(crate::opening::OpenError),
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppError::Locked => f.write_str("The books are locked."),
            AppError::Ai(reason) => f.write_str(reason.code()),
            AppError::Invalid(m) => f.write_str(m),
            AppError::Core(e) => write!(f, "{e}"),
            AppError::Render(m) => write!(f, "Screen failed to render: {m}"),
            AppError::NotFound(p) => write!(f, "No screen at {p}"),
            AppError::Open(e) => write!(f, "{e}"),
        }
    }
}

impl From<fb_db::rusqlite::Error> for AppError {
    fn from(e: fb_db::rusqlite::Error) -> Self {
        AppError::Core(FbError::from(e))
    }
}

impl From<FbError> for AppError {
    fn from(e: FbError) -> Self {
        AppError::Core(e)
    }
}

impl From<fb_templates::RenderError> for AppError {
    fn from(e: fb_templates::RenderError) -> Self {
        AppError::Render(e.0)
    }
}

pub type AppResult<T> = Result<T, AppError>;

/// The unlocked book and who is using it.
pub struct Open {
    pub book: Book,
    pub owner: UserId,
    pub owner_name: String,
    pub staff: bool,
}

/// Single-use tokens for rendered forms (Django's CSRF token, replaced). A token is minted per render,
/// bound to the form's path, consumed by the first submission, and all are dropped on lock.
///
/// Each token also remembers `stamp`, the session host's write sequence when it was minted (always 0
/// on the standalone desktop), so the host can tell whether someone else changed the record after the
/// page was rendered (`multiuser`).
#[derive(Default)]
pub struct Tokens {
    by_token: HashMap<String, (String, u64)>,
    order: VecDeque<String>,
    pub(crate) stamp: u64,
    /// Forms withdrawn by a hosting change (`retire_where`), so their submit can say why.
    retired: std::collections::HashSet<String>,
}

const MAX_LIVE_TOKENS: usize = 1024;

impl Tokens {
    pub fn mint(&mut self, path: &str) -> String {
        if self.order.len() >= MAX_LIVE_TOKENS && let Some(oldest)=self.order.pop_front() {
            self.by_token.remove(&oldest);
        }
        let token = crate::keys::to_hex(&crate::keys::random_bytes::<32>().expect("system random source"));
        self.order.push_back(token.clone());
        self.by_token.insert(token.clone(), (path.to_owned(), self.stamp));
        token
    }
    pub fn consume(&mut self, token: &str, path: &str) -> bool {
        match self.by_token.get(token) {
            Some((p, _)) if p == path => {
                self.by_token.remove(token);
                self.order.retain(|t| t != token);
                true
            }
            _ => false,
        }
    }
    pub fn valid(&self, token: &str, path: &str) -> bool {
        self.by_token.get(token).is_some_and(|(p, _)| p == path)
    }
    /// The write sequence a live token was minted at.
    pub(crate) fn stamp_of(&self, token: &str, path: &str) -> Option<u64> {
        self.by_token.get(token).filter(|(p, _)| p == path).map(|(_, stamp)| *stamp)
    }
    /// The oldest live token's stamp: writes before it can no longer make any of these forms stale.
    pub(crate) fn oldest_stamp(&self) -> Option<u64> {
        self.order.front().and_then(|t| self.by_token.get(t)).map(|(_, stamp)| *stamp)
    }
    pub fn clear(&mut self) {
        self.by_token.clear();
        self.order.clear();
        self.retired.clear();
    }
    /// Withdraw every live form whose path matches `which`: it can no longer be submitted.
    pub(crate) fn retire_where(&mut self, which: impl Fn(&str) -> bool) {
        let gone: Vec<String> = self.by_token.iter().filter(|(_, (path, _))| which(path)).map(|(t, _)| t.clone()).collect();
        if self.retired.len() + gone.len() > MAX_LIVE_TOKENS {
            // Bounded: older retired forms then just read as expired.
            self.retired.clear();
        }
        for token in gone {
            self.by_token.remove(&token);
            self.order.retain(|t| *t != token);
            self.retired.insert(token);
        }
    }
    pub(crate) fn retired(&self, token: &str) -> bool {
        self.retired.contains(token)
    }
}


/// The one outside address the app links to: the project's new-issue chooser. The Electron main process
/// opens exactly this URL in the system browser (`electron/main/guards.cjs` `EXTERNAL_URLS`).
pub const ISSUE_URL: &str = "https://github.com/CyberJody/FreedomBooks/issues/new/choose";

/// What "Report a problem" shows for copying into an issue: the app's own version and the host's
/// operating-system name/version, nothing else about the machine. The desktop shell sets both at start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Host {
    pub version: String,
    pub os: String,
}

impl Host {
    /// Plain one-line text: control/format characters dropped, whitespace collapsed, at most 100
    /// characters, and "Unknown" when nothing is left.
    pub fn new(version: &str, os: &str) -> Host {
        fn clean(s: &str) -> String {
            let kept: String = s.chars().map(|c| if c.is_whitespace() { ' ' } else { c }).filter(|c| !fb_core::py::is_control_or_format(*c)).collect();
            let text: String = kept.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(100).collect();
            if text.trim().is_empty() { "Unknown".into() } else { text.trim_end().into() }
        }
        Host { version: clean(version), os: clean(os) }
    }
}

impl Default for Host {
    /// Before the shell reports in (tests, tools): the core's own version and the OS family.
    fn default() -> Host {
        let os = match std::env::consts::OS {
            "linux" => "Linux",
            "macos" => "macOS",
            "windows" => "Windows",
            other => other,
        };
        Host::new(env!("CARGO_PKG_VERSION"), os)
    }
}

pub struct App {
    pub ai: crate::ai::Authority,
    pub(crate) identity: crate::identity::Identity,
    pub(crate) remote_session: bool,
    pub(crate) scheduled_retry: Option<crate::scheduled_backups::Backoff>,
    pub(crate) legacy_credentials: bool,
    pub(crate) prepared_connection: Option<u64>,
    pub host: Host,
    pub protection: Option<crate::protection::Record>,
    pub recovery_authorized: bool,
    pub os_store_usable: bool,
    /// Installation key supplied by the OS store, independent of every book key.
    pub device_key: Option<MasterKey>,
    /// What this launch knows about `device_key` (see [`ConnectionKeys`]).
    pub connection_keys: ConnectionKeys,
    /// Earlier connection keys this computer kept aside (an older version's plaintext key, superseded
    /// by a readable sealed one). Saved connections they can read are read with them and re-saved under
    /// `device_key`; nothing is ever saved under them.
    pub device_alternates: Vec<MasterKey>,
    /// First run made these books and their only owner in this session: the owner's sign-in setup
    /// needs no "existing owner" confirmation. Cleared on lock.
    pub(crate) first_run_owner: bool,
    /// Where the previous main book was set aside after "Use this as my books" (shown once).
    pub set_aside_note: Option<String>,
    /// After a committed change of main books whose saved choice a restart still has to finish.
    pub(crate) book_change_pending: Option<String>,
    /// Plugin surfaces (bank feeds, AI assistant, receipt reader): shown only in builds with the
    /// `plugins` feature. Release builds hide them from every page and refuse their routes.
    pub plugins: bool,
    /// Enable bank feeds independently of the AI plugin surfaces.
    pub simplefin: bool,
    pub invoice_delivery: bool,
    pub(crate) email: crate::invoice_email::State,
    pub(crate) stripe: crate::invoice_stripe::State,
    /// DESKTOP-BACKUPS: backup folder, open-book path and key copy for `fb_core::backups`.
    pub backups: screens::backups::Host,
    pub(crate) d7a_reviews: screens::d7a::Reviews,
    pub(crate) d5_reviews: screens::d5::Reviews,
    pub(crate) d6_reviews: screens::d6::Reviews,
    pub(crate) salestax_reviews: HashMap<String, fb_core::salestax_payments::Review>,
    pub screens: Screens,
    pub open: Option<Open>,
    pub tokens: Tokens,
    pub messages: Vec<String>,
    pub(crate) recurring_notices: std::collections::HashSet<String>,
    pub(crate) sales_reviews: HashMap<String, fb_core::sales::Review>,
    /// The local date screens treat as "today" (overdue, default dates); injectable for tests.
    pub today: fn() -> chrono::NaiveDate,
    /// Presentation instant for print generation; frozen alongside today by screen tooling.
    pub now: fn() -> chrono::DateTime<chrono::Utc>,
    /// The network process (fb-net) for plugin calls; None in tools and tests that need no network.
    pub net: Option<Box<dyn crate::net::NetHost>>,
    /// The vault's subkey of the independent installation secret, so plugin
    /// operations can save and load credentials during the session. Dropped (wiped) on lock.
    pub(crate) vault_key: Option<zeroize::Zeroizing<[u8; 32]>>,
    /// Attachments-panel notes (level, text) for the next document page, like Django's tagged messages.
    pub(crate) attachment_notes: Vec<(String, String)>,
    /// After an upgrade on open: where the pre-upgrade copy is. Shown once, in the next page's banner.
    pub upgrade_notice: Option<String>,
    /// The company whose page was shown last: account, Settings and help pages keep its frame
    /// (sidebar, title-bar name) while it stays open and the signed-in person may see it.
    pub(crate) frame_company: Option<fb_core::CompanyId>,
    /// The title bar's Sign out command token: one per session, minted again only after it is used or
    /// evicted, so a page renders the same twice and pages do not use up the token ring.
    pub(crate) signout_token: Option<String>,
    /// Users whose roster entry an administrator changed during the current request (role, password,
    /// active, memberships). The session host signs their other sessions out; the desktop ignores it.
    pub(crate) roster_changed: Vec<UserId>,
    /// Who owns each browser sign-in job in fb-net ("chatgpt:<company>", "gmail:<company>"): the
    /// session generation that began it and when. fb-net keys jobs by provider and company only, so
    /// only the owning generation may poll or cancel one, and nobody else may start another until it
    /// ends or goes stale (`net::SIGNIN_JOB_STALE`). One owner on the standalone desktop.
    pub(crate) signin_jobs: HashMap<String, (u64, std::time::Instant)>,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

// In-process tools have an ephemeral installation identity. The desktop replaces it from safeStorage.
static TOOL_DEVICE_KEY: std::sync::LazyLock<MasterKey> = std::sync::LazyLock::new(|| crate::keys::new_master_key().expect("system random source"));

fn local_today() -> chrono::NaiveDate {
    chrono::Local::now().date_naive()
}

pub struct Created {
    pub company_id: CompanyId,
}

/// The installation's connection key (bank feeds, Stripe, Gmail, AI), as the shell found it.
/// Sign-in flags a saved connection "reconnect needed" only when no key on this computer (the
/// connection key or a superseded one kept aside) can read it, and never while `Unavailable`;
/// `Replaced` adds the explanation that the keyring is new.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionKeys {
    /// The desktop has not consulted the keyring yet. Connection forms can request it on submit.
    Deferred,
    /// The saved key opened, or a new installation saved its first key.
    Ready,
    /// A working keyring proved the saved key unreadable (sealed by another keyring, after a
    /// reinstall). A new key replaces it; saved connections must be connected again, once.
    Replaced,
    /// The keyring can't be used right now. Saved connections are left exactly as they are and are
    /// unavailable for this session only.
    Unavailable,
}

impl ConnectionKeys {
    pub fn parse(state: &str) -> Option<Self> {
        match state {
            "ready" | "created" => Some(Self::Ready),
            "replaced" => Some(Self::Replaced),
            "unavailable" => Some(Self::Unavailable),
            _ => None,
        }
    }
}

/// Shown once after sign-in when saved connections can't be used this session.
pub const CONNECTIONS_WAITING: &str = "Bank feeds, Stripe and Gmail are unavailable right now because this computer's keyring can't be used. Nothing was changed: your saved connections will work again once the keyring is available and FreedomBooks is restarted.";
/// Shown once after sign-in when a reinstalled keyring could not read the old connection key.
pub const CONNECTIONS_REPLACED: &str = "This computer's keyring is new (for example after the operating system was reinstalled), so FreedomBooks started a new key for saved connections. Connect each bank, Stripe and Gmail connection again; your books and downloaded transactions are fine.";

impl App {
    pub fn bankfeeds_enabled(&self) -> bool {
        self.simplefin || self.plugins
    }

    pub fn new() -> App {
        App {
            ai: Default::default(),
            identity: Default::default(),
            remote_session: false,
            scheduled_retry: None,
            legacy_credentials: false,
            prepared_connection: None,
            frame_company: None,
            signout_token: None,
            host: Host::default(),
            protection: None,
            recovery_authorized: false,
            os_store_usable: false,
            device_key: Some(MasterKey::new(**TOOL_DEVICE_KEY)),
            connection_keys: ConnectionKeys::Ready,
            device_alternates: vec![],
            first_run_owner: false,
            set_aside_note: None,
            book_change_pending: None,
            plugins: cfg!(feature = "plugins"),
            simplefin: cfg!(feature = "simplefin"),
            invoice_delivery: cfg!(feature = "invoice-delivery"),
            email: Default::default(),
            stripe: Default::default(),
            backups: Default::default(),
            d7a_reviews: Default::default(),
            d5_reviews: Default::default(),
            salestax_reviews: Default::default(),
            d6_reviews: Default::default(),
            screens: Screens::new(),
            open: None,
            tokens: Tokens::default(),
            messages: vec![],
            recurring_notices: Default::default(),
            sales_reviews: HashMap::new(),
            today: local_today,
            now: chrono::Utc::now,
            net: None,
            vault_key: None,
            attachment_notes: vec![],
            upgrade_notice: None,
            roster_changed: vec![],
            signin_jobs: HashMap::new(),
        }
    }

    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// The session's credential subkey, or None while the connection key is unavailable.
    pub(crate) fn connection_vault_key(&self) -> Option<zeroize::Zeroizing<[u8; 32]>> {
        if matches!(self.connection_keys, ConnectionKeys::Deferred | ConnectionKeys::Unavailable) { return None; }
        self.device_key.as_ref().and_then(|k| crate::credentials::wrapping_key(k).ok())
    }

    /// First run (ARCHITECTURE 7): a new encrypted book, its owner account and the first company.
    /// Refuses to touch an existing file.
    pub fn create_book(&mut self, path: &Path, key: &MasterKey, owner_name: &str, company_name: &str) -> AppResult<Created> {
        self.create_book_with_chart(path, key, owner_name, company_name, crate::commands::StarterChart::Empty)
    }

    pub fn create_book_with_chart(&mut self, path: &Path, key: &MasterKey, owner_name: &str,
        company_name: &str, chart: crate::commands::StarterChart) -> AppResult<Created> {
        // Validate raw first-run names before trimming or creating a file. A
        // refused name must not leave a book that prevents first-run retry.
        fb_core::py::clean_free_text(owner_name).map_err(|e| AppError::Invalid(e.message()))?;
        fb_core::py::clean_free_text(company_name).map_err(|e| AppError::Invalid(e.message()))?;
        let owner_name = owner_name.trim();
        let company_name = company_name.trim();
        if owner_name.is_empty() || owner_name.chars().count() > 150 {
            return Err(AppError::Invalid("Enter the owner's name (up to 150 characters).".into()));
        }
        if company_name.is_empty() {
            return Err(AppError::Invalid("Enter the company name.".into()));
        }
        if path.exists() {
            return Err(AppError::Invalid("A book already exists at that location.".into()));
        }
        let mut book = Book::create(path, key).map_err(|e| AppError::Invalid(format!("Could not create the book: {e:?}")))?;
        let owner = fb_core::users::create_user(&mut book, owner_name, true)?;
        book.write(|tx| tx.execute("INSERT INTO desktop_user(user_id,role) VALUES(?1,'administrator')", [owner.to_db()]))?;
        let company = fb_core::services::create_company_with_chart(&mut book, owner, company_name, "USD", true, chart.into())?;
        self.lock();
        self.vault_key = self.connection_vault_key();
        self.open = Some(Open { book, owner, owner_name: owner_name.to_owned(), staff: true });
        self.first_run_owner = true;
        self.backups.opened(path, key);
        Ok(Created { company_id: company.id })
    }

    /// Close the current book and settle any interrupted move before inspecting book files or keys.
    pub(crate) fn prepare_book_open(&mut self) -> AppResult<()> {
        self.lock();
        // A change of main books ("Use this as my books") that failed and could not be undone in place
        // leaves its journal behind. Finish or undo it before any book opens, whichever book this is;
        // if that can't be done, nothing opens (review 2026-11-11: a half-moved book opened without its
        // write-ahead log, and putting the log back later damaged it).
        if let Ok(root) = self.backups.root().map(Path::to_owned) {
            crate::books::recover_before_open(&root)?;
        }
        Ok(())
    }

    /// Unlock: open the book with its key and find its owner (the one account).
    pub fn open_book(&mut self, path: &Path, key: &MasterKey) -> AppResult<()> {
        self.prepare_book_open()?;
        let record = crate::protection::Record::read(path)?;
        let (book, upgraded) = self.open_and_upgrade(path, key, record.as_ref())?;
        // Unlocking proves access to the file, never a named user's identity.
        // Credential restore runs after password verification, under the actual actor.
        self.legacy_credentials = record.is_none();
        self.open = Some(Open { book, owner: UserId::new(), owner_name: String::new(), staff: false });
        self.backups.opened(path, key);
        self.protection = record;
        self.upgrade_notice = upgraded;
        Ok(())
    }

    /// Lock: close the book (its key leaves memory with the connection) and forget every form token.
    pub fn lock(&mut self) {
        self.invalidate_identity();
        self.ai.clear_unattended();
        self.legacy_credentials = false;
        self.open = None;
        self.protection = None;
        self.recovery_authorized = false;
        self.backups.locked();
        self.vault_key = None;
        self.upgrade_notice = None;
        self.tokens.clear();
        self.d7a_reviews = Default::default();
        self.d5_reviews = Default::default();
        self.d6_reviews = Default::default();
        self.messages.clear();
        self.recurring_notices.clear();
        self.sales_reviews.clear();
        self.first_run_owner = false;
        self.set_aside_note = None;
        self.book_change_pending = None;
    }

    pub fn open_mut(&mut self) -> AppResult<&mut Open> {
        self.authorize(None, crate::identity::Permission::Read)?;
        self.open.as_mut().ok_or(AppError::Locked)
    }

    /// The company the window lands on after unlock: the first one by name.
    pub fn first_company(&mut self) -> AppResult<Option<CompanyId>> {
        let open = self.open_mut()?;
        let owner = open.owner;
        let id: Option<String> = open
            .book
            .read(|tx| {
                tx.query_row(
                    "SELECT c.id FROM ledger_company c JOIN ledger_company_members m ON m.company_id=c.id WHERE m.user_id=?1 ORDER BY c.name, c.id LIMIT 1",
                    [owner.to_db()],
                    |r| r.get(0),
                )
                .map(Some)
                .or_else(|e| if matches!(e, fb_db::rusqlite::Error::QueryReturnedNoRows) { Ok(None) } else { Err(e) })
            })
            .map_err(|e| AppError::Invalid(e.to_string()))?;
        Ok(id.and_then(|s| CompanyId::parse(&s)))
    }

    /// Renderer boundary. Every mutation is a path-bound, single-use SubmitForm.
    /// Typed commands remain available to trusted in-process screen dispatch and tooling.
    pub fn run_renderer(&mut self, command: Command) -> AppResult<Outcome> {
        if !matches!(command, Command::SubmitForm { .. } | Command::ExportHistory { .. } | Command::ExportReport { .. } | Command::ExportBackup { .. }) {
            return Err(AppError::Invalid("Unknown command.".into()));
        }
        self.run(command)
    }

    /// The desktop's entry for renderer commands (fb-node `run`): [`App::run_renderer`], except that
    /// a form post that fails after spending its token gets the form back ([`Outcome::Retry`])
    /// instead of an error that leaves an expired form on the screen.
    ///
    /// A fresh token is issued only when a resubmit cannot repeat anything: this post spent a token
    /// that was live, no write transaction committed during it (the same connection's commit
    /// counter is unchanged), the books are still open for the same session, and the form's route
    /// is declared to write only the book ([`screens::resubmit`]; a route nobody declared gets no
    /// token). Otherwise the message says to reopen the page and check, and no token is issued.
    /// Typed forms that save documents also carry request keys and expected revisions, so even an
    /// exact resubmit of a saved document is answered with the same document or a revision refusal.
    pub fn run_desktop(&mut self, command: Command) -> AppResult<Outcome> {
        let form = match &command {
            Command::SubmitForm { path, token, fields } => {
                let outside = screens::resubmit::form(path, fields) == screens::resubmit::Resubmit::Outside;
                Some((path.clone(), self.tokens.valid(token, path), outside))
            }
            _ => None,
        };
        let commits = |app: &App| app.open.as_ref().map(|o| (o.book.connection_id(), o.book.commits()));
        let before = commits(self);
        let generation = self.session_generation();
        let token = match &command { Command::SubmitForm { token, .. } => token.clone(), _ => String::new() };
        let result = self.run_renderer(command);
        let Some((path, live, outside)) = form else { return result };
        let error = match result {
            Ok(outcome) => return Ok(outcome),
            Err(error) => error,
        };
        let spent = live && !self.tokens.valid(&token, &path);
        if !spent || matches!(error, AppError::Locked) || !self.is_open() || self.session_generation() != generation {
            return Err(error);
        }
        // A different connection (the post replaced the open book) cannot be compared: treat it
        // as saved.
        let committed = commits(self) != before;
        note_form_error(&path, &error);
        if committed || outside {
            let check = if committed { "Part of it may already have been saved." } else { "It may already have been done." };
            let message = format!("{} {check} Open this page again and check before trying again.", retry_message(&error, false));
            return Ok(Outcome::Retry { message, token: None });
        }
        Ok(Outcome::Retry { message: retry_message(&error, true), token: Some(self.tokens.mint(&path)) })
    }

    pub fn run(&mut self, command: Command) -> AppResult<Outcome> {
        self.authorize_command(&command)?;
        if let Command::SubmitForm { path, token, fields } = command {
            // A form that carries no command token never had one to expire: say it could not be sent.
            if token.is_empty() {
                return Err(AppError::Invalid("This form could not be sent. Nothing was saved. Open the screen again; if it happens again, use Report a problem.".into()));
            }
            if !self.tokens.consume(&token, &path) {
                if self.tokens.retired(&token) {
                    return Err(AppError::Invalid(
                        "Hosting changed while this record was open, so nothing was saved. Open the record again to see any changes, then make yours.".into(),
                    ));
                }
                return Err(AppError::Invalid("This form has expired. Open the screen again and resubmit.".into()));
            }
            return screens::submit(self, &path, &fields);
        }
        if let Command::ExportReport { company_id, report, filters } = command {
            return screens::export_report(self, company(&company_id)?, &report, &filters);
        }
        if let Command::ExportBackup { company_id, name } = command {
            return screens::backups::export(self, company(&company_id)?, &name);
        }
        if let Command::ExportHistory {
            company_id,
            filters,
        } = command
        {
            return screens::export_history(self, company(&company_id)?, &filters);
        }
        if screens::is_d4_command(&command) {
            return screens::run_d4_command(self, command);
        }
        if screens::is_d3_command(&command) {
            return screens::run_d3_command(self, command);
        }
        if screens::is_d7a_command(&command) { return screens::run_d7a_command(self, command); }
        if screens::is_d7b_command(&command) { return screens::run_d7b_command(self, command); }
        if screens::is_d5_command(&command) { return screens::run_d5_command(self, command); }
        if screens::is_d6_command(&command) { return screens::run_d6_command(self, command); }
        if screens::is_jobs_command(&command) { return screens::run_jobs_command(self, command); }
        let open = self.open_mut()?;
        let actor = open.owner;
        let book = &mut open.book;
        let data = match command {
            Command::SaveCompanyDetails { company_id, name, currency, values } => {
                let c = company(&company_id)?;
                let mut data = serde_json::to_value(&values).expect("values");
                data.as_object_mut().unwrap().remove("logo_upload");
                data.as_object_mut().unwrap().remove("remove_logo");
                data.as_object_mut().unwrap().remove("invoice_preferences");
                let details = serde_json::from_value(data).map_err(|e| AppError::Invalid(e.to_string()))?;
                fb_core::company_documents::save_with_preferences(book, actor, c, &name, &currency,
                    fb_core::company_documents::Changes { details, logo_upload:values.logo_upload, remove_logo:values.remove_logo },
                    values.invoice_preferences.map(|preferences| preferences.into_change()))?;
                json!({"id":company_id})
            }
            Command::SetNextNumber { company_id, kind, bank_id, next, expected_next, expected_revision } => {
                fb_core::numbering::set_next(book,actor,company(&company_id)?,fb_core::numbering::Change {
                    kind:&kind, bank:bank_id.as_deref().map(|id| AccountId::parse(id).ok_or_else(||AppError::Invalid("Invalid bank account id.".into()))).transpose()?, next:&next, expected_next:&expected_next, expected_revision,
                })?
            }
            Command::SaveCompany { company_id, name, currency, starter_chart } => {
                let c=match company_id {
                    Some(id)=>fb_core::services::edit_company(book,actor,company(&id)?,&name,&currency)?,
                    None=>fb_core::services::create_company_with_chart(book,actor,&name,&currency,true,starter_chart.into())?,
                };
                json!({"id":c.id.to_db()})
            }
            Command::SaveJournal { company_id, journal_id, date, memo, entry_number, adjusting, request_key, expected_revision, lines } => {
                let input=fb_core::services::DraftInput {
                    date,memo,entry_number:Some(entry_number),adjusting:Some(adjusting),request_key,expected_revision,
                    journal_id:journal_id.as_deref().map(journal).transpose()?,
                    lines:lines.iter().map(|l|serde_json::to_value(l).expect("typed journal line")).collect(),
                };
                let j=fb_core::services::save_draft(book,actor,company(&company_id)?,input)?;
                json!({"id":j.id.to_db()})
            }
            Command::PostJournal { company_id, journal_id, expected_revision } => {
                let j=fb_core::services::post_journal(book,actor,company(&company_id)?,journal(&journal_id)?,Some(expected_revision))?;
                json!({"id":j.id.to_db()})
            }
            Command::DeleteJournal { company_id, journal_id, expected_revision } => {
                fb_core::services::delete_draft_journal(book,actor,company(&company_id)?,journal(&journal_id)?,Some(expected_revision))?;
                json!({"id":journal_id})
            }
            Command::ReverseJournal { company_id, journal_id, date, reason } => {
                let j=fb_core::services::reverse_journal(book,actor,company(&company_id)?,journal(&journal_id)?,&date,&reason)?;
                json!({"id":j.id.to_db()})
            }
            Command::FillAccountingSetup {
                company_id,
                digest,
                expected_revision,
            } => fb_core::first_run::apply_proposal(
                book,
                actor,
                company(&company_id)?,
                &digest,
                expected_revision,
            )?,
            Command::DismissSetup { company_id } => {
                let c=fb_core::first_run::dismiss_first_run(book,actor,company(&company_id)?)?;
                json!({"id":c.id.to_db()})
            }
            Command::SaveAccount { company_id, account_id, active, postable, code, name, detail_type, is_contra, description } => {
                let c = company(&company_id)?;
                let mut input = fb_core::services::AccountInput::new(&code, &name)
                    .detail(&detail_type)
                    .contra(is_contra)
                    .description(&description)
                    .active(active)
                    .postable(postable);
                input.account = account_id
                    .as_deref()
                    .map(|id| {
                        AccountId::parse(id)
                            .ok_or_else(|| AppError::Invalid("Invalid account id.".into()))
                    })
                    .transpose()?;
                let a = fb_core::services::save_account(book, actor, c, input)?;
                json!({"id": a.id.to_db(), "code": a.code})
            }
            Command::SaveCustomer { company_id, display_name } => {
                let c = company(&company_id)?;
                fb_core::directory::save_record(book, actor, c, fb_core::directory::Model::Customer, &json!({"display_name": display_name}), None)?
            }
            Command::SaveItem { company_id, code, description, kind, sale_rate, income_account_id, expense_account_id } => {
                let c = company(&company_id)?;
                let values = json!({"code": code, "description": description, "kind": kind, "sale_rate": sale_rate,
                                    "income_account": income_account_id, "expense_account": expense_account_id});
                fb_core::directory::save_record(book, actor, c, fb_core::directory::Model::Item, &values, None)?
            }
            Command::SaveSetup { company_id, expected_revision, receivables_id, payables_id, undeposited_id, collected_tax_id, customer_deposits_id, bank_ids } => {
                let c = company(&company_id)?;
                let values = json!({"receivables": receivables_id, "payables": payables_id, "undeposited": undeposited_id, "collected_tax": collected_tax_id, "customer_deposits":customer_deposits_id, "banks": bank_ids});
                save_accounting_setup(book, actor, c, &values, &bank_ids, expected_revision)?
            }
            Command::SaveInvoice { company_id, values } => {
                let c = company(&company_id)?;
                save_invoice(book, actor, c, &values, None, 0)?
            }
            Command::PostInvoice { company_id, invoice_id, expected_revision } => {
                let c = company(&company_id)?;
                let review = fb_core::sales::review_invoice(
                    book,
                    actor,
                    c,
                    &invoice_id,
                    Some(expected_revision),
                )?;
                fb_core::sales::post_invoice(
                    book,
                    actor,
                    c,
                    &invoice_id,
                    Some(expected_revision),
                    Some(&review),
                )?
            }
            Command::SubmitForm { .. } | Command::ExportReport { .. } | Command::ExportHistory { .. } | Command::ExportBackup { .. } => {
                unreachable!("handled above")
            }
            _ => unreachable!("D3 commands handled above"),
        };
        Ok(Outcome::Done { data })
    }

    pub fn query(&mut self, query: Query) -> AppResult<Value> {
        if let Query::Schema = query {
            return Ok(crate::commands::schema());
        }
        self.authorize_query(&query)?;
        let open = self.open_mut()?;
        let actor = open.owner;
        let book = &mut open.book;
        Ok(match query {
            Query::CompanyLogo { company_id } => {
                let c = company(&company_id)?;
                book.read(|tx| fb_core::company_documents::logo_json(tx, actor, c))?
            }
            Query::NumberingSettings { company_id } => {
                fb_core::numbering::settings(book,actor,company(&company_id)?)?
            }
            Query::Companies => {
                let rows = book
                    .read(|tx| {
                        let mut st = tx.prepare(
                            "SELECT c.id, c.name, c.currency FROM ledger_company c JOIN ledger_company_members m ON m.company_id=c.id WHERE m.user_id=?1 ORDER BY c.id",
                        )?;
                        st.query_map([actor.to_db()], |r| Ok(json!({"id": r.get::<_, String>(0)?, "name": r.get::<_, String>(1)?, "currency": r.get::<_, String>(2)?})))?
                            .collect::<Result<Vec<_>, _>>()
                    })
                    .map_err(|e| AppError::Invalid(e.to_string()))?;
                Value::Array(rows)
            }
            Query::Accounts { company_id } => {
                let c = company(&company_id)?;
                let accounts = book.read(|tx| {
                    fb_core::services::access(tx, actor, c)?;
                    fb_core::store::accounts(tx, c)
                })?;
                Value::Array(accounts.iter().map(|a| json!({"id": a.id.to_db(), "code": a.code, "name": a.name, "detail_type": a.detail_type.as_str()})).collect())
            }
            Query::Register { company_id, account_id, start, end } => {
                let c = company(&company_id)?;
                let a = AccountId::parse(&account_id).ok_or_else(|| AppError::Invalid("Invalid account id.".into()))?;
                let r = fb_core::reports::register(book, actor, c, a, start.as_deref(), end.as_deref())?;
                json!({"opening": r.opening.to_string(), "closing": r.closing.to_string(),
                       "rows": r.rows.iter().map(|row| json!({"date": row.date, "memo": row.memo, "debit": row.line.debit.to_string(),
                                                               "credit": row.line.credit.to_string(), "balance": row.balance.to_string()})).collect::<Vec<_>>()})
            }
            Query::Invoices { company_id } => {
                let c = company(&company_id)?;
                Value::Array(fb_core::sales::invoice_balances(book, actor, c)?)
            }
            Query::Schema => unreachable!("handled above"),
        })
    }
}

/// Plain words for a failed form post. Refusals and validation already speak to the owner;
/// database, template and file problems never show their technical text.
/// `nothing_saved` adds that reassurance where the error's own words do not already speak to it.
fn retry_message(error: &AppError, nothing_saved: bool) -> String {
    let nothing = if nothing_saved { " Nothing was saved." } else { "" };
    match error {
        AppError::Core(FbError::Validation(messages)) => messages.join(" "),
        AppError::Core(FbError::Refused(m) | FbError::PermissionDenied(m)) | AppError::Invalid(m) => m.clone(),
        AppError::Core(FbError::DoesNotExist(_)) | AppError::NotFound(_) => {
            format!("That record is no longer there. It may have been changed or deleted in another window.{nothing}")
        }
        AppError::Core(FbError::Integrity { .. }) => format!("The books refused this change because it would break one of their rules.{nothing}"),
        _ if nothing_saved => "FreedomBooks couldn't finish this. Nothing was saved. Try again; if it happens again, use Report a problem.".into(),
        _ => "FreedomBooks couldn't finish this.".into(),
    }
}

/// Database, template and file problems on a form post go to the problem log (kind and route only).
fn note_form_error(path: &str, error: &AppError) {
    let kind = match error {
        AppError::Core(e @ (FbError::Integrity { .. } | FbError::Db(_))) => e.kind(),
        AppError::Render(_) => "Render",
        AppError::Open(_) => "Open",
        _ => return,
    };
    let route = screens::resolve(path).map(|r| r.name).unwrap_or_else(|| "unresolved".into());
    fb_diag::record(fb_diag::Entry { source: "rust", op: "form".into(), route: Some(route), kind: format!("error:{kind}"), outcome: vec!["form returned for retry"], book_open: Some(true), ..Default::default() });
}

/// Shown after unlock for each connection whose saved sign-in couldn't be read.
pub fn reconnect_notice(label: &str) -> String {
    format!("The saved sign-in for {label} couldn't be read. Your books are fine. Reconnect or forget this connection on the Bank feeds page.")
}

fn journal(id: &str) -> AppResult<fb_core::JournalId> { fb_core::JournalId::parse(id).ok_or_else(||AppError::Invalid("Invalid journal id.".into())) }

// The desktop can retain existing bank designations but only add Bank-type assets. Validate
// and save under the same Writer so a concurrent account change cannot invalidate the choice.
fn save_accounting_setup(
    book: &mut Book, actor: UserId, company: CompanyId, values: &Value, bank_ids: &[String], revision: i64,
) -> fb_core::FbResult<Value> {
    book.write(|tx| {
        if !bank_ids.is_empty() {
            let setup = fb_core::documents::setup_snapshot(tx, company)?;
            let existing = setup["banks"].as_array().expect("bank list in setup snapshot");
            for id in bank_ids {
                let account = AccountId::parse(id).map(|id| fb_core::store::account(tx, id)).transpose()?.flatten()
                    .filter(|a| a.company_id == company)
                    .ok_or_else(|| FbError::validation("Select a bank account from this company."))?;
                if !account.active || !account.postable || account.kind != fb_core::model::Kind::Asset {
                    return Err(FbError::validation(format!("{} is unavailable as a bank account. Replace or clear it.", account.label())));
                }
                if account.detail_type != fb_core::model::DetailType::Bank && !existing.contains(&json!(id)) {
                    return Err(FbError::validation(format!("{} is not a Bank-type account.", account.label())));
                }
            }
        }
        fb_core::documents::save_setup_tx(tx, actor, company, values, Some(revision))
    })
}

pub fn company(id: &str) -> AppResult<CompanyId> {
    CompanyId::parse(id).ok_or_else(|| AppError::Invalid("Invalid company id.".into()))
}

/// `sales.services.save_invoice` with the form's values (shared by the typed command and the form post).
pub fn save_invoice(book: &mut Book, actor: UserId, c: CompanyId, values: &InvoiceValues, invoice_id: Option<&str>, expected_revision: i64) -> AppResult<Value> {
    let mut input = invoice_input(values)?;
    input["expected_revision"]=json!(expected_revision);
    if let Some(id)=invoice_id { input["invoice_id"]=json!(id); }
    Ok(fb_core::sales::save_invoice(book, actor, c, &input)?)
}

/// Canonical typed invoice adapter shared by the editor and historical import.
pub(crate) fn invoice_input(values:&InvoiceValues)->AppResult<Value> {
    let mut input = serde_json::to_value(values).map_err(|e| AppError::Invalid(e.to_string()))?;
    let lines: Vec<Value> = values
        .lines
        .iter()
        .map(|l| {
            let mut row = json!({"origin_position": l.origin_position, "item_id": l.item_id, "description": l.description, "quantity": l.quantity,
                   "rate": l.rate, "taxable": l.taxable, "refresh_item": l.refresh_item, "keep_archived": l.keep_archived,
                   "income_account_id": l.income_account_id});
            if let Some(kind) = &l.line_kind { row["line_kind"] = json!(kind); }
            row
        })
        .collect();
    input["lines"] = Value::Array(lines);
    // Absent optional keys stay absent, as the Python form's `values()` leaves them.
    if let Some(map) = input.as_object_mut() {
        for key in ["due_date", "tax_rate", "tax_label", "tax_item_id"] {
            if map.get(key) == Some(&Value::Null) {
                map.remove(key);
            }
        }
    }
    Ok(input)
}

#[cfg(test)]
mod secfix1_tokens {
    use super::*;
    #[test]
    fn consumed_tokens_do_not_accumulate_or_evict_live_forms() {
        let mut tokens=Tokens::default();
        let live=tokens.mint("/live/");
        for _ in 0..2048 {
            let t=tokens.mint("/other/");
            assert!(tokens.consume(&t,"/other/"));
            assert!(!tokens.consume(&t,"/other/"));
        }
        assert_eq!(tokens.order.len(),1);
        assert!(tokens.consume(&live,"/live/"));
        assert!(tokens.order.is_empty());
    }
}
