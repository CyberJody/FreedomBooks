//! Which of the host's screens and forms a connected computer may use (deny by default).
//!
//! Every route in fb-app's route table (`fb-templates/urls.json`) and every desktop page
//! (`fb_app::screens::resubmit::DESKTOP`) is declared here as [`Remote::Allowed`] or
//! [`Remote::HostOnly`]. The dispatcher refuses any path this module does not classify, and the test
//! below fails on any route or desktop page missing from these tables, so a new screen is never
//! reachable remotely until someone decides it should be.
//!
//! Host-only: work whose credentials, files or dialogs live on the host computer (AI provider
//! connections, attachments, backups, bank-feed connections and their removal, book protection,
//! keys, restore), the host's problem log, and the old Django sign-in routes the desktop never uses.
use fb_remote_proto::Remote::{self, Allowed, HostOnly};

/// Route-table names that are host-only. Every other route-table name is listed in [`ALLOWED`].
pub const HOST_ONLY: &[&str] = &[
    "assistant:disconnect",
    "assistant:key",
    "assistant:model",
    "assistant:models_refresh",
    "assistant:notice",
    "assistant:openai_callback",
    "assistant:openai_start",
    "assistant:page",
    "assistant:read_attachment",
    "assistant:read_attachments",
    "assistant:read_upload",
    "attachments:add_document",
    "attachments:add_journal",
    "attachments:download",
    "attachments:remove",
    "backups:create",
    "backups:download",
    "backups:page",
    "bankfeeds:connect",
    "bankfeeds:forget",
    "bankfeeds:sync",
    "login",
    "logout",
];

/// Route-table names a connected computer may use (the session layer's role and company checks
/// still apply, and `fb_remote_proto::policy` still refuses host-only form actions on them).
pub const ALLOWED: &[&str] = &[
    "account_edit", "account_new", "accounts", "ap_aging", "ar_aging", "balance_sheet",
    "bankfeeds:accounts", "bankfeeds:list", "bankfeeds:review", "bankfeeds:rule_edit", "bankfeeds:rule_new", "bankfeeds:rules",
    "bills:calendar", "bills:detail", "bills:edit", "bills:list", "bills:new", "bills:post", "bills:review", "bills:revision", "bills:void",
    "checks:detail", "checks:edit", "checks:list", "checks:new", "checks:number", "checks:record", "checks:review", "checks:revision", "checks:void",
    "company", "company:periods", "company_edit", "company_home", "company_new",
    "credits:apply", "credits:detail", "credits:list", "credits:new", "credits:refund", "credits:refund_detail", "credits:refund_void", "credits:unapply", "credits:void",
    "deposits:detail", "deposits:list", "deposits:new", "deposits:reverse",
    "directory:detail", "directory:edit", "directory:list", "directory:new", "directory:status",
    "documents:setup", "expenses", "find", "general_ledger", "history", "home",
    "invoices:convert", "invoices:detail", "invoices:edit", "invoices:email_preview", "invoices:list", "invoices:new", "invoices:post", "invoices:print", "invoices:print_revision", "invoices:review", "invoices:revision", "invoices:void",
    "jobs:detail", "jobs:edit", "jobs:invoice", "jobs:list", "jobs:new",
    "journal", "journal_edit", "journal_new", "journal_post", "journal_reverse",
    "payments:detail", "payments:list", "payments:new", "payments:reverse",
    "prepayments:apply", "prepayments:detail", "prepayments:invoice_apply", "prepayments:list", "prepayments:new", "prepayments:print", "prepayments:refund", "prepayments:refund_detail", "prepayments:refund_void", "prepayments:unapply", "prepayments:void",
    "profit_loss",
    "reports:customer_statements", "reports:cash_flow", "reports:comparative_profit_and_loss", "reports:sales_by_customer_item",
    "reports:schedule_c_activity", "reports:asset_depreciation_schedule",
    "receipts:detail", "receipts:list", "receipts:new", "receipts:reverse",
    "reconcile:begin", "reconcile:history", "reconcile:list", "reconcile:report", "reconcile:screen", "reconcile:undo",
    "recurring:detail", "recurring:edit", "recurring:list", "recurring:new",
    "register", "registers:cleared", "registers:list", "reminders:page", "reminders:settings", "report_problem", "reports",
    "sales:detail", "sales:edit", "sales:list", "sales:new", "sales:print", "sales:print_revision", "sales:revision", "sales:state",
    "sales_receipts:detail", "sales_receipts:list", "sales_receipts:new", "sales_receipts:print", "sales_receipts:void",
    "salestax:bulk_step", "salestax:group_edit", "salestax:group_new", "salestax:list", "salestax:rate_edit", "salestax:rate_new", "salestax:record_payment", "salestax:settings", "salestax:setup", "salestax:status", "salestax:tax_payment", "salestax:void_payment", "salestax:what_you_owe",
    "settings", "setup_done", "setup_fill", "setup_start", "stripe:payments",
    "time:edit", "time:list", "time:new",
    "transfers:detail", "transfers:list", "transfers:new", "transfers:void", "trial_balance", "vendor_1099",
];

