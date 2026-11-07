//! Bank-feed entry masking plus the sealed central Book policy.
use super::{SecretSet, staging};
use crate::store::User;
use crate::{CompanyId, FbError, FbResult, services};
use fb_db::Writer;
use fb_db::rusqlite::types::{ToSql, ToSqlOutput, Value as SqlValue, ValueRef};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::HashMap;
pub(super) fn retain_secrets(book: &mut crate::Book, secrets: &SecretSet) -> FbResult<()> {
    Ok(book.retain_bankfeed_mask(&secrets.0)?)
}
fn safe_scalar(secrets: &SecretSet, text: &str) -> String {
    fb_db::credential_mask::final_scalar(&secrets.0, &staging::provider_text(secrets, text))
}
#[cfg(test)]
pub(crate) fn final_text(secrets: &SecretSet, text: &str) -> FbResult<String> {
    Ok(fb_db::credential_mask::final_text(&secrets.0, text)?)
}
#[cfg(test)]
pub(super) use fb_db::credential_mask::FINAL_TEXTS;
/// One bank-feeds write transaction and the secrets of the connection(s) it touches.
pub(crate) struct Writes<'a, 'w> {
    /// For reads, and for the other modules' `_tx` services (their payloads go through [`Writes::value`]).
    pub tx: &'a Writer<'w>,
    secrets: &'a SecretSet,
    /// Texts already masked in this transaction (ids and labels repeat on every row).
    seen: RefCell<HashMap<String, String>>,
}
impl<'a, 'w> Writes<'a, 'w> {
    pub fn new(tx: &'a Writer<'w>, secrets: &'a SecretSet) -> Self {
        Self { tx, secrets, seen: RefCell::default() }
    }
    /// A text as it may be written or returned: known secrets masked, the structural rule applied, masked
    /// again.
    pub fn text(&self, value: &str) -> String {
        if let Some(done) = self.seen.borrow().get(value) {
            return done.clone();
        }
        let done = safe_scalar(self.secrets, value);
        self.seen.borrow_mut().insert(value.to_owned(), done.clone());
        done
    }
    /// `value` with every string in it through [`Writes::text`] (keys are the code's own).
    pub fn value(&self, value: &Value) -> Value {
        match value {
            Value::String(s) => Value::String(self.text(s)),
            Value::Array(a) => Value::Array(a.iter().map(|v| self.value(v)).collect()),
            Value::Object(o) => Value::Object(o.iter().map(|(k, v)| (k.clone(), self.value(v))).collect()),
            other => other.clone(),
        }
    }
    /// The only bank-feeds SQL write: every text parameter through [`Writes::text`].
    pub fn execute(&self, sql: &str, params: &[&dyn ToSql]) -> FbResult<usize> {
        let values = params.iter().map(|p| self.param(*p)).collect::<FbResult<Vec<_>>>()?;
        Ok(self.tx.execute(sql, fb_db::rusqlite::params_from_iter(values))?)
    }
    fn param(&self, p: &dyn ToSql) -> FbResult<SqlValue> {
        let value = match p.to_sql()? {
            ToSqlOutput::Borrowed(v) => match v {
                ValueRef::Null => SqlValue::Null,
                ValueRef::Integer(i) => SqlValue::Integer(i),
                ValueRef::Real(r) => SqlValue::Real(r),
                ValueRef::Text(t) => SqlValue::Text(String::from_utf8(t.to_vec()).map_err(|_| FbError::validation("Bank-feed text must be UTF-8."))?),
                ValueRef::Blob(b) => SqlValue::Blob(b.to_vec()),
            },
            ToSqlOutput::Owned(v) => v,
            _ => return Err(FbError::validation("Unsupported bank-feed write parameter.")),
        };
        Ok(match value {
            SqlValue::Text(s) => SqlValue::Text(self.text(&s)),
            other => other,
        })
    }
    /// The only bank-feeds audit writer: target and detail through [`Writes::text`].
    pub fn audit(&self, user: &User, co: CompanyId, action: &str, target: String, detail: Value) -> FbResult<()> {
        services::audit(self.tx, user, co, action, self.text(&target), self.value(&detail))
    }
}
