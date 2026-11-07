//! Desktop-only book protection, recovery and portable-restore screens.
use super::*;
use crate::protection::{KIT, RECOVERY, SETTINGS};
pub const RESTORE: &str = "/settings/restore-backup/";

pub(super) fn handles(path: &str) -> bool {
    [SETTINGS, RECOVERY, KIT, RESTORE].contains(&path)
}

pub(super) fn render(app: &mut App, path: &str, errors: &[String]) -> AppResult<Rendered> {
    let c = app.first_company()?;
    let mut vm = serde_json::to_value(shell(
        app,
        c,
        &Current {
            route: "company_edit",
            kind: None,
        },
    )?)
    .map_err(crate::protection::error)?;
    vm["errors"] = json!(errors);
    vm["csrf_token"] = json!(d2::token(app, path));
    vm["builtin"] = json!(app.builtin());
    vm["legacy"] = json!(app.protection.is_none());
    vm["ask_password"] = json!(app.protection.as_ref().is_none_or(|r| r.ask_password));
    vm["os_store"] = json!(app.os_store_usable);
    vm["saved"] = json!(app.protection.as_ref().is_some_and(|r| r.recovery_saved));
    vm["authorized"] = json!(app.recovery_authorized);
    if path == SETTINGS && backups::has_pre_portable_backups(app) {
        vm["older_backups"] = json!(crate::protection::OLDER_BACKUPS);
    }
    let template = match path {
        SETTINGS => "desktop/protection.html",
        RECOVERY | KIT => {
            if app.builtin() {
                return Err(AppError::Invalid(
                    "Built-in-key books have no recovery key.".into(),
                ));
            }
            if app.recovery_authorized {
                vm["recovery_key"] = json!(app.recovery_text()?.as_str());
            }
            vm["date"] = json!((app.today)().to_string());
            vm["kit"] = json!(path == KIT);
            if path == KIT && app.recovery_authorized {
                "desktop/emergency-kit.html"
            } else {
                "desktop/recovery.html"
            }
        }
        RESTORE => {
            if app.backups.restore_pending.is_none() {
                return Err(AppError::Invalid(
                    "Choose a backup to restore first.".into(),
                ));
            }
            "desktop/restore.html"
        }
        _ => return Err(AppError::NotFound(path.into())),
    };
    let html = app.screens.render(template, &vm)?;
    let html = if path == KIT && app.recovery_authorized {
        printing::capability(app, html, path)
    } else {
        html
    };
    Ok(Rendered {
        html,
        status: if errors.is_empty() { 200 } else { 400 },
        route: "book-protection".into(),
    })
}

pub(super) fn submit(app: &mut App, path: &str, fields: Posted<'_>) -> AppResult<Outcome> {
    let action = get(fields, "action").unwrap_or("");
    let current = get(fields, "current_password").unwrap_or("");
    let result = (|| {
        match (path, action) {
            (SETTINGS, "ask") => {
                app.change_ask_password(current, get(fields, "ask_password") == Some("on"))?;
            }
            (SETTINGS, "add") | (SETTINGS, "adopt") => {
                let password = get(fields, "new_password").unwrap_or("");
                if password != get(fields, "repeat_password").unwrap_or("") {
                    return Err(AppError::Invalid("The two passwords differ.".into()));
                }
                if action == "adopt" {
                    app.adopt_legacy_keys(password)?;
                } else {
                    app.change_protection(current, Some(password))?;
                }
                return Ok(Outcome::Redirect {
                    location: RECOVERY.into(),
                });
            }
            (SETTINGS, "remove") => {
                if get(fields, "understand") != Some("on") {
                    return Err(AppError::Invalid(
                        "Confirm that anyone with a copy of the file can open it.".into(),
                    ));
                }
                app.change_protection(current, None)?;
            }
            (RECOVERY, "show") => {
                app.authorize_recovery(current)?;
                return Ok(Outcome::Redirect {
                    location: RECOVERY.into(),
                });
            }
            (RECOVERY, "print") => {
                app.recovery_text()?;
                return Ok(Outcome::Redirect {
                    location: KIT.into(),
                });
            }
            (RECOVERY, "save") => {
                app.recovery_text()?;
                return Ok(Outcome::Done {
                    data: json!({"desktop_keys":"save-recovery"}),
                });
            }
            (RECOVERY, "copy") => {
                app.recovery_text()?;
                return Ok(Outcome::Done {
                    data: json!({"desktop_keys":"copy-recovery", "next_token":app.tokens.mint(path)}),
                });
            }
            (RECOVERY, "later") => {
                app.recovery_authorized = false;
                app.tokens.clear();
            }
            (RESTORE, "restore") => {
                return backups::finish_portable_restore(app, get(fields, "secret").unwrap_or(""));
            }
            (RESTORE, "cancel") => {
                app.backups.restore_pending = None;
            }
            _ => return Err(AppError::Invalid("Unknown protection action.".into())),
        }
        Ok(Outcome::Redirect {
            location: SETTINGS.into(),
        })
    })();
    match result {
        Ok(r) => Ok(r),
        Err(e) => {
            if !app.is_open() { return Err(e); }
            let rendered = render(app, path, &[e.to_string()])?;
            Ok(Outcome::Invalid {
                html: rendered.html,
            })
        }
    }
}
