//! Offline assistant core. Plugin credentials and all network calls stay outside this module.
//! A receipt is authoritative review evidence; only an explicit human Save creates a bill draft.
use crate::{
    Book, CompanyId, FbError, FbResult, Money, UserId, directory, payables, py, services, store,
};
use fb_db::{
    Query, Writer,
    rusqlite::{OptionalExtension, params},
};
use serde_json::{Value, json};
pub mod receipts;

pub const PURPOSE: &str = "receipt_to_draft_bill";
pub const NOT_CONNECTED: &str = "No AI account is connected for this company. A staff user connects one on the AI assistant page.";
pub const NOT_AVAILABLE: &str = "AI reading is not switched on for this company.";
pub const NEEDS_SIGNIN: &str =
    "The AI connection needs a new sign-in. A staff user signs in again on the AI assistant page.";
pub const SIGNIN_EXPIRED: &str = "Your ChatGPT sign-in is no longer valid. Sign in again on the AI assistant page.";
pub const KEY_REJECTED: &str = "OpenAI did not accept this API key. Check it on the OpenAI platform and paste it again.";
pub const PLAN_SCOPE_LOST: &str = "ChatGPT plan use is no longer granted to FreedomBooks, so nothing was sent. Sign in again on the AI assistant page and approve \"use your ChatGPT plan\", or use an API key instead.";

/// Fixed local client sentences only; never accepts provider text or URLs.
pub fn client_failure_message(code: &str) -> Option<&'static str> {
    Some(match code {
        "invalid_address" => "The OpenAI address could not be read.",
        "missing_host" => "The OpenAI address names no host.",
        "insecure_address" => "OpenAI addresses must use HTTPS.",
        "address_credentials" => "OpenAI addresses never carry credentials.",
        "redirect" => "OpenAI answered with a redirect; redirects are not followed.",
        "certificate" => "The OpenAI server certificate could not be verified.",
        "timeout" => "OpenAI did not answer in time. Try again later.",
        "unreachable" => "OpenAI could not be reached.",
        "response_unreadable" => "The OpenAI response could not be read.",
        "response_oversized" => "The OpenAI response was larger than allowed.",
        "token_incomplete" => "The token response was incomplete.",
        "metadata_no_key_set" => "OpenAI metadata names no key set.",
        "metadata_no_keys" => "OpenAI metadata holds no keys.",
        "invalid_model_list" => "OpenAI returned something that is not a model list.",
        "invalid_responses_request" => "A Responses request needs a model and an input list.",
        "stream_interrupted" => "The OpenAI stream was interrupted.",
        "stream_oversized" => "The OpenAI stream was larger than allowed.",
        _ => return None,
    })
}

/// Validated host lifecycle notification: two actions and three safe provider codes.
pub struct SigninFailure {
    action: &'static str,
    code: &'static str,
}
impl SigninFailure {
    pub fn new(action: &str, code: &str) -> FbResult<Self> {
        let action = match action {
            "ai.call_failed" => "ai.call_failed",
            "ai.refresh_failed" => "ai.refresh_failed",
            _ => return Err(err("Invalid assistant sign-in failure.")),
        };
        let code = match code {
            "unauthorized" => "unauthorized",
            "invalid_grant" => "invalid_grant",
            "plan_not_granted" => "plan_not_granted",
            _ => return Err(err("Invalid assistant sign-in failure.")),
        };
        Ok(Self { action, code })
    }
}
pub const OUT_OF_DATE: &str = "This form is out of date. Reload the page and try again.";
pub const STALE_ERROR: &str = "The earlier reading of this file never finished. Try again.";
fn err(s: impl Into<String>) -> FbError {
    FbError::validation(s)
}
fn write<T>(book: &mut Book, f: impl FnOnce(&Writer<'_>) -> FbResult<T>) -> FbResult<T> {
    book.write(|tx| tx.atomic(|| f(tx)))
}
fn staff(c: &(impl Query + ?Sized), actor: UserId, co: CompanyId) -> FbResult<store::User> {
    let u = services::access(c, actor, co)?;
    if !u.is_staff && !u.is_superuser {
        return Err(FbError::denied("Staff access required."));
    }
    Ok(u)
}
fn string<'a>(v: &'a Value, k: &str) -> FbResult<&'a str> {
    v[k].as_str()
        .ok_or_else(|| err(format!("Invalid assistant {k}.")))
}
fn uuid(s: &str) -> FbResult<String> {
    py::py_uuid(s).map(|v| v.0).ok_or_else(|| err(OUT_OF_DATE))
}
fn bool_field(v: &Value, k: &str) -> FbResult<bool> {
    v[k].as_bool().ok_or_else(|| err("Invalid checkbox value."))
}
fn json_row(
    c: &(impl Query + ?Sized),
    sql: &str,
    args: &[&dyn fb_db::rusqlite::ToSql],
) -> FbResult<Option<Value>> {
    let raw: Option<String> = c.query_row(sql, args, |r| r.get(0)).optional()?;
    raw.map(|s| fb_db::parse_unique_json(&s).map_err(|_| err("Invalid stored assistant evidence.")))
        .transpose()
}
fn connection(c: &(impl Query + ?Sized), co: CompanyId) -> FbResult<Option<Value>> {
    json_row(
        c,
        "SELECT json_object('id',id,'mode',mode,'status',status,'credential_present',json(CASE credential_present WHEN 1 THEN 'true' ELSE 'false' END),'account_email',account_email,'key_last4',key_last4,'model',model,'model_choices',json(model_choices),'plan_usage',json(CASE plan_usage WHEN 1 THEN 'true' ELSE 'false' END),'notice_pending',json(CASE notice_pending WHEN 1 THEN 'true' ELSE 'false' END),'notice_shown',json(CASE notice_shown WHEN 1 THEN 'true' ELSE 'false' END),'read_attachments',json(CASE read_attachments WHEN 1 THEN 'true' ELSE 'false' END),'revoke_confirmed',CASE WHEN revoke_confirmed IS NULL THEN NULL ELSE json(CASE revoke_confirmed WHEN 1 THEN 'true' ELSE 'false' END) END,'last_error',last_error,'token_expires_at',token_expires_at,'last_used_at',last_used_at,'generation',generation) FROM assistant_connection WHERE company_id=?",
        &[&co.to_db()],
    )
}
fn ensure(tx: &Writer<'_>, actor: UserId, co: CompanyId) -> FbResult<Value> {
    if let Some(v) = connection(tx, co)? {
        return Ok(v);
    }
    tx.execute(
        "INSERT INTO assistant_connection(id,company_id,created_by_id) VALUES(?,?,?)",
        params![fb_ids::new_v7().to_string(), co.to_db(), actor.to_db()],
    )?;
    connection(tx, co)?.ok_or_else(|| err("Missing assistant connection."))
}
pub fn connection_for(book: &mut Book, actor: UserId, co: CompanyId) -> FbResult<Option<Value>> {
    book.read(|c| {
        services::access(c, actor, co)?;
        connection(c, co)
    })
}
fn available(c: &(impl Query + ?Sized), co: CompanyId) -> FbResult<Value> {
    let row = connection(c, co)?.ok_or_else(|| err(NOT_CONNECTED))?;
    if row["read_attachments"] != true {
        return Err(err(NOT_AVAILABLE));
    }
    if row["status"] != "connected" || row["credential_present"] != true {
        return Err(err(if row["status"] == "needs_signin" {
            NEEDS_SIGNIN
        } else {
            NOT_CONNECTED
        }));
    }
    Ok(row)
}
pub fn check_consent(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    generation: i64,
) -> FbResult<()> {
    book.read(|c| {
        services::access(c, actor, co)?;
        let row = available(c, co)?;
        if row["generation"] != generation {
            return Err(err(OUT_OF_DATE));
        }
        Ok(())
    })
}
pub fn set_read_attachments(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    enabled: bool,
) -> FbResult<Value> {
    write(book, |tx| {
        let u = staff(tx, actor, co)?;
        let row = ensure(tx, actor, co)?;
        if row["read_attachments"] != enabled {
            tx.execute(
                "UPDATE assistant_connection SET read_attachments=? WHERE company_id=?",
                params![enabled, co.to_db()],
            )?;
            services::audit(
                tx,
                &u,
                co,
                "ai.read_attachments_set",
                "ai:openai".into(),
                json!({"enabled":enabled}),
            )?;
        }
        connection(tx, co)?.ok_or_else(|| err("Missing assistant connection."))
    })
}

