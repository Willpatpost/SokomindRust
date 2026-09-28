use crate::client::TrustedProxies;
use sqlx::postgres::PgConnectOptions;
use std::{env, error::Error, fmt::Display, ops::RangeInclusive, str::FromStr};

const MAX_PROGRESS_CONCURRENCY: u32 = 32;
const MAX_RETENTION_DAYS: i32 = 36_500;

/// The server's settings, read once at startup. Their defaults are written
/// here and in README's Configuration table, nowhere else.
pub struct Config {
    pub bind: String,
    pub proxies: TrustedProxies,
    pub solve_concurrency: usize,
    pub solve_rate: u32,
    pub progress_concurrency: u32,
    /// `None` turns server progress persistence off.
    pub database: Option<Database>,
}

/// Where PostgreSQL is and how the server uses it; see
/// [`crate::database::open`].
pub struct Database {
    /// Without the request limits, which the request pool adds.
    pub options: PgConnectOptions,
    pub pool_size: u32,
    /// Progress older than this many days expires; `None` keeps it forever.
    pub retention_days: Option<i32>,
    pub retention_batch_size: i64,
}

impl Config {
    /// Reads the process environment; see [`Config::from_lookup`].
    pub fn from_env() -> Result<Self, Box<dyn Error>> {
        Self::from_lookup(|name| env::var(name).ok())
    }

    /// Reads each setting once through `var`, which answers like
    /// `env::var(name).ok()`. Numeric settings fall back with a warning on
    /// stderr (see [`parse_setting`]); any other invalid value is an error,
    /// so it stops startup before the database wait.
    pub fn from_lookup(var: impl Fn(&str) -> Option<String>) -> Result<Self, Box<dyn Error>> {
        let vars = Vars(var);
        let proxies = TrustedProxies::parse(&vars.value("TRUSTED_PROXIES").unwrap_or_default())?;
        let solve_concurrency = vars.setting("SOLVE_CONCURRENCY", 1..=8, 1);
        let solve_rate = vars.setting("SOLVE_RATE_PER_MINUTE", 1..=600, 20);
        let progress_concurrency =
            vars.setting("PROGRESS_CONCURRENCY", 1..=MAX_PROGRESS_CONCURRENCY, 4);
        let (pool_size, warnings) = pool_size(
            &vars.value("DB_POOL_SIZE").unwrap_or_default(),
            progress_concurrency,
        );
        for warning in warnings {
            eprintln!("{warning}");
        }
        let retention_batch_size = vars.setting("PROGRESS_RETENTION_BATCH_SIZE", 1..=5000, 500);
        let retention_days =
            retention(vars.setting("PROGRESS_RETENTION_DAYS", 0..=MAX_RETENTION_DAYS, 0));
        let database = vars.database_options()?.map(|options| Database {
            options,
            pool_size,
            retention_days,
            retention_batch_size,
        });
        let bind = vars
            .value("BIND_ADDR")
            .unwrap_or_else(|| "127.0.0.1:3000".into());
        Ok(Self {
            bind,
            proxies,
            solve_concurrency,
            solve_rate,
            progress_concurrency,
            database,
        })
    }

    /// What an empty environment gives: every default, persistence off.
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

    /// DATABASE_URL wins. Otherwise DATABASE_PASSWORD enables persistence and
    /// the other DATABASE_* parts default to the compose service; the parts
    /// are passed as they are, so a raw password needs no URL encoding.
    fn database_options(&self) -> Result<Option<PgConnectOptions>, Box<dyn Error>> {
        if let Some(url) = self.value("DATABASE_URL") {
            return Ok(Some(url.parse()?));
        }
        let Some(password) = self.value("DATABASE_PASSWORD") else {
            return Ok(None);
        };
        let part = |name: &str, default: &str| -> String {
            self.value(name).unwrap_or_else(|| default.into())
        };
        let port = part("DATABASE_PORT", "5432");
        let port: u16 = port
            .parse()
            .map_err(|_| format!("DATABASE_PORT={port:?} is not a port number"))?;
        Ok(Some(
            PgConnectOptions::new()
                .host(&part("DATABASE_HOST", "db"))
                .port(port)
                .username(&part("DATABASE_USER", "sokomind"))
                .password(&password)
                .database(&part("DATABASE_NAME", "sokomind")),
        ))
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

/// DB_POOL_SIZE, read as by [`parse_setting`]. Empty means one connection per
/// progress slot plus a spare for the health probe and the retention sweep.
/// A smaller pool is allowed but warned about: a save holding a progress slot
/// can then wait for a connection and fail with 503 after
/// `database::ACQUIRE_TIMEOUT`.
fn pool_size(value: &str, progress_concurrency: u32) -> (u32, Vec<String>) {
    let wanted = progress_concurrency + 1;
    let range = 1..=MAX_PROGRESS_CONCURRENCY + 1;
    let (size, warning) = parse_setting("DB_POOL_SIZE", value, range, wanted);
    let below = (size < wanted).then(|| {
        format!(
            "DB_POOL_SIZE={size} is below PROGRESS_CONCURRENCY + 1 = {wanted}; \
             saves can wait for a connection and fail with 503"
        )
    });
    (size, warning.into_iter().chain(below).collect())
}

/// PROGRESS_RETENTION_DAYS: 0, the default, keeps progress forever.
fn retention(days: i32) -> Option<i32> {
    (days > 0).then_some(days)
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
            "PROGRESS_CONCURRENCY",
            "DB_POOL_SIZE",
            "PROGRESS_RETENTION_DAYS",
            "PROGRESS_RETENTION_BATCH_SIZE",
            "DATABASE_URL",
            "DATABASE_PASSWORD",
        ]
        .map(|name| (name, ""));
        let unset: [(&str, &str); 0] = [];
        for vars in [&unset[..], &empty[..]] {
            let config = parse(vars).unwrap();
            assert_eq!(config.bind, "127.0.0.1:3000");
            assert_eq!(config.solve_concurrency, 1);
            assert_eq!(config.solve_rate, 20);
            assert_eq!(config.progress_concurrency, 4);
            assert!(config.database.is_none());
        }
    }

