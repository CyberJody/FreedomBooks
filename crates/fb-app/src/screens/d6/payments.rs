use super::*;
use crate::commands::BillAllocationValues;
fn record(app: &mut App, c: CompanyId, id: &str) -> AppResult<Value> {
    let mut v=scoped_rows(app,c,"SELECT p.*,i.number FROM payables_billpayment p JOIN documents_documentidentity i ON i.id=p.identity_id WHERE p.company_id=? AND p.id=?",&[&c.to_db(),&id])?.pop().ok_or_else(||AppError::NotFound("payment".into()))?;
    json_columns(&mut v);
    v["pk"] = v["id"].clone();
    v["identity"] = json!({"number":v["number"]});
    v["amount"] = json!(cash(v["amount"].as_i64().unwrap() as i128));
    let bank = app.open_mut()?.book.read(|tx| {
        fb_core::store::account(tx, AccountId::parse(s(&v, "bank_id")).unwrap())
            .map(|a| a.expect("payment bank"))
    })?;
    v["bank"] = json!({"code":bank.code,"name":bank.name});
    let rev = scoped_rows(
        app,
        c,
        "SELECT * FROM payables_paymentreversal WHERE payment_id=?",
        &[&id],
    )?
    .pop();
    v["reversed"] = json!(rev.is_some());
    v["reversal"] = json!(rev);
    v["get_method_display"] = json!(match s(&v, "method") {
        "CHECK" => "Check",
        "CASH" => "Cash",
        "CARD" => "Card",
        "TRANSFER" => "Bank transfer",
        "OTHER" => "Other",
        s => s,
    });
    Ok(v)
}
// ---------------------------------------------------------------- Pay bills
//
// One screen lists every open bill. The user ticks bills and amounts; on save the page's proposal
// becomes one payment per vendor. Each payment goes through the same review the core has always
// required (ReviewBillPayment), and is recorded only when the review's entries and check number
// equal what the page showed (`shown-<vendor>`), so what posts is exactly what the user saw. All
// payments of one run are recorded in one transaction: if any vendor's payment is refused, none is
// recorded and the page explains why with the selections kept. Each vendor's request key is derived
// from the run's key, so sending the same page twice records nothing new.
const METHODS: [(&str, &str); 5] = [
    ("CHECK", "Check"),
    ("CASH", "Cash"),
    ("CARD", "Card"),
    ("TRANSFER", "Bank transfer"),
    ("OTHER", "Other"),
];
/// A vendor's request key inside one Pay bills run: stable for the same run and vendor.
fn vendor_key(run: &str, vendor: &str) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(format!("fb-pay-bills:{run}:{vendor}").as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(bytes).to_string()
}
/// Bill amounts typed by people: "1,234.5" or "$80". None when it is not a plain amount.
fn typed_cents(text: &str) -> Option<i64> {
    let t = py::py_strip(text).replace([',', '$', ' '], "");
    if t.is_empty() || t.matches('.').count() > 1 || !t.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return None;
    }
    let (whole, frac) = t.split_once('.').unwrap_or((&t, ""));
    if frac.len() > 2 || (whole.is_empty() && frac.is_empty()) || whole.len() > 14 {
        return None;
    }
    let whole = if whole.is_empty() { 0 } else { whole.parse::<i64>().ok()? };
    let frac = format!("{frac:0<2}").parse::<i64>().ok()?;
    whole.checked_mul(100)?.checked_add(frac)
}
fn cents_text(c: i64) -> String {
    cash(c as i128)
}
/// Cents with thousands separators ("1,050.84"), as the page's own figures read.
fn grouped(c: i64) -> String {
    let t = cents_text(c);
    let (whole, frac) = t.split_once('.').unwrap_or((&t, "00"));
    let digits = whole.trim_start_matches('-');
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    format!("{}{out}.{frac}", if whole.starts_with('-') { "-" } else { "" })
}
/// Every posted bill with an open balance, with what the screen shows and the payables account
/// its posting used (the account a payment debits).
fn open_bills(app: &mut App, c: CompanyId, payment_date: &str) -> AppResult<Vec<Value>> {
    let actor = app.open_mut()?.owner;
    let bills = payables::current_bills(&mut app.open_mut()?.book, actor, c)?;
    let names = labels(app, c)?;
    let accounts = app.open_mut()?.book.read(|tx| fb_core::store::accounts(tx, c))?;
    let mut out = vec![];
    for b in bills {
        if b["state"] != "OPEN" || !["OPEN", "PARTIAL"].contains(&s(&b, "payment_state")) {
            continue;
        }
        let open = typed_cents(s(&b, "outstanding")).unwrap_or(0);
        if open <= 0 {
            continue;
        }
        let r = &b["current"];
        let ap = scoped_rows(
            app,
            c,
            "SELECT account_id FROM documents_documentaccount WHERE source_id=? AND role='payables'",
            &[&s(&b, "identity_id")],
        )?
        .pop()
        .map(|v| s(&v, "account_id").to_owned())
        .unwrap_or_default();
        let ap_label = accounts.iter().find(|a| a.id.to_db() == ap).map(|a| a.label()).unwrap_or_default();
        let vendor = s(r, "vendor_id").to_owned();
        let due = s(r, "due_date").to_owned();
        let late = payment_date.parse::<chrono::NaiveDate>().ok()
            .zip(due.parse::<chrono::NaiveDate>().ok())
            .map(|(payment, due)| (payment - due).num_days());
        let status = match late {
            Some(n) if n > 0 => format!("Overdue {n} day{}", if n == 1 { "" } else { "s" }),
            Some(0) => "Due on payment date".into(),
            Some(n) => format!("Due in {} day{}", -n, if n == -1 { "" } else { "s" }),
            None => "Choose a payment date".into(),
        };
        let vendor_label = names[&vendor].as_str().map(str::to_owned).unwrap_or_else(|| s(&r["vendor_snapshot"], "display_name").to_owned());
        out.push(json!({
            "id": b["id"], "number": b["number"], "vendor_id": vendor, "vendor": vendor_label,
            "reference": r["reference"], "date": r["date"], "due_date": due,
            "overdue": late.is_some_and(|n| n > 0), "status": status,
            "total": r["total"], "open": cents_text(open), "open_cents": open,
            "payables_id": ap, "payables_label": ap_label, "revision_id": r["id"],
        }));
    }
    out.sort_by(|a, b| s(a, "due_date").cmp(s(b, "due_date")).then(s(a, "vendor").cmp(s(b, "vendor"))).then(s(a, "number").cmp(s(b, "number"))));
    Ok(out)
}
/// Check numbers for a run: the first vendor takes the number entered, the next vendor the next
/// number, and so on, keeping leading zeros. Other methods use the reference as typed.
fn references_for(method: &str, reference: &str, count: usize) -> Result<Vec<String>, String> {
    let r = py::py_strip(reference).to_owned();
    if method != "CHECK" || count <= 1 || r.is_empty() {
        return Ok(vec![r; count]);
    }
    if !r.bytes().all(|b| b.is_ascii_digit()) || r.len() > 18 {
        return Err("Enter the first check number as digits only; each vendor's check takes the next number.".into());
    }
    let start = r.parse::<u64>().map_err(|_| "Enter a shorter check number.".to_owned())?;
    (0..count as u64)
        .map(|i| start.checked_add(i).map(|n| format!("{n:0width$}", width = r.len())).ok_or_else(|| "Enter a shorter check number.".to_owned()))
        .collect()
}
struct Run {
    run: String,
    bank: String,
    date: String,
    method: String,
    reference: String,
    memo: String,
    ticked: Vec<String>,
    amounts: HashMap<String, String>,
    /// The page's filters when it was sent (worker item 25: a refused run keeps them).
    filter_vendor: String,
    filter_due: String,
}
fn run_values(p: Posted<'_>) -> Run {
    let get_all = |k: &str| p.iter().filter(|(n, _)| n == k).map(|(_, v)| v.clone()).collect::<Vec<_>>();
    let mut ticked = get_all("pay");
    ticked.dedup();
    Run {
        run: get(p, "request_key").unwrap_or("").to_owned(),
        bank: get(p, "bank").unwrap_or("").to_owned(),
        date: get(p, "date").unwrap_or("").to_owned(),
        method: get(p, "method").unwrap_or("CHECK").to_owned(),
        reference: get(p, "reference").unwrap_or("").to_owned(),
        memo: get(p, "memo").unwrap_or("").to_owned(),
        ticked,
        amounts: p.iter().filter_map(|(k, v)| k.strip_prefix("amount-").map(|id| (id.to_owned(), v.clone()))).collect(),
        filter_vendor: get(p, "filter_vendor").unwrap_or("").to_owned(),
        filter_due: get(p, "filter_due").unwrap_or("").to_owned(),
    }
}
#[derive(Default)]
struct PayPage {
    errors: Vec<String>,
    row_errors: HashMap<String, String>,
    field_errors: HashMap<String, String>,
    /// Worker item 26: check numbers used before on the bank account, and the key that confirms them.
    warnings: Vec<String>,
    reuse_ok: String,
    /// How many of `warnings` were a voided check, reversed payment or voided refund, and how many a
    /// sales tax payment's Reference (review 2, S1: only the first may be called reusable).
    reuse_released: usize,
    reuse_tax: usize,
}
#[allow(clippy::too_many_arguments)]
fn pay_bills_page(app: &mut App, r: &Route, path: &str, query: Posted<'_>, posted: Option<&Run>, page: &PayPage) -> AppResult<String> {
    let c = arg_company(r)?;
    let today = (app.today)().to_string();
    let mut vm = ctx(app, r)?;
    let rows = open_bills(app, c, posted.map_or(&today, |run| &run.date))?;
    let offered = banks(app, c, &[])?;
    let default = default_bank(app, c, &offered, "SELECT bank_id FROM payables_billpayment WHERE company_id=? ORDER BY id DESC LIMIT 1")?;
    let mut next = json!({});
    for b in &offered {
        let actor = app.open_mut()?.owner;
        next[b.id.to_db()] = json!(fb_core::checks::next_number(&mut app.open_mut()?.book, actor, c, b.id)?);
    }
    // Arriving from a bill or a vendor: tick the given bills; filter to the given vendor, or to the
    // one vendor all the given bills belong to.
    let wanted = query.iter().filter(|(k, _)| k == "bill").map(|(_, v)| v.clone()).collect::<Vec<_>>();
    let mut filter_vendor = get(query, "vendor").unwrap_or("").to_owned();
    let mut filter_due = get(query, "due").unwrap_or("").to_owned();
    let given = rows.iter().filter(|b| wanted.iter().any(|w| w == s(b, "id"))).collect::<Vec<_>>();
    if filter_vendor.is_empty() && !given.is_empty() && given.iter().all(|b| b["vendor_id"] == given[0]["vendor_id"]) {
        filter_vendor = s(given[0], "vendor_id").to_owned();
    }
    if let Some(run) = posted {
        // Worker item 25: a refused run keeps the filters it was sent with, so a bill the filter hid
        // does not come back into view unticked (a "tick all" would then include it). Ticked bills and
        // bills with a problem are shown whatever the filter.
        filter_vendor = run.filter_vendor.clone();
        filter_due = run.filter_due.clone();
    }
    if fb_db::dates::parse_iso_date(&filter_due).is_none() {
        filter_due = String::new();
    }
    let mut vendors: Vec<(String, String)> = vec![];
    for b in &rows {
        if !vendors.iter().any(|(id, _)| id == s(b, "vendor_id")) {
            vendors.push((s(b, "vendor_id").into(), s(b, "vendor").into()));
        }
    }
    vendors.sort_by(|a, b| a.1.cmp(&b.1));
    if !vendors.iter().any(|(id, _)| *id == filter_vendor) {
        filter_vendor = String::new();
    }
    let mut rows = rows;
    for b in &mut rows {
        let id = s(b, "id").to_owned();
        let (ticked, amount) = match posted {
            Some(run) => (run.ticked.contains(&id), run.amounts.get(&id).cloned().unwrap_or_default()),
            None => {
                let t = wanted.contains(&id);
                (t, if t { s(b, "open").to_owned() } else { String::new() })
            }
        };
        let shown = (filter_vendor.is_empty() || b["vendor_id"] == filter_vendor) && (filter_due.is_empty() || s(b, "due_date") <= filter_due.as_str());
        b["ticked"] = json!(ticked);
        b["amount"] = json!(amount);
        b["hidden"] = json!(!shown && !ticked && !page.row_errors.contains_key(&id));
        b["error"] = json!(page.row_errors.get(&id));
    }
    let run = posted;
    let request_key = run.map(|r| r.run.clone()).unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let bank = run.map(|r| r.bank.clone()).or(default.clone()).unwrap_or_default();
    let date = run.map(|r| r.date.clone()).unwrap_or(today.clone());
    let method = run.map(|r| r.method.clone()).unwrap_or_else(|| "CHECK".into());
    let reference = run.map(|r| r.reference.clone()).unwrap_or_else(|| default.as_ref().map(|b| s(&next, b).to_owned()).unwrap_or_default());
    let memo = run.map(|r| r.memo.clone()).unwrap_or_default();
    let values = json!({"request_key": request_key, "bank": bank, "date": date, "method": method, "reference": reference, "memo": memo});
    put(&mut vm, "rows", &rows);
    put(&mut vm, "values", &values);
    put(&mut vm, "banks", offered.iter().map(|a| json!({"id":a.id.to_db(),"label":a.label()})).collect::<Vec<_>>());
    put(&mut vm, "methods", METHODS.iter().map(|(v, l)| json!({"value":v,"label":l})).collect::<Vec<_>>());
    put(&mut vm, "vendors", vendors.iter().map(|(id, l)| json!({"id":id,"label":l})).collect::<Vec<_>>());
    put(&mut vm, "filter_vendor", &filter_vendor);
    put(&mut vm, "filter_due", &filter_due);
    put(&mut vm, "errors", &page.errors);
    put(&mut vm, "warnings", &page.warnings);
    put(&mut vm, "reuse_ok", &page.reuse_ok);
    put(&mut vm, "reuse_released", page.reuse_released);
    put(&mut vm, "reuse_tax", page.reuse_tax);
    put(&mut vm, "field_errors", json!(page.field_errors));
    put(&mut vm, "pay_data", json!({"next_numbers": next, "today": today}));
    put(&mut vm, "csrf_token", token(app, path));
    // After a successful run: the payments it recorded.
    if let Some(done) = get(query, "paid").and_then(py::py_uuid) {
        let keys = scoped_rows(app, c, "SELECT id FROM directory_vendor WHERE company_id=?", &[&c.to_db()])?
            .iter()
            .map(|v| vendor_key(&done.0, s(v, "id")))
            .collect::<Vec<_>>();
        let mut made = vec![];
        for k in keys {
            if let Some(found) = scoped_rows(app, c, "SELECT p.id FROM payables_billpayment p JOIN documents_documentidentity i ON i.id=p.identity_id WHERE p.company_id=? AND i.request_key=?", &[&c.to_db(), &k])?.pop() {
                made.push(record(app, c, s(&found, "id"))?);
            }
        }
        made.sort_by(|a, b| s(a, "number").cmp(s(b, "number")));
        let total = made.iter().filter_map(|m| typed_cents(s(m, "amount"))).sum::<i64>();
        let bills = made.iter().map(|m| scoped_rows(app, c, "SELECT count(*) AS n FROM payables_paymentallocation WHERE payment_id=?", &[&s(m, "id")]).map(|v| v[0]["n"].as_i64().unwrap_or(0))).sum::<AppResult<i64>>()?;
        let labels = labels(app, c)?;
        for m in &mut made {
            let v = s(m, "vendor_id").to_owned();
            m["vendor_label"] = labels[&v].clone();
        }
        put(&mut vm, "paid", json!({"payments": made, "total": grouped(total), "bills": bills}));
    }
    draw(app, "payables/payments/form.html", &vm)
}
/// Whether any payment was recorded under this run's key (for any vendor).
fn already_paid(app: &mut App, c: CompanyId, run: &str) -> AppResult<bool> {
    for v in scoped_rows(app, c, "SELECT id FROM directory_vendor WHERE company_id=?", &[&c.to_db()])? {
        let k = vendor_key(run, s(&v, "id"));
        if !scoped_rows(app, c, "SELECT p.id FROM payables_billpayment p JOIN documents_documentidentity i ON i.id=p.identity_id WHERE p.company_id=? AND i.request_key=?", &[&c.to_db(), &k])?.is_empty() {
            return Ok(true);
        }
    }
    Ok(false)
}
fn pay_bills_submit(app: &mut App, r: &Route, path: &str, p: Posted<'_>) -> AppResult<Outcome> {
    let c = arg_company(r)?;
    let run = run_values(p);
    let mut page = PayPage::default();
    let mut fe = Errors::new();
    let key = key(p, true, &mut fe);
    let date = clean_date(p, "date", true, &mut fe).unwrap_or_default();
    let reference = clean_text(p, "reference", 120, false, true, &mut fe);
    super::super::d2::refuse_card_number(p, "reference", &mut fe);
    let memo = clean_text(p, "memo", 2000, false, true, &mut fe);
    super::super::d2::refuse_card_number(p, "memo", &mut fe);
    let offered = banks(app, c, &[])?;
    if run.bank.is_empty() || !offered.iter().any(|a| a.id.to_db() == run.bank) {
        error(&mut fe, "bank", if offered.is_empty() { "Add a bank account to the chart of accounts first (account type Bank)." } else { "Choose the bank account to pay from." });
    }
    if !METHODS.iter().any(|(m, _)| *m == run.method) {
        error(&mut fe, "method", "Choose how you are paying.");
    }
    for (k, v) in &fe {
        if k != "request_key" {
            page.field_errors.insert(k.clone(), v.join(" "));
        } else {
            page.errors.push("This page is out of date. Open Pay bills again.".into());
        }
    }
    // Sent twice: the first send already recorded this run's payments, so show them instead of
    // paying again (or reporting their bills as no longer open).
    if !key.is_empty() && already_paid(app, c, &key)? {
        return Ok(Outcome::Redirect { location: format!("{}?paid={key}", url("payments:new", &[&c.to_db()])) });
    }
    // The ticked bills, as the page showed them.
    let rows = open_bills(app, c, &date)?;
    let mut chosen: Vec<(&Value, i64)> = vec![];
    for id in &run.ticked {
        let Some(b) = rows.iter().find(|b| s(b, "id") == id) else {
            page.errors.push("A ticked bill is no longer open: it was paid, voided or changed elsewhere. The list below is up to date; check your choices and pay again.".into());
            continue;
        };
        let typed = run.amounts.get(id).map(String::as_str).unwrap_or("");
        let amount = if py::py_strip(typed).is_empty() { Some(b["open_cents"].as_i64().unwrap_or(0)) } else { typed_cents(typed) };
        match amount {
            None => { page.row_errors.insert(id.clone(), "Enter an amount such as 125.00.".into()); }
            Some(0) => { page.row_errors.insert(id.clone(), "Enter an amount more than zero, or untick this bill.".into()); }
            Some(a) if a > b["open_cents"].as_i64().unwrap_or(0) => {
                page.row_errors.insert(id.clone(), format!("This is more than the open balance of {}. Pay at most {}: a payment cannot leave credit with the vendor.", s(b, "open"), s(b, "open")));
            }
            Some(_) if !date.is_empty() && s(b, "date") > date.as_str() => {
                page.row_errors.insert(id.clone(), format!("The payment date is before this bill's date ({}).", s(b, "date")));
            }
            Some(a) => chosen.push((b, a)),
        }
    }
    if run.ticked.is_empty() {
        page.errors.push("Tick at least one bill to pay.".into());
    }
    if !page.row_errors.is_empty() {
        page.errors.push(format!("{} bill{} need{} a different amount or date; see the marked rows.", page.row_errors.len(), if page.row_errors.len() == 1 { "" } else { "s" }, if page.row_errors.len() == 1 { "s" } else { "" }));
    }
    // One payment per vendor, in vendor name order.
    let mut vendors: Vec<(String, String)> = vec![];
    for (b, _) in &chosen {
        if !vendors.iter().any(|(id, _)| id == s(b, "vendor_id")) {
            vendors.push((s(b, "vendor_id").into(), s(b, "vendor").into()));
        }
    }
    vendors.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
    let refs = match references_for(&run.method, &reference, vendors.len()) {
        Ok(r) => r,
        Err(m) => {
            page.field_errors.insert("reference".into(), m);
            vec![]
        }
    };
    let invalid = |page: &PayPage| !page.errors.is_empty() || !page.row_errors.is_empty() || !page.field_errors.is_empty();
    if invalid(&page) {
        return Ok(Outcome::Invalid { html: pay_bills_page(app, r, path, &[], Some(&run), &page)? });
    }
    let keys = vendors.iter().map(|(v, _)| vendor_key(&key, v)).collect::<Vec<_>>();
    let mut payments = vec![];
    let mut tokens = vec![];
    let mut mismatch = false;
    for (i, (vendor, label)) in vendors.iter().enumerate() {
        let mine = chosen.iter().filter(|(b, _)| s(b, "vendor_id") == vendor).collect::<Vec<_>>();
        let total = mine.iter().map(|(_, a)| a).sum::<i64>();
        let values = BillPaymentValues {
            request_key: keys[i].clone(),
            vendor_id: vendor.clone(),
            date: date.clone(),
            amount: cents_text(total),
            method: run.method.clone(),
            reference: refs[i].clone(),
            memo: memo.clone(),
            bank_id: run.bank.clone(),
            allocations: mine.iter().map(|(b, a)| BillAllocationValues { bill_id: s(b, "id").into(), amount: cents_text(*a) }).collect(),
        };
        match app.run(Command::ReviewBillPayment { company_id: c.to_db(), values: values.clone() }) {
            Ok(Outcome::Done { data }) => {
                let entries = data["entries"].as_array().cloned().unwrap_or_default();
                let server = format!("{}||{}", refs[i], entries_signature(&entries));
                let sent = get(p, &format!("shown-{vendor}")).unwrap_or("");
                if sent != server {
                    mismatch = true;
                }
                tokens.push(s(&data, "token").to_owned());
                payments.push(values);
            }
            Err(AppError::Core(FbError::Validation(m))) => page.errors.push(format!("{label}: {}", m.join(" "))),
            Err(e) => return Err(e),
            Ok(_) => unreachable!(),
        }
    }
    // A refused or unconfirmed run leaves no reviews behind (they would crowd out others).
    let drop_reviews = |app: &mut App, tokens: &[String]| {
        for t in tokens {
            app.d6_reviews.payments.remove(t);
        }
    };
    if !page.errors.is_empty() || mismatch {
        drop_reviews(app, &tokens);
    }
    if !page.errors.is_empty() {
        page.errors.insert(0, "Nothing was paid. Fix the problem below and choose Pay selected bills again; your choices are kept.".into());
        return Ok(Outcome::Invalid { html: pay_bills_page(app, r, path, &[], Some(&run), &page)? });
    }
    if mismatch {
        page.errors.push("Nothing was paid yet: the amounts, accounts or check numbers below changed since the page was shown. Check the payments and choose Pay selected bills again.".into());
        return Ok(Outcome::Invalid { html: pay_bills_page(app, r, path, &[], Some(&run), &page)? });
    }
    // Worker item 26: a check number used before on this bank account by a voided check, a reversed
    // payment or a voided refund (or named in a sales tax payment's reference) may be used again, but
    // the first send only warns; sending again with the same numbers goes ahead. (After the reviews: a
    // number still in use is refused there instead.)
    if run.method == "CHECK" {
        let mut reused = vec![];
        let (mut released, mut tax) = (0, 0);
        for (i, (_, label)) in vendors.iter().enumerate() {
            let n = &refs[i];
            if !(1..=20).contains(&n.len()) || !n.bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            if let Some(earlier) = app.open_mut()?.book.read(|q| fb_core::checks::earlier_use(q, c, &run.bank, n))? {
                if earlier.is_tax_payment() { tax += 1 } else { released += 1 }
                reused.push(format!("Check {n} for {label}: used before by {}.", earlier.text()));
            }
        }
        let confirm = format!("{}:{}", run.bank, refs.join(","));
        if !reused.is_empty() && get(p, "reuse_ok") != Some(confirm.as_str()) {
            drop_reviews(app, &tokens);
            page.warnings = reused;
            page.reuse_ok = confirm;
            page.reuse_released = released;
            page.reuse_tax = tax;
            return Ok(Outcome::Invalid { html: pay_bills_page(app, r, path, &[], Some(&run), &page)? });
        }
    }
    match app.run(Command::RecordBillPayments { company_id: c.to_db(), payments, review_tokens: tokens }) {
        Ok(Outcome::Done { .. }) => Ok(Outcome::Redirect { location: format!("{}?paid={key}", url("payments:new", &[&c.to_db()])) }),
        Err(AppError::Core(FbError::Validation(m))) => {
            page.errors.push(format!("Nothing was paid: {}", m.join(" ")));
            Ok(Outcome::Invalid { html: pay_bills_page(app, r, path, &[], Some(&run), &page)? })
        }
        Err(e) => Err(e),
        Ok(_) => unreachable!(),
    }
}
fn listing(app: &mut App, r: &Route, p: Posted<'_>) -> AppResult<(String, u16)> {
    let c = arg_company(r)?;
    let mut vm = ctx(app, r)?;
    let fs = vec![
        Field::new("q", "Number, reference or vendor", Widget::Text).attr("maxlength", "120"),
        select(
            "vendor",
            "Vendor",
            "Choose…",
            contacts(app, c, false, None, "short")?,
            false,
        ),
        select(
            "state",
            "State",
            "All payments",
            vec![
                ("LIVE".into(), "Recorded".into()),
                ("REVERSED".into(), "Reversed".into()),
            ],
            false,
        ),
    ];
    let mut e = Errors::new();
    let q = clean_text(p, "q", 120, false, true, &mut e);
    let vendor = choice(p, &fs[1], &mut e, true);
    let state = choice(p, &fs[2], &mut e, false);
    let mut records = vec![];
    if e.is_empty() {
        let ids = scoped_rows(
            app,
            c,
            "SELECT id FROM payables_billpayment WHERE company_id=? ORDER BY date DESC,id DESC",
            &[&c.to_db()],
        )?;
        let q = fb_db::collation::upper(&q);
        for id in ids {
            let v = record(app, c, s(&id, "id"))?;
            if (!vendor.is_empty() && v["vendor_id"] != vendor)
                || (!state.is_empty() && (v["reversed"] == true) != (state == "REVERSED"))
                || (!q.is_empty()
                    && ![
                        s(&v, "number"),
                        s(&v, "reference"),
                        s(&v["vendor_snapshot"], "display_name"),
                    ]
                    .iter()
                    .any(|v| fb_db::collation::upper(v).contains(&q)))
            {
                continue;
            }
            records.push(v);
        }
    }
    put(
        &mut vm,
        "filter_form",
        bind_fields(fs, Some(p), &Value::Null, &e, true),
    );
    put(&mut vm, "page", Page::of(&records, 25, get(p, "page")));
    put(
        &mut vm,
        "query_string",
        urlencode(
            &p.iter()
                .filter(|(k, _)| k != "page")
                .cloned()
                .collect::<Vec<_>>(),
        ),
    );
    put(&mut vm, "vendor_labels", labels(app, c)?);
    Ok((
        draw(app, "payables/payments/list.html", &vm)?,
        if e.is_empty() { 200 } else { 400 },
    ))
}
fn detail(app: &mut App, r: &Route) -> AppResult<String> {
    let c = arg_company(r)?;
    let v = record(app, c, &r.args["payment_id"])?;
    let mut vm = ctx(app, r)?;
    put(&mut vm, "payment", &v);
    put(
        &mut vm,
        "references",
        references(app, c, s(&v, "identity_id"))?,
    );
    let mut allocations = scoped_rows(
        app,
        c,
        "SELECT a.*,r.number AS revision_number,i.number AS bill_number FROM payables_paymentallocation a JOIN payables_bill b ON b.id=a.bill_id JOIN documents_documentidentity i ON i.id=b.identity_id JOIN payables_billrevision r ON r.id=a.bill_revision_id WHERE a.payment_id=? ORDER BY a.id",
        &[&s(&v, "id")],
    )?;
    for row in &mut allocations {
        row["amount"] = json!(cash(row["amount"].as_i64().unwrap() as i128));
        row["bill"] = json!({"identity":{"number":row["bill_number"]}});
        row["bill_revision"] = json!({"number":row["revision_number"]});
    }
    put(&mut vm, "allocations", allocations);
    let accounts = app
        .open_mut()?
        .book
        .read(|tx| fb_core::store::accounts(tx, c))?;
    put(
        &mut vm,
        "account_labels",
        accounts
            .iter()
            .map(|a| json!({"id":a.id.to_db(),"code":a.code,"name":a.name}))
            .collect::<Vec<_>>(),
    );
    evidence(app, c, &mut vm)?;
    attachments(app, c, s(&v, "identity_id"), &mut vm)?;
    draw(app, "payables/payments/detail.html", &vm)
}
fn reverse_form(
    app: &mut App,
    r: &Route,
    path: &str,
    p: Option<Posted<'_>>,
    e: &Errors,
) -> AppResult<String> {
    let c = arg_company(r)?;
    let v = record(app, c, &r.args["payment_id"])?;
    let mut vm = ctx(app, r)?;
    let date = crate::screens::document_date(app, &v);
    put(&mut vm, "payment", v);
    put(
        &mut vm,
        "form",
        bind_fields(
            crate::screens::month_notice(app, reversal_fields()),
            p,
            &json!({"expected_revision":1,"date":date}),
            e,
            true,
        ),
    );
    put(&mut vm, "csrf_token", token(app, path));
    draw(app, "payables/payments/reverse.html", &vm)
}
pub(super) fn render(
    app: &mut App,
    r: &Route,
    p: Posted<'_>,
    path: &str,
) -> AppResult<(String, u16)> {
    match r.name.as_str() {
        "payments:list" => listing(app, r, p),
        "payments:new" => Ok((pay_bills_page(app, r, path, p, None, &PayPage::default())?, 200)),
        "payments:detail" => Ok((detail(app, r)?, 200)),
        "payments:reverse" => Ok((reverse_form(app, r, path, None, &Errors::new())?, 200)),
        _ => Err(AppError::NotFound(r.name.clone())),
    }
}
pub(super) fn submit(app: &mut App, r: &Route, path: &str, p: Posted<'_>) -> AppResult<Outcome> {
    let c = arg_company(r)?;
    let mut e = Errors::new();
    if r.name == "payments:reverse" {
        let _ = record(app, c, &r.args["payment_id"])?;
        let (rev, date, reason) = reverse_values(p, &mut e);
        if e.is_empty()
            && result_error(
                app.run(Command::ReverseBillPayment {
                    company_id: c.to_db(),
                    payment_id: r.args["payment_id"].clone(),
                    expected_revision: rev,
                    date,
                    reason,
                }),
                &mut e,
            )?
            .is_some()
        {
            return Ok(Outcome::Redirect {
                location: url("payments:detail", &[&c.to_db(), &r.args["payment_id"]]),
            });
        }
        return Ok(Outcome::Invalid {
            html: reverse_form(app, r, path, Some(p), &e)?,
        });
    }
    if r.name != "payments:new" {
        return Err(AppError::NotFound(path.into()));
    }
    pay_bills_submit(app, r, path, p)
}
