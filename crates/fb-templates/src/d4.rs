//! Frozen sales presentation filters; they never read or write a book.
use minijinja::{Environment, Value};
pub fn datetime_display(text: String) -> String {
    use chrono::{Datelike, Timelike};
    let Ok(t) = chrono::DateTime::parse_from_rfc3339(&text) else {
        return text;
    };
    let months = [
        "Jan.", "Feb.", "March", "April", "May", "June", "July", "Aug.", "Sept.", "Oct.", "Nov.",
        "Dec.",
    ];
    let time = if t.hour() == 0 && t.minute() == 0 {
        "midnight".into()
    } else if t.hour() == 12 && t.minute() == 0 {
        "noon".into()
    } else {
        let hour = if t.hour() % 12 == 0 {
            12
        } else {
            t.hour() % 12
        };
        format!(
            "{hour}{} {}",
            if t.minute() == 0 {
                String::new()
            } else {
                format!(":{:02}", t.minute())
            },
            if t.hour() < 12 { "a.m." } else { "p.m." }
        )
    };
    format!(
        "{} {}, {}, {}",
        months[t.month0() as usize],
        t.day(),
        t.year(),
        time
    )
}
pub fn register(env: &mut Environment<'_>) {
    env.add_filter("datetime_display", datetime_display);
    env.add_filter("filesize", |n: u64| format!("{n} B"));
    env.add_filter("yesno", |v: Value, labels: String| {
        labels
            .split(',')
            .nth(usize::from(!v.is_true()))
            .unwrap_or("")
            .to_owned()
    });
    env.add_filter("stringformat", |v: Value, _format: String| v.to_string());
    env.add_filter("has_balance", |v: Value, name: String| {
        v.get_attr(&name).is_ok_and(|v| !v.is_undefined())
    });
    env.add_filter("account_from_mapping", |mapping: Value, id: String| {
        mapping
            .try_iter()
            .ok()
            .and_then(|mut rows| {
                rows.find(|r| r.get_attr("id").is_ok_and(|v| v.as_str() == Some(&id)))
            })
            .map(|r| {
                format!(
                    "{} · {}",
                    r.get_attr("code").unwrap(),
                    r.get_attr("name").unwrap()
                )
            })
            .unwrap_or_else(|| "(unknown account)".into())
    });
    env.add_filter("posting_role", |role: String, kind: Option<String>| {
        let (prefix, suffix) = role.trim().split_once(':').unwrap_or((role.trim(), ""));
        let label = match prefix {
            "receivables" => "Accounts receivable",
            "income" => "Income account",
            "destination" => "Deposit to",
            "collected_tax" => "Sales tax collected",
            "undeposited" => "Undeposited funds",
            "bank" => "Bank",
            _ => prefix,
        };
        if kind.as_deref() == Some("INV")
            && prefix == "income"
            && !suffix.is_empty()
            && suffix.bytes().all(|b| b.is_ascii_digit())
        {
            format!("{label} (line {suffix})")
        } else {
            label.into()
        }
    });
}
