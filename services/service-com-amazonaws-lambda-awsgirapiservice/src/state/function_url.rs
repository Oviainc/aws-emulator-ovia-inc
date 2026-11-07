// SPDX-License-Identifier: CC0-1.0

//! How a request ADDRESSES a function URL, and which stored configuration it names.
//!
//! `CreateFunctionUrlConfig` hands out "a dedicated HTTP(S) endpoint that you can use to
//! invoke your function", whose form [`super::function_url`] mints:
//! `https://<url-id>.lambda-url.<region>.on.aws/`. The first label of that host names no
//! service, so the emulator's front door resolves no target from it and the request would
//! be answered `UnknownService` — for an address this service itself told the caller to
//! use. This module is the one reading of that addressing: it answers which function URL a
//! request names, out of the SAME records `CreateFunctionUrlConfig` persisted, so the URL
//! this service mints and the request it claims back cannot disagree.
//!
//! THREE addressings resolve, for the reason the shared data-plane reading states
//! ([`service_common::data_plane_addressing`]): a client on the machine this emulator runs
//! on resolves the published host nowhere, so it reaches the emulator's own endpoint and
//! carries in the PATH what it cannot carry in the host.
//!
//! * HOST-STYLE, the addressing `FunctionUrlConfig$FunctionUrl` documents: the host's
//!   first label is the url id and the whole path is the request path.
//! * LOCAL-ENDPOINT style, behind the reserved
//!   [`service_common::data_plane_addressing::EMULATOR_PATH_NAMESPACE`] segment, read on
//!   ANY host because the marker is explicit: `/_aws/lambda-url/<url-id>/<path>`, and the
//!   endpoint HOST spelling `/_aws/<url-id>.lambda-url.<region>.on.aws/<path>`.
//! * PATH-STYLE: the url id is the FIRST PATH SEGMENT and the request path is what follows
//!   it, read only on a host that names NO service (the emulator's own endpoint) or names
//!   THIS one, so a first segment that happens to equal a url id cannot take a request away
//!   from the service whose endpoint the host names.
//!
//! Every one of them resolves only to a configuration this store HOLDS, which is what keeps
//! exactly one service claiming a request: a host or a segment this store knows no function
//! URL for is claimed by nobody here.

use super::*;

/// The label every function URL host carries after its url id, as
/// [`super::function_url`] spells it: `<url-id>.lambda-url.<region>.on.aws`.
pub const FUNCTION_URL_LABEL: &str = FUNCTION_URL_HOST_SUFFIX;

/// The domain a function URL host ends in, after the region label.
const FUNCTION_URL_DOMAIN: &str = "on.aws";

/// What a request ADDRESSES, read from the two parts of it that carry an address.
///
/// Both are passed in rather than a whole request, because the framework hands
/// `Service::can_handle` the request's PARTS and the serving seams a runtime-API request:
/// each has these two things and no common type. Which of the two carries the host is the
/// SHARED data-plane reading's question, not this module's
/// ([`service_common::data_plane_addressing::addressed_host`]), so every plane reads an
/// address the same way.
#[derive(Debug, Clone)]
pub struct AddressedFunctionUrl<'r> {
    /// The host the request is addressed to, when it names one.
    pub host: Option<String>,
    /// The request's path.
    pub path: &'r str,
}

impl<'r> AddressedFunctionUrl<'r> {
    /// The reading of the request parts `Service::can_handle` is handed.
    #[must_use]
    pub fn of_parts(parts: &'r ::smithy_server_runtime::http::request::Parts) -> Self {
        Self {
            host: ::service_common::data_plane_addressing::addressed_host(
                "",
                parts
                    .headers
                    .get("host")
                    .and_then(|host| host.to_str().ok())
                    .or_else(|| {
                        parts
                            .uri
                            .authority()
                            .map(::smithy_server_runtime::http::uri::Authority::as_str)
                    }),
            ),
            path: parts.uri.path(),
        }
    }

    /// The reading of the runtime-API request the serving seams are handed, which carries
    /// its URI as a string — the absolute form of which carries the authority instead of a
    /// `Host` header, which is the case the shared reading answers.
    #[must_use]
    pub fn of_request(req: &'r ::aws_smithy_runtime_api::http::Request) -> Self {
        Self {
            host: ::service_common::data_plane_addressing::addressed_host(
                req.uri(),
                req.headers().get("host"),
            ),
            path: ::service_common::request_path(req.uri()),
        }
    }
}

