//! D7a generated identities in receipt controls and structured check snapshot evidence.
use super::*;
fn plain(text: &str) -> String {
    text.replace("&#x27;", "'")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}
fn snapshot(text: &str, model: &str, ids: &IdMap) -> String {
    let Some(mut v) = literal::parse(&plain(text)) else {
        return text.into();
    };
    if let Some(m) = v.as_object_mut() {
        for (k, model) in [
            ("id", model),
            ("expense_account_id", "account"),
            ("income_account_id", "account"),
        ] {
            if let Some(raw) = m.get_mut(k) {
                let id = raw
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| raw.to_string());
                if let Some(sym) = ids.get(model, &id) {
                    *raw = json_string(sym);
                }
            }
        }
    }
    fb_templates::django::escape(&serde_json::to_string(&canonical(v)).unwrap())
}
fn json_string(s: &str) -> Value {
    Value::String(s.into())
}
pub(super) fn normalize(html: &str, ids: &IdMap) -> String {
    static HISTORY: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)(<table class="(?:check|deposit)-history">)(.*?)(</table>)"#).unwrap()
    });
    static CREATED: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(<br>)([0-9]{4}-[0-9]{2}-[0-9]{2} [0-9]{2}:[0-9]{2}:[0-9]{2})( UTC</td>)"#)
            .unwrap()
    });
    static LINE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(<p>Line [0-9]+ \((?:expense|item)\) · catalog: )(.*?)( · account: )(.*?)(</p>)",
        )
        .unwrap()
    });
    static PAYEE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(<p>Payee snapshot: )(.*?)(</p>)").unwrap());
    static RECEIPT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(<input type="checkbox" name="receipts" value=")([^"]+)(" id="receipt-)([^"]+)("[^>]*>)"#).unwrap()
    });
    static LABEL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(<label for="receipt-)([^"]+)(">)"#).unwrap());
    static REV: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(<p>Saved revision \([^<]*? by [^<]*? at )([0-9]{4}-[0-9]{2}-[0-9]{2} [0-9]{2}:[0-9]{2}:[0-9]{2})( UTC\)\.)").unwrap()
    });
    let html = HISTORY.replace_all(html, |c: &Captures| {
        format!(
            "{}{}{}",
            &c[1],
            CREATED.replace_all(&c[2], |c: &Captures| format!(
                "{}{}{}",
                &c[1],
                ids.get("document_stamp", &c[2]).unwrap_or(&c[2]),
                &c[3]
            )),
            &c[3]
        )
    });
    let text = LINE.replace_all(&html, |c: &Captures| {
        format!(
            "{}{}{}{}{}",
            &c[1],
            snapshot(&c[2], "item", ids),
            &c[3],
            snapshot(&c[4], "account", ids),
            &c[5]
        )
    });
    let text = PAYEE.replace_all(&text, |c: &Captures| {
        format!("{}{}{}", &c[1], snapshot(&c[2], "vendor", ids), &c[3])
    });
    let text = RECEIPT.replace_all(&text, |c: &Captures| {
        format!(
            "{}{}{}{}{}",
            &c[1],
            ids.get("receipt", &c[2]).unwrap_or(&c[2]),
            &c[3],
            ids.get("receipt", &c[4]).unwrap_or(&c[4]),
            &c[5]
        )
    });
    let text = LABEL.replace_all(&text, |c: &Captures| {
        format!(
            "{}{}{}",
            &c[1],
            ids.get("receipt", &c[2]).unwrap_or(&c[2]),
            &c[3]
        )
    });
    REV.replace_all(&text, |c: &Captures| {
        format!(
            "{}{}{}",
            &c[1],
            ids.get("document_stamp", &c[2]).unwrap_or(&c[2]),
            &c[3]
        )
    })
    .into_owned()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshot_ids_are_typed_and_user_text_must_diff() {
        let py = IdMap::new(&BTreeMap::from([
            ("vendor:Supplier".into(), "1".into()),
            ("account:5000".into(), "2".into()),
        ]));
        let rs = IdMap::new(&BTreeMap::from([
            ("vendor:Supplier".into(), "vendor-uuid".into()),
            ("account:5000".into(), "account-uuid".into()),
        ]));
        let wrap = |body: &str| {
            format!(
                "<p>Payee snapshot: {}</p>",
                fb_templates::django::escape(body)
            )
        };
        assert_eq!(
            normalize(&wrap("{'id': 1, 'display_name': 'Supplier #1'}"), &py),
            normalize(
                &wrap("{'display_name': 'Supplier #1', 'id': 'vendor-uuid'}"),
                &rs
            )
        );
        assert_ne!(
            normalize(&wrap("{'id': 1, 'display_name': 'Supplier #1'}"), &py),
            normalize(
                &wrap("{'id': 'vendor-uuid', 'display_name': 'Supplier #vendor-uuid'}"),
                &rs
            )
        );
        let p = "<p>Typed {'id': 1} · 2026-11-01 17:00:00 · #1</p>";
        assert_eq!(normalize(p, &py), p);
        assert_ne!(
            normalize(&wrap("{'id': 2}"), &py),
            normalize(&wrap("{'id': 'account-uuid'}"), &rs)
        );
    }
    #[test]
    fn receipt_controls_preserve_wrong_models_and_unknown_ids() {
        let ids = IdMap::new(&BTreeMap::from([
            ("receipt:RCP-000001".into(), "7".into()),
            ("account:1000".into(), "8".into()),
        ]));
        let input = r#"<input type="checkbox" name="receipts" value="7" id="receipt-7"><label for="receipt-7">RCP-000001</label>"#;
        assert!(normalize(input, &ids).contains("{receipt:RCP-000001}"));
        assert_eq!(
            normalize(&input.replace('7', "8"), &ids),
            input.replace('7', "8")
        );
        let memo = r#"<input name="memo" value="7"><p>receipt-7</p>"#;
        assert_eq!(normalize(memo, &ids), memo);
    }
    #[test]
    fn document_history_times_do_not_depend_on_audit_second_coincidence() {
        let ids = IdMap::new(&BTreeMap::from([(
            "document_stamp:2026-11-01 17:00:00".into(),
            "2026-11-01 17:00:00".into(),
        )]));
        let history =
            r#"<table class="check-history"><td>owner<br>2026-11-01 17:00:00 UTC</td></table>"#;
        assert!(normalize(history, &ids).contains("{document_stamp}"));
        let unknown = history.replace("17:00:00", "17:00:01");
        assert_eq!(normalize(&unknown, &ids), unknown);
        let prose = "<p>2026-11-01 17:00:00 UTC</p>";
        assert_eq!(normalize(prose, &ids), prose);
        let other = history.replace("check-history", "other");
        assert_eq!(normalize(&other, &ids), other);
    }
}
