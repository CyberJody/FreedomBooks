"""Preserve Django's undo reason cleaning, wording and ordering."""
from pathlib import Path
root=Path(__file__).resolve().parents[1]
p=root/'crates/fb-core/src/banking/reconcile.rs';t=p.read_text()
old='''        let uk = uuid(undo)?;
        let reason = text(reason, 500, true)?;'''
new='''        let uk = uuid(undo)?;
        if let Some(raw) = reason.as_str() { py::clean_free_text(raw)?; }
        let reason = reason.as_str().map(py::py_strip).unwrap_or("");
        if reason.is_empty() {
            return Err(FbError::validation("Give a reason for undoing this reconciliation."));
        }
        if py::py_len(reason) > 500 {
            return Err(FbError::validation("Keep the reason to 500 characters."));
        }'''
assert t.count(old)==1;p.write_text(t.replace(old,new))
p=root/'crates/fb-core/src/banking/tests.rs';t=p.read_text();t+='\n#[path = "dfix3_undo_tests.rs"]\nmod dfix3_undo;\n';p.write_text(t)
