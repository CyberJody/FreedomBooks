//! Agent authority and catalogue. No sockets, files, caller-selected actors, or second book handles.
pub mod import;
mod operations;
use crate::{App, AppError, AppResult, Command, Query};
use crate::identity::Permission as UserPermission;
use fb_core::{CompanyId, UserId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    ReadOnly,
    AccountingWrite,
}
struct Grant {
    id: String,
    name: String,
    hash: [u8; 32],
    actor: UserId,
    company: CompanyId,
    generation: u64,
    session_generation: u64,
    permission: Permission,
    expires: Instant,
    previews: HashMap<String, Vec<import::Journal>>,
}
#[derive(Default)]
pub struct Authority {
    generation: u64,
    company: Option<CompanyId>,
    grants: Vec<Grant>,
}
impl Authority {
    /// Integration hook: named sign-out/user switch MUST call this, including switch-away-and-back.
    /// Lock and book changes already call it. Never persist a grant across session generations.
    pub fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.grants.clear();
        self.company = None;
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
}
fn denied() -> AppError {
    AppError::Invalid("Agent connection is unavailable or unauthorized.".into())
}
fn hash(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}
fn equal(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter().zip(b).fold(0u8, |n, (a, b)| n | (a ^ b)) == 0
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Call {
    operation: String,
    arguments: Value,
}

impl App {
    pub fn ai_select_company(&mut self, company: CompanyId) -> AppResult<()> {
        self.authorize(Some(company), UserPermission::Read)?;
        if self.ai.company != Some(company) {
            self.ai.invalidate();
            self.ai.company = Some(company);
        }
        Ok(())
    }
    /// Trusted desktop setup only. The returned secret goes straight to OS sealing, never renderer IPC.
    pub fn ai_grant(
        &mut self,
        company: CompanyId,
        name: &str,
        permission: Permission,
    ) -> AppResult<Value> {
        let session = self.authorize(Some(company), match permission {
            Permission::ReadOnly => UserPermission::Read,
            Permission::AccountingWrite => UserPermission::Write,
        })?;
        self.ai_select_company(company)?;
        fb_core::py::clean_free_text(name)?;
        if name.trim().is_empty() || name.chars().count() > 80 || self.ai.grants.len() >= 32 {
            return Err(denied());
        }
        let actor = self.open_mut()?.owner;
        let token = crate::keys::to_hex(&crate::keys::random_bytes::<32>().map_err(|_| denied())?);
        let id = uuid::Uuid::new_v4().to_string();
        self.ai.grants.push(Grant {
            id: id.clone(),
            name: name.into(),
            hash: hash(&token),
            actor,
            company,
            generation: self.ai.generation,
            session_generation: session.generation,
            permission,
            expires: Instant::now() + Duration::from_secs(8 * 3600),
            previews: HashMap::new(),
        });
        Ok(
            json!({"id":id,"token":token,"company_id":company.to_db(),"actor_id":actor.to_db(),"generation":self.ai.generation,"permission":permission,"expires_in_seconds":8*3600}),
        )
    }
    pub fn ai_connections(&self) -> Value {
        json!(self.ai.grants.iter().filter(|g|g.expires>Instant::now()).map(|g|json!({"id":g.id,"name":g.name,"company_id":g.company.to_db(),"actor_id":g.actor.to_db(),"permission":g.permission})).collect::<Vec<_>>())
    }
    pub fn ai_revoke(&mut self, id: &str) {
        self.ai.grants.retain(|g| g.id != id);
    }
    fn ai_authorize(&mut self, token: &str) -> AppResult<usize> {
        if token.len() != 64 {
            return Err(denied());
        }
        let session = self.authenticated_session()?;
        let actor = session.actor;
        let h = hash(token);
        let i = self
            .ai
            .grants
            .iter()
            .position(|g| equal(&g.hash, &h))
            .ok_or_else(denied)?;
        let g = &self.ai.grants[i];
        if g.actor != actor
            || g.company != self.ai.company.ok_or_else(denied)?
            || g.generation != self.ai.generation
            || g.session_generation != session.generation
            || g.expires <= Instant::now()
        {
            return Err(denied());
        }
        let company = g.company;
        self.require_session(session.generation, Some(company), UserPermission::Read)?;
        Ok(i)
    }
    /// Native workers bind this authority at acceptance and revalidate before execution and delivery.
    pub fn ai_session(&mut self, token: &str) -> AppResult<(u64, u64)> {
        let i = self.ai_authorize(token)?;
        let grant = &self.ai.grants[i];
        Ok((grant.session_generation, grant.generation))
    }
    pub fn ai_catalogue(&mut self, token: &str) -> AppResult<Value> {
        let i = self.ai_authorize(token)?;
        let g = &self.ai.grants[i];
        Ok(
            json!({"version":1,"company_id":g.company.to_db(),"actor_id":g.actor.to_db(),"session_generation":g.session_generation,"authority_generation":g.generation,"permission":g.permission,"operations":catalogue(g.permission),"limitations":["No arbitrary SQL, filesystem, backup/protection, user management or provider-network operations.","Import supports structured posted journals only (1–100 per batch); preview IDs are provisional and apply revalidates current accounting setup and accounts.","Grants expire after eight hours and are revoked by lock, company change, sign-out or session invalidation."]}),
        )
    }
    /// Transport-safe validation feedback, only after authenticating the connection. Database/internal
    /// errors stay generic and the current credential is explicitly removed from every error string.
    pub fn ai_response(&mut self, token: &str, raw: Option<&str>) -> AppResult<Value> {
        self.ai_authorize(token)?;
        let result = match raw {
            Some(raw) => self.ai_call(token, raw),
            None => self.ai_catalogue(token),
        };
        Ok(match result {
            Ok(value) => value,
            Err(error) => {
                let message = match error {
                    AppError::Invalid(m) => m,
                    AppError::Core(fb_core::FbError::Validation(m)) => m.join("; "),
                    AppError::Core(fb_core::FbError::DoesNotExist(m)) => m,
                    _ => "Operation refused. Query current state before retrying.".into(),
                };
                json!({"_agent_error":true,"error":"operation_refused","message":message.replace(token,"[redacted]").chars().take(2048).collect::<String>()})
            }
        })
    }
    /// Called only under fb-node's one APP mutex, on the async task pool.
    pub fn ai_call(&mut self, token: &str, raw: &str) -> AppResult<Value> {
        if raw.len() > crate::commands::MAX_REQUEST_BYTES {
            return Err(AppError::Invalid("Request too large.".into()));
        }
        let i = self.ai_authorize(token)?;
        let call: Call = fb_db::parse_unique_json_as(raw)
            .map_err(|_| AppError::Invalid("Invalid agent request.".into()))?;
        let g = &self.ai.grants[i];
        let permitted = catalogue(g.permission);
        if !permitted.iter().any(|s| s["name"] == call.operation) {
            return Err(denied());
        }
        let context = fb_core::agent_provenance::Context {
            connection_id: g.id.clone(),
            agent_name: g.name.clone(),
            operation: call.operation.clone(),
            request_id: uuid::Uuid::new_v4().to_string(),
        };
        let co = g.company;
        fb_core::agent_provenance::with(context, || self.ai_execute(i, co, call))
    }
    fn ai_execute(&mut self, index: usize, co: CompanyId, call: Call) -> AppResult<Value> {
        let mut args = call.arguments.as_object().cloned().ok_or_else(denied)?;
        if ["type", "company_id", "actor", "actor_id", "user_id"]
            .iter()
            .any(|k| args.contains_key(*k))
        {
            return Err(denied());
        }
        let (kind, name) = call.operation.split_once('_').ok_or_else(denied)?;
        let permission = if kind == "command" || (kind == "app" && operations::is_write(name)) {
            UserPermission::Write
        } else {
            UserPermission::Read
        };
        self.require_session(self.ai.grants[index].session_generation, Some(co), permission)?;
        args.insert("type".into(), json!(name));
        if kind == "command" || kind == "query" {
            args.insert("company_id".into(), json!(co.to_db()));
            let raw = Value::Object(args).to_string();
            if kind == "command" {
                let command = crate::commands::parse_command(&raw).map_err(AppError::Invalid)?;
                return self
                    .run(command)
                    .map(|o| serde_json::to_value(o).expect("outcome"));
            }
            return self.query(crate::commands::parse_query(&raw).map_err(AppError::Invalid)?);
        }
        let operation: operations::Operation = serde_json::from_value(Value::Object(args))
            .map_err(|_| AppError::Invalid("Invalid operation arguments.".into()))?;
        match operation {
            operations::Operation::ConnectionInfo => {
                let open = self.open_mut()?;
                let actor = open.owner;
                let (company, user) = open.book.read(|tx| -> fb_core::FbResult<_> {
                    let user = fb_core::services::access(tx, actor, co)?;
                    Ok((fb_core::store::get_company(tx, co)?, user))
                })?;
                Ok(
                    json!({"company_id":co.to_db(),"company_name":company.name,"currency":company.currency,"actor_id":actor.to_db(),"actor_name":user.username,"generation":self.ai.generation}),
                )
            }
            operations::Operation::ImportPreview { journals } => {
                if self.ai.grants[index].previews.len() >= 8 {
                    return Err(AppError::Invalid(
                        "At most eight import previews per connection. Reconnect to clear them."
                            .into(),
                    ));
                }
                let open = self.open_mut()?;
                let mut result = import::run(&mut open.book, open.owner, co, &journals, true)?;
                let token = uuid::Uuid::new_v4().to_string();
                self.ai.grants[index]
                    .previews
                    .insert(token.clone(), journals);
                result["preview_token"] = json!(token);
                Ok(result)
            }
            operations::Operation::ImportApply { preview_token } => {
                let journals = self.ai.grants[index]
                    .previews
                    .get(&preview_token)
                    .ok_or_else(denied)?
                    .clone();
                let open = self.open_mut()?;
                import::run(&mut open.book, open.owner, co, &journals, false)
            }
            other => operations::execute(self, co, other),
        }
    }
}

/// Catalogue is generated from the very enums parsed at dispatch, with a conservative allowlist.
/// Unknown future command variants are never automatically exposed.
fn commands() -> &'static [&'static str] {
    &[
        "SaveAccount",
        "SaveCustomer",
        "SaveItem",
        "SaveSetup",
        "SaveInvoice",
        "PostInvoice",
        "SaveInvoiceDraft",
        "PostReviewedInvoice",
        "VoidInvoice",
        "SaveEstimate",
        "TransitionEstimate",
        "ConvertEstimate",
        "SaveDirectoryRecord",
        "SetDirectoryActive",
        "SaveTaxRate",
        "SaveTaxGroup",
        "SaveTaxSettings",
        "SetTaxItemActive",
        "SaveJournal",
        "PostJournal",
        "ReverseJournal",
        "FillAccountingSetup",
        "DismissSetup",
        "SetNextNumber",
        "SaveCheck",
        "ReviewCheck",
        "RecordCheck",
        "AssignCheckNumber",
        "VoidCheck",
        "SaveBill",
        "ReviewBill",
        "PostBill",
        "VoidBill",
        "ReviewBillPayment",
        "RecordBillPayment",
        "ReverseBillPayment",
        "ReviewTransfer",
        "RecordTransfer",
        "VoidTransfer",
        "ReviewDeposit",
        "RecordDeposit",
        "ReverseDeposit",
        "ReviewCustomerReceipt",
        "RecordCustomerReceipt",
        "ReverseCustomerReceipt",
        "ReviewCreditMemo",
        "RecordCreditMemo",
        "ReviewCustomerDeposit",
        "RecordCustomerDeposit",
        "ReviewCreditApplication",
        "RecordCreditApplication",
        "ReviewDepositApplication",
        "RecordDepositApplication",
        "ReviewCustomerRefund",
        "RecordCustomerRefund",
        "VoidCreditMemo",
        "VoidCustomerDeposit",
        "UnapplyCredit",
        "UnapplyDeposit",
        "VoidCustomerRefund",
        "MarkBankLine",
        "BeginReconciliation",
        "SaveReconciliationTicks",
        "FinishReconciliation",
        "UndoReconciliation",
    ]
}
fn variants(schema: Value, kind: &str, allow: impl Fn(&str) -> bool) -> Vec<Value> {
    let mut result = Vec::new();
    if let Some(variants) = schema["oneOf"].as_array() {
        for variant in variants {
            let Some(name) = variant["properties"]["type"]["const"].as_str() else {
                continue;
            };
            if !allow(name) {
                continue;
            }
            let mut input = variant.clone();
            if let Some(p) = input["properties"].as_object_mut() {
                p.remove("type");
                p.remove("company_id");
            }
            if let Some(r) = input["required"].as_array_mut() {
                r.retain(|x| x != "type" && x != "company_id");
            }
            if let Some(defs) = schema.get("$defs") {
                input["$defs"] = defs.clone();
            }
            result.push(json!({"name":format!("{kind}_{name}"),"description":description(name),"inputSchema":input}));
        }
    }
    result
}
fn description(name: &str) -> String {
    match name {
        "PostInvoice"=>"Review the current invoice mapping/revision and post through the normal accounting service. No human prompt.".into(),
        "SaveInvoice"=>"Create an invoice draft. Use a stable UUID request_key; then PostInvoice with its current revision.".into(),
        "AcceptFeed"=>"Categorize/accept one feed transaction through existing CHECK, JOURNAL, TRANSFER, DEPOSIT or MATCH service; retries use the feed decision identity.".into(),
        "ImportPreview"=>"Validate a structured journal batch using real save/post services, roll back, and return a connection-bound preview_token. Provisional IDs can change.".into(),
        "ImportApply"=>"Revalidate and atomically post the previewed journal batch. Repeat with the same token is idempotent through stable journal request keys.".into(),
        _=>format!("{name}: validated FreedomBooks operation in the connected company. Money values use exact decimal strings. Review tokens/revisions, when required, come from the matching Review operation.")
    }
}
pub fn catalogue(permission: Permission) -> Vec<Value> {
    let mut result = variants(
        serde_json::to_value(schemars::schema_for!(Query)).unwrap(),
        "query",
        |n| {
            matches!(
                n,
                "Accounts" | "Register" | "Invoices" | "NumberingSettings"
            )
        },
    );
    result.extend(variants(
        serde_json::to_value(schemars::schema_for!(operations::Operation)).unwrap(),
        "app",
        |n| permission == Permission::AccountingWrite || !operations::is_write(n),
    ));
    if permission == Permission::AccountingWrite {
        result.extend(variants(
            serde_json::to_value(schemars::schema_for!(Command)).unwrap(),
            "command",
            |n| commands().contains(&n),
        ));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn expired_grants_and_generation_changes_cannot_authorize_and_errors_redact_key() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new();
        let co = app
            .create_book(
                &dir.path().join("synthetic.fbbook"),
                &crate::keys::MasterKey::new([41; 32]),
                "Synthetic actor",
                "Synthetic expiry",
            )
            .unwrap()
            .company_id;
        app.enroll_owner("Synthetic expiry individual password").unwrap();
        let grant = app
            .ai_grant(co, "Synthetic agent", Permission::AccountingWrite)
            .unwrap();
        let token = grant["token"].as_str().unwrap();
        let error = app
            .ai_response(
                token,
                Some(
                    &json!({"operation":"query_Accounts","arguments":{"actor_id":token}})
                        .to_string(),
                ),
            )
            .unwrap();
        assert!(error["_agent_error"] == true && !error.to_string().contains(token));
        app.ai.grants[0].expires = Instant::now() - Duration::from_secs(1);
        assert!(app.ai_catalogue(token).is_err());
        let grant = app
            .ai_grant(co, "Generation agent", Permission::ReadOnly)
            .unwrap();
        let token = grant["token"].as_str().unwrap();
        app.ai.generation += 1;
        assert!(app.ai_catalogue(token).is_err());
    }
}
