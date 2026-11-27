//! Private main-process capabilities. No renderer bridge registration.
use super::{app_blocking, app_error, fail};
use crate::boundary::{Class, at_door, guarded, within};
use napi::{Env, Result, Task, bindgen_prelude::AsyncTask};
use napi_derive::napi;
use zeroize::Zeroizing;

/// The company selected in this authenticated desktop session, including on global book pages.
/// No authority is created; ai_context and ai_grant still recheck company access.
#[napi]
pub fn ai_current_company() -> Result<Option<String>> {
    guarded("ai_current_company", Class::Read, |app| {
        Ok(fb_app::screens::frame_company(app).map(|company| company.to_db()))
    })
}

#[napi]
pub fn ai_context(company_id: String) -> Result<String> {
    guarded("ai_context", Class::Read, |app| {
        let company = fb_app::session::company(&company_id).map_err(app_error)?;
        Ok(app.ai_context(company).map_err(app_error)?.to_string())
    })
}

#[napi]
pub fn ai_grant(company_id: String, name: String, permission: String, id: Option<String>) -> Result<String> {
    guarded("ai_grant", Class::Write, |app| {
        let company = fb_app::session::company(&company_id).map_err(app_error)?;
        let permission = match permission.as_str() {
            "read_only" => fb_app::ai::Permission::ReadOnly,
            "accounting_write" => fb_app::ai::Permission::AccountingWrite,
            _ => return Err(fail("Invalid permission.")),
        };
        Ok(app.ai_grant_remembered(company, &name, permission, id.as_deref()).map_err(app_error)?.to_string())
    })
}
#[napi]
pub fn ai_connections() -> Result<String> {
    guarded("ai_connections", Class::Read, |app| Ok(app.ai_connections().to_string()))
}
#[napi]
pub fn ai_revoke(id: String) -> Result<()> {
    guarded("ai_revoke", Class::Write, |app| {
        app.ai_revoke(&id);
        Ok(())
    })
}
#[napi]
pub fn ai_invalidate() -> Result<()> {
    guarded("ai_invalidate", Class::Read, |app| {
        app.ai.invalidate();
        app.ai.clear_unattended();
        Ok(())
    })
}
#[napi]
pub fn ai_generation() -> Result<String> {
    guarded("ai_generation", Class::Read, |app| Ok(app.ai.generation().to_string()))
}
#[napi]
pub fn ai_set_unattended(id: String, epoch: String, enabled: bool) -> Result<String> {
    guarded("ai_set_unattended", Class::Write, |app| {
        Ok(app.ai_set_unattended(&id, &epoch, enabled).map_err(app_error)?.to_string())
    })
}
#[napi]
pub fn ai_unattended_status(proof: String) -> Result<Option<String>> {
    let proof = Zeroizing::new(proof);
    guarded("ai_unattended_status", Class::Read, |app| {
        Ok(app.ai_unattended_status(&proof).map_err(app_error)?.map(|event| event.to_string()))
    })
}
#[napi]
pub fn ai_forget_all() -> Result<bool> {
    guarded("ai_forget_all", Class::Write, |app| app.ai_forget_all().map_err(app_error))
}
#[napi]
pub fn ai_request_async(token: String, request: Option<String>, proof: Option<String>) -> Result<AsyncTask<AgentWork>> {
    guarded("ai_request_async", Class::Read, |app| {
        if token.len() != 64 || proof.as_ref().is_some_and(|p| p.len() > 4096)
            || request
                .as_ref()
                .is_some_and(|r| r.len() > fb_app::commands::MAX_REQUEST_BYTES)
        {
            return Err(fail("Invalid agent request."));
        }
        let proof = proof.map(Zeroizing::new);
        let token = if let Some(proof) = proof.as_ref() {
            app.ai_prepare_unattended(proof).map_err(app_error)?
        } else { token };
        let token = Zeroizing::new(token);
        Ok(AsyncTask::new(AgentWork {
            generation: app.ai_session(&token).map_err(app_error)?,
            token,
            request: request.map(Zeroizing::new),
        }))
    })
}
pub struct AgentWork {
    generation: (u64, u64),
    token: Zeroizing<String>,
    request: Option<Zeroizing<String>>,
}
impl AgentWork {
    fn current(&self, app: &mut fb_app::App) -> Result<()> {
        if self.generation != app.ai_session(&self.token).map_err(app_error)? {
            return Err(fail(if self.generation.0 == 0 { "books_closed" } else { "paused_signed_out" }));
        }
        Ok(())
    }
}
impl Task for AgentWork {
    type Output = String;
    type JsValue = String;
    fn compute(&mut self) -> Result<String> {
        // A pool thread, guarded like the main thread's calls (see boundary.rs).
        at_door("ai_request", Class::Write, app_blocking, |door| crate::hosting::with_desk(door, crate::hosting::TASK_WAIT, |app| within("ai_request", Class::Write, app, |app| {
            self.current(app)?;
            let value = app
                .ai_response(&self.token, self.request.as_deref().map(|s| s.as_str()))
                .map_err(app_error)?;
            self.current(app).map_err(|_| fail(if self.request.is_some() { "outcome_unknown" } else { "paused_signed_out" }))?;
            Ok(value.to_string())
        })))
    }
    fn resolve(&mut self, _env: Env, output: String) -> Result<String> {
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn agent_results_require_the_original_session_and_live_connection() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fb_app::App::new();
        let co = app.create_book(&dir.path().join("agent.fbbook"), &fb_app::keys::MasterKey::new([13;32]), "Agent test owner", "Synthetic agent").unwrap().company_id;
        let password = "Synthetic native individual password";
        app.enroll_owner(password).unwrap();
        for revoke in [true,false] {
            let grant = app.ai_grant(co, "Native result", fb_app::ai::Permission::ReadOnly).unwrap();
            let token = grant["token"].as_str().unwrap();
            let work = AgentWork { generation:app.ai_session(token).unwrap(),token:Zeroizing::new(token.into()),request:None };
            work.current(&mut app).unwrap();
            if revoke { app.ai_revoke(grant["id"].as_str().unwrap()); }
            else { app.sign_out(); app.sign_in("Agent test owner",password).unwrap(); }
            assert!(work.current(&mut app).is_err(),"an old response cannot be delivered after revoke or a new same-user session");
        }
    }
}
