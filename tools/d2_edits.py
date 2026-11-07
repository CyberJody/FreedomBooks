"""Asserted, reviewable D2 source edits (run from the workspace root)."""
from pathlib import Path


def replace(path, old, new):
    p = Path(path)
    text = p.read_text()
    assert old in text, (path, old)
    p.write_text(text.replace(old, new))


replace('crates/fb-core/src/lib.rs', 'pub mod documents;', 'pub mod documents;\npub mod first_run;')
replace('crates/fb-core/Cargo.toml', 'sha2 = "0.10"', 'sha2 = "0.10"\nregex = "1"')
# Expose the identical save body for the missing first-run service's atomic composition.
replace('crates/fb-core/src/documents.rs', '''    book.write(|tx|{
        let user=services::access(tx,actor,company)?;
        let before=setup_record(tx,company)?;''', '''    book.write(|tx| save_setup_tx(tx, actor, company, values, expected_revision))
}

pub(crate) fn save_setup_tx(tx: &Writer<'_>, actor: UserId, company: CompanyId, values: &Value, expected_revision: Option<i64>) -> FbResult<Value> {
        let user=services::access(tx,actor,company)?;
        let before=setup_record(tx,company)?;''')
replace('crates/fb-core/src/documents.rs', '''        Ok(after)
    })
}
pub fn lock_configuration''', '''        Ok(after)
}
pub fn lock_configuration''')
