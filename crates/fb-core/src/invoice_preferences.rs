//! Presentation preferences have no authority to send mail, contact a provider or post money.
use crate::{CompanyId, UserId, error::{FbError, FbResult}, py, services};
use fb_db::{Query, Writer, rusqlite::{OptionalExtension, params}};
use serde::{Deserialize, Serialize};
use serde_json::json;
pub use fb_db::invoice_preferences::{Settings, TEXT_FIELDS};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stored {
    pub revision: i64,
    pub settings: Settings,
}
pub struct Change {
    pub expected_revision: i64,
    pub settings: Settings,
}

pub fn get(q: &(impl Query + ?Sized), c: CompanyId) -> FbResult<Stored> {
    let row: Option<(i64, String)> = q.query_row(
        "SELECT revision,data FROM invoice_preferences WHERE company_id=?1", [c.to_db()],
        |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
    match row {
        None => Ok(Stored { revision: 0, settings: Settings::default() }),
        Some((revision, raw)) if revision > 0 && fb_db::invoice_preferences::valid(&raw) => {
            Ok(Stored { revision, settings: fb_db::parse_unique_json_as(&raw)
                .map_err(|_| FbError::validation("Invalid stored invoice preferences."))? })
        }
        _ => Err(FbError::validation("Invalid stored invoice preferences.")),
    }
}

pub fn clean(settings: Settings) -> FbResult<Settings> {
    let mut value = serde_json::to_value(settings).expect("preferences");
    for &(key, label, max, multiline) in TEXT_FIELDS {
        let text = value[key].as_str().expect("text");
        let normalized = if multiline { text.replace("\r\n", "\n").replace('\r', "\n") } else { text.to_owned() };
        let text = py::py_strip(&normalized);
        if text.chars().count() > max || (!multiline && text.chars().any(char::is_control)) {
            return Err(FbError::validation(format!("{label}: use at most {max} characters{}.", if multiline { "" } else { " on one line" })));
        }
        for line in text.split('\n') { py::clean_free_text(line)?; }
        value[key] = json!(text);
    }
    if !fb_db::invoice_preferences::valid(&value.to_string()) {
        return Err(FbError::validation("Choose valid invoice and email preferences."));
    }
    let result: Settings = serde_json::from_value(value).expect("validated preferences");
    if result.zelle_enabled {
        let recipient = &result.zelle_recipient;
        let phone = recipient.chars().all(|c| c.is_ascii_digit() || matches!(c,'+'|'-'|'('|')'|' '|'.'))
            && (10..=15).contains(&recipient.chars().filter(char::is_ascii_digit).count());
        if !crate::email_validation::valid_email(recipient) && !phone {
            return Err(FbError::validation("Enter the email address or phone number enrolled with Zelle."));
        }
    }
    if !result.reply_to.is_empty() && !crate::email_validation::valid_email(&result.reply_to) {
        return Err(FbError::validation("Enter a valid reply-to email address."));
    }
    if result.email_subject.is_empty() {
        return Err(FbError::validation("Enter an email subject."));
    }
    Ok(result)
}

pub(crate) fn save_tx(tx: &Writer<'_>, actor: UserId, c: CompanyId, change: Change) -> FbResult<()> {
    let user = services::access(tx, actor, c)?;
    let old = get(tx, c)?;
    if old.revision != change.expected_revision {
        return Err(FbError::validation("Invoice preferences changed. Reload the company settings before saving."));
    }
    let settings = clean(change.settings)?;
    if settings == old.settings { return Ok(()); }
    let revision = old.revision.checked_add(1).ok_or_else(|| FbError::validation("Settings revision limit reached."))?;
    let data = serde_json::to_string(&settings).expect("preferences");
    if old.revision == 0 {
        tx.execute("INSERT INTO invoice_preferences(company_id,revision,data) VALUES(?1,?2,?3)", params![c.to_db(),revision,data])?;
    } else {
        tx.execute("UPDATE invoice_preferences SET revision=?2,data=?3 WHERE company_id=?1", params![c.to_db(),revision,data])?;
    }
    services::audit(tx,&user,c,"company.invoice_preferences_updated",format!("company:{c}"),
        json!({"revision":revision,"before":old.settings,"after":settings}))?;
    Ok(())
}
