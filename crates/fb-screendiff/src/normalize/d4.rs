//! Sales review capabilities and generated event times, only in their display/widget contexts.
use super::*;
pub(super) fn normalize(html: &str, ids: &IdMap) -> String {
    let mut stamps = HashMap::new();
    for rows in [
        ids.by_model.get("audit_utc_stamp"),
        ids.by_model.get("sales_stamp"),
    ]
    .into_iter()
    .flatten()
    {
        for raw in rows.keys() {
            if let Ok(t) = chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S") {
                let iso = t.and_utc().to_rfc3339();
                // The formatter is shared with the actual template port, not a broad timestamp regex.
                let months = [
                    "Jan.", "Feb.", "March", "April", "May", "June", "July", "Aug.", "Sept.",
                    "Oct.", "Nov.", "Dec.",
                ];
                use chrono::{Datelike, Timelike};
                let time = if t.hour() == 0 && t.minute() == 0 {
                    "midnight".into()
                } else if t.hour() == 12 && t.minute() == 0 {
                    "noon".into()
                } else {
                    format!(
                        "{}{} {}",
                        if t.hour() % 12 == 0 {
                            12
                        } else {
                            t.hour() % 12
                        },
                        if t.minute() == 0 {
                            String::new()
                        } else {
                            format!(":{:02}", t.minute())
                        },
                        if t.hour() < 12 { "a.m." } else { "p.m." }
                    )
                };
                for s in [
                    raw.clone(),
                    format!(
                        "{} {}, {}, {}",
                        months[t.month0() as usize],
                        t.day(),
                        t.year(),
                        time
                    ),
                    t.format("%Y-%m-%d %H:%M").to_string(),
                    iso,
                ] {
                    stamps.insert(s, "{sales-event-time}");
                }
            }
        }
    }
    static ACTOR: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(<td>[^<]*<br>)([^<]*?)( UTC)?(</td>)").unwrap());
    static HIST: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(<p>Read-only evidence · [^·<]* · [^·<]* · )([^·<]*)( · )").unwrap()
    });
    static EVENT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(<tr><td>)([^<]*)(</td><td>sales_receipt\.[^<]*</td>)").unwrap()
    });
    static REVIEW: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(name="review_token" value=")([^"]+)(")"#).unwrap());
    static VOID_JOURNAL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(<td>(?:sales_receipt|credit_memo)\.voided</td><td>[^<]*</td><td class="long-text">void dated [0-9-]+ · journal #)([^ <]+)( · reason:)"#)
            .unwrap()
    });
    let html = VOID_JOURNAL.replace_all(html, |c: &Captures| {
        let known = ids.by_model.get("journal").and_then(|m| {
            m.iter()
                .find(|(raw, _)| {
                    ids.labels
                        .get(&("journal".into(), (*raw).clone()))
                        .map(String::as_str)
                        .unwrap_or(raw)
                        == &c[2]
                })
                .map(|(_, sym)| sym.as_str())
        });
        format!("{}{}{}", &c[1], known.unwrap_or(&c[2]), &c[3])
    });
    static SALES_HISTORY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?s)(<h2>Revision history</h2><table(?: class="invoice-history")?>|<h2>Immutable history</h2><table class="receipt-history">)(.*?)(</table>)"#).unwrap());
    let text = SALES_HISTORY.replace_all(&html, |table: &Captures| {
        let body = ACTOR.replace_all(&table[2], |c: &Captures| format!("{}{}{}{}", &c[1], stamps.get(&c[2]).copied().unwrap_or(&c[2]), c.get(3).map(|c|c.as_str()).unwrap_or(""), &c[4]));
        format!("{}{}{}", &table[1], body, &table[3])
    });
    let sales_revision = html.contains("/static/sales/estimates.css") || html.contains("/static/sales/invoices.css");
    let text = HIST.replace_all(&text, |c: &Captures| {
        if !sales_revision { return c[0].to_owned(); }
        format!(
            "{}{}{}",
            &c[1],
            stamps.get(&c[2]).copied().unwrap_or(&c[2]),
            &c[3]
        )
    });
    let text = EVENT.replace_all(&text, |c: &Captures| {
        format!(
            "{}{}{}",
            &c[1],
            stamps.get(&c[2]).copied().unwrap_or(&c[2]),
            &c[3]
        )
    });
    if !html.contains("class=\"invoice-post\"") && !html.contains("class=\"receipt-review\"") {
        return text.into_owned();
    }
    REVIEW
        .replace_all(&text, |c: &Captures| {
            if c[2].len() == 36 || c[2].starts_with("ey") {
                format!("{}{{review-capability}}{}", &c[1], &c[3])
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
    fn capabilities_only_normalize_on_successful_review_markup() {
        let ids = IdMap::new(&BTreeMap::new());
        let token = r#"<input name="review_token" value="0195b4a4-55af-719e-96cb-a2f946b067bf">"#;
        assert_eq!(normalize(token, &ids), token);
        let review = format!(r#"<section class="receipt-review">{token}</section>"#);
        assert!(normalize(&review, &ids).contains("{review-capability}"));
        let invalid = review.replace("0195b4a4-55af-719e-96cb-a2f946b067bf", "bad-token");
        assert_eq!(normalize(&invalid, &ids), invalid);
    }

    #[test]
    fn event_time_and_void_journal_are_context_and_identity_scoped() {
        let ids = IdMap::new(&BTreeMap::from([
            ("sales_stamp:created".into(), "2026-11-01 16:30:00".into()),
            ("journal:reversal".into(), "journal-uuid".into()),
            ("label:journal:reversal".into(), "4".into()),
        ]));
        let prose = "<p>2026-11-01 16:30:00 · journal #4</p>";
        assert_eq!(normalize(prose, &ids), prose);
        let event = r#"<tr><td>2026-11-01 16:30:00</td><td>sales_receipt.voided</td><td>owner</td><td class="long-text">void dated 2026-11-01 · journal #4 · reason: journal #4</td></tr>"#;
        let result = normalize(event, &ids);
        assert!(result.contains("{sales-event-time}"));
        assert!(result.contains("journal #{journal:reversal} · reason: journal #4"));
        assert!(
            normalize(
                &event.replace("journal #4 · reason", "journal #5 · reason"),
                &ids
            )
            .contains("journal #5 · reason")
        );
        assert!(
            normalize(
                &event.replace("2026-11-01 16:30:00", "2026-11-01 16:31:00"),
                &ids
            )
            .contains("2026-11-01 16:31:00")
        );
    }
}
