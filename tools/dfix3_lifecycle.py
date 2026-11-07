"""Keep strict character validation at input boundaries, with explicit retained provenance."""
from pathlib import Path
ROOT = Path(__file__).resolve().parents[1]
def replace(name, old, new):
    p = ROOT / ('crates/fb-core/src/' + name)
    text = p.read_text()
    assert text.count(old) == 1, (name, old)
    p.write_text(text.replace(old, new))

replace('services.rs', '    py::clean_free_text(&company.name)?;\n', '')
replace('services.rs', '        py::clean_free_text(name)?;',
        '        py::clean_changed_free_text(name, Some(&company.name))?;')
replace('services.rs', '    for text in [&a.code, &a.name, &a.description] { py::clean_free_text(text)?; }\n', '')
for key in ['code', 'name']:
    replace('services.rs', f'py::clean_free_text(&input.{key})?;',
            f'py::clean_changed_free_text(&input.{key}, existing.as_ref().map(|a| a.{key}.as_str()))?;')
replace('services.rs', 'py::clean_free_text(d)?;',
        'py::clean_changed_free_text(d, existing.as_ref().map(|a| a.description.as_str()))?;')
replace('directory.rs', '                py::clean_free_text(raw)?;',
        '                py::clean_changed_free_text(raw, before.as_ref().and_then(|b| b[f].as_str()))?;')
replace('directory.rs', '        if let Err(e) = py::clean_free_text(text) {\n            errors.push(e.message());\n        }\n', '')
# Journal normalization is also used for source posting and exact reversals. Those strings
# were already admitted at their owning input boundary (possibly by an earlier version).
replace('services.rs', '    if let Some(Value::String(raw)) = value { py::clean_free_text(raw)?; }\n', '')
replace('services.rs', '''    py::clean_free_text(&input.memo)?;
    let user = access(tx, actor, company)?;
    let manual = reversal_of.is_none() && source.is_none();
    let rows = normalize_lines(&input.lines, manual)?;''', '''    let user = access(tx, actor, company)?;
    let manual = reversal_of.is_none() && source.is_none();
    if manual {
        let previous = input.journal_id.map(|id| store::journal_in(tx, id, company)).transpose()?;
        py::clean_changed_free_text(&input.memo, previous.as_ref().map(|j| j.memo.as_str()))?;
        let old_lines = previous.as_ref().map(|j| store::lines(tx, j.id)).transpose()?.unwrap_or_default();
        for (index, row) in input.lines.iter().enumerate() {
            let prior = old_lines.get(index);
            for (key, old) in [("description", prior.map(|l| l.description.as_str())), ("name", prior.map(|l| l.name.as_str()))] {
                if let Some(raw) = row[key].as_str() { py::clean_changed_free_text(raw, old)?; }
            }
        }
    }
    let rows = normalize_lines(&input.lines, manual)?;''')
# Calculators process historical tax evidence too. Custom labels are checked on input instead.
for module in ['sales', 'credits']:
    replace(module+'/mod.rs', '    if let Some(label) = tax["tax_label"].as_str() { py::clean_free_text(label)?; }\n', '')

for module, model in [('sales/invoices.rs', 'invoice'), ('sales/estimates.rs', 'estimate')]:
    replace(module, 'fn normalize(input: &Value, editing: bool)', 'fn normalize(input: &Value, old: Option<&Value>)')
    replace(module, '    let customer = id(&input["customer_id"], false)?.ok_or_else(invalid_stored)?;',
        '''    let editing = old.is_some();
    let previous = old.map(|o| &o["current"]);
    if let Some(raw) = input["tax_label"].as_str() {
        py::clean_changed_free_text(raw, previous.and_then(|p| p["tax_label"].as_str()))?;
    }
    let customer = id(&input["customer_id"], false)?.ok_or_else(invalid_stored)?;''')
    replace(module, 'field_text(row,"description",1000,true)?',
        'py::input_text(&row["description"],1000,true,py::retained_line_text(old.and_then(|o| o["lines"].as_array()), &origin, "description"))?')
    replace(module, 'field_text(input,"memo",2000,false)?',
        'py::input_text(input.get("memo").unwrap_or(&json!("")),2000,false,previous.and_then(|p| p["memo"].as_str()))?')
    replace(module, f'''        let normalized = normalize(input, editing.is_some())?;
        let hash = digest(&normalized);
        let old = editing.map(|key| {model}(tx, c, key)).transpose()?;''',
        f'''        let old = editing.map(|key| {model}(tx, c, key)).transpose()?;
        let normalized = normalize(input, old.as_ref())?;
        let hash = digest(&normalized);''')

for module, model in [('payables/mod.rs','bill'), ('checks/mod.rs','check')]:
    signature = 'fn save_bill_inner' if model == 'bill' else 'fn save_inner'
    replace(module, f'''{signature}(tx: &Writer<'_>, actor: UserId, co: CompanyId, p: &Value) -> FbResult<Value> {{
    services::access(tx, actor, co)?;''',
        f'''{signature}(tx: &Writer<'_>, actor: UserId, co: CompanyId, p: &Value) -> FbResult<Value> {{
    services::access(tx, actor, co)?;
    let retained = p["{model}_id"].as_str().map(|key| {model}(tx, co, key)).transpose()?;
    let previous = retained.as_ref().map(|o| &o["current"]);''')
    replace(module, 'field_text(p, "memo", 2000)?',
        'py::input_text(p.get("memo").unwrap_or(&json!("")), 2000, false, previous.and_then(|p| p["memo"].as_str()))?')
    old = 'text(&l["description"],1000,true)?' if model == 'bill' else 'field_text(l,"description",1000)?'
    replace(module, old,
        f'py::input_text(l.get("description").unwrap_or(&json!("")),1000,{str(model == "bill").lower()},py::retained_line_text(previous.and_then(|p| p["lines"].as_array()), &origin, "description"))?')
    if model == 'check':
        replace(module, '''    let mut payee = text(
        p.get("payee_name").unwrap_or(&json!("")),
        120,
        kind == "OTHER",
    )?;''', '''    let mut payee = py::input_text(
        p.get("payee_name").unwrap_or(&json!("")), 120, kind == "OTHER",
        previous.and_then(|p| p["payee_name"].as_str()),
    )?;''')
        replace(module, 'field_text(p, "address", 1000)?',
            'py::input_text(p.get("address").unwrap_or(&json!("")), 1000, false, previous.and_then(|p| p["address"].as_str()))?')