/// Plain provider timestamps only. The host calls this after a renewal/completed response.
/// Credential updates stay in its scoped vault; stale generations may not update a new connection.
pub fn provider_metadata(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    generation: i64,
    expires: Option<i64>,
    used: Option<i64>,
) -> FbResult<()> {
    book.write(|tx| provider_metadata_tx(tx, actor, co, generation, expires, used))
}
pub(crate) fn provider_metadata_tx(
    tx: &Writer<'_>,
    actor: UserId,
    co: CompanyId,
    generation: i64,
    expires: Option<i64>,
    used: Option<i64>,
) -> FbResult<()> {
    tx.atomic(|| {
        services::access(tx, actor, co)?;
        let row = connection(tx, co)?
            .filter(|v| v["status"] == "connected" && v["credential_present"] == true)
            .ok_or_else(|| err(NOT_CONNECTED))?;
        if row["generation"] != generation {
            return Err(err(OUT_OF_DATE));
        }
        if expires.is_some_and(|n| n < 0) || used.is_some_and(|n| n < 0) {
            return Err(err("Invalid assistant timestamp."));
        }
        tx.execute("UPDATE assistant_connection SET token_expires_at=?,last_used_at=COALESCE(?,last_used_at) WHERE company_id=?",params![expires,used,co.to_db()])?;
        Ok(())
    })
}

