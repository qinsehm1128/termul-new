//! Shared credential-free source handshake. Callers must use `gateway_http_client`.
use std::{fmt, time::Duration};

use reqwest::{Client, Url};

use crate::settings::{
    GatewayIdentity, Source, GATEWAY_IDENTITY_PATH, GATEWAY_PROTOCOL, GATEWAY_PROTOCOL_VERSION,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityError {
    InvalidOrigin,
    Unavailable,
    Rejected,
    InvalidIdentity,
    SourceMismatch,
}

impl fmt::Display for IdentityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidOrigin => "MCP_ORIGIN_INVALID",
            Self::Unavailable => "MCP_GATEWAY_UNAVAILABLE",
            Self::Rejected => "MCP_IDENTITY_REJECTED",
            Self::InvalidIdentity => "MCP_IDENTITY_INVALID",
            Self::SourceMismatch => "MCP_SOURCE_MISMATCH",
        })
    }
}

impl std::error::Error for IdentityError {}

/// Never use an environment proxy or follow redirects with gateway credentials.
pub fn gateway_http_client() -> Result<Client, IdentityError> {
    Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(2))
        .build()
        .map_err(|_| IdentityError::Rejected)
}

/// A canonical HTTP origin with an IP-literal loopback host, not a DNS alias.
pub fn validate_gateway_origin(origin: &Url) -> Result<(), IdentityError> {
    let loopback = origin
        .host_str()
        .and_then(|host| {
            host.trim_start_matches('[')
                .trim_end_matches(']')
                .parse::<std::net::IpAddr>()
                .ok()
        })
        .is_some_and(|ip| ip.is_loopback());
    if origin.scheme() != "http"
        || !loopback
        || origin.port_or_known_default() == Some(0)
        || !origin.username().is_empty()
        || origin.password().is_some()
        || origin.path() != "/"
        || origin.query().is_some()
        || origin.fragment().is_some()
    {
        return Err(IdentityError::InvalidOrigin);
    }
    Ok(())
}

/// Verify before attaching any bearer token. Non-success/malformed/old listeners
/// are terminal errors, not offline signals. Only connection refusal permits start.
/// `client` must be created by `gateway_http_client` without default credentials.
pub async fn verify_gateway_identity(
    client: &Client,
    origin: &Url,
    expected: Source,
) -> Result<GatewayIdentity, IdentityError> {
    validate_gateway_origin(origin)?;
    let url = origin
        .join(GATEWAY_IDENTITY_PATH)
        .map_err(|_| IdentityError::InvalidOrigin)?;
    let response = client
        .get(url)
        .timeout(Duration::from_secs(2))
        .send()
        .await
        .map_err(|error| {
            if error.is_connect() {
                IdentityError::Unavailable
            } else {
                IdentityError::Rejected
            }
        })?;
    if !response.status().is_success()
        || response.url().origin() != origin.origin()
        || response.url().path() != GATEWAY_IDENTITY_PATH
        || response.url().query().is_some()
        || response.url().fragment().is_some()
    {
        return Err(IdentityError::Rejected);
    }
    let identity: GatewayIdentity = response
        .json()
        .await
        .map_err(|_| IdentityError::InvalidIdentity)?;
    if identity.protocol != GATEWAY_PROTOCOL
        || identity.protocol_version != GATEWAY_PROTOCOL_VERSION
    {
        return Err(IdentityError::InvalidIdentity);
    }
    if identity.source != expected {
        return Err(IdentityError::SourceMismatch);
    }
    Ok(identity)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_only_literal_loopback_http_origins() {
        for origin in ["http://127.0.0.1:3290/", "http://[::1]:3390/"] {
            assert!(validate_gateway_origin(&Url::parse(origin).unwrap()).is_ok());
        }
        for origin in [
            "http://localhost:3290/",
            "https://127.0.0.1:3290/",
            "http://192.0.2.1:3290/",
            "http://127.0.0.1:0/",
            "http://127.0.0.1:3290/mcp",
            "http://127.0.0.1:3290/?secret",
            "http://127.0.0.1:3290/#secret",
            "http://user:secret@127.0.0.1:3290/",
        ] {
            assert_eq!(
                validate_gateway_origin(&Url::parse(origin).unwrap()),
                Err(IdentityError::InvalidOrigin)
            );
        }
    }
}
