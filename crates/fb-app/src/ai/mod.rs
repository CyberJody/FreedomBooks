//! Agent authority and catalogue. No sockets, caller-selected actors, or second book handles.
pub mod import;
mod operations;
mod unattended;
pub(crate) use unattended::{account_approvals, revoke_account_approvals};
use crate::{App, AppError, AppResult, Command, Query};
use crate::identity::Permission as UserPermission;
use fb_core::{CompanyId, UserId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
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
    proof: Option<Arc<unattended::Proof>>,
    sales_reviews: HashMap<String, fb_core::sales::Review>,
    /// Receipt reviews this approval made with `include_settlement`. They never enter the signed-in
    /// session's shared review map, so no other approval and nobody at the desk can spend them,
    /// and they go when the approval does.
    receipt_reviews: HashMap<String, fb_core::sales::Review>,
    d5_reviews: crate::screens::d5::Reviews,
    d6_reviews: crate::screens::d6::Reviews,
    d7a_reviews: crate::screens::d7a::Reviews,
    salestax_reviews: HashMap<String, fb_core::salestax_payments::Review>,
    previews: HashMap<String, Vec<import::Journal>>,
    invoice_previews: HashMap<String, (Vec<Value>, String)>,
}
#[derive(Default)]
pub(crate) struct Attended {
    generation: u64,
    company: Option<CompanyId>,
    grants: Vec<Grant>,
}
#[derive(Clone, Copy)]
enum GrantIndex {
    Attended(usize),
    Unattended(usize),
}
#[derive(Default)]
pub struct Authority {
    pub(crate) attended: Attended,
    unattended: Vec<Grant>,
    execution: Option<(UserId, CompanyId)>,
    /// The approval whose operation is running now, by the hash of its credential; `None` outside
    /// an AI call. Set and cleared by `ai_call` alone.
    calling: Option<[u8; 32]>,
}
impl Authority {
    /// Integration hook: named sign-out/user switch MUST call this, including switch-away-and-back.
    /// Attended grants never survive a session generation. Unattended grants use book authority.
    pub fn invalidate(&mut self) {
        self.attended.generation = self.attended.generation.wrapping_add(1);
        self.attended.grants.clear();
        self.attended.company = None;
    }
    pub fn clear_unattended(&mut self) {
        self.unattended.clear();
    }

    fn grant(&self, index: GrantIndex) -> &Grant {
        match index {
            GrantIndex::Attended(i) => &self.attended.grants[i],
            GrantIndex::Unattended(i) => &self.unattended[i],
        }
    }

    fn grant_mut(&mut self, index: GrantIndex) -> &mut Grant {
        match index {
            GrantIndex::Attended(i) => &mut self.attended.grants[i],
            GrantIndex::Unattended(i) => &mut self.unattended[i],
        }
    }

    pub(crate) fn operation_actor(&self, company: Option<CompanyId>) -> AppResult<Option<UserId>> {
        let Some((actor, scope)) = self.execution else { return Ok(None) };
        if company.is_some_and(|co| co != scope) {
            return Err(denied());
        }
        Ok(Some(actor))
    }

    pub fn generation(&self) -> u64 {
        self.attended.generation
    }

    fn calling_grant(&mut self) -> Option<&mut Grant> {
        let hash = self.calling?;
        self.unattended.iter_mut().chain(self.attended.grants.iter_mut()).find(|g| equal(&g.hash, &hash))
    }
    /// Whether an approval's operation is running now.
    pub(crate) fn calling(&self) -> bool {
        self.calling.is_some()
    }
    /// Keep a receipt review made with `include_settlement` for the approval that is calling now.
    /// `None` when no approval is calling.
    pub(crate) fn remember_receipt_review(&mut self, review: fb_core::sales::Review) -> Option<String> {
        let reviews = &mut self.calling_grant()?.receipt_reviews;
        if reviews.len() >= 1024 {
            reviews.clear();
        }
        let token = uuid::Uuid::new_v4().to_string();
        reviews.insert(token.clone(), review);
        Some(token)
    }
    /// The review the calling approval itself made with `include_settlement`, once. A token of
    /// another approval's review is not found, and that review stays where it is.
    pub(crate) fn take_receipt_review(&mut self, token: &str) -> Option<fb_core::sales::Review> {
        self.calling_grant()?.receipt_reviews.remove(token)
    }
}
#[derive(Clone, Copy, Debug)]
pub enum Refusal {
    BooksClosed,
    ApprovalGone,
    AnchorUnavailable,
    PausedSignedOut,
}

