//! Mutual TLS with pinned, self-signed certificates (DESIGN.md section 3).
//!
//! Each host and each paired computer has its own Ed25519 key and a self-signed certificate. There
//! is no CA: each side pins the SHA-256 of the other's certificate. TLS 1.3 only, ring provider,
//! ALPN `fb-remote/1` required. Names (SNI) are ignored; the pin is the identity.
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{ClientConfig, DigitallySignedStruct, DistinguishedName, Error, ServerConfig, SignatureScheme};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex, PoisonError};
use zeroize::Zeroizing;

pub const ALPN: &[u8] = b"fb-remote/1";
pub const HOST_MISMATCH: &str = "fb-remote: the host's certificate is not the one paired";

pub type Pin = [u8; 32];

pub fn pin(cert_der: &[u8]) -> Pin {
    Sha256::digest(cert_der).into()
}

/// A short, human-comparable fingerprint of a pin: `AB12 CD34 EF56 7890`.
pub fn fingerprint(pin: &Pin) -> String {
    let hex = fb_remote_proto::hex(&pin[..8]).to_uppercase();
    hex.as_bytes().chunks(4).map(|c| String::from_utf8_lossy(c).into_owned()).collect::<Vec<_>>().join(" ")
}

pub fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// A certificate and its PKCS#8 private key. The key never appears in Debug output.
pub struct Identity {
    pub cert: Vec<u8>,
    key: Zeroizing<Vec<u8>>,
}

impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Identity {{ pin: {} }}", fingerprint(&self.pin()))
    }
}

impl Identity {
    /// A fresh Ed25519 identity. `label` goes in the certificate's name (not secret, not checked).
    pub fn generate(label: &str) -> Result<Identity, String> {
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ED25519).map_err(|e| e.to_string())?;
        let mut params = rcgen::CertificateParams::new(vec!["freedombooks.invalid".to_owned()]).map_err(|e| e.to_string())?;
        params.distinguished_name.push(rcgen::DnType::CommonName, label.chars().filter(|c| c.is_ascii_alphanumeric() || *c == ' ' || *c == '-').take(60).collect::<String>());
        // The pin, not the validity period, is what is checked; a long period means a host
        // never "expires" out from under its paired computers.
        params.not_before = rcgen::date_time_ymd(2024, 1, 1);
        params.not_after = rcgen::date_time_ymd(2124, 1, 1);
        let cert = params.self_signed(&key).map_err(|e| e.to_string())?;
        Ok(Identity { cert: cert.der().to_vec(), key: Zeroizing::new(key.serialize_der()) })
    }

    pub fn pin(&self) -> Pin {
        pin(&self.cert)
    }

    pub fn to_hex(&self) -> fb_remote_proto::wire::IdentityHex {
        fb_remote_proto::wire::IdentityHex { cert: fb_remote_proto::hex(&self.cert), key: fb_remote_proto::hex(&self.key) }
    }

    pub fn from_hex(hex: &fb_remote_proto::wire::IdentityHex) -> Result<Identity, String> {
        let cert = fb_remote_proto::unhex(&hex.cert).ok_or("invalid identity")?;
        let key = Zeroizing::new(fb_remote_proto::unhex(&hex.key).ok_or("invalid identity")?);
        // Prove the pair loads before anyone relies on it.
        let id = Identity { cert, key };
        provider().key_provider.load_private_key(id.private_key()).map_err(|_| "invalid identity key".to_owned())?;
        Ok(id)
    }

    fn chain(&self) -> Vec<CertificateDer<'static>> {
        vec![CertificateDer::from(self.cert.clone())]
    }

    fn private_key(&self) -> PrivateKeyDer<'static> {
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.key.to_vec()))
    }
}

fn algorithms() -> WebPkiSupportedAlgorithms {
    provider().signature_verification_algorithms
}

/// The gateway's view of which device certificates may complete a handshake.
pub trait DeviceGate: Send + Sync + std::fmt::Debug {
    /// A paired, unrevoked device has this pin.
    fn known(&self, pin: &Pin) -> bool;
    /// A pairing code is live, so an unknown certificate may connect to present it.
    fn pairing_open(&self) -> bool;
}

#[derive(Debug)]
struct DeviceVerifier {
    gate: Arc<dyn DeviceGate>,
    algorithms: WebPkiSupportedAlgorithms,
}

impl ClientCertVerifier for DeviceVerifier {
    fn offer_client_auth(&self) -> bool {
        true
    }
    fn client_auth_mandatory(&self) -> bool {
        true
    }
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }
    fn verify_client_cert(&self, end_entity: &CertificateDer<'_>, intermediates: &[CertificateDer<'_>], _now: UnixTime) -> Result<ClientCertVerified, Error> {
        // AccessDenied to the peer: the computer is not (or no longer) paired.
        let refused = || Error::InvalidCertificate(rustls::CertificateError::ApplicationVerificationFailure);
        if !intermediates.is_empty() {
            return Err(refused());
        }
        // Unpaired and revoked computers end here, before any frame is read. Only while an
        // Administrator's pairing code is live may an unknown certificate go on to present it.
        if self.gate.known(&pin(end_entity)) || self.gate.pairing_open() {
            Ok(ClientCertVerified::assertion())
        } else {
            Err(refused())
        }
    }
    fn verify_tls12_signature(&self, _: &[u8], _: &CertificateDer<'_>, _: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, Error> {
        Err(Error::PeerIncompatible(rustls::PeerIncompatible::Tls12NotOffered))
    }
    fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

