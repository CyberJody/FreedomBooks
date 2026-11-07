"""The printed UTC balance date follows the UTC instant, independently of local entry defaults."""
from pathlib import Path
import sys
ROOT=Path(__file__).resolve().parents[1]
if sys.argv[1]=='test':
    p=ROOT/'crates/fb-app/tests/d4_flow.rs';text=p.read_text()
    old='''fn print_invoice_uses_stored_revision_and_only_current_open_balance_without_writes() {
    let mut f = Fixture::new();'''
    new=old+'''
    // 19:30 local is already the following UTC day. Entry defaults stay local.
    f.app.today=||chrono::NaiveDate::from_ymd_opt(2026,11,1).unwrap();
    f.app.now=||chrono::DateTime::parse_from_rfc3339("2026-11-02T00:30:00Z").unwrap().with_timezone(&chrono::Utc);'''
    assert text.count(old)==1;text=text.replace(old,new)
    old='''    assert!(open.contains("Net 31"));'''
    new=old+'''
    assert!(open.contains("Printed 2026-11-02 00:30 UTC"));
    assert!(open.contains("as of 2026-11-02 UTC"),"UTC-labelled balance date must follow the print clock: {open}");
    assert_eq!((f.app.today)(),chrono::NaiveDate::from_ymd_opt(2026,11,1).unwrap());'''
    assert text.count(old)==1;text=text.replace(old,new);p.write_text(text)
    print('PRINT: failing-first UTC/local midnight regression added')
else:
    assert sys.argv[1]=='fix'
    p=ROOT/'crates/fb-app/src/screens/printing.rs';text=p.read_text()
    old='"as_of":(app.today)().to_string()';new='"as_of":(app.now)().date_naive().to_string()'
    assert text.count(old)==1;p.write_text(text.replace(old,new))
    print('PRINT: UTC-labelled print balance date uses the UTC print clock; local entry defaults unchanged')
