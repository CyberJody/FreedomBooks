//! Durable, company-scoped email attempts. Provider calls happen outside a Writer.
//! SUBMITTING survives a crash as an uncertain result; it is never retried automatically.
use crate::{CompanyId, FbError, FbResult, UserId, py, services};
use fb_db::{
    Book, Query,
    rusqlite::{OptionalExtension, params},
};
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Connection {
    pub revision: i64,
    pub client_id: String,
    pub from_address: String,
    pub status: String,
}
pub fn connection(q: &(impl Query + ?Sized), c: CompanyId) -> FbResult<Option<Connection>> {
    Ok(q.query_row("SELECT revision,client_id,from_address,status FROM invoice_email_connection WHERE company_id=?1",
        [c.to_db()],|r|Ok(Connection{revision:r.get(0)?,client_id:r.get(1)?,from_address:r.get(2)?,status:r.get(3)?})).optional()?)
}
fn single(text: &str, max: usize) -> FbResult<String> {
    let text = py::py_strip(text);
    if text.is_empty() || text.chars().count() > max || text.chars().any(char::is_control) {
        return Err(FbError::validation(
            "Enter a single line within the field limit.",
        ));
    }
    py::clean_free_text(text)?;
    Ok(text.into())
}
fn mailbox(text: &str) -> FbResult<String> {
    let text = single(text, 254)?;
    if !crate::email_validation::valid_email(&text) {
        return Err(FbError::validation("Enter a valid email address."));
    }
    Ok(text)
}
fn stale() -> FbError {
    FbError::refused("This connection or invoice changed. Reopen the email preview.")
}

