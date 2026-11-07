//! fb-ids: UUIDv7 row ids, one Rust type per entity.
//!
//! Every row id is a UUIDv7 from day one (Jody 10/30). Ids are stored as lowercase hyphenated TEXT,
//! whose byte order equals the UUID's numeric order, so `ORDER BY id` is creation order: the
//! generator below is monotonic within a process (a later id always sorts after an earlier one),
//! which is what the Python app's serial primary keys gave `order_by("pk")` / `order_by("-pk")`.

use std::fmt;
use std::str::FromStr;
use std::sync::Mutex;
use uuid::Uuid;

static LAST: Mutex<u128> = Mutex::new(0);

/// A new UUIDv7, strictly greater than every id this process made before.
pub fn new_v7() -> Uuid {
    let fresh = Uuid::now_v7().as_u128();
    let mut last = LAST.lock().unwrap_or_else(|p| p.into_inner());
    let next = if fresh > *last { fresh } else { *last + 1 };
    *last = next;
    Uuid::from_u128(next)
}

/// Parse a caller-supplied id: any form the `uuid` crate reads (hyphenated, simple, braced, urn),
/// surrounding ASCII whitespace ignored. Returns the canonical value.
pub fn parse_uuid(text: &str) -> Option<Uuid> {
    Uuid::parse_str(text.trim_matches(|c: char| c.is_ascii_whitespace())).ok()
}

macro_rules! id_type {
    ($name:ident, $label:literal) => {
        #[doc = concat!("Row id of one ", $label, " (UUIDv7).")]
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub Uuid);

        impl $name {
            pub fn new() -> Self {
                $name(new_v7())
            }
            pub fn as_uuid(&self) -> Uuid {
                self.0
            }
            /// The stored TEXT form.
            pub fn to_db(&self) -> String {
                self.0.hyphenated().to_string()
            }
            pub fn parse(text: &str) -> Option<Self> {
                parse_uuid(text).map($name)
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0.hyphenated())
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(s).map($name)
            }
        }
    };
}

id_type!(UserId, "operator (user)");
id_type!(CompanyId, "company");
id_type!(AccountId, "account");
id_type!(JournalId, "journal");
id_type!(JournalLineId, "journal line");
id_type!(AuditEventId, "audit event");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_v7_and_sort_in_creation_order_as_text_too() {
        let ids: Vec<JournalId> = (0..5000).map(|_| JournalId::new()).collect();
        for pair in ids.windows(2) {
            assert!(pair[0] < pair[1]);
            assert!(pair[0].to_db() < pair[1].to_db(), "text order must equal numeric order");
        }
        assert_eq!(ids[0].as_uuid().get_version_num(), 7);
    }

    #[test]
    fn parse_accepts_canonical_forms_and_rejects_garbage() {
        let id = AccountId::new();
        assert_eq!(AccountId::parse(&format!("  {id} ")), Some(id));
        assert_eq!(AccountId::parse(&id.as_uuid().simple().to_string()), Some(id));
        assert_eq!(AccountId::parse("12"), None);
        assert_eq!(AccountId::parse(""), None);
    }
}
