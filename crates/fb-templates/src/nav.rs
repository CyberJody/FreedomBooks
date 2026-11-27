//! The one navigation definition every shell page renders: the sidebar, the tab strips on list pages,
//! the breadcrumb, and the Reports and Settings menus. Which sidebar entry is "you are here" comes from
//! the resolved route name alone, so every page agrees with the sidebar. URLs come from the same route
//! table as `url()`.
use crate::django::reverse;
use serde::Serialize;

/// Matches a resolved route (`invoices:new`, `register`, `desktop:mfa`).
enum Rule {
    /// Exactly these route names.
    Route(&'static [&'static str]),
    /// Every route of a namespace, minus some page names.
    Ns(&'static str, &'static [&'static str]),
    /// A directory (names list) route of one kind.
    Directory(&'static str),
    /// An account register: `true` for bank and credit card accounts, `false` for any other account.
    Register(bool),
}

/// Where a destination links to.
#[derive(Clone, Copy)]
enum Link {
    /// A company route (the company id is its first argument).
    Route(&'static str),
    /// A company directory list of one kind.
    Directory(&'static str),
    /// A company route plus a fragment on that page.
    Anchor(&'static str, &'static str),
    /// A desktop page outside the company route table.
    Path(&'static str),
}

/// Who sees an entry. The server-side checks stay where they are; this only hides what the viewer
/// could not open anyway.
#[derive(Clone, Copy, PartialEq)]
enum Access {
    All,
    /// Administrators only (company settings, users, book protection, backups).
    Admin,
    /// Builds and books with bank feeds turned on.
    BankFeeds,
    /// Administrators, in builds with the AI assistant.
    AdminAi,
    /// Builds with online invoice payments.
    OnlinePayments,
    /// Anyone who may save changes (not read-only users).
    Write,
}

/// One place: a sidebar entry, a tab on a list page, or a section of Reports, Settings or Home.
struct Dest {
    key: &'static str,
    label: &'static str,
    icon: &'static str,
    link: Link,
    rules: &'static [Rule],
    /// The final breadcrumb for a "new" page of this place (`New invoice`, `Receive payment`).
    new_label: &'static str,
    /// What one record of this place is called, for a record page's default crumb.
    record: &'static str,
    description: &'static str,
    access: Access,
}

const fn dest(key: &'static str, label: &'static str, icon: &'static str, link: Link, rules: &'static [Rule]) -> Dest {
    Dest { key, label, icon, link, rules, new_label: "", record: "", description: "", access: Access::All }
}

/// A sidebar entry, with the places that live under it.
struct Item {
    dest: Dest,
    /// Places under this entry. With `tabs`, they show as the tab strip on the list pages (the
    /// entry's own list first); otherwise they give the breadcrumb its middle segment.
    sections: &'static [(&'static str, &'static [Dest])],
    tabs: bool,
    /// Shows the number of downloaded bank transactions waiting for review.
    review_badge: bool,
}

const fn item(dest: Dest) -> Item {
    Item { dest, sections: &[], tabs: false, review_badge: false }
}

struct Group {
    label: &'static str,
    /// Where the group's breadcrumb segment links (the key of one of its entries).
    landing: &'static str,
    items: &'static [Item],
}

// ---- The definition --------------------------------------------------------------------------------

const HOME: Item = Item {
    dest: dest("company_home", "Home", "home", Link::Route("company_home"), &[Rule::Route(&["company_home"])]),
    sections: &[("", &[
        dest("reminders", "Reminders", "reminders", Link::Route("reminders:page"), &[Rule::Ns("reminders", &[])]),
        dest("find", "Find", "home", Link::Route("find"), &[Rule::Route(&["find"])]),
    ])],
    tabs: false,
    review_badge: false,
};

const INVOICE_TABS: &[Dest] = &[
    Dest { new_label: "New invoice", record: "Invoice", ..dest("invoices", "Invoices", "invoice", Link::Route("invoices:list"), &[Rule::Ns("invoices", &[])]) },
    Dest { new_label: "New estimate", record: "Estimate", ..dest("estimates", "Estimates", "estimate", Link::Route("sales:list"), &[Rule::Ns("sales", &[])]) },
    Dest { new_label: "New sales receipt", record: "Sales receipt", ..dest("sales_receipts", "Sales receipts", "sales-receipt", Link::Route("sales_receipts:list"), &[Rule::Ns("sales_receipts", &[])]) },
    Dest { new_label: "New credit memo", record: "Credit memo", ..dest("credits", "Credit memos", "credit-memo", Link::Route("credits:list"), &[Rule::Ns("credits", &[])]) },
];
const RECEIPT_TABS: &[Dest] = &[
    Dest { new_label: "Receive payment", record: "Payment", ..dest("receipts", "Payments received", "payment", Link::Route("receipts:list"), &[Rule::Ns("receipts", &[])]) },
    Dest { new_label: "New customer deposit", record: "Customer deposit", ..dest("customer_deposits", "Customer deposits", "customer-deposit", Link::Route("prepayments:list"), &[Rule::Ns("prepayments", &[])]) },
];
const JOB_TABS: &[Dest] = &[
    Dest { new_label: "New job ticket", record: "Job ticket", ..dest("job_tickets", "Job tickets", "estimate", Link::Route("jobs:list"), &[Rule::Ns("jobs", &[])]) },
    Dest { new_label: "Enter time", record: "Time entry", ..dest("time_entries", "Time entries", "history", Link::Route("time:list"), &[Rule::Ns("time", &[])]) },
];
const BILL_TABS: &[Dest] = &[
    Dest { new_label: "New bill", record: "Bill", ..dest("bills", "Bills list", "bill", Link::Route("bills:list"), &[Rule::Ns("bills", &["calendar"])]) },
    dest("bill_calendar", "Calendar", "calendar", Link::Route("bills:calendar"), &[Rule::Route(&["bills:calendar"])]),
    Dest { new_label: "New recurring bill", record: "Recurring bill", ..dest("recurring", "Recurring bills", "bill", Link::Route("recurring:list"), &[Rule::Ns("recurring", &[])]) },
];
const CHECK_TABS: &[Dest] = &[
    Dest { new_label: "Write check", record: "Check", ..dest("checks", "Checks & expenses", "check", Link::Route("checks:list"), &[Rule::Ns("checks", &[])]) },
    Dest { new_label: "New deposit", record: "Deposit", ..dest("deposits", "Deposits", "deposit", Link::Route("deposits:list"), &[Rule::Ns("deposits", &[])]) },
    Dest { new_label: "New transfer", record: "Transfer", ..dest("transfers", "Transfers", "transfer", Link::Route("transfers:list"), &[Rule::Ns("transfers", &[])]) },
];

const REPORTS: &[(&str, &[Dest])] = &[
    ("Financial statements", &[
        Dest { description: "Compare income and expenses by month, fiscal quarter, or fiscal year.", ..dest("comparative_profit_and_loss", "Comparative Profit and Loss", "profit-loss", Link::Route("reports:comparative_profit_and_loss"), &[Rule::Route(&["reports:comparative_profit_and_loss"])]) },
        Dest { description: "Cash receipts and payments by operating, investing, and financing activity.", ..dest("cash_flow", "Direct-Method Cash Flow", "profit-loss", Link::Route("reports:cash_flow"), &[Rule::Route(&["reports:cash_flow"])]) },
        Dest { description: "Income and expenses for a period, and the net result.", ..dest("profit_loss", "Profit and loss", "profit-loss", Link::Route("profit_loss"), &[Rule::Route(&["profit_loss"])]) },
        Dest { description: "What the company owns and owes, and its equity, on a date.", ..dest("balance_sheet", "Balance sheet", "balance-sheet", Link::Route("balance_sheet"), &[Rule::Route(&["balance_sheet"])]) },
    ]),
    ("Accounting detail", &[
        Dest { description: "Posted fixed-asset balances and an optional straight-line worksheet.", ..dest("asset_depreciation_schedule", "Fixed-Asset Depreciation Schedule", "general-ledger", Link::Route("reports:asset_depreciation_schedule"), &[Rule::Route(&["reports:asset_depreciation_schedule"])]) },
        Dest { description: "Every posted line, account by account, with running balances.", ..dest("general_ledger", "General ledger", "general-ledger", Link::Route("general_ledger"), &[Rule::Route(&["general_ledger"])]) },
        Dest { description: "Debit and credit balances of every account on a date.", ..dest("trial_balance", "Trial balance", "trial-balance", Link::Route("trial_balance"), &[Rule::Route(&["trial_balance"])]) },
        Dest { new_label: "New journal entry", record: "Journal entry", description: "Every journal entry, posted and draft. Make a journal entry from here.", ..dest("journals", "Journal entries", "journal", Link::Route("company"), &[Rule::Route(&["company", "journal_new", "journal", "journal_edit", "journal_post", "journal_reverse"])]) },
    ]),
    ("Customers and vendors", &[
        Dest { description: "Posted sales and credits grouped by customer, item, or both.", ..dest("sales_by_customer_item", "Sales by Customer and Item", "profit-loss", Link::Route("reports:sales_by_customer_item"), &[Rule::Route(&["reports:sales_by_customer_item"])]) },
        Dest { description: "Opening balances, dated activity, and closing balances for one customer.", ..dest("customer_statements", "Customer Statements", "payment", Link::Route("reports:customer_statements"), &[Rule::Route(&["reports:customer_statements"])]) },
        Dest { description: "Open invoices and customer credits by days past due.", ..dest("ar_aging", "A/R Aging Summary", "payment", Link::Route("ar_aging"), &[Rule::Route(&["ar_aging"])]) },
        Dest { description: "Open bills by vendor and days past due.", ..dest("ap_aging", "A/P Aging Summary", "bill", Link::Route("ap_aging"), &[Rule::Route(&["ap_aging"])]) },
        Dest { description: "Payments by vendor for a tax year or date range.", ..dest("vendor_1099", "Vendor 1099 Summary", "check", Link::Route("vendor_1099"), &[Rule::Route(&["vendor_1099"])]) },
    ]),
    ("Tax preparation", &[
        Dest { description: "Assign book income and expenses to business activities and categories.", ..dest("schedule_c_activity", "Schedule C Activity Worksheet", "profit-loss", Link::Route("reports:schedule_c_activity"), &[Rule::Route(&["reports:schedule_c_activity"])]) },
    ]),
    ("Sales tax", &[
        Dest { description: "What you owe by agency, with sales by rate for a filing period.", ..dest("tax_owed", "Sales tax owed", "sales-tax", Link::Route("salestax:what_you_owe"), &[]) },
    ]),
    ("Activity", &[
        Dest { description: "Who changed what, and when.", ..dest("history", "Audit history", "history", Link::Route("history"), &[Rule::Route(&["history"])]) },
    ]),
];

/// The Settings menu: the hub page lists it, and every settings page shows it down its side.
pub const SETTINGS_SECURITY: &str = "Security and data";
const SETTINGS: &[(&str, &[Dest])] = &[
    ("Company", &[
        Dest { access: Access::Admin, description: "Name, address, contact details, tax ID and logo.", ..dest("company_profile", "Company profile", "company-settings", Link::Route("company_edit"), &[Rule::Route(&["company_edit"])]) },
        Dest { access: Access::Admin, description: "The next number for invoices, bills, checks and other documents.", ..dest("numbering", "Document numbering", "company-settings", Link::Anchor("company_edit", "numbering-title"), &[]) },
        Dest { description: "Which accounts hold receivables, payables, undeposited funds and sales tax, and which are bank accounts.", ..dest("accounting_setup", "Accounting setup", "setup", Link::Route("documents:setup"), &[Rule::Ns("documents", &[]), Rule::Route(&["setup_start", "setup_fill", "setup_done"])]) },
        Dest { description: "Fiscal-year start and the date through which the books are closed.", ..dest("accounting_periods", "Accounting periods", "setup", Link::Route("company:periods"), &[Rule::Route(&["company:periods"])]) },
        Dest { description: "Whether you charge sales tax, your tax rates and tax groups.", ..dest("sales_tax", "Sales tax and rates", "sales-tax", Link::Route("salestax:settings"), &[Rule::Ns("salestax", &[])]) },
    ]),
    ("Money in and out", &[
        Dest { access: Access::Admin, description: "Invoice wording, payment instructions and the email account invoices are sent from.", ..dest("invoice_email", "Invoices and email", "invoice", Link::Anchor("company_edit", "invoices-and-email"), &[]) },
        Dest { access: Access::OnlinePayments, description: "Card payments for invoices through Stripe.", ..dest("online_payments", "Online payments", "payment", Link::Route("stripe:payments"), &[Rule::Ns("stripe", &[])]) },
        Dest { access: Access::BankFeeds, description: "Bank connections, and the rules that sort downloaded transactions.", ..dest("bank_feeds", "Bank feeds and rules", "bank-feed", Link::Route("bankfeeds:list"), &[Rule::Ns("bankfeeds", &["review"])]) },
    ]),
    (SETTINGS_SECURITY, &[
        Dest { access: Access::Admin, description: "Who can sign in to these books, and what each person may do.", ..dest("users", "Users", "customers", Link::Path("/desktop/users/"), &[Rule::Route(&["desktop:users"])]) },
        Dest { description: "Your user password and sign-in codes from an authenticator app.", ..dest("my_signin", "My sign-in and MFA", "company-settings", Link::Path("/desktop/mfa/"), &[Rule::Route(&["desktop:mfa", "desktop:session"])]) },
        Dest { access: Access::Admin, description: "The password that protects the book file, and its recovery key.", ..dest("protection", "Book protection", "company-settings", Link::Path("/settings/book-protection/"), &[Rule::Route(&["desktop:protection", "desktop:recovery", "desktop:kit"])]) },
        Dest { access: Access::Admin, description: "Make and restore verified backups of all companies.", ..dest("backups", "Backups", "backup", Link::Route("backups:page"), &[Rule::Ns("backups", &[]), Rule::Route(&["desktop:restore"])]) },
        Dest { access: Access::AdminAi, description: "Let an AI assistant read or record work in this company.", ..dest("ai_access", "AI access", "ai-assistant", Link::Route("assistant:page"), &[Rule::Ns("assistant", &[])]) },
    ]),
];

const GROUPS: &[Group] = &[
    Group { label: "Expenses", landing: "expenses", items: &[
        item(dest("expenses", "Expenses", "bill", Link::Route("expenses"), &[Rule::Route(&["expenses"])])),
    ]},
    Group { label: "Customers", landing: "customers", items: &[
        item(Dest { new_label: "New customer", record: "Customer", ..dest("customers", "Customers", "customers", Link::Directory("customers"), &[Rule::Directory("customers")]) }),
        Item { sections: &[("", INVOICE_TABS)], tabs: true, ..item(dest("invoices", "Invoices", "invoice", Link::Route("invoices:list"), &[])) },
        Item { sections: &[("", RECEIPT_TABS)], tabs: true, ..item(dest("receipts", "Payments received", "payment", Link::Route("receipts:list"), &[])) },
        Item { sections: &[("", JOB_TABS)], tabs: true, ..item(dest("job_tickets", "Job tickets", "estimate", Link::Route("jobs:list"), &[])) },
    ]},
    Group { label: "Vendors", landing: "vendors", items: &[
        item(Dest { new_label: "New vendor", record: "Vendor", ..dest("vendors", "Vendors", "vendors", Link::Directory("vendors"), &[Rule::Directory("vendors")]) }),
        Item { sections: &[("", BILL_TABS)], tabs: true, ..item(dest("bills", "Bills", "bill", Link::Route("bills:list"), &[])) },
        item(Dest { new_label: "Pay bills", record: "Bill payment", ..dest("payments", "Bill payments", "pay-bill", Link::Route("payments:list"), &[Rule::Ns("payments", &[])]) }),
    ]},
    Group { label: "Banking", landing: "registers", items: &[
        Item { review_badge: true, ..item(Dest { access: Access::BankFeeds, ..dest("bank_review", "Review transactions", "bank-feed", Link::Route("bankfeeds:review"), &[Rule::Route(&["bankfeeds:review"])]) }) },
        item(Dest { record: "Register", ..dest("registers", "Bank accounts", "register", Link::Route("registers:list"), &[Rule::Ns("registers", &[]), Rule::Register(true)]) }),
        item(dest("reconcile", "Reconcile", "reconcile", Link::Route("reconcile:list"), &[Rule::Ns("reconcile", &[]), Rule::Route(&["reconcile:report"])])),
        Item { sections: &[("", CHECK_TABS)], tabs: true, ..item(dest("checks", "Checks & expenses", "check", Link::Route("checks:list"), &[])) },
    ]},
    Group { label: "Reports", landing: "reports", items: &[
        Item { sections: REPORTS, ..item(dest("reports", "Reports", "profit-loss", Link::Route("reports"), &[Rule::Route(&["reports"])])) },
    ]},
    Group { label: "Lists", landing: "accounts", items: &[
        item(Dest { new_label: "New account", record: "Account", ..dest("accounts", "Chart of accounts", "accounts", Link::Route("accounts"), &[Rule::Route(&["accounts", "account_new", "account_edit"]), Rule::Register(false)]) }),
        item(Dest { new_label: "New item", record: "Item", ..dest("items", "Items & services", "items", Link::Directory("items"), &[Rule::Directory("items")]) }),
    ]},
];

/// The title bar's "+ New" menu: every kind of record people make all day, by area. Each opens the
/// existing "new" page.
const NEW_MENU: &[(&str, &[Dest])] = &[
    ("Customers", &[
        Dest { access: Access::Write, ..dest("new_invoice", "Invoice", "invoice", Link::Route("invoices:new"), &[]) },
        Dest { access: Access::Write, ..dest("new_payment", "Receive payment", "payment", Link::Route("receipts:new"), &[]) },
        Dest { access: Access::Write, ..dest("new_sales_receipt", "Sales receipt", "sales-receipt", Link::Route("sales_receipts:new"), &[]) },
        Dest { access: Access::Write, ..dest("new_estimate", "Estimate", "estimate", Link::Route("sales:new"), &[]) },
        Dest { access: Access::Write, ..dest("new_credit_memo", "Credit memo", "credit-memo", Link::Route("credits:new"), &[]) },
        Dest { access: Access::Write, ..dest("new_job_ticket", "Job ticket", "estimate", Link::Route("jobs:new"), &[]) },
        Dest { access: Access::Write, ..dest("new_time_entry", "Enter time", "history", Link::Route("time:new"), &[]) },
    ]),
    ("Vendors", &[
        Dest { access: Access::Write, ..dest("new_bill", "Bill", "bill", Link::Route("bills:new"), &[]) },
        Dest { access: Access::Write, ..dest("new_bill_payment", "Pay bills", "pay-bill", Link::Route("payments:new"), &[]) },
    ]),
    ("Banking", &[
        Dest { access: Access::Write, ..dest("new_check", "Write check", "check", Link::Route("checks:new"), &[]) },
        Dest { access: Access::Write, ..dest("new_deposit", "Deposit", "deposit", Link::Route("deposits:new"), &[]) },
        Dest { access: Access::Write, ..dest("new_transfer", "Transfer", "transfer", Link::Route("transfers:new"), &[]) },
    ]),
    ("Accounting", &[
        Dest { access: Access::Write, ..dest("new_journal", "Journal entry", "journal", Link::Route("journal_new"), &[]) },
    ]),
];

const SETTINGS_ITEM: Item = Item {
    sections: SETTINGS,
    ..item(dest("settings", "Settings", "company-settings", Link::Route("settings"), &[Rule::Route(&["settings"])]))
};

// ---- What a page receives --------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct NavEntry {
    pub key: String,
    pub label: String,
    pub icon: String,
    pub url: String,
    pub current: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub badge: Option<i64>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct NavGroup {
    pub label: String,
    pub items: Vec<NavEntry>,
}

/// One breadcrumb segment. `current` marks the page itself (rendered without a link).
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Crumb {
    pub label: String,
    pub url: String,
    pub current: bool,
}

#[derive(Clone, Debug, Serialize, PartialEq, Default)]
pub struct Nav {
    pub companies_current: bool,
    pub home: Option<NavEntry>,
    pub groups: Vec<NavGroup>,
    /// Pinned at the bottom of the sidebar.
    pub settings: Option<NavEntry>,
    /// The highlighted sidebar entry.
    pub current: Option<NavEntry>,
    pub current_group: Option<String>,
    /// Breadcrumb segments above the page. When the page is itself one of these places, its segment is
    /// the last one, with `current` set, and `leaf` is empty.
    pub crumbs: Vec<Crumb>,
    /// The default last crumb for a page below a place (`New bill`, `Invoice`); pages may replace it.
    pub leaf: String,
    /// The tab strip for the current entry's list pages (empty when it has none).
    pub tabs: Vec<NavEntry>,
    /// The Settings menu (only on Settings pages) or the report list (on Reports pages).
    pub menu: Vec<NavGroup>,
    /// True on Settings pages other than the hub: the base layout shows the Settings menu beside them.
    pub settings_frame: bool,
    /// The heading icon: the place's own icon (a tab or section), else the sidebar entry's.
    pub icon: String,
    /// The viewer can record changes in their companies.
    pub write: bool,
    /// The viewer is an administrator (Create company in the company menu).
    pub admin: bool,
    /// The title bar's "+ New" menu, without the entries the viewer may not use (empty for read-only
    /// users, and outside a company).
    pub new_menu: Vec<NavGroup>,
}

/// Where the request is: the resolved route name (`invoices:new`, `register`, `desktop:mfa`) and its
/// `kind` argument (the directory kind, or `bank` / `other` for an account register).
#[derive(Clone, Debug, Default)]
pub struct Current<'a> {
    pub route: &'a str,
    pub kind: Option<&'a str>,
}

/// What the viewer may see.
#[derive(Clone, Copy, Debug, Default)]
pub struct Viewer {
    pub admin: bool,
    /// May save changes (administrators and bookkeepers; not read-only users).
    pub write: bool,
    pub bankfeeds: bool,
    pub ai: bool,
    pub online_payments: bool,
    /// Downloaded bank transactions waiting for review (the Review transactions badge).
    pub to_review: i64,
}

impl Access {
    fn allows(self, v: &Viewer) -> bool {
        match self {
            Access::All => true,
            Access::Admin => v.admin,
            Access::BankFeeds => v.bankfeeds,
            Access::AdminAi => v.admin && v.ai,
            Access::OnlinePayments => v.online_payments,
            Access::Write => v.write,
        }
    }
}

impl Rule {
    fn matches(&self, route: &str, kind: Option<&str>) -> bool {
        let (ns, page) = route.split_once(':').unwrap_or(("", route));
        match self {
            Rule::Route(names) => names.contains(&route),
            Rule::Ns(name, exclude) => ns == *name && !exclude.contains(&page),
            Rule::Directory(k) => ns == "directory" && kind == Some(*k),
            Rule::Register(bank) => route == "register" && (kind == Some("bank")) == *bank,
        }
    }
}

impl Dest {
    fn matches(&self, c: &Current<'_>) -> bool {
        self.rules.iter().any(|r| r.matches(c.route, c.kind))
    }
    fn url(&self, company: &str) -> Result<String, String> {
        match self.link {
            Link::Route(name) => reverse(name, &[company.to_owned()]),
            Link::Directory(kind) => reverse("directory:list", &[company.to_owned(), kind.to_owned()]),
            Link::Anchor(name, fragment) => Ok(format!("{}#{fragment}", reverse(name, &[company.to_owned()])?)),
            Link::Path(path) => Ok(path.to_owned()),
        }
    }
    /// The route this place's own link opens (`None` for a desktop page or an anchor).
    fn own_route(&self) -> Option<&'static str> {
        match self.link {
            Link::Route(name) => Some(name),
            Link::Directory(_) => Some("directory:list"),
            Link::Anchor(..) | Link::Path(_) => None,
        }
    }
    fn is_own_page(&self, c: &Current<'_>) -> bool {
        match self.link {
            Link::Directory(kind) => c.route == "directory:list" && c.kind == Some(kind),
            Link::Path(path) => desktop_route(path) == Some(c.route),
            _ => self.own_route() == Some(c.route),
        }
    }
    fn entry(&self, company: &str, current: bool) -> Result<NavEntry, String> {
        Ok(NavEntry {
            key: self.key.into(),
            label: self.label.into(),
            icon: self.icon.into(),
            url: self.url(company)?,
            current,
            badge: None,
            description: self.description.into(),
        })
    }
}

impl Item {
    fn all_sections(&self) -> impl Iterator<Item = &Dest> {
        self.sections.iter().flat_map(|(_, d)| d.iter())
    }
    fn matches(&self, c: &Current<'_>) -> bool {
        self.dest.matches(c) || self.all_sections().any(|d| d.matches(c))
    }
}

/// The route names the desktop (non-company) pages are known by in the navigation.
pub fn desktop_route(path: &str) -> Option<&'static str> {
    Some(match path {
        "/desktop/users/" => "desktop:users",
        "/desktop/mfa/" => "desktop:mfa",
        "/desktop/session/" => "desktop:session",
        "/settings/book-protection/" => "desktop:protection",
        "/settings/recovery-key/" => "desktop:recovery",
        "/settings/emergency-kit/" => "desktop:kit",
        "/settings/restore-backup/" => "desktop:restore",
        _ => return None,
    })
}

/// The default last crumb of a page below a place, from the page part of its route name.
fn leaf_label(place: &Dest, route: &str) -> String {
    const NAMED: &[(&str, &str)] = &[
        ("salestax:list", "Tax rates"), ("salestax:rate_new", "New tax rate"), ("salestax:rate_edit", "Edit tax rate"),
        ("salestax:group_new", "New tax group"), ("salestax:group_edit", "Edit tax group"), ("salestax:setup", "Guided setup"),
        ("salestax:bulk_step", "Turn on sales tax"), ("bankfeeds:rules", "Bank rules"), ("bankfeeds:rule_new", "New rule"),
        ("bankfeeds:rule_edit", "Edit rule"), ("bankfeeds:connect", "Connect a bank"), ("bankfeeds:accounts", "Bank feed accounts"),
        ("bankfeeds:forget", "Forget connection"), ("reconcile:begin", "Begin reconciliation"), ("reconcile:screen", "Reconcile account"),
        ("reconcile:history", "Reconciliation history"), ("reconcile:report", "Reconciliation report"), ("reconcile:undo", "Undo reconciliation"),
        ("credits:apply", "Apply credit"), ("credits:unapply", "Unapply credit"), ("credits:refund", "Refund"), ("credits:refund_detail", "Refund"),
        ("credits:refund_void", "Void refund"), ("prepayments:apply", "Apply deposit"), ("prepayments:invoice_apply", "Apply deposit"),
        ("prepayments:unapply", "Unapply deposit"), ("prepayments:refund", "Refund"), ("prepayments:refund_detail", "Refund"),
        ("prepayments:refund_void", "Void refund"), ("invoices:convert", "Convert to invoice"), ("invoices:email_preview", "Email preview"),
        ("sales:state", "Change status"), ("checks:number", "Check number"), ("registers:cleared", "Cleared"),
        ("reminders:settings", "Reminder settings"), ("backups:create", "Create backup"), ("directory:status", "Change status"),
    ];
    if let Some((_, label)) = NAMED.iter().find(|(r, _)| *r == route) {
        return (*label).into();
    }
    let page = route.rsplit_once(':').map_or(route, |(_, p)| p);
    let record = if place.record.is_empty() { place.label } else { place.record };
    match page {
        "new" | "journal_new" | "account_new" if !place.new_label.is_empty() => place.new_label.into(),
        "detail" | "journal" | "register" => record.into(),
        "edit" | "journal_edit" | "account_edit" => format!("Edit {}", record.to_lowercase()),
        "revision" | "print_revision" => "Revision".into(),
        "void" => "Void".into(),
        "reverse" | "journal_reverse" => "Reverse".into(),
        "review" => "Review".into(),
        "print" => "Print".into(),
        "settings" => "Settings".into(),
        "setup_start" | "setup_fill" | "setup_done" => "Finish setting up".into(),
        "session" => "Your session".into(),
        "recovery" => "Recovery key".into(),
        "kit" => "Emergency kit".into(),
        "restore" => "Restore a backup".into(),
        other => {
            let mut words = other.replace('_', " ");
            if let Some(first) = words.get(..1) {
                words.replace_range(..1, &first.to_uppercase());
            }
            words
        }
    }
}

/// The navigation for one page. `company_id` is the open company (None on the Companies page and
/// before sign-in); `current` is where the request is.
pub fn build(company_id: Option<&str>, current: &Current<'_>, viewer: &Viewer) -> Result<Nav, String> {
    let mut nav = Nav { companies_current: company_id.is_none(), admin: viewer.admin, write: viewer.write, ..Nav::default() };
    let Some(company) = company_id else { return Ok(nav) };
    let visible = |d: &Dest| d.access.allows(viewer);
    for (label, dests) in NEW_MENU {
        let items = dests.iter().filter(|d| visible(d)).map(|d| d.entry(company, false)).collect::<Result<Vec<_>, _>>()?;
        if !items.is_empty() {
            nav.new_menu.push(NavGroup { label: (*label).into(), items });
        }
    }

    // Sidebar: the first entry that claims the route is "you are here" (the route-table test checks
    // that exactly one does).
    let mut here: Option<(&Item, Option<&Group>)> = None;
    let mut sidebar_entry = |it: &'static Item, group: Option<&'static Group>| -> Result<NavEntry, String> {
        let current_here = here.is_none() && visible(&it.dest) && it.matches(current);
        if current_here {
            here = Some((it, group));
        }
        let mut e = it.dest.entry(company, current_here)?;
        if it.review_badge && viewer.to_review > 0 {
            e.badge = Some(viewer.to_review);
        }
        Ok(e)
    };
    nav.home = Some(sidebar_entry(&HOME, None)?);
    for group in GROUPS {
        let mut items = Vec::new();
        for it in group.items.iter().filter(|it| visible(&it.dest)) {
            items.push(sidebar_entry(it, Some(group))?);
        }
        if !items.is_empty() {
            nav.groups.push(NavGroup { label: group.label.into(), items });
        }
    }
    nav.settings = Some(sidebar_entry(&SETTINGS_ITEM, None)?);
    let current_key = here.map(|(it, _)| it.dest.key);
    nav.current = nav.home.iter()
        .chain(nav.groups.iter().flat_map(|g| g.items.iter()))
        .chain(nav.settings.iter())
        .find(|e| Some(e.key.as_str()) == current_key)
        .cloned();
    let Some((item, group)) = here else { return Ok(nav) };
    nav.current_group = Some(group.map_or(item.dest.label, |g| g.label).to_owned());

    // Breadcrumb: Area › Place › page. The area segment is left out when it names the place itself
    // (Customers › Customers), and so is the sidebar entry when a tab under it is the place.
    let section = item.all_sections().find(|d| visible(d) && d.matches(current));
    if let Some(g) = group.filter(|g| g.label != item.dest.label)
        && let Some(landing) = g.items.iter().find(|it| it.dest.key == g.landing)
    {
        nav.crumbs.push(Crumb { label: g.label.into(), url: landing.dest.url(company)?, current: false });
    }
    let place = match section {
        Some(s) if item.tabs => s,
        _ => &item.dest,
    };
    nav.icon = section.unwrap_or(place).icon.into();
    // The first tab is the entry's own list ("Bills list" on the tab strip); the breadcrumb uses the entry's name.
    let place_label = if item.tabs && item.sections.first().and_then(|(_, d)| d.first()).is_some_and(|d| d.key == place.key) { item.dest.label } else { place.label };
    nav.crumbs.push(Crumb { label: place_label.into(), url: place.url(company)?, current: place.is_own_page(current) });
    if !item.tabs && let Some(s) = section {
        nav.crumbs.push(Crumb { label: s.label.into(), url: s.url(company)?, current: s.is_own_page(current) });
    }
    if !nav.crumbs.last().is_some_and(|c| c.current) {
        nav.leaf = leaf_label(section.filter(|_| !item.tabs).unwrap_or(place), current.route);
    }

    // Tabs on the list pages, and the Reports or Settings menu.
    if item.tabs {
        for (_, dests) in item.sections {
            for d in dests.iter().filter(|d| visible(d)) {
                nav.tabs.push(d.entry(company, d.key == place.key)?);
            }
        }
    } else if !item.sections.is_empty() && item.dest.key != "company_home" {
        for (label, dests) in item.sections {
            let items = dests
                .iter()
                .filter(|d| visible(d))
                .map(|d| d.entry(company, section.is_some_and(|s| s.key == d.key)))
                .collect::<Result<Vec<_>, _>>()?;
            if !items.is_empty() {
                nav.menu.push(NavGroup { label: (*label).into(), items });
            }
        }
        nav.settings_frame = item.dest.key == "settings" && current.route != "settings";
    }
    Ok(nav)
}

/// The keys of the Settings entries a viewer sees, in menu order (for tests and the hub).
pub fn settings_keys(viewer: &Viewer) -> Vec<&'static str> {
    SETTINGS.iter().flat_map(|(_, d)| d.iter()).filter(|d| d.access.allows(viewer)).map(|d| d.key).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: Viewer = Viewer { admin: true, write: true, bankfeeds: true, ai: true, online_payments: true, to_review: 0 };

    fn at(route: &str, kind: Option<&str>) -> Nav {
        build(Some("c1"), &Current { route, kind }, &ALL).unwrap()
    }
    fn crumbs(nav: &Nav) -> Vec<String> {
        let mut out: Vec<String> = nav.crumbs.iter().map(|c| if c.current { format!("[{}]", c.label) } else { c.label.clone() }).collect();
        if !nav.leaf.is_empty() {
            out.push(format!("[{}]", nav.leaf));
        }
        out
    }

    #[test]
    fn the_sidebar_is_short_and_settings_is_pinned() {
        let nav = at("company_home", None);
        let labels: Vec<_> = nav.groups.iter().map(|g| (g.label.as_str(), g.items.iter().map(|i| i.label.as_str()).collect::<Vec<_>>())).collect();
        assert_eq!(labels, vec![
            ("Expenses", vec!["Expenses"]),
            ("Customers", vec!["Customers", "Invoices", "Payments received", "Job tickets"]),
            ("Vendors", vec!["Vendors", "Bills", "Bill payments"]),
            ("Banking", vec!["Review transactions", "Bank accounts", "Reconcile", "Checks & expenses"]),
            ("Reports", vec!["Reports"]),
            ("Lists", vec!["Chart of accounts", "Items & services"]),
        ]);
        let count = 2 + nav.groups.iter().map(|g| g.items.len()).sum::<usize>();
        assert_eq!(count, 17, "Home, 15 entries in six groups, Settings");
        assert_eq!(nav.settings.as_ref().unwrap().url, "/c/c1/settings/");
        assert_eq!(nav.current.unwrap().key, "company_home");
        assert!(nav.crumbs.len() == 1 && nav.crumbs[0].current && nav.leaf.is_empty());
    }

    #[test]
    fn job_tickets_and_time_live_under_customers_with_their_own_tabs() {
        for (route, tab, leaf) in [("jobs:list", "Job tickets", ""), ("jobs:new", "Job tickets", "New job ticket"), ("jobs:detail", "Job tickets", "Job ticket"),
            ("time:list", "Time entries", ""), ("time:new", "Time entries", "Enter time"), ("time:edit", "Time entries", "Edit time entry")] {
            let n = at(route, None);
            assert_eq!((n.current.as_ref().unwrap().key.as_str(), n.current_group.as_deref()), ("job_tickets", Some("Customers")), "{route}");
            assert_eq!(n.tabs.iter().map(|t| t.label.as_str()).collect::<Vec<_>>(), ["Job tickets", "Time entries"], "{route}");
            assert_eq!(n.tabs.iter().find(|t| t.current).unwrap().label, tab, "{route}");
            assert_eq!(n.leaf, leaf, "{route}");
        }
    }

    #[test]
    fn you_are_here_and_the_breadcrumb_follow_the_route() {
        let n = at("invoices:new", None);
        assert_eq!((n.current.as_ref().unwrap().key.as_str(), n.current_group.as_deref()), ("invoices", Some("Customers")));
        assert_eq!(crumbs(&n), ["Customers", "Invoices", "[New invoice]"]);
        assert_eq!(n.crumbs[0].url, "/c/c1/lists/customers/");
        assert_eq!(n.tabs.iter().map(|t| t.label.as_str()).collect::<Vec<_>>(), ["Invoices", "Estimates", "Sales receipts", "Credit memos"]);
        assert_eq!(crumbs(&at("invoices:list", None)), ["Customers", "[Invoices]"]);
        let est = at("sales:detail", None);
        assert_eq!(est.current.as_ref().unwrap().key, "invoices");
        assert_eq!(crumbs(&est), ["Customers", "Estimates", "[Estimate]"]);
        assert!(est.tabs.iter().find(|t| t.key == "estimates").unwrap().current);
        assert_eq!(crumbs(&at("directory:list", Some("customers"))), ["[Customers]"]);
        assert_eq!(crumbs(&at("directory:detail", Some("customers"))), ["Customers", "[Customer]"]);
        assert_eq!(crumbs(&at("bills:new", None)), ["Vendors", "Bills", "[New bill]"]);
        assert_eq!(crumbs(&at("bills:calendar", None)), ["Vendors", "[Calendar]"]);
        assert_eq!(crumbs(&at("receipts:new", None)), ["Customers", "Payments received", "[Receive payment]"]);
        assert_eq!(crumbs(&at("payments:new", None)), ["Vendors", "Bill payments", "[Pay bills]"]);
        assert_eq!(crumbs(&at("checks:new", None)), ["Banking", "Checks & expenses", "[Write check]"]);
        // A bank register is under Bank accounts; any other account's register is under the chart.
        let reg = at("register", Some("bank"));
        assert_eq!(reg.current.as_ref().unwrap().key, "registers");
        assert_eq!(crumbs(&reg), ["Banking", "Bank accounts", "[Register]"]);
        assert_eq!(at("register", Some("other")).current.unwrap().key, "accounts");
        assert_eq!(crumbs(&at("profit_loss", None)), ["Reports", "[Profit and loss]"]);
        assert_eq!(crumbs(&at("journal_new", None)), ["Reports", "Journal entries", "[New journal entry]"]);
        assert_eq!(crumbs(&at("company", None)), ["Reports", "[Journal entries]"]);
        assert_eq!(crumbs(&at("reports", None)), ["[Reports]"]);
        assert_eq!(crumbs(&at("reminders:page", None)), ["Home", "[Reminders]"]);
        assert_eq!(crumbs(&at("desktop:protection", None)), ["Settings", "[Book protection]"]);
        assert_eq!(crumbs(&at("salestax:rate_new", None)), ["Settings", "Sales tax and rates", "[New tax rate]"]);
        assert_eq!(crumbs(&at("company_edit", None)), ["Settings", "[Company profile]"]);
        assert_eq!(crumbs(&at("setup_start", None)), ["Settings", "Accounting setup", "[Finish setting up]"]);
        assert_eq!(at("stripe:payments", None).current.unwrap().key, "settings");
        let s = at("desktop:users", None);
        assert!(s.settings_frame && s.menu.iter().any(|g| g.label == SETTINGS_SECURITY));
        assert!(s.menu.iter().flat_map(|g| &g.items).find(|i| i.key == "users").unwrap().current);
        let hub = at("settings", None);
        assert!(!hub.settings_frame && hub.menu.len() == 3);
    }

    #[test]
    fn entries_the_viewer_cannot_open_are_hidden() {
        let bookkeeper = Viewer { to_review: 3, bankfeeds: true, ..Viewer::default() };
        assert_eq!(settings_keys(&bookkeeper), ["accounting_setup", "accounting_periods", "sales_tax", "bank_feeds", "my_signin"]);
        let nav = build(Some("c1"), &Current { route: "company_home", kind: None }, &bookkeeper).unwrap();
        let banking = nav.groups.iter().find(|g| g.label == "Banking").unwrap();
        let review = banking.items.iter().find(|i| i.key == "bank_review").unwrap();
        assert_eq!(review.badge, Some(3));
        let release = build(Some("c1"), &Current { route: "bankfeeds:review", kind: None }, &Viewer::default()).unwrap();
        assert!(release.groups.iter().flat_map(|g| &g.items).all(|i| i.key != "bank_review"));
        assert!(release.current.is_none(), "a hidden entry is never the current one");
        let none = build(None, &Current::default(), &ALL).unwrap();
        assert!(none.companies_current && none.groups.is_empty() && none.settings.is_none());
    }

    #[test]
    fn the_new_menu_offers_every_kind_of_record_to_those_who_may_make_them() {
        let menu = |viewer: &Viewer| build(Some("c1"), &Current { route: "company_home", kind: None }, viewer).unwrap().new_menu;
        let all = menu(&ALL);
        let labels: Vec<_> = all.iter().map(|g| (g.label.as_str(), g.items.iter().map(|i| i.label.as_str()).collect::<Vec<_>>())).collect();
        assert_eq!(labels, vec![
            ("Customers", vec!["Invoice", "Receive payment", "Sales receipt", "Estimate", "Credit memo", "Job ticket", "Enter time"]),
            ("Vendors", vec!["Bill", "Pay bills"]),
            ("Banking", vec!["Write check", "Deposit", "Transfer"]),
            ("Accounting", vec!["Journal entry"]),
        ]);
        let urls: Vec<_> = all.iter().flat_map(|g| &g.items).map(|i| i.url.as_str()).collect();
        assert_eq!(urls, ["/c/c1/invoices/new/", "/c/c1/receipts/new/", "/c/c1/sales-receipts/new/", "/c/c1/estimates/new/", "/c/c1/credit-memos/new/", "/c/c1/job-tickets/new/", "/c/c1/time/new/",
            "/c/c1/bills/new/", "/c/c1/payments/new/", "/c/c1/checks/new/", "/c/c1/deposits/new/", "/c/c1/transfers/new/", "/c/c1/journals/new/"]);
        // Each entry's page is a "new" page whose breadcrumb names what it makes.
        let leaves: Vec<String> = all.iter().flat_map(|g| &g.items).map(|i| {
            let route = crate::django::route_table().iter().find(|(_, p)| p.replace("<int:company_id>", "c1") == i.url).map(|(n, _)| n.clone()).unwrap();
            let n = at(&route, None);
            n.leaf
        }).collect();
        assert_eq!(leaves, ["New invoice", "Receive payment", "New sales receipt", "New estimate", "New credit memo", "New job ticket", "Enter time", "New bill", "Pay bills", "Write check", "New deposit", "New transfer", "New journal entry"]);
        // A bookkeeper sees all of it; a read-only user, and the Companies page, none.
        assert_eq!(menu(&Viewer { write: true, ..Viewer::default() }), all);
        assert!(menu(&Viewer { admin: false, write: false, bankfeeds: true, ai: true, online_payments: true, to_review: 0 }).is_empty());
        assert!(build(None, &Current::default(), &ALL).unwrap().new_menu.is_empty());
        // Find sits under Home in the breadcrumb.
        let find = at("find", None);
        assert_eq!(find.current.as_ref().unwrap().key, "company_home");
        assert_eq!(crumbs(&find), ["Home", "[Find]"]);
    }

    /// Every company route in the route table resolves to exactly one highlighted sidebar entry, so
    /// "you are here" cannot drift when routes are added.
    #[test]
    fn every_company_route_highlights_exactly_one_entry() {
        // Endpoints that never render a page of their own (downloads and posted actions on a record
        // shown elsewhere).
        const NOT_PAGES: &[&str] = &["attachments:"];
        let mut checked = 0;
        for (name, pattern) in crate::django::route_table() {
            if !pattern.starts_with("/c/") || NOT_PAGES.iter().any(|p| name.starts_with(p)) {
                continue;
            }
            let kinds: &[Option<&str>] = if name.starts_with("directory:") {
                &[Some("customers"), Some("vendors"), Some("items")]
            } else if name == "register" {
                &[Some("bank"), Some("other")]
            } else {
                &[None]
            };
            for kind in kinds {
                let c = Current { route: name, kind: *kind };
                let mut claims: Vec<&str> = std::iter::once(&HOME).chain(GROUPS.iter().flat_map(|g| g.items.iter())).chain(std::iter::once(&SETTINGS_ITEM))
                    .filter(|it| it.matches(&c)).map(|it| it.dest.key).collect();
                claims.dedup();
                assert_eq!(claims.len(), 1, "{name} {kind:?} is claimed by {claims:?}");
                let nav = build(Some("c1"), &c, &ALL).unwrap();
                let highlighted: Vec<_> = nav.home.iter().chain(nav.groups.iter().flat_map(|g| g.items.iter())).chain(nav.settings.iter()).filter(|e| e.current).collect();
                assert_eq!(highlighted.len(), 1, "{name} {kind:?}");
                assert!(!nav.crumbs.is_empty(), "{name} has a breadcrumb");
                checked += 1;
            }
        }
        for desktop in ["/desktop/users/", "/desktop/mfa/", "/desktop/session/", "/settings/book-protection/", "/settings/recovery-key/", "/settings/emergency-kit/", "/settings/restore-backup/"] {
            let nav = build(Some("c1"), &Current { route: desktop_route(desktop).unwrap(), kind: None }, &ALL).unwrap();
            assert_eq!(nav.current.map(|c| c.key).as_deref(), Some("settings"), "{desktop}");
        }
        assert!(checked > 140, "{checked}");
    }
}
