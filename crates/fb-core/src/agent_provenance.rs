//! Trusted host context. Each ordinary audit event gets a linked, append-only provenance event
//! in the SAME Writer transaction. Original domain evidence is never edited or re-shaped.
use crate::{CompanyId, FbResult, store::User};
use fb_db::{Writer, rusqlite::params};
use std::cell::RefCell;

#[derive(Clone)]
pub struct Context {
    pub connection_id: String,
    pub agent_name: String,
    pub operation: String,
    pub request_id: String,
}
thread_local! { static CURRENT: RefCell<Option<Context>> = const { RefCell::new(None) }; }
struct Restore(Option<Context>);
impl Drop for Restore {
    fn drop(&mut self) {
        CURRENT.with(|c| *c.borrow_mut() = self.0.take());
    }
}
/// Synchronous scope only: the App mutex is held for its whole lifetime. Unwind-safe and thread-local.
pub fn with<T>(context: Context, work: impl FnOnce() -> T) -> T {
    let _restore = Restore(CURRENT.with(|c| c.replace(Some(context))));
    work()
}
pub(crate) fn record(
    tx: &Writer<'_>,
    user: &User,
    company: CompanyId,
    event_id: &str,
) -> FbResult<()> {
    CURRENT.with(|c| {
        if let Some(c) = c.borrow().as_ref() {
            tx.execute("INSERT INTO ledger_auditevent(id,company_id,actor_id,actor_name,occurred_at,action,target,detail) VALUES(?1,?2,?3,?4,?5,'agent.provenance',?6,?7)", params![
                fb_ids::AuditEventId::new().to_db(), company.to_db(), user.id.to_db(), user.username,
                crate::py::now_utc(), format!("audit:{event_id}"),
                serde_json::json!({"event_id":event_id,"connection_id":c.connection_id,"agent_name":c.agent_name,"operation":c.operation,"request_id":c.request_id}).to_string()
            ])?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn synchronous_context_is_restored_after_panic_and_nested_calls() {
        let context = Context {
            connection_id: "synthetic".into(),
            agent_name: "agent".into(),
            operation: "test".into(),
            request_id: "request".into(),
        };
        assert!(CURRENT.with(|c| c.borrow().is_none()));
        let failed = std::panic::catch_unwind(|| {
            with(context.clone(), || {
                with(context.clone(), || {
                    assert_eq!(
                        CURRENT.with(|c| c.borrow().as_ref().unwrap().connection_id.clone()),
                        "synthetic"
                    )
                });
                panic!("synthetic failure");
            })
        });
        assert!(failed.is_err());
        assert!(CURRENT.with(|c| c.borrow().is_none()));
    }
}
