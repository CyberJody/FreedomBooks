//! `directory.services`, tax defaults, vendor-default eligibility. Services never post journals.
use crate::{Book, CompanyId, FbError, FbResult, Money, UserId, calculations, py, services, store};
use fb_db::Writer;
use fb_db::rusqlite::{params, types::Value as SqlValue};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    Customer,
    Vendor,
    Item,
}
impl Model {
    pub fn name(self) -> &'static str {
        match self {
            Self::Customer => "customer",
            Self::Vendor => "vendor",
            Self::Item => "item",
        }
    }
    pub fn table(self) -> String {
        format!("directory_{}", self.name())
    }
    pub fn fields(self) -> Vec<&'static str> {
        let mut c = vec![
            "display_name",
            "business_name",
            "contact_name",
            "email",
            "phone",
            "billing_address",
            "notes",
        ];
        match self {
            Self::Customer => c.extend(["tax_status", "exemption_number"]),
            Self::Vendor => c.extend(["default_expense_account", "default_terms"]),
            Self::Item => {
                return vec![
                    "code",
                    "description",
                    "kind",
                    "sale_rate",
                    "purchase_cost",
                    "income_account",
                    "expense_account",
                    "taxable",
                ];
            }
        }
        c
    }
}
fn col(field: &str) -> String {
    if field.ends_with("_account") {
        format!("{field}_id")
    } else {
        field.into()
    }
}
fn defaults(model: Model, company: CompanyId) -> Value {
    let mut out = json!({"id":fb_ids::new_v7().to_string(),"company_id":company.to_db(),"active":true});
    for field in model.fields() {
        out[field] = json!("");
    }
    match model {
        Model::Customer => out["tax_status"] = json!("TAXABLE"),
        Model::Vendor => {
            out["default_terms"] = Value::Null;
            out["default_expense_account"] = Value::Null;
        }
        Model::Item => {
            out["sale_rate"] = Value::Null;
            out["purchase_cost"] = Value::Null;
            out["income_account"] = Value::Null;
            out["expense_account"] = Value::Null;
            out["taxable"] = json!(true);
        }
    }
    out
}
pub fn get(conn: &(impl fb_db::Query + ?Sized), company: CompanyId, model: Model, id: &str) -> FbResult<Value> {
    let mut fields = vec!["id", "company_id", "active"];
    fields.extend(model.fields());
    let sql = format!(
        "SELECT {} FROM {} WHERE company_id=?1 AND id=?2",
        fields.iter().map(|s| col(s)).collect::<Vec<_>>().join(","),
        model.table()
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params![company.to_db(), id])?;
    let row = rows.next()?.ok_or_else(|| {
        FbError::missing(match model {
            Model::Customer => "Customer",
            Model::Vendor => "Vendor",
            Model::Item => "Item",
        })
    })?;
    let mut out = json!({});
    for (i, f) in fields.iter().enumerate() {
        out[*f] = match *f {
            "active" | "taxable" => json!(row.get::<_, bool>(i)?),
            "sale_rate" | "purchase_cost" => row
                .get::<_, Option<i64>>(i)?
                .map(|n| json!(Money::from_db(n).to_string()))
                .unwrap_or(Value::Null),
            _ => row.get::<_, Option<String>>(i)?.map(Value::String).unwrap_or(Value::Null),
        };
    }
    Ok(out)
}
pub fn list(conn: &(impl fb_db::Query + ?Sized), company: CompanyId, model: Model) -> FbResult<Vec<Value>> {
    let order = if model == Model::Item { "code" } else { "display_name" };
    let mut stmt = conn.prepare(&format!(
        "SELECT id FROM {} WHERE company_id=?1 ORDER BY {order} COLLATE fb_text,id",
        model.table()
    ))?;
    let ids = stmt
        .query_map([company.to_db()], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    ids.iter().map(|id| get(conn, company, model, id)).collect()
}
pub fn save_record(
    book: &mut Book,
    actor: UserId,
    company: CompanyId,
    model: Model,
    values: &Value,
    record_id: Option<&str>,
) -> FbResult<Value> {
    book.write(|tx| save_record_tx(tx, actor, company, model, values, record_id))
}
/// Raw/autocommit connections do not prove an owning IMMEDIATE writer.
/// ```compile_fail
/// fn invalid(conn: &fb_db::rusqlite::Connection, actor: fb_core::UserId, company: fb_core::CompanyId) {
///     fb_core::directory::save_record_tx(conn, actor, company, fb_core::directory::Model::Customer, &serde_json::json!({"display_name":"Customer"}), None);
/// }
/// ```
pub fn save_record_tx(
    conn: &Writer<'_>,
    actor: UserId,
    company: CompanyId,
    model: Model,
    values: &Value,
    record_id: Option<&str>,
) -> FbResult<Value> {
    conn.atomic(|| {
        let user = services::access(conn, actor, company)?;
        let fields = model.fields();
        let data = values
            .as_object()
            .ok_or_else(|| FbError::validation("Only editable master-data fields are accepted."))?;
        if data.keys().any(|k| !fields.contains(&k.as_str())) {
            return Err(FbError::validation("Only editable master-data fields are accepted."));
        }
        let before = record_id.map(|id| get(conn, company, model, id)).transpose()?;
        let mut obj = before.clone().unwrap_or_else(|| defaults(model, company));
        for (f, v) in data {
            if ["code", "description", "display_name", "business_name", "contact_name", "email", "phone", "billing_address", "notes", "exemption_number"].contains(&f.as_str())
                && let Some(raw) = v.as_str() {
                let previous = before.as_ref().and_then(|b| b[f].as_str());
                if f == "notes" {
                    if previous != Some(raw) {
                        py::clean_multiline_free_text(raw)?;
                    }
                } else {
                    py::clean_changed_free_text(raw, previous)?;
                }
            }
            obj[f] = match f.as_str() {
                "sale_rate" | "purchase_cost" => {
                    if f == "purchase_cost" && (v.is_null() || v == "") {
                        Value::Null
                    } else {
                        let m = fb_money::money(&calculations::money_input(v)).map_err(|_| FbError::validation(fb_money::INVALID_AMOUNT))?;
                        json!(m.to_string())
                    }
                }
                "default_terms" => {
                    if v.is_null() || v == "" {
                        Value::Null
                    } else if v.as_str().is_some_and(|s| ["0", "15", "30", "60"].contains(&s)) {
                        v.clone()
                    } else {
                        return Err(FbError::validation(
                            "Choose Due on receipt, Net 15, Net 30 or Net 60, or leave terms blank.",
                        ));
                    }
                }
                "taxable" => {
                    if !v.is_boolean() {
                        return Err(FbError::validation("Choose taxable or not taxable."));
                    }
                    v.clone()
                }
                f if f.ends_with("_account") => {
                    if !v.is_null() && v.as_str().and_then(crate::AccountId::parse).is_none() {
                        return Err(FbError::validation("Choose an account from this company."));
                    }
                    v.clone()
                }
                _ => json!(py::py_strip(v.as_str().ok_or_else(|| FbError::validation("Enter text."))?)),
            };
        }
        if model == Model::Item && before.is_none() && !data.contains_key("taxable") {
            obj["taxable"] = json!(item_taxable_default(obj["kind"].as_str().unwrap_or("")));
        }
        full_clean(model, &obj)?;
        validate(conn, company, model, &obj)?;
        if let Err(error) = persist(conn, model, &obj, before.is_some()) {
            if error.constraint() == Some("unique:index 'item_code_ci_per_company'") {
                return Err(FbError::validation(
                    "An item with this code / name already exists in this company (including archived items).",
                ));
            }
            return Err(error);
        }
        services::audit(
            conn,
            &user,
            company,
            &format!("{}.{}", model.name(), if before.is_some() { "updated" } else { "created" }),
            format!("{}:{}", model.name(), obj["id"].as_str().unwrap()),
            json!({"before":before,"after":obj}),
        )?;
        Ok(obj)
    })
}
fn full_clean(model: Model, obj: &Value) -> FbResult<()> {
    let mut errors = Vec::new();
    let texts = if model == Model::Item {
        vec![("code", 120, true), ("description", 2000, false), ("kind", 12, true)]
    } else {
        vec![
            ("display_name", 120, true),
            ("business_name", 120, false),
            ("contact_name", 120, false),
            ("email", 254, false),
            ("phone", 50, false),
            ("billing_address", 1000, false),
            ("notes", 4000, false),
        ]
    };
    for (f, max, required) in texts {
        let text = obj[f].as_str().unwrap_or("");
        if f == "kind" && !text.is_empty() && !["SERVICE", "NONSTOCK"].contains(&text) {
            errors.push(format!("Value '{text}' is not a valid choice."));
            continue;
        }
        if required && text.is_empty() {
            errors.push(py::BLANK.into());
        } else {
            if f == "email" && !text.is_empty() && !crate::email_validation::valid_email(text) {
                errors.push("Enter a valid email address.".into());
            }
            if py::py_len(text) > max {
                errors.push(py::max_length_message(max, text));
            }
        }
    }
    if model == Model::Customer {
        let status = obj["tax_status"].as_str().unwrap_or("");
        if status.is_empty() {
            errors.push(py::BLANK.into());
        } else if !["TAXABLE", "EXEMPT"].contains(&status) {
            errors.push(format!("Value '{status}' is not a valid choice."));
        }
        if let Some(v) = obj["exemption_number"].as_str()
            && py::py_len(v) > 40
        {
            errors.push(py::max_length_message(40, v));
        }
    }
    if model == Model::Item && obj["sale_rate"].is_null() {
        errors.push("This field cannot be null.".into());
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(FbError::Validation(errors))
    }
}

fn validate(conn: &(impl fb_db::Query + ?Sized), company: CompanyId, model: Model, obj: &Value) -> FbResult<()> {
    if model == Model::Item {
        for (field, kind) in [("income_account", "INCOME"), ("expense_account", "EXPENSE")] {
            if let Some(id) = obj[field].as_str() {
                let acct = store::account(conn, crate::AccountId::parse(id).unwrap())?;
                if !acct.is_some_and(|a| a.company_id == company && a.active && a.postable && a.kind.as_str() == kind) {
                    return Err(FbError::validation(format!(
                        "Choose an active, postable {} account in this company, or clear this default.",
                        kind.to_lowercase()
                    )));
                }
            }
        }
        let duplicate: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM directory_item WHERE company_id=?1 AND (fb_upper(code)=fb_upper(?2) OR fb_lower(code)=fb_lower(?2)) AND id<>?3)",
            params![company.to_db(), obj["code"].as_str(), obj["id"].as_str()],
            |r| r.get(0),
        )?;
        if duplicate {
            return Err(FbError::validation(
                "An item with this code / name already exists in this company (including archived items).",
            ));
        }
    } else if model == Model::Vendor
        && let Some(id) = obj["default_expense_account"].as_str()
    {
        let account = store::account(conn, crate::AccountId::parse(id).unwrap())?
            .filter(|a| a.company_id == company)
            .ok_or_else(|| FbError::validation("Choose an account from this company, or clear this default."))?;
        if let Some(role) = crate::documents::control_role(conn, company, account.id)? {
            return Err(FbError::validation(crate::documents::control_message(&account, &role)));
        }
        if !purchase_line_allowed(&account) {
            return Err(FbError::validation(
                "Choose an active account a bill line may charge (an expense, cost, asset, liability or equity account), or clear this default.",
            ));
        }
    }
    Ok(())
}
fn persist(conn: &Writer<'_>, model: Model, obj: &Value, update: bool) -> FbResult<()> {
    let mut fields = vec!["id", "company_id", "active"];
    fields.extend(model.fields());
    let vals = fields
        .iter()
        .map(|f| match *f {
            "active" | "taxable" => SqlValue::Integer(i64::from(obj[*f].as_bool().unwrap())),
            "sale_rate" | "purchase_cost" => {
                if obj[*f].is_null() {
                    SqlValue::Null
                } else {
                    SqlValue::Integer(fb_money::money(&calculations::money_input(&obj[*f])).unwrap().to_db().unwrap())
                }
            }
            _ => obj[*f].as_str().map(|s| SqlValue::Text(s.into())).unwrap_or(SqlValue::Null),
        })
        .collect::<Vec<_>>();
    let cols = fields.iter().map(|f| col(f)).collect::<Vec<_>>();
    let query = if update {
        format!(
            "UPDATE {} SET {} WHERE id=?1",
            model.table(),
            cols.iter()
                .enumerate()
                .skip(1)
                .map(|(i, c)| format!("{c}=?{}", i + 1))
                .collect::<Vec<_>>()
                .join(",")
        )
    } else {
        format!(
            "INSERT INTO {} ({}) VALUES ({})",
            model.table(),
            cols.join(","),
            (1..=cols.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(",")
        )
    };
    conn.execute(&query, fb_db::rusqlite::params_from_iter(vals))?;
    Ok(())
}
pub fn set_active(book: &mut Book, actor: UserId, company: CompanyId, model: Model, id: &str, active: bool) -> FbResult<Value> {
    book.write(|tx| {
        let user = services::access(tx, actor, company)?;
        let mut obj = get(tx, company, model, id)?;
        if obj["active"] == active {
            return Ok(obj);
        }
        let before = obj.clone();
        if active {
            validate(tx, company, model, &obj)?;
        }
        obj["active"] = json!(active);
        persist(tx, model, &obj, true)?;
        services::audit(
            tx,
            &user,
            company,
            &format!("{}.{}", model.name(), if active { "reactivated" } else { "archived" }),
            format!("{}:{id}", model.name()),
            json!({"before":before,"after":obj}),
        )?;
        Ok(obj)
    })
}
pub fn item_taxable_default(kind: &str) -> bool {
    kind != "SERVICE"
}
pub fn line_taxable_default(customer: Option<&Value>, item: Option<&Value>) -> bool {
    if customer.is_some_and(|c| c["tax_status"] == "EXEMPT") {
        return false;
    }
    item.is_some_and(|i| i["taxable"] == true)
}
pub fn customer_tax_hints(conn: &(impl fb_db::Query + ?Sized), company: CompanyId) -> FbResult<Value> {
    let mut result = json!({});
    for c in list(conn, company, Model::Customer)? {
        result[c["id"].as_str().unwrap()] = json!({"exempt":c["tax_status"]=="EXEMPT","certificate":c["exemption_number"]});
    }
    Ok(result)
}
/// Picker hints only: neither bill nor check posting reads vendor defaults.
pub fn prefill_catalog(conn: &(impl fb_db::Query + ?Sized), company: CompanyId) -> FbResult<Value> {
    let mut result = json!({});
    for v in list(conn, company, Model::Vendor)? {
        let mut expense = json!("");
        if let Some(a) = v["default_expense_account"]
            .as_str()
            .and_then(crate::AccountId::parse)
            .map(|id| store::account(conn, id))
            .transpose()?
            .flatten()
            && a.company_id == company
            && purchase_line_allowed(&a)
            && crate::documents::control_role(conn, company, a.id)?.is_none()
        {
            expense = json!(a.id.to_db());
        }
        result[v["id"].as_str().unwrap()] = json!({"expense_account":expense,"terms":v["default_terms"].as_str().unwrap_or("")});
    }
    Ok(result)
}

pub fn purchase_line_allowed(account: &crate::model::Account) -> bool {
    account.active
        && account.postable
        && [
            "BANK",
            "OTHER_CURRENT_ASSET",
            "FIXED_ASSET",
            "OTHER_ASSET",
            "CREDIT_CARD",
            "OTHER_CURRENT_LIABILITY",
            "LONG_TERM_LIABILITY",
            "EQUITY",
            "COST_OF_GOODS_SOLD",
            "EXPENSE",
            "OTHER_EXPENSE",
        ]
        .contains(&account.detail_type.as_str())
}
