"""Keep provider ingress cleaning separate from masking historical/manual text."""
from pathlib import Path
root=Path(__file__).resolve().parents[1]
def replace(name,old,new):
    p=root/name;t=p.read_text();assert t.count(old)==1,(name,old);p.write_text(t.replace(old,new))
p='crates/fb-core/src/bankfeeds/staging.rs'
replace(p,'''pub(crate) fn provider_text(secrets: &SecretSet, value: &str) -> String {
    // Removing controls can join a credential spelling: clean BEFORE known-secret masking.
    let cleaned: String = value.chars().filter(|&c| fb_db::check_numbers::free_text_char_allowed(c)).collect();
    let masked = secrets.mask(&cleaned);''','''pub(crate) fn provider_ingress(secrets: &SecretSet, value: &str) -> String {
    // Removing controls can join a credential spelling: clean BEFORE known-secret masking.
    let cleaned: String = value.chars().filter(|&c| fb_db::check_numbers::free_text_char_allowed(c)).collect();
    provider_text(secrets, &cleaned)
}
/// Masking is also used on carried human text by the central writer: do not clean it here.
pub(crate) fn provider_text(secrets: &SecretSet, value: &str) -> String {
    let masked = secrets.mask(value);''')
replace(p,'''StagedTransaction { remote_id: provider_text(secrets, &t.remote_id), description: provider_text(secrets, &t.description), ..t.clone() }''','''StagedTransaction { remote_id: provider_ingress(secrets, &t.remote_id), description: provider_ingress(secrets, &t.description), ..t.clone() }''')
replace(p,'''    let one = |s: &str| provider_text(secrets, s);''','''    let one = |s: &str| provider_ingress(secrets, s);''')
replace(p,'''        let summary = provider_text(secrets, summary);''','''        let summary = provider_ingress(secrets, summary);''')
replace(p,'''(provider_text(secrets, a), provider_transaction(secrets, t))''','''(provider_ingress(secrets, a), provider_transaction(secrets, t))''')
replace('crates/fb-core/src/bankfeeds/tests/dfix3.rs','''staging::provider_text(&secrets,"syn''','''staging::provider_ingress(&secrets,"syn''')
replace('crates/fb-core/src/bankfeeds/rules.rs','''        let name = text(&get("name"), 120, true)?;
        let match_text = py::py_strip(&text(&get("match_text"), 200, true)?).to_owned();
        let payee_name = text(values.get("payee_name").unwrap_or(&json!("")), 120, false)?;''','''        let previous = values["rule_id"].as_str().map(|key| {
            tx.query_row(&format!("SELECT {RULE_COLS} FROM bankfeeds_rule WHERE company_id=?1 AND id=?2"),params![co.to_db(),key],rule_row)
                .optional()?.ok_or_else(||err("Choose a rule of this company."))
        }).transpose()?;
        let name = py::input_text(&get("name"), 120, true, previous.as_ref().map(|r|r.name.as_str()))?;
        let match_text = py::input_text(&get("match_text"), 200, true, previous.as_ref().map(|r|r.match_text.as_str()))?;
        let payee_name = py::input_text(values.get("payee_name").unwrap_or(&json!("")), 120, false, previous.as_ref().map(|r|r.payee_name.as_str()))?;''')
replace('crates/fb-core/src/payables/mod.rs','''    let reentry = field_text(p, "reentry_reason", 500)?;''','''    let reentry = py::input_text(p.get("reentry_reason").unwrap_or(&json!("")), 500, false, previous.and_then(|p|p["reentry_reason"].as_str()))?;''')
