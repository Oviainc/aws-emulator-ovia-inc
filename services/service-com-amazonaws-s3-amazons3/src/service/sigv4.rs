// SPDX-License-Identifier: CC0-1.0

//! How this service words a request SigV4 authentication refused, and the signing
//! diagnostics its signature refusal carries.
//!
//! The framework verifies every signature and hands this service one STRUCTURAL refusal for
//! every way that can fail — "authentication refused the caller" — so which refusal it is,
//! and therefore the status, the error code, the sentence and the elements S3 answers with,
//! follow from what the REQUEST presented. That is read once per request
//! ([`remember`], called from this service's routing seam, which the framework reaches
//! before it authenticates) and answered once ([`refusal_for`]).
//!
//! Five conditions, each with its own answer:
//!
//! 1. The validity window a SigV4 QUERY-authenticated (presigned) URL asks for is outside
//!    the range query-string authentication accepts — `X-Amz-Expires` must be a
//!    non-negative count of seconds no greater than [`MAX_PRESIGNED_EXPIRES_SECS`] ("the
//!    maximum expiration time ... is one week", AWS General Reference, *Authenticating
//!    requests: using query parameters*; Amazon S3 API Reference, *Authenticating requests:
//!    using query parameters (AWS Signature Version 4)*). The URL is then not an
//!    authorization failure at all but a query string the service cannot use, which S3
//!    answers with HTTP 400 and the error code [`QUERY_PARAMETERS_CODE`] — a non-retryable
//!    "fix the URL", decided BEFORE anything about the credential or the signature, so a
//!    URL outside the range is refused the same way whether or not its signature would
//!    verify.
//! 2. The window is inside that range but has PASSED. The signature is intact and the
//!    request is refused for the EXPIRY, which S3 states as such.
//! 3. The instant the request says it was signed at is outside the window S3 accepts around
//!    its own clock ([`MAX_REQUEST_SKEW`]). A signature is only as good as the moment it
//!    covers, so a request stamped too far from now is refused for its TIME, whatever its
//!    signature computes to — [`CLOCK_SKEW_CODE`], with the request's own instant and the
//!    service's beside the window, which is what tells a caller whose clock has drifted that
//!    the clock is what to fix.
//! 4. The credential resolves — the key id names one this emulator ISSUED — so the caller
//!    is known and what failed is the signature computed under that credential's secret.
//!    S3's answer to that one carries, beside the code and the sentence, the SIGNING
//!    DIAGNOSTICS: the access key id the request named, the string S3 signed, the canonical
//!    request it built it from, each also as the bytes it hashed, and the signature the
//!    request itself presented. They are the only way a caller (or a human debugging a
//!    presigner) can tell WHICH canonical request the service signed from a wrong secret
//!    key, so they are what makes a signing bug diagnosable at all.
//! 5. The request presented NO credential at all — the ANONYMOUS caller, which is a caller
//!    S3 serves ("if you grant READ access to the anonymous user, the `GetObject` operation
//!    returns the object without using an authorization header", `GetObject`'s own
//!    documentation) rather than one that failed to sign. Nothing was signed, so there is no
//!    signature to diagnose and no credential to name, and WHICH authorization refusal it is
//!    follows from the bucket the request addresses: the code is the `AccessDenied` of S3's
//!    published error list either way, and the sentence is the one that bucket's own
//!    configuration words for an unauthenticated caller
//!    ([`crate::state::Bucket::requester_pays_anonymous_refusal`] carries that rule, and is
//!    the same rule an operation's own authorization answers with once a request reaches a
//!    handler).
//!
//! Every one of them is derived from the request as received. Nothing here reads a secret:
//! the signing key is never named, the signature the service COMPUTED is never answered
//! (only the one the request carried, which its sender already holds), and nothing is
//! written anywhere. Every value copied out of a request passes through [`bounded`], and the
//! signed-header set through [`MAX_SIGNED_HEADERS`], so neither the work one request can ask
//! for nor the document it is answered with grows with the head it sent; the derivation
//! itself runs only on the refusal path ([`Diagnostics::of`]).
//!
//! A refusal this module declines to word ([`refusal_for`] answering `None`) keeps the
//! framework's own, which is what leaves a credential that resolves to nothing — refused
//! for the credential, not for the signature or the window — to the boundary that already
//! words it.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::time::{Duration, SystemTime};

use aws_smithy_runtime_api::http::Request;
use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use aws_smithy_schema::{Schema, ShapeId, ShapeType};

use super::wire;

/// The signing algorithm a SigV4 request names, in its `Authorization` header and in the
/// `X-Amz-Algorithm` parameter of a presigned URL. A query string that names anything else
/// is not a SigV4 query-authenticated request, so its `X-Amz-*` fields are not read here.
const SIGV4_ALGORITHM: &str = "AWS4-HMAC-SHA256";

/// The header a header-signed request carries its signing instant in.
const X_AMZ_DATE: &str = "x-amz-date";

/// The header a request declares its payload hash — or a MARKER standing in for one — in.
const X_AMZ_CONTENT_SHA256: &str = "x-amz-content-sha256";

/// The marker a presigned S3 URL signs instead of a payload hash: the body is not covered
/// by the signature, because the URL is built before there is one.
const UNSIGNED_PAYLOAD: &str = "UNSIGNED-PAYLOAD";

/// The one header every SigV4 request signs, whose value is the host the request was
/// addressed to rather than a header this emulator can read back from a rewritten request.
const HOST_HEADER: &str = "host";

