//! Release 1, plugins hidden (NORMALIZATION.md "Release 1: plugins hidden", N14). Used only when
//! fb-screendiff is built without the `plugins` feature, as the release app is. Django side only: its
//! plugin nav entries, the bill page's Read with AI section and the footer's bank-feed phrase are the
//! surfaces the release build leaves out. The Rust side is never rewritten, so a release page that
//! still shows one of them differs.
use super::*;

/// Before id normalization: the two nav entries (exact markup, inside the main navigation) and the
/// bill detail's Read with AI section with its stylesheet.
pub fn django_before(html: &str) -> String {
    static NAV: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)<nav class="appnav" aria-label="Main navigation">.*?</nav>"#).unwrap()
    });
    static ITEM: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(concat!(
            r#"<a href="/c/[0-9]+/bank-feeds/"(?: aria-current="page")?><img class="nav-icon" src="/static/ui/icons/sm/bank-feed\.svg" alt="" aria-hidden="true" width="22" height="22"><span>Bank feeds</span></a>"#,
            r#"|<a href="/c/[0-9]+/ai/"(?: aria-current="page")?><img class="nav-icon" src="/static/ui/icons/sm/ai-assistant\.svg" alt="" aria-hidden="true" width="22" height="22"><span>AI assistant</span></a>"#,
        ))
        .unwrap()
    });
    static READ_WITH_AI: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)<link rel="stylesheet" href="/static/assistant/assistant\.css">\s*<section class="ai-receipt" id="read-with-ai" aria-labelledby="ai-receipt-title">\s*<h2 id="ai-receipt-title">Read a receipt with AI</h2>.*?</section>"#).unwrap()
    });
    static START_FROM_RECEIPT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)<section class="ai-receipt ai-receipt-start" id="start-from-receipt" aria-labelledby="ai-start-title">\s*<h2 id="ai-start-title">Start a bill from a receipt</h2>.*?</section>"#).unwrap()
    });
    let html = NAV.replace(html, |c: &Captures| ITEM.replace_all(&c[0], "").into_owned());
    let html = READ_WITH_AI.replace(&html, "");
    START_FROM_RECEIPT.replace(&html, "").into_owned()
}

/// After normalization (N13a has already removed the payroll phrase): the exact footer suffix loses
/// ` · Bank feeds are read-only`.
pub fn django_after(normalized: &str) -> String {
    const WITH: &str = r#" · Bank feeds are read-only · <a class="report-problem" href="/help/report/">Report a problem</a></footer></body></html>"#;
    const WITHOUT: &str = r#" · <a class="report-problem" href="/help/report/">Report a problem</a></footer></body></html>"#;
    match normalized.strip_suffix(WITH) {
        Some(head) if normalized.matches("<footer>").count() == 1 => format!("{head}{WITHOUT}"),
        _ => normalized.to_owned(),
    }
}

/// A screen case whose route is a plugin route: the release build must refuse it (no page at all).
/// Anything it renders is a difference.
pub fn hidden_outcome(rendered: Result<u16, String>) -> Result<(), String> {
    match rendered {
        Err(message) if message.starts_with("not found") => Ok(()),
        Err(message) => Err(format!("refused for another reason: {message}")),
        Ok(status) => Err(format!("rendered with status {status} in a release build")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ICON: &str = r#"<img class="nav-icon" src="/static/ui/icons/sm/bank-feed.svg" alt="" aria-hidden="true" width="22" height="22">"#;
    const AI_ICON: &str = r#"<img class="nav-icon" src="/static/ui/icons/sm/ai-assistant.svg" alt="" aria-hidden="true" width="22" height="22">"#;

    fn nav(inner: &str) -> String {
        format!(r#"<nav class="appnav" aria-label="Main navigation"><a href="/c/7/registers/">R</a>{inner}</nav>"#)
    }

    #[test]
    fn only_the_two_exact_nav_entries_go_and_only_inside_the_main_nav() {
        let django = nav(&format!(r#"<a href="/c/7/bank-feeds/">{ICON}<span>Bank feeds</span></a><a href="/c/7/ai/" aria-current="page">{AI_ICON}<span>AI assistant</span></a>"#));
        assert_eq!(django_before(&django), nav(""));
        // A renamed label, another address or the same link outside the nav is left alone.
        for kept in [nav(&format!(r#"<a href="/c/7/bank-feeds/">{ICON}<span>Bank feed</span></a>"#)), nav(&format!(r#"<a href="/c/7/bank-feeds/rules/">{ICON}<span>Bank feeds</span></a>"#))] {
            assert_eq!(django_before(&kept), kept);
        }
        let outside = format!(r#"<p><a href="/c/7/bank-feeds/">{ICON}<span>Bank feeds</span></a></p>"#);
        assert_eq!(django_before(&outside), outside);
        // The Rust side is never rewritten: a release page that kept an entry still differs.
        let rust_kept = nav(&format!(r#"<a href="/c/7/bank-feeds/">{ICON}<span>Bank feeds</span></a>"#));
        assert_ne!(django_before(&django), rust_kept);
    }

    #[test]
    fn the_read_with_ai_section_goes_with_its_stylesheet_only() {
        let section = r#"<link rel="stylesheet" href="/static/assistant/assistant.css">
<section class="ai-receipt" id="read-with-ai" aria-labelledby="ai-receipt-title"><h2 id="ai-receipt-title">Read a receipt with AI</h2>
<p class="muted">Only a Draft bill can be filled in from a receipt.</p>
</section>"#;
        assert_eq!(django_before(&format!("<main>{section}</main>")), "<main></main>");
        // Another heading is not the hidden section.
        let other = section.replace("Read a receipt with AI", "Read a receipt");
        assert_eq!(django_before(&other), other);
        // The new-bill page's receipt reader goes too, only as that exact section.
        let start = r#"<section class="ai-receipt ai-receipt-start" id="start-from-receipt" aria-labelledby="ai-start-title">
<h2 id="ai-start-title">Start a bill from a receipt</h2>
<form method="post" action="/c/7/ai/receipt/upload/"><button type="submit">Read with AI</button></form>
</section>"#;
        assert_eq!(django_before(&format!("<main>{start}<form></form></main>")), "<main><form></form></main>");
        let renamed = start.replace("Start a bill from a receipt", "Start a bill");
        assert_eq!(django_before(&renamed), renamed);
    }

    #[test]
    fn the_footer_phrase_goes_only_in_the_exact_single_footer_suffix() {
        let page = |middle: &str| format!(r#"<main></main><footer>FreedomBooks · USD{middle} · <a class="report-problem" href="/help/report/">Report a problem</a></footer></body></html>"#);
        assert_eq!(django_after(&page(" · Bank feeds are read-only")), page(""));
        // A second footer, other text or trailing markup stays.
        let two = format!("<footer>x</footer>{}", page(" · Bank feeds are read-only"));
        assert_eq!(django_after(&two), two);
        let trailing = format!("{}<p>x</p>", page(" · Bank feeds are read-only"));
        assert_eq!(django_after(&trailing), trailing);
        assert_eq!(django_after(&page(" · Bank feeds are read only")), page(" · Bank feeds are read only"));
    }

    #[test]
    fn a_hidden_screen_that_renders_is_a_difference() {
        assert!(hidden_outcome(Err("not found: /c/x/bank-feeds/".into())).is_ok());
        assert!(hidden_outcome(Ok(200)).is_err());
        assert!(hidden_outcome(Ok(404)).is_err(), "a page, even a 404 page, is not the refusal");
        assert!(hidden_outcome(Err("invalid: form expired".into())).is_err());
    }
}
