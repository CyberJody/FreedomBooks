//! D3 identities are rewritten only in known model/markup contexts.
use super::*;
fn short(label: &str, model: &str, id: &str, ids: &IdMap) -> String {
    generated_label(label, model, id, ids)
}

pub(super) fn normalize(html: &str, ids: &IdMap) -> String {
    static RECORD: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?s)(<tr data-record-id=")([^"]+)(">.*?</tr>)"#).unwrap());
    static TAX: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"data-tax-item="([^"]+)""#).unwrap());
    static CONTACT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(<a href="/c/[^/]+/lists/(customers|vendors)/([^/]+)/">)([^<]*)(</a>)"#)
            .unwrap()
    });
    static TAX_SELECT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)(<select name="tax_mode"[^>]*>)(.*?)(</select>)"#).unwrap()
    });
    static MEMBERS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(<input type="checkbox" name="members" value=")([^"]+)("[^>]*>)([^<]*)"#)
            .unwrap()
    });
    static AUDIT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r#"(<li>)([0-9]{4}-[0-9]{2}-[0-9]{2} [0-9]{2}:[0-9]{2}:[0-9]{2})( UTC · [^<]*</li>)"#,
        )
        .unwrap()
    });
    let text = RECORD.replace_all(html, |c: &Captures| {
        let model = if c[3].contains("/lists/customers/") {
            "customer"
        } else if c[3].contains("/lists/vendors/") {
            "vendor"
        } else if c[3].contains("/lists/items/") {
            "item"
        } else {
            return c[0].into();
        };
        format!(
            "{}{}{}",
            &c[1],
            ids.get(model, &c[2]).unwrap_or(&c[2]),
            &c[3]
        )
    });
    let text = TAX.replace_all(&text, |c: &Captures| {
        format!(
            r#"data-tax-item="{}""#,
            ids.get("taxitem", &c[1]).unwrap_or(&c[1])
        )
    });
    let text = CONTACT.replace_all(&text, |c: &Captures| {
        format!(
            "{}{}{}",
            &c[1],
            short(
                &c[4],
                if &c[2] == "customers" {
                    "customer"
                } else {
                    "vendor"
                },
                &c[3],
                ids
            ),
            &c[5]
        )
    });
    let text = TAX_SELECT.replace_all(&text, |c: &Captures| {
        format!(
            "{}{}{}",
            &c[1],
            OPTION.replace_all(&c[2], |o: &Captures| {
                if let Some(raw) = o[1].strip_prefix('T')
                    && let Some(sym) = ids.get("taxitem", raw)
                {
                    format!(r#"<option value="T{sym}"{}>{}</option>"#, &o[2], &o[3])
                } else {
                    o[0].into()
                }
            }),
            &c[3]
        )
    });
    let text = MEMBERS.replace_all(&text, |c: &Captures| {
        format!(
            "{}{}{}{}",
            &c[1],
            ids.get("taxitem", &c[2]).unwrap_or(&c[2]),
            &c[3],
            &c[4]
        )
    });
    let text = AUDIT.replace_all(&text, |c: &Captures| {
        format!(
            "{}{}{}",
            &c[1],
            ids.get("audit_utc_stamp", &c[2]).unwrap_or(&c[2]),
            &c[3]
        )
    });
    DIGEST
        .replace_all(&text, |c: &Captures| {
            if ids
                .by_model
                .get("bulk_digest")
                .is_some_and(|m| m.contains_key(&c[1]))
            {
                r#"name="digest" value="{bulk-digest}""#.to_owned()
            } else {
                c[0].to_owned()
            }
        })
        .into_owned()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn labels_and_ids_remain_model_and_context_specific() {
        let ids = IdMap::new(&BTreeMap::from([
            ("vendor:A:#1".into(), "7".into()),
            ("label:vendor:A:#1".into(), "7".into()),
            ("taxitem:County".into(), "2".into()),
        ]));
        assert_eq!(normalize(r#"<p>7 (#7)</p>"#, &ids), r#"<p>7 (#7)</p>"#);
        assert_eq!(short("A (#7)", "vendor", "7", &ids), "A (#{vendor:A:#1})");
        assert_eq!(short("A (#8)", "vendor", "7", &ids), "A (#8)");
        assert!(
            normalize(
                r#"<input type="checkbox" name="members" value="2" id="id_members_0">"#,
                &ids
            )
            .contains("{taxitem:County}")
        );
        assert!(
            !normalize(
                r#"<input type="checkbox" name="unrelated" value="2">"#,
                &ids
            )
            .contains("{taxitem")
        );
    }
}