/// The query parameter a presigned URL carries its signature in — the one field of the
/// query that the signature cannot cover, and so the one left out of the canonical request.
const SIGNATURE_PARAMETER: &str = "X-Amz-Signature";

/// The error code AWS answers a request whose access key id DOES resolve but whose
/// signature does not verify under that credential's secret: the credential was found, so
/// the caller is known; what failed is the signature.
const SIGNATURE_MISMATCH_CODE: &str = "SignatureDoesNotMatch";

/// How the SigV4 signing layer words that refusal — the fixed sentence it produces for this
/// one condition, which tells the caller to look at the key and the signing method rather
/// than at the request.
const SIGNATURE_MISMATCH_MESSAGE: &str =
    "The request signature we calculated does not match the signature you provided. Check \
     your key and signing method.";

/// The error code AWS answers a SigV4 QUERY-authenticated request whose authentication
/// parameters are themselves unusable, rather than a caller it declined to authorize. A
/// client branches on it: the request cannot be retried as it stands, the URL has to be
/// rebuilt.
const QUERY_PARAMETERS_CODE: &str = "AuthorizationQueryParametersError";

/// The status that refusal carries: a malformed query string is a BAD REQUEST, not an
/// authorization failure.
const QUERY_PARAMETERS_STATUS: u16 = 400;

/// The status every other authentication refusal carries, which is the one the framework
/// renders them with too.
const UNAUTHENTICATED_STATUS: u16 = 403;

/// The sentence AWS answers a presigned URL asking for a validity window longer than
/// [`MAX_PRESIGNED_EXPIRES_SECS`]. Fixed text in full, the ceiling included: it states the
/// limit, and interpolates nothing from the request — the over-limit value the URL asked
/// for is not echoed.
const EXPIRES_OVER_CEILING_MESSAGE: &str =
    "X-Amz-Expires must be less than a week (in seconds); that is, the given X-Amz-Expires \
     must be less than 604800 seconds";

/// The sentence AWS answers a presigned URL asking for a NEGATIVE validity window — the
/// other end of the same range. Fixed text, for the same reason.
const EXPIRES_NEGATIVE_MESSAGE: &str = "X-Amz-Expires must be non-negative";

/// The error code AWS answers a request whose signing instant is outside the window it
/// accepts around its own clock (Amazon S3 "Error responses", list of error codes:
/// "RequestTimeTooSkewed — The difference between the request time and the current time is
/// too large", HTTP 403).
const CLOCK_SKEW_CODE: &str = "RequestTimeTooSkewed";

/// How AWS words that one. Fixed text; the instants and the window it carries travel beside
/// it as the elements S3's own document holds them in ([`Skew`]).
const CLOCK_SKEW_MESSAGE: &str =
    "The difference between the request time and the current time is too large.";

/// How far from this clock a request's signing instant may be, in either direction: fifteen
/// minutes ("the difference between the request time and the current time cannot be more
/// than 15 minutes", Amazon S3 API Reference, *Signing and authenticating REST requests*).
const MAX_REQUEST_SKEW: Duration = Duration::from_secs(15 * 60);

/// The error code AWS answers a presigned URL whose validity window has passed: the
/// signature is intact and the caller resolved, so what refused it is the window.
const REQUEST_EXPIRED_CODE: &str = "AccessDenied";

/// How AWS words that one. Fixed text: it names neither the window nor the instant.
const REQUEST_EXPIRED_MESSAGE: &str = "Request has expired";

/// The longest validity SigV4 query-string authentication accepts: seven days ("the maximum
/// expiration time ... is one week", AWS General Reference, *Authenticating requests: using
/// query parameters*). A URL asking for more is refused for its query string, not for its
/// caller.
const MAX_PRESIGNED_EXPIRES_SECS: u64 = 7 * 24 * 60 * 60;

/// The most signed-header entries one canonical request is built from.
///
/// A bound, not a fidelity choice: the signed-header list is caller text, and the canonical
/// headers block copies one header VALUE per entry of it, so without a cap the work a single
/// request can ask for is the product of two things it chooses. SigV4's canonical headers
/// are a UNIQUE, SORTED set, so entries past this are a list no signer writes; the set below
/// is deduplicated as well, which is what keeps a repeated name from copying its value
/// twice. A signer names a handful (an SDK's object write signs about a dozen).
const MAX_SIGNED_HEADERS: usize = 32;

/// The most characters of caller text one diagnostic element carries.
///
/// The elements are a DIAGNOSTIC of the request's own head, and a bounded prefix of each
/// diagnoses the same signing bug as the whole of it, so every value copied out of the
/// request passes through [`bounded`] on its way in and the rendered document is a bounded
/// size whatever the request that provoked it was. They are derived from one another AFTER
/// that cut, so what the document states stays internally consistent: the hash the string to
/// sign carries is the hash of the canonical request beside it.
const MAX_DIAGNOSTIC_CHARS: usize = 8 * 1024;

/// `text` cut to at most [`MAX_DIAGNOSTIC_CHARS`] characters, on a character boundary so the
/// result is still the text it is a prefix of.
fn bounded(text: impl Into<String>) -> String {
    let text = text.into();
    match text.char_indices().nth(MAX_DIAGNOSTIC_CHARS) {
        Some((cut, _)) => text[..cut].to_string(),
        None => text,
    }
}

thread_local! {
    /// What the request this service is serving presented to authentication, for the length
    /// of that one request.
    ///
    /// Not state: a request-scoped note, needed because the request and the REFUSAL are
    /// handed to this service through different seams — the refusal is handed only the
    /// structural "authentication refused the caller", which says nothing about WHICH
    /// condition refused it, and no request at all. The framework calls both within one
    /// synchronous stretch of the one dispatch, on the thread that dispatch is running on,
    /// so the note a refusal reads is the one that dispatch's own routing seam recorded.
    static PRESENTED: RefCell<Option<Presented>> = const { RefCell::new(None) };
}