/// The url id and the region of `host` when `host` is a function URL endpoint this service
/// publishes, or `None` when it is not one.
///
/// The shape is the one [`super::function_url`] mints —
/// `<url-id>.lambda-url.<region>.on.aws` — so a first label, then the
/// [`FUNCTION_URL_LABEL`], then a non-empty region label, then
/// [`FUNCTION_URL_DOMAIN`]. Any userinfo and port are stripped first, because a `Host`
/// header may carry them.
#[must_use]
pub fn function_url_host_parts(host: &str) -> Option<(&str, &str)> {
    let host = ::service_common::data_plane_addressing::host_only(host);
    let (url_id, rest) = host.split_once('.')?;
    if url_id.is_empty() {
        return None;
    }
    let (label, rest) = rest.split_once('.')?;
    if label != FUNCTION_URL_LABEL {
        return None;
    }
    let (region, domain) = rest.split_once('.')?;
    (!region.is_empty() && domain == FUNCTION_URL_DOMAIN).then_some((url_id, region))
}

/// The url id of `host` when `host` is a function URL endpoint this service publishes; see
/// [`function_url_host_parts`].
#[must_use]
pub fn function_url_id_of_host(host: &str) -> Option<&str> {
    function_url_host_parts(host).map(|(url_id, _region)| url_id)
}

/// What a request ADDRESSES, read from the address ALONE: which function URL it names and
/// the path within it.
///
/// Pure, so the cheap half of the claim happens before any store guard is taken: a request
/// that addresses no function URL at all — every control-plane call, and every other
/// service's traffic — is answered `None` here and never reaches
/// [`held_function_url`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AddressedUrl<'r> {
    /// The url id the address names.
    pub url_id: &'r str,
    /// The path within the URL.
    pub request_path: &'r str,
    /// The region the address names, for the addressings that carry one.
    pub region: Option<&'r str>,
}

/// The function URL `addressed` names, read from the address alone, or `None` when it names
/// none.
#[must_use]
pub fn addressed_url<'r>(
    addressed: &'r AddressedFunctionUrl<'r>,
) -> Option<AddressedUrl<'r>> {
    // This service's own CONTROL-plane path space is never a function URL's, on any host: a
    // request whose first segment is one of `crate::ROUTE_PATH_SEGMENTS` is a control-plane
    // call wherever it arrives, and claiming it here would take `/2015-03-31/functions`
    // away from the operations that serve it.
    if first_segment(addressed.path).is_some_and(|first| crate::ROUTE_PATH_SEGMENTS.contains(&first))
    {
        return None;
    }
    // LOCAL-ENDPOINT style first, because its marker is unambiguous on any host.
    if let Some((url_id, request_path, region)) = local_endpoint_path(addressed.path) {
        return named(url_id, request_path, region);
    }
    if let Some((url_id, region)) = addressed
        .host
        .as_deref()
        .and_then(function_url_host_parts)
    {
        return named(url_id, addressed.path, Some(region));
    }
    // PATH-STYLE, on the emulator's own endpoint or on this service's own host only: any
    // other host is another service's, and whatever its path carries is that service's to
    // answer.
    let names_other_service = addressed
        .host
        .as_deref()
        .and_then(::service_common::data_plane_addressing::service_named_by_host)
        .is_some_and(|service| service != crate::ROUTE_KEY);
    if names_other_service {
        return None;
    }
    let (url_id, request_path) = split_first_segment(addressed.path)?;
    named(url_id, request_path, None)
}

/// One addressing, once its url id is known to be non-empty — an empty first label names no
/// function URL.
fn named<'r>(
    url_id: &'r str,
    request_path: &'r str,
    region: Option<&'r str>,
) -> Option<AddressedUrl<'r>> {
    (!url_id.is_empty()).then_some(AddressedUrl {
        url_id,
        request_path,
        region,
    })
}

/// The function URL a request names: the configuration it resolved, the function and
/// qualifier that configuration is attached to, and the path WITHIN the URL.
#[derive(Debug, Clone)]
pub struct FunctionUrlTarget {
    /// The url id the request addressed, which is the configuration's own.
    pub url_id: String,
    /// The bare name of the function the URL is attached to.
    pub function_name: String,
    /// The qualifier the URL was created on: [`VERSION_LATEST`] for the unpublished
    /// version, or an alias name.
    pub qualifier: String,
    /// The `AuthType` the URL enforces, as stored.
    pub auth_type: String,
    /// The `InvokeMode` the URL serves in, as stored.
    pub invoke_mode: String,
    /// The path within the URL, which is the `rawPath` of the request the handler is given.
    pub request_path: String,
    /// The region the ADDRESS named, for the addressings that carry one.
    pub addressed_region: Option<String>,
}

