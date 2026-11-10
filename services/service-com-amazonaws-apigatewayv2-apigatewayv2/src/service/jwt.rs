// SPDX-License-Identifier: CC0-1.0

//! The JWT a JWT authorizer of an HTTP api evaluates: how a presented token is read, and
//! which of an authorizer's recorded expectations it has to meet.
//!
//! An `Authorizer` whose `AuthorizerType` is `JWT` carries a `JWTConfiguration`
//! ("Required for the JWT authorizer type. Supported only for HTTP APIs"), whose two
//! members are the whole of what the api records about the tokens it will accept:
//!
//! * `Issuer` — "The base domain of the identity provider that issues JSON Web Tokens.
//!   For example, an Amazon Cognito user pool has the following format:
//!   https://cognito-idp.{region}.amazonaws.com/{userPoolId}";
//! * `Audience` — "A list of the intended recipients of the JWT. A valid JWT must provide
//!   an aud that matches at least one entry in this list. See RFC 7519".
//!
//! and the route that fronts the authorizer carries the third: `AuthorizationScopes`, "a
//! list of authorization scopes configured on a route. The scopes are used with a JWT
//! authorizer to authorize the method invocation. The authorization works by matching the
//! route scopes against the scopes parsed from the access token in the incoming request.
//! The method invocation is authorized if any route scope matches a claimed scope in the
//! access token. Otherwise, the invocation is not authorized."
//!
//! So this module is a pure function of the presented token and those recorded values:
//! [`evaluate`] answers whether the request may proceed, and otherwise WHICH refusal it
//! earns — a token the gate cannot accept at all is the authorizer's `401`, and a valid
//! token whose scopes do not meet the route's is the `403` the scope rule above describes.
//! It reads no state and writes none, and every reading of the token is total: a value
//! that is not a token is a refusal, never a panic.
//!
//! # What is checked, and what is not
//!
//! The token's FORM is checked (a compact JWS of three segments, a JOSE header naming a
//! signing algorithm, a claim set that is a JSON object) and its CLAIMS are checked
//! against the recorded configuration. Its SIGNATURE is not verified: verifying it needs
//! the verification key of the identity provider the `Issuer` names, which is published at
//! that provider's own key-set endpoint, and this emulator makes no outbound connection to
//! fetch one — so no key exists here against which a signature could be checked. A token
//! whose header names the unsecured algorithm of RFC 7519 §6 (`alg` `none`), or whose
//! signature segment is empty, is refused rather than treated as signed: that much is
//! decidable from the token alone, and an unsecured token is one no identity provider
//! issues and no JOSE verifier accepts.

use std::collections::BTreeMap;

/// The largest presented token this module reads, in bytes.
///
/// A CHOSEN bound, not a documented one: it caps the base64 decode and the JSON parse a
/// single request can ask for, so a caller cannot make one header's worth of input into
/// unbounded work. It is far above every token a provider issues (a Cognito access token
/// is roughly one kilobyte) and far below the header budget a front door accepts.
const MAX_TOKEN_BYTES: usize = 8 * 1024;

/// The authorization-scheme name a bearer token is presented under in the `Authorization`
/// header (RFC 6750 §2.1, "Bearer"), stripped before the token is read.
///
/// `Authorizer$IdentitySource` names that header as the place a JWT is extracted from
/// ("for example $request.header.Authorization"), and RFC 6750 is how a token travels in
/// it. A source naming any other header, or a query parameter, carries the token bare, so
/// the prefix is removed only when it is there.
const BEARER_SCHEME: &str = "bearer";

/// The JOSE header parameter naming the algorithm a JWS was signed with (RFC 7515 §4.1.1,
/// the one header JWS makes mandatory).
const ALGORITHM_HEADER: &str = "alg";

/// The `alg` value of an UNSECURED JWT — "a JWS using the 'alg' value of 'none'"
/// (RFC 7519 §6.1) — which is a token with no signature at all.
const UNSECURED_ALGORITHM: &str = "none";

/// The claim naming who issued the token: RFC 7519 §4.1.1 `iss`, which
/// `JWTConfiguration$Issuer` is matched against.
const ISSUER_CLAIM: &str = "iss";

