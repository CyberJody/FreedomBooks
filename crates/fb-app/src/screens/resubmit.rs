//! What a resubmitted form could repeat, declared for every route (CRASHLOG).
//!
//! A form post that fails after spending its single-use token comes back to the page
//! (`App::run_desktop`, `Outcome::Retry`). It gets a fresh token only when a resubmit cannot repeat
//! anything the first attempt did. The book's commit counter shows whether the attempt saved
//! anything in the book; it cannot see work done anywhere else. So a route gets a fresh token
//! only when it is declared here as [`Resubmit::BookOnly`]. Everything else, including a route
//! nobody has declared, gets none: the owner is asked to reopen the page and check first.
//!
//! The test below walks the whole route table and fails on any route missing from [`ROUTES`],
//! and on any desktop page missing from [`DESKTOP`], so a new route cannot be added unclassified.

/// What a form at a route may do besides writing the book.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Resubmit {
    /// Writes only the book (or nothing): the commit counter sees all of it, so after a failure
    /// that committed nothing, the form can be sent again safely.
    BookOnly,
    /// May also do work the commit counter cannot see: a network send, a connection, a file
    /// written or picked through the shell, a print, or a change of session, keys or files.
    Outside,
}

use Resubmit::{BookOnly, Outside};

/// Every route in the route table (`fb-templates/urls.json`) with what its forms may do.
pub const ROUTES: &[(&str, Resubmit)] = &[
    ("recurring:list", BookOnly),
    ("recurring:new", BookOnly),
    ("recurring:detail", BookOnly),
    ("recurring:edit", BookOnly),
    ("account_edit", BookOnly),
    ("account_new", BookOnly),
    ("accounts", BookOnly),
    ("assistant:disconnect", Outside), // the AI provider over the network
    ("assistant:key", Outside), // the AI provider over the network
    ("assistant:model", Outside), // the AI provider over the network
    ("assistant:models_refresh", Outside), // the AI provider over the network
    ("assistant:notice", Outside), // the AI provider over the network
    ("assistant:openai_callback", Outside), // the AI provider over the network
    ("assistant:openai_start", Outside), // the AI provider over the network
    ("assistant:page", Outside), // the AI provider over the network
    ("assistant:read_attachment", Outside), // the AI provider over the network
    ("assistant:read_attachments", Outside), // the AI provider over the network
    ("assistant:read_upload", Outside), // the AI provider over the network
    ("attachments:add_document", Outside), // a file dialog in the shell
    ("attachments:add_journal", Outside), // a file dialog in the shell
    ("attachments:download", Outside), // writes a file
    ("attachments:remove", BookOnly),
    ("backups:create", Outside), // backup files outside the book
    ("backups:download", Outside), // backup files outside the book
    ("backups:page", Outside), // backup files outside the book
    ("balance_sheet", BookOnly),
    ("bankfeeds:accounts", Outside), // the bank-feed provider, its connections and rule-file dialogs
    ("bankfeeds:connect", Outside), // the bank-feed provider, its connections and rule-file dialogs
    ("bankfeeds:forget", Outside), // the bank-feed provider, its connections and rule-file dialogs
    ("bankfeeds:list", Outside), // the bank-feed provider, its connections and rule-file dialogs
    ("bankfeeds:review", Outside), // the bank-feed provider, its connections and rule-file dialogs
    ("bankfeeds:rule_edit", Outside), // the bank-feed provider, its connections and rule-file dialogs
    ("bankfeeds:rule_new", Outside), // the bank-feed provider, its connections and rule-file dialogs
    ("bankfeeds:rules", Outside), // the bank-feed provider, its connections and rule-file dialogs
    ("bankfeeds:sync", Outside), // the bank-feed provider, its connections and rule-file dialogs
    ("bills:calendar", BookOnly),
    ("bills:detail", BookOnly),
    ("bills:edit", BookOnly),
    ("bills:list", BookOnly),
    ("bills:new", BookOnly),
    ("bills:post", BookOnly),
    ("bills:review", BookOnly),
    ("bills:revision", BookOnly),
    ("bills:void", BookOnly),
    ("checks:detail", BookOnly),
    ("checks:edit", BookOnly),
    ("checks:list", BookOnly),
    ("checks:new", BookOnly),
    ("checks:number", BookOnly),
    ("checks:record", BookOnly),
    ("checks:review", BookOnly),
    ("checks:revision", BookOnly),
    ("checks:void", BookOnly),
    ("company", BookOnly),
    ("company_edit", BookOnly),
    ("company_home", BookOnly),
    ("company_new", BookOnly),
    ("credits:apply", BookOnly),
    ("credits:detail", BookOnly),
    ("credits:list", BookOnly),
    ("credits:new", BookOnly),
    ("credits:refund", BookOnly),
    ("credits:refund_detail", BookOnly),
    ("credits:refund_void", BookOnly),
    ("credits:unapply", BookOnly),
    ("credits:void", BookOnly),
    ("deposits:detail", BookOnly),
    ("deposits:list", BookOnly),
    ("deposits:new", BookOnly),
    ("deposits:reverse", BookOnly),
    ("directory:detail", BookOnly),
    ("directory:edit", BookOnly),
    ("directory:list", BookOnly),
    ("directory:new", BookOnly),
    ("directory:status", BookOnly),
    ("documents:setup", BookOnly),
    ("expenses", BookOnly),
    ("find", BookOnly),
    ("general_ledger", BookOnly),
    ("history", BookOnly),
    ("home", Outside), // the session
    ("invoices:convert", BookOnly),
    ("invoices:detail", BookOnly),
    ("invoices:edit", BookOnly),
    ("invoices:email_preview", Outside), // sends email
    ("invoices:list", BookOnly),
    ("invoices:new", BookOnly),
    ("invoices:post", BookOnly),
    ("invoices:print", Outside), // prints
    ("invoices:print_revision", Outside), // prints
    ("invoices:review", BookOnly),
    ("invoices:revision", BookOnly),
    ("invoices:void", BookOnly),
    ("jobs:detail", BookOnly),
    ("jobs:edit", BookOnly),
    ("jobs:invoice", BookOnly),
    ("jobs:list", BookOnly),
    ("jobs:new", BookOnly),
    ("journal", BookOnly),
    ("journal_edit", BookOnly),
    ("journal_new", BookOnly),
    ("journal_post", BookOnly),
    ("journal_reverse", BookOnly),
    ("login", Outside), // the session
    ("logout", Outside), // the session
    ("payments:detail", BookOnly),
    ("payments:list", BookOnly),
    ("payments:new", BookOnly),
    ("payments:reverse", BookOnly),
    ("prepayments:apply", BookOnly),
    ("prepayments:detail", BookOnly),
    ("prepayments:invoice_apply", BookOnly),
    ("prepayments:list", BookOnly),
    ("prepayments:new", BookOnly),
    ("prepayments:print", Outside), // prints
    ("prepayments:refund", BookOnly),
    ("prepayments:refund_detail", BookOnly),
    ("prepayments:refund_void", BookOnly),
    ("prepayments:unapply", BookOnly),
    ("prepayments:void", BookOnly),
    ("profit_loss", BookOnly),
    ("receipts:detail", BookOnly),
    ("receipts:list", BookOnly),
    ("receipts:new", BookOnly),
    ("receipts:reverse", BookOnly),
    ("reconcile:begin", BookOnly),
    ("reconcile:history", BookOnly),
    ("reconcile:list", BookOnly),
    ("reconcile:report", BookOnly),
    ("reconcile:screen", BookOnly),
    ("reconcile:undo", BookOnly),
    ("register", BookOnly),
    ("registers:cleared", BookOnly),
    ("registers:list", BookOnly),
    ("reminders:page", BookOnly),
    ("reminders:settings", BookOnly),
    ("report_problem", BookOnly),
    ("reports", BookOnly),
    ("sales:detail", BookOnly),
    ("sales:edit", BookOnly),
    ("sales:list", BookOnly),
    ("sales:new", BookOnly),
    ("sales:print", Outside), // prints
    ("sales:print_revision", Outside), // prints
    ("sales:revision", BookOnly),
    ("sales:state", BookOnly),
    ("sales_receipts:detail", BookOnly),
    ("sales_receipts:list", BookOnly),
    ("sales_receipts:new", BookOnly),
    ("sales_receipts:print", Outside), // prints
    ("sales_receipts:void", BookOnly),
    ("salestax:bulk_step", BookOnly),
    ("salestax:group_edit", BookOnly),
    ("salestax:group_new", BookOnly),
    ("salestax:list", BookOnly),
    ("salestax:rate_edit", BookOnly),
    ("salestax:rate_new", BookOnly),
    ("salestax:record_payment", BookOnly),
    ("salestax:settings", BookOnly),
    ("salestax:setup", BookOnly),
    ("salestax:status", BookOnly),
    ("salestax:tax_payment", BookOnly),
    ("salestax:void_payment", BookOnly),
    ("salestax:what_you_owe", BookOnly),
    ("settings", BookOnly),
    ("setup_done", BookOnly),
    ("setup_fill", BookOnly),
    ("setup_start", BookOnly),
    ("stripe:payments", Outside), // Stripe over the network
    ("time:edit", BookOnly),
    ("time:list", BookOnly),
    ("time:new", BookOnly),
    ("transfers:detail", BookOnly),
    ("transfers:list", BookOnly),
    ("transfers:new", BookOnly),
    ("transfers:void", BookOnly),
    ("trial_balance", BookOnly),
];

