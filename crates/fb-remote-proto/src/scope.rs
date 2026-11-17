//! Private-network scope (design section 8), shared by the book process (which checks the address
//! an Administrator chose before it starts anything) and the `fb-remote` gateway (which checks it
//! again, and every connecting peer). Allowed: loopback, RFC 1918, link-local, IPv6 unique-local and
//! the Tailscale/CGNAT range 100.64.0.0/10. Unspecified (all-interfaces) and public addresses are
//! refused: there is no public exposure.
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

pub const OUT_OF_SCOPE: &str = "FreedomBooks connects only within your own network or tailnet (a private address such as 192.168.x.x, 10.x.x.x or a Tailscale 100.x address).";

fn v4(ip: Ipv4Addr) -> bool {
    ip.is_loopback() || ip.is_private() || ip.is_link_local() || tailscale_v4(ip)
}

/// 100.64.0.0/10, the shared (CGNAT) range Tailscale uses.
pub fn tailscale_v4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 100 && (64..128).contains(&o[1])
}

fn v6(ip: Ipv6Addr) -> bool {
    if let Some(mapped) = ip.to_ipv4_mapped() {
        return v4(mapped);
    }
    let first = ip.segments()[0];
    ip.is_loopback() || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
}

pub fn private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => v4(ip),
        IpAddr::V6(ip) => v6(ip),
    }
}

/// The host's listening address: numeric, private, and one specific interface.
pub fn bind(text: &str) -> Result<SocketAddr, &'static str> {
    let addr: SocketAddr = text.parse().map_err(|_| "Enter the address as numbers and a port, for example 192.168.1.20:47100.")?;
    if addr.ip().is_unspecified() || !private(addr.ip()) {
        return Err(OUT_OF_SCOPE);
    }
    Ok(addr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_private_specific_addresses_are_allowed() {
        for ok in ["127.0.0.1:1", "10.1.2.3:47100", "172.16.0.9:1", "192.168.1.20:47100", "169.254.1.1:1", "100.64.0.1:1", "100.127.255.254:1", "[::1]:1", "[fd7a:115c:a1e0::1]:1", "[fe80::1]:1"] {
            assert!(bind(ok).is_ok(), "{ok}");
        }
        for bad in ["0.0.0.0:47100", "[::]:47100", "8.8.8.8:1", "100.128.0.1:1", "172.32.0.1:1", "[2001:db8::1]:1", "[::ffff:8.8.8.8]:1", "localhost:1", "192.168.1.20"] {
            assert!(bind(bad).is_err(), "{bad}");
        }
        assert!(tailscale_v4("100.100.1.1".parse().unwrap()) && !tailscale_v4("100.128.0.1".parse().unwrap()));
    }
}
