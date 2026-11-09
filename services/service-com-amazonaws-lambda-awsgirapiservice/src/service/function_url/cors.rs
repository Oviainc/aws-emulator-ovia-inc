// SPDX-License-Identifier: CC0-1.0

//! The CROSS-ORIGIN half of a function URL: what the stored `Cors` settings make of a
//! request to the endpoint, and what the endpoint answers a browser's PREFLIGHT.
//!
//! # The rule
//!
//! `FunctionUrlConfig$Cors` is "the cross-origin resource sharing (CORS) settings for your
//! function URL", and it is the ENDPOINT's setting rather than the function's: so the
//! endpoint — not the handler — applies it.
//!
//! * A CORS PREFLIGHT is answered by the endpoint itself, with NOTHING invoked: a preflight
//!   is the browser asking the endpoint what it permits, which is a question only the stored
//!   configuration answers. The answer states the permission as the configured collections
//!   ([`answered_preflight`]) — the admitted origin, `AllowMethods`, `AllowHeaders`,
//!   `MaxAge` and `AllowCredentials` — at HTTP 200 with no body. A preflight the
//!   configuration does not admit is refused the way this endpoint refuses any caller it
//!   does not admit ([`super::forbidden`]), because the permission it asked for is one the
//!   configuration does not grant.
//! * Every OTHER request is served exactly as a request carrying no `Origin` is — the
//!   function RUNS and its own response is answered — and the permission is ADDED to that
//!   answer for an admitted origin ([`decoration`]): the `Access-Control-Allow-Origin` the
//!   collection admitted, `ExposeHeaders` and `AllowCredentials`. A request whose origin the
//!   configuration does not admit is not refused here and carries no such header: the
//!   headers are what a BROWSER enforces with, so this endpoint states the permission and
//!   never withholds the function's answer for it.
//! * A URL configured with no `Cors` at all states no permission: nothing is added to its
//!   answers, and a preflight to it is a request like any other, which the function itself
//!   answers.
//!
//! A preflight is recognised the way the Fetch standard defines one — an `OPTIONS` request
//! carrying `Origin` and `Access-Control-Request-Method` ([`preflight_of`]) — so an
//! `OPTIONS` request a client made for its own sake is not mistaken for one.
//!
//! An origin and a method are matched EXACTLY against the collection that admits them, or
//! through the wildcard entry that collection documents ("you can grant access to all
//! origins using the wildcard character (`*`)", "the HTTP methods that are allowed when
//! calling your function URL. For example: `GET`, `POST`, `DELETE`, or the wildcard
//! character (`*`)"). A collection the configuration leaves out admits nothing: a
//! configuration that lists no origin names no origin that may reach the function URL, and
//! one that lists no method names no method a cross-origin caller may use.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_types::body::SdkBody;

use crate::ops::types::Cors;

/// The entry either collection carries to stand for every value, as both members document.
const WILDCARD: &str = "*";

/// The header a browser states the requesting origin in (RFC 6454 §7).
const ORIGIN_HEADER: &str = "origin";

/// The header a PREFLIGHT states the method it is asking about in (Fetch,
/// "CORS-preflight request").
const REQUEST_METHOD_HEADER: &str = "access-control-request-method";

/// The method a CORS preflight is made with.
const PREFLIGHT_METHOD: &str = "OPTIONS";

/// The status the endpoint answers a preflight it admits with.
const PREFLIGHT_STATUS: u16 = 200;

/// `Cors$AllowOrigins`, "the origins that can access your function URL".
const ALLOW_ORIGIN_HEADER: &str = "access-control-allow-origin";

/// `Cors$AllowMethods`, "the HTTP methods that are allowed when calling your function URL".
const ALLOW_METHODS_HEADER: &str = "access-control-allow-methods";

/// `Cors$AllowHeaders`, "the HTTP headers that origins can include in requests to your
/// function URL".
const ALLOW_HEADERS_HEADER: &str = "access-control-allow-headers";

