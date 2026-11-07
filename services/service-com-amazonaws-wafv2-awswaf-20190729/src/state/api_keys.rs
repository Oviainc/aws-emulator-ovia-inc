// SPDX-License-Identifier: CC0-1.0

//! The CAPTCHA integration surface: the form an `APIKey` VALUE takes, what it
//! decrypts back to, and the application integration URL a client loads the CAPTCHA
//! script from.
//!
//! An API key is "the generated, encrypted API key" a client "can copy for use in
//! its JavaScript CAPTCHA integration" (`APIKeySummary$APIKey`); the key "contains a
//! set of token domains" (`CreateAPIKey`'s own doc), and `GetDecryptedAPIKey`
//! "returns your API key in decrypted form" so a caller "can check the token domains
//! that you have defined for the key". So the value is not an identifier the store
//! happens to key a record by: it is an opaque token that CARRIES the key's own
//! definition, encrypted under a key only this service holds — which is what lets
//! the domains be decrypted back out of it, and what a digest of the definition
//! could never do. The record beside it ([`ApiKeyEntry`]) is what says the key EXISTS
//! in a scope; the token is what says what it carries.

use super::*;

/// The nonce an API key's envelope carries, in bytes: the width AES-GCM is
/// specified at.
const API_KEY_NONCE_LEN: usize = 12;

/// The authentication tag AES-GCM appends to the ciphertext, in bytes.
const API_KEY_TAG_LEN: usize = 16;

/// The byte that separates the fields of an API key's encrypted payload.
///
/// `TokenDomain` is `^[\w\.\-/]+$`, a `Scope` is one of two words and the other
/// fields are digits and hex, so no field of the payload can contain a NUL — the
/// split that reads the payload back is therefore exact for every key this service
/// seals.
const API_KEY_FIELD_SEPARATOR: u8 = 0;

/// The payload layout this service seals an API key under, so a value sealed under
/// a layout this build does not know is not read as though it were this one.
const API_KEY_PAYLOAD_LAYOUT: &str = "1";

/// How many characters the key's own identifier inside the payload carries: the 256
/// bits of [`fresh_entropy`], two characters per byte.
///
/// The identifier is what makes the key UNPREDICTABLE: the rest of the payload is
/// the definition the caller asked for, so without it a key would be a function of
/// the request and anyone could write one.
const API_KEY_ID_LENGTH: usize = 64;

/// The smallest envelope a mint produces, in bytes: the nonce, the tag and a
/// payload carrying the key's identifier alone.
///
/// A value that decodes to less than this carries no key — not even the identifier
/// every key has — so it is not one of this service's keys in any scope.
const API_KEY_ENVELOPE_MIN: usize = API_KEY_NONCE_LEN + API_KEY_TAG_LEN + API_KEY_ID_LENGTH;

/// The AEAD an API key's payload is sealed with, keyed for the lifetime of the
/// process.
///
/// The 256-bit key is drawn once from [`fresh_entropy`] and never leaves this
/// module: it is not a caller's credential and is in no response, no log and no
/// file — it exists so that the token a client is handed is genuinely encrypted,
/// and so that a key handed back is a key this service minted rather than one the
/// client wrote itself. The AEAD is the maintained `aes-gcm` implementation;
/// nothing here hand-rolls one.
fn api_key_cipher() -> &'static ::aes_gcm::Aes256Gcm {
    use ::aes_gcm::KeyInit;

    static CIPHER: ::std::sync::OnceLock<::aes_gcm::Aes256Gcm> = ::std::sync::OnceLock::new();
    CIPHER.get_or_init(|| {
        let secret = fresh_entropy();
        ::aes_gcm::Aes256Gcm::new(::aes_gcm::Key::<::aes_gcm::Aes256Gcm>::from_slice(&secret))
    })
}

/// The coder an API key's envelope is written with: base64 over the URL-safe
/// alphabet, without padding — the alphabet a token copied into a page, a header or
/// a URL is carried in, and every character of it non-whitespace, which is
/// `APIKey`'s own `pattern(\S)`.
fn api_key_coder() -> &'static ::base64::engine::general_purpose::GeneralPurpose {
    &::base64::engine::general_purpose::URL_SAFE_NO_PAD
}