/// Record what `request` presented as the request this thread is serving, `addressed_bucket`
/// naming the bucket its address resolves to (`None` for a request that addresses none).
pub(super) fn remember(request: &Request, addressed_bucket: Option<&str>) {
    let presented = Presented::of(request, addressed_bucket);
    PRESENTED.with(|note| {
        *note.borrow_mut() = Some(presented);
    });
}

/// The note [`remember`] recorded for the request this thread is serving, TAKEN: the note is
/// request-scoped, so reading it is what ends it. A later refusal on this thread that was
/// not preceded by this service's own routing of a request then reads nothing at all rather
/// than the previous request's note.
pub(super) fn presented() -> Option<Presented> {
    PRESENTED.with(|note| note.borrow_mut().take())
}

/// What a refusal needs to know about the request that was refused.
pub(super) struct Presented {
    /// The access key id it named, however that travelled: the `Credential` field of an
    /// `Authorization` header, or the `X-Amz-Credential` parameter of a presigned URL —
    /// "`X-Amz-Credential`: … `<your-access-key-id>/<date>/<AWS Region>/<AWS
    /// service>/aws4_request`" (AWS General Reference, *Authenticating requests: using query
    /// parameters*) — the scope's first element in either form.
    access_key_id: String,
    /// The validity window a SigV4 QUERY-authenticated request asked for, exactly as its
    /// `X-Amz-Expires` carried it; `None` for every request that is not one, which is why
    /// the range below is checked for a presigned URL alone.
    stated_expires: Option<String>,
    /// The instant the request says it was signed at (`X-Amz-Date`), when it names one this
    /// calendar can read.
    signed_at: Option<SystemTime>,
    /// What the signature was computed over, when the request carried a SigV4 signature at
    /// all.
    signing: Option<Signing>,
    /// Whether the request presented a credential AT ALL — in its `Authorization` header or
    /// in the query string of a presigned URL. `false` is the ANONYMOUS caller, which is
    /// refused for what the bucket grants rather than for anything about a signature.
    carries_credential: bool,
    /// The bucket the request's address names, when it names one: what the refusal of an
    /// ANONYMOUS caller is worded from, because which refusal that is turns on the bucket's
    /// own configuration rather than on anything the request signed.
    addressed_bucket: Option<String>,
}

impl Presented {
    /// Read `request` for everything a refusal of it may have to state, `addressed_bucket`
    /// naming the bucket its address resolves to.
    fn of(request: &Request, addressed_bucket: Option<&str>) -> Self {
        let headers = request.headers();
        let query = wire::request_query(request.uri());
        let authorization = headers.get(http::header::AUTHORIZATION.as_str());
        // A SigV4 field travels in the `Authorization` header of a signed request and in the
        // `X-Amz-`-prefixed query parameter of a presigned one. Both are read, so the one
        // reader below answers for either form.
        let field = |name: &str| match authorization {
            Some(header) => authorization_field(header, name),
            None => query_parameter(query, &format!("X-Amz-{name}")),
        };
        let presigned =
            authorization.is_none() && field("Algorithm").as_deref() == Some(SIGV4_ALGORITHM);
        let scope = field("Credential").unwrap_or_default();
        let amz_date = match authorization {
            Some(_) => headers.get(X_AMZ_DATE).unwrap_or_default().to_string(),
            None => field("Date").unwrap_or_default(),
        };
        let signed_headers = field("SignedHeaders").unwrap_or_default();
        let signature = field("Signature").unwrap_or_default();
        let carries_signature =
            !scope.is_empty() && !signed_headers.is_empty() && !signature.is_empty();
        let carries_credential = authorization.is_some()
            || crate::state::CREDENTIAL_QUERY_PARAMETERS
                .iter()
                .any(|parameter| query_parameter(query, parameter).is_some());
        Self {
            access_key_id: bounded(
                scope
                    .split('/')
                    .next()
                    .filter(|key_id| !key_id.is_empty())
                    .unwrap_or_default(),
            ),
            stated_expires: presigned.then(|| field("Expires")).flatten().map(bounded),
            signed_at: amz_instant(&amz_date),
            signing: carries_signature.then(|| Signing {
                method: bounded(request.method()),
                path: bounded(wire::request_path(request.uri())),
                query: bounded(query),
                headers: signed_header_values(request, &signed_headers),
                payload_hash: payload_hash(request, presigned),
                signed_headers: bounded(signed_headers),
                scope: bounded(scope),
                amz_date: bounded(amz_date),
                signature: bounded(signature),
            }),
            carries_credential,
            addressed_bucket: addressed_bucket.map(bounded),
        }
    }

    /// Whether this is a presigned request whose validity window has already PASSED — its
    /// signing instant plus the seconds it asked for are behind the clock. A request that is
    /// not presigned, or that asked for a window outside the accepted range (answered ahead
    /// of this one), or whose window cannot be read, is not an expired one.
    fn window_passed(&self) -> bool {
        let stated = self.stated_expires.as_deref().unwrap_or_default().trim();
        let (Some(signed_at), Ok(expires_secs)) = (self.signed_at, stated.parse::<u64>()) else {
            return false;
        };
        signed_at
            .checked_add(Duration::from_secs(expires_secs))
            .is_none_or(|valid_until| valid_until < SystemTime::now())
    }

