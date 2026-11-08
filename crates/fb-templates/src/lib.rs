//! fb-templates: the approved hybrid v1 screens, ported 1:1 from the frozen Django templates to minijinja,
//! and the view models they render. No I/O: callers pass a view model, get an HTML string back.
//!
//! Ported templates live under `templates/` with the same paths as in the Django app. They are produced
//! by `tools/port_d1.sh` (the mechanical `port_template.py` pass plus the reviewed hand fixes in
//! `tools/d1_hand_fixes.py`), so a later wave can regenerate them from the reference and see every
//! deliberate difference in one file. Desktop-only screens (first run, app lock) are under `templates/desktop/`.
pub mod django;
mod d3;
mod d6;
mod d7a;
mod d7a_unicode;
mod d8;
mod printing;
mod d3_text;
mod d4;
pub mod forms;
pub mod nav;
pub mod vm;

use minijinja::{AutoEscape, Environment, UndefinedBehavior};
use serde::Serialize;

/// Every template the app can render, by its Django path.
const TEMPLATES: &[(&str, &str)] = &[
    ("desktop/users.html", include_str!("../templates/desktop/users.html")),
    ("desktop/restore.html", include_str!("../templates/desktop/restore.html")),
    ("desktop/emergency-kit.html", include_str!("../templates/desktop/emergency-kit.html")),
    ("desktop/recovery.html", include_str!("../templates/desktop/recovery.html")),
    ("desktop/protection.html", include_str!("../templates/desktop/protection.html")),

    ("ledger/numbering.html", include_str!("../templates/ledger/numbering.html")),
    ("sales/print/document.html", include_str!("../templates/sales/print/document.html")),
    ("prepayments/print.html", include_str!("../templates/prepayments/print.html")),
    ("banking/registers/print.html", include_str!("../templates/banking/registers/print.html")),
    ("banking/reconcile/report_print.html", include_str!("../templates/banking/reconcile/report_print.html")),
    ("ledger/reports/print.html", include_str!("../templates/ledger/reports/print.html")),
    ("banking/registers/list.html", include_str!("../templates/banking/registers/list.html")),
    ("banking/registers/register.html", include_str!("../templates/banking/registers/register.html")),
    ("banking/reconcile/chooser.html", include_str!("../templates/banking/reconcile/chooser.html")),
    ("banking/reconcile/begin.html", include_str!("../templates/banking/reconcile/begin.html")),
    ("banking/reconcile/screen.html", include_str!("../templates/banking/reconcile/screen.html")),
    ("banking/reconcile/_summary.html", include_str!("../templates/banking/reconcile/_summary.html")),
    ("banking/reconcile/history.html", include_str!("../templates/banking/reconcile/history.html")),
    ("banking/reconcile/report.html", include_str!("../templates/banking/reconcile/report.html")),
    ("banking/reconcile/undo.html", include_str!("../templates/banking/reconcile/undo.html")),

    ("checks/list.html", include_str!("../templates/checks/list.html")),
    ("checks/detail.html", include_str!("../templates/checks/detail.html")),
    ("checks/document.html", include_str!("../templates/checks/document.html")),
    ("checks/form.html", include_str!("../templates/checks/form.html")),
    ("checks/line_fields.html", include_str!("../templates/checks/line_fields.html")),
    ("checks/number.html", include_str!("../templates/checks/number.html")),
    ("checks/review.html", include_str!("../templates/checks/review.html")),
    ("checks/revision.html", include_str!("../templates/checks/revision.html")),
    ("checks/void.html", include_str!("../templates/checks/void.html")),
    ("checks/vendor_activity.html", include_str!("../templates/checks/vendor_activity.html")),
    ("banking/deposits/list.html", include_str!("../templates/banking/deposits/list.html")),
    ("banking/deposits/detail.html", include_str!("../templates/banking/deposits/detail.html")),
    ("banking/deposits/form.html", include_str!("../templates/banking/deposits/form.html")),
    ("banking/deposits/reverse.html", include_str!("../templates/banking/deposits/reverse.html")),
    ("credits/list.html", include_str!("../templates/credits/list.html")),
    ("credits/detail.html", include_str!("../templates/credits/detail.html")),
    ("credits/form.html", include_str!("../templates/credits/form.html")),
    ("credits/apply.html", include_str!("../templates/credits/apply.html")),
    ("credits/unapply.html", include_str!("../templates/credits/unapply.html")),
    ("credits/void.html", include_str!("../templates/credits/void.html")),
    ("credits/invoice_credits.html", include_str!("../templates/credits/invoice_credits.html")),
    ("credits/customer_activity.html", include_str!("../templates/credits/customer_activity.html")),
    ("credits/refund_form.html", include_str!("../templates/credits/refund_form.html")),
    ("credits/refund_detail.html", include_str!("../templates/credits/refund_detail.html")),
    ("credits/refund_void.html", include_str!("../templates/credits/refund_void.html")),
    ("prepayments/list.html", include_str!("../templates/prepayments/list.html")),
    ("prepayments/detail.html", include_str!("../templates/prepayments/detail.html")),
    ("prepayments/form.html", include_str!("../templates/prepayments/form.html")),
    ("prepayments/apply.html", include_str!("../templates/prepayments/apply.html")),
    ("prepayments/unapply.html", include_str!("../templates/prepayments/unapply.html")),
    ("prepayments/void.html", include_str!("../templates/prepayments/void.html")),
    ("prepayments/invoice_deposits.html", include_str!("../templates/prepayments/invoice_deposits.html")),
    ("prepayments/customer_deposits.html", include_str!("../templates/prepayments/customer_deposits.html")),
    ("prepayments/refund_form.html", include_str!("../templates/prepayments/refund_form.html")),
    ("prepayments/refund_detail.html", include_str!("../templates/prepayments/refund_detail.html")),
    ("prepayments/refund_void.html", include_str!("../templates/prepayments/refund_void.html")),
    ("ledger/reports/_controls.html", include_str!("../templates/ledger/reports/_controls.html")),
    ("ledger/reports/_rows.html", include_str!("../templates/ledger/reports/_rows.html")),
    ("ledger/reports/profit_loss.html", include_str!("../templates/ledger/reports/profit_loss.html")),
    ("ledger/reports/profit_loss_body.html", include_str!("../templates/ledger/reports/profit_loss_body.html")),
    ("ledger/reports/balance_sheet.html", include_str!("../templates/ledger/reports/balance_sheet.html")),
    ("ledger/reports/balance_sheet_body.html", include_str!("../templates/ledger/reports/balance_sheet_body.html")),
    ("ledger/reports/general_ledger.html", include_str!("../templates/ledger/reports/general_ledger.html")),
    ("ledger/reports/general_ledger_body.html", include_str!("../templates/ledger/reports/general_ledger_body.html")),
    // D9 bank feeds (tools/d9_hand_fixes.py).
    ("bankfeeds/list.html", include_str!("../templates/bankfeeds/list.html")),
    ("bankfeeds/connect.html", include_str!("../templates/bankfeeds/connect.html")),
    ("bankfeeds/forget.html", include_str!("../templates/bankfeeds/forget.html")),
    ("bankfeeds/accounts.html", include_str!("../templates/bankfeeds/accounts.html")),
    ("bankfeeds/rules.html", include_str!("../templates/bankfeeds/rules.html")),
    ("bankfeeds/rule_form.html", include_str!("../templates/bankfeeds/rule_form.html")),
    ("bankfeeds/review.html", include_str!("../templates/bankfeeds/review.html")),
    ("backups/page.html", include_str!("../templates/backups/page.html")),
    ("assistant/page.html", include_str!("../templates/assistant/page.html")),

    ("sales/list.html", include_str!("../templates/sales/list.html")),
    ("sales/detail.html", include_str!("../templates/sales/detail.html")),
    ("sales/document.html", include_str!("../templates/sales/document.html")),
    ("sales/form.html", include_str!("../templates/sales/form.html")),
    ("sales/revision.html", include_str!("../templates/sales/revision.html")),
    ("sales/invoices/convert.html", include_str!("../templates/sales/invoices/convert.html")),
    ("sales/invoices/detail.html", include_str!("../templates/sales/invoices/detail.html")),
    ("sales/invoices/document.html", include_str!("../templates/sales/invoices/document.html")),
    ("sales/invoices/review.html", include_str!("../templates/sales/invoices/review.html")),
    ("sales/invoices/revision.html", include_str!("../templates/sales/invoices/revision.html")),
    ("sales/invoices/void.html", include_str!("../templates/sales/invoices/void.html")),
    ("sales/receipts/list.html", include_str!("../templates/sales/receipts/list.html")),
    ("sales/receipts/detail.html", include_str!("../templates/sales/receipts/detail.html")),
    ("sales/receipts/form.html", include_str!("../templates/sales/receipts/form.html")),
    ("sales/receipts/reverse.html", include_str!("../templates/sales/receipts/reverse.html")),
    ("sales/receipts/customer_activity.html", include_str!("../templates/sales/receipts/customer_activity.html")),
    ("sales/sales_receipts/list.html", include_str!("../templates/sales/sales_receipts/list.html")),
    ("sales/sales_receipts/detail.html", include_str!("../templates/sales/sales_receipts/detail.html")),
    ("sales/sales_receipts/form.html", include_str!("../templates/sales/sales_receipts/form.html")),
    ("sales/sales_receipts/void.html", include_str!("../templates/sales/sales_receipts/void.html")),
    ("payables/bills/list.html", include_str!("../templates/payables/bills/list.html")),
    ("payables/bills/detail.html", include_str!("../templates/payables/bills/detail.html")),
    ("payables/bills/document.html", include_str!("../templates/payables/bills/document.html")),
    ("payables/bills/form.html", include_str!("../templates/payables/bills/form.html")),
    ("payables/bills/line_fields.html", include_str!("../templates/payables/bills/line_fields.html")),
    ("payables/bills/review.html", include_str!("../templates/payables/bills/review.html")),
    ("payables/bills/revision.html", include_str!("../templates/payables/bills/revision.html")),
    ("payables/bills/void.html", include_str!("../templates/payables/bills/void.html")),
    ("payables/bills/vendor_activity.html", include_str!("../templates/payables/bills/vendor_activity.html")),
    ("payables/bills/calendar.html", include_str!("../templates/payables/bills/calendar.html")),
    ("payables/payments/list.html", include_str!("../templates/payables/payments/list.html")),
    ("payables/payments/detail.html", include_str!("../templates/payables/payments/detail.html")),
    ("payables/payments/form.html", include_str!("../templates/payables/payments/form.html")),
    ("payables/payments/reverse.html", include_str!("../templates/payables/payments/reverse.html")),
    ("transfers/list.html", include_str!("../templates/transfers/list.html")),
    ("transfers/detail.html", include_str!("../templates/transfers/detail.html")),
    ("transfers/form.html", include_str!("../templates/transfers/form.html")),
    ("transfers/void.html", include_str!("../templates/transfers/void.html")),

    ("directory/base.html", include_str!("../templates/directory/base.html")),
    ("directory/list.html", include_str!("../templates/directory/list.html")),
    ("directory/detail.html", include_str!("../templates/directory/detail.html")),
    ("directory/form.html", include_str!("../templates/directory/form.html")),
    ("directory/status_error.html", include_str!("../templates/directory/status_error.html")),
    ("salestax/base.html", include_str!("../templates/salestax/base.html")),
    ("salestax/settings.html", include_str!("../templates/salestax/settings.html")),
    ("salestax/list.html", include_str!("../templates/salestax/list.html")),
    ("salestax/item_form.html", include_str!("../templates/salestax/item_form.html")),
    ("salestax/members.html", include_str!("../templates/salestax/members.html")),
    ("salestax/bulk_step.html", include_str!("../templates/salestax/bulk_step.html")),
    ("salestax/status_error.html", include_str!("../templates/salestax/status_error.html")),
    ("reminders/page.html", include_str!("../templates/reminders/page.html")),
    ("backups/page.html", include_str!("../templates/backups/page.html")),

    ("ledger/home.html", include_str!("../templates/ledger/home.html")),
    ("ledger/company.html", include_str!("../templates/ledger/company.html")),
    ("ledger/form.html", include_str!("../templates/ledger/form.html")),
    ("ledger/company_form.html", include_str!("../templates/ledger/company_form.html")),
    ("ledger/error.html", include_str!("../templates/ledger/error.html")),
    ("ledger/events.html", include_str!("../templates/ledger/events.html")),
    ("ledger/history.html", include_str!("../templates/ledger/history.html")),
    ("ledger/journal.html", include_str!("../templates/ledger/journal.html")),
    ("attachments/panel.html", include_str!("../templates/attachments/panel.html")),
    ("ledger/journal_form.html", include_str!("../templates/ledger/journal_form.html")),
    ("ledger/report_problem.html", include_str!("../templates/ledger/report_problem.html")),
    ("ledger/setup_start.html", include_str!("../templates/ledger/setup_start.html")),
    ("ledger/setup_fill.html", include_str!("../templates/ledger/setup_fill.html")),
    ("ledger/trial_balance.html", include_str!("../templates/ledger/trial_balance.html")),
    ("ledger/includes/print_toolbar_note.html", include_str!("../templates/ledger/includes/print_toolbar_note.html")),
    ("documents/setup.html", include_str!("../templates/documents/setup.html")),

    ("ledger/base.html", include_str!("../templates/ledger/base.html")),
    ("ledger/nav.html", include_str!("../templates/ledger/nav.html")),
    ("ledger/pager.html", include_str!("../templates/ledger/pager.html")),
    ("ledger/company_home.html", include_str!("../templates/ledger/company_home.html")),
    ("ledger/accounts.html", include_str!("../templates/ledger/accounts.html")),
    ("ledger/register.html", include_str!("../templates/ledger/register.html")),
    ("sales/invoices/list.html", include_str!("../templates/sales/invoices/list.html")),
    ("sales/invoices/form.html", include_str!("../templates/sales/invoices/form.html")),
    ("sales/invoices/line_fields.html", include_str!("../templates/sales/invoices/line_fields.html")),
    ("desktop/gate.html", include_str!("../templates/desktop/gate.html")),
    ("desktop/not_ported.html", include_str!("../templates/desktop/not_ported.html")),
];

