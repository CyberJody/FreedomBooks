//! Connection limits on the host. The certificate is checked first and the limits that can
//! keep someone out apply only to connections that have not shown a paired certificate yet:
//!
//! - Rate: a token bucket of new connections per exact address. It refuses only that address.
//! - Pending (handshaking or pairing): at most [`PENDING_PER_ADDRESS`] per exact address,
//!   [`PENDING_PER_SOURCE`] per source (an IPv4 address, or an IPv6 /64, which one machine can
//!   fill with addresses of its own) and [`PENDING`] in all. A pending pool that is full never
//!   refuses the newcomer: the oldest pending connection of that address, then of that source,
//!   then of the source holding the most, is closed instead. A real handshake takes milliseconds,
//!   so silent connections cannot hold the pool against a paired computer; and a source can only
//!   ever close its own connections or those of a bigger hog, never another source's last ones.
//! - Authenticated (a paired certificate): [`PER_ADDRESS`] per exact address and [`TOTAL`] in
//!   all, counted apart from the pending pool. There is no "trusted address" state: a paired
//!   computer is admitted the same way the first time, after a restart, or from a new address.
//! - At most one audit event per source per minute for refused handshakes, so a flood cannot
//!   fill the audit log.
use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

pub const BURST: f64 = 10.0;
pub const PER_SECOND: f64 = 1.0 / 3.0;
/// Authenticated connections per exact address.
pub const PER_ADDRESS: usize = 8;
/// Authenticated connections in all.
pub const TOTAL: usize = 64;
/// Connections not yet authenticated (handshaking or pairing), in all.
pub const PENDING: usize = 32;
/// Pending connections per exact address.
pub const PENDING_PER_ADDRESS: usize = 4;
/// Pending connections per source (IPv4 address or IPv6 /64).
pub const PENDING_PER_SOURCE: usize = 8;
const QUIET: Duration = Duration::from_secs(60);
const FORGET: Duration = Duration::from_secs(120);

/// The unit pending limits and audit quieting count by: an IPv4 address (IPv4-mapped IPv6
/// included) or an IPv6 /64.
pub fn source(ip: IpAddr) -> IpAddr {
    match canonical(ip) {
        IpAddr::V6(v6) => {
            let s = v6.segments();
            IpAddr::V6(std::net::Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0))
        }
        v4 => v4,
    }
}

/// An exact address, with IPv4-mapped IPv6 written as IPv4.
pub fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        v4 => v4,
    }
}

struct Bucket {
    tokens: f64,
    at: Instant,
    open: usize,
}

struct Pending {
    id: u64,
    ip: IpAddr,
    source: IpAddr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Pending,
    Open,
    Done,
}

/// One admitted connection. Pass it back to [`Limits::authenticated`] and [`Limits::closed`].
#[derive(Debug)]
pub struct Admitted {
    pub id: u64,
    ip: IpAddr,
    stage: Stage,
}

/// A new connection let in, and the pending connections to close to make room for it.
#[derive(Debug)]
pub struct Admission {
    pub admitted: Admitted,
    pub evict: Vec<u64>,
}

#[derive(Default)]
pub struct Limits {
    /// By exact address: the rate bucket and authenticated connections.
    buckets: HashMap<IpAddr, Bucket>,
    /// Oldest first.
    pending: Vec<Pending>,
    open: usize,
    reported: HashMap<IpAddr, Instant>,
    next: u64,
}

