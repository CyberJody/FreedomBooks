//! Only the capability added by the desktop print button is absent from the Django toolbar.
use super::*;
pub(super) fn normalize(html: &str, ids: &IdMap) -> String {
    static BUTTON: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(<button type="button" id="print-now">Print / Save as PDF)<input type="hidden" name="fb_command_token" value="[A-Za-z0-9]+">(</button>)"#).unwrap()
    });
    let mut html = BUTTON.replace_all(html, "$1$2").into_owned();
    // Frozen sales-receipt print incorrectly uses invoices:detail with a RECEIPT pk. Preserve that
    // URL and label, but map its actual model rather than an unrelated numerically colliding invoice.
    if html.contains("<p class=\"doc-title\">SALES RECEIPT") {
        let back =
            Regex::new(r#"<a href="/c/([^/]+)/invoices/([^/]+)/">Back to sales_receipt</a>"#)
                .unwrap();
        html = back
            .replace_all(&html, |c: &Captures| {
                match (ids.get("company", &c[1]), ids.get("receipt", &c[2])) {
                    (Some(company), Some(receipt)) => format!(
                        r#"<a href="/c/{company}/invoices/{receipt}/">Back to sales_receipt</a>"#
                    ),
                    _ => c[0].to_owned(),
                }
            })
            .into_owned();
    }
    if html.contains("class=\"sheet report-paper rec-paper\"") {
        let header = Regex::new(r#"(?s)(<header class="report-head">)(.*?)(</header>)"#).unwrap();
        let stamps = Regex::new(r#"(<p class="report-period">(?:[^<]* · statement [0-9-]+ · reconciled |Undone ))([0-9]{4}-[0-9]{2}-[0-9]{2} [0-9]{2}:[0-9]{2})( UTC by )"#).unwrap();
        html = header
            .replace_all(&html, |c: &Captures| {
                let body = stamps.replace_all(&c[2], |s: &Captures| {
                    format!(
                        "{}{}{}",
                        &s[1],
                        if ids.get("reconciliation_stamp", &s[2]).is_some() {
                            "{reconciliation-stamp}"
                        } else {
                            &s[2]
                        },
                        &s[3]
                    )
                });
                format!("{}{}{}", &c[1], body, &c[3])
            })
            .into_owned();
    }
    html
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_document_text_and_every_other_button_attribute() {
        let ids = IdMap::new(&BTreeMap::new());
        let a = r#"<button type="button" id="print-now">Print / Save as PDF<input type="hidden" name="fb_command_token" value="abcdef123"></button>"#;
        let b = r#"<button type="button" id="print-now">Print / Save as PDF</button>"#;
        assert_eq!(normalize(a, &ids), b);
        let text = r#"<p>fb_command_token abcdef123</p><input type="hidden" name="fb_command_token" value="abcdef123">"#;
        assert_eq!(normalize(text, &ids), text);
        assert_ne!(
            normalize(&a.replace("Save as PDF", "Save as CSV"), &ids),
            normalize(b, &ids)
        );
    }

    #[test]
    fn print_reconciliation_times_are_known_stored_values_in_header_fields_only() {
        let ids = IdMap::new(&BTreeMap::from([(
            "reconciliation_stamp:2026-11-02 15:00".into(),
            "2026-11-02 15:00".into(),
        )]));
        let html = r#"<article class="sheet report-paper rec-paper"><header class="report-head"><p class="report-period">100 · Bank · statement 2026-10-31 · reconciled 2026-11-02 15:00 UTC by owner</p><p class="report-period">Undone 2026-11-02 15:00 UTC by owner: reason 2026-11-02 15:00</p></header><td>2026-11-02 15:00</td></article>"#;
        let result = normalize(html, &ids);
        assert_eq!(result.matches("{reconciliation-stamp}").count(), 2);
        assert!(result.contains("reason 2026-11-02 15:00"));
        assert!(result.contains("<td>2026-11-02 15:00</td>"));
        assert_ne!(
            result,
            normalize(
                &html.replacen(
                    "reconciled 2026-11-02 15:00",
                    "reconciled 2026-11-02 15:01",
                    1
                ),
                &ids
            )
        );
    }

    #[test]
    fn sales_receipt_print_back_link_maps_receipt_model_even_when_an_invoice_pk_collides() {
        let ids = IdMap::new(&BTreeMap::from([
            ("company:Books".into(), "1".into()),
            ("receipt:SR-000001".into(), "2".into()),
            ("invoice:INV-000002".into(), "2".into()),
            ("receipt:SR-000002".into(), "3".into()),
        ]));
        let html = r#"<p class="doc-title">SALES RECEIPT</p><a href="/c/1/invoices/2/">Back to sales_receipt</a>"#;
        let result = normalize(html, &ids);
        assert!(result.contains("/invoices/{receipt:SR-000001}/"));
        assert!(!result.contains("INV-000002"));
        assert_ne!(
            result,
            normalize(&html.replace("invoices/2/", "invoices/3/"), &ids)
        );
        let plain = r#"<a href="/c/1/invoices/2/">Back to sales_receipt</a>"#;
        assert_eq!(normalize(plain, &ids), plain);
    }
}
