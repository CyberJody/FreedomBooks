//! The Django template behaviour the ported screens rely on, reproduced for minijinja.
//!
//! - Output escaping is Django's `escape()` (`& < > " '`, with `'` as `&#x27;`), not minijinja's (which
//!   also escapes `/`). `None` prints as `None`, booleans as `True`/`False`, as Django does.
//! - Filters keep Django's names and meaning: `default` applies to any falsy value, `title` is Python's
//!   `str.title()`, `date` takes Django format letters, `pluralize`, `json_script`.
//! - `url(name, args...)` reverses the frozen app's own route table (`urls.json`, dumped from Django), so
//!   every link in a ported screen is the same path Django would print. `static(path)` and `icon(...)`
//!   match `{% static %}` and the `fb_ui` icon tags.
use minijinja::value::{Kwargs, Rest, Value, ValueKind};
use minijinja::{Error, ErrorKind, State};
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// `django.utils.html.escape`.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            c => out.push(c),
        }
    }
    out
}

/// How a value prints in a template (Django's `str()` of it).
pub fn display(value: &Value) -> String {
    match value.kind() {
        ValueKind::Undefined => String::new(),
        ValueKind::None => "None".to_owned(),
        ValueKind::Bool => if value.is_true() { "True" } else { "False" }.to_owned(),
        _ => value.to_string(),
    }
}

/// The environment formatter: safe strings pass through, everything else is escaped Django-style.
pub fn formatter(out: &mut minijinja::Output<'_>, _state: &State<'_, '_>, value: &Value) -> Result<(), Error> {
    let text = display(value);
    if value.is_safe() {
        out.write_str(&text)
    } else {
        out.write_str(&escape(&text))
    }
    .map_err(|_| Error::new(ErrorKind::WriteFailure, "write failed"))
}

/// Django truthiness: undefined, None, False, 0, "" and empty sequences/maps are false.
fn falsy(value: &Value) -> bool {
    !value.is_true()
}

/// `|default:x`: `x` when the value is falsy (Django), not only when undefined (Jinja).
pub fn default(value: Value, fallback: Value) -> Value {
    if falsy(&value) { fallback } else { value }
}

/// Python `str.title()`: a cased character after a cased character is lowered, otherwise uppered.
/// Django's `title` filter then lowers a letter after an apostrophe that follows a lowercase letter.
pub fn title(value: Value) -> String {
    let text = display(&value);
    let mut out = String::with_capacity(text.len());
    let mut prev_cased = false;
    for c in text.chars() {
        if prev_cased { out.extend(c.to_lowercase()) } else { out.extend(c.to_uppercase()) }
        prev_cased = c.is_lowercase() || c.is_uppercase();
    }
    // re.sub("([a-z])'([A-Z])", lambda m: m[0].lower(), ...)
    let chars: Vec<char> = out.chars().collect();
    let mut fixed = String::with_capacity(out.len());
    for (i, c) in chars.iter().enumerate() {
        if c.is_ascii_uppercase() && i >= 2 && chars[i - 1] == '\'' && chars[i - 2].is_ascii_lowercase() {
            fixed.push(c.to_ascii_lowercase());
        } else {
            fixed.push(*c);
        }
    }
    fixed
}

pub fn lower(value: Value) -> String {
    display(&value).to_lowercase()
}

/// Django's English DATETIME_FORMAT (`N j, Y, P`) for immutable UTC evidence.
pub fn default_datetime(stamp: &str) -> String {
    use chrono::{Datelike, Timelike};
    let Ok(t) = chrono::DateTime::parse_from_rfc3339(stamp) else { return String::new() };
    let months = ["Jan.", "Feb.", "March", "April", "May", "June", "July", "Aug.", "Sept.", "Oct.", "Nov.", "Dec."];
    let time = match (t.hour(), t.minute()) {
        (0, 0) => "midnight".into(),
        (12, 0) => "noon".into(),
        (h, m) => format!("{}{} {}", if h % 12 == 0 {12} else {h % 12}, if m == 0 {String::new()} else {format!(":{m:02}")}, if h < 12 {"a.m."} else {"p.m."}),
    };
    format!("{} {}, {}, {time}", months[t.month0() as usize], t.day(), t.year())
}

/// Frozen ledger.print_banners.toolbar_note. The print wave shares this D2 include.
pub fn print_toolbar_note(audience: &str, local_preview: bool) -> String {
    let hint="Printing uses your browser's print dialog (choose \"Save as PDF\" there).";
    if local_preview {format!("LOCAL TEST-ONLY PREVIEW · Synthetic data only · {hint}")}
    else if audience=="internal" {format!("{} · {hint}",crate::vm::ALPHA_BANNER)}
    else {String::new()}
}

