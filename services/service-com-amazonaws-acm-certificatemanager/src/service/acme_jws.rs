// SPDX-License-Identifier: CC0-1.0

//! The reading of one ACME request's JWS: the envelope every interaction on an ACME
//! endpoint's data plane arrives in (RFC 8555 §6.2).
//!
//! An ACME client does not send a JSON request body. It sends a JSON Web Signature in the
//! JSON serialization of RFC 7515 §7.2.2 — `{"protected": …, "payload": …, "signature":
//! …}`, each member one base64url segment — whose protected header carries four things
//! RFC 8555 §6.2 makes mandatory: the algorithm (`alg`), the anti-replay nonce (`nonce`),
//! the URL the request is addressed to (`url`), and the key it is signed with, as either
//! the key itself (`jwk`, for a request that registers or recovers an account) or the
//! account that holds it (`kid`, for every other request).
//!
//! What this module does is read that envelope and nothing else: it decides whether a
//! body IS such a JWS and reports WHY it is not ([`Defect`], which renders the reason as
//! the `detail` of a problem document). Which key the signature is checked against, which
//! nonce is live, and what the payload means are the ACME server's rules, and live in
//! [`crate::service::acme_http`].
//!
//! Every primitive underneath is the workspace's own: the JSON parse is `serde_json`, the
//! base64url coding and the signature check are
//! [`service_common::jwt`] — the one JOSE reading this repository holds — so nothing here
//! decodes, digests or verifies by hand.

/// The largest ACME request body this module reads, in bytes.
///
/// A CHOSEN bound, not a documented one: an ACME client's JWS carries a public key, a
/// handful of URLs and a short payload, and the largest interaction (a certificate
/// request naming many identifiers) is still kilobytes. It caps the base64 decode and the
/// JSON parse one unauthenticated request can ask for, since this wire carries no AWS
/// credential at all.
pub const MAX_JWS_BYTES: usize = 64 * 1024;

/// The media type an ACME JWS travels under: "the body of the POST MUST be a JWS object"
/// served as `application/jose+json` (RFC 8555 §6.2).
pub const JOSE_CONTENT_TYPE: &str = "application/jose+json";

/// The protected-header parameter naming the signature algorithm (RFC 7515 §4.1.1).
const ALGORITHM: &str = "alg";

/// The protected-header parameter carrying the anti-replay nonce (RFC 8555 §6.5).
const NONCE: &str = "nonce";

/// The protected-header parameter carrying the URL the request is addressed to
/// (RFC 8555 §6.4).
const URL: &str = "url";

/// The protected-header parameter carrying the signing key itself (RFC 7515 §4.1.3).
const JWK: &str = "jwk";

/// The protected-header parameter naming the account whose key signed the request
/// (RFC 7515 §4.1.4, which RFC 8555 §6.2 fixes as the account's URL).
const KEY_ID: &str = "kid";

/// Why a request body is not an ACME JWS.
///
/// `Display` renders the reason as one sentence about the request, which is what the
/// `detail` member of an ACME problem document carries (RFC 8555 §6.7). It names nothing
/// the request sent, so a refusal cannot echo a caller's own text back at them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Defect {
    /// Larger than [`MAX_JWS_BYTES`], so it is not read at all.
    TooLarge,
    /// Not a JSON object, so it is no JWS in the JSON serialization.
    NotJson,
    /// One of the three members RFC 7515 §7.2.2 requires is missing, is not a string, or
    /// is not base64url.
    Segments,
    /// The protected header is not base64url-encoded JSON, or is not a JSON object.
    Header,
    /// The protected header names no signature algorithm.
    Algorithm,
    /// The protected header carries no anti-replay nonce.
    Nonce,
    /// The protected header carries no request URL.
    Url,
    /// The protected header carries neither a key nor an account, or carries both — "the
    /// JWS MUST NOT have the value 'jwk' and 'kid' both present" (RFC 8555 §6.2).
    Key,
}

impl ::std::fmt::Display for Defect {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match self {
            Self::TooLarge => write!(
                f,
                "The request body is larger than the {MAX_JWS_BYTES} bytes this ACME server reads"
            ),
            Self::NotJson => f.write_str("The request body is not a JWS JSON object"),
            Self::Segments => f.write_str(
                "The request body is not a JWS: it must carry the protected, payload and \
                 signature members as base64url strings",
            ),
            Self::Header => {
                f.write_str("The JWS protected header is not base64url-encoded JSON")
            }
            Self::Algorithm => {
                f.write_str("The JWS protected header names no signature algorithm")
            }
            Self::Nonce => f.write_str("The JWS protected header carries no nonce"),
            Self::Url => f.write_str("The JWS protected header carries no url"),
            Self::Key => f.write_str(
                "The JWS protected header must carry exactly one of jwk and kid",
            ),
        }
    }
}

/// Which key an ACME request is signed with, as its protected header states it.
#[derive(Debug, Clone)]
pub enum SigningKey {
    /// The key itself, embedded in the header (`jwk`) — the form a `newAccount` request
    /// takes, because the server does not know the key yet (RFC 8555 §6.2).
    Embedded(::serde_json::Value),
    /// The account that holds the key (`kid`, the account's own URL), which the server
    /// looks the key up by.
    Account(String),
}

/// One ACME request's JWS, read but not verified.
#[derive(Debug, Clone)]
pub struct Jws {
    /// The `alg` the protected header names.
    pub algorithm: String,
    /// The anti-replay nonce the protected header carries.
    pub nonce: String,
    /// The URL the protected header says the request is addressed to.
    pub url: String,
    /// The key the signature is to be checked against.
    pub key: SigningKey,
    /// The exact octets the signature covers: `base64url(protected) '.'
    /// base64url(payload)` (RFC 7515 §5.2), taken from the request as received, so a
    /// payload re-rendered after signing no longer checks out.
    pub signing_input: String,
    /// The decoded payload. Empty for a POST-as-GET, which "MUST have a payload of the
    /// empty string" (RFC 8555 §6.3).
    pub payload: Vec<u8>,
    /// The decoded signature.
    pub signature: Vec<u8>,
}

