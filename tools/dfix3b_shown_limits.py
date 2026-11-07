"""Keep Python's character-count allowance when UUIDs replace 18-digit PKs."""
from pathlib import Path
p=Path(__file__).resolve().parents[1]/'crates/fb-app/src/screens/d7b.rs'
t=p.read_text()
old='''    // UUIDs replace integer PKs. Keep the same count and revision grammar limits.
    if raw.len() > 46 * 900 {'''
new='''    // Python permits 30 characters per pair for 18-digit PKs. UUIDs add
    // 18 characters; count Unicode scalars (as Python len does), not bytes.
    if raw.chars().count() > 48 * 900 {'''
assert t.count(old)==1
t=t.replace(old,new)
old='''        assert_eq!(shown(&"x".repeat(46 * 900 + 1)).unwrap_err(), vec![RELOAD]);'''
new='''        assert_eq!(shown(&"x".repeat(48 * 900 + 1)).unwrap_err(), vec![RELOAD]);
        for revision in ["999999999", "９９９９９９９９９"] {
            let maximum_pair=format!("{id}:{revision}");
            assert_eq!(shown(&vec![maximum_pair;900].join(",")).unwrap()[id],999999999);
        }'''
assert t.count(old)==1
p.write_text(t.replace(old,new))