impl Limits {
    /// Admit a new connection from `ip`, or refuse it (only when this exact address is over its
    /// rate). Close the connections in `evict` (all still pending) to make room.
    pub fn admit(&mut self, ip: IpAddr, now: Instant) -> Option<Admission> {
        let ip = canonical(ip);
        let source = source(ip);
        if self.buckets.len() > 4096 {
            self.buckets.retain(|_, b| b.open > 0 || now.duration_since(b.at) < FORGET);
        }
        let b = self.buckets.entry(ip).or_insert_with(|| Bucket { tokens: BURST, at: now, open: 0 });
        b.tokens = (b.tokens + now.duration_since(b.at).as_secs_f64() * PER_SECOND).min(BURST);
        b.at = now;
        if b.tokens < 1.0 {
            return None;
        }
        b.tokens -= 1.0;
        let mut evict = vec![];
        let mut drop_oldest = |pending: &mut Vec<Pending>, pick: &dyn Fn(&Pending) -> bool| {
            if let Some(i) = pending.iter().position(pick) {
                evict.push(pending.remove(i).id);
            }
        };
        if self.pending.iter().filter(|p| p.ip == ip).count() >= PENDING_PER_ADDRESS {
            drop_oldest(&mut self.pending, &|p| p.ip == ip);
        }
        if self.pending.iter().filter(|p| p.source == source).count() >= PENDING_PER_SOURCE {
            drop_oldest(&mut self.pending, &|p| p.source == source);
        }
        if self.pending.len() >= PENDING {
            // The source holding the most pending connections gives up its oldest.
            let mut counts: HashMap<IpAddr, usize> = HashMap::new();
            for p in &self.pending {
                *counts.entry(p.source).or_default() += 1;
            }
            let most = counts.values().copied().max().unwrap_or(0);
            drop_oldest(&mut self.pending, &|p| counts.get(&p.source) == Some(&most));
        }
        self.next += 1;
        let id = self.next;
        self.pending.push(Pending { id, ip, source });
        Some(Admission { admitted: Admitted { id, ip, stage: Stage::Pending }, evict })
    }

    /// The connection showed a paired certificate: it leaves the pending pool and counts against
    /// the authenticated caps. False: it was closed to make room meanwhile, or this address or
    /// the host already has as many authenticated connections as allowed; close it.
    pub fn authenticated(&mut self, admitted: &mut Admitted, now: Instant) -> bool {
        if admitted.stage != Stage::Pending {
            return admitted.stage == Stage::Open;
        }
        let Some(i) = self.pending.iter().position(|p| p.id == admitted.id) else {
            admitted.stage = Stage::Done;
            return false;
        };
        self.pending.remove(i);
        let b = self.buckets.entry(admitted.ip).or_insert_with(|| Bucket { tokens: BURST, at: now, open: 0 });
        if b.open >= PER_ADDRESS || self.open >= TOTAL {
            admitted.stage = Stage::Done;
            return false;
        }
        b.open += 1;
        self.open += 1;
        admitted.stage = Stage::Open;
        true
    }

    /// Whether this connection is still let in (not closed to make room).
    pub fn admitted(&self, admitted: &Admitted) -> bool {
        match admitted.stage {
            Stage::Pending => self.pending.iter().any(|p| p.id == admitted.id),
            Stage::Open => true,
            Stage::Done => false,
        }
    }

    pub fn closed(&mut self, admitted: Admitted) {
        match admitted.stage {
            Stage::Pending => self.pending.retain(|p| p.id != admitted.id),
            Stage::Open => {
                if let Some(b) = self.buckets.get_mut(&admitted.ip) {
                    b.open = b.open.saturating_sub(1);
                }
                self.open = self.open.saturating_sub(1);
            }
            Stage::Done => {}
        }
    }

    /// Whether a refusal from `ip` should be written to the audit now.
    pub fn report(&mut self, ip: IpAddr, now: Instant) -> bool {
        if self.reported.len() > 4096 {
            self.reported.retain(|_, t| now.duration_since(*t) < QUIET);
        }
        let source = source(ip);
        if self.reported.get(&source).is_some_and(|t| now.duration_since(*t) < QUIET) {
            return false;
        }
        self.reported.insert(source, now);
        true
    }