    #[test]
    fn database_url_wins_and_carries_the_pool_and_retention_settings() {
        let config = parse(&[
            ("DATABASE_URL", "postgres://u:p@example.org:6543/d"),
            ("DATABASE_PASSWORD", "unused"),
            ("DATABASE_HOST", "unused"),
            ("PROGRESS_CONCURRENCY", "8"),
            ("PROGRESS_RETENTION_DAYS", "30"),
            ("PROGRESS_RETENTION_BATCH_SIZE", "50"),
        ])
        .unwrap();
        assert_eq!(config.progress_concurrency, 8);
        let database = config.database.expect("persistence on");
        let options = &database.options;
        assert_eq!(options.get_host(), "example.org");
        assert_eq!(options.get_port(), 6543);
        assert_eq!(options.get_username(), "u");
        assert_eq!(options.get_database(), Some("d"));
        assert_eq!(database.pool_size, 9);
        assert_eq!(database.retention_days, Some(30));
        assert_eq!(database.retention_batch_size, 50);
    }

    #[test]
    fn database_password_uses_the_compose_parts() {
        let config = parse(&[
            ("DATABASE_PASSWORD", "p@ss/word:%"),
            ("DATABASE_PORT", "6000"),
            ("DATABASE_USER", ""),
        ])
        .unwrap();
        let database = config.database.expect("persistence on");
        let options = &database.options;
        assert_eq!(options.get_host(), "db");
        assert_eq!(options.get_port(), 6000);
        assert_eq!(options.get_username(), "sokomind");
        assert_eq!(options.get_database(), Some("sokomind"));
        assert_eq!(database.pool_size, 5);
        assert_eq!(database.retention_days, None);
        assert_eq!(database.retention_batch_size, 500);
    }

    #[test]
    fn invalid_non_numeric_settings_stop_startup() {
        let cases = [
            (
                ("TRUSTED_PROXIES", "10.0.0.0/8,nginx"),
                "TRUSTED_PROXIES entry \"nginx\" is not an IP address or CIDR",
            ),
            (
                ("DATABASE_PORT", "http"),
                "DATABASE_PORT=\"http\" is not a port number",
            ),
        ];
        for (var, message) in cases {
            let Err(error) = parse(&[("DATABASE_PASSWORD", "secret"), var]) else {
                panic!("{var:?} was accepted");
            };
            assert_eq!(error.to_string(), message);
        }
        // Not a URL at all: sqlx's own parse error.
        assert!(parse(&[("DATABASE_URL", "example.org/sokomind")]).is_err());
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

    #[test]
    fn pool_defaults_to_progress_slots_plus_one_and_warns_below() {
        let below = |size: u32| {
            format!(
                "DB_POOL_SIZE={size} is below PROGRESS_CONCURRENCY + 1 = 5; \
                 saves can wait for a connection and fail with 503"
            )
        };
        let cases = [
            // compose passes an empty value for a setting .env leaves out.
            ("", 4, 5, vec![]),
            ("", 1, 2, vec![]),
            ("", 32, 33, vec![]),
            ("5", 4, 5, vec![]),
            ("12", 4, 12, vec![]),
            ("3", 4, 3, vec![below(3)]),
            (
                "0",
                4,
                1,
                vec![
                    "DB_POOL_SIZE=0 is outside 1..33; using 1".to_owned(),
                    below(1),
                ],
            ),
            (
                "99",
                32,
                33,
                vec!["DB_POOL_SIZE=99 is outside 1..33; using 33".to_owned()],
            ),
            (
                "many",
                4,
                5,
                vec!["DB_POOL_SIZE=\"many\" is not a number; using 5".to_owned()],
            ),
        ];
        for (value, progress, size, warnings) in cases {
            let result = pool_size(value, progress);
            assert_eq!(result, (size, warnings), "{value:?} with {progress}");
        }
    }

    #[test]
    fn retention_zero_keeps_forever_and_large_values_clamp() {
        let cases = [
            ("", None),
            ("0", None),
            ("-5", None),
            ("30", Some(30)),
            ("36500", Some(36_500)),
            ("99999", Some(36_500)),
            ("soon", None),
        ];
        for (value, expected) in cases {
            let (days, _) = parse_setting("D", value, 0..=MAX_RETENTION_DAYS, 0);
            assert_eq!(retention(days), expected, "{value:?}");
        }
    }
}
