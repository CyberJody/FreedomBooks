//! Report identities only in the typed GL header anchor and declared CSV columns.
use super::*;

pub(super) fn normalize(html: &str, ids: &IdMap) -> String {
    static ANCHOR: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(<tbody class="gl-account" id="gl-account-)([^"]+)(")"#).unwrap()
    });
    ANCHOR
        .replace_all(html, |c: &Captures| {
            format!(
                "{}{}{}",
                &c[1],
                ids.get("account", &c[2]).unwrap_or(&c[2]),
                &c[3]
            )
        })
        .into_owned()
}

// RFC4180 field spans keep every byte (including original quoting and CRLF) except a declared id/link.
pub(super) fn records(text: &str) -> Option<Vec<Vec<(usize, usize, String)>>> {
    let bytes = text.as_bytes();
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut start = 0;
    let mut i = 0;
    let mut quoted = false;
    while i < bytes.len() {
        match bytes[i] {
            b'"' if quoted && bytes.get(i + 1) == Some(&b'"') => {
                i += 2;
                continue;
            }
            b'"' => quoted = !quoted,
            b',' | b'\r' | b'\n' if !quoted => {
                let raw = &text[start..i];
                let plain = if raw.starts_with('"') && raw.ends_with('"') && raw.len() >= 2 {
                    raw[1..raw.len() - 1].replace("\"\"", "\"")
                } else {
                    raw.into()
                };
                row.push((start, i, plain));
                if bytes[i] != b',' {
                    rows.push(std::mem::take(&mut row));
                    if bytes[i] == b'\r' && bytes.get(i + 1) == Some(&b'\n') {
                        i += 1;
                    }
                }
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    if quoted || start != bytes.len() {
        return None;
    }
    Some(rows)
}
pub(super) fn csv(text: &str, ids: &IdMap) -> Option<String> {
    let rows = records(text)?;
    let financial = [
        "Row type",
        "Section",
        "Group",
        "Account code",
        "Account",
        "Account ID",
        "Amount",
    ];
    let ledger = [
        "Row type",
        "Account code",
        "Account",
        "Account ID",
        "Date",
        "Type",
        "Number",
        "Party",
        "Memo",
        "Debit",
        "Credit",
        "Balance",
        "Link",
    ];
    let mut header = None;
    let mut replacements = Vec::new();
    for row in rows {
        let values = row.iter().map(|(_, _, v)| v.as_str()).collect::<Vec<_>>();
        if values == financial {
            header = Some((7, 5));
            continue;
        }
        if values == ledger {
            header = Some((13, 3));
            continue;
        }
        let Some((count, column)) = header else {
            continue;
        };
        if row.len() != count {
            return None;
        }
        if let Some(sym) = ids.get("account", &row[column].2) {
            replacements.push((row[column].0, row[column].1, sym.to_owned()));
        }
        if count == 13 && row[0].2 == "line" {
            let raw = &row[12].2;
            let relative = raw
                .strip_prefix("http://testserver")
                .or_else(|| raw.strip_prefix("fb://app"));
            if let Some(relative) = relative.filter(|r| route_pattern(r).is_some()) {
                replacements.push((row[12].0, row[12].1, url(relative, ids)));
            }
        }
    }
    header?;
    let mut out = text.to_owned();
    for (start, end, value) in replacements.into_iter().rev() {
        let quoted = text[start..end].starts_with('"');
        out.replace_range(
            start..end,
            &if quoted {
                format!("\"{}\"", value.replace('"', "\"\""))
            } else {
                value
            },
        );
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    const A: &str = "0195b4a4-55af-719e-96cb-a2f946b067c0";
    fn maps() -> (IdMap, IdMap) {
        (
            IdMap::new(&BTreeMap::from([("account:090".into(), "7".into())])),
            IdMap::new(&BTreeMap::from([("account:090".into(), A.into())])),
        )
    }
    #[test]
    fn report_anchor_is_typed_and_never_changes_prose_or_the_wrong_model() {
        let (py, rs) = maps();
        let wrap = |id: &str| format!("<tbody class=\"gl-account\" id=\"gl-account-{id}\">");
        assert_eq!(normalize(&wrap("7"), &py), normalize(&wrap(A), &rs));
        assert_ne!(normalize(&wrap("8"), &py), normalize(&wrap(A), &rs));
        for raw in [
            format!("<p>gl-account-{A}</p>"),
            format!("<tbody class=\"other\" id=\"gl-account-{A}\">"),
        ] {
            assert_eq!(normalize(&raw, &rs), raw);
        }
    }
    #[test]
    fn account_query_maps_only_the_gl_route_or_a_query_only_pager_link() {
        let (py, rs) = maps();
        for prefix in ["/c/1/home/", "/c/1/reports/balance-sheet/", "/unknown/"] {
            assert_ne!(
                super::super::url(&format!("{prefix}?account=7"), &py),
                super::super::url(&format!("{prefix}?account={A}"), &rs)
            );
        }
        assert_eq!(
            super::super::url("?account=7&page=2", &py),
            super::super::url(&format!("?account={A}&page=2"), &rs)
        );
    }
    #[test]
    fn csv_maps_only_the_declared_account_column_and_preserves_literal_text_and_bytes() {
        let (py, rs) = maps();
        let wrap = |id: &str, name: &str| {
            format!(
                "Report,Profit and loss\r\n\r\nRow type,Section,Group,Account code,Account,Account ID,Amount\r\naccount,Income,Income,090,{name},{id},1234.56\r\n"
            )
        };
        assert_eq!(
            csv(&wrap("7", "Typed #7"), &py),
            csv(&wrap(A, "Typed #7"), &rs)
        );
        assert_ne!(
            csv(&wrap("7", "Typed #7"), &py),
            csv(&wrap(A, &format!("Typed #{A}")), &rs)
        );
        assert_ne!(csv(&wrap("7", "Name"), &py), csv(&wrap(A, "\"Name\""), &rs));
        assert_ne!(
            csv(&wrap("7", "Name"), &py),
            csv(&wrap(A, "Name").replace("1234.56", "1234.57"), &rs)
        );
        assert_eq!(csv("Account ID,Other\r\n7,Name\r\n", &py), None);
    }
}
