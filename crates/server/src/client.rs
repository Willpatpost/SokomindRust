use axum::http::HeaderMap;
use std::net::IpAddr;

/// Networks whose X-Forwarded-For entries are believed (`TRUSTED_PROXIES`).
/// Anyone else can write any address into the header, so without a trusted
/// peer the client is simply the socket address.
#[derive(Default)]
pub struct TrustedProxies(Vec<(IpAddr, u8)>);

impl TrustedProxies {
    /// Comma-separated IPs or CIDRs; an empty list trusts no proxy.
    pub fn parse(list: &str) -> Result<Self, String> {
        let mut networks = Vec::new();
        for entry in list
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
        {
            networks.push(network(entry).ok_or_else(|| {
                format!("TRUSTED_PROXIES entry {entry:?} is not an IP address or CIDR")
            })?);
        }
        Ok(Self(networks))
    }
    fn contains(&self, ip: IpAddr) -> bool {
        self.0.iter().any(|&(net, bits)| prefix(ip, bits) == net)
    }
    /// Proxies append, so walk X-Forwarded-For right to left and stop at the
    /// first hop no trusted proxy vouches for: everything left of it is
    /// client-controlled. An unreadable hop ends the walk at the last trusted
    /// one; a chain of only trusted hops yields its leftmost entry.
    pub fn client(&self, headers: &HeaderMap, peer: IpAddr) -> IpAddr {
        let mut client = peer.to_canonical();
        if !self.contains(client) {
            return client;
        }
        let hops = headers
            .get_all("x-forwarded-for")
            .iter()
            .rev()
            .flat_map(|line| line.to_str().unwrap_or("").rsplit(','));
        for hop in hops {
            let Ok(ip) = hop.trim().parse::<IpAddr>() else {
                break;
            };
            client = ip.to_canonical();
            if !self.contains(client) {
                break;
            }
        }
        client
    }
}

/// `ip` with everything after the first `bits` bits cleared.
pub fn prefix(ip: IpAddr, bits: u8) -> IpAddr {
    match ip {
        IpAddr::V4(v4) => {
            let mask = u32::MAX
                .checked_shl(32u32.saturating_sub(bits.into()))
                .unwrap_or(0);
            IpAddr::V4((u32::from(v4) & mask).into())
        }
        IpAddr::V6(v6) => {
            let mask = u128::MAX
                .checked_shl(128u32.saturating_sub(bits.into()))
                .unwrap_or(0);
            IpAddr::V6((u128::from(v6) & mask).into())
        }
    }
}