/// `|pluralize`: "s" unless the count is exactly 1.
/// Django's `|filesizeformat`: "0 bytes", "1 byte", "1.5 KB" … "PB" (1024 steps, one decimal rounded half
/// to even like Python's `round`), with the space made non-breaking (`avoid_wrapping`).
pub fn filesizeformat(value: Value) -> String {
    let Ok(bytes) = i64::try_from(value) else { return "0\u{a0}bytes".into() };
    let size = bytes.unsigned_abs();
    let units = ["KB", "MB", "GB", "TB", "PB"];
    let text = if size < 1024 {
        format!("{size} byte{}", if size == 1 { "" } else { "s" })
    } else {
        let mut unit = 0;
        while unit + 1 < units.len() && size >= 1u64 << (10 * (unit + 2)) {
            unit += 1;
        }
        let scaled = size as f64 / (1u64 << (10 * (unit + 1))) as f64;
        format!("{:.1} {}", (scaled * 10.0).round_ties_even() / 10.0, units[unit])
    };
    format!("{}{text}", if bytes < 0 { "-" } else { "" }).replace(' ', "\u{a0}")
}

pub fn pluralize(value: Value) -> &'static str {
    match i64::try_from(value.clone()) {
        Ok(1) => "",
        Ok(_) => "s",
        Err(_) => match value.len() {
            Some(1) => "",
            _ => "s",
        },
    }
}

/// `|date:'Y-m-d'` on the ISO dates the view models carry. Only the format letters the ported screens
/// use are accepted; anything else is an error rather than a silently different rendering.
pub fn date(value: Value, format: Option<String>) -> Result<String, Error> {
    let text = match value.kind() {
        ValueKind::Undefined | ValueKind::None => return Ok(String::new()),
        _ => display(&value),
    };
    if text.is_empty() {
        return Ok(String::new());
    }
    let day = text.get(..10).filter(|d| d.len() == 10 && d.as_bytes()[4] == b'-' && d.as_bytes()[7] == b'-');
    let Some(day) = day else {
        return Err(Error::new(ErrorKind::InvalidOperation, format!("date filter needs an ISO date, got {text:?}")));
    };
    match format.as_deref().unwrap_or("Y-m-d") {
        "Y-m-d" => Ok(day.to_owned()),
        "Y-m-d H:i" => Ok(format!("{day} {}", text.get(11..16).unwrap_or("00:00"))),
        "Y-m-d H:i:s" | "Y-m-d H:i:s T" => {
            let time=text.get(11..19).unwrap_or("00:00:00");
            Ok(format!("{day} {time}{}",if format.as_deref()==Some("Y-m-d H:i:s T"){" UTC"}else{""}))
        },
        other => Err(Error::new(ErrorKind::InvalidOperation, format!("date format {other:?} is not ported yet"))),
    }
}

/// Python `json.dumps(value)` with the default separators and `ensure_ascii=True`.
pub fn python_json(value: &serde_json::Value) -> String {
    fn string(out: &mut String, s: &str) {
        out.push('"');
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                '\u{8}' => out.push_str("\\b"),
                '\u{c}' => out.push_str("\\f"),
                c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                    let mut buf = [0u16; 2];
                    for unit in c.encode_utf16(&mut buf) {
                        out.push_str(&format!("\\u{unit:04x}"));
                    }
                }
                c => out.push(c),
            }
        }
        out.push('"');
    }
    fn walk(out: &mut String, v: &serde_json::Value) {
        match v {
            serde_json::Value::Null => out.push_str("null"),
            serde_json::Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            serde_json::Value::Number(n) => out.push_str(&n.to_string()),
            serde_json::Value::String(s) => string(out, s),
            serde_json::Value::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    walk(out, item);
                }
                out.push(']');
            }
            serde_json::Value::Object(map) => {
                out.push('{');
                for (i, (k, item)) in map.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    string(out, k);
                    out.push_str(": ");
                    walk(out, item);
                }
                out.push('}');
            }
        }
    }
    let mut out = String::new();
    walk(&mut out, value);
    out
}

