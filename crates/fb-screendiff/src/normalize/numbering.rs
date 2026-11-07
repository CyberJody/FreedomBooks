//! The new settings section has its own recorded screen; all pre-existing company fields still diff.
use super::*;
fn section() -> &'static Regex {
    static SECTION: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)<section id="document-numbering" aria-labelledby="numbering-title">.*?</section>"#).unwrap()
    });
    &SECTION
}
pub(super) fn without_section(html: &str) -> String {
    section().replace(html, "").into_owned()
}
pub fn snapshot(html: &str, ids: &IdMap) -> Option<String> {
    let text = section().find(html)?.as_str();
    static BANK: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"name="bank_id" value="([^"]+)""#).unwrap());
    let text = BANK.replace_all(text, |c: &Captures| match ids.get("account", &c[1]) {
        Some(id) => format!(r#"name="bank_id" value="{id}""#),
        None => c[0].to_owned(),
    });
    let text = TOKEN.replace_all(&text, r#"<input type="hidden" name="{form-token}">"#);
    let text = WS.replace_all(&text, " ");
    Some(BETWEEN_TAGS.replace_all(&text, "><").trim().to_owned())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn removal_is_limited_to_the_new_section_and_snapshot_keeps_values_and_errors() {
        let html = r#"<h1>Edit company</h1><section id="document-numbering" aria-labelledby="numbering-title"><p>INV-000123</p></section><p>Other company field</p>"#;
        let ids = IdMap::new(&BTreeMap::new());
        assert_eq!(
            without_section(html),
            "<h1>Edit company</h1><p>Other company field</p>"
        );
        assert!(snapshot(html, &ids).unwrap().contains("INV-000123"));
        assert_ne!(
            snapshot(html, &ids),
            snapshot(&html.replace("000123", "000124"), &ids)
        );
        assert!(snapshot("<section>other</section>", &ids).is_none());
    }
}