/// The stored function URL `addressed` names, or `None` when this store holds none under
/// that id.
///
/// Read out of the stored [`FunctionUrlConfigRecord`]s themselves, so nothing is claimed
/// that this service cannot answer about.
#[must_use]
pub fn held_function_url(
    store: &ComAmazonawsLambdaAwsgirapiserviceState,
    addressed: &AddressedUrl<'_>,
) -> Option<FunctionUrlTarget> {
    store.functions.values().find_map(|function| {
        function
            .url_configs
            .iter()
            .find(|(_, config)| config.url_id == addressed.url_id)
            .map(|(qualifier, config)| FunctionUrlTarget {
                url_id: config.url_id.clone(),
                function_name: function.config.name.clone(),
                qualifier: qualifier.clone(),
                auth_type: config.auth_type.clone(),
                invoke_mode: config.invoke_mode.clone(),
                request_path: normalized_path(addressed.request_path),
                addressed_region: addressed.region.map(str::to_string),
            })
    })
}

/// The path a request names WITHIN the URL: the one it carried, and `/` for a request that
/// carried none at all — the path a function URL's own root is addressed by.
fn normalized_path(path: &str) -> String {
    if path.is_empty() {
        return "/".to_string();
    }
    path.to_string()
}

/// The url id, request path and region of a request that names the endpoint INSIDE ITS
/// PATH, behind the reserved namespace marker, or `None` for a path carrying no marker.
///
/// Both spellings of the endpoint behind the marker resolve — the endpoint LABEL followed by
/// the url id (`/_aws/lambda-url/<url-id>/…`) and the endpoint HOST itself
/// (`/_aws/<url-id>.lambda-url.<region>.on.aws/…`) — because what a rewriter has to carry
/// across is the host, and those are the two ways to write it. Only the second carries the
/// region.
fn local_endpoint_path(path: &str) -> Option<(&str, &str, Option<&str>)> {
    let (marker, rest) = split_first_segment(path)?;
    if marker != ::service_common::data_plane_addressing::EMULATOR_PATH_NAMESPACE {
        return None;
    }
    let (endpoint, rest) = split_first_segment(rest)?;
    if let Some((url_id, region)) = function_url_host_parts(endpoint) {
        return Some((url_id, rest, Some(region)));
    }
    (endpoint == FUNCTION_URL_LABEL)
        .then(|| split_first_segment(rest))
        .flatten()
        .map(|(url_id, rest)| (url_id, rest, None))
}

/// The first path segment and the path that follows it, with the leading `/` kept on the
/// remainder so it stays a path, through the shared data-plane reading.
fn split_first_segment(path: &str) -> Option<(&str, &str)> {
    ::service_common::data_plane_addressing::first_segment(path)
}

/// The first path segment alone, for the control-plane path-space test.
fn first_segment(path: &str) -> Option<&str> {
    split_first_segment(path).map(|(first, _)| first)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A host reads as a function URL endpoint exactly when it is shaped like the one this
    /// service mints. The rows span what the reading generalises over: the minted form, a
    /// host carrying a port and userinfo, the region label, and every way the shape can
    /// fail.
    #[test]
    fn a_function_url_host_names_its_url_id() {
        let minted = function_url(DEFAULT_REGION, "0123456789abcdef0123456789abcdef");
        let host = minted
            .strip_prefix("https://")
            .and_then(|url| url.split('/').next())
            .expect("the minted URL carries a host");
        let cases: &[(&str, Option<&str>)] = &[
            (host, Some("0123456789abcdef0123456789abcdef")),
            ("abc.lambda-url.us-west-2.on.aws", Some("abc")),
            ("abc.lambda-url.us-west-2.on.aws:443", Some("abc")),
            ("user@abc.lambda-url.eu-west-3.on.aws", Some("abc")),
            // Not a function URL endpoint at all.
            ("abc.lambda-urlx.us-west-2.on.aws", None),
            ("lambda-url.us-west-2.on.aws", None),
            ("abc.lambda-url..on.aws", None),
            ("abc.lambda-url.us-west-2.amazonaws.com", None),
            ("lambda.us-east-1.amazonaws.com", None),
            ("127.0.0.1:9297", None),
        ];
        for (host, expected) in cases {
            let read = function_url_id_of_host(host);
            assert_eq!(
                read, *expected,
                "case {host:?}: expected {expected:?}, got {read:?}"
            );
        }
    }
}
