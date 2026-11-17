//! Find (`/c/<company>/find/?q=`): the results page behind the title-bar search field. fb-core does the
//! searching (read-only, company-scoped, bounded); this page labels the results and links them. A result
//! is shown only when the signed-in person may open the page it links to, by the same check the request
//! path applies to that page (`authorize_path`), so search never reveals what they could not open.
use super::{Route, arg_company, get, shell};
use crate::identity::Permission;
use crate::session::{App, AppResult};
use fb_core::find::{self, Group, Hit, Kind, Terms, TermsError};
use fb_templates::nav::Current;
use serde_json::{Value, json};

/// Results of each kind on the page.
pub(super) const PER_KIND: usize = 10;

fn heading(kind: Kind) -> &'static str {
    match kind {
        Kind::Invoice => "Invoices",
        Kind::Estimate => "Estimates",
        Kind::SalesReceipt => "Sales receipts",
        Kind::CreditMemo => "Credit memos",
        Kind::Payment => "Payments received",
        Kind::CustomerDeposit => "Customer deposits",
        Kind::Bill => "Bills",
        Kind::BillPayment => "Bill payments",
        Kind::Check => "Checks & expenses",
        Kind::Deposit => "Deposits",
        Kind::Transfer => "Transfers",
        Kind::TaxPayment => "Sales tax payments",
        Kind::JournalEntry => "Journal entries",
        Kind::Customer => "Customers",
        Kind::Vendor => "Vendors",
        Kind::Account => "Accounts",
        Kind::Item => "Items & services",
    }
}

/// What the second number means, per kind.
fn alt_label(kind: Kind) -> &'static str {
    match kind {
        Kind::Check => "Check no.",
        Kind::Bill => "Ref.",
        Kind::BillPayment => "Ref.",
        Kind::JournalEntry => "Entry no.",
        _ => "",
    }
}

fn status_label(status: &str) -> &'static str {
    match status {
        "DRAFT" => "Draft",
        "OPEN" => "Open",
        "PARTIAL" => "Partly paid",
        "PAID" => "Paid",
        "VOID" => "Void",
        "RECORDED" => "Recorded",
        "ACCEPTED" => "Accepted",
        "DECLINED" => "Declined",
        "CANCELLED" => "Cancelled",
        "CONVERTED" => "Converted",
        "POSTED" => "Posted",
        "REVERSED" => "Reversed",
        "INACTIVE" => "Inactive",
        _ => "",
    }
}

fn account_kind(kind: &str) -> &'static str {
    match kind {
        "ASSET" => "Asset",
        "LIABILITY" => "Liability",
        "EQUITY" => "Equity",
        "INCOME" => "Income",
        "EXPENSE" => "Expense",
        _ => "",
    }
}

/// The page a result opens (from the route table, as the lists link them).
pub(crate) fn url(c: &str, hit: &Hit) -> String {
    let (route, kind) = match hit.kind {
        Kind::Invoice => ("invoices:detail", None),
        Kind::Estimate => ("sales:detail", None),
        Kind::SalesReceipt => ("sales_receipts:detail", None),
        Kind::CreditMemo => ("credits:detail", None),
        Kind::Payment => ("receipts:detail", None),
        Kind::CustomerDeposit => ("prepayments:detail", None),
        Kind::Bill => ("bills:detail", None),
        Kind::BillPayment => ("payments:detail", None),
        Kind::Check => ("checks:detail", None),
        Kind::Deposit => ("deposits:detail", None),
        Kind::Transfer => ("transfers:detail", None),
        Kind::TaxPayment => ("salestax:tax_payment", None),
        Kind::JournalEntry => ("journal", None),
        Kind::Customer => ("directory:detail", Some("customers")),
        Kind::Vendor => ("directory:detail", Some("vendors")),
        Kind::Item => ("directory:detail", Some("items")),
        Kind::Account => ("register", None),
    };
    let mut args = vec![c.to_owned()];
    args.extend(kind.map(str::to_owned));
    args.push(hit.id.clone());
    // Ids come from the book; one that cannot form an address is left out by `authorize_path`.
    fb_templates::django::reverse(route, &args).unwrap_or_default()
}