/// Persist a verified provider lifecycle result; never accepts credentials or raw provider errors.
/// The host must commit its scoped credential-vault update together with this lifecycle notification.
pub fn connected(book: &mut Book, actor: UserId, co: CompanyId, state: &Value) -> FbResult<Value> {
    book.write(|tx| connected_tx(tx, actor, co, state))
}
pub(crate) fn connected_tx(
    tx: &Writer<'_>,
    actor: UserId,
    co: CompanyId,
    state: &Value,
) -> FbResult<Value> {
    tx.atomic(|| {
        let u = staff(tx, actor, co)?;
        let old = ensure(tx, actor, co)?;
        let mode = string(state, "mode")?;
        if !["siwc", "api_key"].contains(&mode)
            || !fb_db::assistant::choices(&state["model_choices"])
        {
            return Err(err("Invalid assistant connection state."));
        }
        let choices = state["model_choices"]
            .as_array()
            .ok_or_else(|| err("Invalid model list."))?;
        let preferred = if mode == "siwc" {
            old["model"].as_str().unwrap_or("")
        } else {
            ""
        };
        let model = choices
            .iter()
            .find(|v| v["slug"] == preferred)
            .or_else(|| choices.first())
            .and_then(|v| v["slug"].as_str())
            .unwrap_or("");
        let email = match state.get("account_email") {
            None => "",
            Some(v) => v
                .as_str()
                .ok_or_else(|| err("Invalid assistant account_email."))?,
        };
        let email = email.chars().take(254).collect::<String>();
        let models_error = match state.get("models_error") {
            None | Some(Value::Null) => "",
            Some(value) => client_failure_message(value.as_str().ok_or_else(|| err("Invalid assistant model-list failure."))?)
                .filter(|_| mode == "siwc")
                .ok_or_else(|| err("Invalid assistant model-list failure."))?,
        };
        let last4 = match state.get("key_last4") {
            None => "",
            Some(v) => v
                .as_str()
                .ok_or_else(|| err("Invalid assistant key_last4."))?,
        };
        let registration = match state.get("registration") {
            None => false,
            Some(v) => v
                .as_bool()
                .ok_or_else(|| err("Invalid assistant registration."))?,
        };
        let generation = old["generation"]
            .as_i64()
            .and_then(|n| n.checked_add(1))
            .ok_or_else(|| err("Assistant generation exhausted."))?;
        tx.execute("UPDATE assistant_connection SET mode=?,status='connected',credential_present=1,account_email=?,key_last4=?,model=?,model_choices=?,plan_usage=?,notice_pending=?,last_error=?,revoke_confirmed=NULL,token_expires_at=NULL,generation=? WHERE company_id=?",params![mode,if mode=="siwc"{email.as_str()}else{""},if mode=="api_key"{last4}else{""},model,state["model_choices"].to_string(),mode=="siwc",mode=="siwc" && old["notice_shown"]!=true,models_error.chars().take(300).collect::<String>(),generation,co.to_db()])?;
        let detail = if mode == "siwc" {
            json!({"mode":mode,"registration":registration,"models":choices.len()})
        } else {
            json!({"mode":mode})
        };
        services::audit(
            tx,
            &u,
            co,
            if mode == "siwc" {
                "ai.connected"
            } else {
                "ai.key_connected"
            },
            "ai:openai".into(),
            detail,
        )?;
        connection(tx, co)?.ok_or_else(|| err("Missing assistant connection."))
    })
}
pub fn set_model(book: &mut Book, actor: UserId, co: CompanyId, slug: &str) -> FbResult<Value> {
    write(book, |tx| {
        let u = staff(tx, actor, co)?;
        let row = connection(tx, co)?
            .filter(|v| v["status"] == "connected" && v["credential_present"] == true)
            .ok_or_else(|| err("Connect an account first."))?;
        let slug = py::py_strip(slug);
        if !row["model_choices"]
            .as_array()
            .is_some_and(|a| a.iter().any(|v| v["slug"] == slug))
        {
            return Err(err("Choose a model from the list."));
        }
        tx.execute(
            "UPDATE assistant_connection SET model=? WHERE company_id=?",
            params![slug, co.to_db()],
        )?;
        services::audit(
            tx,
            &u,
            co,
            "ai.model_set",
            "ai:openai".into(),
            json!({"mode":row["mode"],"model":slug}),
        )?;
        connection(tx, co)?.ok_or_else(|| err("Missing assistant connection."))
    })
}
pub fn refresh_models(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    choices: &Value,
) -> FbResult<Value> {
    write(book, |tx| {
        let u = staff(tx, actor, co)?;
        let row = connection(tx, co)?
            .filter(|v| v["status"] == "connected" && v["credential_present"] == true)
            .ok_or_else(|| err("Connect an account first."))?;
        if row["mode"] != "siwc" {
            return Err(err(
                "The API-key model list is set by the server administrator (FB_AI_KEY_MODELS).",
            ));
        }
        if !fb_db::assistant::choices(choices) {
            return Err(err("Invalid model list."));
        }
        let a = choices
            .as_array()
            .ok_or_else(|| err("Invalid model list."))?;
        let model = a
            .iter()
            .find(|v| v["slug"] == row["model"])
            .or_else(|| a.first())
            .and_then(|v| v["slug"].as_str())
            .unwrap_or("");
        tx.execute(
            "UPDATE assistant_connection SET model=?,model_choices=? WHERE company_id=?",
            params![model, choices.to_string(), co.to_db()],
        )?;
        services::audit(
            tx,
            &u,
            co,
            "ai.models_refreshed",
            "ai:openai".into(),
            json!({"mode":row["mode"],"models":a.len()}),
        )?;
        connection(tx, co)?.ok_or_else(|| err("Missing assistant connection."))
    })
}
/// `start_authorization`'s `_audit(..., 'ai.signin_started', {'mode': 'siwc', 'registration': …})`: a
/// ChatGPT sign-in began in the system browser. Records only; the connection row is unchanged.
pub fn signin_started(book: &mut Book, actor: UserId, co: CompanyId, registration: bool) -> FbResult<()> {
    write(book, |tx| {
        let u = staff(tx, actor, co)?;
        services::audit(tx, &u, co, "ai.signin_started", "ai:openai".into(), json!({"mode":"siwc","registration":registration}))
    })
}
/// The fixed failure codes a host may record for a ChatGPT sign-in (never provider text).
pub const SIGNIN_FAILURE_CODES: &[&str] =
    &["state_mismatch", "nonce_mismatch", "pkce_mismatch", "callback", "denied", "expired", "unreachable", "certificate", "response_unreadable", "failed"];
