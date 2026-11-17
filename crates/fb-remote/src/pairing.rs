//! Pairing proofs (DESIGN.md section 4). The one-time code keys an HMAC over both certificate pins
//! as each side saw them, so a man in the middle, who must show its own certificate to at least one
//! side, makes both proofs fail. The code is 80 bits and lives ten minutes for one device.
use crate::tls::Pin;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
use std::net::IpAddr;
use std::time::Instant;

/// Wrong proofs from all sources together that destroy a code. With 80 bits, this bounds guessing
/// far below any chance of success while one noisy peer cannot cancel the code for everyone.
pub const MAX_TOTAL_ATTEMPTS: u32 = 50;

const CLIENT_LABEL: &[u8] = b"fb-remote pair client v1\0";
const HOST_LABEL: &[u8] = b"fb-remote pair host v1\0";

fn proof(label: &[u8], code: &str, host: &Pin, device: &Pin) -> [u8; 32] {
    // HMAC accepts any key length; new_from_slice cannot fail for HMAC.
    let mut m = match <Hmac<Sha256> as Mac>::new_from_slice(code.as_bytes()) {
        Ok(m) => m,
        Err(_) => return [0; 32],
    };
    m.update(label);
    m.update(host);
    m.update(device);
    m.finalize().into_bytes().into()
}

pub fn client_proof(code: &str, host: &Pin, device: &Pin) -> [u8; 32] {
    proof(CLIENT_LABEL, code, host, device)
}

pub fn host_proof(code: &str, host: &Pin, device: &Pin) -> [u8; 32] {
    proof(HOST_LABEL, code, host, device)
}

pub fn equal(a: &[u8], b: &[u8]) -> bool {
    bool::from(subtle::ConstantTimeEq::ct_eq(a, b))
}

/// The first and only frame on a pairing connection.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum PairRequest {
    Pair { device_name: String, proof: String },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum PairReply {
    Paired { device_id: String, proof: String, host_name: String },
    Refused { message: String },
}

/// One wrong-or-expired message for every pairing refusal: which part was wrong is not said.
pub const REFUSED: &str = "That pairing code did not work. Ask the host's administrator for a new code, check the host address, and try again.";

/// The live code on the host. Wrong proofs are counted per exact address (not per IPv6 /64, so a
/// neighbour on the same network cannot lock the new computer out): an address that sent five is
/// refused for this code, and the code itself ends after [`MAX_TOTAL_ATTEMPTS`] from everyone
/// together.
pub struct Slot {
    pub code: zeroize::Zeroizing<String>,
    pub expires: Instant,
    pub attempts: u32,
    pub by_source: HashMap<IpAddr, u32>,
    pub minted_by: String,
}

impl Slot {
    pub fn new(code: String, expires: Instant, minted_by: String) -> Slot {
        Slot { code: zeroize::Zeroizing::new(code), expires, attempts: 0, by_source: HashMap::new(), minted_by }
    }

    pub fn live(&self, now: Instant) -> bool {
        now < self.expires && self.attempts < MAX_TOTAL_ATTEMPTS
    }

    /// Whether `ip` may still try this code.
    pub fn allows(&self, ip: IpAddr, now: Instant) -> bool {
        self.live(now) && self.by_source.get(&crate::limits::canonical(ip)).copied().unwrap_or(0) < fb_remote_proto::code::MAX_ATTEMPTS
    }

    /// Count a wrong proof from `ip`.
    pub fn wrong(&mut self, ip: IpAddr) {
        self.attempts += 1;
        *self.by_source.entry(crate::limits::canonical(ip)).or_default() += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_noisy_source_cannot_cancel_the_code_for_everyone() {
        let now = Instant::now();
        let mut slot = Slot::new("CODE".into(), now + std::time::Duration::from_secs(600), "Admin".into());
        let (noisy, other): (IpAddr, IpAddr) = ("10.0.0.9".parse().unwrap(), "10.0.0.10".parse().unwrap());
        for _ in 0..fb_remote_proto::code::MAX_ATTEMPTS {
            assert!(slot.allows(noisy, now));
            slot.wrong(noisy);
        }
        assert!(!slot.allows(noisy, now), "the noisy source is refused");
        assert!(slot.allows(other, now), "the right computer can still pair");
        for i in 0..MAX_TOTAL_ATTEMPTS {
            slot.wrong(format!("10.1.{}.{}", i / 200, i % 200).parse().unwrap());
        }
        assert!(!slot.live(now) && !slot.allows(other, now), "many sources together still end the code");
        // B5: a neighbour in the same IPv6 /64 does not use up the new computer's attempts.
        let mut slot = Slot::new("CODE".into(), now + std::time::Duration::from_secs(600), "Admin".into());
        let (neighbour, new_computer): (IpAddr, IpAddr) = ("fd00:1:2:3::9".parse().unwrap(), "fd00:1:2:3::10".parse().unwrap());
        for _ in 0..fb_remote_proto::code::MAX_ATTEMPTS {
            slot.wrong(neighbour);
        }
        assert!(!slot.allows(neighbour, now) && slot.allows(new_computer, now));
    }

    #[test]
    fn proofs_bind_the_code_both_pins_and_the_direction() {
        let (h, d) = ([1u8; 32], [2u8; 32]);
        let p = client_proof("CODE", &h, &d);
        assert!(equal(&p, &client_proof("CODE", &h, &d)));
        assert!(!equal(&p, &client_proof("CODF", &h, &d)), "code");
        assert!(!equal(&p, &client_proof("CODE", &[3u8; 32], &d)), "a different host certificate (MITM)");
        assert!(!equal(&p, &client_proof("CODE", &h, &[3u8; 32])), "a different device certificate (MITM)");
        assert!(!equal(&p, &host_proof("CODE", &h, &d)), "a client proof is not a host proof");
    }
}
