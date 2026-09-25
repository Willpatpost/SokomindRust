use crate::client::prefix;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const MAX_TRACKED: usize = 65_536;

/// Fixed-window per-client limiter for saves and native solves. Anonymous
/// profile rows are otherwise unbounded (anyone can invent 32-hex profile
/// IDs), and one client could otherwise hold the solver slots.
///
/// The map is hard-bounded so attacker addresses cannot grow it without
/// limit. Stale windows are swept at most once per window; while the map is
/// still full, unknown clients are refused rather than resetting everyone's
/// budget, and known clients keep their windows.
pub struct RateLimiter {
    max: u32,
    window: Duration,
    capacity: usize,
    state: Mutex<Windows>,
}
struct Windows {
    clients: HashMap<IpAddr, (Instant, u32)>,
    swept: Instant,
}
impl RateLimiter {
    pub fn new(max: u32, window: Duration) -> Self {
        Self::with_capacity(max, window, MAX_TRACKED)
    }
    fn with_capacity(max: u32, window: Duration, capacity: usize) -> Self {
        Self {
            max,
            window,
            capacity,
            state: Mutex::new(Windows {
                clients: HashMap::new(),
                swept: Instant::now(),
            }),
        }
    }
    pub fn allow(&self, ip: IpAddr) -> bool {
        self.allow_at(ip, Instant::now())
    }
    fn allow_at(&self, ip: IpAddr, now: Instant) -> bool {
        let key = key(ip);
        let mut state = self.state.lock().unwrap();
        let Windows { clients, swept } = &mut *state;
        if now.saturating_duration_since(*swept) >= self.window {
            clients.retain(|_, (start, _)| now.saturating_duration_since(*start) < self.window);
            *swept = now;
        }
        if clients.len() >= self.capacity && !clients.contains_key(&key) {
            return false;
        }
        let slot = clients.entry(key).or_insert((now, 0));
        if now.saturating_duration_since(slot.0) >= self.window {
            *slot = (now, 0);
        }
        if slot.1 >= self.max {
            return false;
        }
        slot.1 += 1;
        true
    }
}

/// IPv6 clients are keyed by /64: one subscriber usually holds a whole
/// prefix, so per-address keys would hand out 2^64 budgets.
fn key(ip: IpAddr) -> IpAddr {
    match ip.to_canonical() {
        IpAddr::V6(v6) => prefix(IpAddr::V6(v6), 64),
        v4 => v4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: Duration = Duration::from_secs(60);

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn per_ip_cap_and_independent_addresses() {
        let limiter = RateLimiter::new(60, MINUTE);
        let first = ip("127.0.0.1");
        for _ in 0..60 {
            assert!(limiter.allow(first));
        }
        assert!(!limiter.allow(first));
        assert!(limiter.allow(ip("127.0.0.2")));
    }

    #[test]
    fn windows_restart() {
        let limiter = RateLimiter::new(2, MINUTE);
        let start = Instant::now();
        let client = ip("192.0.2.1");
        assert!(limiter.allow_at(client, start));
        assert!(limiter.allow_at(client, start));
        assert!(!limiter.allow_at(client, start + MINUTE / 2));
        assert!(limiter.allow_at(client, start + MINUTE));
    }

    #[test]
    fn ipv6_is_keyed_by_64_prefix() {
        let limiter = RateLimiter::new(2, MINUTE);
        let now = Instant::now();
        assert!(limiter.allow_at(ip("2001:db8:1:2::1"), now));
        assert!(limiter.allow_at(ip("2001:db8:1:2:ffff::9"), now));
        assert!(!limiter.allow_at(ip("2001:db8:1:2::3"), now));
        assert!(limiter.allow_at(ip("2001:db8:1:3::1"), now));
        // Mapped addresses share the IPv4 client's bucket.
        assert!(limiter.allow_at(ip("192.0.2.1"), now));
        assert!(limiter.allow_at(ip("::ffff:192.0.2.1"), now));
        assert!(!limiter.allow_at(ip("192.0.2.1"), now));
    }

    #[test]
    fn full_map_refuses_new_clients_until_swept() {
        let limiter = RateLimiter::with_capacity(3, MINUTE, 2);
        let start = Instant::now();
        let (a, b, c) = (ip("192.0.2.1"), ip("192.0.2.2"), ip("192.0.2.3"));
        assert!(limiter.allow_at(a, start));
        assert!(limiter.allow_at(b, start));
        assert!(!limiter.allow_at(c, start));
        // Known clients keep their budgets; nothing was reset.
        assert!(limiter.allow_at(a, start));
        assert!(limiter.allow_at(a, start));
        assert!(!limiter.allow_at(a, start));
        assert!(!limiter.allow_at(c, start + MINUTE / 2));
        // The next window sweeps the stale entries and admits newcomers.
        assert!(limiter.allow_at(c, start + MINUTE));
        assert!(limiter.allow_at(a, start + MINUTE));
        assert!(!limiter.allow_at(b, start + MINUTE));
    }
}