/// IPv4-mapped entries are stored as IPv4, matching normalized clients.
fn network(entry: &str) -> Option<(IpAddr, u8)> {
    let (address, bits) = match entry.split_once('/') {
        Some((address, bits)) => (address, Some(bits.parse::<u8>().ok()?)),
        None => (entry, None),
    };
    let parsed: IpAddr = address.parse().ok()?;
    let ip = parsed.to_canonical();
    let width = if ip.is_ipv4() { 32 } else { 128 };
    let bits = match bits {
        Some(bits) if parsed.is_ipv6() && ip.is_ipv4() => bits.checked_sub(96)?,
        Some(bits) => bits,
        None => width,
    };
    (bits <= width).then(|| (prefix(ip, bits), bits))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }
    fn forwarded(lines: &[&'static str]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for &line in lines {
            headers.append("x-forwarded-for", HeaderValue::from_static(line));
        }
        headers
    }

    #[test]
    fn parses_addresses_and_networks() {
        let proxies =
            TrustedProxies::parse(" 172.16.0.0/12, 127.0.0.1 ,::1,2001:db8::/32,").unwrap();
        assert!(proxies.contains(ip("172.31.255.1")));
        assert!(!proxies.contains(ip("172.32.0.1")));
        assert!(proxies.contains(ip("127.0.0.1")));
        assert!(!proxies.contains(ip("127.0.0.2")));
        assert!(proxies.contains(ip("::1")));
        assert!(proxies.contains(ip("2001:db8:ffff::1")));
        assert!(!proxies.contains(ip("2001:db9::1")));
        assert!(
            TrustedProxies::parse("0.0.0.0/0")
                .unwrap()
                .contains(ip("203.0.113.9"))
        );
        assert!(TrustedProxies::parse("").unwrap().0.is_empty());
        assert!(TrustedProxies::parse(" , ").unwrap().0.is_empty());
    }

    #[test]
    fn invalid_entries_are_named() {
        for entry in [
            "10.0.0.0/33",
            "::/129",
            "proxy",
            "10.0.0.1/",
            "10.0.0/8",
            "::ffff:10.0.0.0/8",
        ] {
            let error = TrustedProxies::parse(&format!("127.0.0.1,{entry}"))
                .err()
                .unwrap();
            assert!(error.contains(&format!("{entry:?}")), "{error}");
        }
    }

    #[test]
    fn untrusted_peer_ignores_forwarded_for() {
        let proxies = TrustedProxies::parse("10.0.0.0/8").unwrap();
        let headers = forwarded(&["198.51.100.7"]);
        assert_eq!(
            proxies.client(&headers, ip("203.0.113.9")),
            ip("203.0.113.9")
        );
        let nobody = TrustedProxies::default();
        assert_eq!(nobody.client(&headers, ip("10.0.0.2")), ip("10.0.0.2"));
    }

    #[test]
    fn trusted_peer_yields_rightmost_untrusted_hop() {
        let proxies = TrustedProxies::parse("10.0.0.0/8").unwrap();
        let spoofed = forwarded(&["1.2.3.4, 198.51.100.7, 10.0.0.5"]);
        assert_eq!(proxies.client(&spoofed, ip("10.0.0.2")), ip("198.51.100.7"));
        let lines = forwarded(&["1.2.3.4", "198.51.100.7,10.0.0.5", " 10.0.0.6 "]);
        assert_eq!(proxies.client(&lines, ip("10.0.0.2")), ip("198.51.100.7"));
        assert_eq!(
            proxies.client(&HeaderMap::new(), ip("10.0.0.2")),
            ip("10.0.0.2")
        );
    }

    #[test]
    fn all_trusted_chain_yields_leftmost_hop() {
        let proxies = TrustedProxies::parse("10.0.0.0/8").unwrap();
        let headers = forwarded(&["10.0.0.9, 10.0.0.5"]);
        assert_eq!(proxies.client(&headers, ip("10.0.0.2")), ip("10.0.0.9"));
    }

    #[test]
    fn unparseable_hop_stops_at_last_trusted_one() {
        let proxies = TrustedProxies::parse("10.0.0.0/8").unwrap();
        let headers = forwarded(&["198.51.100.7, junk, 10.0.0.5"]);
        assert_eq!(proxies.client(&headers, ip("10.0.0.2")), ip("10.0.0.5"));
        let trailing = forwarded(&["198.51.100.7,"]);
        assert_eq!(proxies.client(&trailing, ip("10.0.0.2")), ip("10.0.0.2"));
        let port = forwarded(&["198.51.100.7:4000"]);
        assert_eq!(proxies.client(&port, ip("10.0.0.2")), ip("10.0.0.2"));
    }

    #[test]
    fn ipv4_mapped_addresses_are_normalized() {
        let proxies = TrustedProxies::parse("10.0.0.0/8,::ffff:192.0.2.0/120").unwrap();
        let headers = forwarded(&["::ffff:198.51.100.7, ::ffff:192.0.2.4"]);
        assert_eq!(
            proxies.client(&headers, ip("::ffff:10.0.0.2")),
            ip("198.51.100.7")
        );
        assert_eq!(
            proxies.client(&HeaderMap::new(), ip("::ffff:203.0.113.9")),
            ip("203.0.113.9")
        );
    }
}