/// `complete_callback`'s `_audit(..., 'ai.signin_failed', {'mode': 'siwc', 'code': exc.code})`.
pub fn signin_failed(book: &mut Book, actor: UserId, co: CompanyId, code: &str) -> FbResult<()> {
    let code = SIGNIN_FAILURE_CODES.iter().find(|c| **c == code).ok_or_else(|| err("Invalid assistant sign-in failure."))?;
    write(book, |tx| {
        let u = staff(tx, actor, co)?;
        services::audit(tx, &u, co, "ai.signin_failed", "ai:openai".into(), json!({"mode":"siwc","code":code}))
    })
}
pub fn dismiss_notice(book: &mut Book, actor: UserId, co: CompanyId) -> FbResult<()> {
    write(book, |tx| {
        staff(tx, actor, co)?;
        tx.execute(
            "UPDATE assistant_connection SET notice_pending=0,notice_shown=1 WHERE company_id=?",
            [co.to_db()],
        )?;
        Ok(())
    })
}
pub fn disconnect(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    revoked: Option<bool>,
) -> FbResult<Value> {
    book.write(|tx| disconnect_tx(tx, actor, co, revoked))
}
pub(crate) fn disconnect_tx(
    tx: &Writer<'_>,
    actor: UserId,
    co: CompanyId,
    revoked: Option<bool>,
) -> FbResult<Value> {
    tx.atomic(|| {
        let u = staff(tx, actor, co)?;
        let row = connection(tx, co)?
            .filter(|v| v["status"] != "disconnected" || v["credential_present"] == true)
            .ok_or_else(|| err("Nothing is connected."))?;
        let generation = row["generation"].as_i64().and_then(|n| n.checked_add(1))
            .ok_or_else(|| err("Assistant generation exhausted."))?;
        tx.execute("UPDATE assistant_connection SET status='disconnected',credential_present=0,account_email='',key_last4='',model='',model_choices='[]',plan_usage=0,notice_pending=0,token_expires_at=NULL,revoke_confirmed=?,last_error=?,generation=? WHERE company_id=?",params![revoked,if revoked==Some(false){"OpenAI did not confirm the revocation; the sign-in was removed here. You can also remove FreedomBooks in ChatGPT Settings -> Security and login."}else{""},generation,co.to_db()])?;
        services::audit(
            tx,
            &u,
            co,
            "ai.disconnected",
            "ai:openai".into(),
            json!({"mode":row["mode"],"revoke_confirmed":revoked}),
        )?;
        connection(tx, co)?.ok_or_else(|| err("Missing assistant connection."))
    })
}
/// Installation-local credentials are unavailable after a portable restore. No provider text.
pub fn local_credentials_unavailable(book: &mut Book, actor: UserId, co: CompanyId) -> FbResult<()> {
    write(book, |tx| {
        let user = services::access(tx, actor, co)?;
        let Some(row) = connection(tx, co)?.filter(|v| v["status"] == "connected" && v["credential_present"] == true) else { return Ok(()) };
        let generation = row["generation"].as_i64().and_then(|n| n.checked_add(1)).ok_or_else(|| err("Assistant generation exhausted."))?;
        tx.execute("UPDATE assistant_connection SET status='needs_signin',credential_present=0,plan_usage=0,token_expires_at=NULL,last_error=?,generation=? WHERE company_id=?",params!["Reconnect needed. This computer cannot read the saved AI sign-in. Connect again on the AI assistant page.",generation,co.to_db()])?;
        services::audit(tx, &user, co, "ai.reconnect_needed", "ai:openai".into(), json!({"reason":"local_credentials_unavailable"}))
    })
}

pub fn needs_signin(book: &mut Book, actor: UserId, co: CompanyId, failure: SigninFailure) -> FbResult<()> {
    book.write(|tx| needs_signin_tx(tx, actor, co, failure))
}
pub(crate) fn needs_signin_tx(tx: &Writer<'_>, actor: UserId, co: CompanyId, failure: SigninFailure) -> FbResult<()> {
    tx.atomic(|| {
        let u = services::access(tx, actor, co)?;
        let row = connection(tx, co)?
            .filter(|v| v["status"] == "connected" && v["credential_present"] == true)
            .ok_or_else(|| err("Nothing is connected."))?;
        let generation = row["generation"].as_i64().and_then(|n| n.checked_add(1))
            .ok_or_else(|| err("Assistant generation exhausted."))?;
        let message = if failure.code == "plan_not_granted" { PLAN_SCOPE_LOST }
            else if row["mode"] == "api_key" { KEY_REJECTED } else { SIGNIN_EXPIRED };
        tx.execute("UPDATE assistant_connection SET status='needs_signin',credential_present=CASE mode WHEN 'siwc' THEN credential_present ELSE 0 END,plan_usage=0,token_expires_at=NULL,last_error=?,generation=? WHERE company_id=?",params![message,generation,co.to_db()])?;
        services::audit(tx, &u, co, failure.action, "ai:openai".into(), json!({"mode":row["mode"],"code":failure.code}))
    })
}

