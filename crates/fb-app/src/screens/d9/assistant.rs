//! D9: the frozen `assistant.views` settings page and its POSTs. The desktop is always Django's loopback
//! case (the sign-in returns to this computer), so the SSH-tunnel branch never shows. Sign-in, its
//! completion and disconnecting go only through `crate::plugins`; the other switches are fb-core calls.
use super::*;
use fb_core::assistant;

const USAGE_URL: &str = "https://chatgpt.com/settings/usage";
const START_URL: &str = "/auth/openai/start";
const ACTIONS: [&str; 6] = ["notice", "model", "models_refresh", "disconnect", "key", "read_attachments"];

fn staff(app: &mut App) -> AppResult<()> {
    if app.open_mut()?.staff {
        Ok(())
    } else {
        Err(AppError::NotFound("The AI assistant settings are limited to staff operators.".into()))
    }
}
fn page_path(c: CompanyId) -> String {
    format!("/c/{}/ai/", c.to_db())
}
fn route_for(c: CompanyId) -> AppResult<Route> {
    resolve(&page_path(c)).ok_or_else(|| AppError::NotFound("assistant:page".into()))
}

/// `assistant/page.html` with Django's message list (`level_tag`, text).
fn page(app: &mut App, c: CompanyId, mut messages: Vec<(&'static str, String)>) -> AppResult<String> {
    staff(app)?;
    match crate::plugins::chatgpt_poll(app, c)? {
        Some(Ok(())) => {
            let owner = app.open_mut()?.owner;
            let model = assistant::connection_for(&mut app.open_mut()?.book, owner, c)?.and_then(|v| v["model"].as_str().map(str::to_owned)).unwrap_or_default();
            messages.push(("success", format!("ChatGPT sign-in complete.{}", if model.is_empty() { " Pick a model below." } else { "" })));
        }
        Some(Err(message)) => messages.push(("error", message)),
        None => {}
    }
    let owner = app.open_mut()?.owner;
    let connection = assistant::connection_for(&mut app.open_mut()?.book, owner, c)?;
    let r = route_for(c)?;
    let path = page_path(c);
    let mut vm = super::base(app, &r, &path)?;
    let mut tokens = serde_json::Map::new();
    for action in ACTIONS {
        let target = format!("/c/{}/ai/{}/", c.to_db(), if action == "models_refresh" { "models/refresh" } else if action == "read_attachments" { "read-attachments" } else { action });
        tokens.insert(action.into(), json!(token(app, &target)));
    }
    tokens.insert("start".into(), json!(token(app, START_URL)));
    let stamp = |v: &Value| v.as_str().and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()).map(|d| d.with_timezone(&chrono::Utc).format("%Y-%m-%d %H:%M").to_string());
    let connected = connection.as_ref().is_some_and(|v| v["status"] == "connected" && v["credential_present"] == true);
    let needs_signin = connection.as_ref().is_some_and(|v| v["status"] == "needs_signin");
    let uses_plan = connection.as_ref().is_some_and(|v| v["mode"] == "siwc");
    let read_attachments = connection.as_ref().is_some_and(|v| v["read_attachments"] == true);
    let connection = connection.map(|mut v| {
        v["updated_display"] = json!(stamp(&v["updated_at"]));
        v["last_used_display"] = json!(stamp(&v["last_used_at"]));
        if v["last_error"] == "" {
            v["last_error"] = Value::Null;
        }
        v
    });
    // APIKeyForm: PasswordInput(autocomplete, spellcheck, placeholder) + maxlength/minlength, never a value.
    let key_widget = r#"<input type="password" name="api_key" autocomplete="off" spellcheck="false" placeholder="sk-..." maxlength="512" minlength="20" required aria-describedby="id_api_key_helptext" id="id_api_key">"#;
    put(&mut vm, "key_form", json!({"f": {"api_key": {"id_for_label": "id_api_key", "label": "OpenAI API key", "help_text": "Pasted once, stored encrypted, never shown again.", "widget": key_widget}}}));
    put(&mut vm, "messages", messages.into_iter().map(|(level, text)| json!({"level_tag": level, "text": text})).collect::<Vec<_>>());
    put(&mut vm, "connection", connection);
    put(&mut vm, "connected", connected);
    put(&mut vm, "needs_signin", needs_signin);
    put(&mut vm, "secrets_available", true);
    put(&mut vm, "loopback", true);
    put(&mut vm, "usage_url", USAGE_URL);
    put(&mut vm, "uses_plan", uses_plan);
    put(&mut vm, "start_url", START_URL);
    put(&mut vm, "read_attachments", read_attachments);
    put(&mut vm, "tokens", tokens);
    draw(app, "assistant/page.html", &vm)
}

/// `receipts.panel`: the Read with AI controls' context (new-bill page, "Start a bill from a receipt").
/// Reading the uploaded file needs the main process (file chooser) and a plugin host call (reported).
pub(crate) fn receipt_panel(app: &mut App, c: CompanyId) -> AppResult<Value> {
    let mut v = bill_actions(app, c, "", false)?;
    let o = v.as_object_mut().expect("panel");
    for k in ["draft", "files", "notes"] {
        o.remove(k);
    }
    Ok(v)
}

