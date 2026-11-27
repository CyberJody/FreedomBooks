//! "Review transactions": downloaded bank transactions waiting to be recorded. Draft edits never write to
//! the ledger; every decision goes through `fb_core::bankfeeds`.
//!
//! A post from the page's script carries `inline=1` and gets back only what changed (the acted-on rows and
//! the status area) as fragments plus the form's next single-use token; the preload swaps them in place
//! (`electron/preload/preload.cjs`, `applyFragments`). Without it the whole page is re-rendered as before.
use super::*;
use std::collections::HashSet;

const PER_PAGE: usize = 50;
const MAX_BULK: usize = 100;
/// Old copies one "Ignore old copies" press handles.
const MAX_COPIES: usize = 500;

// --- review ----------------------------------------------------------------------------------------

/// `views._int`: a positive integer below 10**12, else None.
fn int(v: &str) -> Option<i64> {
    forms::python_integer(v).filter(|n| *n > 0 && *n < 1_000_000_000_000)
}
fn transaction_by_ordinal(
    tx: &(impl Query + ?Sized),
    c: CompanyId,
    ordinal: i64,
) -> Result<Option<bf::Item>, FbError> {
    let id: Option<String> = tx
        .query_row(
            "SELECT t.id FROM bankfeeds_transaction t JOIN bankfeeds_account f ON f.id=t.feed_account_id JOIN bankfeeds_connection c ON c.id=f.connection_id WHERE t.ordinal=?1 AND c.company_id=?2",
            params![ordinal, c.to_db()],
            |r| r.get(0),
        )
        .optional()?;
    id.map(|id| bf::item(tx, c, &id)).transpose()
}
/// The transaction id a posted pk names, or a value no transaction has (the service then refuses it
/// with its own "this company" message, as Python's does for an unknown pk).
fn target_id(app: &mut App, c: CompanyId, ordinal: i64) -> AppResult<String> {
    Ok(app
        .open_mut()?
        .book
        .read(|tx| transaction_by_ordinal(tx, c, ordinal))?
        .map(|i| i.id)
        .unwrap_or_default())
}
fn status_display(s: &str) -> &'static str {
    match s {
        "NEW" => "To review",
        "ACCEPTED" => "Accepted",
        "MATCHED" => "Matched",
        _ => "Ignored",
    }
}
fn kind_display(s: &str) -> &'static str {
    match s {
        "CHECK" => "Check",
        "DEPOSIT" => "Deposit",
        "TRANSFER" => "Transfer",
        "JOURNAL" => "Journal entry",
        "MATCH" => "Existing entry",
        _ => "",
    }
}
/// fb-core's `reference_url` route name (`checks:detail:<id>`) as the URL Django stored.
fn reference_url(c: CompanyId, stored: &str) -> String {
    let Some((route, id)) = stored.rsplit_once(':') else {
        return String::new();
    };
    fb_templates::django::reverse(route, &[c.to_db(), id.to_owned()]).unwrap_or_default()
}
// A stored "JE n" identifies a manual entry by its chosen number. Pair it with the ledger number.
fn display_reference(tx: &(impl Query + ?Sized), c: CompanyId, reference: &str, route: &str) -> Result<String, FbError> {
    if (reference == "JE" || reference.starts_with("JE ")) && let Some(id) = route.strip_prefix("journal:") {
        let number = tx.query_row(
            "SELECT printf('J%06d',number) FROM ledger_journal WHERE company_id=?1 AND id=?2 AND source_id IS NULL",
            params![c.to_db(), id], |r| r.get::<_, String>(0),
        ).optional()?;
        if let Some(number) = number { return Ok(bf::review_journal_number(reference.strip_prefix("JE ").unwrap_or(""), &number)); }
    }
    Ok(reference.to_owned())
}
const MISSING_CATEGORY: &str = "Choose a category for this transaction.";
fn designated(tx: &(impl Query + ?Sized), c: CompanyId) -> Result<Vec<String>, FbError> {
    let setup = documents::setup_snapshot(tx, c)?;
    Ok(setup["banks"]
        .as_array()
        .map(|b| {
            b.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default())
}

struct Posted1 {
    kind: Option<String>,
    account_id: Option<String>,
    payee: String,
    number: String,
    other_bank: String,
    line: String,
    selected: bool,
    error: String,
    /// Worker item 46: "Split" ticked, and the split lines as typed (account, amount, memo), in order.
    split: bool,
    lines: Vec<(String, String, String)>,
    /// Worker item 48: the received payments ticked for a deposit, and its fee or adjustment lines.
    receipts: Vec<String>,
    fees: Vec<crate::screens::fees::FeeLine>,
    /// Multi-entry match: the posted ledger lines ticked to match this bank transaction together.
    match_lines: Vec<String>,
}
/// Multi-entry match: the posted lines ticked for row `pk` (`match_line_<pk>`, repeated).
fn posted_match_lines(p: Posted<'_>, pk: i64) -> Vec<String> {
    let name = format!("match_line_{pk}");
    p.iter()
        .filter(|(k, _)| *k == name)
        .map(|(_, v)| py::py_strip(v).to_owned())
        .filter(|v| !v.is_empty())
        .take(500)
        .collect()
}
/// Worker item 48: the received payments a post ticks for row `pk` (`receipt_<pk>`, repeated).
fn posted_receipts(p: Posted<'_>, pk: i64) -> Vec<String> {
    let name = format!("receipt_{pk}");
    p.iter()
        .filter(|(k, _)| *k == name)
        .map(|(_, v)| py::py_strip(v).to_owned())
        .filter(|v| !v.is_empty())
        .take(51)
        .collect()
}

/// Worker item 46: the split lines a post carries for row `pk`, in page order: each line posts its account,
/// amount and memo (`split_account_<pk>`, `split_amount_<pk>`, `split_memo_<pk>`).
fn posted_lines(p: Posted<'_>, pk: i64) -> Vec<(String, String, String)> {
    let take = |name: String| p.iter().filter(|(k, _)| *k == name).map(|(_, v)| v.clone()).take(bf::MAX_SPLIT_LINES + 1).collect::<Vec<_>>();
    let (accounts, amounts, memos) = (take(format!("split_account_{pk}")), take(format!("split_amount_{pk}")), take(format!("split_memo_{pk}")));
    (0..accounts.len().max(amounts.len()).max(memos.len()))
        .map(|i| (accounts.get(i).cloned().unwrap_or_default(), amounts.get(i).cloned().unwrap_or_default(), memos.get(i).cloned().unwrap_or_default()))
        .collect()
}

#[derive(Default)]
pub(super) struct ReviewPost {
    posted: HashMap<i64, Posted1>,
    results: Vec<Value>,
    /// Rows this post acted on, in order: an in-place reply re-renders exactly these.
    touched: Vec<i64>,
    /// Accepted rows whose Undo asks for confirmation on this render.
    undo_confirm: HashSet<i64>,
    /// Worker items 40 and 42 (an in-place reply from the page's script): the rows the person typed a draft
    /// into. Only these keep `draft_retained`; `None` (a whole-page post) keeps every posted row as before.
    edited: Option<HashSet<i64>>,
    /// Rows drawn again only because the page shows them: drawn as a new page would draw them (their
    /// posted values were the page's own defaults), keeping only the selection tick.
    fresh: HashSet<i64>,
}

fn accept_from_post(
    app: &mut App,
    c: CompanyId,
    pk: i64,
    p: Posted<'_>,
    vendors: &HashMap<String, String>,
    record_again: Option<&str>,
    as_match: bool,
) -> AppResult<Result<bf::Item, FbError>> {
    let owner = actor(app)?;
    let id = target_id(app, c, pk)?;
    let field = |k: &str| get(p, &format!("{k}_{pk}")).unwrap_or("");
    let posted_id = |k: &str| {
        int(field(k)).map(|_| field(k).to_owned()).or_else(|| {
            // Rust ids are UUIDs where Python's are integers; `_int` refuses anything else.
            let v = py::py_strip(field(k));
            fb_ids::parse_uuid(v)
                .filter(|u| u.to_string() == v)
                .map(|_| v.to_owned())
        })
    };
    let payee = py::py_truncate(py::py_strip(field("payee")), 120);
    let vendor_id = if payee.is_empty() {
        None
    } else {
        vendors.get(&bf::text::py_lower(&payee)).cloned()
    };
    let kind = get(p, &format!("kind_{pk}")).unwrap_or("");
    let match_lines = posted_match_lines(p, pk);
    let mut input = json!({"kind": kind, "account_id": posted_id("account"), "payee_name": payee,
        "vendor_id": vendor_id, "check_number": py::py_truncate(py::py_strip(field("number")), 20), "journal_line_id": posted_id("line"),
        "receipt_ids": posted_receipts(p, pk), "transfer_account_id": posted_id("to")});
    if kind == "MATCH" && !match_lines.is_empty() {
        input["kind"] = json!("MATCH");
        input["journal_line_ids"] = json!(match_lines);
    } else if as_match {
        // Worker item 40: "Match" on a row that warned of an entry already on its account: that entry.
        // Worker review S1: with "Match existing entry" chosen on the row, the entry chosen in its "Existing
        // entry" list (the core checks it is still one Match may take).
        let chosen = if kind == "MATCH" { posted_id("line") } else { None };
        input["kind"] = json!("MATCH");
        input["journal_line_id"] = json!(chosen.or_else(|| get(p, &format!("link_to_{pk}")).filter(|v| !v.is_empty()).map(str::to_owned)));
    }
    // Worker items 47 and 48: a deposit row's fee and adjustment lines (a line with no amount is none).
    if kind == "DEPOSIT" {
        let fees = crate::screens::fees::posted_as(p, &format!("fee_account_{pk}"), &format!("fee_amount_{pk}"));
        match crate::screens::fees::adjustments(&fees) {
            Ok(a) if !a.is_empty() => input["adjustments"] = json!(a),
            Ok(_) => {}
            Err(message) => return Ok(Err(FbError::validation(message))),
        }
    }
    // Worker item 46: with "Split" ticked a check or journal row is recorded from its split lines; a line
    // left wholly blank is not a line.
    if checked(p, &format!("split_{pk}")) && ["CHECK", "JOURNAL"].contains(&kind) {
        let lines = posted_lines(p, pk)
            .into_iter()
            .filter(|(a, n, m)| ![a, n, m].iter().all(|v| py::py_strip(v).is_empty()))
            .map(|(account, amount, memo)| json!({"account_id": py::py_strip(&account), "amount": py::py_strip(&amount), "memo": py::py_truncate(py::py_strip(&memo), 160)}))
            .collect::<Vec<_>>();
        if lines.is_empty() {
            return Ok(Err(FbError::validation("Enter the split lines, or untick Split.")));
        }
        input["lines"] = json!(lines);
    }
    if !as_match && ["CHECK", "JOURNAL"].contains(&kind) && input["lines"].is_null() && input["account_id"].is_null() {
        return Ok(Err(FbError::validation(MISSING_CATEGORY)));
    }
    if let Some(seen) = record_again {
        // The user's explicit "Record again" on a row the page showed as already recorded as `seen` (the
        // earlier transaction): the core records it only if that is still the entry it would duplicate.
        input["record_again"] = json!(true);
        input["recorded_as"] = json!(seen);
    }
    Ok(bf::accept(
        &mut app.open_mut()?.book,
        owner,
        c,
        &id,
        &input,
        &bf::SecretSet::none(),
    ))
}

/// One row action of a post: what the user pressed, on which row.
#[derive(Clone, Copy, PartialEq)]
enum Act {
    Accept,
    AcceptAgain,
    /// Worker item 40: match the entry the row warned is already on its account.
    Match,
    Ignore,
    Return,
    Link,
    Undo,
    UndoConfirmed,
    Cancel,
    Keep,
    /// Ignore the never-reviewed rows of disconnected connections that a connected one downloaded again.
    IgnoreCopies,
}

pub(super) fn run_review_post(app: &mut App, c: CompanyId, p: Posted<'_>) -> AppResult<ReviewPost> {
    let owner = actor(app)?;
    let mut out = ReviewPost::default();
    // Retain every visible draft, including unselected rows, when one row fails or succeeds.
    for (key, _) in p
        .iter()
        .filter(|(k, _)| k.starts_with("kind_"))
        .take(PER_PAGE)
    {
        let Some(pk) = key.strip_prefix("kind_").and_then(int) else {
            continue;
        };
        let field = |k: &str| get(p, &format!("{k}_{pk}")).unwrap_or("").to_owned();
        out.posted.insert(
            pk,
            Posted1 {
                kind: Some(field("kind")),
                account_id: Some(field("account")),
                payee: field("payee"),
                number: field("number"),
                other_bank: field("to"),
                line: field("line"),
                selected: p.iter().any(|(k, v)| k == "sel" && int(v) == Some(pk)),
                error: String::new(),
                split: checked(p, &format!("split_{pk}")),
                lines: posted_lines(p, pk),
                receipts: posted_receipts(p, pk),
                fees: crate::screens::fees::posted_as(p, &format!("fee_account_{pk}"), &format!("fee_amount_{pk}")),
                match_lines: posted_match_lines(p, pk),
            },
        );
    }
    let vendors: HashMap<String, String> = app.open_mut()?.book.read(|tx| -> Result<_, FbError> {
        let mut st = tx.prepare("SELECT display_name,id FROM directory_vendor WHERE company_id=?1 AND active=1 ORDER BY id")?;
        let rows = st.query_map([c.to_db()], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows.into_iter().map(|(n, id)| (bf::text::py_lower(&n), id)).collect())
    })?;
    let truthy = |k: &str| get(p, k).filter(|v| !v.is_empty());
    let single = [
        ("accept_one", Act::Accept),
        ("accept_again_one", Act::AcceptAgain),
        ("match_one", Act::Match),
        ("ignore_one", Act::Ignore),
        ("return_one", Act::Return),
        ("link_one", Act::Link),
        ("undo_one", Act::Undo),
        ("undo_confirm_one", Act::UndoConfirmed),
        ("cancel_one", Act::Cancel),
        ("keep_one", Act::Keep),
        ("ignore_copies", Act::IgnoreCopies),
    ];
    let mut targets: Vec<(Act, Option<i64>)> = vec![];
    let mut bulk = false;
    if truthy("ignore_copies").is_some() {
        let copies = app.open_mut()?.book.read(|tx| retired_copies(tx, c))?;
        targets = copies.into_iter().take(MAX_COPIES).map(|i| (Act::IgnoreCopies, Some(i.ordinal))).collect();
        if targets.is_empty() {
            out.results.push(json!({"ok": false, "text": "No old copies are left to ignore."}));
        }
    } else if let Some((v, act)) = single.iter().find_map(|(name, act)| truthy(name).map(|v| (v, *act))) {
        targets.push((act, int(v)));
    } else if let Some(action) = get(p, "action").filter(|a| ["accept", "ignore"].contains(a)) {
        bulk = true;
        let mut found = vec![];
        for (_, v) in p.iter().filter(|(k, _)| k == "sel").take(MAX_BULK + 1) {
            if let Some(n) = int(v)
                && !found.contains(&n)
            {
                found.push(n);
            }
        }
        let act = if action == "accept" { Act::Accept } else { Act::Ignore };
        targets = found.into_iter().map(|n| (act, Some(n))).collect();
    }
    if targets.len() > MAX_BULK {
        out.results.push(
            json!({"ok": false, "text": format!("Choose at most {MAX_BULK} rows at a time.")}),
        );
        targets.clear();
    }
    if targets.is_empty() && out.results.is_empty() {
        out.results.push(
            json!({"ok": false, "text": "Tick the rows to accept or ignore, or use a row button."}),
        );
    }
    // Bulk accept never records a row that looks already recorded unless the user asked for it, and then
    // only a row this page showed as already recorded, as that same entry (its `link_to_<pk>`): a row that
    // became "already recorded" after the page was drawn is not recorded on the strength of the tick.
    let include_recorded = bulk && checked(p, "include_recorded");
    // Rows skipped as already recorded: from an earlier download (Link), and with an entry already on their
    // account (Match, worker item 40).
    let (mut skipped_earlier, mut skipped_on_account) = (0, 0);
    let mut newly_recorded = 0;
    // Worker review M1: rows set to record a new entry while an entry Match would take is on their account,
    // which the page did not show as recorded: each needs Match or Record again… on its own row.
    let mut needs_choice = 0;
    for (action, pk) in targets {
        let Some(pk) = pk else { continue };
        out.touched.push(pk);
        let current = app
            .open_mut()?
            .book
            .read(|tx| transaction_by_ordinal(tx, c, pk))?;
        let decides = matches!(action, Act::Accept | Act::AcceptAgain | Act::Match | Act::Ignore | Act::Link | Act::IgnoreCopies);
        if decides && current.as_ref().is_some_and(|i| i.status != "NEW") {
            let message = "This transaction is no longer new. No review edits were applied; refresh the review list.";
            if let Some(row) = out.posted.get_mut(&pk) {
                row.error = message.into();
            }
            out.results.push(json!({"ok": false, "text": message, "pk": pk}));
            continue;
        }
        let seen = get(p, &format!("link_to_{pk}")).filter(|v| !v.is_empty());
        let mut record_again = (action == Act::AcceptAgain).then(|| seen.unwrap_or(""));
        if bulk && action == Act::Accept && let Some(item) = &current {
            let kind = get(p, &format!("kind_{pk}")).unwrap_or("");
            let found = app.open_mut()?.book.read(|tx| recorded_key(tx, c, item, kind))?;
            let refuse = |out: &mut ReviewPost, message: String| {
                if let Some(row) = out.posted.get_mut(&pk) {
                    row.error = message;
                }
            };
            match &found {
                // Worker review M1: Match chosen (on a row that warned of an entry on its account, say) posts
                // nothing, and the core checks the entry it names: not a change since the page was shown.
                None if kind == "MATCH" => {}
                // A new entry while an entry Match would take is on the account, on a row the page did not
                // show as recorded (drawn with Match chosen and changed, or the entry is new): the person
                // decides on the row.
                Some(f) if f.on_account && seen.is_none() => {
                    needs_choice += 1;
                    refuse(&mut out, format!("{} is already on this account with the same amount. Choose Match or Record again… on this row. Nothing was recorded.", f.named));
                    continue;
                }
                // Shown one way, now the other: nothing is recorded on the strength of the page's tick.
                _ if seen != found.as_ref().map(|f| f.key.as_str()) => {
                    newly_recorded += 1;
                    refuse(&mut out, match &found {
                        Some(f) => format!("This now looks already recorded as {}. Nothing was recorded; review it again.", f.named),
                        None => "This no longer looks already recorded. Nothing was recorded; review it again.".to_owned(),
                    });
                    continue;
                }
                Some(f) if !include_recorded => {
                    if f.on_account { skipped_on_account += 1 } else { skipped_earlier += 1 }
                    continue;
                }
                Some(_) => record_again = seen,
                None => {}
            }
        }
        let id = target_id(app, c, pk)?;
        let none = bf::SecretSet::none();
        let result = match action {
            Act::Accept | Act::AcceptAgain => accept_from_post(app, c, pk, p, &vendors, record_again, false)?,
            Act::Match => accept_from_post(app, c, pk, p, &vendors, None, true)?,
            Act::Ignore | Act::IgnoreCopies => bf::ignore(&mut app.open_mut()?.book, owner, c, &id, &none),
            Act::Return => bf::return_to_review(&mut app.open_mut()?.book, owner, c, &id, &none),
            Act::Link => {
                let expected = get(p, &format!("link_to_{pk}")).filter(|v| !v.is_empty());
                bf::link_existing(&mut app.open_mut()?.book, owner, c, &id, expected, &none)
            }
            Act::UndoConfirmed => bf::undo_accept(&mut app.open_mut()?.book, owner, c, &id, &none),
            Act::Keep => bf::keep_recorded(&mut app.open_mut()?.book, owner, c, &id, &none)
                .and_then(|()| app.open_mut().map_err(|e| FbError::validation(e.to_string()))?.book.read(|tx| bf::item(tx, c, &id))),
            Act::Undo => {
                // First press: the row asks for confirmation; nothing is written.
                out.undo_confirm.insert(pk);
                continue;
            }
            Act::Cancel => continue,
        };
        match result {
            Ok(item) => {
                let head = format!(
                    "{} {} {}",
                    item.date().unwrap_or("None"),
                    py::py_truncate(&item.description, 60),
                    cents(item.amount)
                );
                let url = reference_url(c, &item.reference_url);
                // Worker item 46: say when a row was recorded from its split lines.
                let split = out.posted.get(&pk).filter(|x| x.split && ["CHECK", "JOURNAL"].contains(&item.accepted_as.as_str()));
                let lines = split.map(|x| x.lines.iter().filter(|(a, n, m)| ![a, n, m].iter().all(|v| py::py_strip(v).is_empty())).count()).unwrap_or(0);
                let split_note = if lines > 1 { format!(", split over {lines} lines") } else { String::new() };
                let reference = app.open_mut()?.book.read(|tx| display_reference(tx, c, &item.reference, &item.reference_url))?;
                out.results.push(match action {
                    Act::Accept | Act::AcceptAgain if item.status == "MATCHED" => json!({"ok": true, "text": format!("{head}: matched to {reference}"), "url": url}),
                    Act::Accept | Act::AcceptAgain => json!({"ok": true, "text": format!("{head}: recorded as {reference}{split_note}"), "url": url}),
                    Act::Match => json!({"ok": true, "text": format!("{head}: matched to {reference}"), "url": url}),
                    Act::Ignore => json!({"ok": true, "text": format!("{head}: ignored")}),
                    Act::IgnoreCopies => json!({"ok": true, "text": format!("{head}: old copy ignored")}),
                    Act::Link => json!({"ok": true, "text": format!("{head}: linked to the entry already in the books; nothing posted")}),
                    Act::UndoConfirmed => json!({"ok": true, "text": format!("{head}: undone; the entry was voided and the transaction is back in review")}),
                    Act::Keep => json!({"ok": true, "text": format!("{head}: kept as recorded")}),
                    _ => json!({"ok": true, "text": format!("{head}: back in review")}),
                });
            }
            Err(e) => {
                let message = messages(e)?;
                if let Some(row) = out.posted.get_mut(&pk) {
                    row.error = message.clone();
                }
                let text = if bulk && let Some(item) = &current {
                    format!(
                        "{} {} {}: {message}",
                        item.date().unwrap_or("None"),
                        py::py_truncate(&item.description, 60),
                        cents(item.amount)
                    )
                } else {
                    message
                };
                out.results
                    .push(json!({"ok": false, "text": text, "pk": pk}));
            }
        }
    }
    if newly_recorded > 0 {
        out.results.push(json!({"ok": false, "text": format!(
            "{newly_recorded} selected {} changed since this page was shown (whether {} already recorded), so nothing was recorded for {}. Review {} again.",
            if newly_recorded == 1 { "row" } else { "rows" }, if newly_recorded == 1 { "it looks" } else { "they look" }, if newly_recorded == 1 { "it" } else { "them" }, if newly_recorded == 1 { "it" } else { "them" })}));
    }
    let copies = out.results.iter().filter(|r| r["ok"] == true && r["text"].as_str().is_some_and(|t| t.ends_with(": old copy ignored"))).count();
    if copies > 1 {
        out.results.retain(|r| !(r["ok"] == true && r["text"].as_str().is_some_and(|t| t.ends_with(": old copy ignored"))));
        out.results.push(json!({"ok": true, "text": format!("{copies} old copies from a disconnected bank connection ignored; the new connection's copies stay to review.")}));
    }
    if needs_choice > 0 {
        let one = needs_choice == 1;
        out.results.push(json!({"ok": false, "text": format!(
            "{needs_choice} selected {} {} an entry already on {} account with the same amount, so nothing was recorded for {}. Choose Match or Record again… on {}.",
            if one { "row" } else { "rows" }, if one { "has" } else { "have" }, if one { "its" } else { "their" }, if one { "it" } else { "them" }, if one { "that row" } else { "each of them" })}));
    }
    let skipped_recorded = skipped_earlier + skipped_on_account;
    if skipped_recorded > 0 {
        // Worker review M3: a row warned of an entry on its account offers Match; one from an earlier
        // download offers Link.
        let one = skipped_recorded == 1;
        let verb = match (skipped_earlier > 0, skipped_on_account > 0) {
            (true, true) => "Match or link each one to its existing entry",
            (false, true) if one => "Match it to the existing entry",
            (false, true) => "Match them to the existing entries",
            _ if one => "Link it to the existing entry",
            _ => "Link them to the existing entry",
        };
        out.results.push(json!({"ok": false, "text": format!(
            "Skipped {skipped_recorded} selected {} that {} already recorded. {verb}, or tick “Also record rows that look already recorded”.",
            if one { "row" } else { "rows" }, if one { "looks" } else { "look" })}));
    }
    Ok(out)
}

/// What a row looks already recorded as (the page shows it as `link_to_<pk>` = `key`).
struct RecordedAs {
    key: String,
    /// The entry by its number and date, as the row names it.
    named: String,
    /// An entry already on the account (Match), not an earlier download's (Link).
    on_account: bool,
}
/// What a row looks already recorded as: the earlier download's entry ([`bf::already_recorded`]), else, for a
/// row set to record a new entry, the entry already on its account that Match would take (worker item 40,
/// [`bf::entry_on_account`]).
fn recorded_key(tx: &(impl Query + ?Sized), c: CompanyId, item: &bf::Item, kind: &str) -> Result<Option<RecordedAs>, FbError> {
    if let Some(r) = bf::already_recorded(tx, c, item)? {
        return Ok(Some(RecordedAs { named: format!("{} ({})", display_reference(tx, c, &r.reference, &r.reference_url)?, r.date), key: r.earlier_id, on_account: false }));
    }
    if kind == "MATCH" {
        return Ok(None);
    }
    Ok(bf::entry_on_account(tx, c, item)?.map(|m| RecordedAs { named: m.named(), key: m.line_id, on_account: true }))
}

struct Filter {
    status: String,
    account: Option<String>,
    q: String,
}

fn filtered_ids(tx: &(impl Query + ?Sized), c: CompanyId, filter: Option<&Filter>) -> Result<Vec<String>, FbError> {
    let mut items = vec![];
    if let Some(f) = filter {
        let mut st = tx.prepare("SELECT t.id,t.feed_account_id,t.status,t.description,EXISTS(SELECT 1 FROM bankfeeds_link k WHERE k.item_ordinal=t.ordinal) FROM bankfeeds_transaction t JOIN bankfeeds_account f ON f.id=t.feed_account_id JOIN bankfeeds_connection c ON c.id=f.connection_id WHERE c.company_id=?1 ORDER BY t.posted_on IS NOT NULL,t.posted_on DESC,t.ordinal DESC")?;
        let ids = st.query_map([c.to_db()], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, bool>(4)?)))?.collect::<Result<Vec<_>, _>>()?;
        let wanted = if f.status.is_empty() { "NEW" } else { f.status.as_str() };
        let needle = bf::text::py_lower(py::py_strip(&f.q));
        for (id, feed, st, description, linked) in ids {
            // Rows linked to an entry already in the books show with the matched ones, not as ignored.
            let shown = if linked { "MATCHED" } else { st.as_str() };
            if wanted != "ALL" && wanted != "ATTENTION" && shown != wanted {
                continue;
            }
            if f.account.as_ref().is_some_and(|a| *a != feed) {
                continue;
            }
            if !f.q.is_empty() && !bf::text::py_lower(&description).contains(&needle) {
                continue;
            }
            if wanted == "ATTENTION" && bf::stale_decision(tx, &bf::item(tx, c, &id)?)?.is_none() {
                continue;
            }
            items.push(id);
        }
    }
    Ok(items)
}

