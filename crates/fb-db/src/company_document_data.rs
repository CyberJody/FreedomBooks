//! Shared decoded schema for stored company-document text. No normalization or repair on read.
pub const FIELDS: &[(&str, &str, usize)] = &[
    ("street1", "Street address", 200),
    ("street2", "Address line 2", 200),
    ("city", "City", 100),
    ("region", "State / region", 100),
    ("postal_code", "Postal code", 32),
    ("country", "Country", 100),
    ("phone", "Phone", 50),
    ("email", "Email", 254),
    ("website", "Website", 500),
    ("tax_id", "Tax ID (optional)", 100),
    ("payment_instructions", "Payment instructions", 2000),
];

pub fn valid(raw: &str) -> bool {
    if raw.len() > 65_536 { return false; }
    let Ok(value) = crate::parse_unique_json(raw) else { return false; };
    let Some(object) = value.as_object() else { return false; };
    object.len() == FIELDS.len() && FIELDS.iter().all(|&(key, _, max)| {
        let Some(text) = object.get(key).and_then(serde_json::Value::as_str) else { return false; };
        !text.contains('\0') && text.chars().count() <= max &&
            if key == "payment_instructions" {
                text.split('\n').all(crate::check_numbers::free_text_valid)
            } else {
                !text.chars().any(char::is_control) && crate::check_numbers::free_text_valid(text)
            }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn blank() -> serde_json::Value {
        FIELDS.iter().map(|(key,_,_)|((*key).to_owned(),serde_json::json!(""))).collect()
    }
    #[test]
    fn decoded_schema_is_exact_bounded_and_allows_multiline_payment_text() {
        for &(key,_,max) in FIELDS {
            let mut value=blank();value[key]=serde_json::json!("é".repeat(max));
            assert!(valid(&value.to_string()),"{key} Unicode scalar limit");
            value[key]=serde_json::json!("é".repeat(max+1));
            assert!(!valid(&value.to_string()),"{key} over limit");
            for bad in [serde_json::json!("A\u{0}B"),serde_json::json!(null),serde_json::json!({}),serde_json::json!(42)] {
                value[key]=bad;assert!(!valid(&value.to_string()),"{key} invalid decoded value");
            }
            let mut value=blank();value.as_object_mut().unwrap().remove(key);
            assert!(!valid(&value.to_string()),"required {key}");
        }
        let mut value=blank();value["payment_instructions"]=serde_json::json!("By check\nReference\tinvoice");
        assert!(valid(&value.to_string()));
        value["probe"]=serde_json::json!({});assert!(!valid(&value.to_string()));
        let mut value=blank();value["street1"]=serde_json::json!("Line\nOther line");
        assert!(!valid(&value.to_string()));
        value["street1"]=serde_json::json!("spoof\u{202e}text");assert!(!valid(&value.to_string()));
    }
}