    /// How far outside the accepted window around `now` this request's signing instant is, or
    /// `None` for one inside it.
    ///
    /// Judged only for a request that SIGNED an instant and carries a credential: a request
    /// with no stamp states no time to compare, and an anonymous one presented no signature
    /// for a stamp to cover. A PRESIGNED URL is not judged here either — it carries its own
    /// validity window ([`Self::window_passed`]), which is what decides how long it stays
    /// usable, so a URL signed hours ago and valid for a week is not a skewed request.
    fn skew_outside_window(&self, now: SystemTime) -> Option<Skew> {
        if !self.carries_credential || self.stated_expires.is_some() {
            return None;
        }
        let signed_at = self.signed_at?;
        let difference = match now.duration_since(signed_at) {
            Ok(behind) => behind,
            Err(ahead) => ahead.duration(),
        };
        (difference > MAX_REQUEST_SKEW).then(|| Skew::of(signed_at, now))
    }
}

/// The instants S3's clock-skew refusal states, each rendered the way its own element carries
/// it: the request's own time, this service's time, and the window between them that is
/// allowed.
struct Skew {
    /// `<RequestTime>` — the instant the request signed, in the SigV4 stamp form it signed it
    /// in.
    request_time: String,
    /// `<ServerTime>` — this service's clock when it judged the request, in ISO 8601.
    server_time: String,
    /// `<MaxAllowedSkewMilliseconds>` — [`MAX_REQUEST_SKEW`] in the unit the element names.
    max_allowed_skew_milliseconds: String,
}

impl Skew {
    /// The elements for a request signed at `signed_at` and judged at `now`. Both instants
    /// come from the request and from this service's own clock; nothing here is a stored or
    /// recorded value.
    fn of(signed_at: SystemTime, now: SystemTime) -> Self {
        Self {
            request_time: amz_stamp(signed_at),
            server_time: rendered(now, ::aws_smithy_types::date_time::Format::DateTime),
            max_allowed_skew_milliseconds: MAX_REQUEST_SKEW.as_millis().to_string(),
        }
    }
}

/// `instant` in `format`, or the empty string for an instant no calendar can render — which
/// leaves the element empty rather than failing the refusal that carries it.
fn rendered(instant: SystemTime, format: ::aws_smithy_types::date_time::Format) -> String {
    ::aws_smithy_types::DateTime::from(instant)
        .fmt(format)
        .unwrap_or_default()
}

/// `instant` as a SigV4 stamp, `YYYYMMDDTHHMMSSZ` — the form a request writes its own signing
/// instant in, which is the form the refusal states it back in. Rendered from the instant
/// through the library's calendar ([`rendered`]) with the separators SigV4 omits removed,
/// rather than echoed from the header text.
fn amz_stamp(instant: SystemTime) -> String {
    rendered(instant, ::aws_smithy_types::date_time::Format::DateTime).replace(['-', ':'], "")
}

/// What the request's signature was computed over: the canonical request's own six lines,
/// kept as the request carried them so the refusal can state the canonical request S3 built
/// rather than one rebuilt from a response.
struct Signing {
    /// The request method, the canonical request's first line.
    method: String,
    /// The request's URI path, which the canonical URI is encoded from.
    path: String,
    /// The request's query string, which the canonical query string is built from.
    query: String,
    /// The value of every header the request SIGNED, as the unique, sorted set SigV4's
    /// canonical headers are, at most [`MAX_SIGNED_HEADERS`] of them.
    headers: Vec<(String, String)>,
    /// The signed-header list exactly as the request named it.
    signed_headers: String,
    /// The payload hash the signature covered, or the marker that stood in for one.
    payload_hash: String,
    /// The credential scope the request named, `<key id>/<date>/<region>/<name>/aws4_request`.
    scope: String,
    /// The signing instant the request named.
    amz_date: String,
    /// The signature the request PRESENTED, unchanged.
    signature: String,
}

impl Signing {
    /// The canonical request SigV4 is computed over: the method, the canonical URI, the
    /// canonical query string, the canonical headers, the signed-header list and the payload
    /// hash, one per line — the form the specification fixes (AWS General Reference, *Create
    /// a canonical request*).
    fn canonical_request(&self) -> String {
        format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            self.method,
            canonical_uri(&self.path),
            canonical_query(&self.query),
            canonical_headers(&self.headers),
            self.signed_headers,
            self.payload_hash
        )
    }

    /// The string the signature is taken of: the algorithm, the signing instant, the
    /// credential scope the request named with its key id dropped, and the hash of
    /// `canonical_request` (AWS General Reference, *Create a string to sign*).
    fn string_to_sign(&self, canonical_request: &str) -> String {
        let scope: Vec<&str> = self.scope.split('/').skip(1).collect();
        format!(
            "{SIGV4_ALGORITHM}\n{}\n{}\n{}",
            self.amz_date,
            scope.join("/"),
            sha256_hex(canonical_request.as_bytes())
        )
    }
}

/// The value of each header the request SIGNED, as the UNIQUE, SORTED set SigV4's canonical
/// headers are — at most [`MAX_SIGNED_HEADERS`] of them, each value [`bounded`].
///
/// `host` is answered from the host the request was ADDRESSED to rather than from a header
/// alone, because an in-process request carries it in its URI authority instead.
fn signed_header_values(request: &Request, signed_headers: &str) -> Vec<(String, String)> {
    let names: BTreeSet<String> = signed_headers
        .split(';')
        .map(|name| name.trim().to_ascii_lowercase())
        .filter(|name| !name.is_empty())
        .take(MAX_SIGNED_HEADERS)
        .collect();
    names
        .into_iter()
        .map(|name| {
            let value = match name.as_str() {
                HOST_HEADER => bounded(super::request_host_of(request).unwrap_or_default()),
                _ => bounded(request.headers().get(name.as_str()).unwrap_or_default()),
            };
            (name, value)
        })
        .collect()
}