/// What every row of one render shares.
struct RowCtx {
    rules: Vec<bf::Rule>,
    categories: Vec<fb_core::model::Account>,
    banks: Vec<String>,
    transfer_accounts: Vec<fb_core::model::Account>,
    currency: String,
    owner: UserId,
    /// Worker item 47: the account a new fee line starts with (the company's bank-charges account).
    fee_default: Option<String>,
}
fn row_ctx(app: &mut App, c: CompanyId) -> AppResult<RowCtx> {
    let owner = actor(app)?;
    let rules = bf::active_rules(&mut app.open_mut()?.book, owner, c)?;
    let categories = bf::rule_accounts(&mut app.open_mut()?.book, owner, c)?;
    let (banks, transfer_accounts, currency) = app.open_mut()?.book.read(|tx| -> Result<_, FbError> {
        let banks = designated(tx, c)?;
        let eligible = fb_core::transfers::eligible_account_ids(tx, c)?;
        let transfer_accounts = store::accounts(tx, c)?.into_iter().filter(|a| eligible.contains(&a.id.to_db())).collect();
        let currency: String = tx.query_row("SELECT currency FROM ledger_company WHERE id=?1", [c.to_db()], |r| r.get(0))?;
        Ok((banks, transfer_accounts, currency))
    })?;
    let fee_default = app.open_mut()?.book.read(|q| fb_core::banking::fee_account(q, c))?.map(|a| a.to_db());
    Ok(RowCtx { rules, categories, banks, transfer_accounts, currency, owner, fee_default })
}

