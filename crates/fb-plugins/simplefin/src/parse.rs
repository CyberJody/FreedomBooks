//! Pure Account Set normalization; no I/O and no floating-point money.
use chrono::{DateTime, Datelike, TimeZone, Utc};
use chrono_tz::Tz;
use fb_plugin_api::{
    Amount, Date, DateWindow, FeedAccount, FeedAccountMetadata, FeedCompleteness, FeedResult,
    FeedTransaction, FeedWarning, FeedWarningCode, PluginError, PluginResult, Timestamp,
};
use regex::{Captures, Regex};
use serde_json::{value::RawValue, Value};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::LazyLock,
};

pub const NAME_LIMIT: usize = 120;
pub const DESCRIPTION_LIMIT: usize = 500;
pub const MESSAGE_MAX: usize = 300;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MoneyError {
    Missing,
    NotNumeric,
    NotFinite,
    TooLarge,
}
impl MoneyError {
    /// Python `money()`'s reason string.
    pub fn reason(self) -> &'static str {
        match self {
            MoneyError::Missing => "amount missing",
            MoneyError::NotNumeric => "amount not numeric",
            MoneyError::NotFinite => "amount not finite",
            MoneyError::TooLarge => "amount too large",
        }
    }
}

/// Python Decimal / ROUND_HALF_UP, with the original pre-round magnitude limit.
/// Wire numbers are retained lexically by parse_account_set_json before this step.
/// Callers with a Value can only supply the precision that Value already retains.
pub fn money(value: &Value) -> Result<i64, MoneyError> {
    let owned;
    let raw = match value {
        Value::String(s) => s.trim(),
        Value::Number(n) => {
            owned = n.to_string();
            &owned
        }
        Value::Null | Value::Bool(_) => return Err(MoneyError::Missing),
        _ => return Err(MoneyError::NotNumeric),
    };
    let raw = raw.replace('_', ""); // Decimal accepts separators, including strings from v1 servers.
    let (negative, raw) = match raw.as_bytes().first() {
        Some(b'-') => (true, &raw[1..]),
        Some(b'+') => (false, &raw[1..]),
        _ => (false, raw.as_str()),
    };
    let lower = raw.to_ascii_lowercase();
    if lower == "inf"
        || lower == "infinity"
        || lower
            .strip_prefix("nan")
            .or_else(|| lower.strip_prefix("snan"))
            .is_some_and(|digits| digits.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(MoneyError::NotFinite);
    }
    let mut parts = raw.split(['e', 'E']);
    let mantissa = parts.next().ok_or(MoneyError::NotNumeric)?;
    let exponent = match parts.next() {
        None => 0i64,
        Some(e) => {
            let (minus, e) = match e.as_bytes().first() {
                Some(b'-') => (true, &e[1..]),
                Some(b'+') => (false, &e[1..]),
                _ => (false, e),
            };
            if e.is_empty() || !e.bytes().all(|b| b.is_ascii_digit()) {
                return Err(MoneyError::NotNumeric);
            }
            let n = e.bytes().fold(0i64, |n, d| {
                (n * 10 + i64::from(d - b'0')).min(1_000_000_000)
            });
            if minus {
                -n
            } else {
                n
            }
        }
    };
    if parts.next().is_some() {
        return Err(MoneyError::NotNumeric);
    }
    let mut digits = Vec::with_capacity(mantissa.len());
    let mut fractional = 0i64;
    let mut dot = false;
    for byte in mantissa.bytes() {
        if byte == b'.' && !dot {
            dot = true;
        } else if byte.is_ascii_digit() {
            digits.push(byte - b'0');
            if dot {
                fractional += 1;
            }
        } else {
            return Err(MoneyError::NotNumeric);
        }
    }
    if digits.is_empty() {
        return Err(MoneyError::NotNumeric);
    }
    let Some(first) = digits.iter().position(|d| *d != 0) else {
        return Ok(0);
    };
    let digits = &digits[first..];
    let integer_places = digits.len() as i64 - fractional + exponent;
    if integer_places > 14 {
        return Err(MoneyError::TooLarge);
    }
    // The maximum is 99999999999999.99 BEFORE rounding, not merely 10^14.
    if integer_places == 14
        && digits.len() > 16
        && digits[..16].iter().all(|d| *d == 9)
        && digits[16..].iter().any(|d| *d != 0)
    {
        return Err(MoneyError::TooLarge);
    }
    let places = integer_places + 2;
    let mut cents = 0i64;
    if places > 0 {
        for index in 0..places as usize {
            cents = cents * 10 + i64::from(*digits.get(index).unwrap_or(&0));
        }
    }
    if places >= 0 && digits.get(places as usize).is_some_and(|d| *d >= 5) {
        cents += 1;
    }
    Ok(if negative { -cents } else { cents })
}

