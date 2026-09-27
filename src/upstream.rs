use std::env::VarError;
use std::net::{IpAddr, ToSocketAddrs};
use thiserror::Error;
use url::Url;

#[derive(Debug, Error)]
pub enum UpstreamError {
    #[error("environment variable ZEDPROXY_UPSTREAM_URL is not set")]
    MissingUpstreamUrl(#[from] VarError),
    #[error("invalid ZEDPROXY_UPSTREAM_URL: {0}")]
    InvalidUrl(#[from] url::ParseError),
    #[error("unsupported scheme in ZEDPROXY_UPSTREAM_URL: {0}")]
    UnsupportedScheme(String),
    #[error("ZEDPROXY_UPSTREAM_URL has no host")]
    MissingHost,
    #[error("ZEDPROXY_UPSTREAM_URL has no port")]
    MissingPort,
    #[error("failed to resolve ZEDPROXY_UPSTREAM_URL host: {0}")]
    Resolve(String),
}

pub struct Upstream {
    /// Resolved peer IP, e.g. 1.2.3.4. Resolved once at startup.
    pub socket_addr: IpAddr,
    /// Upstream port (explicit in the URL or scheme default: 80/443).
    pub port: u16,
    /// TLS SNI and upstream Host header value, e.g. "provider.example".
    pub sni: String,
    /// Derived from the URL scheme.
    pub tls: bool,
    /// Endpoint match pattern, e.g. "/v1/chat/completions".
    pub pattern: String,
}

impl Upstream {
    pub fn from_env(url: &str) -> Result<Self, UpstreamError> {
        Self::parse(url)
    }

    pub fn parse(raw: &str) -> Result<Self, UpstreamError> {
        let url = Url::parse(raw)?;
        let tls = match url.scheme() {
            "https" => true,
            "http" => false,
            other => return Err(UpstreamError::UnsupportedScheme(other.to_string())),
        };
        let host = url
            .host_str()
            .ok_or(UpstreamError::MissingHost)?
            .to_string();
        let port = url
            .port_or_known_default() // 80 for http, 443 for https
            .ok_or(UpstreamError::MissingPort)?;

        // Resolve hostname once at startup. HttpPeer stores SocketAddr (IP-only by
        // type), so passing a hostname would force a blocking resolve per request;
        // resolving here removes it entirely. If the host is already an IP literal,
        // this parses directly with no DNS traffic.
        let addr_str = format!("{host}:{port}");
        let socket_addr = addr_str
            .to_socket_addrs() // blocking; runs once, before the server starts
            .map_err(|e| UpstreamError::Resolve(e.to_string()))?
            .next()
            .ok_or_else(|| UpstreamError::Resolve("empty resolve result".to_string()))?;

        // Path part is ONLY for matching (approved): URL path + /chat/completions.
        // A URL without a path ("/") yields "/chat/completions".
        let base = url.path().trim_end_matches('/');
        let pattern = format!("{base}/chat/completions");

        Ok(Upstream {
            socket_addr: socket_addr.ip(),
            port,
            sni: host,
            tls,
            pattern,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_with_path_and_default_port() {
        let u = Upstream::parse("https://1.2.3.4/v1").unwrap();
        assert_eq!(u.socket_addr.to_string(), "1.2.3.4");
        assert_eq!(u.port, 443);
        assert!(u.tls);
        assert_eq!(u.sni, "1.2.3.4");
        assert_eq!(u.pattern, "/v1/chat/completions");
    }

    #[test]
    fn http_with_explicit_port_and_path() {
        let u = Upstream::parse("http://127.0.0.1:8080/api").unwrap();
        assert_eq!(u.socket_addr.to_string(), "127.0.0.1");
        assert_eq!(u.port, 8080);
        assert!(!u.tls);
        assert_eq!(u.pattern, "/api/chat/completions");
    }

    #[test]
    fn url_without_path_yields_root_pattern() {
        let u = Upstream::parse("https://1.2.3.4").unwrap();
        assert_eq!(u.pattern, "/chat/completions");
        assert_eq!(u.port, 443);
    }

    #[test]
    fn trailing_slash_path_is_normalized() {
        let u = Upstream::parse("https://1.2.3.4/v1/").unwrap();
        assert_eq!(u.pattern, "/v1/chat/completions");
    }

    #[test]
    fn malformed_url_is_rejected() {
        assert!(matches!(
            Upstream::parse("not-a-url"),
            Err(UpstreamError::InvalidUrl(_))
        ));
    }

    #[test]
    fn unsupported_scheme_is_rejected() {
        assert!(matches!(
            Upstream::parse("ftp://1.2.3.4/v1"),
            Err(UpstreamError::UnsupportedScheme(s)) if s == "ftp"
        ));
    }
}