impl Jws {
    /// Read `body` as an ACME request's JWS.
    ///
    /// Total: a body that is not one is a [`Defect`], never a panic — nothing is indexed,
    /// sliced or unwrapped on the way through, and the size bound is applied first.
    pub fn read(body: &[u8]) -> Result<Self, Defect> {
        if body.len() > MAX_JWS_BYTES {
            return Err(Defect::TooLarge);
        }
        let document: ::serde_json::Value =
            ::serde_json::from_slice(body).map_err(|_| Defect::NotJson)?;
        let parts = Parts::read(&document)?;
        let member = |name: &str| {
            parts
                .protected
                .get(name)
                .and_then(::serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
        };
        let algorithm = member(ALGORITHM).ok_or(Defect::Algorithm)?.to_string();
        let nonce = member(NONCE).ok_or(Defect::Nonce)?.to_string();
        let url = member(URL).ok_or(Defect::Url)?.to_string();
        let key = match (parts.protected.get(JWK), member(KEY_ID)) {
            (Some(jwk), None) if jwk.is_object() => SigningKey::Embedded(jwk.clone()),
            (None, Some(account)) => SigningKey::Account(account.to_string()),
            _ => return Err(Defect::Key),
        };
        Ok(Self {
            algorithm,
            nonce,
            url,
            key,
            signing_input: parts.signing_input,
            payload: parts.payload,
            signature: parts.signature,
        })
    }

    /// The payload as the JSON object an ACME interaction's request body is.
    ///
    /// `None` for a payload that is not a JSON object — including the EMPTY payload of a
    /// POST-as-GET, which carries no object by construction ([`Self::is_post_as_get`]
    /// tells the two apart).
    pub fn payload_object(&self) -> Option<::serde_json::Map<String, ::serde_json::Value>> {
        match ::serde_json::from_slice::<::serde_json::Value>(&self.payload) {
            Ok(::serde_json::Value::Object(members)) => Some(members),
            _ => None,
        }
    }

    /// Whether this is a POST-as-GET: a request that reads a resource rather than
    /// changing it, which "MUST have a payload of the empty string" (RFC 8555 §6.3).
    pub fn is_post_as_get(&self) -> bool {
        self.payload.is_empty()
    }
}

/// The three members of a JWS in the JSON serialization, decoded.
struct Parts {
    /// The protected header, as the JSON object it decoded to.
    protected: ::serde_json::Map<String, ::serde_json::Value>,
    /// The octets the signature covers, as received.
    signing_input: String,
    /// The decoded payload.
    payload: Vec<u8>,
    /// The decoded signature.
    signature: Vec<u8>,
}

impl Parts {
    /// Read the three members off `document`, or report which part of the form is wrong.
    fn read(document: &::serde_json::Value) -> Result<Self, Defect> {
        let segment = |name: &str| {
            document
                .get(name)
                .and_then(::serde_json::Value::as_str)
                .ok_or(Defect::Segments)
        };
        let protected_segment = segment("protected")?;
        let payload_segment = segment("payload")?;
        let signature_segment = segment("signature")?;
        let decoded = |segment: &str| {
            ::service_common::jwt::decode_base64url(segment).ok_or(Defect::Segments)
        };
        let protected = match ::serde_json::from_slice::<::serde_json::Value>(&decoded(
            protected_segment,
        )?) {
            Ok(::serde_json::Value::Object(members)) => members,
            _ => return Err(Defect::Header),
        };
        Ok(Self {
            protected,
            signing_input: format!("{protected_segment}.{payload_segment}"),
            payload: decoded(payload_segment)?,
            signature: decoded(signature_segment)?,
        })
    }
}

/// An external account binding, as a `newAccount` request carries one: "a JWS object …
/// whose protected header MUST include an 'alg' field … a 'kid' field … and a 'url'
/// field", MAC-signed with the key the CA issued out of band (RFC 8555 §7.3.4).
#[derive(Debug, Clone)]
pub struct ExternalAccountBinding {
    /// The `alg` its protected header names, which this server checks against the one MAC
    /// algorithm it issues keys for.
    pub algorithm: String,
    /// The key id its protected header names: the identifier the CA handed the client.
    pub key_id: String,
    /// The URL its protected header names, which must be the one the registration itself
    /// is addressed to.
    pub url: String,
    /// Its payload: the account key the binding is bound to, as a JWK.
    pub account_key: ::serde_json::Value,
    /// The octets its MAC covers.
    pub signing_input: String,
    /// The MAC it carries.
    pub signature: Vec<u8>,
}

impl ExternalAccountBinding {
    /// Read `value` — the `externalAccountBinding` member of a `newAccount` payload — as
    /// such a JWS. `None` for a value that is not one, which is a malformed registration
    /// rather than an unauthorized one.
    pub fn read(value: &::serde_json::Value) -> Option<Self> {
        let parts = Parts::read(value).ok()?;
        let member = |name: &str| {
            parts
                .protected
                .get(name)
                .and_then(::serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        };
        let account_key = ::serde_json::from_slice::<::serde_json::Value>(&parts.payload).ok()?;
        if !account_key.is_object() {
            return None;
        }
        Some(Self {
            algorithm: member(ALGORITHM)?,
            key_id: member(KEY_ID)?,
            url: member(URL)?,
            account_key,
            signing_input: parts.signing_input,
            signature: parts.signature,
        })
    }
}