/// The bytes an API key's envelope seals: the layout, the scope the key was defined
/// in, when it was created, the key's own identifier, and the token domains it
/// carries, in the order they are stored in ([`normalized_token_domains`]).
fn api_key_payload(scope: &str, token_domains: &[String], created_at_millis: i64) -> Vec<u8> {
    let mut payload: Vec<u8> = Vec::new();
    for field in [
        API_KEY_PAYLOAD_LAYOUT.to_string(),
        scope.to_string(),
        created_at_millis.to_string(),
        ::hex::encode(fresh_entropy()),
    ]
    .iter()
    .chain(token_domains.iter())
    {
        if !payload.is_empty() {
            payload.push(API_KEY_FIELD_SEPARATOR);
        }
        payload.extend_from_slice(field.as_bytes());
    }
    payload
}

/// A fresh API key value for a key defined in `scope` over `token_domains` at
/// `created_at_millis`, or `None` when the payload could not be sealed.
///
/// `CreateAPIKey` answers "the generated, encrypted API key", an `APIKey`
/// (`@length(1..2048)`, `@pattern(\S)` —
/// `smithy_query(query="shape",shape="APIKey")`), and the model gives the value no
/// further syntax, because a client does not parse it: it copies the value into its
/// JavaScript CAPTCHA integration and hands it back to this service. What the model
/// does say is what the value IS — an encrypted key that contains the token domains
/// it was defined over — so the value is that definition sealed: a fresh
/// [`API_KEY_NONCE_LEN`]-byte nonce, the ciphertext of the payload and its
/// [`API_KEY_TAG_LEN`]-byte tag, rendered with [`api_key_coder`]. A key is therefore
/// a value of the order `APIKey`'s own bound is drawn for — the domains it carries
/// plus the seal around them — and not a digest, which nothing could be decrypted
/// out of.
///
/// Each key carries its own identifier ([`API_KEY_ID_LENGTH`]), so two mints over
/// one definition are two different values and no key can be predicted from the
/// definition it was asked for. A key is nevertheless MINTED only for a definition
/// the scope does not already hold one for
/// ([`ComAmazonawsWafv2Awswaf20190729State::api_key_for_token_domains`]), so a scope
/// holds one key per token-domain set however often a mint is called for it.
pub fn new_api_key(
    scope: &str,
    token_domains: &[String],
    created_at_millis: i64,
) -> Option<String> {
    use ::aes_gcm::aead::Aead;
    use ::base64::Engine as _;

    let entropy = fresh_entropy();
    let nonce = &entropy[..API_KEY_NONCE_LEN];
    let sealed = api_key_cipher()
        .encrypt(
            ::aes_gcm::Nonce::from_slice(nonce),
            api_key_payload(scope, token_domains, created_at_millis).as_slice(),
        )
        .ok()?;
    let mut envelope: Vec<u8> = Vec::with_capacity(nonce.len() + sealed.len());
    envelope.extend_from_slice(nonce);
    envelope.extend_from_slice(&sealed);
    Some(api_key_coder().encode(envelope))
}

/// What an API key's envelope carries: the definition the key was sealed over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecryptedApiKey {
    /// The `Scope` the key was defined in.
    pub scope: String,
    /// `APIKeySummary$CreationTimestamp` as milliseconds since the Unix epoch.
    pub created_at_millis: i64,
    /// "The token domains that are defined in this API key"
    /// (`GetDecryptedAPIKeyResponse$TokenDomains`), in the order the key carries
    /// them.
    pub token_domains: Vec<String>,
}

/// The envelope `value` decodes to, or `None` when it is not one at all: not
/// base64 over the key alphabet, or smaller than [`API_KEY_ENVELOPE_MIN`].
fn api_key_envelope(value: &str) -> Option<Vec<u8>> {
    use ::base64::Engine as _;

    let decoded = api_key_coder().decode(value).ok()?;
    (decoded.len() >= API_KEY_ENVELOPE_MIN).then_some(decoded)
}