/// `Cors$ExposeHeaders`, "the HTTP headers in your function response that you want to expose
/// to origins that call your function URL".
const EXPOSE_HEADERS_HEADER: &str = "access-control-expose-headers";

/// `Cors$AllowCredentials`, "whether to allow cookies or other credentials in requests to
/// your function URL".
const ALLOW_CREDENTIALS_HEADER: &str = "access-control-allow-credentials";

/// `Cors$MaxAge`, "the maximum amount of time, in seconds, that web browsers can cache
/// results of a preflight request".
const MAX_AGE_HEADER: &str = "access-control-max-age";

/// The one value `Access-Control-Allow-Credentials` carries when credentials are allowed
/// (Fetch, "`Access-Control-Allow-Credentials` response header"); the header is omitted
/// altogether when they are not.
const CREDENTIALS_ALLOWED: &str = "true";

/// How one collection travels as the single header value it is sent as (RFC 9110 §5.3).
const LIST_SEPARATOR: &str = ",";

/// What a PREFLIGHT asks the endpoint about: the origin it is made from, and the method the
/// browser is asking permission for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Preflight<'r> {
    /// The `Origin` the request carries.
    origin: &'r str,
    /// The method `Access-Control-Request-Method` asks about.
    method: &'r str,
}

/// The preflight `req` is, or `None` when it is not one.
///
/// A preflight is an `OPTIONS` request carrying both of the headers a browser sends on one,
/// each non-empty — the Fetch standard's own definition — so an `OPTIONS` request a client
/// made for its own sake is served like any other request and reaches the function. The
/// method is compared EXACTLY: an HTTP method is case-sensitive (RFC 9110 §9.1), so another
/// spelling of it is another method.
pub(super) fn preflight_of(req: &RtRequest) -> Option<Preflight<'_>> {
    if req.method() != PREFLIGHT_METHOD {
        return None;
    }
    let header = |name| req.headers().get(name).map(str::trim).filter(|value| !value.is_empty());
    Some(Preflight {
        origin: header(ORIGIN_HEADER)?,
        method: header(REQUEST_METHOD_HEADER)?,
    })
}