/// Reconfiguration invalidates every outstanding authorization and preparation.
pub fn begin_connection(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    expected: i64,
    client_id: &str,
    from: &str,
) -> FbResult<Connection> {
    let client_id = single(client_id, 512)?;
    let from = mailbox(from)?;
    book.write(|tx|{
        let user=services::access(tx,actor,c)?;
        let old=connection(tx,c)?;
        if old.as_ref().map_or(0,|v|v.revision)!=expected {return Err(stale());}
        let revision=expected.checked_add(1).ok_or_else(stale)?;
        if old.is_some() {
            tx.execute("UPDATE invoice_email_connection SET revision=?2,client_id=?3,from_address=?4,status='AUTHORIZING',updated_at=?5,updated_by_id=?6 WHERE company_id=?1",
                params![c.to_db(),revision,client_id,from,py::now_utc(),actor.to_db()])?;
        } else {
            tx.execute("INSERT INTO invoice_email_connection(company_id,revision,client_id,from_address,status,updated_at,updated_by_id) VALUES(?1,1,?2,?3,'AUTHORIZING',?4,?5)",
                params![c.to_db(),client_id,from,py::now_utc(),actor.to_db()])?;
        }
        services::audit(tx,&user,c,"invoice_email.authorization_started",format!("company:{c}"),json!({"revision":revision,"from_address":from}))?;
        Ok(Connection{revision,client_id,from_address:from,status:"AUTHORIZING".into()})
    })
}
#[derive(Clone, Copy)]
pub enum ConnectionState {
    Connected,
    NeedsSignIn,
    Disconnected,
}
impl ConnectionState {
    fn value(self) -> &'static str {
        match self {
            Self::Connected => "CONNECTED",
            Self::NeedsSignIn => "NEEDS_SIGNIN",
            Self::Disconnected => "DISCONNECTED",
        }
    }
}
/// The app saves the OS-sealed token before marking CONNECTED. A stale completion
/// cannot revive a disconnected or replaced registration.
pub fn set_connection_state(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    expected: i64,
    state: ConnectionState,
) -> FbResult<Connection> {
    book.write(|tx|{
        let user=services::access(tx,actor,c)?;
        let mut old=connection(tx,c)?.ok_or_else(stale)?;
        if old.revision!=expected || (matches!(state,ConnectionState::Connected)&&old.status!="AUTHORIZING") {return Err(stale());}
        let before=old.status.clone();old.revision=old.revision.checked_add(1).ok_or_else(stale)?;old.status=state.value().into();
        tx.execute("UPDATE invoice_email_connection SET revision=?2,status=?3,updated_at=?4,updated_by_id=?5 WHERE company_id=?1",
            params![c.to_db(),old.revision,old.status,py::now_utc(),actor.to_db()])?;
        services::audit(tx,&user,c,"invoice_email.connection_changed",format!("company:{c}"),json!({"before":before,"after":old.status,"revision":old.revision}))?;
        Ok(old)
    })
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Mode {
    Draft,
    Send,
}
impl Mode {
    pub fn value(self) -> &'static str {
        match self {
            Self::Draft => "DRAFT",
            Self::Send => "SEND",
        }
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct Attempt {
    pub id: String,
    pub invoice_id: String,
    pub invoice_revision_id: String,
    pub created_by_id: String,
    pub connection_revision: i64,
    pub mode: String,
    pub state: String,
    pub to_address: String,
    pub subject: String,
    pub message_key: String,
    pub document_hash: String,
    pub pdf_sha256: String,
    pub provider_message: String,
    pub provider_draft: String,
    pub error_code: String,
    pub created_at: String,
    pub completed_at: Option<String>,
}
fn row(r: &fb_db::rusqlite::Row<'_>) -> fb_db::rusqlite::Result<Attempt> {
    Ok(Attempt {
        id: r.get(0)?,
        invoice_id: r.get(1)?,
        invoice_revision_id: r.get(2)?,
        created_by_id: r.get(3)?,
        connection_revision: r.get(4)?,
        mode: r.get(5)?,
        state: r.get(6)?,
        to_address: r.get(7)?,
        subject: r.get(8)?,
        message_key: r.get(9)?,
        document_hash: r.get(10)?,
        pdf_sha256: r.get(11)?,
        provider_message: r.get(12)?,
        provider_draft: r.get(13)?,
        error_code: r.get(14)?,
        created_at: r.get(15)?,
        completed_at: r.get(16)?,
    })
}
const COLUMNS: &str = "id,invoice_id,invoice_revision_id,created_by_id,connection_revision,mode,state,to_address,subject,message_key,document_hash,pdf_sha256,provider_message,provider_draft,error_code,created_at,completed_at";
pub fn attempt(
    q: &(impl Query + ?Sized),
    actor: UserId,
    c: CompanyId,
    id: &str,
) -> FbResult<Attempt> {
    services::access(q, actor, c)?;
    q.query_row(
        &format!("SELECT {COLUMNS} FROM invoice_email_delivery WHERE company_id=?1 AND id=?2"),
        params![c.to_db(), id],
        row,
    )
    .optional()?
    .ok_or_else(|| FbError::missing("Invoice email"))
}
pub fn history(
    q: &(impl Query + ?Sized),
    actor: UserId,
    c: CompanyId,
    invoice: &str,
) -> FbResult<Vec<Attempt>> {
    services::access(q, actor, c)?;
    crate::sales::invoice(q, c, invoice)?;
    Ok(q.prepare(&format!("SELECT {COLUMNS} FROM invoice_email_delivery WHERE company_id=?1 AND invoice_id=?2 ORDER BY id DESC LIMIT 100"))?
        .query_map(params![c.to_db(),invoice],row)?.collect::<Result<_,_>>()?)
}
pub struct Preparation<'a> {
    pub invoice: &'a str,
    pub revision: &'a str,
    pub connection_revision: i64,
    pub mode: Mode,
    pub to: &'a str,
    pub subject: &'a str,
    pub document_hash: &'a str,
    pub checked_unknown: Option<&'a str>,
}
fn valid_hash(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || b"abcdef".contains(&b))
}
pub fn prepare(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    input: Preparation<'_>,
) -> FbResult<Attempt> {
    let to = mailbox(input.to)?;
    let subject = single(input.subject, 998)?;
    if !valid_hash(input.document_hash) {
        return Err(FbError::validation("Invalid email snapshot."));
    }
    book.write(|tx|{
        let user=services::access(tx,actor,c)?;
        let invoice=crate::sales::invoice(tx,c,input.invoice)?;
        if invoice["current"]["id"]!=input.revision || invoice["current"]["state"]!="OPEN" {return Err(stale());}
        let current=connection(tx,c)?.ok_or_else(stale)?;
        if current.status!="CONNECTED" || current.revision!=input.connection_revision {return Err(stale());}
        // One outstanding operation per invoice. An uncertain result needs an
        // explicit user acknowledgement before another attempt can be created.
        let unfinished:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM invoice_email_delivery WHERE company_id=?1 AND invoice_id=?2 AND state IN ('PREPARING','SUBMITTING'))",params![c.to_db(),input.invoice],|r|r.get(0))?;
        if unfinished {return Err(FbError::refused("An email attempt is still pending. Check its result before creating another."));}
        let previous:Option<(String,String)>=tx.query_row("SELECT id,state FROM invoice_email_delivery WHERE company_id=?1 AND invoice_id=?2 ORDER BY id DESC LIMIT 1",params![c.to_db(),input.invoice],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        let unknown=previous.as_ref().filter(|(_,state)|state=="UNKNOWN").map(|(id,_)|id.as_str());
        if unknown.is_some() && input.checked_unknown!=unknown {return Err(FbError::refused("Check Gmail and confirm before creating another email."));}
        let id=fb_ids::new_v7().to_string();let key=format!("fb-{id}@freedombooks.local");
        tx.execute("INSERT INTO invoice_email_delivery(id,company_id,invoice_id,invoice_revision_id,created_by_id,connection_revision,mode,state,to_address,subject,message_key,document_hash,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,'PREPARING',?8,?9,?10,?11,?12)",
            params![id,c.to_db(),input.invoice,input.revision,actor.to_db(),input.connection_revision,input.mode.value(),to,subject,key,input.document_hash,py::now_utc()])?;
        services::audit(tx,&user,c,"invoice_email.prepared",format!("invoice_email:{id}"),json!({"invoice":input.invoice,"revision":input.revision,"mode":input.mode,"to":to,"document_hash":input.document_hash,"checked_unknown":unknown}))?;
        attempt(tx,actor,c,&id)
    })
}
pub fn mark_submitting(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    id: &str,
    current_hash: &str,
    pdf: &[u8],
) -> FbResult<Attempt> {
    if pdf.len() > 8 * 1024 * 1024 || !pdf.starts_with(b"%PDF-") {
        return Err(FbError::validation(
            "The invoice PDF could not be prepared.",
        ));
    }
    book.write(|tx|{
        let user=services::access(tx,actor,c)?;let old=attempt(tx,actor,c,id)?;
        if old.created_by_id!=actor.to_db() || old.state!="PREPARING" || old.document_hash!=current_hash {return Err(stale());}
        let digest=format!("{:x}",Sha256::digest(pdf));
        tx.execute("UPDATE invoice_email_delivery SET state='SUBMITTING',pdf_sha256=?3 WHERE company_id=?1 AND id=?2",params![c.to_db(),id,digest])?;
        services::audit(tx,&user,c,"invoice_email.submitting",format!("invoice_email:{id}"),json!({"pdf_sha256":digest}))?;
        attempt(tx,actor,c,id)
    })
}
#[derive(Debug, Clone, Copy)]
pub enum Failure {
    Preparation,
    Provider,
    NeedsSignIn,
    Stale,
    Uncertain,
}
impl Failure {
    fn value(self) -> &'static str {
        match self {
            Self::Preparation => "preparation_failed",
            Self::Provider => "provider_failed",
            Self::NeedsSignIn => "needs_signin",
            Self::Stale => "stale",
            Self::Uncertain => "uncertain",
        }
    }
}
pub enum ResultRecord<'a> {
    Accepted {
        message: &'a str,
        draft: Option<&'a str>,
    },
    Refused(Failure),
    Unknown,
}
/// Explicit recovery, never provider submission. An administrator can recover an
/// abandoned operation even if its original operator has left or been disabled.
pub fn resolve_interrupted(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    id: &str,
) -> FbResult<Attempt> {
    book.write(|tx|{
        let user=services::access(tx,actor,c)?;let old=attempt(tx,actor,c,id)?;
        let admin:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM desktop_user WHERE user_id=?1 AND role='administrator')",[actor.to_db()],|r|r.get(0))?;
        if old.created_by_id!=actor.to_db() && !admin {return Err(FbError::denied("An administrator must resolve another operator’s interrupted email."));}
        let (state,error)=match old.state.as_str(){"PREPARING"=>("REFUSED","preparation_failed"),"SUBMITTING"=>("UNKNOWN","uncertain"),_=>return Err(FbError::refused("This email already has a recorded result."))};
        tx.execute("UPDATE invoice_email_delivery SET state=?3,error_code=?4,completed_at=?5 WHERE company_id=?1 AND id=?2",params![c.to_db(),id,state,error,py::now_utc()])?;
        services::audit(tx,&user,c,"invoice_email.interrupted_resolved",format!("invoice_email:{id}"),json!({"original_actor":old.created_by_id,"before":old.state,"after":state}))?;
        attempt(tx,actor,c,id)
    })
}
pub fn finish(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    id: &str,
    result: ResultRecord<'_>,
) -> FbResult<Attempt> {
    book.write(|tx|{
        let user=services::access(tx,actor,c)?;let old=attempt(tx,actor,c,id)?;
        if old.created_by_id!=actor.to_db() {return Err(FbError::denied("This email belongs to another operator."));}
        let (state,message,draft,error)=match result {
            ResultRecord::Accepted{message,draft}=>{if old.state!="SUBMITTING" {return Err(stale());}
                (if old.mode=="DRAFT" {"DRAFTED"}else{"SENT"},message,draft.unwrap_or(""),"")},
            ResultRecord::Refused(error)=>("REFUSED","","",error.value()),
            ResultRecord::Unknown=>{if old.state!="SUBMITTING" {return Err(stale());}("UNKNOWN","","","uncertain")},
        };
        tx.execute("UPDATE invoice_email_delivery SET state=?3,provider_message=?4,provider_draft=?5,error_code=?6,completed_at=?7 WHERE company_id=?1 AND id=?2",
            params![c.to_db(),id,state,message,draft,error,py::now_utc()])?;
        services::audit(tx,&user,c,"invoice_email.completed",format!("invoice_email:{id}"),json!({"state":state,"provider_message":message,"provider_draft":draft,"error_code":error}))?;
        attempt(tx,actor,c,id)
    })
}