    /// Pending and authenticated connections (tests and diagnostics).
    pub fn counts(&self) -> (usize, usize) {
        (self.pending.len(), self.open)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    /// Admit and authenticate one connection from a paired computer at `at`.
    fn paired_gets_in(l: &mut Limits, at: IpAddr, now: Instant) -> bool {
        let Some(a) = l.admit(at, now) else { return false };
        let mut c = a.admitted;
        let ok = l.authenticated(&mut c, now);
        l.closed(c);
        ok
    }

    #[test]
    fn bursts_are_limited_per_exact_address_and_refill() {
        let mut l = Limits::default();
        let a = ip("10.0.0.5");
        let now = Instant::now();
        let mut admitted = 0;
        for _ in 0..20 {
            if let Some(x) = l.admit(a, now) {
                admitted += 1;
                l.closed(x.admitted);
            }
        }
        assert_eq!(admitted, 10);
        assert!(l.admit(ip("10.0.0.6"), now).is_some(), "per address");
        assert!(l.admit(ip("fd00:1:2:3::1"), now).is_some());
        assert!(l.admit(ip("::ffff:10.0.0.5"), now).is_none(), "a mapped address is the same address");
        assert!(l.admit(a, now + Duration::from_secs(3)).is_some(), "refills");
        assert!(l.report(a, now));
        assert!(!l.report(a, now + Duration::from_secs(5)));
        assert!(l.report(a, now + Duration::from_secs(61)));
    }

    #[test]
    fn authenticated_connections_are_capped_per_address_and_in_all() {
        let mut l = Limits::default();
        let now = Instant::now();
        let a = ip("10.0.0.7");
        let mut open = vec![];
        for _ in 0..PER_ADDRESS {
            let mut c = l.admit(a, now).unwrap().admitted;
            assert!(l.authenticated(&mut c, now));
            open.push(c);
        }
        let mut extra = l.admit(a, now).unwrap().admitted;
        assert!(!l.authenticated(&mut extra, now), "beyond the per-address cap");
        l.closed(extra);
        l.closed(open.pop().unwrap());
        let mut again = l.admit(a, now + Duration::from_secs(3)).unwrap().admitted;
        assert!(l.authenticated(&mut again, now));
        open.push(again);
        for c in open {
            l.closed(c);
        }
        let mut all = vec![];
        for i in 0..TOTAL {
            let mut c = l.admit(ip(&format!("10.1.{}.{}", i / 8, i % 8)), now).unwrap().admitted;
            assert!(l.authenticated(&mut c, now));
            all.push(c);
        }
        let mut over = l.admit(ip("10.2.0.1"), now).unwrap().admitted;
        assert!(!l.authenticated(&mut over, now), "beyond the total");
        l.closed(over);
        for c in all {
            l.closed(c);
        }
        assert_eq!(l.counts(), (0, 0));
    }

    #[test]
    fn a_full_pending_pool_closes_the_oldest_instead_of_refusing() {
        let mut l = Limits::default();
        let now = Instant::now();
        let a = ip("10.0.0.8");
        let ids: Vec<u64> = (0..PENDING_PER_ADDRESS).map(|_| l.admit(a, now).unwrap().admitted.id).collect();
        let fifth = l.admit(a, now).unwrap();
        assert_eq!(fifth.evict, vec![ids[0]], "an address beyond its pending share closes its own oldest");
        // An IPv6 /64: addresses of one machine share one pending allowance.
        let mut held = vec![];
        for i in 0..PENDING_PER_SOURCE {
            let x = l.admit(ip(&format!("fd00:1:2:3::{:x}", i + 1)), now).unwrap();
            assert!(x.evict.is_empty());
            held.push(x.admitted.id);
        }
        let next = l.admit(ip("fd00:1:2:3:ffff::9"), now).unwrap();
        assert_eq!(next.evict, vec![held[0]], "another address in the same /64 closes the /64's oldest");
        assert!(l.admit(ip("fd00:1:2:4::1"), now).unwrap().evict.is_empty(), "another /64 is not affected");
        assert_eq!(source(ip("::ffff:10.0.0.1")), ip("10.0.0.1"));
    }

    /// The re-check's A3 [A] (`probe-limits2.out`): an unpaired machine in the paired computer's
    /// IPv6 /64 kept it out (admitted 0 of 60 tries).
    #[test]
    fn a_neighbour_in_the_same_slash_64_cannot_keep_a_paired_computer_out() {
        let mut l = Limits::default();
        let t0 = Instant::now();
        let device = ip("fd7a:115c:a1e0::50");
        let attacker = ip("fd7a:115c:a1e0::66");
        let mut held = vec![];
        while let Some(c) = l.admit(attacker, t0) {
            held.push(c.admitted);
        }
        assert!(paired_gets_in(&mut l, device, t0), "admitted at once");
        let (mut device_in, mut tries) = (0, 0);
        for step in 1..=60 {
            let now = t0 + Duration::from_secs(step);
            if step % 3 == 0 {
                for c in held.drain(..) {
                    l.closed(c);
                }
            }
            while let Some(c) = l.admit(attacker, now) {
                held.push(c.admitted);
            }
            // The paired computer reconnects every 3 s (within its own rate; a client keeps its
            // connection open).
            if step % 3 == 1 {
                tries += 1;
                device_in += usize::from(paired_gets_in(&mut l, device, now));
            }
        }
        assert_eq!((device_in, tries), (20, 20));
        // The neighbour using many addresses of the /64 at once: the paired computer's handshake
        // survives until more than the /64's share of newer connections arrive.
        let now = t0 + Duration::from_secs(100);
        let mut mine = l.admit(device, now).unwrap().admitted;
        for i in 0..PENDING_PER_SOURCE - 1 {
            l.admit(ip(&format!("fd7a:115c:a1e0::1:{i:x}")), now).unwrap();
        }
        assert!(l.authenticated(&mut mine, now));
        l.closed(mine);
    }

    /// The re-check's A3 [B]/[C] (`probe-limits2.out`, `probe-host2.out` [S8]): a paired computer
    /// not seen before (just paired, after a restart, after a weekend, on a new address) was kept
    /// out by 32 silent connections from 4 addresses, and indefinitely by 32 or more addresses.
    #[test]
    fn silent_connections_cannot_keep_a_paired_computer_out() {
        let t0 = Instant::now();
        let device = ip("10.0.0.50");
        // [B] 32 silent connections from 4 addresses: each address holds its share only.
        let mut l = Limits::default();
        let mut flood = vec![];
        for a in 1..=4 {
            for _ in 0..8 {
                flood.extend(l.admit(ip(&format!("10.0.1.{a}")), t0).map(|x| x.admitted));
            }
        }
        assert_eq!(l.counts().0, 4 * PENDING_PER_ADDRESS);
        assert!(paired_gets_in(&mut l, device, t0), "a paired computer never seen in this run gets in");
        // 32 silent connections from 32 addresses fill the pool: the newcomer closes the oldest.
        let mut l = Limits::default();
        for a in 1..=32 {
            l.admit(ip(&format!("10.0.2.{a}")), t0).unwrap();
        }
        assert_eq!(l.counts().0, PENDING);
        let x = l.admit(device, t0).unwrap();
        assert_eq!(x.evict.len(), 1);
        let mut c = x.admitted;
        assert!(l.authenticated(&mut c, t0));
        l.closed(c);
        // [C] Sustained: 16, 32 and 40 addresses each reconnecting as fast as allowed, every
        // silent connection held for the 3 s handshake deadline. The paired computer arrives
        // after the tick's flood and its handshake (milliseconds) ends before the next tick.
        for addrs in [16usize, 32, 40] {
            let mut l = Limits::default();
            let mut open: Vec<(Instant, Admitted)> = vec![];
            let (mut device_in, mut tries) = (0, 0);
            for tick in 0..600u64 {
                let now = t0 + Duration::from_millis(tick * 100);
                let (done, keep): (Vec<_>, Vec<_>) = open.into_iter().partition(|(at, c)| now.duration_since(*at) >= Duration::from_secs(3) || !l.admitted(c));
                open = keep;
                for (_, c) in done {
                    l.closed(c);
                }
                for a in 0..addrs {
                    if let Some(c) = l.admit(ip(&format!("10.0.{}.{}", 1 + a / 200, 1 + a % 200)), now) {
                        open.push((now, c.admitted));
                    }
                }
                // Every 3 s: within the computer's own rate (a client keeps its connection open).
                if tick >= 300 && tick % 30 == 5 {
                    tries += 1;
                    device_in += usize::from(paired_gets_in(&mut l, device, now + Duration::from_millis(5)));
                }
            }
            assert_eq!((device_in, tries), (10, 10), "{addrs} flooding addresses");
            assert!(l.counts().0 <= PENDING);
        }
    }

    /// The re-check's A3 third case: while a pairing code is live, connections that completed
    /// the handshake with an unknown certificate stay pending (so can be closed to make room).
    #[test]
    fn connections_held_open_while_pairing_cannot_fill_the_pool_for_long() {
        let mut l = Limits::default();
        let now = Instant::now();
        for a in 1..=4 {
            for _ in 0..8 {
                l.admit(ip(&format!("10.0.3.{a}")), now);
            }
        }
        assert_eq!(l.counts().0, 16, "four addresses hold four each, not the whole pool");
        let new_computer = l.admit(ip("10.0.0.60"), now).unwrap();
        assert!(new_computer.evict.is_empty());
        assert!(l.admitted(&new_computer.admitted));
    }
}