/// `receipts.bill_actions`: the Read with AI section of a bill detail page. Reading itself needs a
/// plugin host call that `crate::plugins` does not offer yet (reported); the section renders as Django's.
pub(crate) fn bill_actions(app: &mut App, c: CompanyId, identity: &str, draft: bool) -> AppResult<Value> {
    let owner = app.open_mut()?.owner;
    let staff = app.open_mut()?.staff;
    let connection = assistant::connection_for(&mut app.open_mut()?.book, owner, c)?;
    // connection_state: available = connected AND the company switch on.
    let ok = connection.as_ref().is_some_and(|v| v["read_attachments"] == true && v["status"] == "connected" && v["credential_present"] == true);
    let label = if !ok { "" } else if connection.as_ref().is_some_and(|v| v["mode"] == "siwc") { "Using ChatGPT plan" } else { "Using API key" };
    let rows = if identity.is_empty() { vec![] } else { app.open_mut()?.book.read(|tx| {
        let o = fb_core::attachments::document_owner(tx, c, identity)?;
        fb_core::attachments::active(tx, c, &o)
    })? };
    let mut files = vec![];
    for row in rows {
        let id = row["id"].as_str().unwrap_or("").to_owned();
        let url = fb_templates::django::reverse("assistant:read_attachment", &[c.to_db(), id]).map_err(AppError::Render)?;
        files.push(json!({"attachment": {"original_name": row["original_name"], "type_label": super::attachments::type_label(row["content_type"].as_str().unwrap_or(""))},
            "token": token(app, &url), "url": url, "too_large": row["size"].as_i64().unwrap_or(0) > 10 * 1024 * 1024, "request_key": uuid::Uuid::new_v4().to_string()}));
    }
    let ai_page_url = if staff { fb_templates::django::reverse("assistant:page", &[c.to_db()]).map_err(AppError::Render)? } else { String::new() };
    Ok(json!({"available": ok, "label": label, "what_is_sent": "Sends only this file and a fixed instruction to OpenAI with storage off; nothing else from your books.",
        "ai_page_url": ai_page_url, "max_mb": 10, "allowed": "PDF, PNG, JPEG or WebP", "draft": draft, "files": files, "notes": []}))
}

pub(super) fn render(app: &mut App, r: &Route, path: &str) -> AppResult<(String, u16)> {
    if r.name != "assistant:page" {
        // Every other assistant route is POST only (Django answers GET with 405).
        staff(app)?;
        let _ = path;
        return Ok((String::new(), 405));
    }
    let c = arg_company(r)?;
    let pending = std::mem::take(&mut app.messages).into_iter().map(|m| ("info", m)).collect();
    Ok((page(app, c, pending)?, 200))
}

fn after(app: &mut App, c: CompanyId, level: &'static str, text: impl Into<String>) -> AppResult<Outcome> {
    let html = page(app, c, vec![(level, text.into())])?;
    Ok(Outcome::Page { html, status: 200 })
}
fn refusal(e: FbError) -> AppResult<String> {
    match e {
        FbError::Validation(m) => Ok(m.join(" ")),
        FbError::Refused(m) => Ok(m),
        other => Err(other.into()),
    }
}

pub(super) fn submit(app: &mut App, r: &Route, p: Posted<'_>) -> AppResult<Outcome> {
    if r.name == "assistant:openai_start" {
        // CompanyField: the company the attempt is for (Django's IntegerField pk; a book id here).
        let raw = py::py_strip(get(p, "company").unwrap_or(""));
        let c = CompanyId::parse(raw).ok_or_else(|| AppError::NotFound("No such company.".into()))?;
        staff(app)?;
        let owner = app.open_mut()?.owner;
        app.open_mut()?.book.read(|tx| fb_core::services::access(tx, owner, c).map(|_| ()))?;
        return match crate::plugins::chatgpt_start(app, c)? {
            Ok(url) => Ok(Outcome::External { url, location: page_path(c) }),
            Err(message) => after(app, c, "error", message),
        };
    }
    let c = arg_company(r)?;
    staff(app)?;
    let owner = app.open_mut()?.owner;
    match r.name.as_str() {
        "assistant:read_attachments" => {
            let enabled = get(p, "enabled") == Some("1");
            assistant::set_read_attachments(&mut app.open_mut()?.book, owner, c, enabled)?;
            after(app, c, "success", if enabled { "AI may now read attachments for this company." } else { "AI reading of attachments is off for this company." })
        }
        "assistant:notice" => {
            assistant::dismiss_notice(&mut app.open_mut()?.book, owner, c)?;
            Ok(Outcome::Page { html: page(app, c, vec![])?, status: 200 })
        }
        "assistant:model" => {
            // ModelForm: CharField(max_length=100, strip=True), required.
            let model = py::py_strip(get(p, "model").unwrap_or(""));
            if model.is_empty() || py::py_len(model) > 100 || model.contains('\0') {
                return after(app, c, "error", "Choose a model from the list.");
            }
            match assistant::set_model(&mut app.open_mut()?.book, owner, c, model) {
                Ok(_) => after(app, c, "success", "Model saved."),
                Err(e) => {
                    let text = refusal(e)?;
                    after(app, c, "error", text)
                }
            }
        }
        "assistant:disconnect" => match crate::plugins::chatgpt_disconnect(app, c)? {
            Ok(()) => after(app, c, "success", "Disconnected. Nothing secret is kept."),
            Err(message) => after(app, c, "error", message),
        },
        // The API-key path and the model-list refresh need plugin host calls that crate::plugins does
        // not offer yet (reported); nothing is stored or sent.
        _ => Err(AppError::NotFound(format!("no form at {}", r.name))),
    }
}
