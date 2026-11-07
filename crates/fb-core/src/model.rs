//! Port of `ledger/models.py`: company, account types (FX2), accounts, audit events, journals, lines.

use fb_ids::{AccountId, CompanyId, JournalId, JournalLineId, UserId};
use fb_money::Money;

pub const BILL_REMINDER_DEFAULT: i64 = 10;
pub const BILL_REMINDER_MAX: i64 = 90;
pub const DESCRIPTION_MAX: usize = 200;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Company {
    pub id: CompanyId,
    pub name: String,
    pub currency: String,
    pub first_run_open: bool,
    pub bill_reminder_days: i64,
}

/// The five base kinds (posting / normal-balance backbone).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Asset,
    Liability,
    Equity,
    Income,
    Expense,
}

impl Kind {
    pub const ALL: [Kind; 5] = [Kind::Asset, Kind::Liability, Kind::Equity, Kind::Income, Kind::Expense];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Asset => "ASSET",
            Kind::Liability => "LIABILITY",
            Kind::Equity => "EQUITY",
            Kind::Income => "INCOME",
            Kind::Expense => "EXPENSE",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.as_str() == s)
    }

    /// `get_kind_display()`.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Asset => "Asset",
            Kind::Liability => "Liability",
            Kind::Equity => "Equity",
            Kind::Income => "Income",
            Kind::Expense => "Expense",
        }
    }

    /// `DEFAULT_DETAIL`: the plain type of each base kind.
    pub fn default_detail(self) -> DetailType {
        match self {
            Kind::Asset => DetailType::OtherCurrentAsset,
            Kind::Liability => DetailType::OtherCurrentLiability,
            Kind::Equity => DetailType::Equity,
            Kind::Income => DetailType::Income,
            Kind::Expense => DetailType::Expense,
        }
    }
}

/// FX2 account type. Declaration order is the chart and report order (`DETAIL_ORDER`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DetailType {
    Bank,
    Receivable,
    OtherCurrentAsset,
    FixedAsset,
    OtherAsset,
    Payable,
    CreditCard,
    OtherCurrentLiability,
    LongTermLiability,
    Equity,
    Income,
    CostOfGoodsSold,
    Expense,
    OtherIncome,
    OtherExpense,
}

impl DetailType {
    pub const ALL: [DetailType; 15] = [
        DetailType::Bank,
        DetailType::Receivable,
        DetailType::OtherCurrentAsset,
        DetailType::FixedAsset,
        DetailType::OtherAsset,
        DetailType::Payable,
        DetailType::CreditCard,
        DetailType::OtherCurrentLiability,
        DetailType::LongTermLiability,
        DetailType::Equity,
        DetailType::Income,
        DetailType::CostOfGoodsSold,
        DetailType::Expense,
        DetailType::OtherIncome,
        DetailType::OtherExpense,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            DetailType::Bank => "BANK",
            DetailType::Receivable => "RECEIVABLE",
            DetailType::OtherCurrentAsset => "OTHER_CURRENT_ASSET",
            DetailType::FixedAsset => "FIXED_ASSET",
            DetailType::OtherAsset => "OTHER_ASSET",
            DetailType::Payable => "PAYABLE",
            DetailType::CreditCard => "CREDIT_CARD",
            DetailType::OtherCurrentLiability => "OTHER_CURRENT_LIABILITY",
            DetailType::LongTermLiability => "LONG_TERM_LIABILITY",
            DetailType::Equity => "EQUITY",
            DetailType::Income => "INCOME",
            DetailType::CostOfGoodsSold => "COST_OF_GOODS_SOLD",
            DetailType::Expense => "EXPENSE",
            DetailType::OtherIncome => "OTHER_INCOME",
            DetailType::OtherExpense => "OTHER_EXPENSE",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            DetailType::Bank => "Bank",
            DetailType::Receivable => "Accounts receivable",
            DetailType::OtherCurrentAsset => "Other current asset",
            DetailType::FixedAsset => "Fixed asset",
            DetailType::OtherAsset => "Other asset",
            DetailType::Payable => "Accounts payable",
            DetailType::CreditCard => "Credit card",
            DetailType::OtherCurrentLiability => "Other current liability",
            DetailType::LongTermLiability => "Long-term liability",
            DetailType::Equity => "Equity",
            DetailType::Income => "Income",
            DetailType::CostOfGoodsSold => "Cost of goods sold",
            DetailType::Expense => "Expense",
            DetailType::OtherIncome => "Other income",
            DetailType::OtherExpense => "Other expense",
        }
    }

    pub fn parse(s: &str) -> Option<DetailType> {
        DetailType::ALL.into_iter().find(|d| d.as_str() == s)
    }

    /// `DETAIL_KIND`.
    pub fn kind(self) -> Kind {
        use DetailType::*;
        match self {
            Bank | Receivable | OtherCurrentAsset | FixedAsset | OtherAsset => Kind::Asset,
            Payable | CreditCard | OtherCurrentLiability | LongTermLiability => Kind::Liability,
            Equity => Kind::Equity,
            Income | OtherIncome => Kind::Income,
            CostOfGoodsSold | Expense | OtherExpense => Kind::Expense,
        }
    }

    /// `NO_CONTRA`: subledger / cash types that refuse the contra flag.
    pub fn refuses_contra(self) -> bool {
        matches!(self, DetailType::Bank | DetailType::Receivable | DetailType::Payable | DetailType::CreditCard)
    }

    /// `DETAIL_ORDER`.
    pub fn order(self) -> usize {
        self as usize
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Debit,
    Credit,
}

