// SPDX-License-Identifier: CC0-1.0

//! The tokens a pool issues: how one is minted, and the key its JWTs are signed
//! with.

use super::*;

/// A fresh refresh token: the opaque credential a completed sign-in hands its
/// caller, and the value a later refresh presents back.
///
/// "Your user's refresh token" (`AuthenticationResultType.RefreshToken` in this
/// service's model) is a bearer credential and nothing else: unlike the access
/// and id tokens beside it, it states no claim its holder can read, and
/// everything this service knows about it lives in the [`TokenRecord`] keyed by
/// it. It is therefore minted by the one component that generates secrets — the
/// emulator's credential store — and then removed from that store again: the
/// caller gets the secret, the emulator keeps no credential it would accept as a
/// request signature, and nothing accumulates per sign-in.
///
/// The minted secret is rendered base64url with [`base64_url`], the codec this
/// crate already uses for every opaque value it puts on the wire, so the
/// credential satisfies the modelled `TokenModelType` pattern
/// `^[A-Za-z0-9-_=.]+$` whatever alphabet the store drew it from, and keeps every
/// bit of its randomness.
///
/// `None` when the store minted no session-token secret, or when the minted
/// credential could not be removed from the store again — a credential the
/// emulator would still accept as a request signature is not the token a sign-in
/// promises, so it is refused rather than handed out, and the failed removal is
/// logged. Either way the sign-in refuses rather than reporting a credential
/// nothing issued.
pub fn mint_refresh_token(store: &::emulator_server::CredentialStore) -> Option<String> {
    let minted = store.create(&::emulator_server::aws_credentials::AWS_SESSION);
    let token = minted
        .secret(::emulator_server::aws_credentials::SESSION_TOKEN)
        .map(|secret| base64_url(secret.as_bytes()));
    // The refresh token authenticates to this service's own token exchange, not
    // to the emulator's request signing; see above.
    if let Err(error) = store.delete(&minted.id) {
        ::tracing::warn!(
            access_key_id = %minted.id,
            error = ?error,
            "a credential minted for a refresh token could not be deleted"
        );
        return None;
    }
    token
}

/// `len` random hex characters, drawn from v4 UUIDs (122 bits of entropy each)
/// with their hyphens removed, so the result satisfies `^[\w+]+$`.
pub(super) fn random_token(len: usize) -> String {
    let mut token = String::with_capacity(len + 32);
    while token.len() < len {
        token.extend(new_uuid().chars().filter(|c| *c != '-'));
    }
    token.truncate(len);
    token
}

/// `len` random characters of [`ALPHANUMERIC_ALPHABET`], drawn from the same
/// randomness [`random_token`] draws on: each byte of a v4 UUID, read as the hex
/// pair it is rendered as, picks one character of the alphabet. A byte past the
/// last whole multiple of the alphabet is DROPPED rather than folded, so no
/// character is drawn more often than another.
pub(super) fn random_alphanumeric_token(len: usize) -> String {
    let alphabet = ALPHANUMERIC_ALPHABET.len();
    // The byte values that leave the draw uniform: the ones below the last whole
    // multiple of the alphabet's size.
    let drawable = 256 - 256 % alphabet;
    let mut token = String::with_capacity(len);
    while token.len() < len {
        let hex = random_token(len.saturating_sub(token.len()).saturating_mul(4));
        for pair in hex.as_bytes().chunks(2) {
            let drawn = ::std::str::from_utf8(pair)
                .ok()
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                .map(usize::from)
                .filter(|drawn| *drawn < drawable);
            if let Some(drawn) = drawn {
                token.push(char::from(ALPHANUMERIC_ALPHABET[drawn % alphabet]));
            }
        }
    }
    token.truncate(len);
    token
}

/// The 62 characters a mixed-case alphanumeric token is drawn from.
pub(super) const ALPHANUMERIC_ALPHABET: &[u8; 62] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

/// What one sign-in's tokens are built from, read from the stored pool, app
/// client and user under the state lock before any token is minted.
pub(super) struct TokenInputs {
    /// The pool's token issuer (its regional user-pool endpoint).
    pub(super) issuer: String,
    /// The stored user's immutable id, the `sub` claim.
    pub(super) sub: String,
    /// The username as STORED, which a case-insensitive pool may case
    /// differently from the request.
    pub(super) stored_username: String,
    /// The stored `email` attribute, when the user has one.
    pub(super) email: Option<String>,
    /// The names of the pool's groups the stored user belongs to, in stored order.
    pub(super) groups: Vec<String>,
    /// What those groups' roles and precedences put in the identity token.
    pub(super) role_claims: GroupRoleClaims,
    /// The pool's own token-signing key pair: every token of this sign-in is
    /// signed with it and names it by `kid`.
    pub(super) signing_key: PoolSigningKey,
    pub(super) access_ttl: Duration,
    pub(super) id_ttl: Duration,
    pub(super) refresh_ttl: Duration,
}

