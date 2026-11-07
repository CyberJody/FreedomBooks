//! Normalization before comparing a Django page with its fb-templates port (rules in NORMALIZATION.md).
//! Only values with known provenance are rewritten: ids are replaced by symbolic names *per model and per
//! context* (a route argument, a select of that model, a known JSON key), never by global search and
//! replace, so a stray number or a wrong id still shows as a difference.
mod literal;
pub mod numbering;
mod printing;
mod d3;
mod d4;
mod d6;
mod d7a;
mod d7b;
mod d5;
mod d8;
pub mod desktop;
mod company_documents;
mod backups;
mod d9;
mod sim_fix;
mod starter_chart;
pub mod release;
use regex::{Captures, Regex};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::sync::LazyLock;

/// model -> (raw id -> symbolic name), from one side's `ids.json` (`account:1100` -> id).
#[derive(Clone)]
pub struct IdMap {
    generated_undo_key: bool,
    new_company: bool,
    /// The Django reference side: deliberate desktop divergences (desktop.rs) are applied to it.
    reference: bool,
    by_model: HashMap<String, HashMap<String, String>>,
    labels: HashMap<(String,String),String>,
}

impl IdMap {
    pub fn new(ids: &BTreeMap<String, String>) -> IdMap {
        let mut by_model: HashMap<String, HashMap<String, String>> = HashMap::new();
        for (key, id) in ids {
            let model = key.split_once(':').map(|(m, _)| m).unwrap_or(key);
            by_model.entry(model.to_owned()).or_default().insert(id.clone(), if ["audit_local_stamp","audit_utc_stamp","posted_stamp","document_stamp","document_default_stamp","attachment_stamp"].contains(&model){format!("{{{model}}}")}else{format!("{{{key}}}")});
        }
        let mut labels=HashMap::new();
        for (key,value) in ids {if let Some(rest)=key.strip_prefix("label:") && let Some((model,_))=rest.split_once(':') && let Some(raw)=ids.get(rest) {labels.insert((model.into(),raw.clone()),value.clone());}}
        IdMap { by_model, labels, generated_undo_key: false, new_company: false, reference: false }
    }
    /// Mark the map as the Django reference side's (see `desktop`).
    pub fn reference(mut self) -> Self {
        self.reference = true;
        self
    }
    pub fn for_screen(&self, path: &str, is_post: bool) -> Self {
        let mut scoped = self.clone();
        scoped.new_company = path == "/companies/new/";
        scoped.generated_undo_key = path.ends_with("/undo/") && path.contains("/reconcile/") && !is_post;
        scoped
    }
    fn get(&self, model: &str, id: &str) -> Option<&str> {
        self.by_model.get(model).and_then(|m| m.get(id)).map(String::as_str)
    }
}

/// A route argument name -> the model its ids belong to.
fn arg_model(arg: &str) -> Option<&'static str> {
    Some(match arg {
        "company_id" => "company",
        "account_id" => "account",
        "invoice_id" => "invoice",
        "estimate_id"=>"estimate",
        "deposit_id"=>"deposit",
        "document_id"=>"identity",
        "sales_receipt_id"=>"receipt",
        "journal_id" => "journal",
        "receipt_id" => "receipt",
        "bill_id" => "bill",
        "payment_id" => "billpayment",
        "transfer_id" => "transfer",
        "check_id" => "check",
        "credit_memo_id" => "creditmemo",
        "application_id" => "creditapplication",
        "deposit_application_id" => "depositapplication",
        "deposit_refund_id" => "depositrefund",
        "customer_deposit_id" => "receipt",
        "refund_id" => "creditrefund",
        "item_id" => "taxitem",
        "reconciliation_id" => "reconciliation",
        "connection_id" => "bfconnection",
        "attachment_id" => "attachment",
        "rule_id" => "bfrule",
        _ => return None,
    })
}

/// A select's field name (without formset prefix) -> the model of its option values.
fn select_model(name: &str) -> Option<&'static str> {
    let field = name.rsplit('-').next().unwrap_or(name);
    Some(match field {
        "customer" => "customer",
        "invoice" => "invoice",
        "estimate" => "estimate",
        "deposit" => "receipt",
        "vendor" => "vendor",
        "bill" | "supersedes" => "bill",
        "bank" | "from_account" | "to_account" => "account",
        "agency" => "vendor",
        "default_item" => "taxitem",
        "item" => "item",
        "expense_account" | "default_expense_account" | "income_account" | "account" | "receivables" | "payables" | "undeposited" | "collected_tax" | "customer_deposits" | "banks" => "account",
        "user" => "user",
        _ => return None,
    })
}