pub fn epoch(value: &Value) -> Option<i64> {
    let number = match value {
        Value::String(s) => s.trim().replace('_', "").parse::<i64>().ok()?,
        Value::Number(n) => {
            if let Some(n) = n.as_i64() {
                n
            } else {
                let n = n.to_string().parse::<f64>().ok()?;
                if !n.is_finite() || !(0.0..=4_102_444_800.0).contains(&n.trunc()) {
                    return None;
                }
                n as i64
            }
        }
        _ => return None,
    };
    (number > 0 && number <= 4_102_444_800).then_some(number)
}

static TAGS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]*>").unwrap());
static URLS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)\bhttps?://[^\s<>"']+"#).unwrap());
// Both credential patterns take `\s*` where Python has `\s+`: a fold (see `Folded`) also separates
// `basic` from its token, so the separator is checked by hand in `redact_folded`.
static BASIC_HEADER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(authorization\s*[:=]\s*basic\s*)[A-Za-z0-9+/=._~-]+").unwrap()
});
static BASIC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(basic\s*)([A-Za-z0-9+/]{8,}={0,2})").unwrap());
static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\A\w\z").unwrap());

/// Characters that print as nothing: C0 controls that are not whitespace, DEL, and the zero-width
/// space, joiners, word joiner and BOM. Fuzz audit F-04: the credential patterns used to run over them
/// (the controls were blanked only afterwards), so `Basic\0 <token>`, `Auth\0orization: Basic <token>`
/// or a token split by `\u{200b}` kept the token.
pub fn is_hidden(c: char) -> bool {
    (c <= '\u{1f}' && !c.is_whitespace())
        || c == '\u{7f}'
        || matches!(c, '\u{200b}'..='\u{200d}' | '\u{2060}' | '\u{feff}')
}

/// Text with its hidden characters folded out for the credential patterns: `hidden[k]` holds the ones
/// that stood before `chars[k]` (`hidden[chars.len()]` the trailing ones). Keywords and tokens match
/// across a fold, and a fold also counts as the separator after `basic` (and as `\b` before it).
/// Everything outside a redaction unfolds to exactly the input; hidden characters inside a redacted
/// token go with it.
struct Folded {
    chars: Vec<char>,
    hidden: Vec<String>,
}

impl Folded {
    fn new(text: &str) -> Self {
        let mut out = Folded { chars: Vec::with_capacity(text.len()), hidden: vec![String::new()] };
        for c in text.chars() {
            if is_hidden(c) {
                out.hidden.last_mut().unwrap().push(c);
            } else {
                out.chars.push(c);
                out.hidden.push(String::new());
            }
        }
        out
    }

    fn unfold(&self) -> String {
        let mut out = String::with_capacity(self.chars.len());
        for (k, c) in self.chars.iter().enumerate() {
            out.push_str(&self.hidden[k]);
            out.push(*c);
        }
        out.push_str(&self.hidden[self.chars.len()]);
        out
    }
}

