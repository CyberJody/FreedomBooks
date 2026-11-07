//! Executed Python test vectors: documents calculation tests, FX11 rounding/property
//! loops and Django's accepted email grammar. Expectations come from the frozen app.
mod common;
use fb_core::{
    calculations as c,
    directory::{self, Model},
};
use serde_json::{Value, json};
#[test]
fn all_frozen_python_vectors_match() {
    let vectors: Vec<Value> = serde_json::from_str(include_str!("l2a_python_vectors.json")).unwrap();
    let mut fx = common::Fx::new("l2-vectors");
    let co = fx.company("Vectors");
    for (i, row) in vectors.iter().enumerate() {
        let args = &row["args"];
        let result: fb_core::FbResult<Value> = match row["fn"].as_str().unwrap() {
            "town_from_address" => Ok(json!(fb_core::party_display::town_from_address(args[0].as_str().unwrap()))),
            "posting_role_label" => Ok(json!(fb_core::party_display::posting_role_label(
                args[0].as_str(),
                args[1].as_str()
            ))),
            "quantity" => c::quantity(&args[0]).map(|v| json!(v.to_string())),
            "unit_rate" => c::unit_rate(&args[0]).map(|v| json!(v.to_string())),
            "line_amount" => c::line_amount(&args[0], &args[1]).map(|v| json!(v.to_string())),
            "email" => directory::save_record(
                &mut fx.book,
                fx.user,
                co,
                Model::Customer,
                &json!({"display_name":"Email","email":args[0]}),
                None,
            )
            .map(|_| json!(true)),
            "totals" => {
                let tax = &args["tax"];
                c::totals(
                    args["lines"].as_array().unwrap(),
                    tax["tax_mode"].as_str().unwrap_or(""),
                    tax.get("tax_rate").filter(|v| !v.is_null()),
                    tax.get("tax_label").unwrap_or(&json!("")),
                    tax.get("tax_snapshot").filter(|v| !v.is_null()),
                    tax["for_posting"].as_bool().unwrap_or(false),
                )
            }
            other => panic!("unknown fn {other}"),
        };
        if row["ok"] == true {
            assert_eq!(
                result.unwrap_or_else(|e| panic!("vector {i}: {row}: {e}")),
                row["value"],
                "vector {i}: {row}"
            );
        } else {
            assert_eq!(result.unwrap_err().message(), row["message"].as_str().unwrap(), "vector {i}: {row}");
        }
    }
}
#[test]
fn glibc_case_mapping_is_postgres_case_mapping() {
    let fx = common::Fx::new("l2-case");
    let rows: Vec<Vec<String>> = serde_json::from_str(include_str!("l2a_pg_case_vectors.json")).unwrap();
    for row in rows {
        let actual: (String, String) = fx
            .book
            .connection()
            .query_row("SELECT fb_lower(?1),fb_upper(?1)", [&row[0]], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!(actual, (row[1].clone(), row[2].clone()), "{:?}", row[0]);
    }
}