fn reading(c: &(impl Query + ?Sized), co: CompanyId, id: &str) -> FbResult<Value> {
    let v=json_row(c,"SELECT json_object('id',id,'request_key',request_key,'source_file_id',source_file_id,'bill_id',bill_id,'origin_bill_id',origin_bill_id,'original_name',original_name,'content_type',content_type,'size',size,'sha256',sha256,'status',status,'mode',mode,'model',model,'result',json(result),'error',error,'created_by_id',created_by_id,'created_at',created_at,'finished_at',finished_at,'attempt',attempt,'connection_generation',connection_generation) FROM assistant_reading WHERE company_id=? AND id=?",&[&co.to_db(),&id])?.ok_or_else(||err("Not a reading of this company."))?;
    if v["status"] == "ok" && !fb_db::assistant::result(&v["result"]) {
        return Err(err("Invalid stored assistant evidence."));
    }
    Ok(v)
}
pub fn get_reading(book: &mut Book, actor: UserId, co: CompanyId, id: &str) -> FbResult<Value> {
    book.read(|c| {
        services::access(c, actor, co)?;
        reading(c, co, id)
    })
}
fn target(v: &Value) -> String {
    v["bill_id"]
        .as_str()
        .map(|s| format!("bill:{s}"))
        .unwrap_or_else(|| "bill:new".into())
}
fn read_audit(
    tx: &Writer<'_>,
    actor: UserId,
    co: CompanyId,
    v: &Value,
    code: Option<&str>,
) -> FbResult<()> {
    let u = services::access(tx, actor, co)?;
    let mut detail = json!({"purpose":PURPOSE,"reading":v["id"],"attachment":if v["origin_bill_id"].is_string(){v["source_file_id"].clone()}else{Value::Null},"mode":v["mode"],"model":v["model"],"outcome":v["status"]});
    if let Some(code) = code {
        detail["code"] = json!(code);
    } else {
        detail["lines"] = json!(v["result"]["lines"].as_array().map_or(0, Vec::len));
        detail["notes"] = json!(v["result"]["notes"].as_array().map_or(0, Vec::len));
    }
    services::audit(
        tx,
        &u,
        co,
        if code.is_some() {
            "ai.receipt_failed"
        } else {
            "ai.receipt_read"
        },
        target(v),
        detail,
    )
}
/// Begin a read after the encrypted attachment owner verified the file's size/type/hash.
/// Returned attempt + connection_generation form the completion lease; neither is UI authority.
pub fn begin_read(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    key: &str,
    file: &Value,
    now: i64,
) -> FbResult<Value> {
    book.write(|tx| begin_read_tx(tx, actor, co, key, file, now))
}
pub(crate) fn begin_read_tx(
    tx: &Writer<'_>,
    actor: UserId,
    co: CompanyId,
    key: &str,
    file: &Value,
    now: i64,
) -> FbResult<Value> {
    tx.atomic(|| {
        services::access(tx, actor, co)?;
        let key = uuid(key)?;
        if now < 0 {
            return Err(err("Invalid receipt clock."));
        }
        let bill = match file.get("bill_id") {
            None | Some(Value::Null) => None,
            Some(v) => Some(
                v.as_str()
                    .ok_or_else(|| err("Invalid assistant bill_id."))?,
            ),
        };
        if let Some(bid) = bill {
            let draft:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM payables_bill b JOIN payables_billrevision r ON r.bill_id=b.id AND r.number=b.revision WHERE b.id=? AND b.company_id=? AND r.state='DRAFT')",params![bid,co.to_db()],|r|r.get(0))?;
            if !draft {
                return Err(err(
                    "Only an attachment of a Draft bill can be read into that bill.",
                ));
            }
        }
        let conn = available(tx, co)?;
        let mut st=tx.prepare("SELECT id FROM assistant_reading WHERE company_id=? AND status='pending' AND created_at<? ORDER BY id")?;
        let stale = st
            .query_map(params![co.to_db(), now.saturating_sub(600)], |r| {
                r.get::<_, String>(0)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        for id in stale {
            tx.execute(
                "UPDATE assistant_reading SET status='failed',error=? WHERE id=?",
                params![STALE_ERROR, id],
            )?;
            read_audit(tx, actor, co, &reading(tx, co, &id)?, Some("stale"))?;
        }
        let earlier: Option<String> = tx
            .query_row(
                "SELECT id FROM assistant_reading WHERE company_id=? AND request_key=?",
                params![co.to_db(), key],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = earlier.as_deref() {
            let old = reading(tx, co, id)?;
            if old["sha256"] != file["sha256"]
                || (old["origin_bill_id"].is_string() && old["source_file_id"] != file["source_file_id"])
                || old["origin_bill_id"] != file.get("bill_id").cloned().unwrap_or(Value::Null)
            {
                return Err(err(OUT_OF_DATE));
            }
            if old["status"] != "failed" || !old["finished_at"].is_null() {
                return Ok(old);
            }
        }
        let count:i64=tx.query_row("SELECT count(*) FROM assistant_reading WHERE company_id=? AND created_by_id=? AND created_at>=?",params![co.to_db(),actor.to_db(),now.saturating_sub(600)],|r|r.get(0))?;
        if count >= 30 {
            return Err(err(
                "Too many receipts were read in the last few minutes. Wait a few minutes and try again.",
            ));
        }
        let id = earlier.unwrap_or_else(|| fb_ids::new_v7().to_string());
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM assistant_reading WHERE id=?)",
            [&id],
            |r| r.get(0),
        )?;
        if exists {
            let attempt = reading(tx, co, &id)?["attempt"].as_i64().and_then(|n| n.checked_add(1))
                .ok_or_else(|| err("Receipt attempt exhausted."))?;
            tx.execute("UPDATE assistant_reading SET status='pending',error='',result='{}',created_at=?,finished_at=NULL,attempt=?,mode=?,model=?,connection_generation=?,source_file_id=?,original_name=?,content_type=?,size=? WHERE id=?",params![now,attempt,"","",conn["generation"].as_i64(),uuid(string(file,"source_file_id")?)?,string(file,"original_name")?,string(file,"content_type")?,file["size"].as_i64().ok_or_else(||err("Invalid receipt size."))?,id])?;
        } else {
            tx.execute("INSERT INTO assistant_reading(id,company_id,created_by_id,request_key,source_file_id,bill_id,origin_bill_id,original_name,content_type,size,sha256,mode,model,created_at,connection_generation) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",params![id,co.to_db(),actor.to_db(),key,uuid(string(file,"source_file_id")?)?,bill,bill,string(file,"original_name")?,string(file,"content_type")?,file["size"].as_i64().ok_or_else(||err("Invalid receipt size."))?,string(file,"sha256")?,"","",now,conn["generation"].as_i64()])?;
        }
        reading(tx, co, &id)
    })
}
/// Persist the validated plugin result. A late/stale callback cannot overwrite a retried reading.
pub fn finish_read(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    id: &str,
    attempt: i64,
    result: &Value,
    now: i64,
) -> FbResult<Value> {
    book.write(|tx| finish_read_tx(tx, actor, co, id, attempt, result, now))
}
pub(crate) fn finish_read_tx(
    tx: &Writer<'_>,
    actor: UserId,
    co: CompanyId,
    id: &str,
    attempt: i64,
    result: &Value,
    now: i64,
) -> FbResult<Value> {
    tx.atomic(|| {
        services::access(tx, actor, co)?;
        let old = reading(tx, co, id)?;
        if old["status"] != "pending" || old["attempt"] != attempt {
            return Err(err(OUT_OF_DATE));
        }
        let conn = available(tx, co)?;
        if conn["generation"] != old["connection_generation"] {
            return Err(err(OUT_OF_DATE));
        }
        if !fb_db::assistant::result(result) {
            return Err(err("Invalid receipt reading."));
        }
        tx.execute("UPDATE assistant_reading SET status='ok',result=?,finished_at=?,mode=?,model=? WHERE id=?",params![result.to_string(),now,string(&conn,"mode")?,string(&conn,"model")?,id])?;
        let v = reading(tx, co, id)?;
        read_audit(tx, actor, co, &v, None)?;
        Ok(v)
    })
}
/// Only closed failure codes enter audit; provider text must never become persistent error text.
pub fn fail_read(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    id: &str,
    attempt: i64,
    code: &str,
    now: i64,
) -> FbResult<Value> {
    book.write(|tx| fail_read_tx(tx, actor, co, id, attempt, code, now))
}
pub(crate) fn fail_read_tx(
    tx: &Writer<'_>,
    actor: UserId,
    co: CompanyId,
    id: &str,
    attempt: i64,
    code: &str,
    now: i64,
) -> FbResult<Value> {
    if let Some(message) = client_failure_message(code) {
        return fail_validated_tx(tx, actor, co, id, attempt, ("failed", message), now);
    }
    let message = match code {
        "unauthorized" | "not_connected" | "invalid_grant" => NEEDS_SIGNIN,
        "not_available" => NOT_AVAILABLE,
        "limit" => {
            "Your plan's usage limit for FreedomBooks has been reached, so the receipt was not read. Try again later, or check Manage usage on the AI assistant page."
        }
        "schema" => "The AI reply did not have the expected fields, so nothing was filled in.",
        "no_model" => "Choose a model on the AI assistant page first.",
        "incomplete" => "The AI reply was cut short, so nothing was filled in. Try again.",
        "empty_reply" => "The AI returned no reading for this file, so nothing was filled in.",
        "plan_not_granted" => {
            "ChatGPT plan use is no longer granted to FreedomBooks, so nothing was sent. Sign in again on the AI assistant page and approve \"use your ChatGPT plan\", or use an API key instead."
        }
        "crashed" => {
            "Reading this file failed unexpectedly, so nothing was filled in. Try again; the server log has the details."
        }
        _ => return Err(err("Invalid receipt failure code.")),
    };
    fail_validated_tx(tx, actor, co, id, attempt, (code, message), now)
}
fn fail_validated_tx(
    tx: &Writer<'_>,
    actor: UserId,
    co: CompanyId,
    id: &str,
    attempt: i64,
    failure: (&str, &str),
    now: i64,
) -> FbResult<Value> {
    tx.atomic(|| {
        services::access(tx,actor,co)?;let old=reading(tx,co,id)?;
        if old["status"]!="pending" || old["attempt"]!=attempt{return Err(err(OUT_OF_DATE));}
        let conn=connection(tx,co)?.ok_or_else(||err(NOT_CONNECTED))?;
        if conn["generation"]!=old["connection_generation"]{return Err(err(OUT_OF_DATE));}
        let (code,message)=failure;let message=message.chars().take(300).collect::<String>();
        tx.execute("UPDATE assistant_reading SET status='failed',error=?,finished_at=?,mode=?,model=? WHERE id=?",params![message,now,string(&conn,"mode")?,string(&conn,"model")?,id])?;
        let v=reading(tx,co,id)?;read_audit(tx,actor,co,&v,Some(code))?;Ok(v)
    })
}