/// The desktop's own pages (not in the route table), with the name the problem log uses for each.
/// Their forms all change the session, the keys or files outside the book.
pub const DESKTOP: &[(&str, &str)] = &[
    ("/desktop/sign-in/", "desktop:sign-in"),
    ("/desktop/users/", "desktop:users"),
    ("/desktop/mfa/", "desktop:mfa"),
    ("/desktop/session/", "desktop:session"),
    ("/desktop/problem/", "desktop:problem"),
    ("/settings/book-protection/", "desktop:protection"),
    ("/settings/recovery-key/", "desktop:recovery"),
    ("/settings/emergency-kit/", "desktop:kit"),
    ("/settings/restore-backup/", "desktop:restore"),
    ("/settings/restored-book/", "desktop:restored"),
];

/// Form actions that leave the book whatever the route (`gmail_connect`, `stripe_link`,
/// `email_send`, `desktop_print`).
const OUTSIDE_ACTIONS: &[&str] = &["gmail_", "stripe_", "email_", "desktop_print"];

/// The declared class of a route name; `None` for a name nobody declared.
pub fn class(route: &str) -> Option<Resubmit> {
    ROUTES.iter().find(|(name, _)| *name == route).map(|(_, class)| *class)
}

/// What a resubmit of this form post could repeat. Undeclared routes and desktop pages are
/// [`Resubmit::Outside`].
pub fn form(path: &str, fields: &[(String, String)]) -> Resubmit {
    let action = fields.iter().rev().find(|(k, _)| k == "action").map(|(_, v)| v.as_str()).unwrap_or("");
    if OUTSIDE_ACTIONS.iter().any(|p| action.starts_with(p)) {
        return Outside;
    }
    super::resolve(path).and_then(|r| class(&r.name)).unwrap_or(Outside)
}

