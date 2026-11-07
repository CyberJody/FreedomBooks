//! Frozen payables display filters. No I/O or business mutations.
use minijinja::{Environment, Value};
pub fn register(env: &mut Environment<'_>) {
    env.add_filter("party_label", |map: Value, id: Value| {
        map.get_item(&Value::from(id.to_string()))
            .unwrap_or(Value::UNDEFINED)
    });
    env.add_filter("account_from_mapping", |map: Value, id: Value| {
        if let Ok(rows) = map.try_iter() {
            for row in rows {
                if row
                    .get_attr("id")
                    .is_ok_and(|v| v.to_string() == id.to_string())
                {
                    return format!(
                        "{} · {}",
                        row.get_attr("code").unwrap(),
                        row.get_attr("name").unwrap()
                    );
                }
            }
        }
        "(unknown account)".into()
    });
    env.add_filter("posting_role", |role: String, kind: Option<String>| {
        let (prefix, rest) = role.trim().split_once(':').unwrap_or((role.trim(), ""));
        let label = match prefix {
            "receivables" => "Accounts receivable",
            "income" => "Income account",
            "destination" => "Deposit to",
            "collected_tax" => "Sales tax collected",
            "undeposited" => "Undeposited funds",
            "customer_deposits" => "Customer deposits",
            "debit" => "Account charged",
            "payables" => "Accounts payable",
            "bank" => "Bank account",
            "from" => "Transfer from",
            "to" => "Transfer to",
            "expense" => "Expense account",
            _ => "Account",
        };
        if ((kind.as_deref() == Some("BILL") && prefix == "expense")
            || (kind.as_deref() == Some("INV") && prefix == "income")
            || (kind.as_deref() == Some("CHK") && prefix == "debit"))
            && !rest.is_empty()
            && rest.chars().all(|c| c.is_ascii_digit())
        {
            format!("{label} (line {rest})")
        } else {
            label.into()
        }
    });
    env.add_filter("striptags", |v: String| {
        let mut tag = false;
        v.chars()
            .filter(|c| {
                if *c == '<' {
                    tag = true;
                    false
                } else if *c == '>' {
                    tag = false;
                    false
                } else {
                    !tag
                }
            })
            .collect::<String>()
    });
    env.add_filter("stringformat", |v: Value, _fmt: String| v.to_string());
    env.add_filter("filesize", |n: u64| {
        if n < 1024 {
            format!("{n} B")
        } else if n < 1024 * 1024 {
            format!("{:.1} KB", n as f64 / 1024.)
        } else {
            format!("{:.1} MB", n as f64 / 1048576.)
        }
    });
    env.add_filter("pluralize", |v: Value, arg: Option<String>| {
        let one = crate::django::pluralize(v).is_empty();
        if let Some(arg) = arg {
            if let Some((a, b)) = arg.split_once(',') {
                if one { a.into() } else { b.into() }
            } else if one {
                String::new()
            } else {
                arg
            }
        } else if one {
            String::new()
        } else {
            "s".into()
        }
    });
    env.add_filter("date", |v: Value, format: Option<String>| {
        if format.as_deref() == Some("l j F") {
            let raw = v.to_string();
            let d = chrono::NaiveDate::parse_from_str(&raw, "%Y-%m-%d").map_err(|e| {
                minijinja::Error::new(minijinja::ErrorKind::InvalidOperation, e.to_string())
            })?;
            use chrono::Datelike;
            Ok(format!("{} {} {}", d.format("%A"), d.day(), d.format("%B")))
        } else {
            crate::d3::date(v, format)
        }
    });
}