impl Side {
    pub fn as_str(self) -> &'static str {
        match self {
            Side::Debit => "DEBIT",
            Side::Credit => "CREDIT",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    pub id: AccountId,
    pub company_id: CompanyId,
    pub code: String,
    pub name: String,
    pub kind: Kind,
    pub detail_type: DetailType,
    pub is_contra: bool,
    pub description: String,
    pub active: bool,
    pub postable: bool,
}

impl Account {
    /// Debit or credit side of the account's type (its report section): a contra account reads negative.
    pub fn section_balance(&self) -> Side {
        if matches!(self.kind, Kind::Asset | Kind::Expense) { Side::Debit } else { Side::Credit }
    }

    /// The side the account normally carries: the section side, flipped for a contra account.
    pub fn normal_balance(&self) -> Side {
        match (self.section_balance(), self.is_contra) {
            (side, false) => side,
            (Side::Debit, true) => Side::Credit,
            (Side::Credit, true) => Side::Debit,
        }
    }

    pub fn type_order(&self) -> usize {
        self.detail_type.order()
    }

    /// `str(account)`.
    pub fn label(&self) -> String {
        format!("{} · {}", self.code, self.name)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Draft,
    Posted,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Draft => "DRAFT",
            Status::Posted => "POSTED",
        }
    }
    pub fn parse(s: &str) -> Option<Status> {
        match s {
            "DRAFT" => Some(Status::Draft),
            "POSTED" => Some(Status::Posted),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Journal {
    pub id: JournalId,
    /// The book-wide J-number (the Python serial pk's display).
    pub number: i64,
    pub company_id: CompanyId,
    pub date: String,
    pub memo: String,
    pub status: Status,
    pub created_by_id: UserId,
    pub created_at: String,
    pub posted_by_id: Option<UserId>,
    pub posted_at: Option<String>,
    pub reversal_of_id: Option<JournalId>,
    pub source_id: Option<String>,
    pub entry_number: String,
    pub adjusting: bool,
    pub request_key: Option<String>,
    pub creation_digest: String,
    pub revision: i64,
}

impl Journal {
    /// `str(journal)` = `J{pk:06d}`.
    pub fn display(&self) -> String {
        format!("J{:06}", self.number)
    }

    /// Entry number for manual originals; the J-number otherwise.
    pub fn display_number(&self) -> String {
        if self.entry_number.is_empty() { self.display() } else { self.entry_number.clone() }
    }

    /// `journal.manual`: no source document (including manual-entry reversals).
    pub fn manual(&self) -> bool {
        self.source_id.is_none()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalLine {
    pub id: JournalLineId,
    pub journal_id: JournalId,
    pub account_id: AccountId,
    pub position: i64,
    pub debit: Money,
    pub credit: Money,
    pub description: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AuditEvent {
    pub id: String,
    pub company_id: CompanyId,
    pub actor_id: UserId,
    pub actor_name: String,
    pub occurred_at: String,
    pub action: String,
    pub target: String,
    pub detail: serde_json::Value,
}
