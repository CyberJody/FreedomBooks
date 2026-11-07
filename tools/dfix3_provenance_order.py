"""Provenance reads inform retention but preserve the original error boundaries."""
from pathlib import Path
root=Path(__file__).resolve().parents[1]/'crates/fb-core/src'
def replace(name,old,new):
    p=root/name;t=p.read_text()
    if old not in t:
        assert not new or new in t,(name,old)
        return
    assert t.count(old)==1,(name,old);p.write_text(t.replace(old,new))
replace('services.rs','''        let previous = input.journal_id.map(|id| store::journal_in(tx, id, company)).transpose()?;
        py::clean_changed_free_text(&input.memo, previous.as_ref().map(|j| j.memo.as_str()))?;
        let old_lines = previous.as_ref().map(|j| store::lines(tx, j.id)).transpose()?.unwrap_or_default();''','''        // Existing journal/snapshot loads below own their errors, after line cleaning.
        // Only a successful provenance read can exempt an unchanged field.
        let previous_result = input.journal_id.map(|id| store::journal_in(tx, id, company)).transpose();
        let previous = previous_result.as_ref().ok().and_then(|v| v.as_ref());
        py::clean_changed_free_text(&input.memo, previous.map(|j| j.memo.as_str()))?;
        let old_lines_result = previous.map(|j| store::lines(tx, j.id)).transpose();
        let old_lines = old_lines_result.as_ref().ok().and_then(|v| v.as_ref()).map(Vec::as_slice).unwrap_or(&[]);''')
for name,model in [('sales/invoices.rs','invoice'),('sales/estimates.rs','estimate')]:
    replace(name,'fn normalize(input: &Value, old: Option<&Value>)','fn normalize(input: &Value, editing: bool, old: Option<&Value>)')
    replace(name,'    let editing = old.is_some();\n','')
    replace(name,f'''        let old = editing.map(|key| {model}(tx, c, key)).transpose()?;
        let normalized = normalize(input, old.as_ref())?;
        let hash = digest(&normalized);''',f'''        let old = editing.map(|key| {model}(tx, c, key)).transpose();
        let normalized = normalize(input, editing.is_some(), old.as_ref().ok().and_then(|v| v.as_ref()))?;
        let hash = digest(&normalized);
        let old = old?;''')
for name,model in [('payables/mod.rs','bill'),('checks/mod.rs','check')]:
    replace(name,f'''    let retained = p["{model}_id"].as_str().map(|key| {model}(tx, co, key)).transpose()?;
    let previous = retained.as_ref().map(|o| &o["current"]);''',f'''    let retained = p["{model}_id"].as_str().map(|key| {model}(tx, co, key)).transpose();
    let previous = retained.as_ref().ok().and_then(|o| o.as_ref()).map(|o| &o["current"]);''')
    arg='bid' if model=='bill' else 'cid'
    replace(name,f'''    let (mut b, old) = if let Some({arg}) = {arg}.as_deref() {{
        let mut b = {model}(tx, co, {arg})?;''',f'''    let (mut b, old) = if let Some({arg}) = {arg}.as_deref() {{
        let mut b = retained?.ok_or_else(|| crate::FbError::missing("{model.title()}"))?;''')
p=root/'salestax.rs';t=p.read_text()
old='''        let retained=input["item_id"].as_str().map(|id|item(tx,company,id)).transpose()?;'''
new='''        // locked_item below retains its original place in the validation order.
        let retained_result=input["item_id"].as_str().map(|id|item(tx,company,id)).transpose();
        let retained=retained_result.as_ref().ok().and_then(|v|v.as_ref());'''
assert t.count(old)==2;p.write_text(t.replace(old,new))
