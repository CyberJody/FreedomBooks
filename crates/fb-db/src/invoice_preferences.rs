//! Non-secret, company-specific presentation and delivery preferences.
use serde::{Deserialize, Serialize};

pub const TEXT_FIELDS: &[(&str, &str, usize, bool)] = &[
    ("footer", "Invoice footer", 2000, true),
    ("zelle_name", "Zelle recipient name", 200, false),
    ("zelle_recipient", "Zelle email or phone", 254, false),
    ("zelle_instructions", "Zelle instructions", 1000, true),
    ("email_subject", "Email subject", 300, false),
    ("email_greeting", "Email greeting", 2000, true),
    ("email_signature", "Email signature", 2000, true),
    ("reply_to", "Reply-to email (optional)", 254, false),
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub footer: String,
    pub zelle_enabled: bool,
    pub zelle_name: String,
    pub zelle_recipient: String,
    pub zelle_instructions: String,
    pub stripe_enabled: bool,
    pub stripe_mode: String,
    pub email_mode: String,
    pub email_subject: String,
    pub email_greeting: String,
    pub email_signature: String,
    pub reply_to: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            footer: String::new(),
            zelle_enabled: false,
            zelle_name: String::new(),
            zelle_recipient: String::new(),
            zelle_instructions: "Please include the invoice number in the payment memo.".into(),
            stripe_enabled: false,
            stripe_mode: "TEST".into(),
            email_mode: "DRAFT".into(),
            email_subject: "Invoice {invoice_number} from {business_name}".into(),
            email_greeting: "Hi,\n\nThank you for your business! Here is your invoice summary and how to pay.".into(),
            email_signature: "Thank you,\n{business_name}".into(),
            reply_to: String::new(),
        }
    }
}

pub fn valid(raw: &str) -> bool {
    if raw.len() > 65_536 { return false; }
    let Ok(value) = crate::parse_unique_json(raw) else { return false; };
    let Ok(settings) = serde_json::from_value::<Settings>(value.clone()) else { return false; };
    matches!(settings.stripe_mode.as_str(), "TEST" | "LIVE")
        && matches!(settings.email_mode.as_str(), "DRAFT" | "SEND")
        && TEXT_FIELDS.iter().all(|&(key, _, max, multiline)| {
            let Some(text) = value[key].as_str() else { return false; };
            text.chars().count() <= max
                && !text.contains('\0')
                && if multiline {
                    text.split('\n').all(crate::check_numbers::free_text_valid)
                } else {
                    !text.chars().any(char::is_control) && crate::check_numbers::free_text_valid(text)
                }
        })
}