/// The claim naming the token's intended recipients: RFC 7519 §4.1.3 `aud`, "either a
/// single case-sensitive string or an array of strings", which `JWTConfiguration$Audience`
/// is matched against.
const AUDIENCE_CLAIM: &str = "aud";

/// The claim an OAuth 2.0 ACCESS token names its client with (RFC 8693 §2.1 / RFC 9068
/// §2.2, and the claim an Amazon Cognito access token carries in place of `aud`), read as
/// the audience when the token carries no `aud`.
const CLIENT_ID_CLAIM: &str = "client_id";

/// The claim after which the token must not be accepted: RFC 7519 §4.1.4 `exp`, "the
/// expiration time on or after which the JWT MUST NOT be accepted for processing".
const EXPIRY_CLAIM: &str = "exp";

/// The claim before which the token must not be accepted: RFC 7519 §4.1.5 `nbf`, "the time
/// before which the JWT MUST NOT be accepted for processing". Optional, so a token
/// carrying none is not refused for it.
const NOT_BEFORE_CLAIM: &str = "nbf";

/// The claims a token's scopes are read from: the space-delimited `scope` string OAuth 2.0
/// uses (RFC 8693 §4.2, "a JSON string containing a space-separated list of scopes"), and
/// the `scp` array providers issue in its place.
const SCOPE_CLAIMS: &[&str] = &["scope", "scp"];

/// The outcome of evaluating a presented token against an authorizer's configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// The token meets everything the api records about it: the request proceeds to the
    /// route's integration.
    Allow,
    /// The gate cannot accept the token at all — `401`. The reason is for the log, never
    /// for the caller: API Gateway answers one `{"message":"Unauthorized"}` whichever
    /// clause failed, so a client learns nothing about the configuration from it.
    Unauthorized(Unauthorized),
    /// The token is one the gate accepts, but the route requires a scope it does not
    /// claim — `403`, the "otherwise, the invocation is not authorized" of
    /// `Route$AuthorizationScopes`.
    InsufficientScope,
}

/// Why a token could not be accepted at all. One variant per clause, so the reason a
/// request was refused is diagnosable from the emulator's own log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unauthorized {
    /// No token was presented at the identity source the authorizer names.
    NoToken,
    /// The presented value is not a compact JWS carrying a JSON claim set, or is longer
    /// than [`MAX_TOKEN_BYTES`].
    Malformed,
    /// The token is unsecured — `alg` `none`, or an empty signature segment — so nothing
    /// issued it that a verifier could trust.
    Unsecured,
    /// The `iss` claim is not the issuer the authorizer records, or there is none.
    Issuer,
    /// No `aud` entry (nor `client_id`) matches an entry of the recorded `Audience` list.
    Audience,
    /// The claim set carries no usable `exp`.
    NoExpiry,
    /// `exp` is in the past.
    Expired,
    /// `nbf` is in the future.
    NotYetValid,
}

/// What an api records about the tokens one of its JWT authorizers accepts: the
/// authorizer's `JWTConfiguration`, plus the `AuthorizationScopes` of the route being
/// invoked.
#[derive(Debug, Clone, Copy)]
pub struct Expectations<'c> {
    /// `JWTConfiguration$Issuer`, the one member a JWT authorizer cannot be created
    /// without (`crate::state::validate_authorizer_config`).
    pub issuer: &'c str,
    /// `JWTConfiguration$Audience`. Empty when the configuration names none, which is a
    /// configuration the create accepts — then no audience clause applies, because the api
    /// recorded no recipient to require.
    pub audience: &'c [String],
    /// `Route$AuthorizationScopes`. Empty when the route configures none, which is "the
    /// client must provide an access token instead of an identity token" not applying:
    /// nothing is matched and nothing is required.
    pub scopes: &'c [String],
}