/// The payload hash the request's signature covered.
///
/// A request that DECLARES one signs the value it declared — "`x-amz-content-sha256`: …
/// Include a hash of the payload" for a signed S3 request, or the `UNSIGNED-PAYLOAD` /
/// `STREAMING-…` marker that stands in for one — so that is what the canonical request
/// states, whether or not the body that arrived hashes to it (a body that does not is its
/// own refusal, not this one). A presigned S3 URL signs [`UNSIGNED_PAYLOAD`], because it is
/// built before there is a body. Only a request that declared nothing has its body hashed
/// here, which is also the only case the signer hashes one for.
fn payload_hash(request: &Request, presigned: bool) -> String {
    match request.headers().get(X_AMZ_CONTENT_SHA256) {
        Some(declared) => bounded(declared),
        None if presigned => UNSIGNED_PAYLOAD.to_string(),
        None => sha256_hex(request.body().bytes().unwrap_or_default()),
    }
}

/// The canonical form of a request's URI path: percent-encoded ONCE over the unreserved
/// characters, with `/` left as a separator, and never path-normalized — the encoding S3's
/// own endpoint rules select (`disableDoubleEncoding`). So a path arrives already encoded and
/// is encoded once, not twice: a key whose name carries an encoded separator canonicalises as
/// the path it names.
///
/// This is the SAME canonicalisation the emulator's own signature verification performs
/// before it signs a request again (the S3 branch of its signable path, which decodes the
/// wire path and encodes it once over this set), which is what makes the canonical request
/// this module reports the one the compared signature was taken over rather than a second
/// reading of it.
fn canonical_uri(path: &str) -> String {
    if path.is_empty() {
        return "/".to_string();
    }
    let decoded: Vec<u8> = ::percent_encoding::percent_decode_str(path).collect();
    ::percent_encoding::percent_encode(&decoded, PATH_ENCODE_SET).to_string()
}

/// SigV4's `UriEncode` keeps only the unreserved characters, plus `/` in a path.
const PATH_ENCODE_SET: &::percent_encoding::AsciiSet = &::percent_encoding::NON_ALPHANUMERIC
    .remove(b'/')
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// The same set for a query parameter's name and value, where `/` is encoded like any other
/// reserved character.
const QUERY_ENCODE_SET: &::percent_encoding::AsciiSet = &::percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// The canonical form of a request's query string: every parameter but the signature itself,
/// each name and value percent-encoded, sorted by name and then by value, and joined with
/// `&` — a parameter with no value keeping its `=` (AWS General Reference, *Create a
/// canonical request*).
fn canonical_query(query: &str) -> String {
    let mut pairs: Vec<(String, String)> = query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            (percent_decoded(name), percent_decoded(value))
        })
        .filter(|(name, _)| name != SIGNATURE_PARAMETER)
        .map(|(name, value)| (query_encoded(&name), query_encoded(&value)))
        .collect();
    pairs.sort();
    pairs
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<String>>()
        .join("&")
}

/// The canonical form of the headers a request signed: `<lowercase name>:<value>` per line,
/// sorted by name, each value trimmed with its inner runs of whitespace collapsed to one
/// space. At most [`MAX_SIGNED_HEADERS`] lines, which is what [`signed_header_values`]
/// bounds the set to.
fn canonical_headers(headers: &[(String, String)]) -> String {
    let mut lines: Vec<String> = headers
        .iter()
        .map(|(name, value)| format!("{name}:{}\n", collapsed(value)))
        .collect();
    lines.sort();
    lines.concat()
}

/// `value` with its leading and trailing whitespace removed and every inner run of it
/// reduced to a single space, as SigV4 states a signed header's value.
fn collapsed(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<&str>>()
        .join(" ")
}

/// `text` percent-decoded — a signed query carries encoded separators inside its values.
///
/// `pub(super)`: the wire layer decodes the path and the query of the address a request
/// names through this one reader ([`super::wire::addressed_object`]), rather than keeping a
/// second copy of the decode.
pub(super) fn percent_decoded(text: &str) -> String {
    ::percent_encoding::percent_decode_str(text)
        .decode_utf8_lossy()
        .into_owned()
}

/// `text` percent-encoded the way SigV4 encodes a query parameter.
fn query_encoded(text: &str) -> String {
    ::percent_encoding::percent_encode(text.as_bytes(), QUERY_ENCODE_SET).to_string()
}

/// The hex SHA-256 of `bytes`, which is how SigV4 writes every hash it carries.
fn sha256_hex(bytes: &[u8]) -> String {
    use ::sha2::Digest;
    ::hex::encode(::sha2::Sha256::digest(bytes))
}

/// `text`'s bytes as the space-separated hex pairs AWS answers them in, which is what lets a
/// caller see the exact bytes the service hashed — the whitespace and the line endings
/// included, where a rendered string cannot show them.
///
/// Written into one string sized up front, three characters per byte, so the rendering costs
/// a single allocation; `text` is [`bounded`] before it reaches here, which is what bounds
/// that string.
fn hex_bytes(text: &str) -> String {
    let mut rendered = String::with_capacity(text.len().saturating_mul(3));
    for (index, byte) in text.as_bytes().iter().enumerate() {
        if index > 0 {
            rendered.push(' ');
        }
        // Writing to a `String` cannot fail; the formatter's `Result` is the trait's.
        let _ = write!(rendered, "{byte:02x}");
    }
    rendered
}

