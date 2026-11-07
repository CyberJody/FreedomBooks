use super::*;
use crate::payables;
pub fn number_text(v: &Value) -> FbResult<String> {
    let value = v
        .as_str()
        .ok_or_else(|| err("Enter a check number up to 20 characters."))?;
    fb_db::check_numbers::normalize(value)
        .map(str::to_owned)
        .map_err(err)
}
fn digits(n: &str) -> bool {
    (1..=20).contains(&n.len()) && n.bytes().all(|c| c.is_ascii_digit())
}
pub fn next_number(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    bank: AccountId,
) -> FbResult<String> {
    book.read(|c|{
        services::access(c,actor,co)?;
        account(c,co,&bank.to_db())?;
        next_number_tx(c,co,bank)
    })
}
pub(crate) fn next_number_tx(c: &(impl Query + ?Sized), co: CompanyId, bank: AccountId) -> FbResult<String> {
        if let Some(next)=crate::numbering::check_next(c,co,bank)? {return Ok(next);}
        let used=rows(c,"SELECT number AS n FROM fb_check_number_used WHERE company_id=?1 AND bank_id=?2",&[&co.to_db(),&bank.to_db()])?;
        let numbers=used.iter().map(|v|s(v,"n")).collect::<BTreeSet<_>>();
        let taken=numbers.iter().filter(|n|digits(n)).filter_map(|n|n.parse::<u128>().ok()).collect::<BTreeSet<_>>();
        let start=taken.last().copied().unwrap_or(0)+1;
        for n in start..start+64 {
            let text=n.to_string();
            if text.len()>20 { break; }
            if !numbers.contains(text.as_str()) { return Ok(text); }
        }
        let mut n=1u128;while taken.contains(&n)||numbers.contains(n.to_string().as_str()){n+=1;}
        Ok(n.to_string())
}
pub fn refuse_conflict(
    c: &(impl Query + ?Sized),
    co: CompanyId,
    bank: &str,
    number: &str,
    exclude: Option<&str>,
) -> FbResult<()> {
    crate::check_numbers::refuse_conflict(c, co, bank, number, exclude)
}
pub(super) fn validate_number(
    c: &(impl Query + ?Sized),
    co: CompanyId,
    cid: Option<&str>,
    bank: &str,
    number: &str,
) -> FbResult<()> {
    if !number.is_empty() {
        let exists:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM checks_check WHERE company_id=? AND bank_id=? AND number_key=? AND state<>'VOID' AND id<>coalesce(?,''))",params![co.to_db(),bank,number,cid],|r|r.get(0))?;
        if exists {
            return Err(err(&format!(
                "Check number {number} is already used by a live check on this bank account. Choose another number or void the other check first."
            )));
        }
    }
    refuse_conflict(c, co, bank, number, cid)
}
// Payables owns posting. This compatibility facade adds the shared-number preflight;
// a DB guard repeats it under payables' writer lock, closing the intervening race.
fn payment_preflight(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    p: &Value,
    retry: bool,
) -> FbResult<()> {
    book.read(|c|{
        services::access(c,actor,co)?;
        if retry && let Some(request_key) = py::py_uuid(s(p,"request_key")) {
            let exists:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM payables_billpayment p JOIN documents_documentidentity i ON i.id=p.identity_id WHERE p.company_id=? AND i.request_key=?)",params![co.to_db(),request_key.0],|r|r.get(0))?;
            if exists{return Ok(());}
        }
        let number=field_text(p,"reference",120)?;
        if s(p,"method")=="CHECK"&&digits(&number){refuse_conflict(c,co,s(p,"bank_id"),&number,None)?;}
        Ok(())
    })
}
pub fn review_bill_payment(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    p: &Value,
) -> FbResult<payables::Review> {
    payment_preflight(book, actor, co, p, false)?;
    payables::review_payment(book, actor, co, p)
}
pub fn record_bill_payment(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    p: &Value,
    review: &payables::Review,
) -> FbResult<Value> {
    payment_preflight(book, actor, co, p, true)?;
    match payables::record_payment(book, actor, co, p, review) {
        Err(e) if e.constraint() == Some("check_payment_number") => {
            payment_preflight(book, actor, co, p, false)?;
            Err(err("Check number already used on this bank."))
        }
        v => v,
    }
}
