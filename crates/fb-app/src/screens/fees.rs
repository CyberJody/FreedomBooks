//! Worker item 47: "Fees and adjustments" lines on Record deposit, on Receive payment deposited straight to
//! a bank, and on a Review row deposited from received payments (item 48). Each line has an account and a
//! signed amount (a fee is negative). The page posts them as repeated `fee_account` / `fee_amount` fields
//! (`<prefix>account` / `<prefix>amount`); the core checks the amounts and the accounts.
use super::*;

/// One fee or adjustment line as typed.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct FeeLine {
    pub account: String,
    pub amount: String,
}

/// The lines a post carries under `prefix` (`fee_` posts `fee_account` and `fee_amount`), in page order.
/// A line with no amount is not a line (the form starts with the bank-charges account on an empty line).
pub(crate) fn posted(p: Posted<'_>, prefix: &str) -> Vec<FeeLine> {
    posted_as(p, &format!("{prefix}account"), &format!("{prefix}amount"))
}
/// [`posted`] for lines named `account` and `amount` (a Review row's `fee_account_<pk>`, `fee_amount_<pk>`).
pub(crate) fn posted_as(p: Posted<'_>, account: &str, amount: &str) -> Vec<FeeLine> {
    let take = |name: &str| p.iter().filter(|(k, _)| k == name).map(|(_, v)| v.trim().to_owned()).take(fb_db::banking_evidence::MAX_DEPOSIT_ADJUSTMENTS + 5).collect::<Vec<_>>();
    let (accounts, amounts) = (take(account), take(amount));
    (0..accounts.len().max(amounts.len()))
        .map(|i| FeeLine { account: accounts.get(i).cloned().unwrap_or_default(), amount: amounts.get(i).cloned().unwrap_or_default() })
        .filter(|l| !l.amount.is_empty())
        .collect()
}

/// The lines as the core's `adjustments` (`[{account_id, amount}]`), or the message for a line with an
/// amount and no account.
pub(crate) fn adjustments(lines: &[FeeLine]) -> Result<Vec<crate::commands::DepositAdjustmentValues>, String> {
    lines
        .iter()
        .map(|l| {
            if l.account.is_empty() {
                Err("Choose an account for each fee or adjustment.".to_owned())
            } else {
                Ok(crate::commands::DepositAdjustmentValues { account_id: l.account.clone(), amount: l.amount.clone() })
            }
        })
        .collect()
}

/// The accounts a fee or adjustment may use (any active posting account outside the control accounts, as
/// for a bank-feed category), and the one a new line starts with (the company's bank-charges account).
pub(crate) fn choices(app: &mut App, c: CompanyId) -> AppResult<(Vec<Value>, Option<String>)> {
    let o = app.open_mut()?;
    let accounts = fb_core::bankfeeds::rule_accounts(&mut o.book, o.owner, c)?;
    let default = o.book.read(|q| fb_core::banking::fee_account(q, c))?.map(|a| a.to_db());
    Ok((accounts.iter().map(|a| json!({"pk": a.id.to_db(), "label": format!("{} · {}", a.code, a.name), "code": a.code, "expense": a.kind == Kind::Expense})).collect(), default))
}

/// Worker review 3b M1: a fee typed without its minus sign. A fee the bank or card processor kept is negative;
/// typed as a positive amount on an expense account (such as 6160 Bank Service Charges) it increases the
/// deposit and credits that expense, and neither Record deposit nor Receive payment has a bank amount that
/// would show it. Such lines are said before anything is saved, with what they will do; `again` is what
/// records them as typed (choose Save payment again, or Record deposit). Returns the warning and the
/// confirmation the page sends back as `fee_ok`, or None when no line is positive on an expense account.
/// A positive line on an income or other account is an adjustment and is not warned about.
pub(crate) fn positive_expense(app: &mut App, c: CompanyId, lines: &[FeeLine], again: &str) -> AppResult<Option<(String, String)>> {
    let o = app.open_mut()?;
    let mut found: Vec<(String, String, i128)> = vec![];
    for l in lines {
        let Some(cents) = positive_cents(&l.amount) else { continue };
        let Some(id) = AccountId::parse(&l.account) else { continue };
        let Some(a) = o.book.read(|q| fb_core::store::account(q, id))? else { continue };
        if a.company_id == c && a.kind == Kind::Expense {
            found.push((a.id.to_db(), a.label(), cents));
        }
    }
    if found.is_empty() {
        return Ok(None);
    }
    let total: i128 = found.iter().map(|(.., n)| n).sum();
    let named = found.iter().map(|(_, label, n)| format!("{} on {label}", cash(*n))).collect::<Vec<_>>();
    let warning = if let [(_, label, n)] = found.as_slice() {
        format!(
            "{} on {label} is positive, so it will increase the deposit by {} and credit {label}, an expense account. A fee the bank or card processor kept is negative, such as -{}. If the amount is right, {again}; otherwise put a minus sign before it.",
            cash(*n), cash(*n), cash(*n)
        )
    } else {
        let list = format!("{} and {}", named[..named.len() - 1].join(", "), named[named.len() - 1]);
        format!(
            "{list} are positive, so they will increase the deposit by {} and credit those expense accounts. A fee the bank or card processor kept is negative, such as -24.95. If the amounts are right, {again}; otherwise put a minus sign before each.",
            cash(total)
        )
    };
    let confirm = found.iter().map(|(id, _, n)| format!("{id}={}", cash(*n))).collect::<Vec<_>>().join(",");
    Ok(Some((warning, confirm)))
}
/// A fee line's amount when it is a positive amount the core takes (no minus sign, more than zero).
fn positive_cents(text: &str) -> Option<i128> {
    let text = text.trim();
    if text.starts_with('-') {
        return None;
    }
    let d = fb_core::calculations::unit_rate(&json!(text)).ok()?;
    fb_money::Money::from_decimal_exact(d).map(|m| m.cents()).filter(|n| *n > 0)
}

/// The lines to draw: what was posted, else one empty line on the bank-charges account.
pub(crate) fn drawn(lines: &[FeeLine], default: Option<&str>) -> Vec<Value> {
    if lines.is_empty() {
        return vec![json!({"account": default.unwrap_or(""), "amount": ""})];
    }
    lines.iter().map(|l| json!({"account": l.account, "amount": l.amount})).collect()
}