fn url(value: &str, ids: &IdMap) -> String {
    if let Some(q)=value.strip_prefix('?'){return format!("?{}",query_ids(q,ids,true));}
    if !value.starts_with('/') {
        return value.to_owned();
    }
    let (path, query) = match value.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (value, None),
    };
    let mut out = path.to_owned();
    if let Some(pattern) = route_pattern(path) {
        let segs: Vec<String> = path
            .split('/')
            .zip(pattern.split('/'))
            .map(|(seg, pat)| {
                let arg = pat.strip_prefix('<').and_then(|p| p.strip_suffix('>')).map(|p| p.split_once(':').map(|(_, n)| n).unwrap_or(p));
                match arg.and_then(|arg|if arg=="record_id" {
                    if path.contains("/lists/items/"){Some("item")}else if path.contains("/lists/customers/"){Some("customer")}else if path.contains("/lists/vendors/"){Some("vendor")}else{None}
                }else{arg_model(arg)}).and_then(|m| ids.get(m, seg)) {
                    Some(sym) => sym.to_owned(),
                    None => seg.to_owned(),
                }
            })
            .collect();
        out = segs.join("/");
    }
    if let Some(q) = query {
        out.push('?');
        out.push_str(&query_ids(q, ids, path.ends_with("/reports/general-ledger/") && route_pattern(path).is_some()));
    }
    out
}

/// The route pattern a path matches, where an `<int:...>` part is either side's id syntax: Django's
/// digits or fb-core's canonical UUID text.
fn route_pattern(path: &str) -> Option<&'static str> {
    let want: Vec<&str> = path.split('/').collect();
    fb_templates::django::route_table().values().find(|pattern| {
        let have: Vec<&str> = pattern.split('/').collect();
        have.len() == want.len()
            && have.iter().zip(&want).all(|(p, w)| match p.strip_prefix('<').and_then(|p| p.strip_suffix('>')) {
                Some(inner) if inner.starts_with("int:") => {
                    (!w.is_empty() && w.bytes().all(|b| b.is_ascii_digit())) || uuid_text(w)
                }
                Some(_) => !w.is_empty(),
                None => p == w,
            })
    }).map(String::as_str)
}

fn uuid_text(w: &str) -> bool {
    w.len() == 36 && w.bytes().enumerate().all(|(i, b)| if [8, 13, 18, 23].contains(&i) { b == b'-' } else { b.is_ascii_digit() || (b'a'..=b'f').contains(&b) })
}

/// JSON with object keys sorted at every level (key order carries no meaning).
fn canonical(v: Value) -> Value {
    match v {
        Value::Object(map) => {
            let sorted: BTreeMap<String, Value> = map.into_iter().map(|(k, v)| (k, canonical(v))).collect();
            Value::Object(sorted.into_iter().collect())
        }
        Value::Array(items) => Value::Array(items.into_iter().map(canonical).collect()),
        other => other,
    }
}

/// Query strings: only the `customer=` filter carries an id.
fn query_ids(q: &str, ids: &IdMap, account_filter: bool) -> String {
    q.split("&amp;")
        .map(|part| {
            part.split('&')
                .map(|kv| match kv.split_once('=') {
                    Some(("account", v)) if account_filter => format!("account={}", ids.get("account", v).unwrap_or(v)),
                    Some(("vendor", v)) => format!("vendor={}", ids.get("vendor", v).unwrap_or(v)),
                    Some(("customer", v)) => format!("customer={}", ids.get("customer", v).unwrap_or(v)),
                    Some(("estimate", v)) => format!("estimate={}", ids.get("estimate", v).unwrap_or(v)),
                    Some(("user", v)) => format!("user={}", ids.get("user", v).unwrap_or(v)),
                    _ => kv.to_owned(),
                })
                .collect::<Vec<_>>()
                .join("&")
        })
        .collect::<Vec<_>>()
        .join("&amp;")
}