/// The endpoint's own answer to the preflight `asked`, for a URL configured with `cors`:
/// the permission the configuration grants, or the refusal of a permission it does not.
///
/// Nothing is invoked for it — see the module's own header — so the answer carries no body
/// and nothing of the function.
pub(super) fn answered_preflight(cors: &Cors, asked: &Preflight<'_>) -> RtResponse {
    let Some(origin) = admitted(cors.allow_origins.as_deref(), asked.origin) else {
        return super::forbidden();
    };
    if admitted(cors.allow_methods.as_deref(), asked.method).is_none() {
        return super::forbidden();
    }
    let mut permitted: Vec<(&'static str, String)> = vec![(ALLOW_ORIGIN_HEADER, origin)];
    if let Some(methods) = joined(cors.allow_methods.as_deref()) {
        permitted.push((ALLOW_METHODS_HEADER, methods));
    }
    if let Some(headers) = joined(cors.allow_headers.as_deref()) {
        permitted.push((ALLOW_HEADERS_HEADER, headers));
    }
    if let Some(max_age) = cors.max_age {
        permitted.push((MAX_AGE_HEADER, max_age.to_string()));
    }
    permitted.extend(credentials(cors));
    let mut response = RtResponse::new(super::plane_status(PREFLIGHT_STATUS), SdkBody::empty());
    decorate(&mut response, &permitted);
    response
}

/// The permission to ADD to whatever a URL configured with `cors` answers `req` — empty for
/// a request carrying no `Origin`, and for one whose origin the configuration does not
/// admit.
///
/// `AllowMethods` and `AllowHeaders` are not stated here and `ExposeHeaders` is: the first
/// two answer the question a PREFLIGHT asks, while "the HTTP headers in your function
/// response that you want to expose" is about the response this one decorates.
pub(super) fn decoration(cors: &Cors, req: &RtRequest) -> Vec<(&'static str, String)> {
    let Some(origin) = req
        .headers()
        .get(ORIGIN_HEADER)
        .map(str::trim)
        .filter(|origin| !origin.is_empty())
    else {
        return Vec::new();
    };
    let Some(admitted) = admitted(cors.allow_origins.as_deref(), origin) else {
        return Vec::new();
    };
    let mut permitted: Vec<(&'static str, String)> = vec![(ALLOW_ORIGIN_HEADER, admitted)];
    if let Some(exposed) = joined(cors.expose_headers.as_deref()) {
        permitted.push((EXPOSE_HEADERS_HEADER, exposed));
    }
    permitted.extend(credentials(cors));
    permitted
}

/// Add `permitted` to `response`.
///
/// Each header is SET rather than appended: the permission is the endpoint's own statement
/// of what the configuration grants, so a function that answered a header of the same name
/// does not leave the browser two values to choose between. A value no header can carry is
/// dropped rather than panicking on the request's path — a collection holding one states
/// nothing instead of failing the call — and is logged, since the configuration that holds
/// it was accepted by the control plane.
pub(super) fn decorate(response: &mut RtResponse, permitted: &[(&'static str, String)]) {
    for (name, value) in permitted {
        if response
            .headers_mut()
            .try_insert(*name, value.clone())
            .is_err()
        {
            ::tracing::info!(
                header = %name,
                "a function URL's CORS settings hold a value no response header can carry, \
                 so that permission is not stated"
            );
        }
    }
}

/// The value `Access-Control-Allow-Origin` carries for `origin` when `allowed` admits it, or
/// `None` when it does not.
///
/// An exact entry is echoed back, and the wildcard entry is answered AS the wildcard, which
/// is what it grants: "you can grant access to all origins using the wildcard character".
/// A collection the configuration left out, or left empty, admits no origin at all.
fn admitted(allowed: Option<&[String]>, value: &str) -> Option<String> {
    let allowed = allowed?;
    if allowed.iter().any(|entry| entry == value) {
        return Some(value.to_string());
    }
    allowed
        .iter()
        .any(|entry| entry == WILDCARD)
        .then(|| WILDCARD.to_string())
}

/// One collection as the single header value it is sent as, or `None` for a collection the
/// configuration left out or left empty.
fn joined(values: Option<&[String]>) -> Option<String> {
    values
        .filter(|values| !values.is_empty())
        .map(|values| values.join(LIST_SEPARATOR))
}

/// The credentials permission, stated only when the configuration allows them: the header's
/// one value is `true`, so a configuration that does not allow them states nothing rather
/// than stating `false`.
fn credentials(cors: &Cors) -> Option<(&'static str, String)> {
    (cors.allow_credentials == Some(true))
        .then(|| (ALLOW_CREDENTIALS_HEADER, CREDENTIALS_ALLOWED.to_string()))
}

#[cfg(test)]
mod tests {
    //! What the configuration makes of a request, over the collections a `Cors` can hold.
    //! The whole answer on the wire — the status, the body and the run that did or did not
    //! happen — is pinned in `tests/all/function_url_cors.rs`.

    use super::*;

    /// One row of the preflight READING: the case, the request's method, the headers a
    /// browser set on it, and whether it is a preflight.
    type PreflightRow<'r> = (&'r str, &'r str, &'r [(&'r str, &'r str)], bool);

    /// One row of the ADMISSION reading: the case, the collection, the value, and what the
    /// header carries for it.
    type AdmittedRow<'r> = (&'r str, Option<&'r [String]>, &'r str, Option<&'r str>);

    /// One row of the DECORATION: the case, the request's headers, and what the answer
    /// states.
    type DecorationRow<'r> = (&'r str, &'r [(&'r str, &'r str)], &'r [(&'r str, &'r str)]);

    /// One request to a function URL: the method, and the headers a browser sets on it.
    fn request(method: &str, headers: &[(&str, &str)]) -> RtRequest {
        let mut req = RtRequest::new(SdkBody::empty());
        req.set_method(method)
            .expect("the test request's method is a valid one");
        for (name, value) in headers {
            req.headers_mut()
                .insert((*name).to_string(), (*value).to_string());
        }
        req
    }

    /// One `Cors` holding the collections `allow_origins` and `allow_methods`.
    fn cors(allow_origins: Option<&[&str]>, allow_methods: Option<&[&str]>) -> Cors {
        let collection =
            |values: Option<&[&str]>| values.map(|v| v.iter().map(|e| (*e).to_string()).collect());
        Cors {
            allow_origins: collection(allow_origins),
            allow_methods: collection(allow_methods),
            ..Cors::default()
        }
    }

    /// RULE: a preflight is an `OPTIONS` request carrying both of the headers a browser
    /// sends on one, and nothing else is one. The rows span the method — including another
    /// spelling of it, which is another method — each header's presence, and an empty value
    /// for each.
    #[test]
    fn only_a_request_carrying_both_preflight_headers_is_one() {
        let origin = ("origin", "https://example.test");
        let asked = ("access-control-request-method", "GET");
        // (case, method, headers, is a preflight)
        let rows: &[PreflightRow<'_>] = &[
            ("a preflight", "OPTIONS", &[origin, asked], true),
            (
                "a request whose method is another spelling of OPTIONS",
                "options",
                &[origin, asked],
                false,
            ),
            ("an OPTIONS request of the caller's own", "OPTIONS", &[], false),
            ("an OPTIONS request carrying only an origin", "OPTIONS", &[origin], false),
            (
                "an OPTIONS request carrying only the asked method",
                "OPTIONS",
                &[asked],
                false,
            ),
            (
                "an OPTIONS request whose origin is empty",
                "OPTIONS",
                &[("origin", ""), asked],
                false,
            ),
            (
                "an OPTIONS request whose asked method is empty",
                "OPTIONS",
                &[origin, ("access-control-request-method", " ")],
                false,
            ),
            ("a cross-origin GET", "GET", &[origin], false),
            ("a cross-origin GET carrying both headers", "GET", &[origin, asked], false),
        ];
        for (case, method, headers, expected) in rows {
            let read = preflight_of(&request(method, headers)).is_some();
            assert_eq!(
                read, *expected,
                "case {case:?}: expected a preflight == {expected}, got {read}"
            );
        }
    }

    /// RULE: a collection admits an entry it holds exactly, and every value when it holds
    /// the wildcard — which it then answers AS the wildcard — while a collection left out or
    /// left empty admits nothing.
    #[test]
    fn a_collection_admits_its_own_entries_and_the_wildcard_admits_every_value() {
        let listed = ["https://first.example".to_string(), "https://second.example".to_string()];
        let wildcard = [WILDCARD.to_string()];
        let empty: [String; 0] = [];
        // (case, the collection, the value, what the header carries)
        let rows: &[AdmittedRow<'_>] = &[
            ("an entry the collection holds", Some(&listed), "https://first.example", Some("https://first.example")),
            ("another entry of the same collection", Some(&listed), "https://second.example", Some("https://second.example")),
            ("a value the collection does not hold", Some(&listed), "https://third.example", None),
            ("a value differing only in case", Some(&listed), "https://FIRST.example", None),
            ("any value, under the wildcard", Some(&wildcard), "https://third.example", Some(WILDCARD)),
            ("the wildcard itself", Some(&wildcard), WILDCARD, Some(WILDCARD)),
            ("an empty collection", Some(&empty), "https://first.example", None),
            ("a collection the configuration left out", None, "https://first.example", None),
        ];
        for (case, allowed, value, expected) in rows {
            let stated = admitted(*allowed, value);
            assert_eq!(
                stated.as_deref(),
                *expected,
                "case {case:?}: expected {expected:?}, got {stated:?}"
            );
        }
    }

    /// RULE: a preflight is admitted only when the configuration admits BOTH the origin it
    /// came from and the method it asks about; anything else is the refusal this endpoint
    /// gives a caller it does not admit. The rows span both collections, the wildcard in
    /// each, and each collection left out.
    #[test]
    fn a_preflight_is_admitted_only_when_both_collections_admit_it() {
        let origins: &[&str] = &["https://allowed.example"];
        let methods: &[&str] = &["GET", "POST"];
        // (case, the configuration, the origin, the asked method, the status)
        let rows: &[(&str, Cors, &str, &str, u16)] = &[
            ("both admitted", cors(Some(origins), Some(methods)), "https://allowed.example", "GET", 200),
            ("another admitted method", cors(Some(origins), Some(methods)), "https://allowed.example", "POST", 200),
            ("a method the configuration does not list", cors(Some(origins), Some(methods)), "https://allowed.example", "DELETE", 403),
            ("an origin the configuration does not list", cors(Some(origins), Some(methods)), "https://other.example", "GET", 403),
            ("every origin, under the wildcard", cors(Some(&[WILDCARD]), Some(methods)), "https://other.example", "GET", 200),
            ("every method, under the wildcard", cors(Some(origins), Some(&[WILDCARD])), "https://allowed.example", "DELETE", 200),
            ("no origin listed at all", cors(None, Some(methods)), "https://allowed.example", "GET", 403),
            ("no method listed at all", cors(Some(origins), None), "https://allowed.example", "GET", 403),
        ];
        for (case, configured, origin, method, expected) in rows {
            let answered = answered_preflight(
                configured,
                &Preflight {
                    origin,
                    method,
                },
            );
            let status = answered.status().as_u16();
            assert_eq!(
                status, *expected,
                "case {case:?}: expected HTTP {expected}, got {status}"
            );
        }
    }

    /// RULE: what the endpoint ADDS to the answer of an actual request is the admitted
    /// origin, the exposed headers and the credentials permission — and nothing at all for a
    /// request carrying no origin or one the configuration does not admit. The rows span the
    /// origin, the collections the decoration reads, and the ones it does not.
    #[test]
    fn an_actual_requests_answer_states_the_permission_for_an_admitted_origin() {
        let configured = Cors {
            allow_origins: Some(vec!["https://allowed.example".to_string()]),
            allow_methods: Some(vec!["GET".to_string()]),
            allow_headers: Some(vec!["x-asked".to_string()]),
            expose_headers: Some(vec!["x-made".to_string(), "x-also".to_string()]),
            allow_credentials: Some(true),
            max_age: Some(600),
        };
        // (case, the request's headers, what the answer states)
        let rows: &[DecorationRow<'_>] = &[
            (
                "an admitted origin",
                &[("origin", "https://allowed.example")],
                &[
                    ("access-control-allow-origin", "https://allowed.example"),
                    ("access-control-expose-headers", "x-made,x-also"),
                    ("access-control-allow-credentials", "true"),
                ],
            ),
            (
                "an origin the configuration does not admit",
                &[("origin", "https://other.example")],
                &[],
            ),
            ("no origin at all", &[], &[]),
            ("an empty origin", &[("origin", "")], &[]),
        ];
        for (case, headers, expected) in rows {
            let stated = decoration(&configured, &request("GET", headers));
            let stated: Vec<(&str, &str)> = stated
                .iter()
                .map(|(name, value)| (*name, value.as_str()))
                .collect();
            assert_eq!(
                stated.as_slice(),
                *expected,
                "case {case:?}: expected {expected:?}, got {stated:?}"
            );
        }
    }
}
