"""Match the frozen Decimal parse error separately from precision/range errors."""
from pathlib import Path
root=Path(__file__).resolve().parents[1]
p=root/'crates/fb-core/src/banking/reconcile.rs';t=p.read_text()
old='''    let raw = raw.trim_start_matches('-').trim_start_matches('$');
    let n = cents(&json!(raw)).map_err(|_| {'''
new='''    let raw = raw.trim_start_matches('-').trim_start_matches('$');
    if fb_money::parse_python_decimal(raw).is_none() {
        return Err(FbError::validation(format!("Enter the {label} as an amount such as 1250.00.")));
    }
    let n = cents(&json!(raw)).map_err(|_| {'''
assert t.count(old)==1;p.write_text(t.replace(old,new))
# Keep finding 4's separate regression out of finding 3's commit.
p=root/'crates/fb-core/src/banking/dfix3_tests.rs';t=p.read_text();needle='#[test]\nfn undo_reason_matches_python_before_lookup_and_without_writes()'
a,b=t.split(needle)
(p.parent/'dfix3_undo_tests.rs').write_text('use super::*;\n\n'+needle+b)
p.write_text(a)