/// One structured receipt response and the host's injected local calendar date. Never log it.
pub struct Reply<'a> {
    pub text: &'a str,
    pub today: &'a str,
}
/// Parse AND persist a receipt outcome. A local validation failure is a finished failed reading,
/// preserving the frozen plain-language error, rather than an abandoned pending request.
pub fn complete_reply(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    id: &str,
    attempt: i64,
    reply: Reply<'_>,
    now: i64,
) -> FbResult<Value> {
    book.write(|tx| complete_reply_tx(tx, actor, co, id, attempt, reply, now))
}
pub(crate) fn complete_reply_tx(
    tx: &Writer<'_>,
    actor: UserId,
    co: CompanyId,
    id: &str,
    attempt: i64,
    reply: Reply<'_>,
    now: i64,
) -> FbResult<Value> {
    tx.atomic(|| {
        services::access(tx, actor, co)?;
        let old = reading(tx, co, id)?;
        let conn = available(tx, co)?;
        if old["status"] != "pending"
            || old["attempt"] != attempt
            || old["connection_generation"] != conn["generation"]
        {
            return Err(err(OUT_OF_DATE));
        }
        let currency = store::get_company(tx, co)?.currency;
        match receipts::parse_reply(reply.text, &currency, reply.today) {
            Ok(result) => finish_read_tx(tx, actor, co, id, attempt, &result, now),
            Err(error) => {
                let message = error.message();
                let code = error.code;
                fail_validated_tx(tx, actor, co, id, attempt, (code, &message), now)
            }
        }
    })
}