/// `re.sub(pattern, r'\1***', text)` over folded text, for the matches `accept` keeps (it gets the
/// match, the char index where it starts, the char index where the token starts, and the fold flags).
/// A refused candidate resumes the scan one character later, as a failed regex match would.
fn redact_folded(
    f: &Folded,
    re: &Regex,
    accept: impl Fn(&Captures<'_>, usize, usize, &[bool]) -> bool,
) -> Folded {
    let text: String = f.chars.iter().collect();
    let glue: Vec<bool> = f.hidden.iter().map(|h| !h.is_empty()).collect();
    let starts: Vec<usize> = text.char_indices().map(|(b, _)| b).chain([text.len()]).collect();
    let index = |byte: usize| starts.binary_search(&byte).unwrap();
    let mut out = Folded { chars: Vec::with_capacity(f.chars.len()), hidden: vec![String::new()] };
    let keep = |out: &mut Folded, k: usize| {
        out.hidden.last_mut().unwrap().push_str(&f.hidden[k]);
        out.chars.push(f.chars[k]);
        out.hidden.push(String::new());
    };
    let (mut done, mut at) = (0, 0);
    while at < text.len() {
        let Some(c) = re.captures_at(&text, at) else { break };
        let (whole, prefix) = (c.get(0).unwrap(), c.get(1).unwrap());
        let (start, token) = (index(whole.start()), index(prefix.end()));
        if !accept(&c, start, token, &glue) {
            at = starts[index(whole.start()) + 1];
            continue;
        }
        for k in done..token {
            keep(&mut out, k);
        }
        out.hidden.last_mut().unwrap().push_str(&f.hidden[token]);
        for c in "***".chars() {
            out.chars.push(c);
            out.hidden.push(String::new());
        }
        done = index(whole.end());
        at = whole.end();
    }
    for k in done..f.chars.len() {
        keep(&mut out, k);
    }
    out.hidden.last_mut().unwrap().push_str(&f.hidden[f.chars.len()]);
    out
}

/// `basic` and its token are separated by whitespace or a fold.
fn separated(c: &Captures<'_>, token: usize, glue: &[bool]) -> bool {
    c[1].ends_with(char::is_whitespace) || glue[token]
}

pub fn redact(text: &str) -> String {
    let urls = URLS.replace_all(text, |c: &Captures<'_>| {
        let raw = &c[0];
        match raw.rsplit_once('@') {
            Some((_, tail)) => format!("{}://***@{tail}", raw.split_once("://").unwrap().0),
            None => raw.to_owned(),
        }
    });
    let folded = Folded::new(&urls);
    let header = redact_folded(&folded, &BASIC_HEADER, |c, _, token, glue| separated(c, token, glue));
    let basic = redact_folded(&header, &BASIC, |c, start, token, glue| {
        let boundary = start == 0
            || glue[start]
            || !WORD.is_match(header.chars[start - 1].encode_utf8(&mut [0; 4]));
        boundary
            && separated(c, token, glue)
            && c[2]
                .bytes()
                .any(|b| b.is_ascii_digit() || b"+/=".contains(&b))
    });
    basic
        .unfold()
        .chars()
        .map(|c| {
            if c <= '\u{1f}' || c == '\u{7f}' {
                ' '
            } else {
                c
            }
        })
        .collect()
}

pub fn sanitize(text: &str, limit: usize) -> String {
    redact(&TAGS.replace_all(text, ""))
        .trim()
        .chars()
        .take(limit)
        .collect()
}

