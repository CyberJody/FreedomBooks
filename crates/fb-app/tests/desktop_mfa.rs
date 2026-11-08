//! Synthetic MFA acceptance. No real profile, authenticator account or provider is contacted.
use fb_app::{
    App, Command, Outcome, Query,
    identity::{Role, SIGN_IN},
    keys::MasterKey,
    mfa::SETTINGS,
    screens,
};
use fb_core::{CompanyId, UserId};
use ring::hmac;
use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
const PASSWORD: &str = "Synthetic MFA individual password";
const OWNER: &str = "Synthetic MFA owner";
struct Fixture {
    _dir: tempfile::TempDir,
    app: App,
    company: CompanyId,
    owner: UserId,
    path: PathBuf,
    key: MasterKey,
}
fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("books/mfa.fbbook");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let key = MasterKey::new([61; 32]);
    let mut app = App::new();
    app.backups.configure(dir.path()).unwrap();
    let company = app
        .create_book(&path, &key, OWNER, "Synthetic MFA company")
        .unwrap()
        .company_id;
    app.enroll_owner(PASSWORD).unwrap();
    let owner = app.authenticated_session().unwrap().actor;
    Fixture {
        _dir: dir,
        app,
        company,
        owner,
        path,
        key,
    }
}
fn code(secret: &str, offset: i64) -> String {
    let bytes = data_encoding::BASE32_NOPAD
        .decode(secret.as_bytes())
        .unwrap();
    let step = (SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        / 30) as i64
        + offset;
    let tag = hmac::sign(
        &hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, &bytes),
        &(step as u64).to_be_bytes(),
    );
    let bytes = tag.as_ref();
    let i = (bytes[19] & 15) as usize;
    format!(
        "{:06}",
        (u32::from_be_bytes(bytes[i..i + 4].try_into().unwrap()) & 0x7fff_ffff) % 1_000_000
    )
}
fn recovery(app: &mut App) -> Vec<String> {
    let page = screens::render(app, SETTINGS).unwrap().html;
    let body = page
        .split("aria-label=\"Recovery codes\"")
        .nth(1)
        .unwrap()
        .split_once('>')
        .unwrap()
        .1
        .split("</textarea>")
        .next()
        .unwrap();
    let codes: Vec<_> = body
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    assert_eq!(codes.len(), 10);
    codes
}
fn enroll(f: &mut Fixture) -> (String, Vec<String>) {
    f.app.mfa_authorize_management(PASSWORD, "").unwrap();
    let secret = f.app.mfa_begin_totp().unwrap();
    // Previous time step lets the next current code be exercised without sleeping.
    f.app.mfa_confirm_totp(&code(&secret, -1)).unwrap();
    let codes = recovery(&mut f.app);
    (secret, codes)
}
fn post(app: &mut App, path: &str, fields: &[(&str, &str)]) -> Outcome {
    let token = app.tokens.mint(path);
    app.run_renderer(Command::SubmitForm {
        path: path.into(),
        token,
        fields: fields
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    })
    .unwrap()
}
#[test]
fn enrollment_is_optional_confirmed_and_password_only_cannot_grant_book_or_ai_access() {
    let mut f = fixture();
    assert!(
        screens::render(&mut f.app, SETTINGS)
            .unwrap()
            .html
            .contains("Not enabled")
    );
    assert!(f.app.mfa_begin_totp().is_err());
    assert!(f.app.mfa_authorize_management("wrong", "").is_err());
    f.app.mfa_authorize_management(PASSWORD, "").unwrap();
    let secret = f.app.mfa_begin_totp().unwrap();
    assert!(f.app.mfa_confirm_totp("not a code").is_err());
    f.app.sign_out();
    f.app.sign_in(OWNER, PASSWORD).unwrap();
    assert!(
        f.app.authenticated_session().is_ok(),
        "unconfirmed setup must not enable MFA"
    );
    assert!(
        f.app.mfa_confirm_totp(&code(&secret, 0)).is_err(),
        "old enrollment must expire at sign-out"
    );
    let (secret, _) = enroll(&mut f);
    f.app.sign_out();
    f.app.sign_in(OWNER, PASSWORD).unwrap();
    assert!(f.app.mfa_pending());
    assert!(f.app.authenticated_session().is_err());
    assert!(f.app.open_mut().is_err());
    assert!(f.app.query(Query::Companies).is_err());
    assert!(
        f.app
            .ai_grant(f.company, "forged", fb_app::ai::Permission::ReadOnly)
            .is_err()
    );
    assert_eq!(
        screens::render(&mut f.app, &format!("/c/{}/home/", f.company.to_db()))
            .unwrap()
            .html,
        SIGN_IN
    );
    assert!(
        screens::render(&mut f.app, SIGN_IN)
            .unwrap()
            .html
            .contains("Verify your sign-in")
    );
    assert!(f.app.mfa_complete_signin("invalid").is_err());
    f.app.mfa_complete_signin(&code(&secret, 0)).unwrap();
    assert_eq!(f.app.authenticated_session().unwrap().actor, f.owner);
    assert!(f.app.query(Query::Companies).is_ok());
    f.app
        .ai_grant(
            f.company,
            "allowed after MFA",
            fb_app::ai::Permission::ReadOnly,
        )
        .unwrap();
}
#[test]
fn codes_are_single_use_across_reopen_and_recovery_is_hashed_and_audited_without_secrets() {
    let mut f = fixture();
    let (secret, codes) = enroll(&mut f);
    let hashes = fb_core::desktop_mfa::read(&mut f.app.open_mut().unwrap().book, f.owner)
        .unwrap()
        .recovery
        .clone();
    assert_eq!(hashes.len(), 10);
    assert!(hashes.iter().all(|h| h.len() == 32));
    f.app.sign_out();
    f.app.sign_in(OWNER, PASSWORD).unwrap();
    let otp = code(&secret, 0);
    f.app.mfa_complete_signin(&otp).unwrap();
    f.app.lock();
    f.app.open_book(&f.path, &f.key).unwrap();
    f.app.sign_in(OWNER, PASSWORD).unwrap();
    assert!(f.app.mfa_complete_signin(&otp).is_err());
    f.app.mfa_complete_signin(&codes[0]).unwrap();
    f.app.lock();
    f.app.open_book(&f.path, &f.key).unwrap();
    f.app.sign_in(OWNER, PASSWORD).unwrap();
    assert!(f.app.mfa_complete_signin(&codes[0]).is_err());
    f.app.mfa_complete_signin(&codes[1]).unwrap();
    let stored: String = f
        .app
        .open_mut()
        .unwrap()
        .book
        .read(|q| {
            q.query_row(
                "SELECT group_concat(detail) FROM ledger_auditevent",
                [],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert!(!stored.contains(&secret));
    for c in &codes {
        assert!(!stored.contains(c));
    }
    assert!(stored.contains("authenticator") && stored.contains("recovery_code"));
}
#[test]
fn management_needs_current_proof_rotates_grants_and_preserves_mfa_after_password_reset() {
    let mut f = fixture();
    let (_secret, codes) = enroll(&mut f);
    let grant = f
        .app
        .ai_grant(
            f.company,
            "old grant",
            fb_app::ai::Permission::AccountingWrite,
        )
        .unwrap();
    let token = grant["token"].as_str().unwrap().to_owned();
    let generation = f.app.session_generation();
    assert!(f.app.mfa_disable().is_err());
    assert!(f.app.mfa_authorize_management(PASSWORD, "").is_err());
    f.app.mfa_authorize_management(PASSWORD, &codes[0]).unwrap();
    f.app.mfa_replace_recovery().unwrap();
    assert!(f.app.session_generation() > generation);
    assert!(f.app.ai_catalogue(&token).is_err());
    let replacement = recovery(&mut f.app);
    assert_ne!(codes, replacement);
    f.app
        .save_desktop_user(
            Some(f.owner),
            OWNER,
            Role::Administrator,
            true,
            Some("Synthetic replacement password"),
            &[f.company],
        )
        .unwrap();
    f.app
        .sign_in(OWNER, "Synthetic replacement password")
        .unwrap();
    assert!(f.app.mfa_pending());
    assert!(f.app.mfa_complete_signin(&codes[1]).is_err());
    f.app.mfa_complete_signin(&replacement[0]).unwrap();
    f.app
        .mfa_authorize_management("Synthetic replacement password", &replacement[1])
        .unwrap();
    f.app.mfa_disable().unwrap();
    assert!(
        fb_core::desktop_mfa::read(&mut f.app.open_mut().unwrap().book, f.owner)
            .unwrap()
            .recovery
            .is_empty()
    );
    f.app.sign_out();
    f.app
        .sign_in(OWNER, "Synthetic replacement password")
        .unwrap();
    assert!(f.app.authenticated_session().is_ok());
}
#[test]
fn pending_challenges_are_cleared_by_lock_cancel_and_wrong_password_and_attempts_are_limited() {
    let mut f = fixture();
    let (_, codes) = enroll(&mut f);
    f.app.sign_out();
    assert!(f.app.mfa_complete_signin(&codes[0]).is_err());
    f.app.sign_in(OWNER, PASSWORD).unwrap();
    let outcome = post(&mut f.app, SIGN_IN, &[("action", "cancel_mfa")]);
    assert!(matches!(outcome, Outcome::Redirect { .. }));
    assert!(f.app.mfa_complete_signin(&codes[0]).is_err());
    f.app.sign_in(OWNER, PASSWORD).unwrap();
    f.app.lock();
    f.app.open_book(&f.path, &f.key).unwrap();
    assert!(f.app.mfa_complete_signin(&codes[0]).is_err());
    f.app.sign_in(OWNER, PASSWORD).unwrap();
    assert!(f.app.sign_in(OWNER, "wrong password").is_err());
    assert!(!f.app.mfa_pending());
    assert!(f.app.mfa_complete_signin(&codes[0]).is_err());
    for _ in 0..4 {
        f.app.sign_in(OWNER, PASSWORD).unwrap();
        assert!(f.app.mfa_complete_signin("bad").is_err());
    }
    assert!(f.app.sign_in(OWNER, PASSWORD).is_err());
    assert!(f.app.mfa_complete_signin(&codes[0]).is_err());
}
#[test]
fn renderer_forms_complete_enrollment_and_readonly_users_can_secure_their_own_signin() {
    let mut f = fixture();
    let other = f
        .app
        .save_desktop_user(
            None,
            "Synthetic viewer",
            Role::Readonly,
            true,
            Some(PASSWORD),
            &[f.company],
        )
        .unwrap();
    f.app.sign_in("Synthetic viewer", PASSWORD).unwrap();
    assert!(matches!(
        post(
            &mut f.app,
            SETTINGS,
            &[("action", "authorize"), ("password", PASSWORD)]
        ),
        Outcome::Redirect { .. }
    ));
    assert!(matches!(
        post(&mut f.app, SETTINGS, &[("action", "begin")]),
        Outcome::Redirect { .. }
    ));
    let page = screens::render(&mut f.app, SETTINGS).unwrap().html;
    let secret = page
        .split("id=\"setup_key\" value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    let otp = code(secret, -1);
    assert!(matches!(
        post(
            &mut f.app,
            SETTINGS,
            &[("action", "confirm"), ("code", &otp)]
        ),
        Outcome::Redirect { .. }
    ));
    let codes = recovery(&mut f.app);
    assert!(matches!(
        post(&mut f.app, SETTINGS, &[("action", "saved")]),
        Outcome::Redirect { .. }
    ));
    assert!(
        !screens::render(&mut f.app, SETTINGS)
            .unwrap()
            .html
            .contains(&codes[0])
    );
    f.app.sign_out();
    assert!(matches!(
        post(
            &mut f.app,
            SIGN_IN,
            &[
                ("action", "signin"),
                ("username", "Synthetic viewer"),
                ("password", PASSWORD)
            ]
        ),
        Outcome::Redirect { .. }
    ));
    assert!(f.app.authenticated_session().is_err());
    assert!(matches!(
        post(
            &mut f.app,
            SIGN_IN,
            &[("action", "mfa"), ("code", &codes[0])]
        ),
        Outcome::Redirect { .. }
    ));
    assert_eq!(f.app.authenticated_session().unwrap().actor, other);
    assert!(
        f.app
            .ai_grant(
                f.company,
                "not elevated",
                fb_app::ai::Permission::AccountingWrite
            )
            .is_err()
    );
}

#[test]
fn recovery_codes_are_bound_to_the_person_and_agents_cannot_manage_mfa() {
    let mut f = fixture();
    f.app
        .save_desktop_user(
            None,
            "Other MFA user",
            Role::Bookkeeper,
            true,
            Some(PASSWORD),
            &[f.company],
        )
        .unwrap();
    f.app.sign_in(OWNER, PASSWORD).unwrap();
    let (_, owner_codes) = enroll(&mut f);
    f.app.sign_out();
    f.app.sign_in("Other MFA user", PASSWORD).unwrap();
    let (_, other_codes) = enroll(&mut f);
    f.app.sign_out();
    f.app.sign_in(OWNER, PASSWORD).unwrap();
    assert!(f.app.mfa_complete_signin(&other_codes[0]).is_err());
    f.app.mfa_complete_signin(&owner_codes[0]).unwrap();
    let grant = f
        .app
        .ai_grant(
            f.company,
            "Synthetic agent",
            fb_app::ai::Permission::AccountingWrite,
        )
        .unwrap();
    let token = grant["token"].as_str().unwrap();
    let form = f.app.tokens.mint(SETTINGS);
    assert!(f.app.ai_call(token,&serde_json::json!({"operation":"command_SubmitForm","arguments":{"path":SETTINGS,"token":form,"fields":[["action","disable"],["confirm_disable","on"]]}}).to_string()).is_err());
    assert!(
        fb_core::desktop_mfa::read(&mut f.app.open_mut().unwrap().book, f.owner)
            .unwrap()
            .enabled()
    );
}

#[test]
fn enrollment_qr_requires_fresh_proof_and_clears_with_setup() {
    let mut f = fixture();
    let page = screens::render(&mut f.app, SETTINGS).unwrap().html;
    assert!(!page.contains("id=\"mfa_setup_qr\""));
    assert!(f.app.mfa_authorize_management("wrong", "").is_err());
    assert!(!screens::render(&mut f.app, SETTINGS).unwrap().html.contains("id=\"mfa_setup_qr\""));
    f.app.mfa_authorize_management(PASSWORD, "").unwrap();
    let secret = f.app.mfa_begin_totp().unwrap();
    let page = screens::render(&mut f.app, SETTINGS).unwrap().html;
    assert!(page.contains("id=\"mfa_setup_qr\""), "enrollment should offer a scannable QR code");
    assert!(page.contains("<details") && page.contains("id=\"setup_key\""), "keep a manual fallback");
    assert!(!page.contains("otpauth://"), "enrollment URI is encoded as local pixels, never a link");
    f.app.sign_out();
    f.app.sign_in(OWNER, PASSWORD).unwrap();
    let page = screens::render(&mut f.app, SETTINGS).unwrap().html;
    assert!(!page.contains("id=\"mfa_setup_qr\""));
    assert!(!page.contains(&secret));
    assert!(f.app.mfa_confirm_totp(&code(&secret, 0)).is_err());
    f.app.mfa_authorize_management(PASSWORD, "").unwrap();
    let next = f.app.mfa_begin_totp().unwrap();
    f.app.mfa_confirm_totp(&code(&next, -1)).unwrap();
    let page = screens::render(&mut f.app, SETTINGS).unwrap().html;
    assert!(!page.contains("id=\"mfa_setup_qr\""));
    assert!(!page.contains(&next));
}
