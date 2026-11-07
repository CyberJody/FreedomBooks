use base64::{engine::general_purpose::STANDARD, Engine};
use fb_plugin_api::*;
use serde_json::{json, Value};
use std::time::Duration;
use zeroize::Zeroizing;
const INPUT_CAP: usize = 16 * 1024 * 1024;

pub(crate) fn form(
    cx: &PluginContext,
    url: &str,
    fields: &[(&str, &str)],
) -> PluginResult<HttpResponse> {
    let body = Zeroizing::new(
        url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(fields.iter().copied())
            .finish(),
    );
    cx.http()
        .constrained(Duration::from_secs(30), INPUT_CAP, 2 * 1024 * 1024)?
        .execute(HttpRequest {
            method: HttpMethod::Post,
            url: SecretString::from(url.to_string()),
            headers: vec![(
                "Content-Type".into(),
                SecretString::from("application/x-www-form-urlencoded"),
            )],
            body: SecretBytes::new(body.as_bytes().to_vec()),
        })
}
pub(crate) fn decode_ok<T: serde::de::DeserializeOwned>(response: HttpResponse) -> PluginResult<T> {
    status(response.status)?;
    if response.body.expose().len() > 2 * 1024 * 1024 {
        return Err(PluginError::TooLarge);
    }
    serde_json::from_slice(response.body.expose()).map_err(|_| PluginError::InvalidResponse)
}
pub(crate) fn json_ok(response: HttpResponse) -> PluginResult<Value> {
    let v: Value = decode_ok(response)?;
    if !v.is_object() {
        return Err(PluginError::InvalidResponse);
    }
    Ok(v)
}
fn status(s: u16) -> PluginResult<()> {
    match s {
        200 => Ok(()),
        401 => Err(PluginError::NeedsSignIn),
        403 => Err(PluginError::OAuthDenied),
        408 | 504 => Err(PluginError::Timeout),
        429 => Err(PluginError::UsageLimit),
        500..=599 => Err(PluginError::Network),
        _ => Err(PluginError::InvalidResponse),
    }
}
pub(crate) fn get_json(cx: &PluginContext, url: &str) -> PluginResult<Value> {
    json_ok(
        cx.http()
            .constrained(Duration::from_secs(30), INPUT_CAP, 2 * 1024 * 1024)?
            .execute(HttpRequest {
                method: HttpMethod::Get,
                url: SecretString::from(url.to_string()),
                headers: vec![],
                body: SecretBytes::new(vec![]),
            })?,
    )
}
fn bearer_request(
    cx: &PluginContext,
    url: &str,
    bearer: &SecretString,
    body: Option<SecretBytes>,
) -> PluginResult<HttpResponse> {
    let mut headers = vec![(
        "Authorization".into(),
        SecretString::from(format!("Bearer {}", bearer.expose_secret())),
    )];
    let streaming = body.is_some();
    let method = if streaming {
        headers.push((
            "Content-Type".into(),
            SecretString::from("application/json"),
        ));
        headers.push(("Accept".into(), SecretString::from("text/event-stream")));
        HttpMethod::Post
    } else {
        HttpMethod::Get
    };
    cx.http()
        .constrained(
            Duration::from_secs(if streaming { 180 } else { 30 }),
            INPUT_CAP,
            if streaming {
                16 * 1024 * 1024
            } else {
                2 * 1024 * 1024
            },
        )?
        .execute(HttpRequest {
            method,
            url: SecretString::from(url.to_string()),
            headers,
            body: body.unwrap_or_else(|| SecretBytes::new(vec![])),
        })
}
pub(crate) fn valid_slug(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && s.as_bytes()[0].is_ascii_alphanumeric()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
}
fn label(s: &str) -> String {
    let mut tag = false;
    s.chars()
        .filter_map(|c| match c {
            '<' => {
                tag = true;
                None
            }
            '>' => {
                tag = false;
                None
            }
            _ if tag => None,
            _ => Some(if c.is_control() { ' ' } else { c }),
        })
        .take(120)
        .collect::<String>()
        .trim()
        .to_string()
}
pub(crate) fn models(
    cx: &PluginContext,
    api: &str,
    bearer: &SecretString,
) -> PluginResult<Vec<ModelChoice>> {
    let data = json_ok(bearer_request(cx, &format!("{api}/models"), bearer, None)?)?;
    let plan = data["models"].is_array();
    let rows = if plan {
        data["models"].as_array()
    } else {
        data["data"].as_array()
    }
    .ok_or(PluginError::InvalidResponse)?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            if plan && r.get("visibility").is_some_and(|s| s != "list") {
                return None;
            }
            let id = r[if plan { "slug" } else { "id" }].as_str()?.trim();
            if !valid_slug(id) {
                return None;
            }
            Some(ModelChoice {
                id: id.into(),
                label: label(if plan {
                    r["display_name"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .unwrap_or(id)
                } else {
                    id
                }),
            })
        })
        .take(200)
        .collect())
}
pub(crate) fn respond(
    cx: &PluginContext,
    api: &str,
    bearer: &SecretString,
    request: AiRequest,
) -> PluginResult<AiResponse> {
    let model = request
        .model
        .filter(|s| valid_slug(s))
        .ok_or(PluginError::InvalidConfiguration)?;
    if request.input.is_empty() {
        return Err(PluginError::InvalidConfiguration);
    }
    let mut content = vec![];
    for input in request.input {
        match input {
            AiInput::Text(text) => {
                content.push(json!({"type":"input_text", "text":text.expose_secret()}))
            }
            AiInput::File(file) => {
                if file.bytes.expose().is_empty() || file.bytes.expose().len() > 10 * 1024 * 1024 {
                    return Err(PluginError::TooLarge);
                }
                let data_url = Zeroizing::new(format!(
                    "data:{};base64,{}",
                    file.media_type,
                    STANDARD.encode(file.bytes.expose())
                ));
                match file.media_type.as_str() {
                    "application/pdf" => content.push(json!({"type":"input_file", "filename":"receipt.pdf", "file_data":data_url.as_str()})),
                    "image/png" | "image/jpeg" | "image/webp" => content.push(json!({"type":"input_image", "image_url":data_url.as_str(), "detail":"high"})),
                    _ => return Err(PluginError::InvalidConfiguration),
                }
            }
        }
    }
    let mut body = json!({"model":model, "input":[{"role":"user","content":content}],
        "instructions":request.instructions.expose_secret(), "store":false, "stream":true});
    if let Some(format) = request.text_format {
        let format: Value =
            serde_json::from_str(&format).map_err(|_| PluginError::InvalidConfiguration)?;
        if !format.is_object() {
            return Err(PluginError::InvalidConfiguration);
        }
        body["text"] = json!({"format":format});
    }
    let encoded =
        SecretBytes::new(serde_json::to_vec(&body).map_err(|_| PluginError::InvalidConfiguration)?);
    let response = bearer_request(cx, &format!("{api}/responses"), bearer, Some(encoded))?;
    status(response.status)?;
    let output = reply_text(response.body.expose())?;
    Ok(AiResponse { output, model })
}
/// The framework caps and buffers HTTP; parse SSE incrementally from that bounded
/// buffer. Never mistake EOF, refusal, incomplete or deltas alone for completion.
pub(crate) fn reply_text(bytes: &[u8]) -> PluginResult<SecretString> {
    if bytes.len() > 16 * 1024 * 1024 {
        return Err(PluginError::TooLarge);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| PluginError::InvalidResponse)?;
    let mut lines = Zeroizing::new(String::new());
    let mut event_kind = String::new();
    let mut deltas = Zeroizing::new(String::new());
    for line in text.lines().chain(std::iter::once("")) {
        if let Some(data) = line.strip_prefix("data:") {
            if !lines.is_empty() {
                lines.push('\n');
            }
            lines.push_str(data.trim_start());
        } else if let Some(kind) = line.strip_prefix("event:") {
            event_kind = kind.trim().to_string();
        } else if line.is_empty() {
            if !lines.is_empty() && lines.trim() != "[DONE]" {
                let event: Value =
                    serde_json::from_str(&lines).map_err(|_| PluginError::InvalidResponse)?;
                match event["type"].as_str().unwrap_or(&event_kind) {
                    "response.output_text.delta" => deltas.push_str(
                        event["delta"]
                            .as_str()
                            .ok_or(PluginError::InvalidResponse)?,
                    ),
                    "error"
                    | "response.failed"
                    | "response.incomplete"
                    | "response.refusal.delta"
                    | "response.refusal.done" => return Err(PluginError::InvalidResponse),
                    "response.completed" => {
                        let r = &event["response"];
                        if r["status"].as_str().is_some_and(|s| s != "completed")
                            || !r["error"].is_null()
                        {
                            return Err(PluginError::InvalidResponse);
                        }
                        let mut final_text = Zeroizing::new(String::new());
                        if let Some(items) = r["output"].as_array() {
                            for item in items {
                                if item["type"] == "message" {
                                    if let Some(parts) = item["content"].as_array() {
                                        for part in parts {
                                            if part["type"] == "refusal" {
                                                return Err(PluginError::InvalidResponse);
                                            }
                                            if part["type"] == "output_text" {
                                                final_text.push_str(
                                                    part["text"]
                                                        .as_str()
                                                        .ok_or(PluginError::InvalidResponse)?,
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        let output = if final_text.is_empty() {
                            &mut deltas
                        } else {
                            &mut final_text
                        };
                        if output.trim().is_empty() {
                            return Err(PluginError::InvalidResponse);
                        }
                        return Ok(SecretString::from(std::mem::take(&mut **output)));
                    }
                    _ => (),
                }
            }
            lines.clear();
            event_kind.clear();
        }
    }
    Err(PluginError::InvalidResponse)
}
