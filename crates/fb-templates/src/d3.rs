//! The extra frozen Django filters used by D3.
use crate::django::{self, escape};
use minijinja::{Environment, Error, Value};

fn text(v: &Value) -> String {
    if v.is_none() || v.is_undefined() {
        String::new()
    } else {
        v.to_string()
    }
}
pub fn date(v: Value, format: Option<String>) -> Result<String, Error> {
    let fmt = format.as_deref().unwrap_or("Y-m-d");
    if !["l j F Y", "j F Y"].contains(&fmt) {
        return django::date(v, format);
    }
    let s = text(&v);
    if s.is_empty() {
        return Ok(s);
    }
    use chrono::Datelike;
    let d = chrono::NaiveDate::parse_from_str(&s, "%Y-%m-%d").map_err(|e| {
        minijinja::Error::new(minijinja::ErrorKind::InvalidOperation, e.to_string())
    })?;
    Ok(if fmt == "l j F Y" {
        format!(
            "{} {} {} {}",
            d.format("%A"),
            d.day(),
            d.format("%B"),
            d.year()
        )
    } else {
        format!("{} {} {}", d.day(), d.format("%B"), d.year())
    })
}
pub fn register(env: &mut Environment<'_>) {
    env.add_filter("capfirst", |v: Value| {
        let s = text(&v);
        let mut ch = s.chars();
        ch.next()
            .map(|c| c.to_uppercase().to_string() + ch.as_str())
            .unwrap_or_default()
    });
    env.add_filter("default_if_none", |v: Value, default: Value| {
        if v.is_none() || v.is_undefined() {
            default
        } else {
            v
        }
    });
    env.add_filter("truncatechars", |v: Value, n: usize| {
        crate::d3_text::truncate(&text(&v), n)
    });
    env.add_filter("linebreaksbr", |v: Value| {
        Value::from_safe_string(
            escape(&text(&v).replace("\r\n", "\n").replace('\r', "\n")).replace('\n', "<br>"),
        )
    });
    env.add_filter("pct", |v: Value| {
        let s = text(&v);
        if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.').to_owned()
        } else {
            s
        }
    });
    env.add_filter("money", |v: Value| {
        let s = text(&v);
        let (sign, s) = s
            .strip_prefix('-')
            .map(|s| ("-", s))
            .unwrap_or(("", s.as_str()));
        let (whole, frac) = s.split_once('.').unwrap_or((s, "00"));
        let mut groups = String::new();
        for (i, c) in whole.chars().enumerate() {
            if i > 0 && (whole.len() - i).is_multiple_of(3) {
                groups.push(',');
            }
            groups.push(c);
        }
        format!("{sign}{groups}.{frac}")
    });
    env.add_filter("add", |v: i64, n: String| v + n.parse::<i64>().unwrap_or(0));
}