/// One review row's view model (the `row` of `bankfeeds/review_row.html`).
fn row_vm(tx: &(impl Query + ?Sized), c: CompanyId, ctx: &RowCtx, item: &bf::Item, post: &ReviewPost) -> Result<Value, FbError> {
    let feed = bf::feed(tx, &item.feed_account_id)?.ok_or_else(|| FbError::validation("Choose a downloaded transaction of this company."))?;
    let ledger = feed.ledger_account_id.as_deref().and_then(AccountId::parse).map(|id| store::account(tx, id)).transpose()?.flatten();
    let mut iv = json!({"pk": item.ordinal, "status": item.status, "status_display": status_display(&item.status), "pending": item.pending,
        "date": item.date().unwrap_or(""), "description": item.description, "amount": cents(item.amount), "money_in": item.money_in(),
        "remote_id": item.remote_id, "reference": display_reference(tx, c, &item.reference, &item.reference_url)?, "reference_url": reference_url(c, &item.reference_url), "accepted_as": item.accepted_as,
        "accepted_as_display": kind_display(&item.accepted_as),
        "feed_account": {"name": feed.name, "ledger_account": ledger.as_ref().map(|a| json!({"code": a.code, "name": a.name}))}});
    if item.status != "NEW" {
        let reversed = match &item.journal_id {
            Some(j) => tx.query_row("SELECT EXISTS(SELECT 1 FROM ledger_journal WHERE reversal_of_id=?1)", [j], |r| r.get::<_, bool>(0))?,
            None => false,
        };
        // A row linked to an entry already in the books ("Link to existing"): ignored, nothing posted.
        let linked: Option<(String, String, bool)> = tx
            .query_row(
                "SELECT e.reference,e.reference_url,e.status IN('ACCEPTED','MATCHED') AND NOT EXISTS(SELECT 1 FROM ledger_journal r WHERE r.reversal_of_id=e.journal_id)
                   FROM bankfeeds_link k JOIN bankfeeds_transaction e ON e.ordinal=k.earlier_ordinal WHERE k.item_ordinal=?1",
                [item.ordinal],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let linked = linked.map(|(reference, url, live)| -> Result<_, FbError> {
            Ok(json!({"reference":display_reference(tx, c, &reference, &url)?, "url":reference_url(c, &url), "live":live}))
        }).transpose()?;
        if linked.is_some() {
            iv["status_display"] = json!("Already recorded");
        }
        let change: Option<i64> =
            tx.query_row("SELECT amount FROM bankfeeds_bank_change WHERE item_ordinal=?1 AND kept=0", [item.ordinal], |r| r.get(0)).optional()?;
        let (undo, undo_blocked) = match bf::undo_status(tx, ctx.owner, c, item)? {
            Some(Ok(plan)) => (Some(plan), None),
            Some(Err(reason)) => (None, Some(reason)),
            None => (None, None),
        };
        let return_blocked = bf::return_blocker(tx, c, item)?;
        let matched_entries = if item.status == "MATCHED" {
            let entries = bf::matched_entries(tx, c, item)?;
            if entries.len() > 1 {
                let mut out = Vec::with_capacity(entries.len());
                for m in entries {
                    out.push(json!({
                        "reference": display_reference(tx, c, &m.reference, &m.reference_url)?,
                        "url": reference_url(c, &m.reference_url),
                    }));
                }
                out
            } else {
                vec![]
            }
        } else {
            vec![]
        };
        return Ok(json!({"item": iv, "suggestion": null, "reversed": reversed,
            "linked": linked,
            "matched_entries": matched_entries,
            "bank_change": change.map(cents),
            "undo": undo.map(|u| json!({"reference": iv["reference"], "date": u.date, "kind": kind_display(&u.kind).to_lowercase()})),
            "undo_blocked": undo_blocked,
            "undo_manual_void": return_blocked.is_none(),
            "return_blocked": return_blocked,
            "stale": bf::stale_decision(tx, item)?,
            "retired": bf::retired_connection(tx, item)?,
            "undo_confirm": post.undo_confirm.contains(&item.ordinal)}));
    }
    let s = bf::suggest_for(tx, c, item, Some(&ctx.rules))?;
    let automatic_note: Option<String> = tx.query_row("SELECT reason FROM bankfeeds_rule_review WHERE item_id=?1", [&item.id], |r| r.get(0)).optional()?;
    iv["automatic_note"] = json!(automatic_note);
    let mut recorded = bf::already_recorded(tx, c, item)?;
    if let Some(r) = &mut recorded {
        r.reference = display_reference(tx, c, &r.reference, &r.reference_url)?;
    }
    // A never-reviewed row of a disconnected connection: it can only be ignored. Say whether the connected
    // one downloaded it again, so the user knows the books will still get it.
    let retired = bf::retired_connection(tx, item)?
        .map(|label| -> Result<Value, FbError> {
            let again = bf::downloaded_again(tx, c, item)?;
            Ok(json!({"label": label, "again": again.map(|a| json!({"date": a.date(), "status": status_display(&a.status), "account": bf::feed(tx, &a.feed_account_id).ok().flatten().map(|f| f.name)}))}))
        })
        .transpose()?;
    let is_designated = feed.ledger_account_id.as_ref().is_some_and(|a| ctx.banks.contains(a));
    let transfer_source = ctx.transfer_accounts.iter().find(|a| Some(a.id.to_db()) == feed.ledger_account_id);
    let mut other = Vec::new();
    if let Some(mapped) = transfer_source {
        for a in ctx.transfer_accounts.iter().filter(|a| a.id != mapped.id) {
            let (from, to) = if item.money_in() { (a, mapped) } else { (mapped, a) };
            other.push((a, fb_core::transfers::purpose(from, to)));
        }
    }
    let all = post.posted.get(&item.ordinal);
    let posted = all.filter(|_| !post.fresh.contains(&item.ordinal));
    let has_posted_multi = posted.is_some_and(|x| !x.match_lines.is_empty());
    let abs_amount = item.amount.checked_abs().unwrap_or(0);
    let partial_matches = bf::multi_match_candidates(tx, c, item, bf::MULTI_MATCH_POOL_LIMIT)?;
    let partial_sum: i128 = partial_matches.iter().map(|m| i128::from(m.amount)).sum();
    let can_multi_match = (partial_matches.len() >= 2 && partial_sum >= i128::from(abs_amount)) || has_posted_multi;
    let mut options: Vec<(&str, &str)> = vec![];
    if !s.matches.is_empty() {
        options.push(("MATCH", "Match existing entry"));
    } else if can_multi_match {
        options.push(("MATCH", "Match existing entries"));
    }
    // Worker item 48: any undeposited payment in the date window can go into this deposit (several
    // payments, or a payout net of a fee), not only one of the same amount.
    let deposit = if item.money_in() && is_designated { bf::deposit_candidates(tx, c, item, 50)? } else { vec![] };
    if !deposit.is_empty() {
        options.push(("DEPOSIT", "Deposit received payments"));
    }
    if item.money_in() {
        options.push(("JOURNAL", if is_designated { "Deposit (journal)" } else { "Payment (journal)" }));
    } else {
        if is_designated {
            options.push(("CHECK", "Check or debit"));
        }
        options.push(("JOURNAL", if !is_designated { "Charge (journal)" } else { "Journal entry" }));
    }
    if !other.is_empty() {
        options.push(("TRANSFER", "Transfer"));
    }
    let default_kind = if !item.money_in() && is_designated { "CHECK" } else { "JOURNAL" };
    let preferred = if s.kind.is_empty() { default_kind } else { s.kind.as_str() };
    options.sort_by_key(|o| o.0 != preferred);
    let kind = posted.and_then(|x| x.kind.clone()).unwrap_or_else(|| options.first().map(|o| o.0.to_owned()).unwrap_or_default());
    let account_id = posted.and_then(|x| x.account_id.clone()).or_else(|| s.account_id.clone());
    let payee = posted.map(|x| x.payee.clone()).unwrap_or_else(|| s.payee_name.clone());
    // Only a real check number from the bank's text ("CHECK 1041") is filled in. Left blank, a check
    // recorded from a debit still gets its `EFT-<n>` number from the core, as before.
    let number = posted.map(|x| x.number.clone()).unwrap_or_else(|| if item.money_in() { String::new() } else { bf::text::check_number_in(&item.description).unwrap_or_default() });
    // Keep unavailable submitted choices visible instead of silently selecting a new value.
    if !options.iter().any(|(value, _)| *value == kind) {
        options.push((&kind, "Unavailable choice — choose again"));
    }
    let category_valid = account_id.as_ref().is_none_or(|id| id.is_empty() || ctx.categories.iter().any(|a| a.id.to_db() == *id && Some(id) != feed.ledger_account_id.as_ref()));
    // Worker item 40: set to record a new entry while an entry with this amount is already on the account
    // (what Match offers): say which, offer Match, and Record again… to go ahead.
    let on_account = if recorded.is_none() && kind != "MATCH" { s.matches.first() } else { None };
    let bulk_block = if retired.is_some() {
        "From a disconnected bank connection: ignore it"
    } else if !feed.active || ledger.as_ref().is_none_or(|a| !a.active || !a.postable) {
        "Map to an active posting account first"
    } else if item.pending { "Still pending at the bank" }
    else if item.date().is_none() { "Missing transaction date" }
    else if item.amount == 0 { "Zero amount cannot be accepted" }
    else if feed.currency.to_uppercase() != ctx.currency { "Account currency differs from the company" }
    else { "" };
    Ok(json!({
        "item": iv,
        "suggestion": {
            // Worker review M2: each entry by its number and date, its memo after them.
            "matches": s.matches.iter().map(|m| json!({"label": m.label, "number": m.display_number(), "memo": m.memo(), "date": m.date, "line": {"pk": m.line_id}})).collect::<Vec<_>>(),
            "receipts": s.receipts.iter().map(|x| json!({"label": x.label, "date": x.date, "receipt": {"pk": x.receipt_id}})).collect::<Vec<_>>(),
            "rule": s.rule.as_ref().map(|x| json!({"name": x.name, "pk": x.id})),
        },
        "recorded": recorded.map(|r| json!({"reference": r.reference, "url": reference_url(c, &r.reference_url), "date": r.date, "earlier": r.earlier_id, "same_identity": r.same_identity, "same_account": r.same_account})),
        "on_account": on_account.map(|m| json!({"number": m.display_number(), "memo": m.memo(), "date": m.date, "line": m.line_id})),
        "retired": retired,
        "options": options,
        "kind": kind,
        "category_valid": category_valid,
        "draft_retained": if posted.is_some() && post.edited.as_ref().is_none_or(|e| e.contains(&item.ordinal)) { "true" } else { "false" },
        "selected": all.is_some_and(|x| x.selected),
        "other_bank_valid": posted.is_none_or(|x| x.other_bank.is_empty() || other.iter().any(|(a, _)| a.id.to_db() == x.other_bank)),
        "line_valid": posted.is_none_or(|x| x.line.is_empty() || s.matches.iter().any(|m| m.line_id == x.line)),
        "multi_match": {
            "amount": cents(abs_amount),
            "available": can_multi_match,
            "candidates": partial_matches.iter().map(|m| {
                let ticked = posted.is_some_and(|x| x.match_lines.contains(&m.line_id));
                json!({"pk": m.line_id, "number": m.display_number(), "memo": m.memo(), "date": m.date, "amount": cents(m.amount), "checked": ticked})
            }).collect::<Vec<_>>(),
            "missing": posted.map(|x| x.match_lines.iter().filter(|id| !partial_matches.iter().any(|m| m.line_id == **id)).cloned().collect::<Vec<_>>()).unwrap_or_default(),
            "open": posted.is_some_and(|x| !x.match_lines.is_empty()),
        },
        // Worker item 48: the payments a deposit row may take, ticked as posted, else the one of the same
        // amount the suggestion found; and its fee or adjustment lines.
        "deposit": {
            "amount": cents(item.amount.checked_abs().unwrap_or(0)),
            "candidates": deposit.iter().map(|r| {
                let ticked = match posted { Some(x) => x.receipts.contains(&r.receipt_id), None => s.receipts.first().is_some_and(|f| f.receipt_id == r.receipt_id) };
                json!({"pk": r.receipt_id, "label": r.label, "date": r.date, "amount": cents(r.amount), "checked": ticked})
            }).collect::<Vec<_>>(),
            "missing": posted.map(|x| x.receipts.iter().filter(|id| !deposit.iter().any(|r| r.receipt_id == **id)).cloned().collect::<Vec<_>>()).unwrap_or_default(),
            "fee_lines": crate::screens::fees::drawn(posted.map(|x| x.fees.as_slice()).unwrap_or(&[]), ctx.fee_default.as_deref()),
            "fee_default": ctx.fee_default,
        },
        "other_bank": posted.map(|x| x.other_bank.clone()).unwrap_or_default(),
        "line": posted.map(|x| x.line.clone()).unwrap_or_else(|| s.matches.first().map(|m| m.line_id.clone()).unwrap_or_default()),
        "bulk_block": bulk_block,
        "matching_rules": ctx.rules.iter().filter(|rule| bf::rule_for(std::slice::from_ref(*rule), item).is_some()).map(|r| r.id.clone()).collect::<Vec<_>>().join(" "),
        "account_id": account_id,
        "payee": payee,
        "number": number,
        "categories": ctx.categories.iter().filter(|a| Some(a.id.to_db()) != feed.ledger_account_id).map(|a| json!({"pk": a.id.to_db(), "code": a.code, "name": a.name})).collect::<Vec<_>>(),
        "other_banks": other.iter().map(|(a, purpose)| json!({"pk": a.id.to_db(), "code": a.code, "name": a.name, "purpose": purpose})).collect::<Vec<_>>(),
        "error": posted.map(|x| x.error.clone()),
        "category_error": posted.is_some_and(|x| x.error == MISSING_CATEGORY),
        // Worker item 46: the split, as posted (drawn only for a row the person split, so a page of rows
        // does not carry a second set of account lists per row; the page script builds new lines).
        "split": posted.is_some_and(|x| x.split),
        "split_amount": cents(item.amount.checked_abs().unwrap_or(0)),
        "split_lines": posted.filter(|x| x.split).map(|x| x.lines.iter().map(|(account, amount, memo)| json!({"account": account, "amount": amount, "memo": memo,
            "account_valid": account.is_empty() || ctx.categories.iter().any(|a| a.id.to_db() == *account && Some(account) != feed.ledger_account_id.as_ref())})).collect::<Vec<_>>()).unwrap_or_default(),
    }))
}

/// The status area above the table (counts, notices, results), also sent as a fragment.
fn status_vm(tx: &(impl Query + ?Sized), c: CompanyId, vm: &mut Value, results: &[Value]) -> Result<(), FbError> {
    let unmapped: i64 = tx.query_row("SELECT count(DISTINCT f.id) FROM bankfeeds_account f JOIN bankfeeds_connection c ON c.id=f.connection_id JOIN bankfeeds_transaction t ON t.feed_account_id=f.id WHERE c.company_id=?1 AND c.active=1 AND f.active=1 AND f.ledger_account_id IS NULL AND t.status='NEW'", [c.to_db()], |r| r.get(0))?;
    let mut st = tx.prepare("SELECT t.reference,t.reference_url FROM bankfeeds_transaction t
        JOIN bankfeeds_account f ON f.id=t.feed_account_id JOIN bankfeeds_connection c ON c.id=f.connection_id
        WHERE c.company_id=?1 AND t.status IN ('ACCEPTED','MATCHED') AND t.reference_url<>'' ORDER BY t.decided_at DESC,t.ordinal DESC LIMIT 5")?;
    let mut recent = vec![];
    for row in st.query_map([c.to_db()], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (reference, route) = row?;
        recent.push(json!({"label":display_reference(tx, c, &reference, &route)?, "url":reference_url(c, &route)}));
    }
    put(vm, "recent_outcomes", recent);
    put(vm, "unmapped", unmapped);
    // Worker item 43: the left menu's count, which the page script updates from an in-place reply.
    put(vm, "to_review", bf::review_count(tx, c)?);
    put(vm, "recorded_count", bf::already_recorded_count(tx, c)?);
    let mut changes = vec![];
    for b in bf::bank_changes(tx, c)? {
        // A linked row has no entry of its own: the entry it relies on is the earlier transaction's.
        let linked: Option<(String, String)> = tx
            .query_row(
                "SELECT e.reference,e.reference_url FROM bankfeeds_link k JOIN bankfeeds_transaction e ON e.ordinal=k.earlier_ordinal WHERE k.item_ordinal=?1",
                [b.ordinal],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (reference, url) = linked.clone().unwrap_or((b.reference.clone(), b.reference_url.clone()));
        changes.push(json!({"pk": b.ordinal, "date": b.date, "description": b.description, "recorded": cents(b.recorded), "bank": cents(b.bank),
            "reference": display_reference(tx, c, &reference, &url)?, "url": reference_url(c, &url), "linked": linked.is_some()}));
    }
    put(vm, "bank_changes", changes);
    put(vm, "results", results);
    // Never-reviewed rows of disconnected connections, and how many of them a connected one downloaded again.
    let retired: i64 = tx.query_row(
        "SELECT count(*) FROM bankfeeds_transaction t JOIN bankfeeds_account f ON f.id=t.feed_account_id JOIN bankfeeds_connection c ON c.id=f.connection_id
          WHERE c.company_id=?1 AND c.active=0 AND (f.ledger_account_id IS NULL OR f.active=0) AND t.status='NEW'",
        [c.to_db()],
        |r| r.get(0),
    )?;
    put(vm, "retired_count", retired);
    put(vm, "retired_copies", if retired > 0 { retired_copies(tx, c)?.len() } else { 0 });
    put(vm, "stale_count", bf::stale_decisions(tx, c)?.len());
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
    put(vm, "clock_warning", bf::clock_warning(tx, c, &now)?.map(|w| json!({"recorded": w.recorded, "now": w.now})));
    Ok(())
}
/// Never-reviewed rows of disconnected connections that a connected connection downloaded again: safe to
/// ignore, since the books get the transaction through the new copy.
fn retired_copies(tx: &(impl Query + ?Sized), c: CompanyId) -> Result<Vec<bf::Item>, FbError> {
    let mut st = tx.prepare(
        "SELECT t.id FROM bankfeeds_transaction t JOIN bankfeeds_account f ON f.id=t.feed_account_id JOIN bankfeeds_connection c ON c.id=f.connection_id
          WHERE c.company_id=?1 AND c.active=0 AND (f.ledger_account_id IS NULL OR f.active=0) AND t.status='NEW' ORDER BY t.ordinal",
    )?;
    let ids = st.query_map([c.to_db()], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
    let mut out = vec![];
    for id in ids {
        let item = bf::item(tx, c, &id)?;
        if bf::downloaded_again(tx, c, &item)?.is_some() {
            out.push(item);
        }
    }
    Ok(out)
}
/// Categories the company used most recently on accepted downloads, newest first ("recently used first").
fn recent_categories(tx: &(impl Query + ?Sized), c: CompanyId) -> Result<Vec<String>, FbError> {
    let mut st = tx.prepare(
        "SELECT l.account_id FROM bankfeeds_transaction t JOIN bankfeeds_account f ON f.id=t.feed_account_id JOIN bankfeeds_connection c ON c.id=f.connection_id
          JOIN ledger_journalline l ON l.journal_id=t.journal_id AND l.account_id IS NOT f.ledger_account_id
          WHERE c.company_id=?1 AND t.status='ACCEPTED' GROUP BY l.account_id ORDER BY max(t.decided_at) DESC LIMIT 8",
    )?;
    Ok(st.query_map([c.to_db()], |r| r.get(0))?.collect::<Result<Vec<_>, _>>()?)
}

/// An in-place reply: the acted-on rows and the status area as fragments, and the form's next token.
/// Worker item 40: with the page's list of shown rows (`shown`) every one of them is drawn again, since an
/// action can change another row's choices (the card side of a card payment now matches the entry the
/// checking side just recorded). A row the person typed a draft into (`edited`) keeps it; any other row is
/// drawn as a new page would draw it.
pub(super) fn review_fragments(app: &mut App, r: &Route, path: &str, p: Posted<'_>, mut post: ReviewPost) -> AppResult<Outcome> {
    let c = arg_company(r)?;
    let ctx = row_ctx(app, c)?;
    let mut vm = base(app, r, path)?;
    let editable = get(p, "review_layout") == Some("new");
    put(&mut vm, "review_editable", editable);
    let list = |name: &str| -> Option<Vec<i64>> { get(p, name).map(|v| v.split(',').filter_map(|s| int(s.trim())).take(PER_PAGE * 2).collect()) };
    let mut order = post.touched.clone();
    if let Some(edited) = list("edited") {
        let edited: HashSet<i64> = edited.into_iter().collect();
        post.fresh = post.posted.keys().copied().filter(|pk| !edited.contains(pk) && !post.touched.contains(pk)).collect();
        post.edited = Some(edited);
    }
    for pk in list("shown").unwrap_or_default() {
        if !order.contains(&pk) {
            order.push(pk);
        }
    }
    let (rows, ()) = app.open_mut()?.book.read(|tx| -> Result<_, FbError> {
        let mut rows = vec![];
        for pk in &order {
            if let Some(item) = transaction_by_ordinal(tx, c, *pk)? {
                rows.push((*pk, row_vm(tx, c, &ctx, &item, &post)?));
            }
        }
        status_vm(tx, c, &mut vm, &post.results)?;
        let filter = Filter {
            status: get(p, "review_status").unwrap_or("").into(),
            account: get(p, "review_account").filter(|s| !s.is_empty()).map(str::to_owned),
            q: get(p, "review_q").unwrap_or("").into(),
        };
        let count = filtered_ids(tx, c, Some(&filter))?.len();
        put(&mut vm, "filtered_count", count);
        put(&mut vm, "pending_filter", filter.status.is_empty());
        Ok((rows, ()))
    })?;
    let mut fragments = vec![json!({"id": "review-status", "html": draw(app, "bankfeeds/review_status.html", &vm)?})];
    let mut seen = HashSet::new();
    for (pk, row) in rows {
        if !seen.insert(pk) {
            continue;
        }
        put(&mut vm, "row", row);
        fragments.push(json!({"id": format!("review-row-{pk}"), "html": draw(app, "bankfeeds/review_row.html", &vm)?}));
    }
    Ok(Outcome::Done { data: json!({"fragments": fragments, "next_token": app.tokens.mint(path), "touched": post.touched}) })
}

pub(super) fn review_page(
    app: &mut App,
    r: &Route,
    path: &str,
    full_path: &str,
    params: Posted<'_>,
    post: Option<ReviewPost>,
) -> AppResult<(String, u16)> {
    let c = arg_company(r)?;
    let mut status = 200;
    let post = post.unwrap_or_default();
    if post.results.iter().any(|x| x["ok"] == false) {
        status = 400;
    }
    // ReviewFilterForm(request.GET): always bound.
    let feed_choices: Vec<(String, String)> = app.open_mut()?.book.read(|tx| -> Result<_, FbError> {
        let mut st = tx.prepare("SELECT f.id,f.org_name,f.name,c.label FROM bankfeeds_account f JOIN bankfeeds_connection c ON c.id=f.connection_id WHERE c.company_id=?1 ORDER BY f.org_name COLLATE fb_text,f.name COLLATE fb_text,f.first_seen_at,f.id")?;
        let rows = st
            .query_map([c.to_db()], |r| {
                let (org, name, label): (String, String, String) = (r.get(1)?, r.get(2)?, r.get(3)?);
                Ok((r.get::<_, String>(0)?, format!("{} ({label})", if org.is_empty() { name } else { format!("{org} · {name}") })))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })?;
    let status_choices: Vec<(String, String)> = [
        ("", "To review"),
        ("ACCEPTED", "Accepted"),
        ("MATCHED", "Matched or linked"),
        ("IGNORED", "Ignored"),
        ("ATTENTION", "Pointing at a voided entry"),
        ("ALL", "Everything"),
    ]
    .iter()
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .collect();
    let mut e = Errors::new();
    let raw_status = get(params, "status").unwrap_or("").to_owned();
    if !raw_status.is_empty() && !status_choices.iter().any(|(v, _)| *v == raw_status) {
        error(
            &mut e,
            "status",
            format!("Select a valid choice. {raw_status} is not one of the available choices."),
        );
    }
    let account = model_choice(params, "account", &feed_choices, false, &mut e);
    let q = char_field(params, "q", 120, false, &mut e);
    let valid = e.is_empty();
    let filter = valid.then(|| Filter {
        status: raw_status.clone(),
        account: account.clone(),
        q: q.clone(),
    });
    if !valid {
        status = 400;
    }
    let mut account_choices = vec![(String::new(), "All accounts".to_owned())];
    account_choices.extend(feed_choices);
    let fields = vec![
        Field::new(
            "status",
            "Show",
            Widget::Select {
                choices: status_choices,
            },
        )
        .value_opt(get(params, "status").map(str::to_owned)),
        Field::new(
            "account",
            "Bank account",
            Widget::Select {
                choices: account_choices,
            },
        )
        .value_opt(get(params, "account").map(str::to_owned)),
        Field::new("q", "Description contains", Widget::Text)
            .attr("maxlength", "120")
            .value_opt(get(params, "q").map(str::to_owned)),
    ];
    let filter_form = forms::bind(&bind_errors(fields, &e), &[]);

    let ctx = row_ctx(app, c)?;
    let mut vm = base(app, r, path)?;
    let (rows, page, vendor_names, recent) = app.open_mut()?.book.read(|tx| -> Result<_, FbError> {
        let items = filtered_ids(tx, c, filter.as_ref())?;
        // Paginator(PER_PAGE).get_page(request.GET.get('page')).
        let count = items.len();
        let pages = count.div_ceil(PER_PAGE).max(1);
        let number = match get(params, "page").and_then(forms::python_integer) {
            Some(n) if n >= 1 => (n as usize).min(pages),
            Some(_) => pages,
            None => 1,
        };
        let mut rows = vec![];
        for id in items.iter().skip((number - 1) * PER_PAGE).take(PER_PAGE) {
            rows.push(row_vm(tx, c, &ctx, &bf::item(tx, c, id)?, &post)?);
        }
        let page = json!({"number": number, "has_previous": number > 1, "has_next": number < pages, "previous_page_number": number.saturating_sub(1),
            "next_page_number": number + 1, "paginator": {"num_pages": pages, "count": count}});
        status_vm(tx, c, &mut vm, &post.results)?;
        put(&mut vm, "filtered_count", count);
        put(&mut vm, "pending_filter", raw_status.is_empty());
        let mut st = tx.prepare("SELECT display_name FROM directory_vendor WHERE company_id=?1 AND active=1 ORDER BY display_name COLLATE fb_text,id LIMIT 500")?;
        let vendor_names = st.query_map([c.to_db()], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
        Ok((rows, page, vendor_names, recent_categories(tx, c)?))
    })?;
    let query: Vec<(String, String)> = params
        .iter()
        .filter(|(k, _)| k != "page")
        .cloned()
        .collect();
    put(
        &mut vm,
        "review_filters",
        ["status", "account", "q", "page"].map(
            |key| json!({"name": format!("review_{key}"), "value": get(params, key).unwrap_or("")}),
        ),
    );
    put(
        &mut vm,
        "bulk_categories",
        ctx.categories
            .iter()
            .map(|a| json!({"pk": a.id.to_db(), "label": account_label(a)}))
            .collect::<Vec<_>>(),
    );
    put(&mut vm, "bulk_rules", ctx.rules.iter().map(|r| json!({"pk": r.id, "name": r.name, "account": r.account_id, "payee": r.payee_name})).collect::<Vec<_>>());
    put(&mut vm, "filter_form", filter_form);
    put(&mut vm, "page", page);
    put(
        &mut vm,
        "review_editable",
        rows.iter().any(|r| r["item"]["status"] == "NEW"),
    );
    put(&mut vm, "rows", rows);
    put(&mut vm, "recent_categories", recent.join(" "));
    put(&mut vm, "query_string", urlencode(&query));
    put(&mut vm, "vendor_names", vendor_names);
    put(
        &mut vm,
        "showing",
        if valid && !raw_status.is_empty() {
            raw_status
        } else {
            "NEW".into()
        },
    );
    put(&mut vm, "full_path", full_path);
    Ok((draw(app, "bankfeeds/review.html", &vm)?, status))
}

/// Prefill the existing rule form from a company-owned review row or its reviewed values.
pub(super) fn rule_initial(
    app: &mut App,
    c: CompanyId,
    params: Posted<'_>,
    choices: &[(String, String)],
) -> AppResult<HashMap<&'static str, Option<String>>> {
    let mut initial = HashMap::new();
    if let Some(from) = get(params, "from").filter(|v| !v.is_empty()) {
        let ordinal = int(from).unwrap_or(0);
        let source = app
            .open_mut()?
            .book
            .read(|tx| transaction_by_ordinal(tx, c, ordinal))?;
        if let Some(item) = source {
            let d = &item.description;
            initial.insert("feed_account", Some(item.feed_account_id.clone()));
            initial.insert(
                "name",
                Some(if d.is_empty() {
                    "New rule".into()
                } else {
                    py::py_truncate(d, 60)
                }),
            );
            initial.insert("match_text", Some(py::py_truncate(d, 200)));
            initial.insert(
                "sign",
                Some(if item.money_in() { "IN" } else { "OUT" }.into()),
            );
            initial.insert(
                "payee_name",
                Some(py::py_truncate(get(params, "payee").unwrap_or(d), 120)),
            );
            if let Some(account) =
                get(params, "category").filter(|id| choices.iter().any(|(key, _)| key == id))
            {
                initial.insert("account", Some(account.to_owned()));
            } else if let Some(journal) = &item.journal_id {
                // Accepted journal/check: reuse its counterpart account and reviewed payee.
                let reviewed: Option<(String, String)> = app.open_mut()?.book.read(|tx| {
                    tx.query_row("SELECT l.account_id,l.name FROM ledger_journalline l JOIN bankfeeds_account f ON f.id=?2 WHERE l.journal_id=?1 AND l.account_id != f.ledger_account_id AND (SELECT count(*) FROM ledger_journalline c WHERE c.journal_id=l.journal_id AND c.account_id != f.ledger_account_id)=1 ORDER BY l.id LIMIT 1", params![journal, item.feed_account_id], |r| Ok((r.get(0)?, r.get(1)?))).optional()
                })?;
                if let Some((account, payee)) =
                    reviewed.filter(|(id, _)| choices.iter().any(|(key, _)| key == id))
                {
                    initial.insert("account", Some(account));
                    // Check journal lines carry accounting amounts, not the reviewed payee.
                    // Read it through the owning document service and verify the journal link.
                    let payee = if item.accepted_as == "CHECK" {
                        let check_id = item
                            .reference_url
                            .strip_prefix("checks:detail:")
                            .ok_or_else(|| {
                                AppError::Invalid(
                                    "This transaction's check reference is damaged.".into(),
                                )
                            })?;
                        let owner = actor(app)?;
                        let check = fb_core::checks::get_check(
                            &mut app.open_mut()?.book,
                            owner,
                            c,
                            check_id,
                        )?;
                        if check["current"]["journal_id"].as_str() != Some(journal.as_str()) {
                            return Err(AppError::Invalid(
                                "This transaction's check does not match its journal.".into(),
                            ));
                        }
                        check["current"]["payee_name"]
                            .as_str()
                            .unwrap_or_default()
                            .to_owned()
                    } else {
                        payee
                    };
                    initial.insert("payee_name", Some(payee));
                }
            } else {
                let owner = actor(app)?;
                let rules = bf::active_rules(&mut app.open_mut()?.book, owner, c)?;
                if let Some(rule) = bf::rule_for(&rules, &item) {
                    initial.insert("account", Some(rule.account_id.clone()));
                    if get(params, "payee").is_none() {
                        initial.insert("payee_name", Some(rule.payee_name.clone()));
                    }
                }
            }
        }
    }
    Ok(initial)
}