pub fn server_config(identity: &Identity, gate: Arc<dyn DeviceGate>) -> Result<Arc<ServerConfig>, String> {
    let verifier = Arc::new(DeviceVerifier { gate, algorithms: algorithms() });
    let mut config = ServerConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| e.to_string())?
        .with_client_cert_verifier(verifier)
        .with_single_cert(identity.chain(), identity.private_key())
        .map_err(|e| e.to_string())?;
    config.alpn_protocols = vec![ALPN.to_vec()];
    config.session_storage = Arc::new(rustls::server::NoServerSessionStorage {});
    config.send_tls13_tickets = 0;
    Ok(Arc::new(config))
}

/// The client's check of the host: the pinned certificate, or (while pairing) any certificate,
/// recording what was seen so the pairing proof covers it.
#[derive(Debug)]
pub struct HostVerifier {
    expected: Option<Pin>,
    seen: Mutex<Option<Pin>>,
    algorithms: WebPkiSupportedAlgorithms,
}

impl HostVerifier {
    pub fn seen(&self) -> Option<Pin> {
        *self.seen.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl ServerCertVerifier for HostVerifier {
    fn verify_server_cert(&self, end_entity: &CertificateDer<'_>, intermediates: &[CertificateDer<'_>], _: &ServerName<'_>, _: &[u8], _: UnixTime) -> Result<ServerCertVerified, Error> {
        let got = pin(end_entity);
        *self.seen.lock().unwrap_or_else(PoisonError::into_inner) = Some(got);
        if !intermediates.is_empty() {
            return Err(Error::General(HOST_MISMATCH.into()));
        }
        match self.expected {
            Some(expected) if !bool::from(subtle::ConstantTimeEq::ct_eq(&expected[..], &got[..])) => Err(Error::General(HOST_MISMATCH.into())),
            _ => Ok(ServerCertVerified::assertion()),
        }
    }
    fn verify_tls12_signature(&self, _: &[u8], _: &CertificateDer<'_>, _: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, Error> {
        Err(Error::PeerIncompatible(rustls::PeerIncompatible::Tls12NotOffered))
    }
    fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

/// `expected` None only for pairing (the proof then binds the host certificate that was seen).
pub fn client_config(identity: &Identity, expected: Option<Pin>) -> Result<(Arc<ClientConfig>, Arc<HostVerifier>), String> {
    let verifier = Arc::new(HostVerifier { expected, seen: Mutex::new(None), algorithms: algorithms() });
    let mut config = ClientConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| e.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(verifier.clone())
        .with_client_auth_cert(identity.chain(), identity.private_key())
        .map_err(|e| e.to_string())?;
    config.alpn_protocols = vec![ALPN.to_vec()];
    config.resumption = rustls::client::Resumption::disabled();
    config.enable_sni = false;
    Ok((Arc::new(config), verifier))
}

/// The name passed to rustls (never sent: SNI is off; never checked: the pin is).
pub fn server_name() -> ServerName<'static> {
    ServerName::try_from("freedombooks.invalid").unwrap_or(ServerName::IpAddress(std::net::Ipv4Addr::LOCALHOST.into()))
}

/// Complete a TLS handshake by `deadline`, however slowly the peer sends. (rustls' `complete_io`
/// keeps reading while bytes trickle in, so a deadline per read is not a deadline per handshake.)
/// A fatal alert this side raised is sent before returning the error.
pub fn handshake<D, C>(tls: &mut C, socket: &mut std::net::TcpStream, deadline: std::time::Instant) -> std::io::Result<()>
where
    C: std::ops::DerefMut<Target = rustls::ConnectionCommon<D>>,
{
    use std::io::{Error as IoError, ErrorKind};
    while tls.is_handshaking() {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return Err(IoError::new(ErrorKind::TimedOut, "handshake took too long"));
        }
        socket.set_read_timeout(Some(left))?;
        while tls.wants_write() {
            tls.write_tls(socket)?;
        }
        if !tls.is_handshaking() {
            break;
        }
        if tls.wants_read() {
            if tls.read_tls(socket)? == 0 {
                return Err(IoError::new(ErrorKind::UnexpectedEof, "peer closed during handshake"));
            }
            if let Err(e) = tls.process_new_packets() {
                while tls.wants_write() {
                    if tls.write_tls(socket).is_err() {
                        break;
                    }
                }
                return Err(IoError::new(ErrorKind::InvalidData, e));
            }
        }
    }
    while tls.wants_write() {
        tls.write_tls(socket)?;
    }
    Ok(())
}