/// `|json_script:'id'` (Django 5.2): JSON with `<`, `>` and `&` escaped, in a non-executable script tag.
/// The renderer's CSP blocks inline *scripts*; `application/json` blocks are data and are allowed.
pub fn json_script(value: Value, id: String) -> Result<Value, Error> {
    let json: serde_json::Value =
        serde_json::to_value(&value).map_err(|e| Error::new(ErrorKind::InvalidOperation, e.to_string()))?;
    let text = python_json(&json).replace('<', "\\u003C").replace('>', "\\u003E").replace('&', "\\u0026");
    Ok(Value::from_safe_string(format!(r#"<script id="{}" type="application/json">{}</script>"#, escape(&id), text)))
}

fn routes() -> &'static BTreeMap<String, String> {
    static ROUTES: OnceLock<BTreeMap<String, String>> = OnceLock::new();
    ROUTES.get_or_init(|| {
        serde_json::from_str(include_str!("../urls.json")).expect("urls.json is valid")
    })
}

/// `reverse(name, args=[...])` over the frozen app's route table. Arguments fill `<conv:name>` parts in order.
pub fn reverse(name: &str, args: &[String]) -> Result<String, String> {
    let route = routes().get(name).ok_or_else(|| format!("no route named {name:?}"))?;
    let mut out = String::new();
    let mut rest = route.as_str();
    let mut used = 0;
    while let Some(start) = rest.find('<') {
        let end = rest[start..].find('>').ok_or("malformed route")? + start;
        out.push_str(&rest[..start]);
        let arg = args.get(used).ok_or_else(|| format!("route {name:?} needs more arguments"))?;
        if arg.is_empty() || arg.contains(['/', '?', '#']) {
            return Err(format!("route {name:?}: invalid argument {arg:?}"));
        }
        out.push_str(arg);
        used += 1;
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    if used != args.len() {
        return Err(format!("route {name:?} takes {used} arguments, got {}", args.len()));
    }
    Ok(out)
}

/// Every route name and pattern, for the app's router (it matches incoming paths against the same table).
pub fn route_table() -> &'static BTreeMap<String, String> {
    routes()
}

pub fn url(name: String, args: Rest<Value>) -> Result<String, Error> {
    let args: Vec<String> = args.iter().map(display).collect();
    reverse(&name, &args).map_err(|e| Error::new(ErrorKind::InvalidOperation, e))
}

pub fn static_url(path: String) -> String {
    format!("/static/{path}")
}

/// `{% icon name size css small %}` from `fb_ui`.
pub fn icon(name: String, size: Option<Value>, css: Option<String>, small: Option<bool>, kwargs: Kwargs) -> Result<Value, Error> {
    kwargs.assert_all_used()?;
    let folder = if small.unwrap_or(false) { "ui/icons/sm" } else { "ui/icons" };
    let size = size.map(|v| display(&v)).unwrap_or_else(|| "24".to_owned());
    let css = css.unwrap_or_else(|| "icon".to_owned());
    Ok(Value::from_safe_string(format!(
        r#"<img class="{}" src="{}" alt="" aria-hidden="true" width="{}" height="{}">"#,
        escape(&css),
        escape(&static_url(format!("{folder}/{name}.svg"))),
        escape(&size),
        escape(&size)
    )))
}

pub fn icon_url(name: String) -> String {
    static_url(format!("ui/icons/{name}.svg"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filesizeformat_is_djangos() {
        assert_eq!(filesizeformat(Value::from(0)), "0\u{a0}bytes");
        assert_eq!(filesizeformat(Value::from(1)), "1\u{a0}byte");
        assert_eq!(filesizeformat(Value::from(1023)), "1023\u{a0}bytes");
        assert_eq!(filesizeformat(Value::from(1024)), "1.0\u{a0}KB");
        assert_eq!(filesizeformat(Value::from(256 * 1024 * 1024)), "256.0\u{a0}MB");
        assert_eq!(filesizeformat(Value::from(1536)), "1.5\u{a0}KB");
    }

    #[test]
    fn escape_matches_django() {
        assert_eq!(escape(r#"<a href="x">'&/'</a>"#), "&lt;a href=&quot;x&quot;&gt;&#x27;&amp;/&#x27;&lt;/a&gt;");
    }

    #[test]
    fn title_matches_python() {
        assert_eq!(title(Value::from("PARTIAL")), "Partial");
        assert_eq!(title(Value::from("DEBIT")), "Debit");
        assert_eq!(title(Value::from("they're bill's")), "They're Bill's");
        assert_eq!(title(Value::from("o'neil 2nd")), "O'Neil 2Nd");
    }

    #[test]
    fn python_json_matches_json_dumps() {
        let v = serde_json::json!({"1": {"description": "Café <b>", "rate": "45.00", "n": 8, "ok": false}});
        assert_eq!(python_json(&v), "{\"1\": {\"description\": \"Caf\\u00e9 <b>\", \"rate\": \"45.00\", \"n\": 8, \"ok\": false}}");
        assert_eq!(python_json(&serde_json::json!("\u{1F600}")), "\"\\ud83d\\ude00\"");
    }

    #[test]
    fn reverse_fills_arguments_in_order() {
        assert_eq!(reverse("register", &["c1".into(), "a1".into()]).unwrap(), "/c/c1/accounts/a1/register/");
        assert_eq!(reverse("home", &[]).unwrap(), "/");
        assert!(reverse("register", &["c1".into()]).is_err());
        assert!(reverse("register", &["c1".into(), "a/../b".into()]).is_err());
        assert!(reverse("nope", &[]).is_err());
    }

    #[test]
    fn date_filter_refuses_unported_formats() {
        assert_eq!(date(Value::from("2026-10-01"), Some("Y-m-d".into())).unwrap(), "2026-10-01");
        assert_eq!(date(Value::from(()), Some("Y-m-d".into())).unwrap(), "");
        assert!(date(Value::from("2026-10-01"), Some("N j, Y".into())).is_err());
        assert!(date(Value::from("é"), None).is_err());
    }
}
