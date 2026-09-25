use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const WINDOW_SECS: u64 = 60;
const MAX_PER_WINDOW: u32 = 60;
const MAX_TRACKED: usize = 65_536;

/// Fixed-window per-IP limiter for progress saves. Anonymous profile rows
/// are otherwise unbounded: anyone can invent unlimited 32-hex profile IDs.
/// The map is hard-bounded so attacker addresses cannot grow it without
/// limit; stale windows are dropped first, and a full map resets.
pub struct SaveLimiter {
    window: Mutex<HashMap<IpAddr, (u64, u32)>>,
}
impl SaveLimiter {
    pub fn new() -> Self {
        Self {
            window: Mutex::new(HashMap::new()),
        }
    }
    pub fn allow(&self, ip: IpAddr) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let mut window = self.window.lock().unwrap();
        window.retain(|_, (start, _)| now.saturating_sub(*start) < WINDOW_SECS);
        if window.len() >= MAX_TRACKED {
            window.clear();
        }
        let slot = window.entry(ip).or_insert((now, 0));
        if now.saturating_sub(slot.0) >= WINDOW_SECS {
            *slot = (now, 0);
        }
        slot.1 += 1;
        slot.1 <= MAX_PER_WINDOW
    }
}
impl Default for SaveLimiter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_ip_cap_and_independent_addresses() {
        let limiter = SaveLimiter::new();
        let first: IpAddr = "127.0.0.1".parse().unwrap();
        let second: IpAddr = "127.0.0.2".parse().unwrap();
        for _ in 0..MAX_PER_WINDOW {
            assert!(limiter.allow(first));
        }
        assert!(!limiter.allow(first));
        assert!(limiter.allow(second));
    }
}
