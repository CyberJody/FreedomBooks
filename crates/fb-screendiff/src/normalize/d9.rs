//! D9 bank-feed identities, each only in the place the frozen templates print it (NORMALIZATION.md, D9):
//! the mapping select of a downloaded account (`map_<feed account id>` in name / id / label / error ids)
//! and its account options; a review row's category, transfer, matching-line and receipt selects; the
//! review filter's bank-account select (feed-account ids, not ledger accounts); and the rule buttons'
//! values on the rules page. Transaction pks need no rewrite: fb-core's book-wide `ordinal` is the
//! number Django prints.
use super::*;

static MAP_NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"((?:name|id|for|aria-describedby)="(?:id_)?map_)([0-9a-f-]{36}|[0-9]+)((?:_error)?")"#).unwrap());
static ROW_SELECT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?s)<select name="((?:map|account|to|line|receipt)_[^"]+)"([^>]*)>(.*?)</select>"#).unwrap());
static FILTER_SELECT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?s)<select name="account" id="id_account">(.*?)</select>"#).unwrap());
static RULE_BUTTON: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(<button type="submit" name="(?:move_up|move_down|deactivate|activate)" value=")([^"]+)(">)"#).unwrap());

fn sym<'a>(ids: &'a IdMap, model: &str, raw: &'a str) -> &'a str {
    ids.get(model, raw).unwrap_or(raw)
}

/// The desktop-only note on the ChatGPT card (Jody's rule), removed only as this exact element.
pub(super) const DESKTOP_NOTE: &str = r#"<p class="ai-desktop-note">For powering your personal finance.</p>"#;
static START_COMPANY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(<form method="post" action="/auth/openai/start"><input type="hidden" name="(?:csrfmiddlewaretoken|fb_command_token)" value="[A-Za-z0-9]+"><input type="hidden" name="company" value=")([^"]+)(">)"#).unwrap());

/// The AI assistant page: drop the desktop note after the plan card's heading, and map the start
/// form's hidden company (Django's integer pk vs the book's company id) through the company map.
fn assistant(html: &str, ids: &IdMap) -> String {
    if !html.contains(r#"<h2 id="ai-plan-title">Use your ChatGPT plan</h2>"#) {
        return html.to_owned();
    }
    let text = html.replacen(&format!(r#"<h2 id="ai-plan-title">Use your ChatGPT plan</h2>{DESKTOP_NOTE}"#), r#"<h2 id="ai-plan-title">Use your ChatGPT plan</h2>"#, 1);
    START_COMPANY.replace(&text, |c: &Captures| format!("{}{}{}", &c[1], sym(ids, "company", &c[2]), &c[3])).into_owned()
}

static PANEL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?s)<section class="attachments-panel" id="attachments".*?</section>"#).unwrap());
static PANEL_STAMP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"( · | removed by [^<·]* )([0-9]{4}-[0-9]{2}-[0-9]{2} [0-9]{2}:[0-9]{2})( UTC)"#).unwrap());

/// The attachments panel's upload / removal minute: the moment each runner stored the row (the
/// scenario's attachment rows are the provenance). Names, sizes, reasons and actors stay exact.
fn panel(html: &str, ids: &IdMap) -> String {
    PANEL
        .replace_all(html, |c: &Captures| {
            PANEL_STAMP.replace_all(&c[0], |s: &Captures| match ids.get("attachment_stamp", &s[2]) {
                Some(sym) => format!("{}{sym}{}", &s[1], &s[3]),
                None => s[0].to_owned(),
            }).into_owned()
        })
        .into_owned()
}

pub(super) fn normalize(html: &str, ids: &IdMap) -> String {
    let html = &panel(&assistant(html, ids), ids);
    if !html.contains(r#"<div class="bankfeed">"#) {
        return html.to_owned();
    }
    let text = MAP_NAME.replace_all(html, |c: &Captures| format!("{}{}{}", &c[1], sym(ids, "bfaccount", &c[2]), &c[3]));
    let text = ROW_SELECT.replace_all(&text, |c: &Captures| {
        let name = &c[1];
        let model = if name.starts_with("line_") {
            "bfline"
        } else if name.starts_with("receipt_") {
            "receipt"
        } else {
            "account"
        };
        format!(r#"<select name="{name}"{}>{}</select>"#, &c[2], options(model, &c[3], ids))
    });
    let text = if text.contains(r#"class="review-filters""#) {
        FILTER_SELECT.replace_all(&text, |c: &Captures| format!(r#"<select name="account" id="id_account">{}</select>"#, options("bfaccount", &c[1], ids))).into_owned()
    } else {
        text.into_owned()
    };
    RULE_BUTTON.replace_all(&text, |c: &Captures| format!("{}{}{}", &c[1], sym(ids, "bfrule", &c[2]), &c[3])).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn maps() -> (IdMap, IdMap) {
        let py: BTreeMap<String, String> = [("bfaccount:chk", "3"), ("bfrule:Office", "2"), ("account:D9 feed books:1000", "7")].iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        let rs: BTreeMap<String, String> = [("bfaccount:chk", "0195b4a4-55af-719e-96cb-a2f946b067c1"), ("bfrule:Office", "0195b4a4-55af-719e-96cb-a2f946b067c2"), ("account:D9 feed books:1000", "0195b4a4-55af-719e-96cb-a2f946b067c3")]
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect();
        (IdMap::new(&py), IdMap::new(&rs))
    }

    #[test]
    fn mapping_rows_and_rule_buttons_match_by_identity() {
        let (py, rs) = maps();
        let a = normalize(r#"<div class="bankfeed"><label for="id_map_3">x</label><select name="map_3" id="id_map_3"><option value="7" selected>1000 · Checking</option></select><button type="submit" name="move_up" value="2">Up</button></div>"#, &py);
        let b = normalize(r#"<div class="bankfeed"><label for="id_map_0195b4a4-55af-719e-96cb-a2f946b067c1">x</label><select name="map_0195b4a4-55af-719e-96cb-a2f946b067c1" id="id_map_0195b4a4-55af-719e-96cb-a2f946b067c1"><option value="0195b4a4-55af-719e-96cb-a2f946b067c3" selected>1000 · Checking</option></select><button type="submit" name="move_up" value="0195b4a4-55af-719e-96cb-a2f946b067c2">Up</button></div>"#, &rs);
        assert_eq!(a, b);
    }

    #[test]
    fn panel_stamps_map_only_known_attachment_minutes_inside_the_panel() {
        let ids = |t: &str| IdMap::new(&[(format!("attachment_stamp:{t}"), t.to_owned())].into_iter().collect());
        let panel = |t: &str, reason: &str| format!(r#"<section class="attachments-panel" id="attachments"><td>owner · {t} UTC</td><li>a.pdf · removed by owner {t} UTC · {reason}</li></section>"#);
        let a = normalize(&panel("2026-11-03 10:01", "Wrong file"), &ids("2026-11-03 10:01"));
        assert_eq!(a, normalize(&panel("2026-11-03 10:02", "Wrong file"), &ids("2026-11-03 10:02")));
        // A different reason, or a minute no attachment row carries, must DIFF.
        assert_ne!(a, normalize(&panel("2026-11-03 10:02", "Other"), &ids("2026-11-03 10:02")));
        assert_ne!(a, normalize(&panel("2026-11-03 10:02", "Wrong file"), &ids("2026-11-03 10:05")));
        // Outside the panel the same text is data.
        let outside = "<td>owner · 2026-11-03 10:01 UTC</td>";
        assert_eq!(normalize(outside, &ids("2026-11-03 10:01")), outside);
    }

    #[test]
    fn only_the_exact_desktop_note_after_the_plan_heading_is_dropped() {
        let (py, _) = maps();
        let h = r#"<h2 id="ai-plan-title">Use your ChatGPT plan</h2>"#;
        let django = format!("{h}<p>Complete eligible AI requests</p>");
        let desktop = format!("{h}{DESKTOP_NOTE}<p>Complete eligible AI requests</p>");
        assert_eq!(normalize(&django, &py), normalize(&desktop, &py));
        // A changed note, a note elsewhere, or other added text must DIFF.
        let changed = format!(r#"{h}<p class="ai-desktop-note">For powering your business.</p><p>Complete eligible AI requests</p>"#);
        assert_ne!(normalize(&django, &py), normalize(&changed, &py));
        let elsewhere = format!("{h}<p>Complete eligible AI requests</p>{DESKTOP_NOTE}");
        assert_ne!(normalize(&django, &py), normalize(&elsewhere, &py));
        // Without the plan card (another page) the note is never removed.
        assert_eq!(normalize(DESKTOP_NOTE, &py), DESKTOP_NOTE);
    }

    #[test]
    fn a_wrong_identity_or_a_page_outside_bank_feeds_still_differs() {
        let (py, rs) = maps();
        // The mapping select names the wrong downloaded account: must DIFF.
        let a = normalize(r#"<div class="bankfeed"><select name="map_3" id="id_map_3"></select></div>"#, &py);
        let b = normalize(r#"<div class="bankfeed"><select name="map_0195b4a4-55af-719e-96cb-a2f946b067c2" id="id_map_0195b4a4-55af-719e-96cb-a2f946b067c2"></select></div>"#, &rs);
        assert_ne!(a, b);
        // A rule button value is rewritten only as a rule, never as an account.
        let a = normalize(r#"<div class="bankfeed"><button type="submit" name="activate" value="7">On</button></div>"#, &py);
        let b = normalize(r#"<div class="bankfeed"><button type="submit" name="activate" value="0195b4a4-55af-719e-96cb-a2f946b067c3">On</button></div>"#, &rs);
        assert_ne!(a, b);
        // Text a person typed that looks like an id is data, and other screens are untouched.
        let typed = r#"<p>map_3</p><button type="submit" name="move_up" value="2">Up</button>"#;
        assert_eq!(normalize(typed, &py), typed);
        // The review filter's select holds feed accounts: a ledger account id there must DIFF.
        let a = normalize(r#"<div class="bankfeed"><form method="get" class="review-filters"><select name="account" id="id_account"><option value="3">chk</option></select></form></div>"#, &py);
        let b = normalize(r#"<div class="bankfeed"><form method="get" class="review-filters"><select name="account" id="id_account"><option value="0195b4a4-55af-719e-96cb-a2f946b067c3">chk</option></select></form></div>"#, &rs);
        assert_ne!(a, b);
    }
}
