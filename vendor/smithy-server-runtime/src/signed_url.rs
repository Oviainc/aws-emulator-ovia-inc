// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//! Signed URLs a handler hands out that lead back to this emulator.
//!
//! Some operations return a URL instead of the data, such as where to download a result or
//! where to upload a file. Hand one out with
//! [`OperationContext::url_signer`](crate::OperationContext::url_signer): it SigV4
//! query-signs the request, so any client that fetches the URL (an SDK, `curl`, `urllib`)
//! reaches this emulator, not the real service.
//!
//! A signed URL names the emulator's base URL; [`crate::endpoint`] says where that points
//! when the emulator is served and when it is not, and how to fetch an unserved one.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use emulator_server::request_authentication::{
    presign_url, presigned_payload_header, PresignTarget,
};

use crate::endpoint::{UrlBase, UNSERVED_URL_BASE};

/// The longest a signed URL may stay valid: seven days, as AWS allows.
pub const MAX_SIGNED_URL_EXPIRY: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// The region a URL is signed for when the request being served names none.
const DEFAULT_REGION: &str = "us-east-1";

/// One request a later caller will send to a signed URL.
///
/// Name the target the way the caller addresses it: the signing name of the service the URL
/// belongs to, and the path DECODED, laid out as the service's model lays it out. The signer
/// encodes the path the way that service's verifier reads it back, so a path segment with
/// spaces, `+` or `%` survives.
#[derive(Debug, Clone)]
pub struct SignedUrlRequest<'a> {
    method: http::Method,
    signing_name: &'a str,
    region: Option<&'a str>,
    path: &'a str,
    query: Vec<(&'a str, &'a str)>,
    headers: Vec<(&'a str, &'a str)>,
}

impl<'a> SignedUrlRequest<'a> {
    /// A `method` request to `path` (decoded, starting with `/`) on the service signed as
    /// `signing_name`.
    pub fn new(method: http::Method, signing_name: &'a str, path: &'a str) -> Self {
        Self {
            method,
            signing_name,
            region: None,
            path,
            query: Vec::new(),
            headers: Vec::new(),
        }
    }

    /// A query field the URL carries, decoded, such as a subresource or a response override.
    pub fn query(mut self, name: &'a str, value: &'a str) -> Self {
        self.query.push((name, value));
        self
    }

    /// A header the caller MUST send with exactly this value, bound into the signature, such
    /// as the `content-type` of an upload. [`SignedUrl::required_headers`] lists it.
    pub fn header(mut self, name: &'a str, value: &'a str) -> Self {
        self.headers.push((name, value));
        self
    }

    /// The region to sign for, such as `eu-west-1`; defaults to the region of the request
    /// being served. A region that is not lowercase letters, digits and hyphens (at most 32)
    /// makes [`UrlSigner::sign`] refuse the request.
    pub fn region(mut self, region: &'a str) -> Self {
        self.region = Some(region);
        self
    }
}

/// SigV4 query-signs URLs that address this emulator.
///
/// Get one from [`OperationContext::url_signer`](crate::OperationContext::url_signer). It is
/// cheap to clone, so code without a context (state, a background task) can be handed one.
/// It signs for the emulator's base URL at the time of each [`sign`](Self::sign): the data
/// port when the emulator is served, [`UNSERVED_URL_BASE`] when it is not (see
/// [`crate::endpoint`] for fetching that in-process).
///
/// Sign every URL that must lead back to this emulator here. A URL signed by hand, or by an
/// SDK's presigning against a real service endpoint, reaches the real service instead.
///
/// # Example
///
/// An operation that returns where to download a resource:
///
/// ```ignore
/// use smithy_server_runtime::SignedUrlRequest;
/// use std::time::Duration;
///
/// let path = format!("/{container}/{name}"); // decoded, as the model lays it out
/// let request = SignedUrlRequest::new(http::Method::GET, "<signing-name>", &path);
/// let url = ctx.url_signer().sign(request, Duration::from_secs(600))?;
/// output.download_url = Some(url.as_str().to_string());
/// ```
#[derive(Clone)]
pub struct UrlSigner {
    base: Arc<UrlBase>,
    default_region: String,
    key_id: String,
    secret: String,
}

impl UrlSigner {
    pub(crate) fn new(router: &crate::MultiServiceRouter, default_region: String) -> Self {
        let credentials = router.vended_credentials();
        let (key_id, secret) = emulator_server::embedded::signing_key_pair(&credentials);
        Self {
            base: router.url_base.clone(),
            default_region,
            key_id: key_id.to_string(),
            secret: secret.to_string(),
        }
    }

