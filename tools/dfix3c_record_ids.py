"""FIX3c: v2 record-ID checks (Python takes int primary keys there) keep the strict UUID grammar."""
from pathlib import Path

root = Path(__file__).resolve().parents[1]
edits = {
    'crates/fb-core/src/checks/support.rs': [('36 == s.len() && py::py_uuid(s).is_some()', '36 == s.len() && py::record_uuid(s).is_some()')],
    'crates/fb-core/src/payables/mod.rs': [('36 == s.len() && py::py_uuid(s).is_some()', '36 == s.len() && py::record_uuid(s).is_some()')],
    'crates/fb-core/src/bankfeeds/mod.rs': [('s.len() == 36 && py::py_uuid(s).is_some()', 's.len() == 36 && py::record_uuid(s).is_some()')],
    'crates/fb-core/src/salestax.rs': [('request["tax_item_id"].as_str().and_then(py::py_uuid).is_none()',
                                       'request["tax_item_id"].as_str().and_then(py::record_uuid).is_none()')],
}
for name, pairs in edits.items():
    path = root / name
    text = path.read_text()
    for old, new in pairs:
        assert text.count(old) == 1, (name, old)
        text = text.replace(old, new)
    path.write_text(text)
    print('patched', name)
