//! Fail closed before comparer indexing/normalization; identical malformed dumps are not parity.
use serde_json::Value;
use std::collections::BTreeSet;

pub(super) enum Shape {
    EmptyOr(&'static Shape),
    String,
    Bool,
    Int,
    Nullable(&'static Shape),
    Optional(&'static Shape),
    Array(&'static Shape),
    Tuple(&'static [Shape]),
    Object(&'static [(&'static str, Shape)]),
    Map(&'static Shape),
    Detail,
    Directory,
    Tax,
    ResultValue,
    Snapshot,
}
use Shape::*;
const REMINDER_GROUP: Shape = Object(&[
    ("count",Int),("total",String),("overdue_count",Int),("overdue_total",String),("capped",Bool),
    ("rows",Array(&Object(&[("document_id",String),("number",String),("party",String),("due_date",String),("days",Int),("balance",String),("overdue",Bool),("when",String)]))),
]);
const LINE: Shape = Object(&[
    ("position", Int),
    ("account", String),
    ("debit", Int),
    ("credit", Int),
    ("description", String),
    ("name", String),
]);

const PARTY: &[(&str, Shape)] = &[
    ("id", String),
    ("company_id", String),
    ("model", String),
    ("active", Bool),
    ("display_name", String),
    ("business_name", String),
    ("contact_name", String),
    ("email", String),
    ("phone", String),
    ("billing_address", String),
    ("notes", String),
];
const TAX: Shape = Object(&[
    ("id", String),
    ("company", String),
    ("kind", String),
    ("name", String),
    ("description", String),
    ("active", Bool),
    ("revision", Int),
    ("rate", Nullable(&String)),
    ("agency_id", Nullable(&String)),
    ("member_ids", Optional(&Array(&String))),
]);
pub(super) const TAX_MEMBER: Shape = Object(&[
    ("id", String),
    ("name", String),
    ("description", String),
    ("rate", String),
    ("agency_id", Nullable(&String)),
    ("agency_name", String),
    ("tax", String),
]);
const L2A: Shape = Object(&[
    ("directory", Array(&Directory)),
    ("tax", Array(&Tax)),
    (
        "settings",
        Map(&Object(&[
            ("charge_tax", Bool),
            ("default_item_id", Nullable(&String)),
            ("owed_basis", String),
            ("filing_frequency", String),
            ("bulk_step", String),
            ("revision", Int),
        ])),
    ),
    (
        "identities",
        Array(&Object(&[
            ("id", String),
            ("company_id", String),
            ("kind", String),
            ("ordinal", Int),
            ("number", String),
            ("request_key", String),
            ("currency", String),
        ])),
    ),
    ("sequences", Array(&Tuple(&[String, String, Int]))),
    (
        "setup",
        Map(&Object(&[
            ("revision", Int),
            ("banks", Array(&String)),
            ("receivables", Nullable(&String)),
            ("payables", Nullable(&String)),
            ("undeposited", Nullable(&String)),
            ("collected_tax", Nullable(&String)),
            ("customer_deposits", Nullable(&String)),
        ])),
    ),
    (
        "snapshots",
        Array(&Object(&[
            ("source", String),
            ("role", String),
            ("account_id", String),
            ("account_code", String),
            ("account_name", String),
            ("account_kind", String),
        ])),
    ),
    ("results", Map(&ResultValue)),
    ("source_journals", Array(&Tuple(&[String, String]))),
]);

const DUMP: Shape = Object(&[
    ("backups", Optional(&Detail)),
    ("book_identity", Optional(&Detail)),
    ("sales", crate::sales::shape::SALES),
    (
        "outcomes",
        Array(&Object(&[
            ("step", Int),
            ("op", String),
            ("ok", Bool),
            ("kind", Optional(&String)),
            ("message", Optional(&String)),
            ("unexpected", Optional(&Bool)),
            ("audit_counts", Map(&Int)),
            ("query", Optional(&Object(&[
                ("ref", String), ("company", String),
                ("today", Nullable(&String)), ("days", Nullable(&Int)),
                ("attachment", Nullable(&String)),
            ]))),
            ("result_ref", Optional(&String)),
            ("expect_kind", Optional(&String)),
            ("expect_message", Optional(&String)),
            ("manifest", Optional(&Detail)),
            ("evidence", Optional(&Detail)),
        ])),
    ),
    (
        "companies",
        Array(&Object(&[
            ("ref", String),
            ("name", String),
            ("currency", String),
            ("first_run_open", Bool),
            ("bill_reminder_days", Int),
            ("members", Array(&String)),
        ])),
    ),
    (
        "accounts",
        Array(&Object(&[
            ("ref", String),
            ("company", String),
            ("code", String),
            ("name", String),
            ("kind", String),
            ("detail_type", String),
            ("is_contra", Bool),
            ("description", String),
            ("active", Bool),
            ("postable", Bool),
        ])),
    ),
    (
        "journals",
        Array(&Object(&[
            ("ref", String),
            ("company", String),
            ("date", String),
            ("memo", String),
            ("status", String),
            ("entry_number", String),
            ("adjusting", Bool),
            ("revision", Int),
            ("reversal_of", Nullable(&String)),
            ("request_key", Nullable(&String)),
            ("created_by", String),
            ("posted_by", Nullable(&String)),
            ("lines", Array(&LINE)),
        ])),
    ),
    ("l2a", L2A),
    ("payables", crate::payables_shape::SHAPE),
    ("checks", crate::checks_shape::SHAPE),
    ("prepayments", crate::prepayments_shape::SHAPE),
    ("credits", crate::credits_shape::SHAPE),
    ("banking", crate::banking_shape::SHAPE),
    ("attachments", crate::attachments_shape::SHAPE),
    ("assistant", crate::assistant_shape::SHAPE),
    ("bankfeeds", crate::bankfeeds_shape::SHAPE),
    ("display", Map(&String)),
    (
        "registers",
        Map(&Object(&[
            ("opening", Int),
            ("closing", Int),
            ("rows", Array(&Tuple(&[String, Int, Int]))),
        ])),
    ),
    (
        "trial_balances",
        Map(&Map(&Object(&[
            ("rows", Array(&Tuple(&[String, Int, Int]))),
            ("debit", Int),
            ("credit", Int),
        ]))),
    ),
    (
        "audit",
        Map(&Array(&Object(&[
            ("action", String),
            ("actor", String),
            ("actor_name", String),
            ("target", String),
            ("detail", Detail),
        ]))),
    ),
]);

impl Shape {
    pub(super) fn validate(&self, value: &Value, path: &str) -> Result<(), std::string::String> {
        let bad = || format!("{path}: invalid type or shape: {value}");
        match self {
            EmptyOr(_) if value.as_object().is_some_and(|m|m.is_empty()) => Ok(()),
            EmptyOr(shape) => shape.validate(value,path),
            String if value.is_string() => Ok(()),
            Bool if value.is_boolean() => Ok(()),
            Int if value.as_i64().is_some() => Ok(()),
            Detail if value.is_object() => Ok(()),
            Directory => {
                let mut fields: Vec<(&str, &Shape)> =
                    PARTY.iter().map(|(key, shape)| (*key, shape)).collect();
                let customer = [("tax_status", String), ("exemption_number", String)];
                let vendor = [
                    ("default_expense_account", Nullable(&String)),
                    ("default_terms", Nullable(&String)),
                ];
                let item = [
                    ("id", String),
                    ("company_id", String),
                    ("model", String),
                    ("active", Bool),
                    ("code", String),
                    ("description", String),
                    ("kind", String),
                    ("sale_rate", Nullable(&String)),
                    ("purchase_cost", Nullable(&String)),
                    ("income_account", Nullable(&String)),
                    ("expense_account", Nullable(&String)),
                    ("taxable", Bool),
                ];
                match value["model"].as_str() {
                    Some("customer") => fields.extend(customer.iter().map(|(k, s)| (*k, s))),
                    Some("vendor") => fields.extend(vendor.iter().map(|(k, s)| (*k, s))),
                    Some("item") => fields = item.iter().map(|(k, s)| (*k, s)).collect(),
                    _ => return Err(bad()),
                }
                let object = value.as_object().ok_or_else(bad)?;
                for (name, shape) in &fields {
                    shape.validate(
                        object
                            .get(*name)
                            .ok_or_else(|| format!("{path}/{name}: required field missing"))?,
                        &format!("{path}/{name}"),
                    )?;
                }
                if object
                    .keys()
                    .any(|key| !fields.iter().any(|(name, _)| key == name))
                {
                    return Err(bad());
                }
                Ok(())
            }
            Tax => {
                TAX.validate(value, path)?;
                match value["kind"].as_str() {
                    Some("RATE")
                        if value["rate"].is_string()
                            && value["agency_id"].is_string()
                            && value.get("member_ids").is_none() =>
                    {
                        Ok(())
                    }
                    Some("GROUP") if value["rate"].is_null() && value["agency_id"].is_null() => {
                        Array(&String).validate(&value["member_ids"], &format!("{path}/member_ids"))
                    }
                    _ => Err(bad()),
                }
            }
            Snapshot if value.as_object().is_some_and(|m| m.is_empty()) => Ok(()),
            Snapshot => Object(&[
                ("kind", String),
                ("id", String),
                ("name", String),
                ("description", String),
                ("members", Array(&TAX_MEMBER)),
                ("rate", String),
                ("taxable", String),
                ("tax", String),
            ])
            .validate(value, path),
            ResultValue if value.is_boolean() => Ok(()),
            ResultValue if value.get("today").is_some() => Object(&[
                ("today",String),("days",Int),("through",String),
                ("bills",REMINDER_GROUP),("invoices",REMINDER_GROUP),
            ]).validate(value,path),
            ResultValue if value.get("checklist").is_some() => Object(&[
                ("empty",Bool),("revision",Int),
                ("chosen",Object(&[("receivables",Optional(&String)),("payables",Optional(&String)),("undeposited",Optional(&String)),("collected_tax",Optional(&String)),("banks",Optional(&Array(&String)))])),
                ("reasons",Object(&[("receivables",Optional(&String)),("payables",Optional(&String)),("undeposited",Optional(&String)),("collected_tax",Optional(&String)),("banks",Optional(&String))])),
                ("checklist",Object(&[("chart",Bool),("special",Bool),("tax",Bool),("customer",Bool),("vendor",Bool),("item",Bool)])),
            ]).validate(value,path),
            ResultValue => Object(&[
                ("lines", Array(&String)),
                ("subtotal", String),
                ("taxable", String),
                ("tax", String),
                ("total", String),
                ("tax_mode", String),
                ("tax_rate", String),
                ("tax_label", String),
                ("tax_snapshot", Snapshot),
            ])
            .validate(value, path),
            Nullable(_) if value.is_null() => Ok(()),
            Nullable(shape) | Optional(shape) => shape.validate(value, path),
            Array(shape) => {
                for (i, item) in value.as_array().ok_or_else(bad)?.iter().enumerate() {
                    shape.validate(item, &format!("{path}/{i}"))?;
                }
                Ok(())
            }
            Tuple(fields) => {
                let values = value
                    .as_array()
                    .filter(|v| v.len() == fields.len())
                    .ok_or_else(bad)?;
                for (i, (shape, item)) in fields.iter().zip(values).enumerate() {
                    shape.validate(item, &format!("{path}/{i}"))?;
                }
                Ok(())
            }
            Object(fields) => {
                let object = value.as_object().ok_or_else(bad)?;
                for (name, shape) in *fields {
                    if let Some(item) = object.get(*name) {
                        shape.validate(item, &format!("{path}/{name}"))?;
                    } else if !matches!(shape, Optional(_)) {
                        return Err(format!("{path}/{name}: required field missing"));
                    }
                }
                for name in object.keys() {
                    if !fields.iter().any(|(key, _)| key == name) {
                        return Err(format!("{path}/{name}: unexpected field"));
                    }
                }
                Ok(())
            }
            Map(shape) => {
                for (name, item) in value.as_object().ok_or_else(bad)? {
                    shape.validate(item, &format!("{path}/{name}"))?;
                }
                Ok(())
            }
            _ => Err(bad()),
        }
    }
}

pub(super) fn validate(value: &Value) -> Result<(), std::string::String> {
    DUMP.validate(value, "")?;
    crate::backups::validate(value)?;
    crate::sales::shape::unique(value)?;
    crate::payables_shape::validate(value)?;
    crate::checks_shape::validate(value)?;
    crate::prepayments_shape::validate(value)?;
    crate::credits_shape::validate(value)?;
    crate::banking_shape::validate(value)?;
    crate::attachments_shape::validate(value)?;
    crate::assistant_shape::validate(value)?;
    crate::bankfeeds_shape::validate(value)?;
    // Dumps must contain the reports belonging to their own company/account collections,
    // even if both runners accidentally omitted the same report.
    let refs = |key: &str| -> BTreeSet<&str> {
        value[key]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["ref"].as_str().unwrap())
            .collect()
    };
    for (collection, reports) in [
        ("companies", "trial_balances"),
        ("companies", "audit"),
        ("accounts", "registers"),
        ("journals", "display"),
    ] {
        let keys: BTreeSet<&str> = value[reports]
            .as_object()
            .unwrap()
            .keys()
            .map(|s| s.as_str())
            .collect();
        if refs(collection) != keys {
            return Err(format!("/{reports}: keys must equal {collection} refs"));
        }
    }
    for (company, dates) in value["trial_balances"].as_object().unwrap() {
        if dates.as_object().unwrap().is_empty() {
            return Err(format!(
                "/trial_balances/{company}: at least one report date is required"
            ));
        }
    }
    for report in ["settings", "setup"] {
        let keys: BTreeSet<_> = value["l2a"][report]
            .as_object()
            .unwrap()
            .keys()
            .map(|s| s.as_str())
            .collect();
        if refs("companies") != keys {
            return Err(format!("/l2a/{report}: keys must equal company refs"));
        }
    }
    for collection in ["identities", "sequences", "source_journals"] {
        let mut seen = BTreeSet::new();
        for row in value["l2a"][collection].as_array().unwrap() {
            let key = match collection {
                "identities" => row["id"].to_string(),
                "sequences" => format!("{}:{}", row[0], row[1]),
                _ => row[1].to_string(),
            };
            if !seen.insert(key) {
                return Err(format!("/l2a/{collection}: duplicate key"));
            }
        }
    }
    for (i, outcome) in value["outcomes"].as_array().unwrap().iter().enumerate() {
        if outcome["ok"] == false
            && (outcome.get("kind").is_none() || outcome.get("message").is_none())
        {
            return Err(format!(
                "/outcomes/{i}: refused outcome requires kind and message"
            ));
        }
    }
    Ok(())
}
