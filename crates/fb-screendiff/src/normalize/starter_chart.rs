use super::*;

fn choice() -> &'static str {
    concat!(
        "\n  \n  <p>\n    ",
        "<label for=\"id_starter_chart\">Chart of accounts:</label>\n    ",
        "<select name=\"starter_chart\" id=\"id_starter_chart\">\n  ",
        "<option value=\"standard\" selected>Standard small-business chart (recommended)</option>\n\n  ",
        "<option value=\"empty\">Start with an empty chart (for accountants)</option>\n\n</select>\n    \n    \n  </p>\n  "
    )
}

pub(super) fn normalize(html: &str, ids: &IdMap) -> String {
    if !ids.new_company || ids.reference {
        return html.into();
    }
    // It must be the final field of the company form, immediately before Save.
    let exact = format!("{}<button type=\"submit\">Save</button>", choice());
    html.replace(&exact, "<button type=\"submit\">Save</button>")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn starter_choice_normalizes_only_exact_default_on_new_company_form() {
        let ids = IdMap::new(&BTreeMap::new()).for_screen("/companies/new/", false);
        let html = format!(
            "<form>{}<button type=\"submit\">Save</button></form>",
            choice()
        );
        assert_eq!(
            normalize(&html, &ids),
            "<form><button type=\"submit\">Save</button></form>"
        );
        for changed in [
            html.replace("recommended", "changed"),
            html.replace("standard\" selected", "standard\""),
            html.replace("value=\"empty\"", "value=\"bogus\""),
            html.replace("<button", "<hr><button"),
        ] {
            assert_eq!(normalize(&changed, &ids), changed);
        }
        assert_eq!(normalize(&html, &ids.clone().reference()), html);
        assert_eq!(
            normalize(
                &html,
                &IdMap::new(&BTreeMap::new()).for_screen("/c/1/edit/", false)
            ),
            html
        );
    }
}
