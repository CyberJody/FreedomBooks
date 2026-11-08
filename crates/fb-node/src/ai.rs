//! Private main-process capabilities. No renderer bridge registration.
use super::{app, app_blocking, app_error, fail};
use napi::{Env, Result, Task, bindgen_prelude::AsyncTask};
use napi_derive::napi;
use zeroize::Zeroizing;

#[napi]
pub fn ai_grant(company_id: String, name: String, permission: String) -> Result<String> {
    let company = fb_app::session::company(&company_id).map_err(app_error)?;
    let permission = match permission.as_str() {
        "read_only" => fb_app::ai::Permission::ReadOnly,
        "accounting_write" => fb_app::ai::Permission::AccountingWrite,
        _ => return Err(fail("Invalid permission.")),
    };
    Ok(app()?
        .ai_grant(company, &name, permission)
        .map_err(app_error)?
        .to_string())
}
#[napi]
pub fn ai_connections() -> Result<String> {
    Ok(app()?.ai_connections().to_string())
}
#[napi]
pub fn ai_revoke(id: String) -> Result<()> {
    app()?.ai_revoke(&id);
    Ok(())
}
#[napi]
pub fn ai_invalidate() -> Result<()> {
    app()?.ai.invalidate();
    Ok(())
}
#[napi]
pub fn ai_generation() -> Result<String> {
    Ok(app()?.ai.generation().to_string())
}
#[napi]
pub fn ai_request_async(token: String, request: Option<String>) -> Result<AsyncTask<AgentWork>> {
    if token.len() != 64
        || request
            .as_ref()
            .is_some_and(|r| r.len() > fb_app::commands::MAX_REQUEST_BYTES)
    {
        return Err(fail("Invalid agent request."));
    }
    Ok(AsyncTask::new(AgentWork {
        generation: app()?.ai_session(&token).map_err(app_error)?,
        token: Zeroizing::new(token),
        request: request.map(Zeroizing::new),
    }))
}
pub struct AgentWork {
    generation: (u64, u64),
    token: Zeroizing<String>,
    request: Option<Zeroizing<String>>,
}
impl AgentWork {
    fn current(&self, app: &mut fb_app::App) -> Result<()> {
        if self.generation != app.ai_session(&self.token).map_err(app_error)? {
            return Err(fail("The active user or AI connection changed. Reconnect the AI."));
        }
        Ok(())
    }
}
impl Task for AgentWork {
    type Output = String;
    type JsValue = String;
    fn compute(&mut self) -> Result<String> {
        let mut app = app_blocking()?;
        self.current(&mut app)?;
        let value = app
            .ai_response(&self.token, self.request.as_deref().map(|s| s.as_str()))
            .map_err(app_error)?;
        Ok(value.to_string())
    }
    fn resolve(&mut self, _env: Env, output: String) -> Result<String> {
        self.current(&mut *app()?)?;
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
