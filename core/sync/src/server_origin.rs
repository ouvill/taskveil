use std::{fmt, net::IpAddr};

use reqwest::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerOriginError;

impl fmt::Display for ServerOriginError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid server origin")
    }
}

impl std::error::Error for ServerOriginError {}

/// Validates and canonicalizes the credential issuer/resource origin.
///
/// Production credentials require HTTPS. Plain HTTP is accepted only for a
/// loopback host so local development and device simulators can reach a local
/// test server. Paths, credentials, query strings, and fragments are rejected
/// because a token set is bound to exactly one web origin.
pub fn canonical_server_origin(value: &str) -> Result<String, ServerOriginError> {
    let url = Url::parse(value.trim()).map_err(|_| ServerOriginError)?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ServerOriginError);
    }

    let host = url.host_str().ok_or(ServerOriginError)?;
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        return Err(ServerOriginError);
    }

    let origin = url.origin();
    if origin.is_tuple() {
        Ok(origin.ascii_serialization())
    } else {
        Err(ServerOriginError)
    }
}

#[cfg(test)]
mod tests {
    use super::canonical_server_origin;

    #[test]
    fn canonicalizes_https_and_loopback_origins() {
        assert_eq!(
            canonical_server_origin(" HTTPS://Example.COM:443/ ").unwrap(),
            "https://example.com"
        );
        assert_eq!(
            canonical_server_origin("http://127.0.0.1:3000/").unwrap(),
            "http://127.0.0.1:3000"
        );
        assert_eq!(
            canonical_server_origin("http://[::1]:3000").unwrap(),
            "http://[::1]:3000"
        );
    }

    #[test]
    fn rejects_credential_leak_and_ambiguous_base_urls() {
        for value in [
            "http://example.com",
            "https://user@example.com",
            "https://example.com/api",
            "https://example.com/?issuer=other",
            "https://example.com/#fragment",
            "file:///tmp/server",
        ] {
            assert!(canonical_server_origin(value).is_err(), "{value}");
        }
    }
}