/// Whether `presented` meets `expected` at `now_unix_seconds`, and otherwise which refusal
/// it earns.
///
/// The clauses are applied in the order a gate can decide them: the token has to BE a
/// signed token before any claim of it is read, it has to come from the recorded issuer
/// and name a recorded audience before its validity window matters, and the route's scopes
/// are the last word — the only clause that is a `403` rather than a `401`, because by
/// then the caller is authenticated and merely not authorized for this route.
pub fn evaluate(presented: Option<&str>, expected: Expectations<'_>, now_unix_seconds: i64) -> Decision {
    let Some(presented) = presented else {
        return Decision::Unauthorized(Unauthorized::NoToken);
    };
    let claims = match read(presented) {
        Ok(claims) => claims,
        Err(refusal) => return Decision::Unauthorized(refusal),
    };
    if let Some(refusal) = claims.refusal(expected, now_unix_seconds) {
        return Decision::Unauthorized(refusal);
    }
    if expected.scopes.is_empty() || claims.claims_any_scope(expected.scopes) {
        Decision::Allow
    } else {
        Decision::InsufficientScope
    }
}

/// The claim set of a token whose FORM this module accepted: a JSON object, with nothing
/// read out of it yet.
#[derive(Debug, Clone)]
struct Claims(::serde_json::Map<String, ::serde_json::Value>);

/// Read `presented` as a compact JWS and report its claim set.
///
/// A JWT is "three base64url segments joined by `.`: a header and a claim set, each
/// base64url-encoded JSON, and a signature" — the same form this tree already reads
/// elsewhere (`service-com-amazonaws-eksauth-eksauthfrontend`'s
/// `parse_service_account_token`). The `Bearer` scheme prefix RFC 6750 puts in front of
/// the token in an `Authorization` header is removed first, when it is there.
fn read(presented: &str) -> Result<Claims, Unauthorized> {
    let token = strip_bearer(presented.trim());
    if token.is_empty() || token.len() > MAX_TOKEN_BYTES {
        return Err(Unauthorized::Malformed);
    }
    let mut segments = token.split('.');
    let (Some(header), Some(claims), Some(signature), None) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return Err(Unauthorized::Malformed);
    };
    if header.is_empty() || claims.is_empty() {
        return Err(Unauthorized::Malformed);
    }
    let header = json_segment(header).ok_or(Unauthorized::Malformed)?;
    let claims = json_segment(claims).ok_or(Unauthorized::Malformed)?;
    // Decidable from the token alone: an unsecured JWT (RFC 7519 §6) is one nothing
    // signed, so the gate refuses it rather than reading its claims. A header naming no
    // algorithm at all is not a JWS header (RFC 7515 §4.1.1).
    let algorithm = header
        .get(ALGORITHM_HEADER)
        .and_then(::serde_json::Value::as_str)
        .map(str::trim)
        .filter(|algorithm| !algorithm.is_empty())
        .ok_or(Unauthorized::Malformed)?;
    if algorithm.eq_ignore_ascii_case(UNSECURED_ALGORITHM) || signature.is_empty() {
        return Err(Unauthorized::Unsecured);
    }
    decode_segment(signature).ok_or(Unauthorized::Malformed)?;
    Ok(Claims(claims))
}

impl Claims {
    /// The refusal these claims earn against `expected` at `now`, or `None` when every
    /// clause but the route's scopes is met.
    fn refusal(&self, expected: Expectations<'_>, now: i64) -> Option<Unauthorized> {
        // "The base domain of the identity provider that issues JSON Web Tokens"
        // (`JWTConfiguration$Issuer`) is compared as a URI: a base URI with and without a
        // trailing `/` names the same provider, and nothing else about it is normalised.
        let Some(issuer) = self.string(ISSUER_CLAIM) else {
            return Some(Unauthorized::Issuer);
        };
        if trimmed_issuer(issuer) != trimmed_issuer(expected.issuer) {
            return Some(Unauthorized::Issuer);
        }
        // "A valid JWT must provide an aud that matches at least one entry in this list"
        // (`JWTConfiguration$Audience`). An ACCESS token names its client in `client_id`
        // rather than in `aud`, so that claim is read when there is no `aud` — the same
        // token a client gets from the issuer the configuration names.
        if !expected.audience.is_empty() {
            let presented = self.audiences();
            let matches = presented
                .iter()
                .any(|audience| expected.audience.iter().any(|entry| entry == audience));
            if !matches {
                return Some(Unauthorized::Audience);
            }
        }
        // "The expiration time on or after which the JWT MUST NOT be accepted for
        // processing" (RFC 7519 §4.1.4): a gate that admitted a token with no expiry would
        // admit it for ever, so a claim set carrying no usable `exp` is refused.
        let expiry = self.seconds(EXPIRY_CLAIM).ok_or(Unauthorized::NoExpiry);
        match expiry {
            Err(refusal) => return Some(refusal),
            Ok(expiry) if expiry <= now => return Some(Unauthorized::Expired),
            Ok(_) => {}
        }
        // "The time before which the JWT MUST NOT be accepted for processing"
        // (RFC 7519 §4.1.5), optional: only a token that carries one is held to it.
        if self.seconds(NOT_BEFORE_CLAIM).is_some_and(|not_before| not_before > now) {
            return Some(Unauthorized::NotYetValid);
        }
        None
    }