/// A value the SigV4 `Authorization` header states, such as the `Credential` that names the
/// signing scope.
fn authorization_field(header: &str, name: &str) -> Option<String> {
    let fields = header.strip_prefix(SIGV4_ALGORITHM)?.trim_start();
    fields
        .split(',')
        .find_map(|field| field.trim().strip_prefix(name)?.strip_prefix('='))
        .map(str::to_string)
}

/// The value of the query parameter `name`, percent-decoded — a presigned URL encodes the
/// separators of the values it carries.
///
/// `pub(super)`, like [`percent_decoded`]: the wire layer reads the `versionId` a request
/// addresses through this one reader, so one query string is read one way.
pub(super) fn query_parameter(query: &str, name: &str) -> Option<String> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(parameter, _)| percent_decoded(parameter) == name)
        .map(|(_, value)| percent_decoded(value))
}

/// The instant the SigV4 stamp `YYYYMMDDTHHMMSSZ` names.
///
/// Parsed by the library's own RFC 3339 reader rather than by any calendar arithmetic
/// written here: SigV4's basic form differs from RFC 3339 only in the separators it omits,
/// so they are put back and the timestamp type reads it. `None` for anything that is not
/// that form, which is why the digit check precedes the split.
fn amz_instant(stamp: &str) -> Option<SystemTime> {
    let (date, time) = stamp.strip_suffix('Z')?.split_once('T')?;
    if date.len() != 8 || time.len() != 6 {
        return None;
    }
    if !date.chars().chain(time.chars()).all(|c| c.is_ascii_digit()) {
        return None;
    }
    let rfc3339 = format!(
        "{}-{}-{}T{}:{}:{}Z",
        &date[..4],
        &date[4..6],
        &date[6..],
        &time[..2],
        &time[2..4],
        &time[4..]
    );
    let parsed = ::aws_smithy_types::DateTime::from_str(
        &rfc3339,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .ok()?;
    SystemTime::try_from(parsed).ok()
}

/// How this service answers the authentication refusal of the request `presented` describes,
/// `issued` answering whether an access key id names a credential this emulator ISSUED and
/// `anonymous_message` the sentence an ANONYMOUS request against the bucket it addresses is
/// refused with; or `None` for a refusal this service does not word, which keeps the
/// framework's own.
///
/// The order is AWS's: a query string the service cannot use is answered before anything
/// about the caller, so a presigned URL whose window is outside the accepted range is
/// refused for the URL whether or not its signature verifies; then the window that has
/// passed, which refuses an intact signature; then the signing INSTANT, which a signature
/// cannot rescue either; then the request that presented no credential at all, which has
/// neither; and last the signature itself, which is the only condition left once the
/// credential has resolved.
pub(super) fn refusal_for(
    presented: &Presented,
    issued: &dyn Fn(&str) -> bool,
    anonymous_message: &dyn Fn(Option<&str>) -> &'static str,
) -> Option<AuthRefusal> {
    if let Some(message) = presented
        .stated_expires
        .as_deref()
        .and_then(expires_out_of_range)
    {
        return Some(AuthRefusal {
            code: QUERY_PARAMETERS_CODE,
            status: QUERY_PARAMETERS_STATUS,
            message,
            diagnostics: None,
            skew: None,
        });
    }
    if presented.window_passed() {
        return Some(AuthRefusal {
            code: REQUEST_EXPIRED_CODE,
            status: UNAUTHENTICATED_STATUS,
            message: REQUEST_EXPIRED_MESSAGE,
            diagnostics: None,
            skew: None,
        });
    }
    // The signing INSTANT, before anything about the signature: a stamp too far from this
    // clock is refused for its time whatever the signature computes to, which is what lets a
    // caller tell a drifted clock from a wrong secret.
    if let Some(skew) = presented.skew_outside_window(SystemTime::now()) {
        return Some(AuthRefusal {
            code: CLOCK_SKEW_CODE,
            status: UNAUTHENTICATED_STATUS,
            message: CLOCK_SKEW_MESSAGE,
            diagnostics: None,
            skew: Some(skew),
        });
    }
    if !presented.carries_credential {
        // Nothing was presented, so nothing about a signature or a credential can be said:
        // the request speaks for the anonymous caller, and what refused it is what the
        // BUCKET it addresses says about such a caller — the charge it cannot be billed for
        // where the bucket makes the requester pay, and otherwise that the bucket grants it
        // nothing. One code either way, which is what makes the sentence the only thing
        // telling the two refusals apart.
        return Some(AuthRefusal {
            code: crate::state::ACCESS_DENIED_CODE,
            status: UNAUTHENTICATED_STATUS,
            message: anonymous_message(presented.addressed_bucket.as_deref()),
            diagnostics: None,
            skew: None,
        });
    }
    if !issued(&presented.access_key_id) {
        return None;
    }
    Some(AuthRefusal {
        code: SIGNATURE_MISMATCH_CODE,
        status: UNAUTHENTICATED_STATUS,
        message: SIGNATURE_MISMATCH_MESSAGE,
        diagnostics: presented
            .signing
            .as_ref()
            .map(|signing| Diagnostics::of(presented, signing)),
        skew: None,
    })
}

/// Why the validity window `stated` asks for is outside the range SigV4 query-string
/// authentication accepts — a non-negative count of seconds no greater than
/// [`MAX_PRESIGNED_EXPIRES_SECS`] — or `None` for a window inside it.
///
/// A value that is not a count of seconds at all states no window, so it is not answered
/// here: that request is refused for its credentials, which is what the framework already
/// words.
fn expires_out_of_range(stated: &str) -> Option<&'static str> {
    let stated = stated.trim();
    if let Some(magnitude) = stated.strip_prefix('-') {
        let negative = !magnitude.is_empty()
            && magnitude.chars().all(|c| c.is_ascii_digit())
            && magnitude.chars().any(|c| c != '0');
        return negative.then_some(EXPIRES_NEGATIVE_MESSAGE);
    }
    if stated.is_empty() || !stated.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    // Leading zeros are not magnitude, and a count of more digits than 64 bits holds is past
    // the ceiling by a long way — neither is a window inside the range.
    let digits = stated.trim_start_matches('0');
    let over_ceiling = match digits.parse::<u64>() {
        Ok(seconds) => seconds > MAX_PRESIGNED_EXPIRES_SECS,
        Err(_) => !digits.is_empty(),
    };
    over_ceiling.then_some(EXPIRES_OVER_CEILING_MESSAGE)
}

