//! Port of `ledger/navigation.py`: the one navigation definition every shell page renders. Entries,
//! icons, order and the `aria-current` rules are copied from the frozen app; URLs come from the same
//! route table as `url()`.
use crate::django::reverse;
use serde::Serialize;

enum Rule {
    /// Pages of the ledger app (no URL namespace).
    Root(&'static [&'static str]),
    /// Every page of a namespace, minus some page names.
    Ns(&'static str, &'static [&'static str]),
    /// A directory list of one kind.
    Directory(&'static str),
    /// Exactly one namespaced page.
    Page(&'static str, &'static str),
}

struct Item {
    key: &'static str,
    label: &'static str,
    icon: &'static str,
    url_name: &'static str,
    rule: Rule,
    kind: Option<&'static str>,
    staff_only: bool,
    badge: bool,
}

const fn item(key: &'static str, label: &'static str, icon: &'static str, url_name: &'static str, rule: Rule) -> Item {
    Item { key, label, icon, url_name, rule, kind: None, staff_only: false, badge: false }
}

const GROUPS: &[(&str, &str, &[Item])] = &[
    ("Customers", "Customer work", &[
        Item { kind: Some("customers"), ..item("customers", "Customers", "customers", "directory:list", Rule::Directory("customers")) },
        item("estimates", "Estimates", "estimate", "sales:list", Rule::Ns("sales", &[])),
        item("invoices", "Invoices", "invoice", "invoices:list", Rule::Ns("invoices", &[])),
        item("sales_receipts", "Sales receipts", "sales-receipt", "sales_receipts:list", Rule::Ns("sales_receipts", &[])),
        item("credits", "Credit memos", "credit-memo", "credits:list", Rule::Ns("credits", &[])),
        item("receipts", "Customer receipts", "payment", "receipts:list", Rule::Ns("receipts", &[])),
        item("customer_deposits", "Customer deposits", "customer-deposit", "prepayments:list", Rule::Ns("prepayments", &[])),
    ]),
    ("Vendors", "Vendor work", &[
        Item { kind: Some("vendors"), ..item("vendors", "Vendors", "vendors", "directory:list", Rule::Directory("vendors")) },
        item("bills", "Bills", "bill", "bills:list", Rule::Ns("bills", &["calendar"])),
        item("bill_calendar", "Bill calendar", "calendar", "bills:calendar", Rule::Page("bills", "calendar")),
        item("payments", "Bill payments", "pay-bill", "payments:list", Rule::Ns("payments", &[])),
    ]),
    ("Banking", "Banking", &[
        item("bank_review", "Review transactions", "bank-feed", "bankfeeds:review", Rule::Page("bankfeeds", "review")),
        item("checks", "Checks", "check", "checks:list", Rule::Ns("checks", &[])),
        item("deposits", "Deposits", "deposit", "deposits:list", Rule::Ns("deposits", &[])),
        item("transfers", "Transfers", "transfer", "transfers:list", Rule::Ns("transfers", &[])),
        item("registers", "Registers", "register", "registers:list", Rule::Ns("registers", &[])),
        item("reconcile", "Reconcile", "reconcile", "reconcile:list", Rule::Ns("reconcile", &[])),
        item("bank_feeds", "Bank feeds", "bank-feed", "bankfeeds:list", Rule::Ns("bankfeeds", &["review"])),
    ]),
    ("Company", "Company", &[
        Item { badge: true, ..item("reminders", "Reminders", "reminders", "reminders:page", Rule::Ns("reminders", &[])) },
        item("accounts", "Chart of accounts", "accounts", "accounts", Rule::Root(&["accounts", "account_new", "account_edit", "register"])),
        Item { kind: Some("items"), ..item("items", "Items & services", "items", "directory:list", Rule::Directory("items")) },
        item("journals", "Journals", "journal", "company", Rule::Root(&["company", "journal_new", "journal", "journal_edit"])),
        item("documents", "Accounting setup", "setup", "documents:setup", Rule::Ns("documents", &[])),
        item("sales_tax", "Sales tax", "sales-tax", "salestax:settings", Rule::Ns("salestax", &[])),
        item("company_edit", "Company settings", "company-settings", "company_edit", Rule::Root(&["company_edit", "setup_start", "setup_fill"])),
        Item { staff_only: true, ..item("backups", "Backups", "backup", "backups:page", Rule::Ns("backups", &[])) },
        Item { staff_only: true, ..item("ai_assistant", "AI assistant", "ai-assistant", "assistant:page", Rule::Ns("assistant", &[])) },
    ]),
    ("Reports", "Reports", &[
        item("profit_loss", "Profit and loss", "profit-loss", "profit_loss", Rule::Root(&["profit_loss"])),
        item("balance_sheet", "Balance sheet", "balance-sheet", "balance_sheet", Rule::Root(&["balance_sheet"])),
        item("general_ledger", "General ledger", "general-ledger", "general_ledger", Rule::Root(&["general_ledger"])),
        item("trial_balance", "Trial balance", "trial-balance", "trial_balance", Rule::Root(&["trial_balance"])),
        item("history", "Audit history", "history", "history", Rule::Root(&["history"])),
    ]),
];

const HOME: Item = item("company_home", "Home", "home", "company_home", Rule::Root(&["company_home"]));

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct NavEntry {
    pub key: String,
    pub label: String,
    pub icon: String,
    pub url: String,
    pub current: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub badge: Option<i64>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct NavGroup {
    pub label: String,
    pub caption: String,
    pub items: Vec<NavEntry>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Default)]
pub struct Nav {
    pub companies_current: bool,
    pub home: Option<NavEntry>,
    pub groups: Vec<NavGroup>,
    pub current: Option<NavEntry>,
    pub current_group: Option<String>,
}

/// Where the request is: the resolved route name (`invoices:new`, `register`) and its `kind` argument.
#[derive(Clone, Debug, Default)]
pub struct Current<'a> {
    pub route: &'a str,
    pub kind: Option<&'a str>,
}

impl Rule {
    fn matches(&self, ns: &str, page: &str, kind: Option<&str>) -> bool {
        match self {
            Rule::Root(pages) => ns.is_empty() && pages.contains(&page),
            Rule::Ns(name, exclude) => ns == *name && !exclude.contains(&page),
            Rule::Directory(k) => ns == "directory" && kind == Some(*k),
            Rule::Page(name, p) => ns == *name && page == *p,
        }
    }
}

/// `build_nav(request, company)`. `overdue` is the reminders badge count (bills + invoices overdue).
pub fn build(company_id: Option<&str>, current: &Current<'_>, staff: bool, overdue: i64) -> Result<Nav, String> {
    let mut nav = Nav { companies_current: company_id.is_none(), ..Nav::default() };
    let Some(company) = company_id else { return Ok(nav) };
    let (ns, page) = match current.route.split_once(':') {
        Some((ns, page)) => (ns, page),
        None => ("", current.route),
    };
    let entry = |item: &Item, group: Option<&str>, nav: &mut Nav| -> Result<NavEntry, String> {
        let mut args = vec![company.to_owned()];
        if let Some(kind) = item.kind {
            args.push(kind.to_owned());
        }
        let row = NavEntry {
            key: item.key.into(),
            label: item.label.into(),
            icon: item.icon.into(),
            url: reverse(item.url_name, &args)?,
            current: item.rule.matches(ns, page, current.kind),
            badge: item.badge.then_some(overdue),
        };
        if row.current {
            nav.current = Some(row.clone());
            nav.current_group = group.map(str::to_owned);
        }
        Ok(row)
    };
    nav.home = Some(entry(&HOME, None, &mut nav)?);
    for (label, caption, items) in GROUPS {
        let mut rows = Vec::new();
        for it in items.iter().filter(|it| staff || !it.staff_only) {
            rows.push(entry(it, Some(caption), &mut nav)?);
        }
        nav.groups.push(NavGroup { label: (*label).into(), caption: (*caption).into(), items: rows });
    }
    Ok(nav)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_entry_follows_the_route() {
        let nav = build(Some("c1"), &Current { route: "invoices:new", kind: None }, false, 0).unwrap();
        assert_eq!(nav.current.unwrap().key, "invoices");
        assert_eq!(nav.current_group.as_deref(), Some("Customer work"));
        let nav = build(Some("c1"), &Current { route: "register", kind: None }, false, 2).unwrap();
        assert_eq!(nav.current.as_ref().unwrap().key, "accounts");
        let reminders = &nav.groups[3].items[0];
        assert_eq!((reminders.key.as_str(), reminders.badge), ("reminders", Some(2)));
        assert!(nav.groups[3].items.iter().all(|i| i.key != "backups"), "staff-only entries hidden");
        let staff = build(Some("c1"), &Current { route: "bills:calendar", kind: None }, true, 0).unwrap();
        assert_eq!(staff.current.unwrap().key, "bill_calendar");
        assert!(staff.groups[3].items.iter().any(|i| i.key == "backups"));
        let none = build(None, &Current::default(), false, 0).unwrap();
        assert!(none.companies_current && none.groups.is_empty());
    }
}
