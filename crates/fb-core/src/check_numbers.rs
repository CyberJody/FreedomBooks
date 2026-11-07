//! Python checks.numbering namespace; provider views are extended at integration.
use crate::{CompanyId, FbError, FbResult};
use fb_db::{
    Query,
    rusqlite::{OptionalExtension, params},
};
pub(crate) fn payment_number<'a>(method: &str, reference: &'a str) -> &'a str {
    if method == "CHECK"
        && (1..=20).contains(&reference.len())
        && reference.bytes().all(|b| b.is_ascii_digit())
    {
        reference
    } else {
        ""
    }
}
pub(crate) fn refuse_conflict(
    q: &(impl Query + ?Sized),
    company: CompanyId,
    bank: &str,
    number: &str,
    exclude_check: Option<&str>,
) -> FbResult<()> {
    if number.is_empty() {
        return Ok(());
    }
    let found: Option<(String,String)> = q.query_row("SELECT h.kind,i.number FROM fb_check_number_live h JOIN documents_documentidentity i ON i.id=h.identity_id WHERE h.company_id=? AND h.bank_id=? AND h.number=? AND NOT(h.kind='check' AND h.holder_id=coalesce(?,'')) ORDER BY h.priority,h.holder_id LIMIT 1",params![company.to_db(),bank,number,exclude_check],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    if let Some((kind, document)) = found {
        let release = match kind.as_str() {
            "check" => "void that check",
            "bill payment" => "reverse that payment",
            _ => "void that refund",
        };
        return Err(FbError::Validation(vec![format!(
            "Check number {number} is already used by {kind} {document} on this bank account. Choose another number or {release} first."
        )]));
    }
    Ok(())
}
