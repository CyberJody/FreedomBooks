//! Storage for verified desktop second factors. Callers authenticate the person before mutating;
//! all changes use the guarded atomic Writer and append actor audit without factor secrets.
use crate::{Book, CompanyId, FbError, FbResult, UserId, services};
use fb_db::rusqlite::{OptionalExtension, params};
use serde_json::json;
use zeroize::{Zeroize, Zeroizing};

pub struct Factors {
    pub totp: Option<(Vec<u8>, i64)>,
    pub recovery: Vec<Vec<u8>>,
}
impl Drop for Factors {
    fn drop(&mut self) {
        if let Some((secret, _)) = &mut self.totp {
            secret.zeroize();
        }
    }
}
impl Factors {
    pub fn enabled(&self) -> bool {
        self.totp.is_some()
    }
}
pub fn read(book: &mut Book, user: UserId) -> FbResult<Factors> {
    book.read(|q| {
        let totp = q
            .query_row(
                "SELECT secret,last_step FROM desktop_mfa_totp WHERE user_id=?1",
                [user.to_db()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let recovery = q
            .prepare("SELECT hash FROM desktop_mfa_recovery WHERE user_id=?1 ORDER BY hash")?
            .query_map([user.to_db()], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        Ok(Factors { totp, recovery })
    })
}
pub enum Proof {
    Totp { previous: i64, step: i64 },
    Recovery(Vec<u8>),
}
pub enum Change {
    Totp { secret: Vec<u8>, step: i64 },
    Recovery,
    Disable,
}
fn audit(
    tx: &fb_db::Writer<'_>,
    user: UserId,
    action: &str,
    detail: serde_json::Value,
) -> FbResult<()> {
    let actor = services::authenticated(tx, user)?;
    let companies: Vec<String> = tx
        .prepare("SELECT company_id FROM ledger_company_members WHERE user_id=?1")?
        .query_map([user.to_db()], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    for co in companies {
        services::audit(
            tx,
            &actor,
            CompanyId::parse(&co).ok_or_else(|| FbError::validation("Invalid company."))?,
            action,
            format!("user:{}", user.to_db()),
            detail.clone(),
        )?;
    }
    Ok(())
}
pub fn consume(book: &mut Book, user: UserId, proof: Proof) -> FbResult<()> {
    book.write(|tx| {
        services::authenticated(tx,user)?;
        let (count,method)=match proof {
            Proof::Totp{previous,step} => {
                if step<=previous { return Err(FbError::validation("This authenticator code was already used.")); }
                (tx.execute("UPDATE desktop_mfa_totp SET last_step=?2 WHERE user_id=?1 AND last_step=?3",params![user.to_db(),step,previous])?,"authenticator")
            },
            Proof::Recovery(hash) => (tx.execute("DELETE FROM desktop_mfa_recovery WHERE user_id=?1 AND hash=?2",params![user.to_db(),hash])?,"recovery_code"),
        };
        if count!=1 { return Err(FbError::validation("Second factor changed or was already used. Sign in again.")); }
        audit(tx,user,"user.mfa_verified",json!({"method":method}))
    })
}
/// An optional new recovery set replaces the old set in the same commit as enrollment.
pub fn change(
    book: &mut Book,
    user: UserId,
    change: Change,
    recovery: Option<&[Vec<u8>]>,
) -> FbResult<()> {
    book.write(|tx| {
        services::authenticated(tx,user)?;
        let method=match change {
            Change::Totp{secret,step} => { let secret=Zeroizing::new(secret); tx.execute("INSERT INTO desktop_mfa_totp(user_id,secret,last_step) VALUES(?1,?2,?3) ON CONFLICT(user_id) DO UPDATE SET secret=excluded.secret,last_step=excluded.last_step",params![user.to_db(),secret.as_slice(),step])?; "authenticator_enabled" },
            Change::Recovery => "recovery_codes_replaced",
            Change::Disable => { tx.execute("DELETE FROM desktop_mfa_totp WHERE user_id=?1",[user.to_db()])?; "disabled" },
        };
        let enabled:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM desktop_mfa_totp WHERE user_id=?1)",[user.to_db()],|r|r.get(0))?;
        if recovery.is_some() || !enabled { tx.execute("DELETE FROM desktop_mfa_recovery WHERE user_id=?1",[user.to_db()])?; }
        if enabled && let Some(hashes)=recovery {
            if hashes.len()!=10 || hashes.iter().any(|h|h.len()!=32) { return Err(FbError::validation("Invalid recovery set.")); }
            for hash in hashes {tx.execute("INSERT INTO desktop_mfa_recovery(user_id,hash) VALUES(?1,?2)",params![user.to_db(),hash])?;}
        }
        audit(tx,user,"user.mfa_changed",json!({"change":method,"enabled":enabled,"recovery_codes_replaced":recovery.is_some()}))
    })
}
