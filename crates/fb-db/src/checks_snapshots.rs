//! Typed, fallible decoding shared by storage, SQL predicates and the comparer.
use rusqlite::{Connection, functions::FunctionFlags};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Account {
    id: String,
    code: String,
    name: String,
    kind: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Company {
    id: String,
    name: String,
    currency: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Party {
    id: String,
    display_name: String,
    business_name: String,
    contact_name: String,
    billing_address: String,
    email: String,
    phone: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Other {
    display_name: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Item {
    id: String,
    code: String,
    description: String,
    kind: String,
    purchase_cost: Option<String>,
    expense_account_id: Option<String>,
}

/// Canonical nonnegative, two-place money, bounded to the document domain.
pub fn money_text(s: &str) -> bool {
    let Some((whole, fraction)) = s.split_once('.') else {
        return false;
    };
    !whole.is_empty()
        && whole.len() <= 14
        && fraction.len() == 2
        && (whole.len() == 1 || !whole.starts_with('0'))
        && whole
            .bytes()
            .chain(fraction.bytes())
            .all(|b| b.is_ascii_digit())
}

fn same(actual: &str, expected: Option<&str>) -> bool {
    !actual.is_empty() && expected.is_none_or(|id| actual == id)
}

/// `expected` is the relational identity, or None for an unlinked empty snapshot.
/// Company snapshots may be decoded without a parent identity by the row adapter;
/// the SQL boundary and comparer additionally bind them to their company.
pub fn valid(kind: &str, value: &Value, expected: Option<&str>) -> bool {
    match kind {
        "account" | "item" if expected.is_none() => value.as_object().is_some_and(|m| m.is_empty()),
        "account" => serde_json::from_value::<Account>(value.clone()).is_ok_and(|v| {
            same(&v.id, expected)
                && !v.code.is_empty()
                && !v.name.is_empty()
                && matches!(
                    v.kind.as_str(),
                    "ASSET" | "LIABILITY" | "EQUITY" | "INCOME" | "EXPENSE"
                )
        }),
        "company" => serde_json::from_value::<Company>(value.clone()).is_ok_and(|v| {
            same(&v.id, expected)
                && !v.name.is_empty()
                && v.currency.len() == 3
                && v.currency.bytes().all(|b| b.is_ascii_uppercase())
        }),
        "party" if expected.is_none() => {
            serde_json::from_value::<Other>(value.clone()).is_ok_and(|v| {
                !v.display_name.trim().is_empty() && v.display_name.chars().count() <= 120
            })
        }
        "party" => serde_json::from_value::<Party>(value.clone()).is_ok_and(|v| {
            same(&v.id, expected) && !v.display_name.trim().is_empty()
                && v.display_name.chars().count() <= 120
                // All fields must be present and strings, including blank optional text.
                && [&v.business_name, &v.contact_name, &v.billing_address, &v.email, &v.phone]
                    .iter().all(|s| !s.contains('\0'))
        }),
        "item" => {
            value.get("purchase_cost").is_some()
                && value.get("expense_account_id").is_some()
                && serde_json::from_value::<Item>(value.clone()).is_ok_and(|v| {
                    same(&v.id, expected)
                        && !v.code.is_empty()
                        && !v.description.contains('\0')
                        && matches!(v.kind.as_str(), "SERVICE" | "NONSTOCK")
                        && v.purchase_cost.as_deref().is_none_or(money_text)
                        && v.expense_account_id
                            .as_deref()
                            .is_none_or(|id| !id.is_empty())
                })
        }
        _ => false,
    }
}

pub(crate) fn register(conn: &Connection) -> rusqlite::Result<()> {
    conn.create_scalar_function(
        "fb_checks_snapshot_valid",
        3,
        FunctionFlags::SQLITE_UTF8
            | FunctionFlags::SQLITE_DETERMINISTIC
            | FunctionFlags::SQLITE_INNOCUOUS,
        |ctx| {
            let kind = ctx.get::<String>(0)?;
            let text = ctx.get::<String>(1)?;
            let id = ctx.get::<Option<String>>(2)?;
            Ok(crate::json_evidence::parse_unique_json_as(&text).is_ok_and(|v| valid(&kind, &v, id.as_deref())))
        },
    )
}
