//! Generated allocation field IDs and event evidence, each restricted to its HTML context.
use super::*;
pub(super) fn normalize(html: &str, ids: &IdMap) -> String {
    static FIELD: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"((?:name|id|for|aria-describedby)="(?:id_)?amount_)([0-9a-f-]+)(_error)?(")"#)
            .unwrap()
    });
    static EVENT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(<tr><td>)([0-9]{4}-[0-9]{2}-[0-9]{2} [0-9]{2}:[0-9]{2})(</td><td>(?:credit_memo|customer_deposit)\.[^<]+</td>)"#).unwrap()
    });
    static APP: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r#"(<td>credit_memo\.unapplied</td><td>[^<]*</td><td class="long-text">application #)([0-9]+)( unapplied on [0-9-]+ · reason:)"#,
        )
        .unwrap()
    });
    let html = FIELD.replace_all(html, |c: &Captures| {
        format!(
            "{}{}{}{}",
            &c[1],
            ids.get("invoice", &c[2]).unwrap_or(&c[2]),
            c.get(3).map_or("", |c| c.as_str()),
            &c[4]
        )
    });
    let html = EVENT.replace_all(&html, |c: &Captures| {
        format!(
            "{}{}{}",
            &c[1],
            if ids.get("d5_event_stamp", &c[2]).is_some() {
                "{d5-event-time}"
            } else {
                &c[2]
            },
            &c[3]
        )
    });
    APP.replace_all(&html, |c: &Captures| {
        let known = ids.by_model.get("creditapplication").and_then(|m| {
            m.iter()
                .find(|(raw, _)| {
                    ids.labels
                        .get(&("creditapplication".into(), (*raw).clone()))
                        .map(String::as_str)
                        .unwrap_or(raw)
                        == &c[2]
                })
                .map(|(_, sym)| sym.as_str())
        });
        format!("{}{}{}", &c[1], known.unwrap_or(&c[2]), &c[3])
    })
    .into_owned()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn allocation_ids_and_event_evidence_do_not_rewrite_customer_text() {
        let ids = IdMap::new(&BTreeMap::from([
            ("invoice:INV-1".into(), "42".into()),
            ("d5_event_stamp:known".into(), "2026-11-01 12:34".into()),
        ]));
        let prose = "<p>amount_42 2026-11-01 12:34</p><input name=\"memo\" value=\"amount_42\">";
        assert_eq!(normalize(prose, &ids), prose);
        assert!(
            normalize("<input name=\"amount_42\" id=\"id_amount_42\">", &ids)
                .contains("amount_{invoice:INV-1}")
        );
        assert!(normalize("<input name=\"amount_43\">", &ids).contains("amount_43"));
        assert_ne!(
            normalize(prose, &ids),
            normalize(&prose.replace("42", "43"), &ids)
        );
    }

    #[test]
    fn equal_sales_and_bill_clocks_do_not_change_the_owning_normalization() {
        let py = IdMap::new(&BTreeMap::from([
            ("sales_stamp:recorded".into(), "2026-11-01 12:34:56".into()),
            ("document_stamp:bill".into(), "2026-11-01 12:34:56".into()),
        ]));
        let rs = IdMap::new(&BTreeMap::from([
            ("sales_stamp:recorded".into(), "2026-11-01 12:35:56".into()),
            ("document_stamp:bill".into(), "2026-11-01 12:36:56".into()),
        ]));
        let bill = r#"<table class="bill-history"><tr><td>owner<br>2026-11-01 12:34:56 UTC</td></tr></table>"#;
        let rust_bill = bill.replace("12:34:56", "12:36:56");
        assert_eq!(
            crate::normalize::normalize(bill, &py),
            crate::normalize::normalize(&rust_bill, &rs)
        );
        let sale = r#"<h2>Revision history</h2><table class="invoice-history"><tr><td>owner<br>2026-11-01 12:34:56 UTC</td></tr></table>"#;
        let rust_sale = sale.replace("12:34:56", "12:35:56");
        assert_eq!(
            crate::normalize::normalize(sale, &py),
            crate::normalize::normalize(&rust_sale, &rs)
        );
        let literal = "<p>2026-11-01 12:34:56</p>";
        assert_ne!(
            crate::normalize::normalize(literal, &py),
            crate::normalize::normalize(&literal.replace("12:34:56", "12:36:56"), &rs)
        );
    }

    #[test]
    fn document_line_text_cannot_be_mistaken_for_generated_history() {
        let ids = IdMap::new(&BTreeMap::from([
            ("journal:J000001".into(), "journal-uuid".into()),
            ("label:journal:J000001".into(), "1".into()),
            ("creditapplication:CM-1:1".into(), "application-uuid".into()),
            ("label:creditapplication:CM-1:1".into(), "2".into()),
        ]));
        for text in [
            r#"<td class="long-text">void dated 2026-11-01 · journal #1 · reason: typed</td>"#,
            r#"<td class="long-text">application #2 unapplied on 2026-11-01 · reason: typed</td>"#,
        ] {
            assert_eq!(crate::normalize::normalize(text, &ids), text);
        }
    }
}