    /// SigV4 query-presigns `request` for the emulator's base URL, valid for `expires_in`
    /// (whole seconds, at least one and at most [`MAX_SIGNED_URL_EXPIRY`]).
    ///
    /// With authentication on the URL is signed with the emulator's own key; with it off,
    /// with the placeholder keys any signature passes with. A request that sends a body is
    /// signed so that any body verifies; send it with every header
    /// [`SignedUrl::required_headers`] lists.
    ///
    /// # Errors
    ///
    /// [`SignError::ExpiryOutOfRange`] for an expiry outside that range, and
    /// [`SignError::Unsignable`] for a request that cannot be signed.
    pub fn sign(
        &self,
        request: SignedUrlRequest<'_>,
        expires_in: Duration,
    ) -> Result<SignedUrl, SignError> {
        let expires_secs = expires_in.as_secs();
        if expires_secs == 0 || expires_in > MAX_SIGNED_URL_EXPIRY {
            return Err(SignError::ExpiryOutOfRange);
        }
        let headers_valid = request.headers.iter().all(|(name, value)| {
            http::HeaderName::from_bytes(name.as_bytes()).is_ok()
                && http::HeaderValue::from_str(value).is_ok()
        });
        let text_valid = !has_control(request.path)
            && request
                .query
                .iter()
                .all(|(name, value)| !has_control(name) && !has_control(value));
        let region_valid = request.region.is_none_or(is_region);
        if !headers_valid
            || !text_valid
            || !region_valid
            || request.signing_name.is_empty()
            || !request.path.starts_with('/')
        {
            return Err(SignError::Unsignable);
        }
        let endpoint = self.base.get();
        let signed_at = whole_seconds_now();
        let target = PresignTarget {
            endpoint: endpoint.as_str(),
            method: request.method.as_str(),
            signing_name: request.signing_name,
            region: request.region.unwrap_or(&self.default_region),
            path: request.path,
            query: &request.query,
            headers: &request.headers,
        };
        let url = presign_url(&target, &self.key_id, &self.secret, signed_at, expires_secs)
            .ok_or(SignError::Unsignable)?;
        let payload_header =
            presigned_payload_header(request.signing_name, request.method.as_str());
        let headers = request.headers.iter().copied().chain(payload_header);
        let headers = headers.map(|(name, value)| (name.to_string(), value.to_string()));
        Ok(SignedUrl {
            url,
            expires_at: signed_at + Duration::from_secs(expires_secs),
            required_headers: headers.collect(),
        })
    }
}

/// Prints the key id but never the secret.
impl std::fmt::Debug for UrlSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UrlSigner")
            .field("default_region", &self.default_region)
            .field("key_id", &self.key_id)
            .finish_non_exhaustive()
    }
}

/// Whether `text` holds an ASCII control character (U+0000 to U+001F, or U+007F), which no
/// signed path or query may carry.
fn has_control(text: &str) -> bool {
    text.chars().any(|c| c.is_ascii_control())
}

/// Whether `region` has the shape of an AWS region name: 1 to 32 lowercase letters, digits
/// and hyphens.
fn is_region(region: &str) -> bool {
    (1..=32).contains(&region.len())
        && region
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Now, to the second: SigV4 dates carry no fraction, so the expiry is computed from the same
/// instant the verifier reads back.
fn whole_seconds_now() -> SystemTime {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    UNIX_EPOCH + Duration::from_secs(since_epoch.as_secs())
}

/// A URL this emulator signed, when it stops working, and the headers a caller must send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedUrl {
    url: String,
    expires_at: SystemTime,
    required_headers: Vec<(String, String)>,
}

impl SignedUrl {
    /// The URL text, for the output member that carries it.
    pub fn as_str(&self) -> &str {
        &self.url
    }

    /// The instant the URL stops verifying, for an output that reports the expiry as well.
    pub fn expires_at(&self) -> SystemTime {
        self.expires_at
    }

    /// The headers a caller must send with exactly these values for the URL to verify: each
    /// [`SignedUrlRequest::header`], and for a request that sends a body to a service outside
    /// the S3 family, `x-amz-content-sha256: UNSIGNED-PAYLOAD`. Return them to the caller with
    /// the URL, as an output member that lists required headers, or in the operation's docs.
    pub fn required_headers(&self) -> impl Iterator<Item = (&str, &str)> {
        self.required_headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
    }

    /// A distinct, well-formed signed URL for a test that needs one but never fetches it.
    #[must_use]
    pub fn test_value() -> Self {
        Self {
            url: format!("{UNSERVED_URL_BASE}/test-value/{}", uuid::Uuid::new_v4()),
            expires_at: whole_seconds_now() + Duration::from_secs(600),
            required_headers: Vec::new(),
        }
    }
}