/// One authentication refusal as this service answers it: the wire error code, the status,
/// the fixed sentence, and the signing diagnostics where the refusal carries them.
pub(super) struct AuthRefusal {
    /// The `<Code>` a client branches on.
    pub(super) code: &'static str,
    /// The HTTP status it is answered with.
    pub(super) status: u16,
    /// The `<Message>`. Fixed text for every condition here: none of them states a value
    /// from the request.
    message: &'static str,
    /// The SigV4 signing diagnostics, for the one refusal that carries them.
    diagnostics: Option<Diagnostics>,
    /// The instants and the accepted window, for the one refusal that carries them.
    skew: Option<Skew>,
}

/// The signing diagnostics S3's signature refusal carries, every one of them derived from
/// the request as received.
struct Diagnostics {
    /// The access key id the request named.
    access_key_id: String,
    /// The string the service signed.
    string_to_sign: String,
    /// The canonical request it built that string from.
    canonical_request: String,
    /// The signature the request itself presented, unchanged.
    signature_provided: String,
}

impl Diagnostics {
    /// The diagnostics for the request `presented` and `signing` describe — built HERE, on
    /// the refusal path, rather than when the request was read: a request that is not
    /// refused never pays for the canonical request, the hash of it or the rendering of
    /// either.
    fn of(presented: &Presented, signing: &Signing) -> Self {
        let canonical_request = bounded(signing.canonical_request());
        Self {
            access_key_id: presented.access_key_id.clone(),
            string_to_sign: bounded(signing.string_to_sign(&canonical_request)),
            canonical_request,
            signature_provided: signing.signature.clone(),
        }
    }
}

/// The `<Message>` of this service's authentication refusal, named the way every other error
/// document of this service names it.
static MESSAGE: Schema = Schema::new_member(
    ShapeId::from_static(
        "smithy.framework#SigV4Refusal$message",
        "smithy.framework",
        "message",
    ),
    ShapeType::String,
    "message",
    0,
)
.with_xml_name("Message");

/// Declare one diagnostic element of the refusal document: a string member whose wire name
/// is the element S3 publishes it as.
macro_rules! diagnostic_member {
    ($constant:ident, $member:literal, $element:literal, $index:literal) => {
        static $constant: Schema = Schema::new_member(
            ShapeId::from_static(
                concat!("smithy.framework#SigV4Refusal$", $member),
                "smithy.framework",
                $member,
            ),
            ShapeType::String,
            $member,
            $index,
        )
        .with_xml_name($element);
    };
}

diagnostic_member!(ACCESS_KEY_ID, "awsAccessKeyId", "AWSAccessKeyId", 1);
diagnostic_member!(STRING_TO_SIGN, "stringToSign", "StringToSign", 2);
diagnostic_member!(SIGNATURE_PROVIDED, "signatureProvided", "SignatureProvided", 3);
diagnostic_member!(STRING_TO_SIGN_BYTES, "stringToSignBytes", "StringToSignBytes", 4);
diagnostic_member!(CANONICAL_REQUEST, "canonicalRequest", "CanonicalRequest", 5);
diagnostic_member!(
    CANONICAL_REQUEST_BYTES,
    "canonicalRequestBytes",
    "CanonicalRequestBytes",
    6
);
diagnostic_member!(REQUEST_TIME, "requestTime", "RequestTime", 7);
diagnostic_member!(SERVER_TIME, "serverTime", "ServerTime", 8);
diagnostic_member!(
    MAX_ALLOWED_SKEW,
    "maxAllowedSkewMilliseconds",
    "MaxAllowedSkewMilliseconds",
    9
);

/// The shape of this service's authentication-refusal document.
///
/// Synthetic, and deliberately in the `smithy.framework` namespace: these are protocol-level
/// S3 error elements, which the model declares no shape for, so the shape must not collide
/// with one it does declare. The wire `<Code>` and the status travel on [`AuthRefusal`]
/// rather than here.
pub(super) static REFUSAL_SCHEMA: Schema = Schema::new_struct(
    ShapeId::from_static(
        "smithy.framework#SigV4Refusal",
        "smithy.framework",
        "SigV4Refusal",
    ),
    ShapeType::Structure,
    &[
        &MESSAGE,
        &ACCESS_KEY_ID,
        &STRING_TO_SIGN,
        &SIGNATURE_PROVIDED,
        &STRING_TO_SIGN_BYTES,
        &CANONICAL_REQUEST,
        &CANONICAL_REQUEST_BYTES,
        &REQUEST_TIME,
        &SERVER_TIME,
        &MAX_ALLOWED_SKEW,
    ],
);