/// The desktop's own pages (not in the route table), by path.
pub const DESKTOP: &[(&str, Remote)] = &[
    ("/desktop/sign-in/", Allowed),
    ("/desktop/users/", Allowed),
    ("/desktop/mfa/", Allowed),
    ("/desktop/session/", Allowed),
    ("/desktop/problem/", HostOnly),
    ("/settings/book-protection/", HostOnly),
    ("/settings/recovery-key/", HostOnly),
    ("/settings/emergency-kit/", HostOnly),
    ("/settings/restore-backup/", HostOnly),
    ("/settings/restored-book/", HostOnly),
];

/// The class of a route-table name; `None` if nobody declared it.
pub fn route(name: &str) -> Option<Remote> {
    if HOST_ONLY.contains(&name) {
        Some(HostOnly)
    } else if ALLOWED.contains(&name) {
        Some(Allowed)
    } else {
        None
    }
}

/// The class of a path (no query string), as fb-app's router resolves it; `None` for a path the
/// host does not serve or nobody classified.
pub fn classify(path: &str) -> Option<Remote> {
    if let Some((_, class)) = DESKTOP.iter().find(|(p, _)| *p == path) {
        return Some(*class);
    }
    fb_app::screens::resolve(path).and_then(|r| route(&r.name))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A new route in the route table, or a new desktop page, must be classified here first.
    #[test]
    fn every_route_and_desktop_page_has_a_remote_class_and_nothing_else_does() {
        let table: Vec<&String> = fb_templates::django::route_table().keys().collect();
        let missing: Vec<&&String> = table.iter().filter(|name| route(name).is_none()).collect();
        assert!(missing.is_empty(), "classify these routes for remote access in fb-host/src/routes.rs: {missing:?}");
        let stale: Vec<&str> = HOST_ONLY.iter().chain(ALLOWED).copied().filter(|n| !table.iter().any(|t| t.as_str() == *n)).collect();
        assert!(stale.is_empty(), "no longer in the route table: {stale:?}");
        let both: Vec<&str> = HOST_ONLY.iter().copied().filter(|n| ALLOWED.contains(n)).collect();
        assert!(both.is_empty(), "declared both ways: {both:?}");
        let mut all: Vec<&str> = HOST_ONLY.iter().chain(ALLOWED).copied().collect();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), HOST_ONLY.len() + ALLOWED.len(), "declared twice");
        for (path, _) in fb_app::screens::resubmit::DESKTOP {
            assert!(DESKTOP.iter().any(|(p, _)| p == path), "classify desktop page {path} for remote access in fb-host/src/routes.rs");
        }
        for (path, _) in DESKTOP {
            assert!(fb_app::screens::resubmit::desktop_route_name(path).is_some(), "{path} is not a desktop page any more");
        }
    }

    #[test]
    fn report_routes_are_allowed_for_connected_sessions() {
        let root = "/c/0195b4a4-55af-719e-96cb-a2f946b067bf/reports";
        for slug in ["customer-statements", "cash-flow", "comparative-profit-and-loss", "sales-by-customer-item", "schedule-c", "asset-depreciation"] {
            assert_eq!(classify(&format!("{root}/{slug}/")), Some(Allowed));
        }
    }

    #[test]
    fn paths_resolve_to_their_class_and_unknown_paths_to_none() {
        let c = "/c/0195b4a4-55af-719e-96cb-a2f946b067bf";
        assert_eq!(classify(&format!("{c}/home/")), Some(Allowed));
        assert_eq!(classify(&format!("{c}/accounts/new/")), Some(Allowed));
        assert_eq!(classify(&format!("{c}/backups/")), Some(HostOnly));
        assert_eq!(classify(&format!("{c}/ai/")), Some(HostOnly));
        assert_eq!(classify("/auth/openai/start"), Some(HostOnly));
        assert_eq!(classify("/desktop/sign-in/"), Some(Allowed));
        assert_eq!(classify("/settings/book-protection/"), Some(HostOnly));
        assert_eq!(classify(&format!("{c}/no-such-screen/")), None);
        assert_eq!(classify("/c/not-a-company/home/"), None);
        // The fixed list in the protocol crate agrees with this table on every host-only route.
        for (path, class) in DESKTOP {
            assert_eq!(fb_remote_proto::policy::host_only(path), *class == HostOnly, "{path}");
        }
    }
}