/// Local review queue: successful unlinked readings remain suggestions until a person saves.
pub fn review_queue(book: &mut Book, actor: UserId, co: CompanyId) -> FbResult<Vec<Value>> {
    book.read(|c|{
    services::access(c,actor,co)?;let mut s=c.prepare("SELECT id FROM assistant_reading WHERE company_id=? AND status='ok' AND bill_id IS NULL ORDER BY created_at DESC,id DESC")?;
    let ids=s.query_map([co.to_db()],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
    ids.iter().map(|id|reading(c,co,id)).collect()
})
}

fn initial(c: &(impl Query + ?Sized), co: CompanyId, row: &Value) -> FbResult<Value> {
    let result = &row["result"];
    if row["status"] != "ok" || !fb_db::assistant::result(result) {
        return Err(err("Not a successful reading."));
    }
    let name = string(result, "vendor_name")?;
    let wanted = receipts::normalize_name(name);
    let mut matches = directory::list(c, co, directory::Model::Vendor)?
        .into_iter()
        .filter(|v| {
            v["active"] == true
                && !wanted.is_empty()
                && [
                    v["display_name"].as_str().unwrap_or(""),
                    v["business_name"].as_str().unwrap_or(""),
                ]
                .iter()
                .any(|s| receipts::normalize_name(s) == wanted)
        })
        .collect::<Vec<_>>();
    matches.sort_by_key(|v| v["id"].as_str().unwrap_or("").to_owned());
    let vendor = if matches.len() == 1 {
        matches.first()
    } else {
        None
    };
    let vid = vendor.and_then(|v| v["id"].as_str());
    let defaults = directory::prefill_catalog(c, co)?;
    let hint = vid.map(|id| &defaults[id]);
    let terms = hint.and_then(|v| v["terms"].as_str()).unwrap_or("");
    let mut header = json!({"reference":result["document_number"],"vendor":vid});
    let dt = string(result, "document_date")?;
    let mut due = string(result, "due_date")?.to_string();
    let mut notes = result["notes"]
        .as_array()
        .cloned()
        .ok_or_else(|| err("Invalid receipt notes."))?;
    let note = if name.is_empty() {
        "The receipt names no vendor; choose one.".into()
    } else if let Some(v) = vendor {
        format!(
            "Vendor matched by name to \"{}\"; check it.",
            string(v, "display_name")?
        )
    } else if matches.is_empty() {
        format!(
            "No active vendor is named \"{name}\". Choose one, or add it (the link is prefilled), then reload this page."
        )
    } else {
        format!("Several active vendors match \"{name}\"; choose the right one.")
    };
    notes.insert(0, json!(note));
    if !dt.is_empty() {
        header["date"] = json!(dt);
        if due.is_empty() {
            let day = chrono::NaiveDate::parse_from_str(dt, "%Y-%m-%d")
                .map_err(|_| err("Invalid receipt date."))?;
            let days = if terms.is_empty() {
                0
            } else {
                terms
                    .parse::<u64>()
                    .map_err(|_| err("Invalid vendor terms."))?
            };
            due = day
                .checked_add_days(chrono::Days::new(days))
                .ok_or_else(|| err("Invalid receipt date."))?
                .to_string();
            notes.push(json!(format!(
                "No usable due date was printed; it was set from {}. Check it.",
                if terms.is_empty() {
                    "the bill date".into()
                } else {
                    format!("the vendor's terms (Net {days})")
                }
            )));
        }
    }
    if !due.is_empty() {
        header["due_date"] = json!(due);
    }
    if !terms.is_empty() {
        header["terms"] = json!(terms);
    }
    let rows = result["lines"]
        .as_array()
        .ok_or_else(|| err("Invalid receipt lines."))?;
    let lines=rows.iter().enumerate().map(|(i,l)|{
        let mut v=json!({"description":l["description"],"quantity":l["quantity"],"rate":l["rate"],"ai_line":i+1,"expense_account":hint.and_then(|v|v["expense_account"].as_str()).filter(|s|!s.is_empty()),"amount":l["amount"],"mismatch":l["mismatch"]==true});
        if l["mismatch"]==true {let q=fb_db::assistant::scaled(&l["quantity"],4,999_999_999_999_999_999,true).ok_or_else(||err("Invalid stored quantity."))?;let r=fb_db::assistant::scaled(&l["rate"],2,fb_db::assistant::MAX,false).ok_or_else(||err("Invalid stored rate."))?;v["product"]=json!(Money::from_cents(fb_db::assistant::product(q,r).ok_or_else(||err("Invalid receipt product."))?).to_string());}Ok(v)
    }).collect::<FbResult<Vec<_>>>()?;
    Ok(
        json!({"initial":header,"lines":lines,"marked":["date","due_date","reference","vendor"],"notes":notes,"vendor":{"name":name,"matched":vid,"candidates":matches.iter().map(|v|v["id"].clone()).collect::<Vec<_>>(),"new_vendor":{"display_name":name.chars().take(120).collect::<String>(),"billing_address":result["vendor_address"].as_array().ok_or_else(||err("Invalid receipt address."))?.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n").chars().take(1000).collect::<String>()}},"warnings":result["warnings"],"mismatched":rows.iter().any(|l|l["mismatch"]==true),"file_name":row["original_name"],"total":result["total"],"subtotal":result["subtotal"],"tax":result["tax"]}),
    )
}
pub fn form_initial(book: &mut Book, actor: UserId, co: CompanyId, id: &str) -> FbResult<Value> {
    book.read(|c| {
        services::access(c, actor, co)?;
        initial(c, co, &reading(c, co, id)?)
    })
}

/// Python int(str(hidden_index or '')) syntax, with a bounded positive result.
fn ai_index(value: &Value, count: usize) -> Option<usize> {
    if let Some(n) = value.as_u64() {
        return usize::try_from(n).ok().filter(|n| *n >= 1 && *n <= count);
    }
    let text = value
        .as_str()?
        .trim_matches(|c| fb_money::py_isspace(c) && !('\u{1c}'..='\u{1f}').contains(&c));
    let text = text.strip_prefix('+').unwrap_or(text);
    let mut n = 0usize;
    let mut previous_digit = false;
    for c in text.chars() {
        if c == '_' {
            if !previous_digit {
                return None;
            }
            previous_digit = false;
            continue;
        }
        let digit = fb_money::unicode_decimal(c)? as usize;
        n = n.checked_mul(10)?.checked_add(digit)?;
        if n > count {
            return None;
        }
        previous_digit = true;
    }
    (previous_digit && n >= 1).then_some(n)
}

/// Human Save. Printed amounts are loaded from stored reading, never from submitted form values.
/// ai_line binds a row to the reading's 1-based line; deleted/new/unbound rows follow Python's rules.
pub fn save_review(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    id: &str,
    payload: &Value,
) -> FbResult<Value> {
    book.write(|tx| save_review_tx(tx, actor, co, id, payload))
}
pub(crate) fn save_review_tx(
    tx: &Writer<'_>,
    actor: UserId,
    co: CompanyId,
    id: &str,
    payload: &Value,
) -> FbResult<Value> {
    tx.atomic(|| {
        let u = services::access(tx, actor, co)?;
        let row = reading(tx, co, id)?;
        if row["status"] != "ok" {
            return Err(err("Not a successful reading."));
        }
        let bid = payload.get("bill_id").cloned().unwrap_or(Value::Null);
        if row["bill_id"] != bid {
            return Err(err(OUT_OF_DATE));
        }
        let printed = row["result"]["lines"]
            .as_array()
            .ok_or_else(|| err("Invalid receipt lines."))?;
        let mut p = payload.clone();
        let mut lines = vec![];
        let mut kept = vec![];
        let inputs = payload["lines"]
            .as_array()
            .ok_or_else(|| err("Enter between 1 and 50 meaningful lines."))?;
        for (i, input) in inputs.iter().enumerate() {
            if input.get("DELETE").is_some() && bool_field(input, "DELETE")? {
                continue;
            }
            let keep=input.get("keep_as_entered").map(|_|bool_field(input,"keep_as_entered")).transpose()?.unwrap_or(false);
            let q = crate::calculations::quantity(&input["quantity"])?;
            let r = crate::calculations::unit_rate(&input["rate"])?;
            let product = fb_db::assistant::product(q.mantissa(), r.mantissa())
                .ok_or_else(|| err("Receipt amount exceeds supported limits."))?;
            let index=input.get("ai_line").and_then(|value|ai_index(value,printed.len()));
            if let Some(index) = index {
                let amount = fb_db::assistant::scaled(
                    &printed[index - 1]["amount"],
                    2,
                    fb_db::assistant::EVIDENCE_MAX,
                    false,
                )
                .ok_or_else(|| err("Invalid receipt amount."))?;
                if amount != product {
                    if !keep {
                        return Err(err(format!(
                            "This line doesn't add up: {} × {} = {}, the receipt shows {}. Change the quantity or price, or tick 'Keep as entered'.",
                            receipts::plain(q.mantissa()),
                            r,
                            Money::from_cents(product),
                            Money::from_cents(amount)
                        )));
                    }
                    kept.push(json!({"line":i+1,"read":index,"quantity":receipts::plain(q.mantissa()),"rate":r.to_string(),"charged":Money::from_cents(product).to_string(),"printed":Money::from_cents(amount).to_string()}));
                }
            }
            let mut line = input.clone();
            let m = line
                .as_object_mut()
                .ok_or_else(|| err("Invalid bill line."))?;
            for key in ["ai_line", "keep_as_entered", "DELETE"] {
                m.remove(key);
            }
            lines.push(line);
        }
        p["lines"] = json!(lines);
        let bill = payables::save_bill_tx(tx, actor, co, &p)?;
        tx.execute(
            "UPDATE assistant_reading SET bill_id=? WHERE id=?",
            params![string(&bill, "id")?, id],
        )?;
        if !kept.is_empty() {
            let reason = kept
                .iter()
                .map(|k| {
                    format!(
                        "Line {}: {} × {} = {} charged, receipt shows {}",
                        k["line"],
                        k["quantity"].as_str().unwrap_or(""),
                        k["rate"].as_str().unwrap_or(""),
                        k["charged"].as_str().unwrap_or(""),
                        k["printed"].as_str().unwrap_or("")
                    )
                })
                .collect::<Vec<_>>()
                .join("; ")
                .chars()
                .take(500)
                .collect::<String>();
            services::audit(
                tx,
                &u,
                co,
                "bill.ai_lines_kept",
                format!("bill:{}", string(&bill, "id")?),
                json!({"purpose":PURPOSE,"reading":id,"lines":kept,"reason":reason}),
            )?;
        }
        Ok(bill)
    })
}

/// CoreBridge draft entry point. Host persists the full reading FIRST, indexed by source_remote_id.
/// The existing plugin DraftBill omits printed amounts; this method verifies every supplied field
/// against that authoritative artifact and returns its draft reference. It never performs human Save.
pub fn create_draft_bill(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    source_remote_id: &str,
    draft: &Value,
) -> FbResult<String> {
    book.read(|c| {
        services::access(c, actor, co)?;
        available(c, co)?;
        let source = uuid(source_remote_id)?;
        let id: String = c
            .query_row(
                "SELECT id FROM assistant_reading WHERE company_id=? AND request_key=?",
                params![co.to_db(), source],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| err("Persist the complete receipt reading before staging a draft."))?;
        let row = reading(c, co, &id)?;
        let result = &row["result"];
        if row["status"] != "ok"
            || !row["bill_id"].is_null()
            || draft["vendor"] != result["vendor_name"]
            || draft["date"] != result["document_date"]
        {
            return Err(err(OUT_OF_DATE));
        }
        let lines = draft["lines"]
            .as_array()
            .ok_or_else(|| err("Invalid draft lines."))?;
        let stored = result["lines"]
            .as_array()
            .ok_or_else(|| err("Invalid receipt lines."))?;
        if lines.len() != stored.len() {
            return Err(err(OUT_OF_DATE));
        }
        for (l, s) in lines.iter().zip(stored) {
            let q = fb_db::assistant::scaled(&s["quantity"], 4, 999_999_999_999_999_999, true)
                .ok_or_else(|| err("Invalid receipt quantity."))?;
            let r = fb_db::assistant::scaled(&s["rate"], 2, fb_db::assistant::MAX, false)
                .ok_or_else(|| err("Invalid receipt rate."))?;
            if s["mismatch"] == true
                || l["description"] != s["description"]
                || l["quantity"].as_i64().map(i128::from) != Some(q)
                || l["unit_price_cents"].as_i64().map(i128::from) != Some(r)
                || l["currency"] != result["currency"]
                || l["tax_collected_cents"] != 0
            {
                return Err(err(OUT_OF_DATE));
            }
        }
        Ok(id)
    })
}

#[cfg(test)]
mod tests;
