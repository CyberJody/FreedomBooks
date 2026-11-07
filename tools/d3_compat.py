"""Keep the frozen forms' per-field legacy omission semantics."""
from pathlib import Path
def edit(name,old,new):
    p=Path(name);s=p.read_text();assert old in s,(name,old);p.write_text(s.replace(old,new))
edit('crates/fb-app/src/commands.rs','defaults_shown: bool','expense_shown: bool, terms_shown: bool')
edit('crates/fb-app/src/screens/d3.rs','if let DirectoryValues::Vendor{defaults_shown,..}=values {map.remove("defaults_shown");if !defaults_shown {map.remove("default_expense_account");map.remove("default_terms");}}','if let DirectoryValues::Vendor{expense_shown,terms_shown,..}=values {map.remove("expense_shown");map.remove("terms_shown");if !expense_shown {map.remove("default_expense_account");}if !terms_shown{map.remove("default_terms");}}')
edit('crates/fb-app/src/screens/d3.rs','defaults_shown:true','expense_shown:get(p,"default_expense_account").is_some(),terms_shown:get(p,"default_terms").is_some()')
edit('crates/fb-app/src/screens/d3.rs','get(p,"taxable_shown").is_some_and(|s|!s.is_empty())','get(p,"taxable_shown").is_some_and(|s|!py::py_strip(s).is_empty())')
# Read-only rendering should not silently consume errors or parse aggregates through money input limits.
edit('crates/fb-app/src/screens/d3/reminders.rs','        let ordinal=open.book.read(|tx|d2::ordinals(tx,c,&m.table()))?;','')
edit('crates/fb-app/src/screens/d3/reminders.rs','        }let _=ordinal;','        }')
edit('crates/fb-app/src/screens/d3/reminders.rs','&decorate(fs,&errors.clone())','&decorate(fs,errors)')