static WS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
static BETWEEN_TAGS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r">\s+<").unwrap());
static TOKEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<input type="hidden" name="(?:csrfmiddlewaretoken|fb_command_token)" value="[A-Za-z0-9]+">"#).unwrap());
static REQUEST_KEY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"name="request_key" value="[0-9a-f-]{36}""#).unwrap());
static LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"\b(href|src|action)="([^"]*)""#).unwrap());
static SELECT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?s)<select name="([^"]+)"([^>]*)>(.*?)</select>"#).unwrap());
static OPTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"<option value="([^"]*)"([^>]*)>([^<]*)</option>"#).unwrap());
static AUDIT_PRE: LazyLock<Regex> = LazyLock::new(||Regex::new(r"(?s)(<details><summary>View change</summary><pre>)(.*?)(</pre>)").unwrap());
static WHEN: LazyLock<Regex> = LazyLock::new(||Regex::new(r#"(<td class="when" data-label="When">)([^<]*)(</td>)"#).unwrap());
static EVENT_STAMP: LazyLock<Regex> = LazyLock::new(||Regex::new(r#"(<tr><td>)([0-9]{4}-[0-9]{2}-[0-9]{2} [0-9]{2}:[0-9]{2}:[0-9]{2})(</td>)"#).unwrap());
static POSTED_STAMP: LazyLock<Regex> = LazyLock::new(||Regex::new(r#"(Posted by [^<]* at )([0-9]{4}-[0-9]{2}-[0-9]{2} [0-9]{2}:[0-9]{2}:[0-9]{2} UTC)(\.)"#).unwrap());
static DIGEST: LazyLock<Regex> = LazyLock::new(||Regex::new(r#"name="digest" value="([0-9a-f]{64})""#).unwrap());
static JSON_SCRIPT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?s)<script id="([^"]+)" type="application/json">(.*?)</script>"#).unwrap());

/// Only the explicit generated suffix, after its delimiter, can be replaced. A name's #text is data.
fn generated_label(label: &str, model: &str, raw: &str, ids: &IdMap) -> String {
    let Some(sym) = ids.get(model, raw) else { return label.into() };
    let ordinal = ids.labels.get(&(model.into(),raw.into())).map(String::as_str).unwrap_or(raw);
    let contact = ["customer", "vendor"].contains(&model);
    // Contact labels have no generated id unless the fixture records a duplicate-name identity.
    if contact && !sym.rsplit_once(":#").is_some_and(|(_, n)| !n.trim_end_matches('}').is_empty() && n.trim_end_matches('}').bytes().all(|b| b.is_ascii_digit())) {
        return label.into();
    }
    let marker = if contact { format!(" (#{ordinal})") } else if ["account", "item"].contains(&model) {format!(" · #{ordinal}")} else {return label.into()};
    for suffix in ["", " (inactive)", " (archived)", " (archived — confirm retention or replace)", " (unavailable — replace before posting)", " (unavailable — replace)"] {
        if let Some(prefix) = label.strip_suffix(suffix).and_then(|s| s.strip_suffix(&marker)) {
            let generated = if contact {format!(" (#{sym})")} else {format!(" · #{sym}")};
            return format!("{prefix}{generated}{suffix}");
        }
    }
    label.into()
}

fn options(model: &str, body: &str, ids: &IdMap) -> String {
    OPTION
        .replace_all(body, |c: &Captures| {
            let raw = &c[1];
            let Some(sym) = ids.get(model, raw) else { return c[0].to_owned() };
            let label = generated_label(&c[3], model, raw, ids);
            format!(r#"<option value="{sym}"{}>{label}</option>"#, &c[2])
        })
        .into_owned()
}

/// `invoice-catalog` ({item id: {..., income_account: account id}}) and `customer-tax` ({customer id: ...}).
fn json_ids(id: &str, text: &str, ids: &IdMap) -> String {
    let unescaped = text.replace("\\u003C", "<").replace("\\u003E", ">").replace("\\u0026", "&");
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&unescaped) else { return text.to_owned() };
    let key_model = match id {
        "invoice-catalog" | "estimate-catalog" | "sales-receipt-catalog" | "bill-catalog" | "item-catalog" => "item",
        "vendor-defaults" | "vendor-catalog" => "vendor",
        "check-next-numbers" | "next-numbers" => "account",
        "customer-tax" => "customer",
        "tax-rates" => "taxitem",
        _ => return text.to_owned(),
    };
    let mut out = BTreeMap::new();
    for (k, mut v) in map {
        let key = ids.get(key_model, &k).map(str::to_owned).unwrap_or(k);
        if ["invoice-catalog","sales-receipt-catalog","bill-catalog","vendor-defaults"].contains(&id)
            && let Some(acct) = v.get(if ["invoice-catalog","sales-receipt-catalog"].contains(&id){"income_account"}else{"expense_account"})
        {
            let raw = match acct {
                Value::Number(n) => n.to_string(),
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            if let Some(sym) = ids.get("account", &raw) {
                v[if ["invoice-catalog","sales-receipt-catalog"].contains(&id){"income_account"}else{"expense_account"}] = Value::String(sym.to_owned());
            }
        }
        if ["item-catalog", "vendor-catalog"].contains(&id) {
            let field=if id=="item-catalog"{"account"}else{"expense_account"};
            if let Some(raw)=v.get(field){let raw=raw.as_str().map(str::to_owned).unwrap_or_else(||raw.to_string());if let Some(sym)=ids.get("account",&raw){v[field]=Value::String(sym.into());}}
        }
        out.insert(key, canonical(v));
    }
    serde_json::to_string(&out).unwrap()
}

pub fn normalize(html: &str, ids: &IdMap) -> String {
    let html=numbering::without_section(html);
    let html=printing::normalize(&html,ids);
    let html=d9::normalize(&html,ids);
    let html=d3::normalize(&html,ids);
    let html=d7a::normalize(&html,ids);
    let html=d7b::normalize(&html,ids);
    let html=d4::normalize(&html,ids);
    let html=d6::normalize(&html,ids);
    let html=d5::normalize(&html,ids);
    let html=d8::normalize(&html,ids);
    let html=backups::normalize(&html);
    let html=sim_fix::normalize(&html,ids);
    let html=starter_chart::normalize(&html,ids);
    let html=html.as_str();
    let text = TOKEN.replace_all(html, r#"<input type="hidden" name="{form-token}">"#);
    let text = REQUEST_KEY.replace_all(&text, r#"name="request_key" value="{request-key}""#);
    let text = LINK.replace_all(&text, |c: &Captures| format!(r#"{}="{}""#, &c[1], url(&c[2], ids)));
    let text = SELECT.replace_all(&text, |c: &Captures| match select_model(&c[1]) {
        Some(model) => format!(r#"<select name="{}"{}>{}</select>"#, &c[1], &c[2], options(model, &c[3], ids)),
        None => c[0].to_owned(),
    });
    let text = JSON_SCRIPT.replace_all(&text, |c: &Captures| format!(r#"<script id="{}" type="application/json">{}</script>"#, &c[1], json_ids(&c[1], &c[2], ids)));
    let text = AUDIT_PRE.replace_all(&text, |c:&Captures| {
        let plain=c[2].replace("&#x27;","'").replace("&quot;","\"").replace("&lt;","<").replace("&gt;",">").replace("&amp;","&");
        let Some(mut value)=literal::parse(&plain)else{return c[0].to_owned()};
        for key in ["before","after"] {
            if let Some(lines)=value.get_mut(key).and_then(|v|v.get_mut("lines")).and_then(Value::as_array_mut) {
                for line in lines {if let Some(raw)=line.get_mut("account_id") {
                    let id=raw.as_str().map(str::to_owned).unwrap_or_else(||raw.to_string());
                    if let Some(sym)=ids.get("account",&id){*raw=Value::String(sym.into());}
                }}
            }
        }
        // J-ATTACH: an attachment.added / attachment.removed detail names its attachment by id.
        if let Some(raw)=value.get_mut("attachment") {
            let id=raw.as_str().map(str::to_owned).unwrap_or_else(||raw.to_string());
            if let Some(sym)=ids.get("attachment",&id){*raw=Value::String(sym.into());}
        }
        if let Some(raw)=value.get_mut("reversal_id") {
            let id=raw.as_str().map(str::to_owned).unwrap_or_else(||raw.to_string());
            if let Some(sym)=ids.get("journal",&id){*raw=Value::String(sym.into());}
        }
        format!("{}{}{}",&c[1],fb_templates::django::escape(&serde_json::to_string(&canonical(value)).unwrap()),&c[3])
    });
    let text = DIGEST.replace_all(&text, |c:&Captures| match ids.get("proposal_digest",&c[1]){Some(sym)=>format!(r#"name="digest" value="{sym}""#),None=>c[0].to_owned()});
    let text = WHEN.replace_all(&text,|c:&Captures|match ids.get("audit_local_stamp",&c[2]){Some(sym)=>format!("{}{sym}{}",&c[1],&c[3]),None=>c[0].to_owned()});
    let text = EVENT_STAMP.replace_all(&text,|c:&Captures|match ids.get("audit_utc_stamp",&c[2]){Some(sym)=>format!("{}{sym}{}",&c[1],&c[3]),None=>c[0].to_owned()});
    let text = POSTED_STAMP.replace_all(&text,|c:&Captures|match ids.get("posted_stamp",&c[2]){Some(sym)=>format!("{}{sym}{}",&c[1],&c[3]),None=>c[0].to_owned()});
    let text = WS.replace_all(&text, " ");
    let text = BETWEEN_TAGS.replace_all(&text, "><").trim().to_owned();
    if ids.reference { company_documents::django(desktop::django(text)) } else { text }
}

/// Only the first CSV column is a generated audit timestamp. Preserve all CSV bytes otherwise,
/// including BOM, CRLF, quoting, formula guards, column labels and rows.
pub fn normalize_csv(csv: &str, ids: &IdMap) -> String {
    if let Some(csv) = d7b::csv(csv, ids) { return csv; }
    if let Some(csv) = d8::csv(csv, ids) { return csv; }
    csv.split_inclusive('\n').map(|line|{
        match line.split_once(',') {
            Some((stamp,rest))=>match ids.get("audit_local_stamp",stamp){Some(sym)=>format!("{sym},{rest}"),None=>line.to_owned()},
            None=>line.to_owned(),
        }
    }).collect()
}

pub fn normalize_location(location:&str,ids:&IdMap)->String { url(location,ids) }

/// The first differing tag-level token with context, or None when equal.
pub fn first_difference(a: &str, b: &str) -> Option<String> {
    if a == b {
        return None;
    }
    let ta: Vec<&str> = a.split("><").collect();
    let tb: Vec<&str> = b.split("><").collect();
    let i = ta.iter().zip(&tb).position(|(x, y)| x != y).unwrap_or(ta.len().min(tb.len()));
    let ctx = |t: &[&str]| t.iter().skip(i.saturating_sub(1)).take(4).map(|s| format!("<{s}>")).collect::<Vec<_>>().join("\n      ");
    Some(format!("  at token {i} of {} (django) / {} (rust)\n  django: {}\n  rust:   {}", ta.len(), tb.len(), ctx(&ta), ctx(&tb)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const C: &str = "0195b4a4-55af-719e-96cb-a2f946b067bf";
    const A: &str = "0195b4a4-55af-719e-96cb-a2f946b067c0";

    fn maps() -> (IdMap, IdMap) {
        let py: BTreeMap<String, String> = [("company", "1"), ("account:1100", "2"), ("customer:A", "1"), ("item:MOW", "1")].iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        let rs: BTreeMap<String, String> = [("company", C), ("account:1100", A), ("customer:A", "k-u"), ("item:MOW", "i-u")].iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        (IdMap::new(&py), IdMap::new(&rs))
    }

    #[test]
    fn ids_are_rewritten_by_route_argument_not_by_value() {
        let (py, rs) = maps();
        // company 1 and customer 1 share the raw value "1"; only the route position decides.
        let a = normalize(r#"<a href="/c/1/accounts/2/register/">x</a>"#, &py);
        let b = normalize(&format!(r#"<a href="/c/{C}/accounts/{A}/register/">x</a>"#), &rs);
        assert_eq!(a, b);
        assert_eq!(a, r#"<a href="/c/{company}/accounts/{account:1100}/register/">x</a>"#);
        // An id of the wrong model in a route position is left raw, so it shows as a difference.
        assert_ne!(normalize(r#"<a href="/c/1/accounts/1/register/">x</a>"#, &py), a);
    }

    #[test]
    fn select_options_and_json_scripts_map_by_model() {
        let (py, rs) = maps();
        let a = normalize("<select name=\"lines-0-item\" id=\"x\">\n  <option value=\"1\">MOW · #1</option>\n</select><script id=\"invoice-catalog\" type=\"application/json\">{\"1\": {\"income_account\": 2, \"rate\": \"45.00\"}}</script>", &py);
        let b = normalize("<select name=\"lines-0-item\" id=\"x\"><option value=\"i-u\">MOW · #i-u</option></select><script id=\"invoice-catalog\" type=\"application/json\">{\"i-u\": {\"rate\": \"45.00\", \"income_account\": \"0195b4a4-55af-719e-96cb-a2f946b067c0\"}}</script>", &rs);
        assert_eq!(a, b);
        // A customer select does not map item ids.
        assert!(normalize(r#"<select name="customer"><option value="i-u">x</option></select>"#, &rs).contains(r#"value="i-u""#));
    }

    #[test]
    fn tokens_request_keys_and_whitespace() {
        let (py, rs) = maps();
        let a = normalize("<form>\n<input type=\"hidden\" name=\"csrfmiddlewaretoken\" value=\"abcDEF123\">\n  <input type=\"hidden\" name=\"request_key\" value=\"bf2cb4a4-55af-419e-96cb-a2f946b067bf\" id=\"id_request_key\"></form>", &py);
        let b = normalize("<form><input type=\"hidden\" name=\"fb_command_token\" value=\"00ff\"><input type=\"hidden\" name=\"request_key\" value=\"0195b4a4-55af-719e-96cb-a2f946b067bf\" id=\"id_request_key\"></form>", &rs);
        assert_eq!(a, b);
        assert!(first_difference(&a, &b).is_none());
        assert!(first_difference("<a><b>", "<a><c>").unwrap().contains("token 1"));
    }

    #[test]
    fn posted_at_maps_only_a_posted_journal_stamp_in_the_posted_sentence() {
        let side = |stamp: &str| IdMap::new(&BTreeMap::from([("posted_stamp:J000328".into(), stamp.into())]));
        let page = |stamp: &str| format!(r#"<p class="muted">Posted by owner at {stamp}.</p>"#);
        let a = normalize(&page("2026-11-05 16:35:12 UTC"), &side("2026-11-05 16:35:12 UTC"));
        assert_eq!(a, normalize(&page("2026-11-05 16:40:47 UTC"), &side("2026-11-05 16:40:47 UTC")));
        // A stamp no posted journal carries, another actor, or the same stamp outside the sentence must DIFF.
        assert_ne!(a, normalize(&page("2026-11-05 16:41:00 UTC"), &side("2026-11-05 16:40:47 UTC")));
        assert_ne!(a, normalize(&page("2026-11-05 16:40:47 UTC").replace("owner", "other"), &side("2026-11-05 16:40:47 UTC")));
        let outside = "<p>Dated 2026-11-05 16:40:47 UTC.</p>";
        assert_eq!(normalize(outside, &side("2026-11-05 16:40:47 UTC")), outside);
    }

    #[test]
    fn ordinal_labels_are_tied_to_the_option_identity_and_do_not_hide_a_wrong_ordinal() {
        let py=BTreeMap::from([("item:MOW".into(),"8".into()),("label:item:MOW".into(),"8".into())]);
        let rs=BTreeMap::from([("item:MOW".into(),A.into()),("label:item:MOW".into(),"2".into())]);
        let a=normalize(r#"<select name="lines-0-item"><option value="8">MOW #8 · #8</option></select>"#,&IdMap::new(&py));
        let correct=format!(r#"<select name="lines-0-item"><option value="{A}">MOW #8 · #2</option></select>"#);
        assert_eq!(a,normalize(&correct,&IdMap::new(&rs)));
        assert_ne!(a,normalize(&correct.replace("#2","#3"),&IdMap::new(&rs)));
    }

    #[test]
    fn only_typed_audit_fields_are_normalized_never_memos_or_other_pre_elements() {
        let (py,rs)=maps();
        let wrap=|body:&str|format!("<details><summary>View change</summary><pre>{}</pre></details>",fb_templates::django::escape(body));
        let a=wrap("{'after': {'lines': [{'account_id': 2}], 'memo': \"'account_id': 2\"}}");
        let b=wrap(&format!("{{'after': {{'lines': [{{'account_id': '{A}'}}], 'memo': \"'account_id': 2\"}}}}"));
        assert_eq!(normalize(&a,&py),normalize(&b,&rs));
        assert_ne!(normalize(&a,&py),normalize(&b.replace("2&quot;","3&quot;"),&rs));
        let ordinary=format!("<pre>{}</pre>",fb_templates::django::escape("{'account_id': 2}"));
        assert_eq!(normalize(&ordinary,&py),ordinary);
    }

    #[test]
    fn an_attachment_event_maps_only_its_own_attachment_id() {
        let py=IdMap::new(&BTreeMap::from([("attachment:opening.pdf".into(),"3".into())]));
        let rs=IdMap::new(&BTreeMap::from([("attachment:opening.pdf".into(),A.into())]));
        let wrap=|body:&str|format!("<details><summary>View change</summary><pre>{}</pre></details>",fb_templates::django::escape(body));
        let a=normalize(&wrap(r#"{"attachment":3,"name":"opening.pdf","size":75}"#),&py);
        assert_eq!(a,normalize(&wrap(&format!(r#"{{"attachment":"{A}","name":"opening.pdf","size":75}}"#)),&rs));
        // Another attachment's id, or a changed name or size, must DIFF.
        assert_ne!(a,normalize(&wrap(&format!(r#"{{"attachment":"{C}","name":"opening.pdf","size":75}}"#)),&rs));
        assert_ne!(a,normalize(&wrap(&format!(r#"{{"attachment":"{A}","name":"other.pdf","size":75}}"#)),&rs));
        assert_ne!(a,normalize(&wrap(&format!(r#"{{"attachment":"{A}","name":"opening.pdf","size":76}}"#)),&rs));
    }
}

#[cfg(test)]
mod label_regressions {
    use super::*;
    const UUID:&str="0195b4a4-55af-719e-96cb-a2f946b067bf";
    #[test]
    fn user_hash_text_in_account_and_customer_labels_must_diff() {
        let py=IdMap::new(&BTreeMap::from([("account:4000".into(),"1".into()),("customer:Typed".into(),"7".into())]));
        let rs=IdMap::new(&BTreeMap::from([("account:4000".into(),UUID.into()),("customer:Typed".into(),"3".into())]));
        let wrap=|value:&str,label:&str,name:&str|format!(r#"<select name="{name}"><option value="{value}">{label}</option></select>"#);
        assert_ne!(normalize(&wrap("1","Sales #1","account"),&py),normalize(&wrap(UUID,&format!("Sales #{UUID}"),"account"),&rs));
        assert_ne!(normalize(&wrap("7","Typed #7","customer"),&py),normalize(&wrap("3","Typed #3","customer"),&rs));
        assert_ne!(normalize(&wrap("7","Typed (#7)","customer"),&py),normalize(&wrap("3","Typed (#3)","customer"),&rs));
        assert_eq!(normalize(&wrap("1","Sales #1 · #1","account"),&py),normalize(&wrap(UUID,&format!("Sales #1 · #{UUID}"),"account"),&rs));
        assert_ne!(normalize(&wrap("1","Sales #1 · #1","account"),&py),normalize(&wrap(UUID,&format!("Sales #{UUID} · #{UUID}"),"account"),&rs));
    }
}

#[cfg(test)]
mod card_number_regressions {
    use super::*;
    use fb_core::card_numbers::{CARD_NUMBER, mask_card_numbers};
    const PAN: &str = "4111 1111 1111 1111";
    #[test]
    fn card_number_refusal_and_masking_must_diff() {
        for pan in [PAN, "4111    1111 1111 1111", "4111        1111 1111 1111"] {
            let ids = IdMap::new(&BTreeMap::new());
            let input = |value: &str| format!(r#"<p><label for="id_reference">Reference:</label><input type="text" name="reference" value="{value}" maxlength="120" id="id_reference"></p>"#);
            let refused = format!(r#"<ul class="errorlist" id="id_reference_error"><li>{}</li></ul>{}"#, fb_templates::django::escape(CARD_NUMBER), input(pan));
            assert_ne!(normalize(&input(pan), &ids), normalize(&refused, &ids));
            let pre = |memo: &str| format!("<details><summary>View change</summary><pre>{}</pre></details>", fb_templates::django::escape(&format!("{{'memo': '{memo}'}}")));
            assert_ne!(normalize(&pre(pan), &ids), normalize(&pre(&mask_card_numbers(pan)), &ids));
            assert_ne!(normalize_csv(&format!("When,Reason\r\n2026-11-05,{pan}\r\n"), &ids), normalize_csv(&format!("When,Reason\r\n2026-11-05,{}\r\n", mask_card_numbers(pan)), &ids));
        }
        // Text without a card number is untouched, so ordinary screens still match exactly.
        assert_eq!(mask_card_numbers("Check 1042 · card ending 1111"), "Check 1042 · card ending 1111");
    }
}