// ---------------------------------------------------------------------------
// Token signing
//
// Amazon Cognito signs every token a user pool issues against a key pair
// belonging to THAT pool, and publishes the verifying half as a JSON Web Key
// Set: a verifier must "decode the token ... and verify the signature" against
// the key the token's `kid` names in the pool's `jwks.json` (Amazon Cognito
// Developer Guide, "Verifying a JSON web token"). Every consumer of such a
// token — Verified Permissions, an API Gateway JWT authorizer, any JOSE
// library — reads `alg` and `kid` from the header, finds the matching JWK and
// checks the signature over `base64url(header) '.' base64url(claims)`, so a
// token whose third segment is EMPTY is unusable: it is not even a
// syntactically valid signed JWT.
//
// AWS signs with RS256. This emulator signs with ES256 (ECDSA on NIST P-256
// with SHA-256, RFC 7518 §3.4) because the only maintained pure-Rust RSA
// implementation carries two unfixable timing advisories (RUSTSEC-2023-0071 /
// CVE-2023-49092 and GHSA-4grx-2x9w-596c, both "no fixed version") that this
// repository's dependency audit blocks. ES256 comes from `ring`, the
// cryptography library this workspace already signs and encrypts with
// elsewhere, rather than from the `p256`/`elliptic-curve` stack, whose
// `crypto-bigint` 0.5 dependency carries a timing advisory (RUSTSEC-2026-0387)
// with no release in that line that fixes it. Nothing a verifier does changes:
// the algorithm is named in the token header AND in the published JWK, which is
// where a JOSE library reads it from, so the token verifies by the same
// `kid`-lookup-then-check-signature path. Both algorithms are JOSE registered
// (RFC 7518 §3.1), and the signature segment is a non-empty base64url run
// either way.
// ---------------------------------------------------------------------------

/// The `alg` of every token this service signs, and of the JWK it publishes for
/// the key that signed it: ECDSA using P-256 and SHA-256 (RFC 7518 §3.1/§3.4).
pub(super) const TOKEN_SIGNING_ALGORITHM: &str = "ES256";

/// The `crv` of the curve [`TOKEN_SIGNING_ALGORITHM`] is defined over, as the
/// published JWK names it (RFC 7518 §6.2.1.1).
pub(super) const TOKEN_SIGNING_CURVE: &str = "P-256";

/// The signing algorithm every pool's key is generated for and signs with:
/// ECDSA on P-256 with SHA-256, in the FIXED (`R || S`) signature encoding RFC
/// 7518 §3.4 puts in a JWS segment — not the ASN.1 DER form, which a JOSE
/// verifier would reject.
pub(super) const SIGNING_ALGORITHM: &::ring::signature::EcdsaSigningAlgorithm =
    &::ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING;

/// One user pool's token-signing key pair: the private key the pool's tokens
/// are signed with, named by the `kid` its published JWK carries.
///
/// The key belongs to the pool, so a token one pool issued carries one pool's
/// `kid` and verifies against only that pool's JWKS.
#[derive(Clone)]
pub struct PoolSigningKey {
    /// The `kid` the token header carries and the published JWK is found by:
    /// this key's RFC 7638 JWK thumbprint, so it is derived from the key itself
    /// rather than minted beside it.
    pub(super) kid: String,
    /// The pool's private key. Behind an [`::std::sync::Arc`] because the key
    /// pair itself is not clonable and a pool's key is READ from many places —
    /// sharing the one key is also what makes every token of one pool verify
    /// against that pool's single published JWK.
    pub(super) private: ::std::sync::Arc<::ring::signature::EcdsaKeyPair>,
}

/// Renders the key's PUBLIC identity only. Private key material is never
/// logged, traced or put in a response, so it is not in this rendering either.
impl ::std::fmt::Debug for PoolSigningKey {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("PoolSigningKey")
            .field("kid", &self.kid)
            .field("alg", &TOKEN_SIGNING_ALGORITHM)
            .finish_non_exhaustive()
    }
}

impl PoolSigningKey {
    /// A freshly generated key pair, drawn from the operating system's CSPRNG
    /// (`ring::rand::SystemRandom`).
    ///
    /// One scalar multiplication, so this is microseconds of work: a pool mints
    /// its key when the pool is created and no request path ever generates one.
    /// INFALLIBLE: the only failure the generator reports is the OS CSPRNG or
    /// the key encoding failing, neither of which a pool can be created without.
    #[must_use]
    pub fn generate() -> Self {
        let rng = ::ring::rand::SystemRandom::new();
        let document = ::ring::signature::EcdsaKeyPair::generate_pkcs8(SIGNING_ALGORITHM, &rng)
            .expect("the operating system CSPRNG yields a P-256 key pair");
        let private =
            ::ring::signature::EcdsaKeyPair::from_pkcs8(SIGNING_ALGORITHM, document.as_ref(), &rng)
                .expect("a freshly generated P-256 key pair parses");
        let kid = jwk_thumbprint(&public_coordinates(&private));
        Self {
            kid,
            private: ::std::sync::Arc::new(private),
        }
    }

