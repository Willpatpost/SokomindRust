//! The server's settings, read from the environment once at startup.
use crate::client::TrustedProxies;
use std::{env, error::Error, fmt::Display, ops::RangeInclusive, str::FromStr};

/// SOLVE_CONCURRENCY's top. Each slot may reserve up to the top of
/// MEMORY_MIB in crates/server/src/solve.rs, which asserts that 8 slots
/// make 8 GiB; README and .env.example quote both numbers.
pub const MAX_SOLVE_CONCURRENCY: u32 = 8;

/// The server's settings, read once at startup. Their defaults are written
/// here and in README's Configuration table, nowhere else.
pub struct Config {
    /// BIND_ADDR: the address the API listens on.
    pub bind: String,
    /// TRUSTED_PROXIES: whose X-Forwarded-For names the client.
    pub proxies: TrustedProxies,
    /// SOLVE_CONCURRENCY: native solves that may run at once.
    pub solve_concurrency: u32,
    /// SOLVE_RATE_PER_MINUTE: native solves each client may start a minute.
    pub solve_rate: u32,
}

impl Config {
    /// Reads the process environment; see [`Config::from_lookup`].
    pub fn from_env() -> Result<Self, Box<dyn Error>> {
        Self::from_lookup(|name| env::var(name).ok())
    }

    /// Reads each setting once through `var`, which answers like
    /// `env::var(name).ok()`. Numeric settings fall back with a warning on
    /// stderr (see [`parse_setting`]); any other invalid value is an error,
    /// so it stops startup.
    pub fn from_lookup(var: impl Fn(&str) -> Option<String>) -> Result<Self, Box<dyn Error>> {
        let vars = Vars(var);
        let proxies = TrustedProxies::parse(&vars.value("TRUSTED_PROXIES").unwrap_or_default())?;
        let solve_concurrency = vars.setting("SOLVE_CONCURRENCY", 1..=MAX_SOLVE_CONCURRENCY, 1);
        let solve_rate = vars.setting("SOLVE_RATE_PER_MINUTE", 1..=600, 20);
        let bind = vars
            .value("BIND_ADDR")
            .unwrap_or_else(|| "127.0.0.1:3000".into());
        Ok(Self {
            bind,
            proxies,
            solve_concurrency,
            solve_rate,
        })
    }

    /// What an empty environment gives: every default.
    #[cfg(test)]
    pub fn defaults() -> Self {
        Self::from_lookup(|_| None).unwrap()
    }
}

/// A settings source; `.0` answers like `env::var(name).ok()`.
struct Vars<F>(F);

impl<F: Fn(&str) -> Option<String>> Vars<F> {
    /// `name`'s value. Empty counts as unset, which is what compose's
    /// `${X:-}` passes for a setting `.env` leaves out.
    fn value(&self, name: &str) -> Option<String> {
        (self.0)(name).filter(|value| !value.is_empty())
    }

    /// `name`, read by [`parse_setting`]; a warning goes to stderr.
    fn setting<T: FromStr + PartialOrd + Copy + Display>(
        &self,
        name: &str,
        range: RangeInclusive<T>,
        default: T,
    ) -> T {
        let (value, warning) =
            parse_setting(name, &self.value(name).unwrap_or_default(), range, default);
        if let Some(warning) = warning {
            eprintln!("{warning}");
        }
        value
    }
}

/// Empty means `default`; out-of-range values are clamped and anything
/// else falls back to `default`, with a warning either way.
fn parse_setting<T: FromStr + PartialOrd + Copy + Display>(
    name: &str,
    value: &str,
    range: RangeInclusive<T>,
    default: T,
) -> (T, Option<String>) {
    if value.is_empty() {
        return (default, None);
    }
    let (start, end) = (*range.start(), *range.end());
    match value.parse::<T>() {
        Ok(n) if range.contains(&n) => (n, None),
        Ok(n) => {
            let clamped = if n < start { start } else { end };
            let warning = format!("{name}={n} is outside {start}..{end}; using {clamped}");
            (clamped, Some(warning))
        }
        Err(_) => {
            let warning = format!("{name}={value:?} is not a number; using {default}");
            (default, Some(warning))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads `vars` as the whole environment.
    fn parse(vars: &[(&str, &str)]) -> Result<Config, Box<dyn Error>> {
        Config::from_lookup(|name| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        })
    }

    #[test]
    fn empty_values_are_unset_and_take_the_defaults() {
        let empty = [
            "BIND_ADDR",
            "TRUSTED_PROXIES",
            "SOLVE_CONCURRENCY",
            "SOLVE_RATE_PER_MINUTE",
        ]
        .map(|name| (name, ""));
        let unset: [(&str, &str); 0] = [];
        for vars in [&unset[..], &empty[..]] {
            let config = parse(vars).unwrap();
            assert_eq!(config.bind, "127.0.0.1:3000");
            assert_eq!(config.solve_concurrency, 1);
            assert_eq!(config.solve_rate, 20);
        }
    }

    #[test]
    fn invalid_proxies_stop_startup() {
        let Err(error) = parse(&[("TRUSTED_PROXIES", "10.0.0.0/8,nginx")]) else {
            panic!("an invalid TRUSTED_PROXIES was accepted");
        };
        assert_eq!(
            error.to_string(),
            "TRUSTED_PROXIES entry \"nginx\" is not an IP address or CIDR"
        );
    }

    #[test]
    fn settings_default_clamp_and_reject() {
        let cases = [
            ("", 1, None),
            ("4", 4, None),
            ("8", 8, None),
            ("0", 1, Some("N=0 is outside 1..8; using 1")),
            ("99", 8, Some("N=99 is outside 1..8; using 8")),
            ("-1", 1, Some("N=\"-1\" is not a number; using 1")),
            ("two", 1, Some("N=\"two\" is not a number; using 1")),
        ];
        for (value, expected, warning) in cases {
            let (n, message) = parse_setting::<usize>("N", value, 1..=8, 1);
            assert_eq!((n, message.as_deref()), (expected, warning), "{value:?}");
        }
    }
}
