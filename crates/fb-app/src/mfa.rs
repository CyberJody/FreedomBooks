//! Optional RFC 6238 authenticator-app second factor. A verified password alone is not a
//! session when MFA is enabled. Enrollment and recovery never go through the agent catalogue.
use crate::{App, AppError, AppResult, identity::Permission};
use fb_core::{
    UserId,
    desktop_mfa::{self as storage, Change, Factors, Proof},
};
use ring::hmac;
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use zeroize::Zeroizing;

pub const SETTINGS: &str = "/desktop/mfa/";
const WINDOW: Duration = Duration::from_secs(300);

fn setup_uri(username: &str, secret: &str) -> String {
    // The account label cannot itself contain the issuer separator. This affects
    // only the authenticator's display name, never the user's sign-in identity.
    let label = username.replace(':', " - ");
    let label = percent_encoding::utf8_percent_encode(&label, percent_encoding::NON_ALPHANUMERIC);
    format!("otpauth://totp/FreedomBooks:{label}?secret={secret}&issuer=FreedomBooks&algorithm=SHA1&digits=6&period=30")
}

/// Generated locally from the already-authorized, unexpired enrollment secret.
/// Only fixed SVG markup and numeric module coordinates enter the trusted HTML.
pub(crate) fn setup_qr(username: &str, secret: &str) -> AppResult<String> {
    use std::fmt::Write;
    let uri = Zeroizing::new(setup_uri(username, secret));
    let qr = qrcodegen::QrCode::encode_text(&uri, qrcodegen::QrCodeEcc::Medium)
        .map_err(|_| invalid("Could not create the authenticator QR code."))?;
    let size = qr.size() + 8; // Four light modules on every side for reliable scanning.
    let pixels = size * 5;
    let mut dots = String::new();
    for y in 0..qr.size() {
        for x in 0..qr.size() {
            if qr.get_module(x, y) {
                write!(dots, "M{},{}h1v1h-1z", x + 4, y + 4).expect("write to String");
            }
        }
    }
    Ok(format!(r##"<svg id="mfa_setup_qr" xmlns="http://www.w3.org/2000/svg" role="img" aria-label="Authenticator setup QR code" viewBox="0 0 {size} {size}" width="{pixels}" height="{pixels}" shape-rendering="crispEdges"><rect width="{size}" height="{size}" fill="#fff"/><path d="{dots}" fill="#000"/></svg>"##))
}

pub(crate) struct PendingSignIn {
    pub actor: UserId,
    pub name: String,
    pub password_hash: String,
    pub expires: Instant,
}
struct Enrollment {
    secret: Zeroizing<Vec<u8>>,
    expires: Instant,
}
#[derive(Default)]
pub(crate) struct State {
    pub pending: Option<PendingSignIn>,
    management: Option<(u64, Instant)>,
    enrollment: Option<Enrollment>,
    pub recovery_display: Zeroizing<Vec<String>>,
}
fn invalid(message: &str) -> AppError {
    AppError::Invalid(message.into())
}
fn now() -> AppResult<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|t| t.as_secs())
        .map_err(|_| invalid("Check your computer's clock before using an authenticator."))
}
fn equal(a: &[u8], b: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    a.ct_eq(b).into()
}
fn code_at(secret: &[u8], step: u64) -> u32 {
    let key = hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, secret);
    let mac = hmac::sign(&key, &step.to_be_bytes());
    let bytes = mac.as_ref();
    let offset = (bytes[19] & 15) as usize;
    u32::from_be_bytes(
        bytes[offset..offset + 4]
            .try_into()
            .expect("HMAC-SHA1 truncation"),
    ) & 0x7fff_ffff
}
fn verify_totp(secret: &[u8], previous: i64, code: &str, seconds: u64) -> Option<i64> {
    let code = code.trim();
    if code.len() != 6 || !code.bytes().all(|c| c.is_ascii_digit()) || secret.len() != 20 {
        return None;
    }
    let current = i64::try_from(seconds / 30).ok()?;
    // Modest clock skew; an accepted step is persisted and can never be used again.
    [current, current.checked_sub(1)?, current.checked_add(1)?]
        .into_iter()
        .filter(|step| *step > previous && *step >= 0)
        .find(|step| {
            equal(
                format!("{:06}", code_at(secret, *step as u64) % 1_000_000).as_bytes(),
                code.as_bytes(),
            )
        })
}
fn recovery_hash(actor: UserId, code: &str) -> Option<Vec<u8>> {
    let text = Zeroizing::new(code.trim().replace('-', "").to_ascii_lowercase());
    if text.len() != 32 || !text.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let mut hash = Sha256::new();
    hash.update(b"FreedomBooks MFA recovery v1\0");
    hash.update(actor.to_db());
    hash.update(text.as_bytes());
    Some(hash.finalize().to_vec())
}
#[derive(Default)]
struct RecoverySet {
    plain: Zeroizing<Vec<String>>,
    hashes: Vec<Vec<u8>>,
}
fn recovery_set(actor: UserId) -> AppResult<RecoverySet> {
    let mut plain = Zeroizing::new(Vec::new());
    let mut hashes = Vec::new();
    for _ in 0..10 {
        let bytes = Zeroizing::new(
            crate::keys::random_bytes::<16>()
                .map_err(|_| invalid("Could not create recovery codes."))?,
        );
        let hex = Zeroizing::new(crate::keys::to_hex(&*bytes));
        let code = format!(
            "{}-{}-{}-{}",
            &hex[..8],
            &hex[8..16],
            &hex[16..24],
            &hex[24..]
        );
        hashes.push(recovery_hash(actor, &code).expect("generated hex"));
        plain.push(code);
    }
    Ok(RecoverySet { plain, hashes })
}
fn verify(factors: &Factors, actor: UserId, code: &str, seconds: u64) -> Option<Proof> {
    if code.len() > 80 {
        return None;
    }
    if let Some((secret, previous)) = &factors.totp
        && let Some(step) = verify_totp(secret, *previous, code, seconds)
    {
        return Some(Proof::Totp {
            previous: *previous,
            step,
        });
    }
    if !factors.enabled() {
        return None;
    }
    let hash = recovery_hash(actor, code)?;
    factors
        .recovery
        .iter()
        .any(|saved| equal(saved, &hash))
        .then_some(Proof::Recovery(hash))
}

