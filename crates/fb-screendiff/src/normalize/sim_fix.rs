//! SIM-FIX (owner approved 11/03): deliberate desktop divergences from the v1 virtual-employee run,
//! each normalized only in its own span (NORMALIZATION.md "SIM-FIX"; DESKTOP-WAVES.md; TRIAGE.md).
use super::*;

pub(super) fn normalize(html: &str, ids: &IdMap) -> String {
    // Item 2: the invoice document panel's status line. The desktop says "Posted · Paid" (posting state
    // plus the live payment state) where Django says "Open"; only a posted (OPEN) invoice has one.
    static INVOICE_STATUS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(<section class="panel estimate-document"><h2>[^<]*</h2><p>INV-[0-9]{6} · )Posted · (?:Open|Partial|Paid)( · [A-Z]{3}</p>)"#).unwrap()
    });
    let html = INVOICE_STATUS.replace_all(html, "${1}Open${2}");
    // Item 2 on the invoice print copy: its revision line's "Status:" (an INVOICE print only).
    static PRINT_STATUS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(<p class="revision-line">Revision [0-9]+ of [0-9]+ · Status: )Posted · (?:Open|Partial|Paid)(<br>)"#).unwrap()
    });
    static INVOICE_PRINT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"<p class="doc-title">INVOICE[< ]"#).unwrap());
    let html = if INVOICE_PRINT.is_match(&html) { PRINT_STATUS.replace(&html, "${1}Open${2}").into_owned() } else { html.into_owned() };
    // Item 3: the transfer list's "Bank (from or to)" filter. Django offers every asset account; drop the
    // options of asset accounts that are neither designated banks nor used by a transfer. Only Django's
    // ids.json names that class (`transfer_nonbank`), so the desktop side is never rewritten here.
    static TRANSFER_FILTER: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)(<form method="get" class="filters transfer-filters" aria-label="Filter transfers">.*?<select name="account"[^>]*>)(.*?)(</select>)"#).unwrap()
    });
    static OPTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"\s*<option value="([^"]*)"[^>]*>[^<]*</option>"#).unwrap());
    let html = TRANSFER_FILTER.replace(&html, |c: &Captures| {
        let kept = OPTION.replace_all(&c[2], |o: &Captures| if ids.get("transfer_nonbank", &o[1]).is_some() { String::new() } else { o[0].to_owned() });
        format!("{}{kept}{}", &c[1], &c[3])
    });
    let html = report_presets(&html);
    let html = audit_history(&html);
    // Item 4: the display-name help on the customer/vendor forms names what the lists really use.
    // Exact sentence, only in the help right after the display-name input.
    html.replace(
        concat!(r#"id="id_display_name"><div class="helptext">"#, "Required. Two customers or vendors may share a name; lists tell them apart by email, or else by a number.</div>"),
        concat!(r#"id="id_display_name"><div class="helptext">"#, "Required. Two customers or vendors may share a name; use town or email to tell them apart in lists.</div>"),
    )
}

/// Item 5, only inside `<form class="filters report-controls">`: (a) the desktop's `data-start`/`data-end`
/// date pairs on the `#report-preset` options are dropped; (b) when the selected preset is not Custom,
/// the `start=`/`end=` values in the Print/CSV links (`<span class="report-actions">`) become the page's
/// own From/To box values (Django keeps stale submitted dates there; the desktop links carry the
/// resolved ones, which the boxes show on both sides). Everything else in the form stays literal.
fn report_presets(html: &str) -> String {
    static FORM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?s)<form class="filters report-controls" method="get" aria-label="Report options">.*?</form>"#).unwrap());
    static PRESET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?s)<select name="preset" id="report-preset">.*?</select>"#).unwrap());
    static DATES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(<option value="[a-z_]+") data-start="[0-9]{4}-[0-9]{2}-[0-9]{2}" data-end="[0-9]{4}-[0-9]{2}-[0-9]{2}""#).unwrap());
    static SELECTED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"<option value="([a-z_]+)"[^>]*\sselected>"#).unwrap());
    static BOX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"<input type="date" name="(start|end)" value="([0-9]{4}-[0-9]{2}-[0-9]{2})?">"#).unwrap());
    static ACTIONS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?s)<span class="report-actions">.*?</span>"#).unwrap());
    static PARAM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(\?|&amp;)(start|end)=[^&"]*"#).unwrap());
    static PAGER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?s)<nav class="pager" aria-label="Pages">.*?</nav>"#).unwrap());
    let mut shown_dates: Option<HashMap<String, String>> = None;
    let html = FORM.replace_all(html, |form: &Captures| {
        let form = PRESET.replace(&form[0], |c: &Captures| DATES.replace_all(&c[0], "${1}").into_owned()).into_owned();
        let preset = PRESET.find(&form).and_then(|p| SELECTED.captures(p.as_str())).map(|c| c[1].to_owned());
        if preset.as_deref().is_none_or(|p| p == "custom") {
            return form;
        }
        let shown: HashMap<String, String> = BOX.captures_iter(&form).map(|c| (c[1].to_owned(), c.get(2).map_or("", |m| m.as_str()).to_owned())).collect();
        if shown.len() != 2 {
            return form;
        }
        let form = ACTIONS.replace(&form, |a: &Captures| PARAM.replace_all(&a[0], |p: &Captures| format!("{}{}={}", &p[1], &p[2], shown[&p[2]])).into_owned()).into_owned();
        shown_dates = Some(shown);
        form
    }).into_owned();
    // The general ledger's pager links carry the same query (page=N) as Print/CSV.
    match shown_dates {
        Some(shown) => PAGER.replace_all(&html, |n: &Captures| PARAM.replace_all(&n[0], |p: &Captures| format!("{}{}={}", &p[1], &p[2], shown[&p[2]])).into_owned()).into_owned(),
        None => html,
    }
}

