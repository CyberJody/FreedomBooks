mod connection;

use crate::{message::PreparedMessage, Security, SmtpSender};
use fb_plugin_api::{
    ExposeSecret, PinnedEndpoint, PluginContext, PluginError, PluginHttp, PluginResult,
    SecretString,
};
use lettre::transport::smtp::{
    authentication::{Credentials, DEFAULT_MECHANISMS},
    client::TlsParameters,
    extension::ClientId,
};
use std::{
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

// Compiled only into this crate's unit tests. No production feature exposes an
// alternate address, resolver, root certificate or an insecure TLS mode.
#[cfg(test)]
pub(super) struct TestEndpoint {
    pub address: std::net::SocketAddr,
    pub certificate: lettre::transport::smtp::client::Certificate,
    pub tls_name: String,
}

fn remaining(start: Instant, timeout: Duration) -> PluginResult<Duration> {
    timeout
        .checked_sub(start.elapsed())
        .filter(|d| !d.is_zero())
        .ok_or(PluginError::Timeout)
}

fn addresses(
    sender: &SmtpSender,
    http: &PluginHttp,
    start: Instant,
) -> PluginResult<PinnedEndpoint> {
    #[cfg(test)]
    if let Some(endpoint) = &sender.test_endpoint {
        return http.resolve_endpoint(
            "127.0.0.1",
            endpoint.address.port(),
            remaining(start, sender.config.timeout)?,
        );
    }
    resolve_addresses(&sender.config, http, start)
}
fn resolve_addresses(
    config: &crate::SmtpConfig,
    http: &PluginHttp,
    start: Instant,
) -> PluginResult<PinnedEndpoint> {
    http.resolve_endpoint(&config.host, config.port, remaining(start, config.timeout)?)
}

pub(super) fn deliver(
    sender: &SmtpSender,
    cx: &PluginContext,
    message: &PreparedMessage,
    credentials: (SecretString, SecretString),
    start: Instant,
    submitted: &AtomicBool,
) -> PluginResult<()> {
    cx.check_active()?;
    let endpoint = addresses(sender, cx.http(), start)?;
    let resolved_at = Instant::now();
    let tls = tls_parameters(sender)?;
    // Synchronous slot, but async I/O underneath so one deadline cancels stalled
    // greetings, TLS, AUTH, DATA and slow-drip responses (not just each socket read).
    // A scoped thread also works when the host itself uses a Tokio runtime.
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("smtp-delivery".into())
            .spawn_scoped(scope, || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|_| PluginError::Network)?;
                runtime.block_on(async {
                    let operation =
                        tokio::time::timeout(remaining(resolved_at, endpoint.remaining)?, async {
                            cx.check_active()?;
                            let hello = ClientId::Domain("freedombooks.invalid".into());
                            let implicit = (sender.config.security == Security::ImplicitTls)
                                .then(|| tls.clone());
                            // Numeric pinned addresses ONLY. TLS still authenticates the original
                            // consented hostname. Never hand a hostname back to lettre's resolver.
                            let mut connection = connection::Connection::connect(
                                endpoint.addresses.as_slice(),
                                remaining(resolved_at, endpoint.remaining)?,
                                &hello,
                                implicit,
                            )
                            .await?;
                            if sender.config.security == Security::StartTls {
                                if !connection.can_starttls() {
                                    return Err(PluginError::TlsRequired);
                                }
                                connection.starttls(tls, &hello).await?;
                            }
                            if !connection.is_encrypted() {
                                return Err(PluginError::TlsRequired);
                            }
                            // Lettre owns transient String copies for its AUTH protocol. Never
                            // print its errors, commands or tracing; secrets remain scoped here.
                            cx.check_active()?;
                            let auth = Credentials::new(
                                credentials.0.expose_secret().to_owned(),
                                credentials.1.expose_secret().to_owned(),
                            );
                            let response = connection.auth(DEFAULT_MECHANISMS, &auth).await?;
                            if !response.has_code(235) {
                                return Err(PluginError::NeedsSignIn);
                            }
                            drop(auth);
                            cx.check_active()?;
                            let response = connection
                                .send(&message.envelope, message.bytes.expose(), submitted)
                                .await?;
                            if !response.has_code(250) {
                                return Err(PluginError::InvalidResponse);
                            }
                            // Acceptance is the commit point. Drop the socket, rather than turn a
                            // stalled QUIT into a reported failure (and tempt a duplicate retry).
                            // Provider response text/queue ids are deliberately never retained.
                            Ok(())
                        });
                    let cancelled = async {
                        loop {
                            if let Err(error) = cx.check_active() {
                                break Err(error);
                            }
                            tokio::time::sleep(Duration::from_millis(20)).await;
                        }
                    };
                    futures_util::pin_mut!(operation, cancelled);
                    match futures_util::future::select(operation, cancelled).await {
                        futures_util::future::Either::Left((result, _)) => {
                            result.map_err(|_| PluginError::Timeout)?
                        }
                        futures_util::future::Either::Right((result, _)) => result,
                    }
                })
            })
            .map_err(|_| PluginError::Network)?
            .join()
            .map_err(|_| PluginError::Network)?
    })
}

fn tls_parameters(sender: &SmtpSender) -> PluginResult<TlsParameters> {
    #[cfg(test)]
    if let Some(endpoint) = &sender.test_endpoint {
        return TlsParameters::builder(endpoint.tls_name.clone())
            .add_root_certificate(endpoint.certificate.clone())
            .build()
            .map_err(|_| PluginError::Network);
    }
    TlsParameters::new(sender.config.host.clone()).map_err(|_| PluginError::InvalidConfiguration)
}

#[cfg(test)]
mod tests;

fn smtp_error(error: lettre::transport::smtp::Error) -> PluginError {
    if error.is_timeout() {
        return PluginError::Timeout;
    }
    if let Some(status) = error.status() {
        if matches!(status.to_string().as_str(), "530" | "534" | "535" | "538") {
            return PluginError::NeedsSignIn;
        }
    }
    if error.is_response() || error.is_permanent() {
        PluginError::InvalidResponse
    } else {
        PluginError::Network
    }
}
