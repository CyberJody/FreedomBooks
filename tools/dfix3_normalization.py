"""Scope undo-key normalization to generated GET forms, using screen-request provenance."""
from pathlib import Path
root=Path(__file__).resolve().parents[1]
def replace(name,old,new):
    p=root/name;t=p.read_text();assert t.count(old)==1,(name,old);p.write_text(t.replace(old,new))
replace('crates/fb-screendiff/src/normalize.rs','pub struct IdMap {','#[derive(Clone)]\npub struct IdMap {\n    generated_undo_key: bool,')
replace('crates/fb-screendiff/src/normalize.rs','        IdMap { by_model, labels }','        IdMap { by_model, labels, generated_undo_key: false }')
replace('crates/fb-screendiff/src/normalize.rs','''    fn get(&self, model: &str, id: &str) -> Option<&str> {''','''    pub fn for_screen(&self, path: &str, is_post: bool) -> Self {
        let mut scoped = self.clone();
        scoped.generated_undo_key = path.ends_with("/undo/") && path.contains("/reconcile/") && !is_post;
        scoped
    }
    fn get(&self, model: &str, id: &str) -> Option<&str> {''')
replace('crates/fb-screendiff/src/normalize/d7b.rs','''                "completion_key" | "undo_key" if fb_core::py::py_uuid(raw).is_some() => {''','''                "completion_key" if fb_core::py::py_uuid(raw).is_some() => {
                    format!("{{{}}}", &c[2])
                }
                "undo_key" if ids.generated_undo_key && fb_core::py::py_uuid(raw).is_some() => {''')
replace('crates/fb-screendiff/src/main.rs','''        let a = normalize(&django, &py_map);
        let b = normalize(&rendered.html, &rs_map);''','''        let is_post = screen.get("post").is_some();
        let a = normalize(&django, &py_map.for_screen(&path, is_post));
        let b = normalize(&rendered.html, &rs_map.for_screen(&path, is_post));''')