/// Why a URL could not be signed.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignError {
    /// `expires_in` was under one second or over [`MAX_SIGNED_URL_EXPIRY`].
    ExpiryOutOfRange,
    /// The request cannot be signed: an empty signing name, a path that does not start with
    /// `/`, a control character in the path or query, a region that is not a region name, or
    /// a header that is not valid HTTP.
    Unsignable,
}

impl std::fmt::Display for SignError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::ExpiryOutOfRange => "a signed URL must expire after 1 second to 7 days",
            Self::Unsignable => "the request cannot be signed",
        })
    }
}

impl std::error::Error for SignError {}

/// The region `parts` was signed for, from its `Authorization` header or presigned query. A
/// region that is not a region name counts as absent.
pub(crate) fn signed_region(parts: &http::request::Parts) -> String {
    let from_header = emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|s| s.region)
        .filter(|r| is_region(r));
    let from_query = || {
        let credential = parts.uri.query()?.split('&').find_map(|item| {
            let (name, value) = item.split_once('=')?;
            (name == "X-Amz-Credential").then(|| crate::percent_decode(value))
        })?;
        credential
            .split('/')
            .nth(2)
            .filter(|r| is_region(r))
            .map(str::to_string)
    };
    from_header
        .or_else(from_query)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EndpointUrl, MultiServiceRouter};
    use emulator_server::EmulatorService;

    fn signer(router: &MultiServiceRouter) -> UrlSigner {
        UrlSigner::new(router, DEFAULT_REGION.to_string())
    }

    fn object(path: &str) -> SignedUrlRequest<'_> {
        SignedUrlRequest::new(http::Method::GET, "s3", path)
    }

    #[test]
    fn an_unserved_router_signs_for_the_unserved_base() {
        let router = MultiServiceRouter::new();
        let url = signer(&router)
            .sign(object("/b/k"), Duration::from_secs(60))
            .unwrap();
        assert!(
            url.as_str()
                .starts_with(&format!("{UNSERVED_URL_BASE}/b/k?")),
            "{}",
            url.as_str()
        );
        assert!(url
            .as_str()
            .contains("X-Amz-Credential=EMULATORPLACEHOLDER%2F"));
        assert_eq!(router.url_base.get().as_str(), UNSERVED_URL_BASE);
    }

    #[test]
    fn serving_at_moves_every_clone_and_existing_signer_to_the_data_port() {
        let router = MultiServiceRouter::new();
        let made_before = signer(&router);
        router.clone().serving_at("https://127.0.0.1:9297");
        for url in [
            made_before.sign(object("/b/k"), Duration::from_secs(60)),
            signer(&router.clone()).sign(object("/b/k"), Duration::from_secs(60)),
        ] {
            let url = url.unwrap();
            assert!(
                url.as_str().starts_with("https://127.0.0.1:9297/b/k?"),
                "{}",
                url.as_str()
            );
        }
    }

    #[test]
    fn a_url_base_set_on_the_router_is_signed_for() {
        let mut router = MultiServiceRouter::new();
        router.set_url_base(EndpointUrl::parse("https://emulator.test:8443/").unwrap());
        let url = signer(&router)
            .sign(object("/b/k"), Duration::from_secs(60))
            .unwrap();
        assert!(url.as_str().starts_with("https://emulator.test:8443/b/k?"));
        assert_eq!(router.url_base.get().as_str(), "https://emulator.test:8443");
    }

    #[test]
    fn expiry_is_whole_seconds_between_one_second_and_seven_days() {
        let router = MultiServiceRouter::new();
        let signer = signer(&router);
        for out_of_range in [
            Duration::ZERO,
            Duration::from_millis(999),
            MAX_SIGNED_URL_EXPIRY + Duration::from_secs(1),
        ] {
            assert_eq!(
                signer.sign(object("/b/k"), out_of_range),
                Err(SignError::ExpiryOutOfRange)
            );
        }
        let before = SystemTime::now();
        let url = signer.sign(object("/b/k"), MAX_SIGNED_URL_EXPIRY).unwrap();
        let after = SystemTime::now() + MAX_SIGNED_URL_EXPIRY;
        assert!(
            url.expires_at() <= after
                && url.expires_at() + Duration::from_secs(1) >= before + MAX_SIGNED_URL_EXPIRY
        );
        assert!(url.as_str().contains("X-Amz-Expires=604800"));
    }

    #[test]
    fn a_request_that_cannot_be_signed_is_refused() {
        let router = MultiServiceRouter::new();
        let signer = signer(&router);
        let minute = Duration::from_secs(60);
        assert_eq!(
            signer.sign(object("b/k"), minute),
            Err(SignError::Unsignable)
        );
        let unnamed = SignedUrlRequest::new(http::Method::GET, "", "/b/k");
        assert_eq!(signer.sign(unnamed, minute), Err(SignError::Unsignable));
        let bad_header = object("/b/k").header("content-type", "a\nb");
        assert_eq!(signer.sign(bad_header, minute), Err(SignError::Unsignable));
    }

    #[test]
    fn a_control_character_in_the_path_or_query_is_refused() {
        let router = MultiServiceRouter::new();
        let signer = signer(&router);
        let minute = Duration::from_secs(60);
        for control in ["\0", "\n", "\r", "\t", "\x1f", "\x7f"] {
            let path = format!("/b/k{control}x");
            let text = format!("v{control}x");
            for request in [
                object(&path),
                object("/b/k").query(&text, "v"),
                object("/b/k").query("n", &text),
            ] {
                assert_eq!(
                    signer.sign(request, minute),
                    Err(SignError::Unsignable),
                    "{control:?}"
                );
            }
        }
        let printable = object("/b/a b+c%d/é").query("n x+%", "v é/");
        assert!(signer.sign(printable, minute).is_ok());
    }

    #[test]
    fn only_a_region_name_is_signed_for() {
        let router = MultiServiceRouter::new();
        let signer = signer(&router);
        let minute = Duration::from_secs(60);
        let long = "a".repeat(33);
        for bad in ["", "EU-WEST-1", "eu_west_1", "eu-west-1/x", "us&x=y", &long] {
            let request = object("/b/k").region(bad);
            assert_eq!(
                signer.sign(request, minute),
                Err(SignError::Unsignable),
                "{bad:?}"
            );
        }
        let good = signer.sign(object("/b/k").region("eu-west-3"), minute);
        assert!(good.unwrap().as_str().contains("%2Feu-west-3%2Fs3%2F"));

        let parts = |uri: &str| {
            let request = http::Request::builder().uri(uri).body(()).unwrap();
            request.into_parts().0
        };
        for bad in [
            "EU-WEST-1",
            "eu-west-1%26x%3Dy",
            "eu%0Awest",
            "eu-west-1%20",
        ] {
            let uri = format!("/b/k?X-Amz-Credential=AKID%2F20240101%2F{bad}%2Fs3%2Faws4_request");
            assert_eq!(signed_region(&parts(&uri)), DEFAULT_REGION, "{bad}");
        }
        let header = http::Request::builder()
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKID/20240101/EU_WEST/svc/aws4_request, \
                 SignedHeaders=host, Signature=00",
            )
            .body(())
            .unwrap();
        assert_eq!(signed_region(&header.into_parts().0), DEFAULT_REGION);
    }

    #[test]
    fn required_headers_list_the_bound_ones_and_the_payload_marker_where_needed() {
        let router = MultiServiceRouter::new();
        let minute = Duration::from_secs(60);
        let put = |name| {
            let request = SignedUrlRequest::new(http::Method::PUT, name, "/r")
                .header("content-type", "text/plain");
            let url = signer(&router).sign(request, minute).unwrap();
            url.required_headers()
                .map(|(n, v)| format!("{n}: {v}"))
                .collect::<Vec<_>>()
        };
        assert_eq!(put("s3"), ["content-type: text/plain"]);
        assert_eq!(
            put("other"),
            [
                "content-type: text/plain",
                "x-amz-content-sha256: UNSIGNED-PAYLOAD"
            ]
        );
        let get = signer(&router).sign(object("/b/k"), minute).unwrap();
        assert_eq!(get.required_headers().count(), 0);
    }

    #[test]
    fn the_region_defaults_to_the_one_the_request_was_signed_for() {
        let parts = |builder: http::request::Builder| builder.body(()).unwrap().into_parts().0;
        let header = http::Request::builder().header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKID/20240101/eu-west-3/svc/aws4_request, \
             SignedHeaders=host, Signature=00",
        );
        assert_eq!(signed_region(&parts(header)), "eu-west-3");
        let query = http::Request::builder()
            .uri("/b/k?X-Amz-Credential=AKID%2F20240101%2Fap-south-1%2Fs3%2Faws4_request");
        assert_eq!(signed_region(&parts(query)), "ap-south-1");
        assert_eq!(
            signed_region(&parts(http::Request::builder())),
            DEFAULT_REGION
        );
    }

    #[test]
    fn signed_url_test_values_are_distinct() {
        assert_ne!(SignedUrl::test_value(), SignedUrl::test_value());
    }
}
