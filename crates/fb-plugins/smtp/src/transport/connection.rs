//! Bounded SMTP framing over lettre's existing pinned TCP/Rustls transport.
//!
//! lettre 0.11.23's AsyncSmtpConnection checks limits only AFTER read_line.
//! Its stream injection hook sits below TLS, so a wrapper there would not bound
//! decrypted replies after STARTTLS. Keep lettre's TLS, command, AUTH and response
//! helpers, but own the read loop above TLS for every stage of the conversation.
//! The deprecated network API is deliberately isolated here; lettre is pinned.
#![allow(deprecated)]

use super::smtp_error;
use fb_plugin_api::{PluginError, PluginResult};
use futures_util::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use lettre::{
    address::Envelope,
    transport::smtp::{
        authentication::{Credentials, Mechanism},
        client::{AsyncNetworkStream, TlsParameters},
        commands::{Auth, Data, Ehlo, Mail, Rcpt, Starttls},
        extension::{ClientId, Extension, MailBodyParameter, MailParameter, ServerInfo},
        response::Response,
    },
};
use std::{
    fmt::Display,
    net::SocketAddr,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use zeroize::Zeroizing;

// Same ceilings as lettre 0.11.23, but enforced before extending the buffer.
// Both include status prefixes and CRLF. Socket/TLS buffering is separately
// bounded by the fixed BufReader capacity and Rustls' record buffering.
pub(super) const MAX_LINE_BYTES: usize = 1000;
pub(super) const MAX_RESPONSE_BYTES: usize = 100_000;
const READ_BUFFER_BYTES: usize = 1024;

#[cfg(test)]
mod tests;

pub(super) struct Connection {
    stream: BufReader<AsyncNetworkStream>,
    server: ServerInfo,
}

impl Connection {
    pub async fn connect(
        addresses: &[SocketAddr],
        timeout: Duration,
        hello: &ClientId,
        implicit: Option<TlsParameters>,
    ) -> PluginResult<Self> {
        // Never pass a hostname: all addresses were validated and pinned by the
        // caller. lettre still verifies the original hostname through TLS params.
        let stream = AsyncNetworkStream::connect_tokio1(addresses, Some(timeout), implicit, None)
            .await
            .map_err(smtp_error)?;
        let mut connection = Self {
            stream: BufReader::with_capacity(READ_BUFFER_BYTES, stream),
            server: ServerInfo::default(),
        };
        let greeting = read_response(&mut connection.stream).await?;
        require_code(&greeting, 220)?;
        connection.ehlo(hello).await?;
        Ok(connection)
    }

    pub fn is_encrypted(&self) -> bool {
        self.stream.get_ref().is_encrypted()
    }

    pub fn can_starttls(&self) -> bool {
        !self.is_encrypted() && self.server.supports_feature(Extension::StartTls)
    }

    pub async fn starttls(&mut self, tls: TlsParameters, hello: &ClientId) -> PluginResult<()> {
        if !self.can_starttls() {
            return Err(PluginError::Network);
        }
        let response = self.command(Starttls).await?;
        require_code(&response, 220)?;
        // Never let plaintext read-ahead survive into the authenticated session.
        // SMTP is lock-step here: no server bytes are expected until ClientHello.
        if !self.stream.buffer().is_empty() {
            return Err(PluginError::InvalidResponse);
        }
        self.stream
            .get_mut()
            .upgrade_tls(tls)
            .await
            .map_err(smtp_error)?;
        self.ehlo(hello).await
    }

    async fn ehlo(&mut self, hello: &ClientId) -> PluginResult<()> {
        let response = self.command(Ehlo::new(hello.clone())).await?;
        require_code(&response, 250)?;
        self.server = ServerInfo::from_response(&response).map_err(smtp_error)?;
        Ok(())
    }

    pub async fn auth(
        &mut self,
        mechanisms: &[Mechanism],
        credentials: &Credentials,
    ) -> PluginResult<Response> {
        if !self.is_encrypted() {
            return Err(PluginError::Network);
        }
        let mechanism = self
            .server
            .get_auth_mechanism(mechanisms)
            .ok_or(PluginError::Network)?;
        let mut command = Auth::new(mechanism, credentials.clone(), None).map_err(smtp_error)?;
        // Bound challenge exchanges as well as the bytes in each response.
        for _ in 0..10 {
            let response = self.command(command).await?;
            if !response.has_code(334) {
                return Ok(response);
            }
            command = Auth::new_from_response(mechanism, credentials.clone(), &response)
                .map_err(smtp_error)?;
        }
        Err(PluginError::InvalidResponse)
    }

    pub async fn send(
        &mut self,
        envelope: &Envelope,
        message: &[u8],
        submitted: &AtomicBool,
    ) -> PluginResult<Response> {
        if !self.is_encrypted() {
            return Err(PluginError::Network);
        }
        let mut options = Vec::new();
        if envelope
            .from()
            .into_iter()
            .chain(envelope.to())
            .any(|address| !address.user().is_ascii() || !address.domain().is_ascii())
        {
            if !self.server.supports_feature(Extension::SmtpUtfEight) {
                return Err(PluginError::Network);
            }
            options.push(MailParameter::SmtpUtfEight);
        }
        if !message.is_ascii() {
            if !self.server.supports_feature(Extension::EightBitMime) {
                return Err(PluginError::Network);
            }
            options.push(MailParameter::Body(MailBodyParameter::EightBitMime));
        }
        let response = self
            .command(Mail::new(envelope.from().cloned(), options))
            .await?;
        require_code(&response, 250)?;
        for recipient in envelope.to() {
            let response = self.command(Rcpt::new(recipient.clone(), vec![])).await?;
            if !response.has_code(250) && !response.has_code(251) && !response.has_code(252) {
                return Err(PluginError::InvalidResponse);
            }
        }
        let response = self.command(Data).await?;
        require_code(&response, 354)?;
        // From the first DATA byte onward, loss of the final response is uncertain.
        submitted.store(true, Ordering::SeqCst);
        // Dot transparency without an additional message-sized allocation.
        // MIME serialization supplies CRLF; only dots at the beginning of a
        // CRLF-delimited line (or the message start) need doubling.
        let mut start = 0;
        for (index, byte) in message.iter().enumerate() {
            if *byte == b'.' && (index == 0 || message[..index].ends_with(b"\r\n")) {
                self.write(&message[start..index]).await?;
                self.write(b".").await?;
                start = index;
            }
        }
        self.write(&message[start..]).await?;
        self.write(b"\r\n.\r\n").await?;
        self.stream
            .get_mut()
            .flush()
            .await
            .map_err(|_| PluginError::Network)?;
        read_response(&mut self.stream).await
    }

    async fn write(&mut self, bytes: &[u8]) -> PluginResult<()> {
        self.stream
            .get_mut()
            .write_all(bytes)
            .await
            .map_err(|_| PluginError::Network)
    }

    async fn command(&mut self, command: impl Display) -> PluginResult<Response> {
        let encoded = Zeroizing::new(command.to_string());
        self.write(encoded.as_bytes()).await?;
        self.stream
            .get_mut()
            .flush()
            .await
            .map_err(|_| PluginError::Network)?;
        read_response(&mut self.stream).await
    }
}

fn require_code(response: &Response, expected: u16) -> PluginResult<()> {
    if response.has_code(expected) {
        Ok(())
    } else {
        Err(PluginError::InvalidResponse)
    }
}

/// The only reply-reading path, used before AND after TLS. Never call read_line
/// or read_to_end on server input. Allocation and accumulation cannot exceed the
/// response ceiling, even if no newline ever arrives. On any error the caller
/// drops the connection immediately; do not attempt QUIT on an invalid stream.
async fn read_response<R: AsyncBufRead + Unpin>(stream: &mut R) -> PluginResult<Response> {
    // A boxed slice cannot grow. Server-controlled input never chooses allocation
    // size, and only the initialized prefix is passed to the response parser.
    let mut response = vec![0; MAX_RESPONSE_BYTES].into_boxed_slice();
    let mut used = 0;
    let mut line_start = 0;
    let mut code = None;
    loop {
        let bytes = stream.fill_buf().await.map_err(|_| PluginError::Network)?;
        if bytes.is_empty() {
            return Err(PluginError::InvalidResponse);
        }
        let count = bytes
            .iter()
            .position(|b| *b == b'\n')
            .map_or(bytes.len(), |i| i + 1);
        let line_remaining = MAX_LINE_BYTES - (used - line_start);
        let response_remaining = MAX_RESPONSE_BYTES - used;
        if count > line_remaining || count > response_remaining {
            return Err(PluginError::InvalidResponse);
        }
        response[used..used + count].copy_from_slice(&bytes[..count]);
        used += count;
        stream.consume_unpin(count);
        if !response[..used].ends_with(b"\n") {
            continue;
        }
        let line = &response[line_start..used];
        if line.len() < 6 || !line.ends_with(b"\r\n") || !line[..3].iter().all(u8::is_ascii_digit) {
            return Err(PluginError::InvalidResponse);
        }
        let current = [line[0], line[1], line[2]];
        if code.is_some_and(|code| code != current) {
            return Err(PluginError::InvalidResponse);
        }
        code = Some(current);
        let continued = line[3] == b'-';
        if !continued && line[3] != b' ' {
            return Err(PluginError::InvalidResponse);
        }
        if continued {
            line_start = used;
            continue;
        }
        let response: Response = std::str::from_utf8(&response[..used])
            .map_err(|_| PluginError::InvalidResponse)?
            .parse()
            .map_err(smtp_error)?;
        return if response.is_positive() {
            Ok(response)
        } else if [530, 534, 535, 538].contains(&u16::from(response.code())) {
            Err(PluginError::NeedsSignIn)
        } else if u16::from(response.code()) >= 500 {
            Err(PluginError::InvalidResponse)
        } else {
            Err(PluginError::Network)
        };
    }
}