/// The document: the message, the instants a clock-skew refusal states, and — for the
/// signature refusal alone — the diagnostics, in the order S3's own `<Error>` carries them.
/// The `<Code>` and the request-correlation elements around them are the envelope's, written
/// by the protocol.
impl SerializableStruct for AuthRefusal {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_string(&MESSAGE, self.message)?;
        if let Some(skew) = &self.skew {
            s.write_string(&REQUEST_TIME, &skew.request_time)?;
            s.write_string(&SERVER_TIME, &skew.server_time)?;
            s.write_string(&MAX_ALLOWED_SKEW, &skew.max_allowed_skew_milliseconds)?;
        }
        let Some(diagnostics) = &self.diagnostics else {
            return Ok(());
        };
        s.write_string(&ACCESS_KEY_ID, &diagnostics.access_key_id)?;
        s.write_string(&STRING_TO_SIGN, &diagnostics.string_to_sign)?;
        s.write_string(&SIGNATURE_PROVIDED, &diagnostics.signature_provided)?;
        s.write_string(
            &STRING_TO_SIGN_BYTES,
            &hex_bytes(&diagnostics.string_to_sign),
        )?;
        s.write_string(&CANONICAL_REQUEST, &diagnostics.canonical_request)?;
        s.write_string(
            &CANONICAL_REQUEST_BYTES,
            &hex_bytes(&diagnostics.canonical_request),
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The accepted window around this service's clock, at both its bounds and one second
    /// past each of them, for a request that signed an instant and carries a credential.
    #[test]
    fn the_accepted_signing_window_is_the_published_one() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let rows: [(&str, Duration, bool, bool); 8] = [
            ("signed at this instant", Duration::ZERO, false, false),
            ("signed a minute ago", Duration::from_secs(60), false, false),
            ("signed at the window's edge", MAX_REQUEST_SKEW, false, false),
            (
                "signed one second past the window",
                MAX_REQUEST_SKEW + Duration::from_secs(1),
                false,
                true,
            ),
            (
                "signed far behind the clock",
                Duration::from_secs(1_500),
                false,
                true,
            ),
            (
                "stamped at the window's edge AHEAD of the clock",
                MAX_REQUEST_SKEW,
                true,
                false,
            ),
            (
                "stamped one second past the window AHEAD of the clock",
                MAX_REQUEST_SKEW + Duration::from_secs(1),
                true,
                true,
            ),
            (
                "stamped an hour AHEAD of the clock",
                Duration::from_secs(3_600),
                true,
                true,
            ),
        ];
        for (case, difference, ahead, refused) in rows {
            let signed_at = match ahead {
                true => now + difference,
                false => now - difference,
            };
            let presented = Presented {
                access_key_id: "EMULATORKEY".to_string(),
                stated_expires: None,
                signed_at: Some(signed_at),
                signing: None,
                carries_credential: true,
                addressed_bucket: None,
            };
            let skew = presented.skew_outside_window(now);
            assert_eq!(
                skew.is_some(),
                refused,
                "case {case:?}: expected refused={refused}, got {}",
                skew.is_some()
            );
        }
    }

    /// A PRESIGNED URL is judged by the window it asked for, not by the signing instant: one
    /// signed long ago and valid for a week is not a skewed request.
    #[test]
    fn a_presigned_url_is_not_judged_for_its_signing_instant() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let presented = Presented {
            access_key_id: "EMULATORKEY".to_string(),
            stated_expires: Some("604800".to_string()),
            signed_at: Some(now - Duration::from_secs(3_600)),
            signing: None,
            carries_credential: true,
            addressed_bucket: None,
        };
        assert!(
            presented.skew_outside_window(now).is_none(),
            "a presigned URL inside the window it asked for must not be refused for its \
             signing instant"
        );
    }

    /// The range SigV4 query-string authentication accepts, at both its bounds and one past
    /// each of them.
    #[test]
    fn the_accepted_expires_range_is_the_published_one() {
        let rows: [(&str, Option<&str>); 8] = [
            ("1", None),
            ("900", None),
            ("0", None),
            ("604800", None),
            ("604801", Some(EXPIRES_OVER_CEILING_MESSAGE)),
            ("99999999999999999999999", Some(EXPIRES_OVER_CEILING_MESSAGE)),
            ("-1", Some(EXPIRES_NEGATIVE_MESSAGE)),
            ("not-a-number", None),
        ];
        for (stated, expected) in rows {
            assert_eq!(
                expires_out_of_range(stated),
                expected,
                "case {stated:?}: expected {expected:?}, got {:?}",
                expires_out_of_range(stated)
            );
        }
    }

    /// The canonical query string drops the signature, keeps every other pair as received,
    /// and sorts what is left.
    #[test]
    fn the_canonical_query_is_sorted_and_signature_free() {
        let canonical = canonical_query(
            "X-Amz-Signature=deadbeef&X-Amz-Expires=600&X-Amz-Date=20260101T000000Z\
             &X-Amz-Credential=AKID%2F20260101%2Fus-east-1%2Fs3%2Faws4_request",
        );
        assert_eq!(
            canonical,
            "X-Amz-Credential=AKID%2F20260101%2Fus-east-1%2Fs3%2Faws4_request\
             &X-Amz-Date=20260101T000000Z&X-Amz-Expires=600",
            "the canonical query must be sorted, encoded once and carry no signature, got \
             {canonical:?}"
        );
    }
}