fn text(value: &Value, limit: usize) -> String {
    match value {
        Value::String(s) => sanitize(s, limit),
        Value::Number(n) => sanitize(&n.to_string(), limit),
        _ => String::new(),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SkipReason {
    AccountNotObject,
    AccountWithoutId,
    AccountWithoutCurrency,
    AccountBalance(MoneyError),
    TransactionsNotList,
    TransactionNotObject,
    TransactionWithoutId,
    TransactionAmount(MoneyError),
}

// Deliberately no Debug/Serialize: these contain bank data, not log messages.
pub struct ProtocolError {
    pub code: String,
    pub message: String,
    pub connection_id: String,
    pub account_id: String,
    /// Index in errlist, or errors when the v1 fallback supplied this diagnostic.
    pub source_index: usize,
}
pub struct ParsedTransaction {
    pub remote_id: String,
    pub posted_epoch: Option<i64>,
    pub transacted_epoch: Option<i64>,
    pub cents: i64,
    pub description: String,
    pub pending: bool,
}
pub struct ParsedAccount {
    pub connection_key: String,
    pub remote_id: String,
    pub name: String,
    pub organization_name: String,
    pub organization_remote_id: String,
    pub currency: String,
    pub balance_cents: i64,
    pub available_cents: Option<i64>,
    pub balance_epoch: Option<i64>,
    pub transactions: Vec<ParsedTransaction>,
    pub skipped: Vec<SkipReason>,
    /// `skipped` in Python's words (`parse_account_set`'s reason strings, with the provider ids).
    pub skipped_text: Vec<String>,
}
pub struct AccountSet {
    pub accounts: Vec<ParsedAccount>,
    pub errors: Vec<ProtocolError>,
    pub skipped: Vec<SkipReason>,
    /// `skipped` in Python's words (`parse_account_set`'s reason strings, with the provider ids).
    pub skipped_text: Vec<String>,
    // Inventory includes identifiable accounts even when their rows failed normalization.
    present_account_ids: Vec<String>,
    warnings: Vec<FeedWarning>,
}
impl AccountSet {
    pub fn feed_accounts(&self) -> Vec<FeedAccount> {
        self.accounts
            .iter()
            .map(|a| FeedAccount {
                remote_id: a.remote_id.clone(),
                name: a.name.clone(),
                currency: a.currency.clone(),
                balance: Some(Amount {
                    cents: a.balance_cents,
                    currency: a.currency.clone(),
                }),
            })
            .collect()
    }
    pub fn feed_transactions(&self, zone: Tz) -> Vec<FeedTransaction> {
        self.accounts
            .iter()
            .flat_map(|a| {
                a.transactions.iter().map(move |t| FeedTransaction {
                    account_remote_id: a.remote_id.clone(),
                    remote_id: t.remote_id.clone(),
                    posted_on: to_date(t.posted_epoch, zone),
                    transacted_on: to_date(t.transacted_epoch, zone),
                    amount: Amount {
                        cents: t.cents,
                        currency: a.currency.clone(),
                    },
                    description: t.description.clone(),
                    pending: t.pending,
                })
            })
            .collect()
    }
    /// Provider-neutral envelope; raw diagnostics remain only in the protocol parser.
    pub(crate) fn feed_result<T>(&self, items: Vec<T>) -> FeedResult<T> {
        FeedResult {
            items,
            completeness: if self.warnings.is_empty() {
                FeedCompleteness::Complete
            } else {
                FeedCompleteness::Partial
            },
            // A missing/non-list accounts field is rejected by parse_account_set, never
            // converted to an empty inventory. An explicit [] is known-empty.
            present_account_ids: Some(self.present_account_ids.clone()),
            account_metadata: self
                .accounts
                .iter()
                .map(|a| FeedAccountMetadata {
                    account_remote_id: a.remote_id.clone(),
                    organization_remote_id: nonempty(&a.organization_remote_id),
                    organization_name: nonempty(&a.organization_name),
                    connection_remote_id: nonempty(&a.connection_key),
                    available_balance: a.available_cents.map(|cents| Amount {
                        cents,
                        currency: a.currency.clone(),
                    }),
                    balance_at: a
                        .balance_epoch
                        .and_then(|n| u64::try_from(n).ok())
                        .map(Timestamp),
                })
                .collect(),
            warnings: self.warnings.clone(),
        }
    }
    /// Legacy vectors cannot return warnings. Fail closed rather than hide partial feeds.
    pub(crate) fn require_complete(&self) -> PluginResult<()> {
        if self.errors.iter().any(|e| e.code.starts_with("gen.auth")) {
            return Err(PluginError::NeedsSignIn);
        }
        if !self.errors.is_empty()
            || !self.skipped.is_empty()
            || self.accounts.iter().any(|a| !a.skipped.is_empty())
        {
            return Err(PluginError::InvalidResponse);
        }
        Ok(())
    }
}

/// Decode wire amounts without a float round trip or a workspace-wide serde_json
/// arbitrary_precision feature. The existing exact decimal parser also handles
/// coefficients/exponents beyond rust_decimal's representable precision.
pub fn parse_account_set_json(raw: &[u8]) -> PluginResult<AccountSet> {
    let raw: &RawValue = serde_json::from_slice(raw).map_err(|_| PluginError::InvalidResponse)?;
    let value = wire_value(raw, WireContext::Root).map_err(|_| PluginError::InvalidResponse)?;
    parse_account_set(&value)
}

#[derive(Clone, Copy)]
enum WireContext {
    Root,
    Accounts,
    Account,
    Transactions,
    Transaction,
    Amount,
    Other,
}

fn wire_value(raw: &RawValue, context: WireContext) -> serde_json::Result<Value> {
    use WireContext::*;
    let text = raw.get();
    match (context, text.as_bytes()[0]) {
        (Amount, b'-' | b'0'..=b'9') => Ok(Value::String(text.to_owned())),
        (Root | Account | Transaction, b'{') => {
            let fields: BTreeMap<String, &RawValue> = serde_json::from_str(text)?;
            fields
                .into_iter()
                .map(|(key, value)| {
                    let next = match (context, key.as_str()) {
                        (Root, "accounts") => Accounts,
                        (Account, "balance" | "available-balance") | (Transaction, "amount") => {
                            Amount
                        }
                        (Account, "transactions") => Transactions,
                        _ => Other,
                    };
                    Ok((key, wire_value(value, next)?))
                })
                .collect::<serde_json::Result<serde_json::Map<String, Value>>>()
                .map(Value::Object)
        }
        (Accounts | Transactions, b'[') => {
            let values: Vec<&RawValue> = serde_json::from_str(text)?;
            let next = if matches!(context, Accounts) {
                Account
            } else {
                Transaction
            };
            values
                .into_iter()
                .map(|value| wire_value(value, next))
                .collect::<serde_json::Result<Vec<Value>>>()
                .map(Value::Array)
        }
        _ => serde_json::from_str(text),
    }
}

/// The Account Set's `connections` by sanitized `conn_id`, built once per response: an account's
/// connection is the last object in `connections` whose non-null `conn_id` sanitizes to the same text
/// as the account's own `conn_id` (absent or non-text ids sanitize to `""` on both sides). Fuzz audit
/// F-01: the scan used to re-sanitize both ids for every account x connection pair, so 6,000 x 6,000
/// (a 506 KiB body, well inside the 20 MiB cap) took minutes.
#[doc(hidden)]
pub fn connections_by_id(data: &Value) -> HashMap<String, &Value> {
    let mut index = HashMap::new();
    for c in data["connections"].as_array().into_iter().flatten() {
        if c.is_object() && !c["conn_id"].is_null() {
            // Later entries win, as the old `rev().find` did.
            index.insert(text(&c["conn_id"], 200), c);
        }
    }
    index
}

pub fn parse_account_set(data: &Value) -> PluginResult<AccountSet> {
    let raw_accounts = data
        .get("accounts")
        .and_then(Value::as_array)
        .ok_or(PluginError::InvalidResponse)?;
    let errors = errors_from(data);
    let warnings = errors
        .iter()
        .map(|e| FeedWarning {
            code: if e.code.starts_with("gen.auth") {
                FeedWarningCode::AuthenticationRequired
            } else {
                // Do not guess semantics from arbitrary provider strings.
                FeedWarningCode::Other
            },
            account_remote_id: nonempty(&e.account_id),
            transaction_remote_id: None,
            connection_remote_id: nonempty(&e.connection_id),
            source_index: Some(e.source_index),
        })
        .collect();
    let mut result = AccountSet {
        accounts: vec![],
        errors,
        skipped: vec![],
        skipped_text: vec![],
        present_account_ids: vec![],
        warnings,
    };
    let mut seen_account_ids = HashSet::new();
    let connections = connections_by_id(data);
    for (index, raw) in raw_accounts.iter().enumerate() {
        let org = &raw["org"];
        let first = |values: &[&Value], limit| {
            values
                .iter()
                .map(|v| text(v, limit))
                .find(|s| !s.is_empty())
                .unwrap_or_default()
        };
        let connection_key = first(&[&raw["conn_id"], &org["id"], &org["domain"]], 200);
        let warning = |code| FeedWarning {
            code,
            account_remote_id: nonempty(&text(&raw["id"], 200)),
            transaction_remote_id: None,
            connection_remote_id: nonempty(&connection_key),
            source_index: Some(index),
        };
        if !raw.is_object() {
            result.skipped.push(SkipReason::AccountNotObject);
            result.skipped_text.push("account entry is not an object".into());
            result
                .warnings
                .push(warning(FeedWarningCode::InvalidAccount));
            continue;
        }
        let remote_id = text(&raw["id"], 200);
        if remote_id.is_empty() {
            result.skipped.push(SkipReason::AccountWithoutId);
            result.skipped_text.push("account without an id".into());
            result
                .warnings
                .push(warning(FeedWarningCode::MissingIdentifier));
            continue;
        }
        if seen_account_ids.insert(remote_id.clone()) {
            result.present_account_ids.push(remote_id.clone());
        }
        let currency = text(&raw["currency"], 200);
        if currency.is_empty() {
            result.skipped.push(SkipReason::AccountWithoutCurrency);
            result.skipped_text.push(format!("account {remote_id}: no currency"));
            result
                .warnings
                .push(warning(FeedWarningCode::UnsupportedCurrency));
            continue;
        }
        let balance = match money(&raw["balance"]) {
            Ok(v) => v,
            Err(why) => {
                result.skipped.push(SkipReason::AccountBalance(why));
                result.skipped_text.push(format!("account {remote_id}: balance {}", why.reason()));
                result
                    .warnings
                    .push(warning(FeedWarningCode::InvalidAmount));
                continue;
            }
        };
        let available = if raw["available-balance"].is_null() {
            Some(balance)
        } else {
            money(&raw["available-balance"]).ok()
        };
        let conn = connections
            .get(&text(&raw["conn_id"], 200))
            .copied()
            .unwrap_or(&Value::Null);
        let name = text(&raw["name"], NAME_LIMIT);
        let mut account = ParsedAccount {
            organization_remote_id: text(&org["id"], 200),
            connection_key: connection_key.clone(),
            organization_name: first(
                &[
                    &conn["org_name"],
                    &conn["name"],
                    &raw["conn_name"],
                    &org["name"],
                    &org["domain"],
                ],
                NAME_LIMIT,
            ),
            name: if name.is_empty() {
                remote_id.clone()
            } else {
                name
            },
            remote_id,
            currency,
            balance_cents: balance,
            available_cents: available,
            balance_epoch: epoch(&raw["balance-date"]),
            transactions: vec![],
            skipped: vec![],
            skipped_text: vec![],
        };
        if let Some(transactions) = raw["transactions"].as_array() {
            for (transaction_index, entry) in transactions.iter().enumerate() {
                let transaction_warning = |code| FeedWarning {
                    code,
                    account_remote_id: Some(account.remote_id.clone()),
                    transaction_remote_id: nonempty(&text(&entry["id"], 200)),
                    connection_remote_id: nonempty(&account.connection_key),
                    source_index: Some(transaction_index),
                };
                if !entry.is_object() {
                    account.skipped.push(SkipReason::TransactionNotObject);
                    account.skipped_text.push("transaction entry is not an object".into());
                    result
                        .warnings
                        .push(transaction_warning(FeedWarningCode::InvalidTransaction));
                    continue;
                }
                let remote_id = text(&entry["id"], 200);
                if remote_id.is_empty() {
                    account.skipped.push(SkipReason::TransactionWithoutId);
                    account.skipped_text.push("transaction without an id".into());
                    result
                        .warnings
                        .push(transaction_warning(FeedWarningCode::MissingIdentifier));
                    continue;
                }
                let cents = match money(&entry["amount"]) {
                    Ok(v) => v,
                    Err(why) => {
                        account.skipped.push(SkipReason::TransactionAmount(why));
                        account.skipped_text.push(format!("transaction {remote_id}: {}", why.reason()));
                        result
                            .warnings
                            .push(transaction_warning(FeedWarningCode::InvalidAmount));
                        continue;
                    }
                };
                let posted = epoch(&entry["posted"]);
                account.transactions.push(ParsedTransaction {
                    remote_id,
                    cents,
                    posted_epoch: posted,
                    transacted_epoch: epoch(&entry["transacted_at"]),
                    description: text(&entry["description"], DESCRIPTION_LIMIT),
                    pending: entry["pending"] == Value::Bool(true) || posted.is_none(),
                });
            }
        } else if !raw["transactions"].is_null()
            && raw["transactions"] != Value::Bool(false)
            && raw["transactions"] != Value::String(String::new())
            && raw["transactions"] != 0
            && raw["transactions"] != serde_json::json!({})
        {
            account.skipped.push(SkipReason::TransactionsNotList);
            account.skipped_text.push("transactions is not a list".into());
            result
                .warnings
                .push(warning(FeedWarningCode::InvalidTransaction));
        }
        result.accounts.push(account);
    }
    Ok(result)
}

fn nonempty(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_owned())
}

fn errors_from(data: &Value) -> Vec<ProtocolError> {
    let mut errors = Vec::new();
    if let Some(items) = data["errlist"].as_array() {
        for (source_index, item) in items.iter().enumerate().filter(|(_, v)| v.is_object()) {
            let message = text(&item["msg"], MESSAGE_MAX);
            errors.push(ProtocolError {
                source_index,
                code: text(&item["code"], 40),
                message: if message.is_empty() {
                    "SimpleFIN reported a problem.".into()
                } else {
                    message
                },
                connection_id: text(&item["conn_id"], 200),
                account_id: text(&item["account_id"], 200),
            });
        }
    }
    if errors.is_empty() {
        if let Some(items) = data["errors"].as_array() {
            for (source_index, item) in items
                .iter()
                .enumerate()
                .filter_map(|(i, v)| v.as_str().map(|s| (i, s)))
            {
                errors.push(ProtocolError {
                    source_index,
                    code: String::new(),
                    message: sanitize(item, MESSAGE_MAX),
                    connection_id: String::new(),
                    account_id: String::new(),
                });
            }
        }
    }
    errors
}

/// Python `_epoch_to_date` as ISO text: the UTC instant's calendar date in the book's display zone.
pub fn local_date(epoch: Option<i64>, zone: Tz) -> Option<String> {
    to_date(epoch, zone).map(|d| {
        let (y, m, day) = d.parts();
        format!("{y:04}-{m:02}-{day:02}")
    })
}

fn to_date(epoch: Option<i64>, zone: Tz) -> Option<Date> {
    let date = DateTime::<Utc>::from_timestamp(epoch?, 0)?.with_timezone(&zone);
    Date::new(
        date.year().try_into().ok()?,
        date.month() as u8,
        date.day() as u8,
    )
    .ok()
}

/// Inclusive local calendar days; DST-aware and with no invented dates on bad input.
pub fn window_epochs(window: DateWindow, zone: Tz) -> PluginResult<(i64, i64)> {
    let midnight = |d: Date| {
        let (y, m, d) = d.parts();
        zone.with_ymd_and_hms(i32::from(y), u32::from(m), u32::from(d), 0, 0, 0)
            .single()
            .map(|d| d.timestamp())
            .ok_or(PluginError::InvalidConfiguration)
    };
    let start = midnight(window.start)?;
    let (y, m, d) = window.end.parts();
    let next = chrono::NaiveDate::from_ymd_opt(i32::from(y), u32::from(m), u32::from(d))
        .and_then(|d| d.succ_opt())
        .ok_or(PluginError::InvalidConfiguration)?;
    let end = midnight(Date::new(
        next.year()
            .try_into()
            .map_err(|_| PluginError::InvalidConfiguration)?,
        next.month() as u8,
        next.day() as u8,
    )?)? - 1;
    if start < 0 || start > end || end > 4_102_444_800 {
        return Err(PluginError::InvalidConfiguration);
    }
    Ok((start, end))
}
