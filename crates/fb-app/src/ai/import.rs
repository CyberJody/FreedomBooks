//! Bounded structured journal import. Preview executes the real save/post services and rolls back;
//! apply repeats all validation and commits the entire batch in one Writer transaction.
use crate::{AppError, AppResult, commands::JournalLineValues};
use fb_core::{Book, CompanyId, FbError, UserId, services};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Journal {
    pub request_key: String,
    pub date: String,
    #[serde(default = "import_memo")]
    pub memo: String,
    #[serde(default)]
    pub entry_number: String,
    #[serde(default)]
    pub adjusting: bool,
    #[schemars(length(min = 2, max = 50))]
    pub lines: Vec<JournalLineValues>,
}
fn import_memo() -> String {
    "Imported journal".into()
}

#[derive(Debug)]
enum RunError {
    Core(FbError),
    Preview(Value),
}
impl From<FbError> for RunError {
    fn from(e: FbError) -> Self {
        Self::Core(e)
    }
}
impl From<fb_db::rusqlite::Error> for RunError {
    fn from(e: fb_db::rusqlite::Error) -> Self {
        Self::Core(e.into())
    }
}

pub fn run(
    book: &mut Book,
    actor: UserId,
    company: CompanyId,
    journals: &[Journal],
    preview: bool,
) -> AppResult<Value> {
    if journals.is_empty() || journals.len() > 100 {
        return Err(AppError::Invalid("Import requires 1–100 journals.".into()));
    }
    let mut keys = std::collections::HashSet::new();
    for j in journals {
        // Match the core's idempotency identity before opening the Writer: spelling aliases
        // must not turn two requested entries into one posted journal with a misleading count.
        let key = fb_core::py::py_uuid(&j.request_key)
            .ok_or_else(|| AppError::Invalid("Supply a stable UUID request key.".into()))?;
        if !keys.insert(key.0) {
            return Err(AppError::Invalid(
                "Each imported journal requires a distinct stable UUID request_key.".into(),
            ));
        }
    }
    let result = book.write(|tx| {
        services::access(tx, actor, company)?;
        let mut rows = Vec::new();
        for j in journals {
            let input = services::DraftInput {
                date:j.date.clone(), memo:j.memo.clone(), entry_number:Some(j.entry_number.clone()),
                adjusting:Some(j.adjusting), request_key:Some(j.request_key.clone()),
                lines:j.lines.iter().map(|l| serde_json::to_value(l).expect("typed lines")).collect(),
                ..Default::default()
            };
            let mut saved = services::save_draft_tx(tx, actor, company, &input, None)?;
            // A stable create key can resolve to a draft edited since its original creation.
            // Never post that different current draft merely because its creation digest matches.
            let current=fb_core::store::lines(tx,saved.id)?;
            let same_lines=current.len()==j.lines.len() && current.iter().zip(&j.lines).all(|(a,b)| {
                fb_core::AccountId::parse(&b.account)==Some(a.account_id)
                    && services::money_json(&json!(b.debit)).is_ok_and(|v|v==a.debit)
                    && services::money_json(&json!(b.credit)).is_ok_and(|v|v==a.credit)
                    && a.description==b.description && a.name==b.name
            });
            let number=services::entry_number_text(Some(&j.entry_number))?;
            if saved.date!=fb_core::py::form_date(&j.date).map_err(FbError::validation)?.to_string()
                || saved.memo!=j.memo || saved.adjusting!=j.adjusting || !same_lines
                || (!number.is_empty() && number!=saved.entry_number) {
                return Err(RunError::Core(FbError::validation("An imported journal was changed since creation. Review its current contents and use a new request key for a different import.")));
            }
            if saved.status.as_str() == "DRAFT" {
                saved = services::post_journal_tx(tx, actor, company, saved.id, Some(saved.revision))?;
            }
            rows.push(json!({"id":saved.id.to_db(),"date":saved.date,"entry_number":saved.entry_number,"status":saved.status.as_str(),"revision":saved.revision}));
        }
        let value = json!({"journals":rows,"count":rows.len(),"preview":preview});
        if preview { Err(RunError::Preview(value)) } else { Ok(value) }
    });
    match result {
        Ok(v) | Err(RunError::Preview(v)) => Ok(v),
        Err(RunError::Core(e)) => Err(e.into()),
    }
}
