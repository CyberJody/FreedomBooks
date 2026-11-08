//! fb-core: the FreedomBooks domain, translated 1:1 from the frozen Python app
//! (tag python-reference-20261030). Layer 1 = the ledger: companies, the chart of accounts (FX2 types,
//! contra accounts), journals and lines, draft -> post -> reverse, entry numbers, the append-only audit
//! trail, company scoping, registers and the trial balance. L2a adds directory, sales tax and
//! document identities/configuration/snapshots/calculations and generic source boundaries.

pub mod agent_provenance;
pub mod backups;
pub mod accounts;
pub mod assistant;
pub mod credits;
pub mod attachments;
pub mod reminders;
pub mod sales;
pub mod prepayments;
pub mod banking;
pub mod bankfeeds;
pub mod calculations;
pub mod card_numbers;
pub mod directory;
pub mod party_display;
mod python_digits;
pub mod documents;
pub mod numbering;
pub mod first_run;
mod first_run_words;
pub mod payables;
pub mod checks;
mod check_numbers;
pub mod transfers;
pub mod salestax;
pub mod email_validation;
pub mod error;
pub mod model;
pub mod py;
pub mod reports;
pub mod services;
pub mod company_documents;
pub mod starter_chart;
pub mod store;
pub mod users;

pub use error::{FbError, FbResult};
pub use fb_db::Book;
pub use fb_ids::{AccountId, CompanyId, JournalId, UserId};
pub use fb_money::Money;

pub mod desktop_mfa;