impl Refusal {
    pub fn code(self) -> &'static str {
        match self {
            Self::BooksClosed => "books_closed",
            Self::ApprovalGone => "approval_gone",
            Self::AnchorUnavailable => "anchor_unavailable",
            Self::PausedSignedOut => "paused_signed_out",
        }
    }
}

const GRANT_LIFE: Duration = Duration::from_secs(8 * 3600);

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
    fn ai_require_books(&self) -> AppResult<()> {
        if !self.is_open() || self.backups.viewing_restored().is_some() {
            return Err(AppError::Ai(Refusal::BooksClosed));
        }
        Ok(())
    }

    /// Trusted desktop only: remembered consent is bound to this named user and book.
    /// This does not create authority and is deliberately absent from the agent catalogue.
    pub fn ai_context(&mut self, company: CompanyId) -> AppResult<Value> {
        let session = self.authorize(Some(company), UserPermission::Read)?;
        let path = self.book_path().ok_or_else(denied)?.canonicalize().map_err(|_| denied())?;
        let path = path.to_str().ok_or_else(denied)?;
        let restored = self.backups.viewing_restored().is_some();
        let open = self.open_mut()?;
        let name = open.book.read(|tx| fb_core::store::company(tx, company))?
            .ok_or_else(denied)?.name;
        Ok(json!({"company_id":company.to_db(),"company_name":name,
            "actor_id":session.actor.to_db(),"actor_name":open.owner_name,
            "role":session.role.as_str(),"book_path":path,"restored":restored,
            "session_generation":session.generation}))
    }

    pub fn ai_select_company(&mut self, company: CompanyId) -> AppResult<()> {
        self.authorize(Some(company), UserPermission::Read)?;
        if self.ai.attended.company != Some(company) {
            self.ai.invalidate();
            self.ai.attended.company = Some(company);
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
        self.ai_grant_remembered(company, name, permission, None)
    }

    pub fn ai_grant_remembered(
        &mut self,
        company: CompanyId,
        name: &str,
        permission: Permission,
        remembered_id: Option<&str>,
    ) -> AppResult<Value> {
        self.ai_require_books()?;
        let session = self.authorize(Some(company), match permission {
            Permission::ReadOnly => UserPermission::Read,
            Permission::AccountingWrite => UserPermission::Write,
        })?;
        self.ai_select_company(company)?;
        let id = match remembered_id {
            Some(id) => uuid::Uuid::parse_str(id).map_err(|_| denied())?,
            None => uuid::Uuid::new_v4(),
        }.to_string();
        self.ai.attended.grants.retain(|g| g.id != id && g.expires > Instant::now());
        fb_core::py::clean_free_text(name)?;
        if name.trim().is_empty() || name.chars().count() > 80 || self.ai.attended.grants.len() >= 32 {
            return Err(denied());
        }
        let actor = self.open_mut()?.owner;
        let token = crate::keys::to_hex(&crate::keys::random_bytes::<32>().map_err(|_| denied())?);
        self.ai.attended.grants.push(Grant {
            id: id.clone(),
            name: name.into(),
            hash: hash(&token),
            actor,
            company,
            generation: self.ai.attended.generation,
            session_generation: session.generation,
            permission,
            expires: Instant::now() + GRANT_LIFE,
            proof: None,
            sales_reviews: HashMap::new(),
            receipt_reviews: HashMap::new(),
            d5_reviews: Default::default(), d6_reviews: Default::default(),
            d7a_reviews: Default::default(), salestax_reviews: Default::default(),
            previews: HashMap::new(),
            invoice_previews: HashMap::new(),
        });
        Ok(
            json!({"id":id,"token":token,"company_id":company.to_db(),"actor_id":actor.to_db(),"generation":self.ai.attended.generation,"permission":permission,"expires_in_seconds":8*3600}),
        )
    }
    pub fn ai_connections(&self) -> Value {
        json!(self.ai.attended.grants.iter().filter(|g|g.expires>Instant::now()).map(|g|json!({"id":g.id,"name":g.name,"company_id":g.company.to_db(),"actor_id":g.actor.to_db(),"permission":g.permission})).collect::<Vec<_>>())
    }
    pub fn ai_revoke(&mut self, id: &str) {
        self.ai.attended.grants.retain(|g| g.id != id);
        self.ai.unattended.retain(|g| g.id != id);
    }
    /// Check the sealed proof before making a worker credential. The eight-hour idle lifetime
    /// renews automatically. Import previews stay scoped to the same grant.
    pub fn ai_prepare_unattended(&mut self, raw: &str) -> AppResult<String> {
        let gone = || AppError::Ai(Refusal::ApprovalGone);
        let proof: unattended::Proof = fb_db::parse_unique_json_as(raw).map_err(|_| gone())?;
        self.ai_validate_unattended(&proof)?;
        let token = proof.token()?;
        let digest = hash(&token);
        let now = Instant::now();
        let id = proof.id.clone();
        self.ai.unattended.retain(|g| g.expires > now && (g.id != id || equal(&g.hash, &digest)));
        if !self.ai.unattended.iter().any(|g| equal(&g.hash, &digest)) {
            if self.ai.unattended.len() >= 32 {
                return Err(gone());
            }
            self.ai.unattended.push(Grant {
                id, name: proof.name.clone(), hash: digest, actor: proof.actor()?, company: proof.company()?,
                generation: self.open.as_ref().ok_or(AppError::Locked)?.book.connection_id(),
                session_generation: 0, permission: proof.permission, expires: now + GRANT_LIFE,
                proof: Some(Arc::new(proof)), previews: HashMap::new(), invoice_previews: HashMap::new(), sales_reviews: HashMap::new(),
                receipt_reviews: HashMap::new(),
                d5_reviews: Default::default(), d6_reviews: Default::default(),
                d7a_reviews: Default::default(), salestax_reviews: Default::default(),
            });
        }
        Ok(token)
    }

    fn ai_authorize(&mut self, token: &str) -> AppResult<GrantIndex> {
        self.ai_require_books()?;
        let revoked = || AppError::Ai(Refusal::ApprovalGone);
        let paused = || AppError::Ai(Refusal::PausedSignedOut);
        if token.len() != 64 {
            return Err(revoked());
        }
        let digest = hash(token);
        if let Some(i) = self.ai.unattended.iter().position(|g| equal(&g.hash, &digest)) {
            let grant = &self.ai.unattended[i];
            let connection = self.open.as_ref().ok_or(AppError::Locked)?.book.connection_id();
            if connection != grant.generation {
                return Err(revoked());
            }
            let proof = grant.proof.clone().ok_or_else(revoked)?;
            self.ai_validate_unattended(&proof)?;
            self.ai.unattended[i].expires = Instant::now() + GRANT_LIFE;
            return Ok(GrantIndex::Unattended(i));
        }
        let attended = &self.ai.attended;
        let i = attended.grants.iter().position(|g| equal(&g.hash, &digest)).ok_or_else(revoked)?;
        let grant = &attended.grants[i];
        let (actor, company, session_generation) = (grant.actor, grant.company, grant.session_generation);
        if Some(company) != attended.company || grant.generation != attended.generation || grant.expires <= Instant::now() {
            return Err(paused());
        }
        let session = self.require_session(session_generation, Some(company), UserPermission::Read).map_err(|_| paused())?;
        if actor != session.actor {
            return Err(paused());
        }
        Ok(GrantIndex::Attended(i))
    }

    /// Native workers bind this authority at acceptance and revalidate before execution and delivery.
    pub fn ai_session(&mut self, token: &str) -> AppResult<(u64, u64)> {
        let i = self.ai_authorize(token)?;
        let grant = self.ai.grant(i);
        Ok((grant.session_generation, grant.generation))
    }
    pub fn ai_catalogue(&mut self, token: &str) -> AppResult<Value> {
        let i = self.ai_authorize(token)?;
        let g = self.ai.grant(i);
        Ok(
            json!({"version":1,"company_id":g.company.to_db(),"actor_id":g.actor.to_db(),"session_generation":g.session_generation,"authority_generation":g.generation,"unattended":g.proof.is_some(),"permission":g.permission,"operations":catalogue(g.permission),"limitations":["No arbitrary SQL, filesystem, backup/protection, user management or provider-network operations.","Import supports structured posted journals only (1–100 per batch); preview IDs are provisional and apply revalidates current accounting setup and accounts.","Attended access pauses on lock, company change and sign-out. Unattended access needs the approved books to be open and a current Administrator approval; it renews automatically."]}),
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
                json!({"_agent_error":true,"error":"operation_refused","code":"operation_refused","message":message.replace(token,"[redacted]").chars().take(2048).collect::<String>()})
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
        let g = self.ai.grant(i);
        let permitted = catalogue(g.permission);
        if !permitted.iter().any(|s| s["name"] == call.operation) {
            return Err(denied());
        }
        let context = fb_core::agent_provenance::Context {
            connection_id: g.id.clone(),
            agent_name: g.name.clone(),
            operation: call.operation.clone(),
            request_id: uuid::Uuid::new_v4().to_string(),
            unattended: g.proof.is_some(),
        };
        let co = g.company;
        // Reviews made with `include_settlement` belong to the approval whose operation this is.
        self.ai.calling = Some(g.hash);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            fb_core::agent_provenance::with(context, || match i {
                GrantIndex::Unattended(index) => self.ai_with_actor(index, co, |app| app.ai_execute(i, co, call)),
                GrantIndex::Attended(_) => self.ai_execute(i, co, call),
            })
        }));
        self.ai.calling = None;
        result.unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    }

    // Only synchronous catalogue dispatch can enter this scope, under the same book lease as
    // authorization. Never set identity.actor or touch interactive sign-in or MFA state.
    fn ai_with_actor<T>(&mut self, i: usize, company: CompanyId, work: impl FnOnce(&mut Self) -> AppResult<T>) -> AppResult<T> {
        let proof = self.ai.unattended[i].proof.clone().ok_or_else(denied)?;
        let actor = self.ai.unattended[i].actor;
        let name = self.ai_validate_unattended(&proof)?;
        let open = self.open.as_mut().ok_or(AppError::Locked)?;
        let owner = std::mem::replace(&mut open.owner, actor);
        let owner_name = std::mem::replace(&mut open.owner_name, name);
        let staff = std::mem::replace(&mut open.staff, true);
        self.ai.execution = Some((actor, company));
        self.ai_swap_reviews(i);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(self)));
        self.ai_swap_reviews(i);
        self.ai.execution = None;
        if let Some(open) = self.open.as_mut() {
            open.owner = owner;
            open.owner_name = owner_name;
            open.staff = staff;
        }
        result.unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    }

    fn ai_swap_reviews(&mut self, index: usize) {
        let grant = &mut self.ai.unattended[index];
        std::mem::swap(&mut self.sales_reviews, &mut grant.sales_reviews);
        std::mem::swap(&mut self.d5_reviews, &mut grant.d5_reviews);
        std::mem::swap(&mut self.d6_reviews, &mut grant.d6_reviews);
        std::mem::swap(&mut self.d7a_reviews, &mut grant.d7a_reviews);
        std::mem::swap(&mut self.salestax_reviews, &mut grant.salestax_reviews);
    }
    fn ai_execute(&mut self, index: GrantIndex, co: CompanyId, call: Call) -> AppResult<Value> {
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
        if matches!(index, GrantIndex::Unattended(_)) {
            self.authorize(Some(co), permission)?;
            if permission == UserPermission::Write {
                self.prepare_connections_for_write()?;
            }
        } else {
            self.require_session(self.ai.grant(index).session_generation, Some(co), permission)?;
        }
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
                    json!({"company_id":co.to_db(),"company_name":company.name,"currency":company.currency,"actor_id":actor.to_db(),"actor_name":user.username,"generation":self.ai.attended.generation}),
                )
            }
            operations::Operation::InvoiceImportPreview { invoices } => {
                if self.ai.grant(index).invoice_previews.len() >= 8 {return Err(AppError::Invalid("At most eight invoice import previews per connection.".into()));}
                let rows=invoices.iter().map(|v|Ok(json!({"invoice":crate::session::invoice_input(&v.invoice)?,"source":v.source}))).collect::<AppResult<Vec<_>>>()?;
                let open=self.open_mut()?;
                let mut result=fb_core::sales::historical::run(&mut open.book,open.owner,co,&rows,true,None)?;
                let token=uuid::Uuid::new_v4().to_string();
                let hash=result["review_digest"].as_str().unwrap_or("").to_owned();
                self.ai.grant_mut(index).invoice_previews.insert(token.clone(),(rows,hash));
                result["preview_token"]=json!(token);Ok(result)
            }
            operations::Operation::InvoiceImportApply { preview_token } => {
                let (rows,hash)=self.ai.grant(index).invoice_previews.get(&preview_token).ok_or_else(denied)?.clone();
                let open=self.open_mut()?;
                Ok(fb_core::sales::historical::run(&mut open.book,open.owner,co,&rows,false,Some(&hash))?)
            }
            operations::Operation::ImportPreview { journals } => {
                if self.ai.grant(index).previews.len() >= 8 {
                    return Err(AppError::Invalid(
                        "At most eight import previews per connection. Reconnect to clear them."
                            .into(),
                    ));
                }
                let open = self.open_mut()?;
                let mut result = import::run(&mut open.book, open.owner, co, &journals, true)?;
                let token = uuid::Uuid::new_v4().to_string();
                self.ai.grant_mut(index)
                    .previews
                    .insert(token.clone(), journals);
                result["preview_token"] = json!(token);
                Ok(result)
            }
            operations::Operation::ImportApply { preview_token } => {
                let journals = self.ai.grant(index)
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
        "SaveCompanyDetails",
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
        "SaveTimeEntry",
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
        "SaveTimeEntry"=>"Record billable time for a customer: service item_id (a SERVICE item), date, hours as exact text (\"3.5\", \"3.5h\", \"3:30\"), optional ticket_id of that customer's open job ticket, optional rate (blank uses the item's price), billable and notes. worker_id defaults to the connected person. Use a stable UUID request_key. Time is not an accounting posting.".into(),
        "JobTickets"=>"List job tickets with customer, status, hours and unbilled billable amount. Optional customer_id and status (OPEN, COMPLETED, INVOICED, CLOSED, ACTIVE or ALL).".into(),
        "FeedReview"=>"Read feed accounts and all bank-review rows together in one company snapshot.".into(),
        "DecidedFeed"=>"List decided bank-feed rows and their recorded accounts, receipts and references (1–100 per page).".into(),
        "AcceptFeed"=>"Categorize/accept one feed transaction through existing CHECK, JOURNAL, TRANSFER, DEPOSIT or MATCH service; optional preconditions compare the reviewed row and mapping before any posting in the writer.".into(),
        "InvoiceImportPreview"=>"Preview mapped CyberBrain invoices with original source evidence, discounts, taxes, statuses and recorded payments. Uses real Writer services and rolls everything back.".into(),
        "InvoiceImportApply"=>"Apply a connection-bound invoice preview atomically, preserving source history and preventing duplicate invoices/payments. Does not call payment or email providers.".into(),
        "ImportPreview"=>"Validate a structured journal batch using real save/post services, roll back, and return a connection-bound preview_token. Provisional IDs can change.".into(),
        "ImportApply"=>"Revalidate and atomically post the previewed journal batch. Repeat with the same token is idempotent through stable journal request keys.".into(),
        "ReviewCustomerReceipt"=>"Review a customer receipt or sales receipt before recording it. Writes nothing. Returns the single-use review token. With include_settlement true it also returns `settlement`: for each invoice the receipt is allocated to, its total and outstanding amount and every receipt, credit and customer deposit counted toward it, reversed ones flagged, up to 500 entries in all; above that `settlement` is null and `settlement_unavailable` says so. A review made with include_settlement can be recorded only by the connection that made it. Money values use exact decimal strings; settlement amounts are whole cents as strings.".into(),
        "RecordCustomerReceipt"=>"Record the reviewed receipt with the review token from ReviewCustomerReceipt and the same values. With require_same_settlement true it is recorded only if that review was made by this connection with include_settlement, returned a settlement, and every allocated invoice's settlement is still what it returned; otherwise nothing is recorded and the message starts \"Receipt precondition failed\". Money values use exact decimal strings.".into(),
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
        app.ai.attended.grants[0].expires = Instant::now() - Duration::from_secs(1);
        assert!(app.ai_catalogue(token).is_err());
        let grant = app
            .ai_grant(co, "Generation agent", Permission::ReadOnly)
            .unwrap();
        let token = grant["token"].as_str().unwrap();
        app.ai.attended.generation += 1;
        assert!(app.ai_catalogue(token).is_err());
    }

    #[test]
    fn unattended_renews_an_expired_worker_and_refuses_a_restored_preview() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new();
        app.backups.configure(dir.path()).unwrap();
        let co = app.create_book(
            &dir.path().join("synthetic-unattended.fbbook"),
            &crate::keys::MasterKey::new([42; 32]),
            "Synthetic approver",
            "Synthetic unattended expiry",
        ).unwrap().company_id;
        app.enroll_owner("Synthetic unattended expiry password").unwrap();
        let grant = app.ai_grant(co, "Synthetic agent", Permission::ReadOnly).unwrap();
        let proof = app.ai_set_unattended(grant["id"].as_str().unwrap(), &uuid::Uuid::new_v4().to_string(), true).unwrap().to_string();
        app.sign_out();
        let worker = app.ai_prepare_unattended(&proof).unwrap();
        app.ai.unattended[0].expires = Instant::now() - Duration::from_secs(1);
        assert_eq!(app.ai_prepare_unattended(&proof).unwrap(), worker);
        let first_expiry = app.ai.unattended[0].expires;
        assert_eq!(app.ai_catalogue(&worker).unwrap()["unattended"], true);
        assert!(app.ai.unattended[0].expires >= first_expiry);
        assert!(app.authenticated_session().is_err());
        app.backups.set_viewing(Some("Synthetic preview".into()));
        assert_eq!(app.ai_prepare_unattended(&proof).unwrap_err().to_string(), "books_closed");
        assert_eq!(app.ai_catalogue(&worker).unwrap_err().to_string(), "books_closed");
    }

}
