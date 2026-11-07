use std::net::SocketAddr;
use std::time::Duration;

pub const DEFAULT_ADDR: &str = "127.0.0.1:8787";
pub const DEFAULT_MAX_BODY: usize = 10 * 1024 * 1024;
pub const DEFAULT_MAX_CONCURRENCY: usize = 8;
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct Config {
    addr: SocketAddr,
    max_body: usize,
    max_concurrency: usize,
    timeout: Duration,
}

impl Config {
    pub fn new(
        addr: SocketAddr,
        max_body: usize,
        max_concurrency: usize,
        timeout: Duration,
    ) -> Self {
        Self {
            addr,
            max_body: max_body.max(1),
            max_concurrency: max_concurrency.max(1),
            timeout,
        }
    }

    pub fn from_env() -> Result<Self, String> {
        let raw_addr = std::env::var("RTK_WEB_ADDR").unwrap_or_else(|_| DEFAULT_ADDR.to_string());
        let addr = raw_addr
            .parse()
            .map_err(|err| format!("invalid RTK_WEB_ADDR `{raw_addr}`: {err}"))?;
        let max_body = parse_env_usize("RTK_WEB_MAX_BODY", DEFAULT_MAX_BODY)?;
        let max_concurrency = parse_env_usize("RTK_WEB_MAX_CONCURRENCY", DEFAULT_MAX_CONCURRENCY)?;
        let timeout_ms =
            parse_env_usize("RTK_WEB_TIMEOUT_MS", DEFAULT_TIMEOUT.as_millis() as usize)?;
        Ok(Self::new(
            addr,
            max_body,
            max_concurrency,
            Duration::from_millis(timeout_ms.max(1) as u64),
        ))
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn max_body(&self) -> usize {
        self.max_body
    }

    pub fn max_concurrency(&self) -> usize {
        self.max_concurrency
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }
}

fn parse_env_usize(key: &str, default: usize) -> Result<usize, String> {
    match std::env::var(key) {
        Ok(raw) => raw
            .parse()
            .map_err(|err| format!("invalid {key} `{raw}`: {err}")),
        Err(_) => Ok(default),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_clamps_zero_values() {
        let config = Config::new(
            "127.0.0.1:1".parse().expect("addr"),
            0,
            0,
            Duration::from_millis(0),
        );
        assert_eq!(config.max_body(), 1);
        assert_eq!(config.max_concurrency(), 1);
    }
}