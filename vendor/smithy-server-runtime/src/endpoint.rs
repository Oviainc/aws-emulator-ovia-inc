// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//! The emulator's own base URL, which every URL a handler hands out names.
//!
//! An output member that carries a resource's address, for a caller to send normally signed
//! requests to, starts from [`OperationContext::endpoint`](crate::OperationContext::endpoint):
//! the base URL, unsigned, with no expiry. A URL a caller fetches later, signed, comes from
//! [`OperationContext::url_signer`](crate::OperationContext::url_signer), which signs for the
//! same base.
//!
//! # Where the URLs point
//!
//! - **Served** (the emulator binary): the data port, at the URL the published shared config
//!   names as `endpoint_url`. The router learns it from `EmulatorService::serving_at` when the
//!   data port starts. A client trusts it through the published `ca.pem`.
//! - **Not served** (embedded in a host's process, or a test): [`UNSERVED_URL_BASE`], which
//!   no network client can reach, unless the host names another with
//!   [`MultiServiceRouter::set_url_base`](crate::MultiServiceRouter::set_url_base). Fetch such
//!   a URL in-process: pass it as the absolute URI of a request to
//!   `EmbeddedEmulator::http_connector().call(..)`. The router routes it by its
//!   `X-Amz-Credential`, and with authentication on verifies its signature, signed host and
//!   expiry like any other request.
//!
//! No URL opens a port: the router names wherever it is already reachable.

use std::sync::RwLock;

/// The base URL of an emulator that is not served on a port: embedded in a host's process, or
/// built by a test.
///
/// `.invalid` is reserved by RFC 6761 and never resolves, so a network client that is handed
/// such a URL fails at name lookup instead of reaching some other local process. Fetch it
/// in-process through `EmbeddedEmulator::http_connector().call(..)` instead.
pub const UNSERVED_URL_BASE: &str = "https://emulator.invalid";

/// The base URL a later request addresses, for an output member that carries a resource's
/// address: `scheme://host[:port]`, with no path, no signature and no expiry.
///
/// Get it from [`OperationContext::endpoint`](crate::OperationContext::endpoint). It is the
/// data port's published URL when the emulator is served, and [`UNSERVED_URL_BASE`] when it
/// is not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointUrl {
    url: String,
}

impl EndpointUrl {
    /// `base` as an endpoint, when it is an absolute `https` URL with a host and nothing after
    /// it (a trailing `/` is dropped); `None` otherwise, including for any `http` URL.
    pub fn parse(base: &str) -> Option<Self> {
        let base = base.strip_suffix('/').unwrap_or(base);
        let uri: http::Uri = base.parse().ok()?;
        // `http::Uri` drops a fragment without an error, so refuse one here.
        let https = uri.scheme_str() == Some("https") && !base.contains('#');
        let bare = uri
            .path_and_query()
            .is_none_or(|pq| pq.as_str().is_empty() || pq == "/");
        (https && uri.host().is_some_and(|host| !host.is_empty()) && bare).then(|| Self {
            url: base.to_string(),
        })
    }

    /// The URL text, `scheme://host[:port]`. Append the resource's own path to it.
    pub fn as_str(&self) -> &str {
        &self.url
    }

    /// A distinct, well-formed endpoint for a test that needs one but never dials it.
    #[must_use]
    pub fn test_value() -> Self {
        Self {
            url: format!("https://test-value-{}.invalid", uuid::Uuid::new_v4()),
        }
    }
}

/// A router's base URL, shared by all its clones and by every
/// [`UrlSigner`](crate::UrlSigner) it made: [`UNSERVED_URL_BASE`] until the router is served
/// or given another, then the data port's URL.
#[derive(Debug)]
pub(crate) struct UrlBase {
    current: RwLock<EndpointUrl>,
}

impl Default for UrlBase {
    fn default() -> Self {
        Self {
            current: RwLock::new(EndpointUrl {
                url: UNSERVED_URL_BASE.to_string(),
            }),
        }
    }
}

impl UrlBase {
    pub(crate) fn get(&self) -> EndpointUrl {
        self.current
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub(crate) fn set(&self, base: EndpointUrl) {
        *self.current.write().unwrap_or_else(|e| e.into_inner()) = base;
    }

    /// Records the URL the data port is published at. The core passes its own `https://`
    /// URL; one that does not parse leaves the base as it was.
    pub(crate) fn serving_at(&self, endpoint_url: &str) {
        let parsed = EndpointUrl::parse(endpoint_url);
        debug_assert!(parsed.is_some(), "unusable endpoint URL {endpoint_url:?}");
        if let Some(base) = parsed {
            self.set(base);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_bare_https_url_parses_as_an_endpoint() {
        for good in [
            "https://emulator.invalid",
            "https://[::1]:80",
            "https://h:1/",
        ] {
            assert!(EndpointUrl::parse(good).is_some(), "{good}");
        }
        for bad in [
            "",
            "emulator.invalid",
            "http://h",
            "http://127.0.0.1:9297",
            "http://[::1]:80/",
            "ftp://h",
            "https://h/p",
            "https://h?q",
            "https://h#f",
            "/p",
        ] {
            assert_eq!(EndpointUrl::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn endpoint_test_values_are_distinct_and_parse() {
        assert_ne!(EndpointUrl::test_value(), EndpointUrl::test_value());
        assert!(EndpointUrl::parse(EndpointUrl::test_value().as_str()).is_some());
    }
}
