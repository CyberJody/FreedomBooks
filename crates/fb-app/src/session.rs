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
#[derive(Default)]
pub struct Tokens {
    by_token: HashMap<String, String>,
    order: VecDeque<String>,
}

const MAX_LIVE_TOKENS: usize = 1024;

impl Tokens {
    pub fn mint(&mut self, path: &str) -> String {
        if self.order.len() >= MAX_LIVE_TOKENS && let Some(oldest)=self.order.pop_front() {
            self.by_token.remove(&oldest);
        }
        let token = crate::keys::to_hex(&crate::keys::random_bytes::<32>().expect("system random source"));
        self.order.push_back(token.clone());
        self.by_token.insert(token.clone(), path.to_owned());
        token
    }
    pub fn consume(&mut self, token: &str, path: &str) -> bool {
        match self.by_token.get(token) {
            Some(p) if p == path => {
                self.by_token.remove(token);
                self.order.retain(|t| t != token);
                true
            }
            _ => false,
        }
    }
    pub fn valid(&self, token: &str, path: &str) -> bool {
        self.by_token.get(token).is_some_and(|p| p == path)
    }
    pub fn clear(&mut self) {
        self.by_token.clear();
        self.order.clear();
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
    pub host: Host,
    pub protection: Option<crate::protection::Record>,
    pub recovery_authorized: bool,
    pub os_store_usable: bool,
    /// Installation key supplied by the OS store, independent of every book key.
    pub device_key: Option<MasterKey>,
    /// Plugin surfaces (bank feeds, AI assistant, receipt reader): shown only in builds with the
    /// `plugins` feature. Release builds hide them from every page and refuse their routes.
    pub plugins: bool,
    /// Enable bank feeds independently of the AI plugin surfaces.
    pub simplefin: bool,
    /// DESKTOP-BACKUPS: backup folder, open-book path and key copy for `fb_core::backups`.
    pub backups: screens::backups::Host,
    pub(crate) d7a_reviews: screens::d7a::Reviews,
    pub(crate) d5_reviews: screens::d5::Reviews,
    pub(crate) d6_reviews: screens::d6::Reviews,
    pub screens: Screens,
    pub open: Option<Open>,
    pub tokens: Tokens,
    pub messages: Vec<String>,
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

impl App {
    pub fn bankfeeds_enabled(&self) -> bool {
        self.simplefin || self.plugins
    }

    pub fn new() -> App {
        App {
            host: Host::default(),
            protection: None,
            recovery_authorized: false,
            os_store_usable: false,
            device_key: Some(MasterKey::new(**TOOL_DEVICE_KEY)),
            plugins: cfg!(feature = "plugins"),
            simplefin: cfg!(feature = "simplefin"),
            backups: Default::default(),
            d7a_reviews: Default::default(),
            d5_reviews: Default::default(),
            d6_reviews: Default::default(),
            screens: Screens::new(),
            open: None,
            tokens: Tokens::default(),
            messages: vec![],
            sales_reviews: HashMap::new(),
            today: local_today,
            now: chrono::Utc::now,
            net: None,
            vault_key: None,
            attachment_notes: vec![],
            upgrade_notice: None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open.is_some()
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
        let company = fb_core::services::create_company_with_chart(&mut book, owner, company_name, "USD", true, chart.into())?;
        self.lock();
        self.vault_key = self.device_key.as_ref().and_then(|k| crate::credentials::wrapping_key(k).ok());
        self.open = Some(Open { book, owner, owner_name: owner_name.to_owned(), staff: true });
        self.backups.opened(path, key);
        Ok(Created { company_id: company.id })
    }

    /// Unlock: open the book with its key and find its owner (the one account).
    pub fn open_book(&mut self, path: &Path, key: &MasterKey) -> AppResult<()> {
        self.lock();
        let record = crate::protection::Record::read(path)?;
        let (mut book, upgraded) = self.open_and_upgrade(path, key, record.as_ref())?;
        let (owner, owner_name, staff) = book
            .read(|tx| {
                tx.query_row("SELECT id, username, is_staff FROM fb_user WHERE is_active=1 ORDER BY id LIMIT 1", [], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, bool>(2)?))
                })
            })
            .map_err(|_| AppError::Invalid("This book has no owner account.".into()))?;
        let owner = UserId::parse(&owner).ok_or_else(|| AppError::Invalid("This book's owner id is damaged.".into()))?;
        let legacy = record.is_none();
        let device = self.device_key.as_ref().ok_or_else(|| AppError::Invalid("Connection key storage is unavailable.".into()))?;
        let vault = crate::credentials::BookCredentials::with_device_key(book, owner, device, legacy.then_some(&**key))
            .map_err(|_| AppError::Invalid("The stored connection credentials could not be restored.".into()))?;
        let reconnect = vault.restore_bankfeeds()
            .map_err(|_| AppError::Invalid("The stored connection credentials could not be restored.".into()))?;
        let book = vault.into_book()
            .map_err(|_| AppError::Invalid("The stored connection credentials could not be restored.".into()))?;
        self.vault_key = self.device_key.as_ref().and_then(|k| crate::credentials::wrapping_key(k).ok());
        self.open = Some(Open { book, owner, owner_name, staff });
        self.backups.opened(path, key);
        self.protection = record;
        self.upgrade_notice = upgraded;
        let show_connections = self.bankfeeds_enabled();
        for label in reconnect.into_iter().filter(|_| show_connections) {
            self.messages.push(reconnect_notice(&label));
        }
        Ok(())
    }