#[derive(Debug)]
pub struct RenderError(pub String);

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for RenderError {}

impl From<minijinja::Error> for RenderError {
    fn from(e: minijinja::Error) -> Self {
        let mut text = e.to_string();
        let mut source = std::error::Error::source(&e);
        while let Some(s) = source {
            text.push_str(&format!(": {s}"));
            source = s.source();
        }
        RenderError(text)
    }
}

/// The template environment, built once per process.
pub struct Screens {
    env: Environment<'static>,
}

impl Default for Screens {
    fn default() -> Self {
        Self::new()
    }
}

impl Screens {
    pub fn new() -> Screens {
        let mut env = Environment::new();
        env.set_auto_escape_callback(|_| AutoEscape::Html);
        env.set_formatter(django::formatter);
        // Django prints a missing variable or attribute as "" (and `None.x` as ""). Chainable keeps that, so
        // a typo in a ported template is caught by the screen diff, not by the renderer.
        env.set_undefined_behavior(UndefinedBehavior::Chainable);
        env.set_keep_trailing_newline(true);
        env.set_trim_blocks(false);
        for (name, source) in TEMPLATES {
            env.add_template(name, source).unwrap_or_else(|e| panic!("template {name} does not compile: {e}"));
        }
        env.add_filter("default", django::default);
        env.add_filter("title", django::title);
        env.add_filter("lower", django::lower);
        env.add_filter("pluralize", django::pluralize);
        env.add_filter("date", d3::date);
        d3::register(&mut env);
        d4::register(&mut env);
        d6::register(&mut env);
        d7a::register(&mut env);
        d8::register(&mut env);
        printing::register(&mut env);
        env.add_filter("json_script", django::json_script);
        env.add_filter("filesizeformat", django::filesizeformat);
        env.add_function("url", django::url);
        env.add_function("static", django::static_url);
        env.add_function("icon", django::icon);
        env.add_function("icon_url", django::icon_url);
        Screens { env }
    }

    /// Render one template with a view model.
    pub fn render<T: Serialize>(&self, template: &str, vm: &T) -> Result<String, RenderError> {
        let tmpl = self.env.get_template(template)?;
        Ok(tmpl.render(minijinja::Value::from_serialize(vm))?)
    }

    pub fn templates() -> impl Iterator<Item = &'static str> {
        TEMPLATES.iter().map(|(n, _)| *n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_template_compiles() {
        let _ = Screens::new();
        assert!(Screens::templates().count() >= 9);
    }

    #[test]
    fn output_is_escaped_the_django_way() {
        let s = Screens::new();
        let html = s
            .render("desktop/not_ported.html", &json!({"title": "<b>'x'</b>", "path": "/a/", "user": {"is_authenticated": false}, "nav": {"companies_current": true, "groups": []}}))
            .unwrap();
        assert!(html.contains("&lt;b&gt;&#x27;x&#x27;&lt;/b&gt;"), "{html}");
        assert!(!html.contains("<b>'x'"));
    }
}

#[cfg(test)]
mod company_documents_tests;