    /// Whether the token claims any of `required`: "the method invocation is authorized if
    /// any route scope matches a claimed scope in the access token"
    /// (`Route$AuthorizationScopes`).
    fn claims_any_scope(&self, required: &[String]) -> bool {
        let claimed = self.scopes();
        required.iter().any(|scope| claimed.contains(scope.trim()))
    }

    /// One claim as a non-blank string, or `None` when it is absent, blank, or of another
    /// JSON type.
    fn string(&self, claim: &str) -> Option<&str> {
        self.0
            .get(claim)
            .and_then(::serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }

    /// One claim as whole seconds since the epoch — the `NumericDate` of RFC 7519 §2, "a
    /// JSON numeric value representing the number of seconds from 1970-01-01T00:00:00Z
    /// UTC" — truncated toward zero, which is what comparing a seconds-resolution claim
    /// asks for. `None` for an absent claim and for a value that is no number.
    fn seconds(&self, claim: &str) -> Option<i64> {
        let value = self.0.get(claim)?;
        value
            .as_i64()
            .or_else(|| value.as_f64().map(|seconds| seconds as i64))
    }

    /// The token's audiences: `aud` as "either a single case-sensitive string or an array
    /// of strings" (RFC 7519 §4.1.3), falling back to [`CLIENT_ID_CLAIM`] when the token
    /// carries no `aud`.
    fn audiences(&self) -> Vec<&str> {
        let presented = match self.0.get(AUDIENCE_CLAIM) {
            Some(::serde_json::Value::String(single)) => vec![single.trim()],
            Some(::serde_json::Value::Array(many)) => many
                .iter()
                .filter_map(::serde_json::Value::as_str)
                .map(str::trim)
                .collect(),
            _ => Vec::new(),
        };
        let presented: Vec<&str> = presented
            .into_iter()
            .filter(|audience| !audience.is_empty())
            .collect();
        if presented.is_empty() {
            return self.string(CLIENT_ID_CLAIM).into_iter().collect();
        }
        presented
    }

    /// The scopes the token claims, from either spelling in [`SCOPE_CLAIMS`]: a
    /// space-delimited string is split on whitespace, and an array contributes its string
    /// members.
    fn scopes(&self) -> ::std::collections::BTreeSet<&str> {
        let mut scopes = ::std::collections::BTreeSet::new();
        for claim in SCOPE_CLAIMS {
            match self.0.get(*claim) {
                Some(::serde_json::Value::String(delimited)) => {
                    scopes.extend(delimited.split_whitespace());
                }
                Some(::serde_json::Value::Array(many)) => {
                    scopes.extend(
                        many.iter()
                            .filter_map(::serde_json::Value::as_str)
                            .map(str::trim)
                            .filter(|scope| !scope.is_empty()),
                    );
                }
                _ => {}
            }
        }
        scopes
    }

    /// The whole claim set as the event's claims map carries it: every claim, by name, as
    /// TEXT.
    ///
    /// The map's values are strings — a claim's value in the documented event is quoted
    /// whatever JSON type the token wrote it as — so a string claim is reported as the
    /// string it is and any other claim as the JSON text it had, which is the only
    /// rendering that loses nothing about a number, a flag or a nested value.
    fn reported(&self) -> BTreeMap<String, String> {
        self.0
            .iter()
            .map(|(claim, value)| {
                let text = match value {
                    ::serde_json::Value::String(text) => text.clone(),
                    other => other.to_string(),
                };
                (claim.clone(), text)
            })
            .collect()
    }
}

/// `presented` with the RFC 6750 `Bearer` scheme prefix removed when it carries one.
fn strip_bearer(presented: &str) -> &str {
    let Some((scheme, rest)) = presented.split_once(char::is_whitespace) else {
        return presented;
    };
    if scheme.eq_ignore_ascii_case(BEARER_SCHEME) {
        rest.trim()
    } else {
        presented
    }
}

/// `issuer` with one trailing `/` removed, so that a base URI written with and without it
/// names the same identity provider.
fn trimmed_issuer(issuer: &str) -> &str {
    issuer.trim().trim_end_matches('/')
}

/// One base64url segment of a token, read as a JSON object. `None` when the segment is not
/// base64url or does not decode to a JSON object — neither is a JOSE header or a claim set.
fn json_segment(segment: &str) -> Option<::serde_json::Map<String, ::serde_json::Value>> {
    let bytes = decode_segment(segment)?;
    match ::serde_json::from_slice::<::serde_json::Value>(&bytes) {
        Ok(::serde_json::Value::Object(members)) => Some(members),
        _ => None,
    }
}

/// Decode one segment of a compact JWS: base64url without padding (RFC 7515 §2, which is
/// RFC 4648 §5 "with all trailing '=' characters omitted"), accepting a presenter that
/// padded it anyway.
///
/// The codec is `aws_smithy_types::base64`, the standard-alphabet decoder this workspace
/// already uses for every Smithy blob, plus the two alphabet substitutions (`-` -> `+`,
/// `_` -> `/`) and the restored padding that turn §5 back into §4 — the same translation
/// the Amazon Cognito service applies when it ENCODES the segments of the tokens it mints.
/// No second codec is introduced for it.
fn decode_segment(segment: &str) -> Option<Vec<u8>> {
    let mut standard = segment.replace('-', "+").replace('_', "/");
    while !standard.len().is_multiple_of(4) {
        standard.push('=');
    }
    ::aws_smithy_types::base64::decode(&standard).ok()
}

/// The claims of a token whose evaluation the caller has already decided, for a log line:
/// the issuer it names and the audiences it names, and nothing else. Used by the data plane
/// to say WHY a refusal happened without putting a caller's token in a log.
pub fn described(presented: Option<&str>) -> BTreeMap<&'static str, String> {
    let mut described = BTreeMap::new();
    let Some(claims) = presented.and_then(|presented| read(presented).ok()) else {
        return described;
    };
    if let Some(issuer) = claims.string(ISSUER_CLAIM) {
        described.insert(ISSUER_CLAIM, issuer.to_string());
    }
    let audiences = claims.audiences();
    if !audiences.is_empty() {
        described.insert(AUDIENCE_CLAIM, audiences.join(","));
    }
    described
}