/// Extra (audit history), only on the audit history page: the desktop's added action labels
/// (`fb_app` `SIM_FIX_ACTION_LABELS`, exact code/label pairs) map back to Django's code names in the
/// Action cells and drop out of the action filter; Document cells that now resolve sales tax rates or
/// groups, the sales tax settings, deposit refunds and backups map back to Django's fallback text.
fn audit_history(html: &str) -> String {
    static FILTER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?s)(<form class="filters audit-filters" method="get" aria-label="Filter audit history">.*?<select name="action"[^>]*>)(.*?)(</select>)"#).unwrap());
    static TABLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?s)<table class="audit-history">.*?</table>"#).unwrap());
    static OPTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"\s*<option value="([a-z_.]+)"( selected)?>([^<]*)</option>"#).unwrap());
    static ACTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"<td data-label="Action">([^<]*)</td>"#).unwrap());
    static TAX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"<td class="document" data-label="Document"><a href="/c/[^/"]+/sales-tax/(?:(?:rates|groups)/[^/"]+/edit/)?">[^<]*</a></td>"#).unwrap());
    static REFUND: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"<td class="document" data-label="Document"><a href="/c/[^/"]+/deposit-refunds/[^/"]+/">([^<]*)</a></td>"#).unwrap());
    static BACKUP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"<td class="document" data-label="Document">Backup [^<]+</td>"#).unwrap());
    let code = |label: &str| SIM_FIX_LABELS.iter().find(|(_, l)| *l == label).map(|(c, _)| *c);
    let html = FILTER.replace(html, |c: &Captures| {
        let kept = OPTION.replace_all(&c[2], |o: &Captures| if SIM_FIX_LABELS.contains(&(&o[1], &o[3])) { String::new() } else { o[0].to_owned() });
        format!("{}{kept}{}", &c[1], &c[3])
    });
    TABLE.replace_all(&html, |t: &Captures| {
        let t = ACTION.replace_all(&t[0], |a: &Captures| match code(&a[1]) { Some(c) => format!(r#"<td data-label="Action">{c}</td>"#), None => a[0].to_owned() });
        let t = TAX.replace_all(&t, r#"<td class="document" data-label="Document">Record (no longer on file)</td>"#);
        let t = REFUND.replace_all(&t, r#"<td class="document" data-label="Document">${1} (no longer on file)</td>"#);
        BACKUP.replace_all(&t, r#"<td class="document" data-label="Document">Record (no longer on file)</td>"#).into_owned()
    }).into_owned()
}

/// Exact copy of `fb_app::screens::d2::audit::SIM_FIX_ACTION_LABELS` (checked by `item_extra` below).
const SIM_FIX_LABELS: [(&str, &str); 19] = [
    ("salestax.rate.created", "Sales tax rate created"),
    ("salestax.rate.updated", "Sales tax rate updated"),
    ("salestax.rate.deactivated", "Sales tax rate made inactive"),
    ("salestax.rate.reactivated", "Sales tax rate reactivated"),
    ("salestax.group.created", "Sales tax group created"),
    ("salestax.group.updated", "Sales tax group updated"),
    ("salestax.group.deactivated", "Sales tax group made inactive"),
    ("salestax.group.reactivated", "Sales tax group reactivated"),
    ("salestax.settings.updated", "Sales tax settings saved"),
    ("salestax.bulk_step", "Sales tax turn-on step finished"),
    ("customer_deposit.recorded", "Customer deposit recorded"),
    ("customer_deposit.applied", "Customer deposit applied"),
    ("customer_deposit.unapplied", "Customer deposit unapplied"),
    ("customer_deposit.voided", "Customer deposit voided"),
    ("deposit_refund.recorded", "Deposit refund recorded"),
    ("deposit_refund.voided", "Deposit refund voided"),
    ("backup.create", "Backup created"),
    ("backup.download", "Backup copy saved"),
    ("backup.restore", "Backup restored as a new book"),
];

#[cfg(test)]
mod tests {
    use super::*;

    const PANEL: &str = r#"<section class="panel estimate-document"><h2>Kettle Pond Supply</h2><p>INV-000001 · {status} · USD</p>"#;
    fn panel(status: &str) -> String {
        PANEL.replace("{status}", status)
    }

    fn none() -> IdMap {
        IdMap::new(&BTreeMap::new())
    }
    fn normalize(html: &str) -> String {
        super::normalize(html, &none())
    }

    #[test]
    fn item2_print_status_matches_djangos_open_only_on_an_invoice_print() {
        let print = |title: &str, status: &str| format!(r#"<p class="doc-title">{title}</p><p class="revision-line">Revision 2 of 2 · Status: {status}<br>Printed"#);
        for state in ["Open", "Partial", "Paid"] {
            assert_eq!(normalize(&print("INVOICE", &format!("Posted · {state}"))), print("INVOICE", "Open"), "{state}");
        }
        let paid = normalize(&print("INVOICE", "Posted · Paid"));
        for django in [print("INVOICE", "Voided"), print("INVOICE", "Draft"), print("INVOICE", "Open").replace("Revision 2", "Revision 1")] {
            assert_ne!(paid, normalize(&django), "{django}");
        }
        assert_ne!(normalize(&print("INVOICE", "Posted · Refunded")), print("INVOICE", "Open"));
        // An estimate or sales receipt print keeps the words literally.
        for title in ["ESTIMATE", "SALES RECEIPT"] {
            assert_eq!(normalize(&print(title, "Posted · Paid")), print(title, "Posted · Paid"));
        }
    }

    #[test]
    fn item2_posted_payment_state_matches_djangos_open_only_there() {
        for state in ["Open", "Partial", "Paid"] {
            assert_eq!(normalize(&panel(&format!("Posted · {state}"))), panel("Open"), "{state}");
        }
        // Every other difference in the line still diffs.
        let paid = normalize(&panel("Posted · Paid"));
        for django in [panel("Voided"), panel("Draft"), panel("Open").replace("INV-000001", "INV-000002"), panel("Open").replace("USD", "EUR")] {
            assert_ne!(paid, normalize(&django), "{django}");
        }
        assert_ne!(normalize(&panel("Posted · Refunded")), panel("Open"));
        assert_ne!(normalize(&panel("Posted · Paid").replace("INV-000001", "INV-000002")), panel("Open"));
        // Outside the invoice document panel the words stay literal.
        let elsewhere = "<p>INV-000001 · Posted · Paid · USD</p>";
        assert_eq!(normalize(elsewhere), elsewhere);
        let estimate = r#"<section class="panel estimate-document"><h2>K</h2><p>EST-000001 · Posted · Paid · USD</p>"#;
        assert_eq!(normalize(estimate), estimate);
    }
}

#[cfg(test)]
mod item3 {
    use super::*;

    fn page(options: &[(&str, &str)]) -> String {
        let opts: String = options.iter().map(|(v, l)| format!("\n  <option value=\"{v}\">{l}</option>\n")).collect();
        format!(r#"<form method="get" class="filters transfer-filters" aria-label="Filter transfers"><label>Bank (from or to) <select name="account" id="id_account">
  <option value="" selected>All banks</option>
{opts}</select></label></form>"#)
    }

    #[test]
    fn item3_only_djangos_named_non_bank_assets_drop_from_the_transfer_filter() {
        let django_ids = IdMap::new(&BTreeMap::from([("transfer_nonbank:D6:1500".to_owned(), "24".to_owned())]));
        let rust_ids = IdMap::new(&BTreeMap::new());
        let django = page(&[("20", "1000 · Checking"), ("21", "1010 · Savings"), ("24", "1500 · Equipment")]);
        let rust = page(&[("20", "1000 · Checking"), ("21", "1010 · Savings")]);
        assert_eq!(normalize(&django, &django_ids), normalize(&rust, &rust_ids));
        // The desktop listing a non-bank asset still diffs (its ids never name the class).
        assert_ne!(normalize(&django, &django_ids), normalize(&django, &rust_ids));
        // A missing or relabelled bank still diffs.
        assert_ne!(normalize(&django, &django_ids), normalize(&page(&[("20", "1000 · Checking")]), &rust_ids));
        assert_ne!(normalize(&django, &django_ids), normalize(&rust.replace("Savings", "Reserve"), &rust_ids));
        // The 400 page marks the select invalid; the rule still applies there.
        let invalid = |h: &str| h.replace(r#"<select name="account" id="id_account">"#, r#"<select name="account" aria-invalid="true" aria-describedby="id_account_error" id="id_account">"#);
        assert_eq!(normalize(&invalid(&django), &django_ids), normalize(&invalid(&rust), &rust_ids));
        // Other selects named "account" (not the transfer filter) are untouched on either side.
        let other = r#"<select name="account" id="id_account"><option value="24">1500 · Equipment</option></select>"#;
        assert_eq!(normalize(other, &django_ids), other);
    }
}

#[cfg(test)]
mod item4 {
    use super::*;
    const SPAN: &str = r#"id="id_display_name"><div class="helptext">"#;

    #[test]
    fn item4_only_the_exact_twin_hint_in_its_span_normalizes() {
        let ids = IdMap::new(&BTreeMap::new());
        let django = format!("{SPAN}Required. Two customers or vendors may share a name; use town or email to tell them apart in lists.</div>");
        let rust = format!("{SPAN}Required. Two customers or vendors may share a name; lists tell them apart by email, or else by a number.</div>");
        assert_eq!(normalize(&rust, &ids), django);
        // Any other wording, or the sentence in another field's help, still diffs.
        assert_ne!(normalize(&rust.replace("by email", "by phone"), &ids), django);
        assert_ne!(normalize(&rust.replace("Required. ", ""), &ids), django);
        let other = rust.replace("id_display_name", "id_business_name");
        assert_eq!(normalize(&other, &ids), other);
    }
}

#[cfg(test)]
mod item5 {
    use super::*;

    fn controls(options: &str, start: &str, end: &str, links: &str) -> String {
        format!(r#"<form class="filters report-controls" method="get" aria-label="Report options">
<label>Dates <select name="preset" id="report-preset">{options}</select></label>
<label>From <input type="date" name="start" value="{start}"></label>
<label>To <input type="date" name="end" value="{end}"></label>
<button type="submit">Run report</button>
<span class="report-actions">{links}</span>
</form>"#)
    }
    fn links(q: &str) -> String {
        format!(r#"<a class="button secondary" href="/r/?{q}&amp;output=print">Print</a> <a class="button secondary" href="/r/?{q}&amp;output=csv">Download CSV</a>"#)
    }
    const DJANGO_OPTS: &str = r#"<option value="this_month">This month</option><option value="last_month" selected>Last month</option><option value="custom">Custom</option>"#;
    const RUST_OPTS: &str = r#"<option value="this_month" data-start="2026-11-01" data-end="2026-11-30">This month</option><option value="last_month" data-start="2026-10-01" data-end="2026-10-31" selected>Last month</option><option value="custom">Custom</option>"#;

    #[test]
    fn item5_preset_dates_and_link_dates_normalize_only_in_the_report_controls() {
        let ids = IdMap::new(&BTreeMap::new());
        let django = controls(DJANGO_OPTS, "2026-10-01", "2026-10-31", &links("preset=last_month&amp;start=2026-01-01&amp;end=2026-01-31&amp;basis=ACCRUAL"));
        let rust = controls(RUST_OPTS, "2026-10-01", "2026-10-31", &links("preset=last_month&amp;start=2026-10-01&amp;end=2026-10-31&amp;basis=ACCRUAL"));
        assert_eq!(normalize(&django, &ids), normalize(&rust, &ids));
        // Still diffs: different shown dates, a different preset or basis, a Custom range's link dates.
        assert_ne!(normalize(&django, &ids), normalize(&rust.replace(r#"name="end" value="2026-10-31""#, r#"name="end" value="2026-10-30""#), &ids));
        assert_ne!(normalize(&django, &ids), normalize(&rust.replace("basis=ACCRUAL", "basis=CASH"), &ids));
        assert_ne!(normalize(&django, &ids), normalize(&rust.replace("preset=last_month&amp;", "preset=this_month&amp;"), &ids));
        let custom = |opts: &str, q: &str| controls(&opts.replace(" selected>Last", ">Last").replace(r#""custom">"#, r#""custom" selected>"#), "2026-10-01", "2026-10-31", &links(q));
        assert_ne!(normalize(&custom(DJANGO_OPTS, "preset=custom&amp;start=2026-01-01&amp;end=2026-10-31"), &ids), normalize(&custom(RUST_OPTS, "preset=custom&amp;start=2026-10-01&amp;end=2026-10-31"), &ids));
        // Malformed or extra option attributes stay literal.
        assert_ne!(normalize(&controls(DJANGO_OPTS, "", "", ""), &ids), normalize(&controls(&RUST_OPTS.replace("data-end", "data-stop"), "", "", ""), &ids));
        // The pager carries the same query; outside a report page it stays literal.
        let pager = |q: &str| format!(r#"<nav class="pager" aria-label="Pages"><span>Page 1 of 2</span> <a href="?{q}&amp;page=2">Next</a></nav>"#);
        let stale = "preset=last_month&amp;start=2026-01-01&amp;end=2026-01-31";
        let fresh = "preset=last_month&amp;start=2026-10-01&amp;end=2026-10-31";
        assert_eq!(normalize(&(django.clone() + &pager(stale)), &ids), normalize(&(rust.clone() + &pager(fresh)), &ids));
        assert_ne!(normalize(&(django.clone() + &pager(stale)), &ids), normalize(&(rust.clone() + &pager(&fresh.replace("last_month", "this_month"))), &ids));
        assert_eq!(normalize(&pager(stale), &ids), pager(stale));
        // Outside the report controls nothing changes.
        let other = r#"<select name="preset" id="report-preset"><option value="this_month" data-start="2026-11-01" data-end="2026-11-30">x</option></select>"#;
        assert_eq!(normalize(other, &ids), other);
    }
}

#[cfg(test)]
mod item_extra {
    use super::*;

    fn page(options: &str, rows: &str) -> String {
        format!(r#"<form class="filters audit-filters" method="get" aria-label="Filter audit history">
  <label>Action <select name="action" id="id_action">
  <option value="" selected>All actions</option>
{options}</select></label></form>
<section>
<table class="audit-history"><tbody>{rows}</tbody></table></section>"#)
    }
    fn row(action: &str, document: &str) -> String {
        format!(r#"<tr><td data-label="Action">{action}</td><td class="document" data-label="Document">{document}</td></tr>"#)
    }

    #[test]
    fn item_extra_label_table_is_the_desktops() {
        assert_eq!(SIM_FIX_LABELS, fb_app::screens::SIM_FIX_ACTION_LABELS);
    }

    #[test]
    fn item_extra_labels_and_resolved_records_map_back_only_on_the_history_page() {
        let ids = IdMap::new(&BTreeMap::new());
        let django = page("\n  <option value=\"account.saved\">Account saved</option>\n", &[
            row("salestax.rate.created", "Record (no longer on file)"),
            row("salestax.settings.updated", "Record (no longer on file)"),
            row("deposit_refund.recorded", "DRF-000001 (no longer on file)"),
            row("backup.create", "Record (no longer on file)"),
            row("bankfeed.synced", "Record (no longer on file)"),
        ].concat());
        let rust = page("\n  <option value=\"account.saved\">Account saved</option>\n\n  <option value=\"backup.create\">Backup created</option>\n\n  <option value=\"salestax.rate.created\">Sales tax rate created</option>\n", &[
            row("Sales tax rate created", r#"<a href="/c/1/sales-tax/rates/9/edit/">County</a>"#),
            row("Sales tax settings saved", r#"<a href="/c/1/sales-tax/">Sales tax settings</a>"#),
            row("Deposit refund recorded", r#"<a href="/c/1/deposit-refunds/4/">DRF-000001</a>"#),
            row("Backup created", "Backup fb-2026.fbbackup"),
            row("bankfeed.synced", "Record (no longer on file)"),
        ].concat());
        assert_eq!(normalize(&django, &ids), normalize(&rust, &ids));
        // Still diffs: a wrong label, a label on the wrong code, another document, a missing v1 option.
        assert_ne!(normalize(&django, &ids), normalize(&rust.replace("Sales tax rate created</td>", "Sales tax rate made</td>"), &ids));
        assert_ne!(normalize(&django, &ids), normalize(&rust.replace(r#""backup.create">Backup created"#, r#""backup.create">Sales tax rate created"#), &ids));
        assert_ne!(normalize(&django, &ids), normalize(&rust.replace("DRF-000001</a>", "DRF-000002</a>"), &ids));
        assert_ne!(normalize(&django, &ids), normalize(&rust.replace("account.saved\">Account saved", "account.saved\">Account kept"), &ids));
        assert_ne!(normalize(&django, &ids), normalize(&rust.replace("/sales-tax/rates/9/edit/", "/invoices/9/"), &ids));
        // Elsewhere (not the history table) the labels stay literal.
        let other = r#"<td data-label="Action">Sales tax rate created</td>"#;
        assert_eq!(normalize(other, &ids), other);
    }
}
