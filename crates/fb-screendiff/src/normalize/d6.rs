//! Generated review capabilities and document creation stamps, only in their known fields.
use super::*;
pub(super) fn normalize(html:&str,ids:&IdMap)->String {
    static REVIEW:LazyLock<Regex>=LazyLock::new(||Regex::new(r#"(name="review_token" value=")([^"]+)(")"#).unwrap());
    static HISTORY:LazyLock<Regex>=LazyLock::new(||Regex::new(r#"(?s)(<table class="(?:bill|payment|transfer)-history">)(.*?)(</table>)"#).unwrap());
    static CREATED:LazyLock<Regex>=LazyLock::new(||Regex::new(r#"(<br>)([0-9]{4}-[0-9]{2}-[0-9]{2} [0-9]{2}:[0-9]{2}:[0-9]{2})( UTC</td>)"#).unwrap());
    static REVISION:LazyLock<Regex>=LazyLock::new(||Regex::new(r#"(<p>Read-only evidence · [^<]*? · [^<]*? · )([^<]*?)( · [^<]*</p>)"#).unwrap());
    // A successful review renders its posting preview. Invalid bound fields can carry
    // caller text in the same hidden input and must compare literally.
    let generated_review=["class=\"bill-post\"","class=\"payment-review\"","id=\"transfer-review\"","class=\"deposit-review\""].iter().any(|marker|html.contains(marker));
    let text=REVIEW.replace_all(html,|c:&Captures|if generated_review {format!("{}{{review-token}}{}",&c[1],&c[3])}else{c[0].to_owned()});
    let text=HISTORY.replace_all(&text,|c:&Captures|format!("{}{}{}",&c[1],CREATED.replace_all(&c[2],|c:&Captures|format!("{}{}{}",&c[1],ids.get("document_stamp",&c[2]).unwrap_or(&c[2]),&c[3])),&c[3]));
    if html.contains("/static/payables/bills/") {
        REVISION.replace_all(&text,|c:&Captures|format!("{}{}{}",&c[1],ids.get("document_default_stamp",&c[2]).unwrap_or(&c[2]),&c[3])).into_owned()
    } else { text.into_owned() }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn distinct_bound_invalid_review_tokens_must_diff_across_all_form_families() {
        let ids = IdMap::new(&BTreeMap::new());
        for family in ["credit-memos", "customer-deposits", "payments", "transfers", "deposits", "receipts", "sales-receipts", "bills", "checks"] {
            for (a,b) in [("bad-a", "bad-b"), ("ey-invalid-a", "ey-invalid-b"),
                ("00000000-0000-4000-8000-000000000001", "00000000-0000-4000-8000-000000000002")] {
                let page = |token: &str| format!(r#"<form action="/{family}/new/"><ul class="errorlist nonfield"><li>Invalid review.</li></ul><input type="hidden" name="review_token" value="{token}"></form>"#);
                assert_ne!(super::super::normalize(&page(a), &ids), super::super::normalize(&page(b), &ids), "{family}");
            }
        }
    }
    #[test]
    fn capabilities_and_created_stamps_are_only_normalized_in_their_fields() {
        let ids=IdMap::new(&BTreeMap::from([("document_stamp:2026-11-01 12:00:00".into(),"2026-11-01 12:00:00".into())]));
        let stamp="2026-11-01 12:00:00";
        let ordinary=format!("<p>{stamp}</p><input name=\"memo\" value=\"token\">");
        assert_eq!(normalize(&ordinary,&ids),ordinary);
        let history=format!("<table class=\"bill-history\"><tr><td>owner<br>{stamp} UTC</td></tr></table>");
        assert!(normalize(&history,&ids).contains("{document_stamp}"));
        let token=r#"<input name="review_token" value="capability">"#;
        assert_eq!(normalize(token,&ids),token);
        assert!(normalize(&format!(r#"<section class="deposit-review">{token}</section>"#),&ids).contains("{review-token}"));
        assert_eq!(normalize("<table class=\"other\"><br>2026-11-01 12:00:00 UTC</td></table>",&ids),"<table class=\"other\"><br>2026-11-01 12:00:00 UTC</td></table>");
    }
}
