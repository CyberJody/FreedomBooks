//! Private-network scope (DESIGN.md section 8): the classification is `fb_remote_proto::scope`
//! (the book process checks a chosen address with the same rules); this adds the client's name
//! resolution, where every address a host name resolves to must be private.
use std::net::{SocketAddr, ToSocketAddrs};

pub use fb_remote_proto::scope::{OUT_OF_SCOPE, bind, private};

/// A host address as typed on the client (`192.168.1.20:47100`, `office-pc.tailnet.ts.net:47100`).
/// Every address it resolves to must be private.
pub fn resolve(text: &str) -> Result<Vec<SocketAddr>, &'static str> {
    if text.len() > 300 || text.chars().any(|c| c.is_whitespace() || c.is_control() || c == '/' || c == '@') {
        return Err("Enter the host address and port, for example 192.168.1.20:47100.");
    }
    let addrs: Vec<SocketAddr> = text.to_socket_addrs().map_err(|_| "That host address could not be found. Check it and the port, for example 192.168.1.20:47100.")?.collect();
    if addrs.is_empty() {
        return Err("That host address could not be found.");
    }
    if addrs.iter().any(|a| a.ip().is_unspecified() || !private(a.ip())) {
        return Err(OUT_OF_SCOPE);
    }
    Ok(addrs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_private_specific_addresses_are_allowed() {
        assert!(bind("192.168.1.20:47100").is_ok() && bind("0.0.0.0:47100").is_err());
        assert!(resolve("127.0.0.1:47100").is_ok());
        assert_eq!(resolve("8.8.8.8:47100").unwrap_err(), OUT_OF_SCOPE);
        assert!(resolve("https://192.168.1.2:1").is_err());
    }
}
