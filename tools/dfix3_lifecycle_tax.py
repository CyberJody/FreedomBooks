"""Retain unchanged tax master text while refusing edited text at its boundary."""
from pathlib import Path
root=Path(__file__).resolve().parents[1]
p=root/'crates/fb-core/src/salestax.rs'
t=p.read_text()
old='''fn text(v: &Value, max: usize, label: &str, required: bool) -> FbResult<String> {
    if let Some(raw)=v.as_str() { crate::py::clean_free_text(raw)?; }'''
new='''fn text(v: &Value, max: usize, label: &str, required: bool) -> FbResult<String> {
    retained_text(v, max, label, required, None)
}
fn retained_text(v: &Value, max: usize, label: &str, required: bool, previous: Option<&str>) -> FbResult<String> {
    if let Some(raw)=v.as_str() { py::clean_changed_free_text(raw, previous)?; }'''
assert t.count(old)==1
t=t.replace(old,new)
old='''        let name=text(&input["name"],60,"a name",true)?;let description=text(&input["description"],120,"a description to print on forms",true)?;'''
new='''        let retained=input["item_id"].as_str().map(|id|item(tx,company,id)).transpose()?;
        let name=retained_text(&input["name"],60,"a name",true,retained.as_ref().and_then(|r|r["name"].as_str()))?;
        let description=retained_text(&input["description"],120,"a description to print on forms",true,retained.as_ref().and_then(|r|r["description"].as_str()))?;'''
assert t.count(old)==2
p.write_text(t.replace(old,new))