/// Each group of results with the address each result opens.
pub(crate) type Found = Vec<(Group, Vec<String>)>;

/// Search, then keep only results the viewer may open. `None` when the query is not searchable, with
/// the reason to show.
pub(crate) fn search(app: &mut App, c: fb_core::CompanyId, raw: &str, per_kind: usize) -> AppResult<Result<Found, &'static str>> {
    let terms = match Terms::parse(raw) {
        Ok(t) => t,
        Err(TermsError::Empty) => return Ok(Err("")),
        Err(TermsError::TooShort) => return Ok(Err("Type at least 2 characters, or a document number.")),
        Err(TermsError::TooLong) => return Ok(Err("Use at most 120 characters.")),
    };
    let open = app.open_mut()?;
    let groups = find::find(&mut open.book, open.owner, c, &terms, per_kind)?;
    let company = c.to_db();
    let mut out = Vec::new();
    for mut group in groups {
        let mut urls = Vec::new();
        let mut kept = Vec::new();
        for hit in group.hits {
            let target = url(&company, &hit);
            if app.authorize_path(&target, Permission::Read).is_ok() {
                urls.push(target);
                kept.push(hit);
            }
        }
        group.hits = kept;
        if !group.hits.is_empty() {
            out.push((group, urls));
        }
    }
    Ok(Ok(out))
}

pub(super) fn render(app: &mut App, route: &Route, params: &[(String, String)]) -> AppResult<(String, u16)> {
    let c = arg_company(route)?;
    let raw = get(params, "q").unwrap_or("").trim().to_owned();
    let base = shell(app, Some(c), &Current { route: "find", kind: None })?;
    let results = search(app, c, &raw, PER_KIND)?;
    // An exact document number with one match opens that document (the results page stays one step
    // back in history only if the person came to it some other way: this is a redirect).
    if let Ok(groups) = &results
        && get(params, "all").is_none()
    {
        let exact: Vec<&String> = groups
            .iter()
            .flat_map(|(g, urls)| g.hits.iter().zip(urls))
            .filter(|(h, _)| h.exact && h.kind.is_document())
            .map(|(_, u)| u)
            .collect();
        if let [only] = exact.as_slice() {
            return Ok(((*only).clone(), 303));
        }
    }
    let mut vm = serde_json::to_value(base).expect("shell");
    let (message, groups) = match results {
        Ok(groups) => ("", groups),
        Err(message) => (message, vec![]),
    };
    let total: usize = groups.iter().map(|(g, _)| g.hits.len()).sum();
    let groups: Vec<Value> = groups.iter().map(|(g, urls)| group_vm(g, urls)).collect();
    let all = vm.as_object_mut().expect("shell object");
    all.insert("find_query".into(), json!(raw));
    all.insert("searched".into(), json!(message.is_empty() && !raw.is_empty()));
    all.insert("message".into(), json!(message));
    all.insert("groups".into(), json!(groups));
    all.insert("total".into(), json!(total));
    all.insert("per_kind".into(), json!(PER_KIND));
    Ok((app.screens.render("ledger/find.html", &vm)?, 200))
}

fn group_vm(g: &Group, urls: &[String]) -> Value {
    let hits: Vec<Value> = g
        .hits
        .iter()
        .zip(urls)
        .map(|(h, url)| {
            let detail = if h.kind == Kind::Account { account_kind(&h.detail).to_owned() } else { h.detail.clone() };
            json!({
                "url": url,
                "number": h.number,
                "alt": h.alt,
                "alt_label": alt_label(h.kind),
                "name": h.name,
                "date": h.date,
                "amount": h.amount.map(|m| m.to_string()),
                "status": status_label(&h.status),
                "status_key": h.status.to_lowercase(),
                "detail": detail,
                "exact": h.exact,
            })
        })
        .collect();
    json!({
        "key": serde_json::to_value(g.kind).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default(),
        "heading": heading(g.kind),
        "document": g.kind.is_document(),
        "item": g.kind == Kind::Item,
        "account": g.kind == Kind::Account,
        "more": g.more,
        "hits": hits,
    })
}
