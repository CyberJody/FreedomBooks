//! Same-desktop user enrollment, sign-in, sign-out and administrator roster.
use super::*;
use crate::identity::{Permission, Role, SESSION, SIGN_IN, USERS};
use crate::mfa::SETTINGS as MFA;

pub(super) fn handles(path: &str) -> bool {
    [SIGN_IN, SESSION, USERS, MFA].contains(&path)
}
fn landing(app: &mut App) -> AppResult<Outcome> {
    if app.mfa_pending() { return Ok(Outcome::Redirect{location:SIGN_IN.into()}); }
    let administrator = app.authenticated_session()?.role == Role::Administrator;
    let location = if administrator && app.protection.is_none() {
        crate::protection::SETTINGS.to_owned()
    } else if administrator
        && app.protection.as_ref().is_some_and(|r| {
            r.keys.mode == fb_core::backups::portable::Mode::OwnPassword && !r.recovery_saved
        })
    {
        crate::protection::RECOVERY.to_owned()
    } else {
        app.first_company()?
            .map(|c| format!("/c/{}/home/", c.to_db()))
            .unwrap_or_else(|| "/".into())
    };
    Ok(Outcome::Redirect { location })
}

pub(super) fn render(
    app: &mut App,
    path: &str,
    errors: &[String],
    edit: Option<UserId>,
) -> AppResult<Rendered> {
    if !app.is_open() {
        return Err(AppError::Locked);
    }
    if path == SIGN_IN && app.authenticated_session().is_ok() {
        let Outcome::Redirect { location } = landing(app)? else {
            unreachable!()
        };
        return Ok(Rendered {
            html: location,
            status: 303,
            route: "signed-in".into(),
        });
    }
    if path != SIGN_IN && app.authenticated_session().is_err() {
        return Ok(Rendered {
            html: SIGN_IN.into(),
            status: 303,
            route: "sign-in".into(),
        });
    }
    let mut vm = if path == SIGN_IN {
        json!({"company":null,"user":{"username":"","is_authenticated":false},"nav":nav::build(None,&Current::default(),false,0).map_err(AppError::Render)?,"plugins":false,"bankfeeds":false,"fb_local_preview":false,"fb_banner":""})
    } else {
        serde_json::to_value(shell(app, None, &Current::default())?)
            .map_err(crate::protection::error)?
    };
    vm["errors"] = json!(errors);
    vm["csrf_token"] = json!(d2::token(app, path));
    vm["screen"] = json!(path);
    vm["signin_path"] = json!(SIGN_IN);
    vm["session_path"] = json!(SESSION);
    vm["users_path"] = json!(USERS);
    vm["mfa_path"] = json!(MFA);
    vm["builtin"] = json!(app.builtin());
    if path == SIGN_IN {
        vm["bootstrap"] = json!(app.bootstrap_owner()?.map(|(_, name)| name));
        vm["mfa_pending"] = json!(app.mfa_pending());
        vm["mfa_name"] = json!(app.identity.mfa.pending.as_ref().filter(|_| app.mfa_pending()).map(|p| p.name.as_str()));
        // Names only: choose a local sign-in identity without exposing company data or authority.
        let names: Vec<String> = app.open.as_mut().ok_or(AppError::Locked)?.book.read(|q| {
            q.prepare("SELECT u.username FROM fb_user u JOIN desktop_user d ON d.user_id=u.id WHERE u.is_active=1 AND d.password_hash IS NOT NULL ORDER BY u.username,u.id")?
                .query_map([], |r| r.get(0))?.collect::<Result<_,_>>()
        })?;
        vm["signin_name"] = json!(if names.len() == 1 { names[0].as_str() } else { "" });
        vm["signin_names"] = json!(names);
    } else {
        let session = app.authenticated_session()?;
        vm["role"] = json!(session.role.as_str());
        vm["admin"] = json!(session.role == Role::Administrator);
        vm["home"] = json!(
            app.first_company()?
                .map(|c| format!("/c/{}/home/", c.to_db()))
                .unwrap_or_else(|| "/".into())
        );
        if path == MFA {
            let factors=app.mfa_factors(session.actor)?;
            vm["mfa_enabled"]=json!(factors.enabled());
            vm["recovery_count"]=json!(factors.recovery.len());
            vm["mfa_management"]=json!(app.mfa_management_open());
            if let Some(secret) = app.mfa_setup_secret() {
                vm["setup_qr"]=json!(crate::mfa::setup_qr(&app.open_mut()?.owner_name, &secret)?);
                vm["setup_key"]=json!(secret);
            }
            vm["recovery_codes"]=json!(&*app.identity.mfa.recovery_display);
        }
        if path == USERS {
            app.authorize(None, Permission::Administer)?;
            let open = app.open_mut()?;
            let users=open.book.read(|q| {
                let mut st=q.prepare("SELECT u.id,u.username,u.is_active,d.role,d.password_hash IS NOT NULL FROM fb_user u JOIN desktop_user d ON d.user_id=u.id ORDER BY u.username,u.id")?;
                st.query_map([],|r|Ok(json!({"id":r.get::<_,String>(0)?,"name":r.get::<_,String>(1)?,"active":r.get::<_,bool>(2)?,"role":r.get::<_,String>(3)?,"enrolled":r.get::<_,bool>(4)?})))?.collect::<Result<Vec<_>,_>>()
            })?;
            let companies = open.book.read(|q| {
                let mut st = q.prepare("SELECT id,name FROM ledger_company ORDER BY name,id")?;
                st.query_map([], |r| {
                    Ok(json!({"id":r.get::<_,String>(0)?,"name":r.get::<_,String>(1)?}))
                })?
                .collect::<Result<Vec<_>, _>>()
            })?;
            let selected = edit
                .and_then(|id| users.iter().find(|u| u["id"] == id.to_db()))
                .cloned();
            let memberships: Vec<String> = if let Some(id) = edit {
                open.book.read(|q| {
                    q.prepare("SELECT company_id FROM ledger_company_members WHERE user_id=?1")?
                        .query_map([id.to_db()], |r| r.get(0))?
                        .collect::<Result<_, _>>()
                })?
            } else {
                vec![]
            };
            vm["users"] = json!(users);
            vm["companies"] = json!(companies);
            vm["editing"] = selected.unwrap_or_else(
                || json!({"id":"","name":"","active":true,"role":"bookkeeper","enrolled":false}),
            );
            vm["memberships"] = json!(memberships);
        }
    }
    Ok(Rendered {
        html: app.screens.render("desktop/users.html", &vm)?,
        status: if errors.is_empty() { 200 } else { 400 },
        route: "desktop-users".into(),
    })
}
pub(super) fn submit(app: &mut App, path: &str, fields: Posted<'_>) -> AppResult<Outcome> {
    let action = get(fields, "action").unwrap_or("");
    let edit = get(fields, "user_id")
        .filter(|s| !s.is_empty())
        .map(|s| UserId::parse(s).ok_or_else(|| AppError::Invalid("Invalid user.".into())))
        .transpose()?;
    let result = (|| match (path, action) {
        (SIGN_IN, "enroll") => {
            if get(fields, "confirm_owner") != Some("on") {
                return Err(AppError::Invalid(
                    "Confirm that you are setting up the existing owner on this desktop.".into(),
                ));
            }
            let password = get(fields, "password").unwrap_or("");
            if Some(password) != get(fields, "password_repeat") {
                return Err(AppError::Invalid("The two user passwords differ.".into()));
            }
            app.enroll_owner(password)?;
            landing(app)
        }
        (SIGN_IN, "signin") => {
            app.sign_in(
                get(fields, "username").unwrap_or(""),
                get(fields, "password").unwrap_or(""),
            )?;
            landing(app)
        }
        (SIGN_IN,"mfa") => { app.mfa_complete_signin(get(fields,"code").unwrap_or(""))?; landing(app) },
        (SIGN_IN,"cancel_mfa") => { app.sign_out(); Ok(Outcome::Redirect{location:SIGN_IN.into()}) },
        (MFA,"authorize") => {
            app.mfa_authorize_management(get(fields,"password").unwrap_or(""),get(fields,"code").unwrap_or(""))?;
            Ok(Outcome::Redirect{location:MFA.into()})
        },
        (MFA,"begin") => { app.mfa_begin_totp()?; Ok(Outcome::Redirect{location:MFA.into()}) },
        (MFA,"confirm") => { app.mfa_confirm_totp(get(fields,"code").unwrap_or(""))?; Ok(Outcome::Redirect{location:MFA.into()}) },
        (MFA,"recovery") => { app.mfa_replace_recovery()?; Ok(Outcome::Redirect{location:MFA.into()}) },
        (MFA,"disable") => {
            if get(fields,"confirm_disable")!=Some("on") {return Err(AppError::Invalid("Confirm that you want to disable MFA.".into()));}
            app.mfa_disable()?; Ok(Outcome::Redirect{location:MFA.into()})
        },
        (MFA,"saved") => {
            app.authorize(None,Permission::Read)?;
            app.identity.mfa.recovery_display=Default::default();
            Ok(Outcome::Redirect{location:MFA.into()})
        },
        (SESSION, "signout") => {
            app.authorize(None, Permission::Read)?;
            app.sign_out();
            Ok(Outcome::Redirect {
                location: SIGN_IN.into(),
            })
        }
        (USERS, "edit") => {
            app.authorize(None, Permission::Administer)?;
            let page = render(app, USERS, &[], edit)?;
            Ok(Outcome::Page {
                html: page.html,
                status: 200,
            })
        }
        (USERS, "save") => {
            let role = Role::parse(get(fields, "role").unwrap_or(""))?;
            let companies = fields
                .iter()
                .filter(|(k, _)| k == "company")
                .map(|(_, v)| crate::session::company(v))
                .collect::<AppResult<Vec<_>>>()?;
            let password = get(fields, "password").filter(|s| !s.is_empty());
            if password.is_some() && password != get(fields, "password_repeat") {
                return Err(AppError::Invalid("The two user passwords differ.".into()));
            }
            app.save_desktop_user(
                edit,
                get(fields, "username").unwrap_or(""),
                role,
                get(fields, "active") == Some("on"),
                password,
                &companies,
            )?;
            Ok(Outcome::Redirect {
                location: SIGN_IN.into(),
            })
        }
        _ => Err(AppError::Invalid("Unknown user action.".into())),
    })();
    match result {
        Ok(outcome) => Ok(outcome),
        Err(error) => {
            let message = match error {
                AppError::Core(error) => error.message(),
                other => other.to_string(),
            };
            let page = render(app, path, &[message], edit)?;
            Ok(Outcome::Invalid { html: page.html })
        }
    }
}