    /// Lock: close the book (its key leaves memory with the connection) and forget every form token.
    pub fn lock(&mut self) {
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
        self.sales_reviews.clear();
    }

    pub fn open_mut(&mut self) -> AppResult<&mut Open> {
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

    pub fn run(&mut self, command: Command) -> AppResult<Outcome> {
        if let Command::SubmitForm { path, token, fields } = command {
            // A form that carries no command token never had one to expire: say it could not be sent.
            if token.is_empty() {
                return Err(AppError::Invalid("This form could not be sent. Nothing was saved. Open the screen again; if it happens again, use Report a problem.".into()));
            }
            if !self.tokens.consume(&token, &path) {
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
        let open = self.open_mut()?;
        let actor = open.owner;
        let book = &mut open.book;
        let data = match command {
            Command::SaveCompanyDetails { company_id, name, currency, values } => {
                let c = company(&company_id)?;
                let mut data = serde_json::to_value(&values).expect("values");
                data.as_object_mut().unwrap().remove("logo_upload");
                data.as_object_mut().unwrap().remove("remove_logo");
                let details = serde_json::from_value(data).map_err(|e| AppError::Invalid(e.to_string()))?;
                fb_core::company_documents::save(book, actor, c, &name, &currency,
                    fb_core::company_documents::Changes { details, logo_upload:values.logo_upload, remove_logo:values.remove_logo })?;
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
                fb_core::documents::save_setup(book, actor, c, &values, Some(expected_revision))?
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

/// Shown after unlock for each connection whose saved sign-in couldn't be read.
pub fn reconnect_notice(label: &str) -> String {
    format!("The saved sign-in for {label} couldn't be read. Your books are fine. Reconnect or forget this connection on the Bank feeds page.")
}

fn journal(id: &str) -> AppResult<fb_core::JournalId> { fb_core::JournalId::parse(id).ok_or_else(||AppError::Invalid("Invalid journal id.".into())) }

pub fn company(id: &str) -> AppResult<CompanyId> {
    CompanyId::parse(id).ok_or_else(|| AppError::Invalid("Invalid company id.".into()))
}

/// `sales.services.save_invoice` with the form's values (shared by the typed command and the form post).
pub fn save_invoice(book: &mut Book, actor: UserId, c: CompanyId, values: &InvoiceValues, invoice_id: Option<&str>, expected_revision: i64) -> AppResult<Value> {
    let mut input = serde_json::to_value(values).map_err(|e| AppError::Invalid(e.to_string()))?;
    let lines: Vec<Value> = values
        .lines
        .iter()
        .map(|l| {
            json!({"origin_position": l.origin_position, "item_id": l.item_id, "description": l.description, "quantity": l.quantity,
                   "rate": l.rate, "taxable": l.taxable, "refresh_item": l.refresh_item, "keep_archived": l.keep_archived,
                   "income_account_id": l.income_account_id})
        })
        .collect();
    input["lines"] = Value::Array(lines);
    input["expected_revision"] = json!(expected_revision);
    if let Some(id) = invoice_id {
        input["invoice_id"] = json!(id);
    }
    // Absent optional keys stay absent, as the Python form's `values()` leaves them.
    if let Some(map) = input.as_object_mut() {
        for key in ["due_date", "tax_rate", "tax_label", "tax_item_id"] {
            if map.get(key) == Some(&Value::Null) {
                map.remove(key);
            }
        }
    }
    Ok(fb_core::sales::save_invoice(book, actor, c, &input)?)
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