/// `true` when `value` has the form this service mints an API key in: an envelope
/// of at least the size every key carries, as [`new_api_key`] renders it.
///
/// A key is "the generated, encrypted API key", so a value that does not even
/// decode to an envelope is not a key WAF could decrypt, whatever any scope holds —
/// which is the difference between a value that is not a key and a well-formed key
/// naming nothing stored. This judges the FORM only: a well-formed envelope this
/// service did not seal is a key the scope does not hold, not a malformed one.
pub fn is_api_key_shaped(value: &str) -> bool {
    api_key_envelope(value).is_some()
}

/// The definition `value` decrypts to, or `None` when it is not a key this service
/// sealed — a value of another form, one sealed under another payload layout, or
/// one whose authentication tag does not hold.
pub fn decrypt_api_key(value: &str) -> Option<DecryptedApiKey> {
    use ::aes_gcm::aead::Aead;

    let envelope = api_key_envelope(value)?;
    let (nonce, sealed) = envelope.split_at(API_KEY_NONCE_LEN);
    let payload = api_key_cipher()
        .decrypt(::aes_gcm::Nonce::from_slice(nonce), sealed)
        .ok()?;
    let mut fields = payload.split(|byte| *byte == API_KEY_FIELD_SEPARATOR);
    if fields.next()? != API_KEY_PAYLOAD_LAYOUT.as_bytes() {
        return None;
    }
    let scope = ::std::str::from_utf8(fields.next()?).ok()?.to_string();
    let created_at_millis = ::std::str::from_utf8(fields.next()?)
        .ok()?
        .parse::<i64>()
        .ok()?;
    // The key's own identifier is not part of the definition: it is read past.
    let _key_id = fields.next()?;
    let token_domains = fields
        .map(|domain| ::std::str::from_utf8(domain).map(str::to_string))
        .collect::<Result<Vec<String>, _>>()
        .ok()?;
    Some(DecryptedApiKey {
        scope,
        created_at_millis,
        token_domains,
    })
}

/// The domain WAF serves the CAPTCHA and challenge JavaScript integration from.
///
/// `ListAPIKeysResponse$ApplicationIntegrationURL` is "the CAPTCHA application
/// integration URL, for use in your JavaScript implementation", and the recorded
/// WAF responses answer an `https` URL under this domain, hosted per region and per
/// account.
const CAPTCHA_SDK_DOMAIN: &str = "captcha-sdk.awswaf.com";

/// How many hex characters the integration id in [`application_integration_url`]
/// carries: the 128 bits of the digest it is rendered from, two characters per
/// byte.
const CAPTCHA_INTEGRATION_ID_LENGTH: usize = 32;

/// The id of this account's CAPTCHA integration: the host label, and the path
/// segment, of [`application_integration_url`].
///
/// An account's integration is not a resource a caller creates — nothing in this
/// model mints or names one — so the id is a PROPERTY of the account: the same
/// value on every call, derived from the account the emulator serves (the `sha2`
/// digest this crate renders every other derived value with), never from the
/// request that happens to ask for the URL.
fn captcha_integration_id() -> String {
    use ::sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(CAPTCHA_SDK_DOMAIN.as_bytes());
    hasher.update(::engine::emulator_account_id().as_bytes());
    let digest = ::hex::encode(hasher.finalize());
    digest
        .chars()
        .take(CAPTCHA_INTEGRATION_ID_LENGTH)
        .collect()
}

/// The `ApplicationIntegrationURL` this account's clients load the CAPTCHA
/// JavaScript integration from, at the endpoint region `region`.
///
/// "The CAPTCHA application integration URL, for use in your JavaScript
/// implementation" (`ListAPIKeysResponse$ApplicationIntegrationURL`): WAF hosts the
/// integration per account and per region and answers its URL whatever keys the
/// account holds — a client reads the URL from this listing BEFORE it mints its
/// first key — so it is reported from the account and the region alone, and is the
/// same URL on every call.
pub fn application_integration_url(region: &str) -> String {
    let integration = captcha_integration_id();
    format!("https://{integration}.{region}.{CAPTCHA_SDK_DOMAIN}/{integration}/")
}
