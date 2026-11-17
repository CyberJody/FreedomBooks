//! The remote route policy: what a connected computer may ask the host to do, before and after
//! sign-in, and which outcomes may travel back. Host-only work (keys, protection, backups, files on
//! the host, provider sign-ins whose credentials live on the host) is refused before it reaches the
//! session layer. The session layer's own role and company checks still apply to everything else.
//!
//! Paths are deny-by-default: a screen or form is served only when the backend classifies its
//! route as [`Remote::Allowed`] (fb-host keeps that table and a test fails on any route it does not
//! classify). The fixed list in [`host_only`] is a second layer that also refuses odd spellings.
use crate::backend::Remote;
use crate::wire::RefusalCode;
use serde_json::Value;

pub const SIGN_IN: &str = "/desktop/sign-in/";
pub const SESSION: &str = "/desktop/session/";

pub const HOST_ONLY_TEXT: &str = "This can only be done on the host computer.";
pub const SIGN_IN_TEXT: &str = "Sign in with your own user name and password first.";

pub type Refusal = (RefusalCode, &'static str);

pub fn path_of(target: &str) -> &str {
    target.split_once(['?', '#']).map_or(target, |(p, _)| p)
}

/// `/c/<anything>/rest` → `/rest`, whatever the company segment holds (the router decides whether
/// it is a company; the policy must not depend on that).
fn company_suffix(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("/c/")?;
    rest.find('/').map(|i| &rest[i..])
}

/// Paths that are only ever served to the host computer's own window.
pub fn host_only(target: &str) -> bool {
    let path = path_of(target);
    // Any odd spelling (dot segments, encoded slashes, backslashes, doubled slashes) is refused:
    // the policy must never match a different path from the one the router resolves.
    if !path.starts_with('/') || path.contains("//") || path.contains('\\') || path.contains('%') || path.contains(';') || path.chars().any(char::is_control) || path.split('/').any(|s| s == "." || s == "..") {
        return true;
    }
    const HOST_PATHS: &[&str] = &[
        "/settings/book-protection/",
        "/settings/recovery-key/",
        "/settings/emergency-kit/",
        "/settings/restore-backup/",
        "/settings/restored-book/",
        "/desktop/problem/",
        "/auth/",
    ];
    if HOST_PATHS.iter().any(|p| path.starts_with(p)) {
        return true;
    }
    if let Some(tail) = company_suffix(path) {
        const COMPANY_PREFIXES: &[&str] = &["/backups/", "/ai/", "/bank-feeds/connect/", "/attachments/"];
        if COMPANY_PREFIXES.iter().any(|p| tail.starts_with(p)) {
            return true;
        }
        // Forgetting a bank connection removes host-held credentials.
        if tail.starts_with("/bank-feeds/") && tail.ends_with("/forget/") {
            return true;
        }
    }
    false
}

/// The path decision: refused unless the backend classified the route as allowed remotely.
fn path_allowed(target: &str, class: Option<Remote>) -> Result<(), Refusal> {
    if host_only(target) || class != Some(Remote::Allowed) {
        return Err((RefusalCode::HostOnly, HOST_ONLY_TEXT));
    }
    Ok(())
}

/// A screen request. Before sign-in only the sign-in screen (which also carries MFA) is served.
/// `class` is the backend's classification of the target's path.
pub fn render(target: &str, signed_in: bool, class: Option<Remote>) -> Result<(), Refusal> {
    if target.is_empty() || target.len() > 4096 || !target.starts_with('/') {
        return Err((RefusalCode::Invalid, "Invalid screen address."));
    }
    path_allowed(target, class)?;
    if !signed_in && path_of(target) != SIGN_IN {
        return Err((RefusalCode::SignInRequired, SIGN_IN_TEXT));
    }
    Ok(())
}

/// What a command is, for the dispatcher (throttle and audit).
#[derive(Debug, PartialEq, Eq)]
pub struct CommandInfo {
    pub path: Option<String>,
    pub sign_in: bool,
    /// A read-only export: repeating it changes nothing, so its answer is not kept for retries.
    pub export: bool,
}

/// Form actions refused remotely whatever the route: provider sign-ins and connections (Gmail,
/// Stripe) open a browser on, and keep credentials on, the host; sending or drafting an invoice
/// email hands a PDF to the host's own window. Refused here, before the command runs, so a refused
/// command has no effect in the book.
fn host_action(name: &str, value: &str) -> bool {
    match name {
        "action" => value.starts_with("gmail_") || value.starts_with("stripe_") || value == "email_send" || value == "email_draft",
        // Choosing a bank-rule file opens a file dialog on the host.
        "import_action" => value == "choose",
        _ => false,
    }
}

/// Actions a signed-out session may post to the sign-in screen: sign in, the second factor, and
/// cancelling it. Everything else there waits for sign-in.
const SIGNED_OUT_ACTIONS: &[&str] = &["signin", "mfa", "cancel_mfa"];

/// A command. Only the renderer's command kinds are accepted remotely (as `App::run_renderer`),
/// minus backup export; form posts are checked against the path policy and `classify` (the
/// backend's route classification).
pub fn command(text: &str, signed_in: bool, classify: impl Fn(&str) -> Option<Remote>) -> Result<CommandInfo, Refusal> {
    const INVALID: Refusal = (RefusalCode::Invalid, "This request could not be read. Nothing was saved.");
    if text.len() > 256 * 1024 {
        return Err(INVALID);
    }
    let value: Value = crate::strict::parse(text).map_err(|_| INVALID)?;
    let kind = value.get("type").and_then(Value::as_str).ok_or(INVALID)?;
    match kind {
        "SubmitForm" => {
            let path = value.get("path").and_then(Value::as_str).ok_or(INVALID)?;
            if path.len() > 4096 || path.contains(['?', '#']) {
                return Err(INVALID);
            }
            path_allowed(path, classify(path))?;
            let fields = value.get("fields").and_then(Value::as_array).ok_or(INVALID)?;
            let mut actions = vec![];
            for field in fields {
                let pair = field.as_array().filter(|p| p.len() == 2).ok_or(INVALID)?;
                let (Some(name), Some(v)) = (pair[0].as_str(), pair[1].as_str()) else { return Err(INVALID) };
                if host_action(name, v) {
                    return Err((RefusalCode::HostOnly, HOST_ONLY_TEXT));
                }
                if name == "action" {
                    actions.push(v);
                }
            }
            let sign_in = path == SIGN_IN;
            if !signed_in && !sign_in {
                return Err((RefusalCode::SignInRequired, SIGN_IN_TEXT));
            }
            // Before sign-in, only the sign-in steps themselves reach the app.
            if !signed_in && (actions.is_empty() || !actions.iter().all(|a| SIGNED_OUT_ACTIONS.contains(a))) {
                return Err((RefusalCode::SignInRequired, SIGN_IN_TEXT));
            }
            Ok(CommandInfo { path: Some(path.to_owned()), sign_in, export: false })
        }
        "ExportHistory" | "ExportReport" => {
            if !signed_in {
                return Err((RefusalCode::SignInRequired, SIGN_IN_TEXT));
            }
            Ok(CommandInfo { path: None, sign_in: false, export: true })
        }
        _ => Err((RefusalCode::HostOnly, HOST_ONLY_TEXT)),
    }
}

/// Outcomes that ask the host's own window to do something on the host (a file dialog, the key
/// store, a browser, a printer driver with a host path) never travel to a connected computer.
pub fn outcome(text: &str) -> Result<(), Refusal> {
    let value: Value = crate::strict::parse(text).map_err(|_| (RefusalCode::Refused, "The host's answer could not be read."))?;
    match value.get("outcome").and_then(Value::as_str) {
        Some("external") => return Err((RefusalCode::HostOnly, HOST_ONLY_TEXT)),
        Some("done") => {
            if let Some(data) = value.get("data").and_then(Value::as_object)
                && ["desktop", "desktop_keys", "invoice_email_pdf"].iter().any(|k| data.contains_key(*k))
            {
                return Err((RefusalCode::HostOnly, HOST_ONLY_TEXT));
            }
        }
        Some("redirect") => {
            let location = value.get("location").and_then(Value::as_str).unwrap_or("");
            if !location.starts_with('/') || location.starts_with("//") {
                return Err((RefusalCode::Refused, "The host answered with an invalid address."));
            }
        }
        Some(_) => {}
        None => return Err((RefusalCode::Refused, "The host's answer could not be read.")),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const C: &str = "/c/0190b6f4-6b3e-7c4f-9a0e-1f2d3c4b5a69";

    /// A stand-in for the backend's table: everything under `/c/` and the desktop pages is allowed.
    fn all(path: &str) -> Option<Remote> {
        (path.starts_with("/c/") || path.starts_with("/desktop/")).then_some(Remote::Allowed)
    }

    #[test]
    fn host_only_paths_are_refused_including_odd_spellings() {
        for p in [
            "/settings/book-protection/".to_owned(),
            "/settings/recovery-key/?x=1".into(),
            "/settings/emergency-kit/".into(),
            "/settings/restore-backup/".into(),
            "/settings/restored-book/".into(),
            "/desktop/problem/".into(),
            "/auth/openai/start".into(),
            format!("{C}/backups/"),
            format!("{C}/backups/create/"),
            format!("{C}/ai/"),
            format!("{C}/bank-feeds/connect/"),
            format!("{C}/bank-feeds/0190b6f4-6b3e-7c4f-9a0e-1f2d3c4b5a69/forget/"),
            format!("{C}/attachments/document/x/add/"),
            format!("{C}/home/;x"),
            "/c/../settings/book-protection/".into(),
            "/settings//book-protection/".into(),
            "/settings/%62ook-protection/".into(),
            "relative/".into(),
        ] {
            assert!(host_only(&p), "{p}");
            let class = all(path_of(&p)).or(Some(Remote::Allowed));
            assert_eq!(render(&p, true, class).unwrap_err().0, if p.starts_with('/') { RefusalCode::HostOnly } else { RefusalCode::Invalid }, "{p}");
        }
        for p in [format!("{C}/home/"), format!("{C}/invoices/?state=DRAFT"), format!("{C}/bank-feeds/review/"), "/desktop/users/".into()] {
            assert!(!host_only(&p), "{p}");
            assert!(render(&p, true, all(path_of(&p))).is_ok());
        }
    }

    #[test]
    fn paths_are_deny_by_default() {
        // A route nobody classified (a new screen) is refused, screen and form alike.
        let new_route = format!("{C}/documents/x/files/");
        assert!(!host_only(&new_route), "not in the fixed list");
        assert_eq!(render(&new_route, true, None).unwrap_err().0, RefusalCode::HostOnly);
        assert_eq!(render(&new_route, true, Some(Remote::HostOnly)).unwrap_err().0, RefusalCode::HostOnly);
        let post = format!(r#"{{"type":"SubmitForm","path":"{new_route}","token":"t","fields":[]}}"#);
        assert_eq!(command(&post, true, |_| None).unwrap_err().0, RefusalCode::HostOnly);
        assert_eq!(command(&post, true, |_| Some(Remote::HostOnly)).unwrap_err().0, RefusalCode::HostOnly);
        assert!(command(&post, true, |_| Some(Remote::Allowed)).is_ok());
        // A classification never overrides the fixed host-only list.
        assert_eq!(render("/settings/book-protection/", true, Some(Remote::Allowed)).unwrap_err().0, RefusalCode::HostOnly);
        // A query string in a form path is not a path.
        let query = format!(r#"{{"type":"SubmitForm","path":"{C}/home/?x=1","token":"t","fields":[]}}"#);
        assert_eq!(command(&query, true, all).unwrap_err().0, RefusalCode::Invalid);
    }

    #[test]
    fn before_sign_in_only_the_sign_in_screen_and_its_steps_are_served() {
        assert!(render("/desktop/sign-in/", false, all("/desktop/sign-in/")).is_ok());
        assert!(render("/desktop/sign-in/?next=x", false, all("/desktop/sign-in/")).is_ok());
        assert_eq!(render(&format!("{C}/home/"), false, all("/c/x/")).unwrap_err().0, RefusalCode::SignInRequired);
        for action in ["signin", "mfa", "cancel_mfa"] {
            let post = format!(r#"{{"type":"SubmitForm","path":"/desktop/sign-in/","token":"t","fields":[["action","{action}"],["username","a"],["password","b"]]}}"#);
            assert_eq!(command(&post, false, all).unwrap(), CommandInfo { path: Some(SIGN_IN.into()), sign_in: true, export: false }, "{action}");
        }
        for fields in [r#"[["action","enroll"],["password","b"]]"#, r#"[["action","signin"],["action","enroll"]]"#, r#"[["username","a"]]"#] {
            let post = format!(r#"{{"type":"SubmitForm","path":"/desktop/sign-in/","token":"t","fields":{fields}}}"#);
            assert_eq!(command(&post, false, all).unwrap_err().0, RefusalCode::SignInRequired, "{fields}");
        }
        let post = format!(r#"{{"type":"SubmitForm","path":"{C}/journal/new/","token":"t","fields":[]}}"#);
        assert_eq!(command(&post, false, all).unwrap_err().0, RefusalCode::SignInRequired);
        assert!(command(&post, true, all).is_ok());
        assert_eq!(command(r#"{"type":"ExportHistory","company_id":"x"}"#, false, all).unwrap_err().0, RefusalCode::SignInRequired);
        assert!(command(r#"{"type":"ExportHistory","company_id":"x"}"#, true, all).unwrap().export);
    }

    #[test]
    fn host_commands_and_actions_are_refused_before_they_run() {
        assert_eq!(command(r#"{"type":"ExportBackup"}"#, true, all).unwrap_err().0, RefusalCode::HostOnly);
        assert_eq!(command(r#"{"type":"CreateCompany","name":"x"}"#, true, all).unwrap_err().0, RefusalCode::HostOnly);
        for (name, value) in [("action", "gmail_connect"), ("action", "stripe_link"), ("action", "email_send"), ("action", "email_draft"), ("import_action", "choose")] {
            let post = format!(r#"{{"type":"SubmitForm","path":"{C}/invoices/x/email-preview/","token":"t","fields":[["action","save"],["{name}","{value}"]]}}"#);
            assert_eq!(command(&post, true, all).unwrap_err().0, RefusalCode::HostOnly, "{name}={value}");
        }
        for (name, value) in [("action", "email_resolve"), ("action", "desktop_print"), ("import_action", "confirm")] {
            let post = format!(r#"{{"type":"SubmitForm","path":"{C}/invoices/x/email-preview/","token":"t","fields":[["{name}","{value}"]]}}"#);
            assert!(command(&post, true, all).is_ok(), "{name}={value}");
        }
        let protect = r#"{"type":"SubmitForm","path":"/settings/book-protection/","token":"t","fields":[]}"#;
        assert_eq!(command(protect, true, |_| Some(Remote::Allowed)).unwrap_err().0, RefusalCode::HostOnly);
        let dup = r#"{"type":"SubmitForm","path":"/desktop/sign-in/","path":"/settings/book-protection/","token":"t","fields":[]}"#;
        assert_eq!(command(dup, true, all).unwrap_err().0, RefusalCode::Invalid);
        assert!(outcome(r#"{"outcome":"done","data":{"saved":true}}"#).is_ok());
        assert!(outcome(r#"{"outcome":"redirect","location":"/c/x/home/"}"#).is_ok());
        assert!(outcome(r#"{"outcome":"export","filename":"a.csv","contents":"x"}"#).is_ok());
        for bad in [
            r#"{"outcome":"done","data":{"desktop":"pick-attachment","ticket":"t"}}"#,
            r#"{"outcome":"done","data":{"desktop_keys":"save-recovery"}}"#,
            r#"{"outcome":"done","data":{"invoice_email_pdf":"t"}}"#,
            r#"{"outcome":"external","url":"https://x","location":"/"}"#,
            r#"{"outcome":"redirect","location":"https://evil.example/"}"#,
            r#"{"outcome":"redirect","location":"//evil.example/"}"#,
        ] {
            assert!(outcome(bad).is_err(), "{bad}");
        }
    }
}
