//! Errors of the port. The Python services raise three kinds of exception and the HTTP layer maps them;
//! here they are one enum so the harness can compare error paths:
//! `ValidationError` -> `Validation` (its `messages` list, in Django's order), `PermissionDenied`,
//! `Model.DoesNotExist` -> `DoesNotExist`, `IntegrityError` -> `Integrity` (a database guard refused).

use std::fmt;

#[derive(Debug)]
pub enum FbError {
    Validation(Vec<String>),
    Refused(String),
    PermissionDenied(String),
    DoesNotExist(String),
    /// A database guard, CHECK, UNIQUE or foreign key refused the write (Python `IntegrityError`).
    Integrity {
        constraint: Option<String>,
        message: String,
    },
    Db(fb_db::rusqlite::Error),
}

impl FbError {
    pub fn refused(message: impl Into<String>) -> FbError {
        FbError::Refused(message.into())
    }
    pub fn validation(message: impl Into<String>) -> FbError {
        FbError::Validation(vec![message.into()])
    }

    pub fn denied(message: impl Into<String>) -> FbError {
        FbError::PermissionDenied(message.into())
    }

    pub fn missing(model: &str) -> FbError {
        FbError::DoesNotExist(format!("{model} matching query does not exist."))
    }

    /// Python's exception class name, as the harness records it.
    pub fn kind(&self) -> &'static str {
        match self {
            FbError::Validation(_) => "ValidationError",
            FbError::Refused(_) => "Refused",
            FbError::PermissionDenied(_) => "PermissionDenied",
            FbError::DoesNotExist(_) => "DoesNotExist",
            FbError::Integrity { .. } => "IntegrityError",
            FbError::Db(_) => "DatabaseError",
        }
    }

    /// `"; ".join(exc.messages)` for a ValidationError; the message otherwise.
    pub fn message(&self) -> String {
        match self {
            FbError::Validation(messages) => messages.join("; "),
            FbError::Refused(m) | FbError::PermissionDenied(m) | FbError::DoesNotExist(m) => {
                m.clone()
            }
            FbError::Integrity { message, .. } => message.clone(),
            FbError::Db(e) => e.to_string(),
        }
    }

    pub fn messages(&self) -> Vec<String> {
        match self {
            FbError::Validation(messages) => messages.clone(),
            other => vec![other.message()],
        }
    }

    pub fn constraint(&self) -> Option<&str> {
        match self {
            FbError::Integrity { constraint, .. } => constraint.as_deref(),
            _ => None,
        }
    }
}

impl fmt::Display for FbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind(), self.message())
    }
}

impl std::error::Error for FbError {}

impl From<fb_db::rusqlite::Error> for FbError {
    fn from(e: fb_db::rusqlite::Error) -> Self {
        if fb_db::is_constraint(&e) {
            FbError::Integrity {
                constraint: fb_db::constraint_of(&e),
                message: fb_db::guard_message(&e).unwrap_or_else(|| e.to_string()),
            }
        } else {
            FbError::Db(e)
        }
    }
}

pub type FbResult<T> = Result<T, FbError>;

impl From<fb_db::credential_mask::MaskError> for FbError {
    fn from(e: fb_db::credential_mask::MaskError) -> Self { Self::validation(e.message()) }
}
