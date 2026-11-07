//! Reconciliation form identities are scoped by element and model, never by text replacement.
use super::*;
pub(super) fn csv(text: &str, ids: &IdMap) -> Option<String> {
    let rows = super::d8::records(text)?;
    let mut header = None;
    let mut replacements = vec![];
    for row in rows {
        let values = row.iter().map(|(_, _, v)| v.as_str()).collect::<Vec<_>>();
        if values.len() == 11
            && values[..6] == ["Date", "Number", "Type", "Payee / name", "Account", "Memo"]
            && values[7] == "Cleared"
            && values[9..] == ["Balance", "Link"]
        {
            header = Some((11, Some(10)));
            continue;
        }
        if values
            == [
                "Section",
                "Type",
                "Date",
                "Number",
                "Payee / name",
                "Memo",
                "Amount",
                "Link",
            ]
        {
            header = Some((8, Some(7)));
            continue;
        }
        if values == ["Section", "Items", "Amount"] {
            header = Some((3, None));
            continue;
        }
        if header.is_none() && row.len() == 2 {
            let raw = &row[1].2;
            if row[0].2 == "Reconciled at" {
                if ids.get("reconciliation_csv_stamp", raw).is_some() {
                    replacements.push((row[1].0, row[1].1, "{reconciliation-csv-stamp}".into()));
                }
            } else if row[0].2 == "Undone"
                && let Some((stamp, tail)) = raw.split_once(" by ")
                && ids.get("reconciliation_csv_stamp", stamp).is_some()
            {
                replacements.push((
                    row[1].0,
                    row[1].1,
                    format!("{{reconciliation-csv-stamp}} by {tail}"),
                ));
            }
        }
        if let Some((count, link)) = header {
            if row.len() != count {
                return None;
            }
            if let Some(col) = link {
                let raw = &row[col].2;
                if let Some(path) = raw
                    .strip_prefix("http://testserver")
                    .or_else(|| raw.strip_prefix("fb://app"))
                    && route_pattern(path).is_some()
                {
                    replacements.push((row[col].0, row[col].1, url(path, ids)));
                }
            }
        }
    }
    header?;
    let mut out = text.to_owned();
    for (start, end, value) in replacements.into_iter().rev() {
        let value = if text[start..end].starts_with('"') {
            format!("\"{}\"", value.replace('"', "\"\""))
        } else {
            value
        };
        out.replace_range(start..end, &value);
    }
    Some(out)
}
pub(super) fn normalize(html: &str, ids: &IdMap) -> String {
    // Django's CSV GET link has no CSRF input. The desktop uses a single-use SubmitForm
    // capability in that exact link; discard only that generated bridge metadata.
    let mut html = Regex::new(r#"(<a href="[^"]*(?:\?|&amp;)output=csv">CSV)<input type="hidden" name="fb_command_token" value="[A-Za-z0-9]+">"#)
        .unwrap().replace_all(html, "$1").into_owned();
    let input=Regex::new(r#"(<input\b[^>]*\bname=")(line|tick|reconciliation|shown|completion_key|undo_key)("[^>]*\bvalue=")([^"]*)("[^>]*>)"#).unwrap();
    html = input
        .replace_all(&html, |c: &Captures| {
            let raw = &c[4];
            let value = match &c[2] {
                "line" | "tick" => ids.get("line", raw).unwrap_or(raw).to_owned(),
                "reconciliation" => ids.get("reconciliation", raw).unwrap_or(raw).to_owned(),
                "completion_key" if fb_core::py::py_uuid(raw).is_some() => {
                    format!("{{{}}}", &c[2])
                }
                "undo_key" if ids.generated_undo_key && fb_core::py::py_uuid(raw).is_some() => {
                    format!("{{{}}}", &c[2])
                }
                "shown" => raw
                    .split(',')
                    .map(|p| match p.split_once(':') {
                        Some((id, rev)) => format!("{}:{rev}", ids.get("line", id).unwrap_or(id)),
                        None => p.into(),
                    })
                    .collect::<Vec<_>>()
                    .join(","),
                _ => raw.to_owned(),
            };
            format!("{}{}{}{}{}", &c[1], &c[2], &c[3], value, &c[5])
        })
        .into_owned();
    html = Regex::new(r#"(<tr id="line-)([^"]+)(")"#)
        .unwrap()
        .replace_all(&html, |c: &Captures| {
            format!(
                "{}{}{}",
                &c[1],
                ids.get("line", &c[2]).unwrap_or(&c[2]),
                &c[3]
            )
        })
        .into_owned();
    // Begin uses id-before-name selects; the shared select normalizer accepts name-first widgets.
    html=Regex::new(r#"(<select id="id_(?:service_account|interest_account)" name=")(service_account|interest_account)(">)(.*?)(</select>)"#).unwrap().replace_all(&html,|c:&Captures|format!("{}{}{}{}{}",&c[1],&c[2],&c[3],options("account",&c[4],ids),&c[5])).into_owned();
    let stamp = Regex::new(r#"([0-9]{4}-[0-9]{2}-[0-9]{2} [0-9]{2}:[0-9]{2})"#).unwrap();
    let tables =
        Regex::new(r#"(?s)(<table class="registers-list reconcile-history">)(.*?)(</table>)"#)
            .unwrap();
    html = tables
        .replace_all(&html, |c: &Captures| {
            format!(
                "{}{}{}",
                &c[1],
                stamp.replace_all(&c[2], |c: &Captures| ids
                    .get("reconciliation_stamp", &c[1])
                    .map(|_| "{reconciliation-stamp}")
                    .unwrap_or(&c[1])
                    .to_owned()),
                &c[3]
            )
        })
        .into_owned();
    let rec=Regex::new(r#"(<p>Reconciled |<p class="notice warning" role="note">Undone )([0-9]{4}-[0-9]{2}-[0-9]{2} [0-9]{2}:[0-9]{2})( by )"#).unwrap();
    rec.replace_all(&html, |c: &Captures| {
        format!(
            "{}{}{}",
            &c[1],
            ids.get("reconciliation_stamp", &c[2])
                .map(|_| "{reconciliation-stamp}")
                .unwrap_or(&c[2]),
            &c[3]
        )
    })
    .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn csv_bridge_capability_does_not_hide_the_link_or_label() {
        let ids=IdMap::new(&BTreeMap::new());
        let django=r#"<a href="?view=summary&amp;output=csv">CSV</a>"#;
        let desktop=r#"<a href="?view=summary&amp;output=csv">CSV<input type="hidden" name="fb_command_token" value="abcdef0123"></a>"#;
        assert_eq!(normalize(django,&ids),normalize(desktop,&ids));
        assert_ne!(normalize(django,&ids),normalize(&desktop.replace("CSV<input","Changed<input"),&ids));
        assert_ne!(normalize(django,&ids),normalize(&desktop.replace("summary","detail"),&ids));
    }
    #[test]
    fn echoed_undo_key_on_d7b_undo_invalid_must_diff() {
        let screens: Value=serde_json::from_str(include_str!("../../screens.json")).unwrap();
        let key=screens["d7b-undo-invalid"]["post"]["undo_key"].as_str().unwrap();
        let html=format!("<form><ul class=\"errorlist\"><li>Give a reason for undoing this reconciliation.</li></ul><input type=\"hidden\" name=\"undo_key\" value=\"{key}\"></form>");
        let ids=IdMap::new(&BTreeMap::new());
        let mutant=html.replace(key,"00000000-0000-0000-0000-000000df3bad");
        assert_ne!(normalize(&html,&ids),normalize(&mutant,&ids),"submitted error echoes must stay exact");
        let posted=ids.for_screen("/c/1/reconcile/2/undo/",true);
        assert_ne!(normalize(&html,&posted),normalize(&mutant,&posted));
        let generated=ids.for_screen("/c/1/reconcile/2/undo/",false);
        assert_eq!(normalize(&html,&generated),normalize(&mutant,&generated));
    }
    fn maps() -> (IdMap, IdMap) {
        (
            IdMap::new(&BTreeMap::from([
                ("line:J000001:1".into(), "7".into()),
                ("account:1000".into(), "8".into()),
                ("reconciliation:1000:2026-10-31".into(), "9".into()),
            ])),
            IdMap::new(&BTreeMap::from([
                ("line:J000001:1".into(), "line-uuid".into()),
                (
                    "account:1000".into(),
                    "0195b4a4-55af-719e-96cb-a2f946b067c0".into(),
                ),
                ("reconciliation:1000:2026-10-31".into(), "rec-uuid".into()),
            ])),
        )
    }
    #[test]
    fn ticks_and_shown_use_only_the_line_model() {
        let (py, rs) = maps();
        let html = r#"<tr id="line-7"><input type="checkbox" name="tick" value="7"><input type="hidden" name="shown" value="7:2"><input type="hidden" name="reconciliation" value="9">"#;
        let other = html
            .replace("line-7", "line-line-uuid")
            .replace("value=\"7\"", "value=\"line-uuid\"")
            .replace("value=\"7:2\"", "value=\"line-uuid:2\"")
            .replace("value=\"9\"", "value=\"rec-uuid\"");
        assert_eq!(normalize(html, &py), normalize(&other, &rs));
        assert_ne!(
            normalize(html, &py),
            normalize(&other.replace("line-uuid:2", "line-uuid:3"), &rs)
        );
        let wrong = r#"<input name="tick" value="8"><p>line-7</p><input name="memo" value="7">"#;
        assert_eq!(normalize(wrong, &py), wrong);
    }
    #[test]
    fn generated_keys_and_timestamps_do_not_normalize_user_text() {
        let ids = IdMap::new(&BTreeMap::from([(
            "reconciliation_stamp:2026-11-02 06:00".into(),
            "2026-11-02 06:00".into(),
        )]));
        let prose = "<p>2026-11-02 06:00</p><input name=\"memo\" value=\"00000000-0000-0000-0000-000000000001\">";
        assert_eq!(normalize(prose, &ids), prose);
        let generated = "<p>Reconciled 2026-11-02 06:00 by owner</p>";
        assert!(normalize(generated, &ids).contains("{reconciliation-stamp}"));
        assert_ne!(
            normalize(generated, &ids),
            normalize(&generated.replace("06:00", "06:01"), &ids)
        );
    }
    #[test]
    fn banking_csv_preserves_every_cell_except_the_declared_route() {
        let (py, rs) = maps();
        let a = "Date,Number,Type,Payee / name,Account,Memo,Payment,Cleared,Deposit,Balance,Link\r\n2026-10-01,J1,General journal,7,8,7,,C,100.00,100.00,http://testserver/c/1/accounts/8/register/\r\n";
        // No company mapping: the unchanged company route segment remains literal on both sides.
        let b = a.replace("http://testserver", "fb://app").replace(
            "/accounts/8/",
            "/accounts/0195b4a4-55af-719e-96cb-a2f946b067c0/",
        );
        assert_eq!(csv(a, &py), csv(&b, &rs));
        assert_ne!(csv(a, &py), csv(&b.replace("100.00", "101.00"), &rs));
        assert_ne!(csv(a, &py), csv(&b.replace(",7,,C", ",line-uuid,,C"), &rs));
        assert!(
            csv(
                "Memo,Link\r\n7,http://testserver/c/1/accounts/8/register/\r\n",
                &py
            )
            .is_none()
        );
    }
}
