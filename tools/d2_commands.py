"""Asserted typed command additions for D2's form submissions."""
from pathlib import Path


def replace(path, old, new):
    p=Path(path); text=p.read_text(); assert old in text,(path,old); p.write_text(text.replace(old,new))


replace('crates/fb-app/src/commands.rs','pub enum Command {','''pub enum Command {
    SaveCompany { company_id: Option<Id>, name: String, currency: String },
    SaveJournal { company_id: Id, journal_id: Option<Id>, date: String, memo: String,
        entry_number: String, adjusting: bool, request_key: Option<String>, expected_revision: Option<i64>, lines: Vec<JournalLineValues> },
    PostJournal { company_id: Id, journal_id: Id, expected_revision: i64 },
    ReverseJournal { company_id: Id, journal_id: Id, date: String, reason: String },
    FillAccountingSetup { company_id: Id, digest: String, expected_revision: Option<i64> },
    DismissSetup { company_id: Id },''')
replace('crates/fb-app/src/commands.rs','''    SaveAccount {
        company_id: Id,''','''    SaveAccount {
        company_id: Id,
        #[serde(default)]
        account_id: Option<Id>,
        #[serde(default = "yes")]
        active: bool,
        #[serde(default = "yes")]
        postable: bool,''')
replace('crates/fb-app/src/commands.rs','''        receivables_id: Id,
        undeposited_id: Id,''','''        receivables_id: Option<Id>,
        undeposited_id: Option<Id>,
        #[serde(default)]
        payables_id: Option<Id>,
        #[serde(default)]
        customer_deposits_id: Option<Id>,''')
replace('crates/fb-app/src/commands.rs','pub type Id = String;','''pub type Id = String;
fn yes() -> bool { true }

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct JournalLineValues { pub account: Id, pub debit: String, pub credit: String, pub description: String, pub name: String }
''')
replace('crates/fb-app/src/session.rs','''            Command::SaveAccount { company_id, code, name, detail_type, is_contra, description } => {''','''            Command::SaveCompany { company_id, name, currency } => {
                let c=match company_id {
                    Some(id)=>fb_core::services::edit_company(book,actor,company(&id)?,&name,&currency)?,
                    None=>fb_core::services::create_company(book,actor,&name,&currency,true)?,
                };
                json!({"id":c.id.to_db()})
            }
            Command::SaveJournal { company_id, journal_id, date, memo, entry_number, adjusting, request_key, expected_revision, lines } => {
                let input=fb_core::services::DraftInput {
                    date,memo,entry_number:Some(entry_number),adjusting:Some(adjusting),request_key,expected_revision,
                    journal_id:journal_id.as_deref().map(journal).transpose()?,
                    lines:lines.iter().map(|l|serde_json::to_value(l).expect("typed journal line")).collect(),
                };
                let j=fb_core::services::save_draft(book,actor,company(&company_id)?,input)?;
                json!({"id":j.id.to_db()})
            }
            Command::PostJournal { company_id, journal_id, expected_revision } => {
                let j=fb_core::services::post_journal(book,actor,company(&company_id)?,journal(&journal_id)?,Some(expected_revision))?;
                json!({"id":j.id.to_db()})
            }
            Command::ReverseJournal { company_id, journal_id, date, reason } => {
                let j=fb_core::services::reverse_journal(book,actor,company(&company_id)?,journal(&journal_id)?,&date,&reason)?;
                json!({"id":j.id.to_db()})
            }
            Command::FillAccountingSetup { company_id, digest, expected_revision } => {
                fb_core::first_run::apply_proposal(book,actor,company(&company_id)?,&digest,expected_revision)?
            }
            Command::DismissSetup { company_id } => {
                let c=fb_core::first_run::dismiss_first_run(book,actor,company(&company_id)?)?;
                json!({"id":c.id.to_db()})
            }
            Command::SaveAccount { company_id, account_id, active, postable, code, name, detail_type, is_contra, description } => {''')
replace('crates/fb-app/src/session.rs','''let input = fb_core::services::AccountInput::new(&code, &name).detail(&detail_type).contra(is_contra).description(&description);''','''let mut input = fb_core::services::AccountInput::new(&code, &name).detail(&detail_type).contra(is_contra).description(&description).active(active).postable(postable);
                input.account=account_id.as_deref().map(|id|AccountId::parse(id).ok_or_else(||AppError::Invalid("Invalid account id.".into()))).transpose()?;''')
replace('crates/fb-app/src/session.rs','''Command::SaveSetup { company_id, expected_revision, receivables_id, undeposited_id, collected_tax_id, bank_ids }''','''Command::SaveSetup { company_id, expected_revision, receivables_id, payables_id, undeposited_id, collected_tax_id, customer_deposits_id, bank_ids }''')
replace('crates/fb-app/src/session.rs','''let values = json!({"receivables": receivables_id, "undeposited": undeposited_id, "collected_tax": collected_tax_id, "banks": bank_ids});''','''let values = json!({"receivables": receivables_id, "payables": payables_id, "undeposited": undeposited_id, "collected_tax": collected_tax_id, "customer_deposits":customer_deposits_id, "banks": bank_ids});''')
replace('crates/fb-app/src/session.rs','pub fn company(id: &str)', '''fn journal(id: &str) -> AppResult<fb_core::JournalId> { fb_core::JournalId::parse(id).ok_or_else(||AppError::Invalid("Invalid journal id.".into())) }

pub fn company(id: &str)''')
replace('electron/main/guards.cjs',"new Set(['SaveAccount',", "new Set(['SaveCompany', 'SaveJournal', 'PostJournal', 'ReverseJournal', 'FillAccountingSetup', 'DismissSetup', 'SaveAccount',")
