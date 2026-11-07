"""Complete the raw service validator audit: shown/ticked before record lookup."""
from pathlib import Path
p=Path(__file__).resolve().parents[1]/'crates/fb-core/src/banking/reconcile.rs'
t=p.read_text()
start=t.index('fn required_revision(')
end=t.index('fn optional_revision(',start)
t=t[:start]+'''fn integer_value(value: &Value) -> Option<i64> {
    value.as_i64().or_else(|| value.as_bool().map(i64::from))
        .or_else(|| match value.as_str().and_then(py::py_int) {
            Some(py::PyInt::Small(n)) => Some(n),
            Some(py::PyInt::Large { negative }) => Some(if negative { i64::MIN } else { i64::MAX }),
            _ => None,
        })
        // Python int() truncates floats. Values beyond i64 saturate here: they
        // cannot equal a stored revision, but a no-op tick save ignores them.
        .or_else(|| value.as_f64().filter(|n| n.is_finite()).map(|n| n.trunc() as i64))
}
fn required_revision(value: &Value) -> FbResult<i64> {
    integer_value(value).filter(|n| !value.is_boolean() && (0..=1_000_000_000).contains(n))
        .ok_or_else(|| FbError::validation("Reload the page and try again."))
}
'''+t[end:]
old='''        let mut rec = reconciliation(tx, c, key, aid)?;
        if rec["status"] != "OPEN" {
            return Err(FbError::validation(
                "The reconciliation is no longer in progress.",
            ));
        }
'''
assert t.count(old)==1
t=t.replace(old,'')
old='''            .map(id)
            .collect::<FbResult<BTreeSet<_>>>()?;'''
new='''            .map(|v| id(v).map_err(|_| FbError::validation("Reload the reconciliation and try again.")))
            .collect::<FbResult<BTreeSet<_>>>()?;'''
assert t.count(old)==1
t=t.replace(old,new)
needle='''        let hide = input
'''
insert='''        let shown = shown.iter().map(|(key, value)| {
            if fb_ids::parse_uuid(key).is_none() && py::py_int(key).is_none() {
                return Err(FbError::validation("Reload the reconciliation and try again."));
            }
            integer_value(value).map(|n| (key.clone(), n))
                .ok_or_else(|| FbError::validation("Reload the reconciliation and try again."))
        }).collect::<FbResult<BTreeMap<_, _>>>()?;
        let mut rec = reconciliation(tx, c, key, aid)?;
        if rec["status"] != "OPEN" {
            return Err(FbError::validation("The reconciliation is no longer in progress."));
        }
'''
assert t.count(needle)==1
t=t.replace(needle,insert+needle)
old='''        for (k, v) in shown {
            let expected = v
                .as_i64()
                .filter(|n| *n >= 0)
                .ok_or_else(|| FbError::validation("Reload the reconciliation and try again."))?;'''
new='''        for (k, expected) in &shown {
            let expected = *expected;'''
assert t.count(old)==1
t=t.replace(old,new)
p.write_text(t)
