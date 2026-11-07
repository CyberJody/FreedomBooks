//! Raw-SQL manifest: every table, exact schema fingerprints and per-company
//! posted/draft counts and trial balances. No posting services are used here.
use super::{BackupError, Result};
use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use fb_db::{Book, Query};
use rand::{RngCore, rngs::OsRng};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use zeroize::Zeroizing;

pub fn new_nonce() -> String {
    let mut nonce = [0; 12];
    OsRng.fill_bytes(&mut nonce);
    STANDARD.encode(nonce)
}
fn cipher(key: &fb_db::Key) -> Result<Aes256Gcm> {
    let mut subkey = Zeroizing::new([0; 32]);
    hkdf::Hkdf::<Sha256>::new(None, key)
        .expand(b"FreedomBooks backup manifest v3", subkey.as_mut())
        .map_err(|_| BackupError::refused("Manifest key derivation failed."))?;
    Aes256Gcm::new_from_slice(subkey.as_ref())
        .map_err(|_| BackupError::refused("Invalid manifest key."))
}
fn nonce(header: &serde_json::Value) -> Result<Vec<u8>> {
    STANDARD
        .decode(
            header["manifest_nonce"]
                .as_str()
                .ok_or_else(|| BackupError::refused("Missing manifest nonce."))?,
        )
        .ok()
        .filter(|n| n.len() == 12)
        .ok_or_else(|| BackupError::refused("Invalid manifest nonce."))
}
/// Local-backup metadata encryption. Uses a domain-separated master subkey and
/// authenticates the exact public header, with a fresh random 96-bit nonce.
pub fn encrypt(
    plain: &[u8],
    key: &fb_db::Key,
    header: &serde_json::Value,
    header_bytes: &[u8],
) -> Result<Vec<u8>> {
    let nonce = nonce(header)?;
    cipher(key)?
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: plain,
                aad: header_bytes,
            },
        )
        .map_err(|_| BackupError::refused("Manifest encryption failed."))
}
pub fn decrypt(
    encrypted: &[u8],
    key: &fb_db::Key,
    header: &serde_json::Value,
    header_bytes: &[u8],
) -> Result<Zeroizing<Vec<u8>>> {
    let nonce = nonce(header)?;
    cipher(key)?
        .decrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: encrypted,
                aad: header_bytes,
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| BackupError::refused("Wrong master key or changed backup manifest/header."))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Contents {
    pub tables: BTreeMap<String, i64>,
    pub schema: BTreeMap<String, String>,
    pub companies: Vec<Company>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Company {
    pub id: String,
    pub name: String,
    pub currency: String,
    pub journals: BTreeMap<String, i64>,
    pub trial_balance: BTreeMap<String, String>,
    pub accounts: Vec<Account>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Account {
    pub id: String,
    pub code: String,
    pub name: String,
    pub kind: String,
    pub posted_debits: String,
    pub posted_credits: String,
    pub debit_balance: String,
    pub credit_balance: String,
}

fn money(cents: i128) -> String {
    format!(
        "{}{}.{:02}",
        if cents < 0 { "-" } else { "" },
        cents.abs() / 100,
        cents.abs() % 100
    )
}
fn add(a: i128, b: i128) -> Result<i128> {
    a.checked_add(b)
        .ok_or_else(|| BackupError::refused("Manifest total overflow."))
}

pub fn contents(q: &(impl Query + ?Sized)) -> Result<Contents> {
    let tables = q.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT GLOB 'sqlite_*' ORDER BY name")?
        .query_map([], |r| r.get::<_, String>(0))?.collect::<std::result::Result<Vec<_>, _>>()?;
    let mut counts = BTreeMap::new();
    for name in tables {
        let quoted = name.replace('"', "\"\"");
        counts.insert(
            name,
            q.query_row(&format!("SELECT count(*) FROM \"{quoted}\""), [], |r| {
                r.get(0)
            })?,
        );
    }
    let schema = q.prepare("SELECT type,name,sql FROM sqlite_master WHERE name NOT GLOB 'sqlite_*' AND sql IS NOT NULL ORDER BY type,name")?
        .query_map([], |r| Ok((format!("{}:{}", r.get::<_, String>(0)?, r.get::<_, String>(1)?), format!("{:x}", Sha256::digest(r.get::<_, String>(2)?.as_bytes())))))?
        .collect::<std::result::Result<BTreeMap<_, _>, _>>()?;
    let mut companies = Vec::new();
    let mut cs = q.prepare("SELECT id,name,currency FROM ledger_company ORDER BY id")?;
    for row in cs.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })? {
        let (id, name, currency) = row?;
        let mut journals = BTreeMap::from([("posted".into(), 0), ("draft".into(), 0)]);
        for (key, status) in [("posted", "POSTED"), ("draft", "DRAFT")] {
            journals.insert(
                key.into(),
                q.query_row(
                    "SELECT count(*) FROM ledger_journal WHERE company_id=?1 AND status=?2",
                    [&id, status],
                    |r| r.get(0),
                )?,
            );
        }
        let mut accounts = Vec::new();
        let mut td = 0i128;
        let mut tc = 0i128;
        let mut ac = q.prepare("SELECT id,code,name,kind FROM ledger_account WHERE company_id=?1 ORDER BY code COLLATE fb_text,id")?;
        for row in ac.query_map([&id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })? {
            let (aid, code, name, kind) = row?;
            let mut d = 0i128;
            let mut c = 0i128;
            let mut lines = q.prepare("SELECT l.debit,l.credit FROM ledger_journalline l JOIN ledger_journal j ON j.id=l.journal_id JOIN ledger_account a ON a.id=l.account_id WHERE l.account_id=?1 AND j.status='POSTED' AND j.company_id=a.company_id")?;
            for pair in
                lines.query_map([&aid], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?
            {
                let (debit, credit) = pair?;
                d = add(d, i128::from(debit))?;
                c = add(c, i128::from(credit))?;
            }
            let net = d
                .checked_sub(c)
                .ok_or_else(|| BackupError::refused("Manifest total overflow."))?;
            let db = net.max(0);
            let cb = (-net).max(0);
            td = add(td, db)?;
            tc = add(tc, cb)?;
            accounts.push(Account {
                id: aid,
                code,
                name,
                kind,
                posted_debits: money(d),
                posted_credits: money(c),
                debit_balance: money(db),
                credit_balance: money(cb),
            });
        }
        companies.push(Company {
            id,
            name,
            currency,
            journals,
            trial_balance: BTreeMap::from([
                ("debit".into(), money(td)),
                ("credit".into(), money(tc)),
            ]),
            accounts,
        });
    }
    Ok(Contents {
        tables: counts,
        schema,
        companies,
    })
}

pub fn from_book(book: &mut Book) -> Result<Contents> {
    book.read(|q| contents(q))
}

/// Human-readable recursive differences, with the Python default cap of 20.
pub fn differences(
    expected: &serde_json::Value,
    actual: &serde_json::Value,
    limit: usize,
) -> Vec<String> {
    fn walk(
        a: &serde_json::Value,
        b: &serde_json::Value,
        path: &str,
        limit: usize,
        out: &mut Vec<String>,
    ) {
        if out.len() >= limit {
            return;
        }
        match (a, b) {
            (serde_json::Value::Object(a), serde_json::Value::Object(b)) => {
                let keys: std::collections::BTreeSet<_> = a.keys().chain(b.keys()).collect();
                for key in keys {
                    if out.len() >= limit {
                        break;
                    }
                    match (a.get(key), b.get(key)) {
                        (None, _) => out.push(format!("{path}.{key}: not in the backup manifest")),
                        (_, None) => {
                            out.push(format!("{path}.{key}: missing from the restored database"))
                        }
                        (Some(x), Some(y)) => walk(x, y, &format!("{path}.{key}"), limit, out),
                    }
                }
            }
            (serde_json::Value::Array(a), serde_json::Value::Array(b)) => {
                if a.len() != b.len() {
                    out.push(format!(
                        "{path}: {} entries in the manifest, {} restored",
                        a.len(),
                        b.len()
                    ));
                }
                for (i, (x, y)) in a.iter().zip(b).enumerate() {
                    walk(x, y, &format!("{path}[{i}]"), limit, out);
                }
            }
            _ if a != b => out.push(format!("{path}: manifest {a}, restored {b}")),
            _ => (),
        }
    }
    let mut out = Vec::new();
    walk(expected, actual, "contents", limit, &mut out);
    out
}