/// What the gateway reports to the integration about a token the gate ACCEPTED: the token's
/// own claim set and the scopes it claims.
///
/// A JWT authorizer's whole decision is about the caller, and the integration behind the
/// route is told who that caller is in the proxy event's `requestContext.authorizer` and
/// nowhere else — "jwt: claims, scopes", the two members the documented event carries for a
/// route a JWT authorizer admitted. So this is the authorizer's OUTPUT, beside
/// [`evaluate`]'s decision: it is read only for a request that was admitted, which is the
/// only request whose caller an integration is told about.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Accepted {
    /// Every claim of the token, by claim name, as TEXT: a claims map's values are strings
    /// in the event, so a claim that is not a JSON string is reported as its JSON text.
    pub claims: BTreeMap<String, String>,
    /// The scopes the token claims ([`Claims::scopes`]), in the claim-name order that reads
    /// them — a set, so a scope claimed under both spellings is reported once.
    pub scopes: Vec<String>,
}

/// What [`evaluate`] admitted, as the integration is told it. A value that is not a token at
/// all reports nothing, which is what a route with no JWT authorizer in front of it has to
/// report.
pub fn accepted(presented: Option<&str>) -> Accepted {
    let Some(claims) = presented.and_then(|presented| read(presented).ok()) else {
        return Accepted::default();
    };
    Accepted {
        claims: claims.reported(),
        scopes: claims
            .scopes()
            .into_iter()
            .map(str::to_string)
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    //! The clauses this module owns, over tokens a wire test cannot build as cheaply. The
    //! served behaviour — which status each decision becomes — is pinned end to end in
    //! `tests/all/http_api_jwt_data_plane.rs`.

    use super::*;

    /// `value` as one base64url segment of a compact JWS, the encoding RFC 7515 §2 states.
    fn segment(value: &::serde_json::Value) -> String {
        ::aws_smithy_types::base64::encode(value.to_string().as_bytes())
            .replace('+', "-")
            .replace('/', "_")
            .trim_end_matches('=')
            .to_string()
    }

    /// A compact JWS over `claims`, signed in the shape a provider's token has: a header
    /// naming a real algorithm and a non-empty signature segment.
    fn token(claims: ::serde_json::Value) -> String {
        signed_with(::serde_json::json!({ "alg": "ES256", "typ": "JWT" }), claims)
    }

    /// A compact JWS with the header given, so a test can vary the JOSE header.
    fn signed_with(header: ::serde_json::Value, claims: ::serde_json::Value) -> String {
        format!("{}.{}.c2lnbmF0dXJl", segment(&header), segment(&claims))
    }

    /// The issuer of a Cognito user pool, the form `JWTConfiguration$Issuer` documents.
    const ISSUER: &str = "https://cognito-idp.eu-west-1.amazonaws.com/eu-west-1_abc123";

    /// Noon of a day, as the `now` every case is evaluated at.
    const NOW: i64 = 1_767_225_600;

    fn expectations<'c>(
        issuer: &'c str,
        audience: &'c [String],
        scopes: &'c [String],
    ) -> Expectations<'c> {
        Expectations {
            issuer,
            audience,
            scopes,
        }
    }

    #[test]
    fn a_token_is_admitted_only_when_every_recorded_clause_is_met() {
        let audience = vec!["1example23456789".to_string()];
        let no_audience: Vec<String> = Vec::new();
        let no_scopes: Vec<String> = Vec::new();
        let scopes = vec!["orders/read".to_string()];

        let cases: Vec<(&str, Option<String>, Expectations<'_>, Decision)> = vec![
            (
                "the recorded case: a pool token naming the configured issuer and audience",
                Some(token(::serde_json::json!({
                    "iss": ISSUER,
                    "aud": audience[0],
                    "exp": NOW + 3600,
                    "token_use": "id"
                }))),
                expectations(ISSUER, &audience, &no_scopes),
                Decision::Allow,
            ),
            (
                "an access token, whose client_id is read as its audience",
                Some(token(::serde_json::json!({
                    "iss": ISSUER,
                    "client_id": audience[0],
                    "exp": NOW + 1,
                    "token_use": "access"
                }))),
                expectations(ISSUER, &audience, &no_scopes),
                Decision::Allow,
            ),
            (
                "an aud ARRAY, one of whose entries the configuration names",
                Some(token(::serde_json::json!({
                    "iss": ISSUER,
                    "aud": ["someone-else", audience[0]],
                    "exp": NOW + 60
                }))),
                expectations(ISSUER, &audience, &no_scopes),
                Decision::Allow,
            ),
            (
                "the issuer written with a trailing slash the configuration omits",
                Some(token(::serde_json::json!({
                    "iss": format!("{ISSUER}/"),
                    "aud": audience[0],
                    "exp": NOW + 60
                }))),
                expectations(ISSUER, &audience, &no_scopes),
                Decision::Allow,
            ),
            (
                "a configuration naming no audience, which requires none",
                Some(token(::serde_json::json!({
                    "iss": ISSUER,
                    "aud": "nobody-configured-this",
                    "exp": NOW + 60
                }))),
                expectations(ISSUER, &no_audience, &no_scopes),
                Decision::Allow,
            ),
            (
                "a foreign issuer",
                Some(token(::serde_json::json!({
                    "iss": "https://cognito-idp.eu-west-1.amazonaws.com/eu-west-1_other9",
                    "aud": audience[0],
                    "exp": NOW + 3600
                }))),
                expectations(ISSUER, &audience, &no_scopes),
                Decision::Unauthorized(Unauthorized::Issuer),
            ),
            (
                "a token carrying no issuer at all",
                Some(token(::serde_json::json!({ "aud": audience[0], "exp": NOW + 1 }))),
                expectations(ISSUER, &audience, &no_scopes),
                Decision::Unauthorized(Unauthorized::Issuer),
            ),
            (
                "a foreign audience",
                Some(token(::serde_json::json!({
                    "iss": ISSUER,
                    "aud": "9different7654321",
                    "exp": NOW + 3600
                }))),
                expectations(ISSUER, &audience, &no_scopes),
                Decision::Unauthorized(Unauthorized::Audience),
            ),
            (
                "an expired token",
                Some(token(::serde_json::json!({
                    "iss": ISSUER,
                    "aud": audience[0],
                    "exp": NOW - 1
                }))),
                expectations(ISSUER, &audience, &no_scopes),
                Decision::Unauthorized(Unauthorized::Expired),
            ),
            (
                "a token expiring exactly now: `exp` is the instant it is no longer accepted",
                Some(token(::serde_json::json!({
                    "iss": ISSUER,
                    "aud": audience[0],
                    "exp": NOW
                }))),
                expectations(ISSUER, &audience, &no_scopes),
                Decision::Unauthorized(Unauthorized::Expired),
            ),
            (
                "a token that is not valid yet",
                Some(token(::serde_json::json!({
                    "iss": ISSUER,
                    "aud": audience[0],
                    "exp": NOW + 3600,
                    "nbf": NOW + 60
                }))),
                expectations(ISSUER, &audience, &no_scopes),
                Decision::Unauthorized(Unauthorized::NotYetValid),
            ),
            (
                "a token with no expiry, which could never expire",
                Some(token(::serde_json::json!({ "iss": ISSUER, "aud": audience[0] }))),
                expectations(ISSUER, &audience, &no_scopes),
                Decision::Unauthorized(Unauthorized::NoExpiry),
            ),
            (
                "an unsecured token (RFC 7519 §6), which nothing signed",
                Some(signed_with(
                    ::serde_json::json!({ "alg": "none" }),
                    ::serde_json::json!({ "iss": ISSUER, "aud": audience[0], "exp": NOW + 60 }),
                )),
                expectations(ISSUER, &audience, &no_scopes),
                Decision::Unauthorized(Unauthorized::Unsecured),
            ),
            (
                "a token whose signature segment is empty",
                Some(format!(
                    "{}.{}.",
                    segment(&::serde_json::json!({ "alg": "ES256" })),
                    segment(&::serde_json::json!({
                        "iss": ISSUER,
                        "aud": audience[0],
                        "exp": NOW + 60
                    }))
                )),
                expectations(ISSUER, &audience, &no_scopes),
                Decision::Unauthorized(Unauthorized::Unsecured),
            ),
            (
                "a value that is not a token at all",
                Some("not-a-token".to_string()),
                expectations(ISSUER, &audience, &no_scopes),
                Decision::Unauthorized(Unauthorized::Malformed),
            ),
            (
                "a token whose claim set is not JSON",
                Some(format!(
                    "{}.bm90LWpzb24.c2ln",
                    segment(&::serde_json::json!({ "alg": "ES256" }))
                )),
                expectations(ISSUER, &audience, &no_scopes),
                Decision::Unauthorized(Unauthorized::Malformed),
            ),
            (
                "a token whose header names no algorithm",
                Some(signed_with(
                    ::serde_json::json!({ "typ": "JWT" }),
                    ::serde_json::json!({ "iss": ISSUER, "aud": audience[0], "exp": NOW + 60 }),
                )),
                expectations(ISSUER, &audience, &no_scopes),
                Decision::Unauthorized(Unauthorized::Malformed),
            ),
            (
                "a token longer than the bound this module reads",
                Some(token(::serde_json::json!({
                    "iss": ISSUER,
                    "aud": audience[0],
                    "exp": NOW + 60,
                    "padding": "x".repeat(MAX_TOKEN_BYTES)
                }))),
                expectations(ISSUER, &audience, &no_scopes),
                Decision::Unauthorized(Unauthorized::Malformed),
            ),
            (
                "no token presented at all",
                None,
                expectations(ISSUER, &audience, &no_scopes),
                Decision::Unauthorized(Unauthorized::NoToken),
            ),
            (
                "a route scope the access token claims in its space-delimited scope",
                Some(token(::serde_json::json!({
                    "iss": ISSUER,
                    "aud": audience[0],
                    "exp": NOW + 60,
                    "scope": "openid orders/read profile"
                }))),
                expectations(ISSUER, &audience, &scopes),
                Decision::Allow,
            ),
            (
                "a route scope the access token claims in an scp array",
                Some(token(::serde_json::json!({
                    "iss": ISSUER,
                    "aud": audience[0],
                    "exp": NOW + 60,
                    "scp": ["orders/read"]
                }))),
                expectations(ISSUER, &audience, &scopes),
                Decision::Allow,
            ),
            (
                "a route scope the token does not claim",
                Some(token(::serde_json::json!({
                    "iss": ISSUER,
                    "aud": audience[0],
                    "exp": NOW + 60,
                    "scope": "openid profile"
                }))),
                expectations(ISSUER, &audience, &scopes),
                Decision::InsufficientScope,
            ),
            (
                "a route requiring a scope of a token that claims none",
                Some(token(::serde_json::json!({
                    "iss": ISSUER,
                    "aud": audience[0],
                    "exp": NOW + 60
                }))),
                expectations(ISSUER, &audience, &scopes),
                Decision::InsufficientScope,
            ),
            (
                // The scope clause is reached only by a token the gate accepts: an expired
                // token is a 401 whatever its scopes say.
                "an expired token carrying the route's scope",
                Some(token(::serde_json::json!({
                    "iss": ISSUER,
                    "aud": audience[0],
                    "exp": NOW - 3600,
                    "scope": "orders/read"
                }))),
                expectations(ISSUER, &audience, &scopes),
                Decision::Unauthorized(Unauthorized::Expired),
            ),
        ];

        for (case, presented, expected, decision) in cases {
            let actual = evaluate(presented.as_deref(), expected, NOW);
            assert_eq!(
                actual, decision,
                "case {case:?}: expected {decision:?}, got {actual:?}"
            );
        }
    }

    /// The `Bearer` scheme prefix RFC 6750 puts in front of a token in the `Authorization`
    /// header is removed; a token presented bare is read as it stands.
    #[test]
    fn a_bearer_prefixed_token_is_read_as_the_token_it_carries() {
        let audience = vec!["1example23456789".to_string()];
        let no_scopes: Vec<String> = Vec::new();
        let claims = ::serde_json::json!({
            "iss": ISSUER,
            "aud": audience[0],
            "exp": NOW + 60
        });
        let bare = token(claims);
        for (case, presented) in [
            ("bare, as a query parameter carries it", bare.clone()),
            ("Bearer-prefixed, as RFC 6750 §2.1 spells it", format!("Bearer {bare}")),
            ("the scheme in another case", format!("bearer {bare}")),
            ("extra whitespace around the scheme", format!("  Bearer   {bare}  ")),
        ] {
            let actual = evaluate(
                Some(&presented),
                expectations(ISSUER, &audience, &no_scopes),
                NOW,
            );
            assert_eq!(
                actual,
                Decision::Allow,
                "case {case:?}: expected the token to be read, got {actual:?}"
            );
        }
    }

    /// What a log line may say about a refused token: the issuer and audience it names,
    /// and never the token itself.
    #[test]
    fn a_token_is_described_by_its_issuer_and_audience_only() {
        let presented = token(::serde_json::json!({
            "iss": ISSUER,
            "aud": ["a", "b"],
            "exp": NOW + 60,
            "secret": "not-described"
        }));
        let describing = described(Some(&presented));
        assert_eq!(
            describing.get("iss").map(String::as_str),
            Some(ISSUER),
            "expected the issuer to be described, got {describing:?}"
        );
        assert_eq!(
            describing.get("aud").map(String::as_str),
            Some("a,b"),
            "expected the audiences to be described, got {describing:?}"
        );
        assert_eq!(
            describing.len(),
            2,
            "expected nothing else to be described, got {describing:?}"
        );
        assert!(
            described(Some("not-a-token")).is_empty(),
            "a value that is not a token describes nothing"
        );
    }
}
