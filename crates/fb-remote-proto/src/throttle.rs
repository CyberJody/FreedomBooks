//! Sign-in throttle per connected device, on top of fb-app's own. Five failures in a row, then a
//! wait of one minute that doubles on each further failure, up to fifteen minutes. Success clears it.
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub const FAILURES_BEFORE_WAIT: u32 = 5;
pub const FIRST_WAIT: Duration = Duration::from_secs(60);
pub const MAX_WAIT: Duration = Duration::from_secs(15 * 60);

#[derive(Default)]
struct Entry {
    failures: u32,
    wait: Option<Duration>,
    until: Option<Instant>,
}

#[derive(Default)]
pub struct Throttle {
    entries: HashMap<String, Entry>,
}

impl Throttle {
    /// `Err(remaining)` while the key must wait.
    pub fn check(&self, key: &str, now: Instant) -> Result<(), Duration> {
        match self.entries.get(key).and_then(|e| e.until) {
            Some(until) if until > now => Err(until - now),
            _ => Ok(()),
        }
    }

    /// Returns true when this failure started a wait.
    pub fn failure(&mut self, key: &str, now: Instant) -> bool {
        let entry = self.entries.entry(key.to_owned()).or_default();
        entry.failures = entry.failures.saturating_add(1);
        if entry.failures < FAILURES_BEFORE_WAIT {
            return false;
        }
        let wait = entry.wait.map_or(FIRST_WAIT, |w| (w * 2).min(MAX_WAIT));
        entry.wait = Some(wait);
        entry.until = Some(now + wait);
        entry.failures = FAILURES_BEFORE_WAIT - 1;
        true
    }

    pub fn success(&mut self, key: &str) {
        self.entries.remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_after_five_failures_and_doubles() {
        let mut t = Throttle::default();
        let now = Instant::now();
        for _ in 0..4 {
            assert!(!t.failure("d", now));
            assert!(t.check("d", now).is_ok());
        }
        assert!(t.failure("d", now));
        assert_eq!(t.check("d", now).unwrap_err(), FIRST_WAIT);
        assert!(t.check("other", now).is_ok(), "per device");
        let later = now + FIRST_WAIT;
        assert!(t.check("d", later).is_ok());
        assert!(t.failure("d", later), "the next failure waits again");
        assert_eq!(t.check("d", later).unwrap_err(), FIRST_WAIT * 2);
        for _ in 0..10 {
            t.failure("d", later);
        }
        assert_eq!(t.check("d", later).unwrap_err(), MAX_WAIT);
        t.success("d");
        assert!(t.check("d", later).is_ok());
    }
}
