//! Python checks.numbering namespace; provider views are extended at integration.
use crate::{CompanyId, FbError, FbResult};
use fb_db::{
    Query,
    rusqlite::{OptionalExtension, params},
};
use std::collections::BTreeSet;
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

/// Worker item 26: the check number a free-text reference names, as a sales tax payment's Reference
/// holds it ("127", "Check 127", "Check #127", "Chk no. 127", "#127"); `None` for other text.
pub(crate) fn number_in_reference(reference: &str) -> Option<u128> {
    let lower = reference.trim().to_ascii_lowercase();
    let mut rest = lower.as_str();
    for word in ["check", "chk", "ck"] {
        if let Some(r) = rest.strip_prefix(word) {
            rest = r.trim_start();
            break;
        }
    }
    for word in ["number", "no.", "no", "#"] {
        if let Some(r) = rest.strip_prefix(word) {
            rest = r.trim_start();
        }
    }
    let rest = rest.trim_start_matches(['#', ':']).trim();
    if rest.is_empty() || rest.len() > 20 || !rest.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    rest.parse().ok()
}

/// Check numbers that sales tax payments from `bank` name in their Reference (live or voided). That
/// form has no check-number box (owner decision of 2026-11-23), so these are not reserved; the next
/// suggested number only steps over them.
pub(crate) fn tax_payment_numbers(q: &(impl Query + ?Sized), company: CompanyId, bank: &str) -> FbResult<BTreeSet<u128>> {
    let mut st = q.prepare("SELECT reference FROM salestax_payment WHERE company_id=?1 AND bank_id=?2")?;
    let refs = st.query_map(params![company.to_db(), bank], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
    Ok(refs.iter().filter_map(|r| number_in_reference(r)).collect())
}

/// Worker item 26: how check `number` was used before on a bank account, where that use does not
/// reserve the number (so a later document may still take it). For a warning only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EarlierUse {
    /// A voided check, a reversed bill payment or a voided refund: the number may be used again.
    /// `how` is "which was voided" or "which was reversed".
    Released { document: String, how: &'static str },
    /// A sales tax payment whose Reference names the number. That form has no check-number box
    /// (owner decision of 2026-11-23), so the program cannot tell whether that check was written.
    TaxPayment { document: String, reference: String, voided: bool },
}
impl EarlierUse {
    /// "BPAY-000003, which was reversed", "sales tax payment STP-000002 (reference “Check 127”)",
    /// "sales tax payment STP-000002 (reference “Check 127”), which was voided".
    pub fn text(&self) -> String {
        match self {
            EarlierUse::Released { document, how } => format!("{document}, {how}"),
            EarlierUse::TaxPayment { document, reference, voided } => {
                format!("sales tax payment {document} (reference “{reference}”){}", if *voided { ", which was voided" } else { "" })
            }
        }
    }
    pub fn is_tax_payment(&self) -> bool {
        matches!(self, EarlierUse::TaxPayment { .. })
    }
}

/// Worker item 26: an earlier use of check `number` on `bank` that does not reserve it (a voided check,
/// a reversed bill payment, a voided refund) or that the program cannot reserve (a sales tax payment's
/// reference). A live tax payment comes first (its check may have gone out), then the released uses,
/// then a voided tax payment.
pub fn earlier_use(q: &(impl Query + ?Sized), company: CompanyId, bank: &str, number: &str) -> FbResult<Option<EarlierUse>> {
    if number.is_empty() {
        return Ok(None);
    }
    let named = match number.parse::<u128>().ok().filter(|_| number.bytes().all(|b| b.is_ascii_digit())) {
        None => vec![],
        Some(wanted) => {
            let mut st = q.prepare(
                "SELECT i.number, t.reference, EXISTS(SELECT 1 FROM salestax_paymentvoid v WHERE v.payment_id=t.id)
                 FROM salestax_payment t JOIN documents_documentidentity i ON i.id=t.identity_id
                 WHERE t.company_id=?1 AND t.bank_id=?2 ORDER BY t.date DESC, i.number DESC",
            )?;
            let rows = st
                .query_map(params![company.to_db(), bank], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, bool>(2)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            rows.into_iter()
                .filter(|(_, reference, _)| number_in_reference(reference) == Some(wanted))
                .map(|(document, reference, voided)| EarlierUse::TaxPayment { document, reference, voided })
                .collect()
        }
    };
    if let Some(live) = named.iter().find(|u| matches!(u, EarlierUse::TaxPayment { voided: false, .. })) {
        return Ok(Some(live.clone()));
    }
    let found: Option<(String, i64)> = q
        .query_row(
            "SELECT i.number, x.o FROM (
               SELECT k.identity_id AS identity_id, 1 AS o FROM checks_check k
                 WHERE k.company_id=?1 AND k.bank_id=?2 AND k.number_key=?3 AND k.state='VOID'
               UNION ALL SELECT p.identity_id, 2 FROM payables_billpayment p
                 WHERE p.company_id=?1 AND p.bank_id=?2 AND p.method='CHECK' AND p.reference=?3
                 AND EXISTS(SELECT 1 FROM payables_paymentreversal v WHERE v.payment_id=p.id)
               UNION ALL SELECT r.identity_id, 3 FROM credits_creditrefund r
                 WHERE r.company_id=?1 AND r.bank_id=?2 AND r.check_number=?3
                 AND EXISTS(SELECT 1 FROM credits_creditrefundvoid v WHERE v.refund_id=r.id)
               UNION ALL SELECT r.identity_id, 4 FROM prepayments_depositrefund r
                 WHERE r.company_id=?1 AND r.bank_id=?2 AND r.check_number=?3
                 AND EXISTS(SELECT 1 FROM prepayments_depositrefundvoid v WHERE v.refund_id=r.id)
             ) x JOIN documents_documentidentity i ON i.id=x.identity_id ORDER BY x.o, i.number DESC LIMIT 1",
            params![company.to_db(), bank, number],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((document, source)) = found {
        return Ok(Some(EarlierUse::Released { document, how: if source == 2 { "which was reversed" } else { "which was voided" } }));
    }
    Ok(named.into_iter().next())
}

#[cfg(test)]
mod reference_tests {
    use super::number_in_reference;
    #[test]
    fn a_reference_names_a_check_number_only_in_check_wording() {
        for (text, n) in [("127", Some(127)), ("Check 127", Some(127)), ("check #127", Some(127)), ("Chk no. 0127", Some(127)), ("#127", Some(127)),
            ("CK 5150", Some(5150)), ("Check number 88", Some(88)), ("Online payment", None), ("Confirmation 88812345", None), ("2026-06", None), ("", None), ("Check", None)] {
            assert_eq!(number_in_reference(text), n, "{text}");
        }
    }
}