/// A desktop page's fixed name for the problem log.
pub fn desktop_route_name(path: &str) -> Option<&'static str> {
    DESKTOP.iter().find(|(p, _)| *p == path).map(|(_, name)| *name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A new route in the route table must be declared here before this passes.
    #[test]
    fn every_route_and_desktop_page_is_classified_and_nothing_else_is() {
        let table = fb_templates::django::route_table();
        let missing: Vec<&String> = table.keys().filter(|name| class(name).is_none()).collect();
        assert!(missing.is_empty(), "declare these routes in screens/resubmit.rs ROUTES: {missing:?}");
        let stale: Vec<&str> = ROUTES.iter().map(|(n, _)| *n).filter(|n| !table.contains_key(*n)).collect();
        assert!(stale.is_empty(), "no longer in the route table: {stale:?}");
        let mut names: Vec<&str> = ROUTES.iter().map(|(n, _)| *n).collect();
        names.dedup();
        assert_eq!(names.len(), ROUTES.len(), "declared twice");
        let pages = [crate::identity::SIGN_IN, crate::identity::SESSION, crate::identity::USERS, crate::mfa::SETTINGS,
            crate::protection::SETTINGS, crate::protection::RECOVERY, crate::protection::KIT, super::super::protection::RESTORE, super::super::protection::RESTORED];
        for page in pages {
            assert!(super::super::identity::handles(page) || super::super::protection::handles(page), "{page}");
            assert!(desktop_route_name(page).is_some(), "declare {page} in DESKTOP");
            assert_eq!(form(page, &[]), Outside, "{page}");
        }
        for (path, _) in DESKTOP {
            assert!(super::super::resolve(path).is_none(), "{path} is in the route table; declare it in ROUTES");
        }
    }

    #[test]
    fn undeclared_paths_and_outside_actions_get_no_fresh_token() {
        let c = "0195b4a4-55af-719e-96cb-a2f946b067bf";
        assert_eq!(form(&format!("/c/{c}/accounts/new/"), &[]), BookOnly);
        assert_eq!(form(&format!("/c/{c}/receipts/new/"), &[]), BookOnly);
        assert_eq!(form(&format!("/c/{c}/bank-feeds/connect/"), &[]), Outside);
        assert_eq!(form("/no/such/route/", &[]), Outside);
        for action in ["gmail_connect", "stripe_link", "email_send", "desktop_print"] {
            assert_eq!(form(&format!("/c/{c}/edit/"), &[("action".into(), action.into())]), Outside, "{action}");
        }
        assert_eq!(form(&format!("/c/{c}/edit/"), &[("action".into(), "save".into())]), BookOnly);
    }
}