impl App {
    pub(crate) fn mfa_factors(&mut self, actor: UserId) -> AppResult<Factors> {
        Ok(storage::read(
            &mut self.open.as_mut().ok_or(AppError::Locked)?.book,
            actor,
        )?)
    }
    pub fn mfa_pending(&self) -> bool {
        self.identity
            .mfa
            .pending
            .as_ref()
            .is_some_and(|p| p.expires > Instant::now())
    }
    pub(crate) fn mfa_begin_signin(&mut self, actor: UserId, name: String, password_hash: String) {
        self.identity.mfa.pending = Some(PendingSignIn {
            actor,
            name,
            password_hash,
            expires: Instant::now() + WINDOW,
        });
    }
    /// No accounting authority is granted until verification and single-use consumption succeed.
    pub fn mfa_complete_signin(&mut self, code: &str) -> AppResult<()> {
        self.identity_retry_allowed()?;
        let pending = self
            .identity
            .mfa
            .pending
            .take()
            .ok_or_else(|| invalid("Enter your user name and password again."))?;
        if pending.expires <= Instant::now()
            || !self.mfa_same_password(pending.actor, &pending.password_hash)?
        {
            return Err(invalid(
                "Sign-in expired or the user changed. Enter your password again.",
            ));
        }
        let factors = self.mfa_factors(pending.actor)?;
        let proof = verify(&factors, pending.actor, code, now()?);
        let Some(proof) = proof else {
            self.identity_failure();
            if self.identity_retry_allowed().is_ok() {
                self.identity.mfa.pending = Some(pending);
            }
            return Err(invalid(
                "That code is incorrect or already used. Try the next authenticator code or an unused recovery code.",
            ));
        };
        // Restore masking only after the complete proof. Do not temporarily publish a named actor.
        self.restore_connections_for(pending.actor)?;
        storage::consume(
            &mut self.open.as_mut().ok_or(AppError::Locked)?.book,
            pending.actor,
            proof,
        )?;
        self.finish_sign_in(pending.actor, pending.name)
    }
    fn mfa_same_password(&mut self, actor: UserId, password_hash: &str) -> AppResult<bool> {
        let open = self.open.as_mut().ok_or(AppError::Locked)?;
        Ok(open.book.read(|q|q.query_row("SELECT EXISTS(SELECT 1 FROM desktop_user d JOIN fb_user u ON u.id=d.user_id WHERE d.user_id=?1 AND d.password_hash=?2 AND u.is_active=1)",fb_db::rusqlite::params![actor.to_db(),password_hash],|r|r.get(0)))?)
    }
    pub(crate) fn mfa_management_open(&self) -> bool {
        self.identity
            .mfa
            .management
            .is_some_and(|(generation, expires)| {
                generation == self.session_generation() && expires > Instant::now()
            })
    }
    fn require_mfa_management(&mut self) -> AppResult<UserId> {
        let actor = self.authorize(None, Permission::Read)?.actor;
        if !self.mfa_management_open() {
            return Err(invalid(
                "Confirm your password and current second factor to change MFA.",
            ));
        }
        Ok(actor)
    }
    pub fn mfa_authorize_management(&mut self, password: &str, code: &str) -> AppResult<()> {
        let actor = self.authorize(None, Permission::Read)?.actor;
        self.identity_retry_allowed()?;
        self.identity.mfa.management = None;
        self.identity.mfa.enrollment = None;
        if !self.verify_current_password(actor, password)? {
            self.identity_failure();
            return Err(invalid("The password or second factor is incorrect."));
        }
        let factors = self.mfa_factors(actor)?;
        if factors.enabled() {
            let Some(proof) = verify(&factors, actor, code, now()?) else {
                self.identity_failure();
                return Err(invalid(
                    "The password or second factor is incorrect or already used.",
                ));
            };
            storage::consume(&mut self.open_mut()?.book, actor, proof)?;
        }
        self.identity_success();
        self.identity.mfa.management = Some((self.session_generation(), Instant::now() + WINDOW));
        Ok(())
    }
    pub fn mfa_begin_totp(&mut self) -> AppResult<String> {
        self.require_mfa_management()?;
        let secret = Zeroizing::new(
            crate::keys::random_bytes::<20>()
                .map_err(|_| invalid("Could not create an authenticator secret."))?
                .to_vec(),
        );
        let text = data_encoding::BASE32_NOPAD.encode(&secret);
        self.identity.mfa.enrollment = Some(Enrollment {
            secret,
            expires: Instant::now() + WINDOW,
        });
        Ok(text)
    }
    pub(crate) fn mfa_setup_secret(&self) -> Option<String> {
        if !self.mfa_management_open() {
            return None;
        }
        self.identity
            .mfa
            .enrollment
            .as_ref()
            .filter(|e| e.expires > Instant::now())
            .map(|e| data_encoding::BASE32_NOPAD.encode(&e.secret))
    }
    pub fn mfa_confirm_totp(&mut self, code: &str) -> AppResult<()> {
        let actor = self.require_mfa_management()?;
        self.identity_retry_allowed()?;
        let enrollment = self
            .identity
            .mfa
            .enrollment
            .as_ref()
            .filter(|e| e.expires > Instant::now())
            .ok_or_else(|| invalid("Authenticator setup expired. Start setup again."))?;
        let Some(step) = verify_totp(&enrollment.secret, -1, code, now()?) else {
            self.identity_failure();
            return Err(invalid(
                "The code did not match. Check the setup key and your phone's clock.",
            ));
        };
        let secret = enrollment.secret.to_vec();
        self.mfa_commit_change(actor, Change::Totp { secret, step }, true)
    }
    pub fn mfa_replace_recovery(&mut self) -> AppResult<()> {
        let actor = self.require_mfa_management()?;
        if !self.mfa_factors(actor)?.enabled() {
            return Err(invalid("Enable an authenticator first."));
        }
        self.mfa_commit_change(actor, Change::Recovery, true)
    }
    pub fn mfa_disable(&mut self) -> AppResult<()> {
        let actor = self.require_mfa_management()?;
        self.mfa_commit_change(actor, Change::Disable, false)
    }
    fn mfa_commit_change(
        &mut self,
        actor: UserId,
        change: Change,
        new_codes: bool,
    ) -> AppResult<()> {
        let recovery = if new_codes {
            recovery_set(actor)?
        } else {
            RecoverySet::default()
        };
        let name = self.open_mut()?.owner_name.clone();
        storage::change(
            &mut self.open_mut()?.book,
            actor,
            change,
            new_codes.then_some(recovery.hashes.as_slice()),
        )?;
        // Revoke every AI grant, form and previous management capability while retaining this
        // freshly reauthenticated person's session. Confirmation proved the new factor too.
        self.finish_sign_in(actor, name)?;
        self.identity.mfa.recovery_display = recovery.plain;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn expired_password_challenge_and_management_cannot_restore_authority() {
        let mut app = App::new();
        app.mfa_begin_signin(
            UserId::new(),
            "Synthetic pending".into(),
            "synthetic hash".into(),
        );
        app.identity.mfa.pending.as_mut().unwrap().expires =
            Instant::now() - Duration::from_secs(1);
        assert!(!app.mfa_pending());
        assert!(
            app.mfa_complete_signin("123456")
                .unwrap_err()
                .to_string()
                .contains("expired")
        );
        assert!(app.identity.mfa.pending.is_none());
        assert!(app.authenticated_session().is_err());
        app.identity.mfa.management = Some((
            app.session_generation(),
            Instant::now() - Duration::from_secs(1),
        ));
        app.identity.mfa.enrollment = Some(Enrollment {
            secret: Zeroizing::new(vec![42; 20]),
            expires: Instant::now() + WINDOW,
        });
        assert!(app.mfa_setup_secret().is_none());
        app.identity.mfa.management = Some((
            app.session_generation().wrapping_add(1),
            Instant::now() + WINDOW,
        ));
        assert!(app.mfa_setup_secret().is_none());
    }
    #[test]
    fn qr_uri_preserves_totp_parameters_and_escapes_the_display_label() {
        let secret = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ"; // Public RFC test vector.
        let name = "Avery: <admin> & Renée/?#%";
        let uri = setup_uri(name, secret);
        let parsed = url::Url::parse(&uri).unwrap();
        assert_eq!(parsed.scheme(), "otpauth");
        assert_eq!(parsed.host_str(), Some("totp"));
        assert!(parsed.fragment().is_none());
        assert_eq!(percent_encoding::percent_decode_str(parsed.path()).decode_utf8().unwrap(),
            "/FreedomBooks:Avery -  <admin> & Renée/?#%");
        let parameters: std::collections::BTreeMap<_, _> = parsed.query_pairs().collect();
        assert_eq!(parameters.len(), 5);
        for (key, value) in [("secret", secret), ("issuer", "FreedomBooks"), ("algorithm", "SHA1"), ("digits", "6"), ("period", "30")] {
            assert_eq!(parameters.get(key).map(|s| s.as_ref()), Some(value));
        }
        let svg = setup_qr(name, secret).unwrap();
        assert!(svg.starts_with("<svg id=\"mfa_setup_qr\""));
        for forbidden in [secret, name, "otpauth://", "<script", "href=", "style="] {
            assert!(!svg.contains(forbidden));
        }
        assert!(setup_qr(&"é".repeat(150), secret).is_ok());
    }
    #[test]
    fn rfc6238_vectors_and_replay_window() {
        let secret = b"12345678901234567890";
        for (time, expected) in [
            (59, 94287082),
            (1111111109, 7081804),
            (1111111111, 14050471),
            (1234567890, 89005924),
            (2000000000, 69279037),
            (20000000000, 65353130),
        ] {
            assert_eq!(code_at(secret, time / 30) % 100_000_000, expected);
            let code = format!("{:06}", expected % 1_000_000);
            let step = (time / 30) as i64;
            assert_eq!(verify_totp(secret, step - 1, &code, time), Some(step));
            assert_eq!(verify_totp(secret, step, &code, time), None);
            assert_eq!(verify_totp(secret, -1, &code, time + 120), None);
        }
        for bad in ["", "12345", "1234567", "１２３４５６", "123 456"] {
            assert_eq!(verify_totp(secret, -1, bad, 59), None);
        }
    }
}
