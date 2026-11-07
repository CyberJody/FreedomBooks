"""Translate reconcile's Python validators and their service ordering exactly."""
from pathlib import Path
root = Path(__file__).resolve().parents[1]
p = root / 'crates/fb-core/src/banking/reconcile.rs'
t = p.read_text()
def replace(old, new):
    global t
    assert t.count(old) == 1, old[:100]
    t = t.replace(old, new)

replace('use super::*;\n', '''use super::*;

// These are reconcile's _uuid/_revision, rather than the generic banking rules.
fn request_key(value: &Value, what: &str) -> FbResult<String> {
    value.as_str().and_then(py::py_uuid).map(|u| u.0)
        .ok_or_else(|| FbError::validation(format!("Reload the page and try again (missing {what} key).")))
}
fn required_revision(value: &Value) -> FbResult<i64> {
    let n = value.as_i64().or_else(|| match value.as_str().and_then(py::py_int) {
        Some(py::PyInt::Small(n)) => Some(n),
        _ => None,
    });
    n.filter(|n| (0..=1_000_000_000).contains(n))
        .ok_or_else(|| FbError::validation("Reload the page and try again."))
}
fn optional_revision(value: &Value) -> FbResult<Option<i64>> {
    if value.is_null() || value.as_str() == Some("") { Ok(None) }
    else { required_revision(value).map(Some) }
}
''')
replace('        let key = uuid(&input["request_key"])?;', '        let key = request_key(&input["request_key"], "request")?;')
replace('        let rev = revision(&input["expected_revision"])?;', '        let rev = optional_revision(&input["expected_revision"])?;') if t.count('        let rev = revision(&input["expected_revision"])?;') == 1 else None
# begin and save_ticks require different treatment of missing revisions.
old = '        let rev = revision(&input["expected_revision"])?;'
if t.count(old) == 2:
    t = t.replace(old, '        let rev = optional_revision(&input["expected_revision"])?;', 1)
replace(old, '        let rev = Some(required_revision(&input["expected_revision"])?);')
replace('        let ck = uuid(completion)?;', '        let ck = request_key(completion, "completion")?;\n        let expected = Some(required_revision(&json!(expected))?);')
replace('        let uk = uuid(undo)?;', '        let uk = request_key(undo, "undo")?;\n        let expected = Some(required_revision(&json!(expected))?);')

# Resolve both optional posting accounts before checking reconciliation state,
# as Python does; do not defer account validation until posting a journal.
start = t.index('    let kind = if interest { "INCOME" } else { "EXPENSE" };')
end = t.index('    let namespace = ', start)
validation = t[start:end]
validation = validation.replace('''    let other = id(other).map_err(|_| {
        FbError::validation(format!(
            "Choose the account for the {}.",
            label.to_lowercase()
        ))
    })?;''', '''    let other = other.as_str().filter(|s| fb_ids::parse_uuid(s).is_some() || py::py_int(s).is_some())
        .ok_or_else(|| FbError::validation(format!("Choose the account for the {}.", label.to_lowercase())))?;''')
validation = validation.replace('account(tx, c, &other)', 'account(tx, c, other)')
label_start = t.index('    let label = if interest {', t.index('fn charge('))
label_end = t.index('    let kind = ', label_start)
label = t[label_start:label_end]
helper = '''fn charge_account(tx: &Writer<'_>, c: CompanyId, a: &crate::model::Account, other: &Value, interest: bool) -> FbResult<crate::model::Account> {
''' + label + validation + '''    Ok(b)
}
'''
t = t[:start] + '    let b = charge_account(tx, c, a, other, interest)?;\n' + t[end:]
t = t.replace('fn charge(\n', helper + 'fn charge(\n', 1)
replace('''        if earned.is_some() && a.section_balance() == crate::model::Side::Credit {''', '''        if ch.is_some() { charge_account(tx, c, &a, &input["service_account_id"], false)?; }
        if earned.is_some() && a.section_balance() == crate::model::Side::Credit {''')
replace('''        if last(tx, aid)?.is_some_and''', '''        if earned.is_some() { charge_account(tx, c, &a, &input["interest_account_id"], true)?; }
        if last(tx, aid)?.is_some_and''')
p.write_text(t)
print('Reconcile validators and ordering translated')