    /// The `kid` naming this key in the pool's JWKS.
    #[must_use]
    pub fn kid(&self) -> &str {
        &self.kid
    }

    /// This key's PUBLIC half as a JWK (RFC 7517), the entry a verifier finds
    /// by `kid` in the pool's key set: `kty`/`crv` and the `x`/`y` curve
    /// coordinates RFC 7518 §6.2.1 defines, with the `use` and `alg` the
    /// signature is checked under. No private parameter (`d`) is in it.
    #[must_use]
    pub fn to_jwk(&self) -> serde_json::Value {
        let (x, y) = public_coordinates(&self.private);
        serde_json::json!({
            "alg": TOKEN_SIGNING_ALGORITHM,
            "crv": TOKEN_SIGNING_CURVE,
            "kid": self.kid,
            "kty": "EC",
            "use": "sig",
            "x": base64_url(&x),
            "y": base64_url(&y),
        })
    }

    /// The ES256 signature over `signing_input` — the 64-byte `R || S` form RFC
    /// 7518 §3.4 puts in the third segment — or `None` when signing failed, in
    /// which case the caller issues no token rather than one carrying bytes
    /// that are not a signature.
    #[must_use]
    pub fn sign(&self, signing_input: &[u8]) -> Option<Vec<u8>> {
        let rng = ::ring::rand::SystemRandom::new();
        let signature = self.private.sign(&rng, signing_input).ok()?;
        Some(signature.as_ref().to_vec())
    }
}

/// The public curve point `point` (uncompressed SEC1, `0x04 || X || Y`) as the
/// two fixed-width coordinates a JWK publishes: `x` and `y`, 32 bytes each
/// (RFC 7518 §6.2.1.2 — "the octet string representation MUST utilize all
/// octets", i.e. no leading zero is dropped).
pub(super) fn public_coordinates(key: &::ring::signature::EcdsaKeyPair) -> (Vec<u8>, Vec<u8>) {
    // SEC 1 §2.3.3: the uncompressed point is `0x04 || X || Y`, each coordinate
    // the field element's full width, which on P-256 is
    // [`P256_COORDINATE_BYTES`].
    let point = key.public_key().as_ref();
    let x = point
        .get(1..=P256_COORDINATE_BYTES)
        .unwrap_or_default()
        .to_vec();
    let y = point
        .get(1 + P256_COORDINATE_BYTES..=2 * P256_COORDINATE_BYTES)
        .unwrap_or_default()
        .to_vec();
    (x, y)
}

/// The width of one P-256 coordinate: the curve's field elements are 256 bits,
/// and RFC 7518 §6.2.1.2 publishes each as the full-width octet string.
pub(super) const P256_COORDINATE_BYTES: usize = 32;

/// The RFC 7638 JWK thumbprint of the public key whose coordinates are
/// `(x, y)`: the base64url SHA-256 of the key's canonical JWK — its required
/// members (`crv`, `kty`, `x`, `y` for an EC key) in lexicographic order, with
/// no whitespace. Two pools' keys therefore never share a `kid`, and a key's
/// `kid` is the same every time it is published.
pub(super) fn jwk_thumbprint((x, y): &(Vec<u8>, Vec<u8>)) -> String {
    let canonical = serde_json::json!({
        "crv": TOKEN_SIGNING_CURVE,
        "kty": "EC",
        "x": base64_url(x),
        "y": base64_url(y),
    })
    .to_string();
    base64_url(&::sha2::Sha256::digest(canonical.as_bytes()))
}

/// `claims` as a JWS in compact serialization (RFC 7515 §3.1) signed with
/// `key`: `base64url(header) '.' base64url(claims) '.' base64url(signature)`,
/// where the header names the algorithm and the key (`kid`) a verifier checks
/// the signature with.
///
/// `None` when the signature could not be computed, so a token is either
/// signed or not issued.
pub(super) fn signed_jws(claims: &serde_json::Value, key: &PoolSigningKey) -> Option<String> {
    let header = serde_json::json!({
        "alg": TOKEN_SIGNING_ALGORITHM,
        "kid": key.kid(),
        "typ": "JWT",
    });
    // RFC 7515 §5.1: the signature is computed over the ENCODED header and
    // payload, joined by '.', so those exact bytes are what the token carries.
    let signing_input = format!(
        "{}.{}",
        base64_url(header.to_string().as_bytes()),
        base64_url(claims.to_string().as_bytes())
    );
    let signature = key.sign(signing_input.as_bytes())?;
    Some(format!("{signing_input}.{}", base64_url(&signature)))
}
