//! Narrow Stripe capability in the network process. No book, card data, arbitrary
//! URL, charge/refund operation or automatic ledger posting is available here.
use super::*;
use serde_json::{Value, json};
use url::{Url, form_urlencoded};

const API_VERSION: &str = "2026-10-28.endive";
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Input {
    #[serde(default)]
    pub link: Option<LinkRequest>,
    #[serde(default)]
    pub cursor: String,
    #[serde(default)]
    pub known_links: Vec<KnownLink>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct KnownLink {
    request: Uuid,
    provider: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LinkRequest {
    pub id: Uuid,
    pub company: Uuid,
    pub invoice: Uuid,
    pub revision: Uuid,
    pub amount: i64,
    pub currency: String,
    pub description: String,
    #[serde(default)]
    pub provider_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    version: u8,
    #[serde(deserialize_with = "secret")]
    key: SecretString,
    mode: String,
    account: String,
}
fn secret<'de, D: serde::Deserializer<'de>>(d: D) -> Result<SecretString, D::Error> {
    String::deserialize(d).map(SecretString::from)
}
fn id(value: &str, prefix: &str) -> bool {
    value.starts_with(prefix)
        && value.len() > prefix.len()
        && value.len() <= 256
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
}
fn field<'a>(v: &'a Value, k: &str) -> PluginResult<&'a str> {
    v[k].as_str().ok_or(PluginError::InvalidResponse)
}
fn checked_id(v: &Value, k: &str, prefix: &str) -> PluginResult<String> {
    let s = field(v, k)?;
    if !id(s, prefix) {
        return Err(PluginError::InvalidResponse);
    }
    Ok(s.into())
}
fn amount(v: &Value, k: &str) -> PluginResult<i64> {
    v[k].as_i64()
        .filter(|n| *n >= 0 && *n <= 99_999_999)
        .ok_or(PluginError::InvalidResponse)
}
fn flag(v: &Value, k: &str) -> PluginResult<bool> {
    v[k].as_bool().ok_or(PluginError::InvalidResponse)
}
struct Client {
    http: PluginHttp,
    base: String,
    stored: Stored,
}
impl Client {
    fn new(request: &Request) -> PluginResult<Self> {
        let stored: Stored =
            serde_json::from_str(request.record.as_deref().ok_or(PluginError::NeedsSignIn)?)
                .map_err(|_| PluginError::InvalidConfiguration)?;
        let key = stored.key.expose_secret();
        let prefix = match stored.mode.as_str() {
            "TEST" => ["rk_test_", "sk_test_"],
            "LIVE" => ["rk_live_", "sk_live_"],
            _ => return Err(PluginError::InvalidConfiguration),
        };
        if stored.version != 1
            || !prefix.iter().any(|p| key.starts_with(p))
            || key.len() < 16
            || key.len() > 512
            || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            || (!stored.account.is_empty() && !id(&stored.account, "acct_"))
        {
            return Err(PluginError::InvalidConfiguration);
        }
        #[cfg(not(feature = "fake-network"))]
        let (base, policy) = (
            "https://api.stripe.com".into(),
            HttpPolicy::new(&["api.stripe.com"])?,
        );
        #[cfg(feature = "fake-network")]
        let (base, policy) = {
            let raw = std::env::var("FB_NET_FAKE_STRIPE")
                .map_err(|_| PluginError::InvalidConfiguration)?;
            let u = Url::parse(&raw).map_err(|_| PluginError::InvalidConfiguration)?;
            if u.scheme() != "http"
                || u.host_str() != Some("127.0.0.1")
                || u.path() != "/"
                || u.query().is_some()
                || u.fragment().is_some()
                || !u.username().is_empty()
                || u.password().is_some()
            {
                return Err(PluginError::InvalidConfiguration);
            }
            (
                raw.trim_end_matches('/').to_owned(),
                HttpPolicy::loopback_for_tests(u.port().ok_or(PluginError::InvalidConfiguration)?)?,
            )
        };
        Ok(Self {
            http: PluginHttp::new(policy.with_limits(
                std::time::Duration::from_secs(25),
                64 * 1024,
                2 * 1024 * 1024,
            )?),
            base,
            stored,
        })
    }
    fn call(
        &self,
        method: HttpMethod,
        path: &str,
        fields: &[(String, String)],
        key: Option<&str>,
    ) -> PluginResult<Value> {
        let body = form_urlencoded::Serializer::new(String::new())
            .extend_pairs(fields.iter().map(|(k, v)| (k, v)))
            .finish();
        let get = matches!(method, HttpMethod::Get);
        let mut headers = vec![
            (
                "Authorization".into(),
                SecretString::from(format!("Bearer {}", self.stored.key.expose_secret())),
            ),
            ("Stripe-Version".into(), SecretString::from(API_VERSION)),
        ];
        if let Some(key) = key {
            headers.push(("Idempotency-Key".into(), SecretString::from(key)));
        }
        if !get {
            headers.push((
                "Content-Type".into(),
                SecretString::from("application/x-www-form-urlencoded"),
            ));
        }
        let response = self.http.execute(HttpRequest {
            method,
            url: SecretString::from(format!(
                "{}{path}{}",
                self.base,
                if get && !body.is_empty() {
                    format!("?{body}")
                } else {
                    String::new()
                }
            )),
            headers,
            body: SecretBytes::new(if get { vec![] } else { body.into_bytes() }),
        })?;
        match response.status {
            200..=299 => serde_json::from_slice(response.body.expose())
                .map_err(|_| PluginError::InvalidResponse),
            401 | 403 => Err(PluginError::NeedsSignIn),
            400 | 404 | 422 => Err(PluginError::InvalidConfiguration),
            429 => Err(PluginError::UsageLimit),
            _ => Err(PluginError::DeliveryUnknown),
        }
    }
    fn account(&self) -> PluginResult<String> {
        let v = self.call(HttpMethod::Get, "/v1/account", &[], None)?;
        let account = checked_id(&v, "id", "acct_")?;
        if !self.stored.account.is_empty() && self.stored.account != account {
            return Err(PluginError::WrongScope);
        }
        Ok(account)
    }
    fn mode(&self, v: &Value) -> PluginResult<()> {
        if flag(v, "livemode")? != (self.stored.mode == "LIVE") {
            return Err(PluginError::WrongScope);
        }
        Ok(())
    }
}
impl LinkRequest {
    fn validate(&self) -> PluginResult<()> {
        if [self.id, self.company, self.invoice, self.revision]
            .iter()
            .any(Uuid::is_nil)
            || self.currency != "USD"
            || !(50..=99_999_999).contains(&self.amount)
            || self.description.is_empty()
            || self.description.chars().count() > 500
            || self.description.chars().any(char::is_control)
            || (!self.provider_id.is_empty() && !id(&self.provider_id, "plink_"))
        {
            return Err(PluginError::InvalidConfiguration);
        }
        Ok(())
    }
    fn metadata(&self) -> [(String, String); 4] {
        [
            ("fb_request".into(), self.id.to_string()),
            ("fb_company".into(), self.company.to_string()),
            ("fb_invoice".into(), self.invoice.to_string()),
            ("fb_revision".into(), self.revision.to_string()),
        ]
    }
    fn matches(&self, v: &Value) -> bool {
        self.metadata()
            .iter()
            .all(|(k, value)| v["metadata"][k] == *value)
    }
    fn read_link(&self, c: &Client, v: &Value) -> PluginResult<Value> {
        c.mode(v)?;
        if !self.matches(v)
            || field(v, "currency")? != "usd"
            || v["restrictions"]["completed_sessions"]["limit"] != 1
        {
            return Err(PluginError::WrongScope);
        }
        let items = v["line_items"]["data"]
            .as_array()
            .filter(|v| v.len() == 1)
            .ok_or(PluginError::InvalidResponse)?;
        let item = &items[0];
        if v["line_items"]["has_more"] != false
            || item["quantity"] != 1
            || amount(item, "amount_total")? != self.amount
            || item["currency"] != "usd"
            || v["automatic_tax"]["enabled"] != false
            || v["allow_promotion_codes"] != false
        {
            return Err(PluginError::WrongScope);
        }
        let provider_id = checked_id(v, "id", "plink_")?;
        if !self.provider_id.is_empty() && self.provider_id != provider_id {
            return Err(PluginError::WrongScope);
        }
        let raw = field(v, "url")?;
        let url = Url::parse(raw).map_err(|_| PluginError::InvalidResponse)?;
        if url.scheme() != "https"
            || url.host_str() != Some("buy.stripe.com")
            || url.port().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.path().len() < 2
            || raw.len() > 2048
        {
            return Err(PluginError::InvalidResponse);
        }
        Ok(json!({"id":provider_id,"url":raw,"active":flag(v,"active")?}))
    }
}
fn page(v: &Value, cursor: &str, prefix: &str) -> PluginResult<(Vec<Value>, String)> {
    let rows = v["data"]
        .as_array()
        .filter(|r| r.len() <= 100)
        .ok_or(PluginError::InvalidResponse)?;
    let mut ids = std::collections::HashSet::new();
    for row in rows {
        let key = checked_id(row, "id", prefix)?;
        if key == cursor || !ids.insert(key) {
            return Err(PluginError::InvalidResponse);
        }
    }
    let next = if flag(v, "has_more")? {
        checked_id(
            rows.last().ok_or(PluginError::InvalidResponse)?,
            "id",
            prefix,
        )?
    } else {
        String::new()
    };
    Ok((rows.clone(), next))
}
pub(super) fn run(request: &Request) -> PluginResult<()> {
    let input = request
        .stripe
        .as_ref()
        .ok_or(PluginError::InvalidConfiguration)?;
    let client = Client::new(request)?;
    if request.op == "stripe_probe" {
        if input.link.is_some() || !input.cursor.is_empty() {
            return Err(PluginError::InvalidConfiguration);
        }
        let account = client.account()?;
        reply(&json!({"ok":true,"account":account,"mode":client.stored.mode}));
        return Ok(());
    }
    let link = input
        .link
        .as_ref()
        .ok_or(PluginError::InvalidConfiguration)?;
    link.validate()?;
    let recover = matches!(request.op.as_str(), "stripe_recover" | "stripe_conflicts");
    if !input.cursor.is_empty() && !id(&input.cursor, if recover { "plink_" } else { "cs_" }) {
        return Err(PluginError::InvalidConfiguration);
    }
    if client.stored.account.is_empty() {
        return Err(PluginError::WrongScope);
    }
    if let Err(error) = client.account() {
        if request.op == "stripe_create" {
            reply(&json!({"ok":true,"outcome":"refused"}));
            return Ok(());
        }
        return Err(error);
    }
    match request.op.as_str() {
        "stripe_create" => {
            if !link.provider_id.is_empty() || !input.cursor.is_empty() {
                return Err(PluginError::InvalidConfiguration);
            }
            let mut fields = vec![
                ("expand[]".into(), "line_items".into()),
                ("line_items[0][price_data][currency]".into(), "usd".into()),
                (
                    "line_items[0][price_data][unit_amount]".into(),
                    link.amount.to_string(),
                ),
                (
                    "line_items[0][price_data][product_data][name]".into(),
                    link.description.clone(),
                ),
                ("line_items[0][quantity]".into(), "1".into()),
                ("restrictions[completed_sessions][limit]".into(), "1".into()),
                (
                    "after_completion[type]".into(),
                    "hosted_confirmation".into(),
                ),
                (
                    "after_completion[hosted_confirmation][custom_message]".into(),
                    "Thank you. Your payment will be reviewed and applied to your invoice.".into(),
                ),
                ("automatic_tax[enabled]".into(), "false".into()),
                ("allow_promotion_codes".into(), "false".into()),
            ];
            for (k, v) in link.metadata() {
                fields.push((format!("metadata[{k}]"), v.clone()));
                fields.push((format!("payment_intent_data[metadata][{k}]"), v));
            }
            let result = client
                .call(
                    HttpMethod::Post,
                    "/v1/payment_links",
                    &fields,
                    Some(&format!("fb-invoice-{}", link.id)),
                )
                .and_then(|v| link.read_link(&client, &v));
            match result {
                Ok(v) => reply(&json!({"ok":true,"outcome":"created","link":v})),
                Err(e) => reply(
                    &json!({"ok":true,"outcome":if matches!(e,PluginError::NeedsSignIn|PluginError::InvalidConfiguration|PluginError::UsageLimit){"refused"}else{"unknown"}}),
                ),
            }
        }
        "stripe_conflicts" => {
            if input.known_links.len() > 2000
                || input.known_links.iter().any(|k| !id(&k.provider, "plink_"))
            {
                return Err(PluginError::InvalidConfiguration);
            }
            let mut fields = vec![("limit".into(), "100".into())];
            if !input.cursor.is_empty() {
                fields.push(("starting_after".into(), input.cursor.clone()));
            }
            let v = client.call(HttpMethod::Get, "/v1/payment_links", &fields, None)?;
            let (rows, next) = page(&v, &input.cursor, "plink_")?;
            let mut conflict = false;
            let mut blocking = vec![];
            for row in rows {
                client.mode(&row)?;
                if row["metadata"]["fb_company"] == link.company.to_string()
                    && row["metadata"]["fb_invoice"] == link.invoice.to_string()
                {
                    let known = input.known_links.iter().find(|k| {
                        row["metadata"]["fb_request"] == k.request.to_string()
                            && row["id"] == k.provider
                    });
                    if let Some(k) = known {
                        if flag(&row, "active")? {
                            conflict = true;
                            blocking.push(json!({"request":k.request,"provider":k.provider}));
                        }
                    } else {
                        conflict = true;
                    }
                }
            }
            reply(&json!({"ok":true,"conflict":conflict,"blocking":blocking,"next":next}));
        }
        "stripe_recover" => {
            let mut fields = vec![
                ("limit".into(), "100".into()),
                ("expand[]".into(), "data.line_items".into()),
            ];
            if !input.cursor.is_empty() {
                fields.push(("starting_after".into(), input.cursor.clone()));
            }
            let v = client.call(HttpMethod::Get, "/v1/payment_links", &fields, None)?;
            let (rows, next) = page(&v, &input.cursor, "plink_")?;
            let found = rows
                .iter()
                .filter(|v| v["metadata"]["fb_request"] == link.id.to_string())
                .map(|v| link.read_link(&client, v))
                .collect::<PluginResult<Vec<_>>>()?;
            if found.len() > 1 {
                return Err(PluginError::InvalidResponse);
            }
            reply(&json!({"ok":true,"found":found.first(),"next":next}));
        }
        "stripe_deactivate" | "stripe_refresh" => {
            if !id(&link.provider_id, "plink_") {
                return Err(PluginError::InvalidConfiguration);
            }
            let current = client.call(
                HttpMethod::Get,
                &format!("/v1/payment_links/{}", link.provider_id),
                &[("expand[]".into(), "line_items".into())],
                None,
            )?;
            let remote = link.read_link(&client, &current)?;
            if request.op == "stripe_deactivate" {
                if !input.cursor.is_empty() {
                    return Err(PluginError::InvalidConfiguration);
                }
                let v = client.call(
                    HttpMethod::Post,
                    &format!("/v1/payment_links/{}", link.provider_id),
                    &[
                        ("active".into(), "false".into()),
                        ("expand[]".into(), "line_items".into()),
                    ],
                    None,
                )?;
                let result = link.read_link(&client, &v)?;
                if result["active"] != false {
                    return Err(PluginError::InvalidResponse);
                }
                // Disabling a Payment Link does not close already opened Checkout
                // sessions. Expire a bounded batch; the book stays blocked until
                // a follow-up query proves none remain open.
                let open_fields = vec![
                    ("payment_link".into(), link.provider_id.clone()),
                    ("status".into(), "open".into()),
                    ("limit".into(), "5".into()),
                ];
                let open =
                    client.call(HttpMethod::Get, "/v1/checkout/sessions", &open_fields, None)?;
                let (sessions, _) = page(&open, "", "cs_")?;
                for session in sessions {
                    client.mode(&session)?;
                    if !link.matches(&session)
                        || session["payment_link"] != link.provider_id
                        || session["status"] != "open"
                    {
                        return Err(PluginError::WrongScope);
                    }
                    let id = checked_id(&session, "id", "cs_")?;
                    let expired = client.call(
                        HttpMethod::Post,
                        &format!("/v1/checkout/sessions/{id}/expire"),
                        &[],
                        None,
                    )?;
                    client.mode(&expired)?;
                    if expired["id"] != id
                        || expired["payment_link"] != link.provider_id
                        || expired["status"] != "expired"
                        || !link.matches(&expired)
                    {
                        return Err(PluginError::InvalidResponse);
                    }
                }
                let remaining =
                    client.call(HttpMethod::Get, "/v1/checkout/sessions", &open_fields, None)?;
                let (sessions, next) = page(&remaining, "", "cs_")?;
                reply(
                    &json!({"ok":true,"link":result,"closed":sessions.is_empty()&&next.is_empty()}),
                );
            } else {
                let mut fields = vec![
                    ("payment_link".into(), link.provider_id.clone()),
                    ("limit".into(), "100".into()),
                    (
                        "expand[]".into(),
                        "data.payment_intent.latest_charge".into(),
                    ),
                ];
                if !input.cursor.is_empty() {
                    fields.push(("starting_after".into(), input.cursor.clone()));
                }
                let v = client.call(HttpMethod::Get, "/v1/checkout/sessions", &fields, None)?;
                let (rows, next) = page(&v, &input.cursor, "cs_")?;
                let observations = rows
                    .iter()
                    .filter(|v| v["status"] == "complete" || !v["payment_intent"].is_null())
                    .map(|v| observation(&client, link, v))
                    .collect::<PluginResult<Vec<_>>>()?;
                reply(&json!({"ok":true,"link":remote,"observations":observations,"next":next}));
            }
        }
        _ => return Err(PluginError::InvalidConfiguration),
    }
    Ok(())
}
fn observation(client: &Client, link: &LinkRequest, v: &Value) -> PluginResult<Value> {
    client.mode(v)?;
    if !link.matches(v) || v["payment_link"] != link.provider_id || v["mode"] != "payment" {
        return Err(PluginError::WrongScope);
    }
    let session = checked_id(v, "id", "cs_")?;
    let total = amount(v, "amount_total")?;
    let currency = field(v, "currency")?;
    if currency.len() != 3 || !currency.bytes().all(|b| b.is_ascii_lowercase()) {
        return Err(PluginError::InvalidResponse);
    }
    let created = v["created"]
        .as_i64()
        .filter(|n| *n > 0 && *n < 253_402_300_800)
        .ok_or(PluginError::InvalidResponse)?;
    let mut state = "PENDING";
    let mut intent = String::new();
    let mut received = 0;
    let mut refunded = 0;
    let mut disputed = false;
    if !v["payment_intent"].is_null() {
        let pi = &v["payment_intent"];
        client.mode(pi)?;
        intent = checked_id(pi, "id", "pi_")?;
        if !link.matches(pi) {
            return Err(PluginError::WrongScope);
        }
        received = amount(pi, "amount_received")?;
        if matches!(field(pi, "status")?, "canceled" | "requires_payment_method") {
            state = "FAILED";
        }
        if !pi["latest_charge"].is_null() {
            let charge = &pi["latest_charge"];
            client.mode(charge)?;
            checked_id(charge, "id", "ch_")?;
            if charge["payment_intent"] != intent {
                return Err(PluginError::WrongScope);
            }
            refunded = amount(charge, "amount_refunded")?;
            disputed = flag(charge, "disputed")?;
            if pi["status"] == "succeeded"
                && v["payment_status"] == "paid"
                && v["status"] == "complete"
                && flag(charge, "paid")?
                && flag(charge, "captured")?
            {
                state = "READY";
            }
            if amount(charge, "amount")? != total
                || amount(charge, "amount_captured")? != total
                || charge["currency"] != currency
            {
                state = "MISMATCH";
            }
        }
        if amount(pi, "amount")? != total
            || pi["currency"] != currency
            || (state == "READY" && received != total)
        {
            state = "MISMATCH";
        }
    }
    if total != link.amount || currency != "usd" {
        state = "MISMATCH";
    }
    if refunded > 0 {
        state = "REFUNDED";
    }
    if disputed {
        state = "DISPUTED";
    }
    Ok(
        json!({"session":session,"intent":intent,"amount":total,"received":received,"refunded":refunded,"disputed":disputed,"currency":currency.to_uppercase(),"status":state,"created":created}),
    )
}
