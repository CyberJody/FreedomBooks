//! Sales checks are part of Book::open/Book::check; callbacks register before the authorizer.
use super::*;
pub fn initialize(book: &Book) -> FbResult<()> {
    let problems = check(book)?;
    if problems.is_empty() {
        Ok(())
    } else {
        Err(FbError::validation(format!(
            "Sales integrity check failed: {}",
            problems.join("; ")
        )))
    }
}
pub fn check(book: &Book) -> FbResult<Vec<String>> {
    book.check().map_err(|e| FbError::validation(e.to_string()))
}
