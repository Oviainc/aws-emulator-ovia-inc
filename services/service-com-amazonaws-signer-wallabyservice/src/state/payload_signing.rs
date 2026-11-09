// SPDX-License-Identifier: CC0-1.0

//! The payload-signing data plane: what `SignPayload` signs, the key pair it signs
//! with, and what it reports beside the signature.
//!
//! `SignPayload` "signs a binary payload and returns a signature envelope"
//! (`smithy_query(query="operation", operation="SignPayload")`): the caller hands
//! over the payload itself, so nothing is read from or written to S3 and the
//! signature goes straight back in the answer. The signature is produced HERE, by
//! the in-process cryptography provider, over the caller's own payload bytes and
//! under a key pair this store holds — it is a real signature of the request's
//! payload, verifiable with the public half of that pair, not a value shaped to
//! look like one.
//!
//! The key pair belongs to the signing profile VERSION whose signatures it makes:
//! a profile signs under the material of the version it holds. The pair therefore
//! lives on that version's own record
//! ([`SigningProfileRecord::payload_signing_key`]) and needs no store of its own.
//! The private half is only ever read here, to make a signature; nothing puts it in
//! a response, a log or a file, and [`PayloadSigningKey`]'s own `Debug` keeps it out
//! of a formatted record of the store.

use std::collections::HashMap;

use aws_lc_rs::encoding::{AsDer, PublicKeyX509Der};
use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{
    EcdsaKeyPair, EcdsaSigningAlgorithm, KeyPair as _, ECDSA_P256_SHA256_ASN1_SIGNING,
    ECDSA_P384_SHA384_ASN1_SIGNING,
};

use super::SigningProfileRecord;

/// The one payload content type a payload signature is made over.
///
/// Not a chosen value: `SignPayloadRequest$payloadFormat` is documented "Payload
/// content type. The single valid type is
/// application/vnd.cncf.notary.payload.v1+json."
/// (`smithy_query(query="doc", shape="SignPayloadRequest",
/// member="payloadFormat")`), and the member is a bare string the model puts no
/// enumeration on, so the one value it admits is stated here.
pub const PAYLOAD_FORMAT: &str = "application/vnd.cncf.notary.payload.v1+json";

/// The refusal prose for a `payloadFormat` outside the one the operation signs:
/// the value the request carried, and the value it has to carry.
///
/// Worded like this crate's other refusals of a value outside a published option
/// set ([`super::unsupported_override_message`]): the member, the value it
/// carried, then what is supported.
pub fn unsupported_payload_format_message(payload_format: &str) -> String {
    format!(
        "payloadFormat {} is not supported. Supported values: [{PAYLOAD_FORMAT}]",
        super::stated_value(payload_format)
    )
}

/// The refusal prose for a payload signature asked of a profile whose lifecycle
/// status forbids it: the profile the request named, and the status the store holds
/// for it.
///
/// A profile signs only while it is `Active`
/// ([`SigningProfileRecord::can_sign`]); the state word comes from the stored
/// status, so the sentence names what a read of the profile reports rather than a
/// status written out per call site. `profileName` is bounded by its modelled shape
/// (`ProfileName`, `@length(2..64)`), so it is stated as it arrived.
///
/// Stated here rather than shared with `StartSigningJob`'s refusal of the same
/// condition: that operation words its own answer (a modelled 404 whose `code`
/// names the status), and each operation answers as its own recorded refusal does.
pub fn signing_refused_message(profile_name: &str, status: &str) -> String {
    format!("Signing profile with name: {profile_name} is in state {status}")
}

/// The key the signature's `metadata` reports the signing profile version the
/// signature was made under.
///
/// `Metadata` is a bare `map<String, String>` the model names no keys of
/// (`smithy_query(query="shape", shape="Metadata")`) and the member is documented
/// as "Information including the signing profile ARN and the signing job ID"
/// (`smithy_query(query="doc", shape="SignPayloadResponse", member="metadata")`),
/// so each key is spelled the way this service's own members spell the value it
/// carries.
pub const METADATA_KEY_PROFILE_VERSION_ARN: &str = "profileVersionArn";

/// The key the signature's `metadata` reports the signing job under — the `jobId`
/// the answer carries as a member too, so a client that kept only the metadata map
/// still has it (see [`METADATA_KEY_PROFILE_VERSION_ARN`]).
pub const METADATA_KEY_JOB_ID: &str = "jobId";

/// The `metadata` a payload signature is reported with: the profile version it was
/// made under and the signing job it was made by, both read off the stored job.
pub fn signature_metadata(profile_version_arn: &str, job_id: &str) -> HashMap<String, String> {
    HashMap::from([
        (
            METADATA_KEY_PROFILE_VERSION_ARN.to_string(),
            profile_version_arn.to_string(),
        ),
        (METADATA_KEY_JOB_ID.to_string(), job_id.to_string()),
    ])
}

/// The key pair the payload signatures of one signing profile version are made
/// under.
///
/// Held as the two DER encodings the signer and a verifier need: the private key as
/// the PKCS#8 `PrivateKeyInfo` (RFC 5208) the signature is made with, and the
/// public key as the X.509 `SubjectPublicKeyInfo` (RFC 5280 section 4.1.2.7) a
/// verifier checks that signature against.
#[derive(Clone)]
pub struct PayloadSigningKey {
    /// The ECDSA scheme this pair signs under — the curve and digest the profile's
    /// platform publishes (see [`profile_signing_algorithm`]).
    algorithm: &'static EcdsaSigningAlgorithm,
    /// The private key, as a DER PKCS#8 `PrivateKeyInfo`.
    private_key_der: Vec<u8>,
    /// The public key, as a DER X.509 `SubjectPublicKeyInfo`.
    public_key_der: Vec<u8>,
}

/// Prints the pair WITHOUT its private key: the store this sits in derives
/// `Debug`, and a private key must not reach a log or an error message through it.
impl ::std::fmt::Debug for PayloadSigningKey {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        f.debug_struct("PayloadSigningKey")
            .field("public_key_der", &self.public_key_der)
            .finish_non_exhaustive()
    }
}

impl PayloadSigningKey {
    /// The signature this key makes over `payload`, as the DER `Ecdsa-Sig-Value`
    /// of RFC 3279 section 2.2.3 — the encoding the `ASN1` schemes of the provider
    /// produce.
    ///
    /// `None` only when the provider refuses to sign, which is this emulator's own
    /// fault rather than anything about the request: the caller is then told the
    /// service failed instead of being handed bytes that are not a signature of
    /// its payload.
    pub fn signature(&self, payload: &[u8]) -> Option<Vec<u8>> {
        let key = EcdsaKeyPair::from_pkcs8(self.algorithm, &self.private_key_der).ok()?;
        let signature = key.sign(&SystemRandom::new(), payload).ok()?;
        Some(signature.as_ref().to_vec())
    }

    /// The PUBLIC half of this pair, as a DER X.509 `SubjectPublicKeyInfo`.
    ///
    /// What a verifier checks a signature this key made with; the private half
    /// never leaves this process.
    pub fn public_key_info(&self) -> &[u8] {
        &self.public_key_der
    }

    /// The signature of `payload` under this pair, with the PUBLIC half a
    /// verifier checks it against — the two halves of one answer, for a caller
    /// that writes both into a signature envelope
    /// ([`super::code_signing::signature_envelope`]).
    ///
    /// `None` for the reason [`Self::signature`] answers `None`: the provider
    /// refused to sign, so the caller reports that nothing was signed rather than
    /// handing over bytes that are not a signature.
    pub fn signature_and_public_key(&self, payload: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
        Some((self.signature(payload)?, self.public_key_der.clone()))
    }
}

/// The ECDSA scheme a profile's payload signatures are made under: the encryption
/// and hash algorithms its platform publishes, as overridden by the profile.
///
/// A platform publishes one option set per algorithm member and a `defaultValue`
/// within it ([`super::SigningPlatformEntry::signing_configuration`]), and a
/// profile's `overrides.signingConfiguration` selects among the published values —
/// which is the set `PutSigningProfile` already held the override to
/// ([`super::unsupported_signing_configuration_override`]). So the pair a
/// signature is made under is the profile's override where it carries one and the
/// platform's default otherwise.
///
/// `None` when the profile is on a platform this emulator publishes no entry for,
/// or when the pair names no scheme the provider implements
/// ([`ecdsa_signing_algorithm`]).
pub fn profile_signing_algorithm(
    profile: &SigningProfileRecord,
) -> Option<&'static EcdsaSigningAlgorithm> {
    let entry = super::signing_platform(&profile.platform_id)?;
    let configuration = profile
        .overrides
        .as_ref()
        .and_then(|overrides| overrides.signing_configuration.as_ref());
    let encryption_algorithm = configuration
        .and_then(|configuration| configuration.encryption_algorithm.as_deref())
        .or_else(|| entry.encryption_algorithms.first().copied())?;
    let hash_algorithm = configuration
        .and_then(|configuration| configuration.hash_algorithm.as_deref())
        .or_else(|| entry.hash_algorithms.first().copied())?;
    ecdsa_signing_algorithm(encryption_algorithm, hash_algorithm)
}

/// The signature scheme an (`EncryptionAlgorithm`, `HashAlgorithm`) pair names, as
/// the provider names it — or `None` for a pair it implements no scheme for.
///
/// An ECDSA signature is made on a curve, and the pair fixes it: FIPS 186-5 pairs
/// each NIST curve with the digest of its own strength, which is how the published
/// platform ids spell their algorithms too (`…-SHA384-ECDSA` on the platform whose
/// `hashAlgorithmOptions` publish `SHA384`). So `SHA256` names P-256 and `SHA384`
/// names P-384, and a pair outside that — an encryption algorithm that is not
/// ECDSA, a digest no published curve is paired with — names no scheme here.
fn ecdsa_signing_algorithm(
    encryption_algorithm: &str,
    hash_algorithm: &str,
) -> Option<&'static EcdsaSigningAlgorithm> {
    match (encryption_algorithm, hash_algorithm) {
        ("ECDSA", "SHA256") => Some(&ECDSA_P256_SHA256_ASN1_SIGNING),
        ("ECDSA", "SHA384") => Some(&ECDSA_P384_SHA384_ASN1_SIGNING),
        _ => None,
    }
}

/// A fresh key pair signing under `algorithm`, or `None` when the provider refuses
/// to mint or to encode one.
fn generate_payload_signing_key(
    algorithm: &'static EcdsaSigningAlgorithm,
) -> Option<PayloadSigningKey> {
    let private = EcdsaKeyPair::generate_pkcs8(algorithm, &SystemRandom::new()).ok()?;
    let pair = EcdsaKeyPair::from_pkcs8(algorithm, private.as_ref()).ok()?;
    let public: PublicKeyX509Der = AsDer::as_der(pair.public_key()).ok()?;
    Some(PayloadSigningKey {
        algorithm,
        private_key_der: private.as_ref().to_vec(),
        public_key_der: public.as_ref().to_vec(),
    })
}

impl SigningProfileRecord {
    /// The key pair the payload signatures of THIS version of the profile are made
    /// under, minting it when this version has signed nothing yet.
    ///
    /// Minted on first use and then kept for as long as the version's record lives:
    /// every signature of one profile version is made under one pair, so a client
    /// holding the public half can verify every signature that version made.
    ///
    /// `None` when this profile's platform names no signature scheme the provider
    /// implements ([`profile_signing_algorithm`]): there is then no key to sign
    /// with, and the caller is told the service failed rather than handed bytes
    /// that are not a signature.
    pub fn payload_signing_key(&mut self) -> Option<&PayloadSigningKey> {
        if self.payload_signing_key.is_none() {
            self.payload_signing_key =
                Some(generate_payload_signing_key(profile_signing_algorithm(self)?)?);
        }
        self.payload_signing_key.as_ref()
    }

    /// The key pair the signatures of THIS version of the profile are made under,
    /// taken OUT of the record — minting it when this version has signed nothing
    /// yet.
    ///
    /// For a caller that has to sign without the store guard held, which a
    /// code-signing job does: signing a whole object is CPU-bound work, and
    /// running it while holding the lock over shared state would make one
    /// caller's object the latency of every other caller's request
    /// ([`super::code_signing::signed_artifact`] is where it runs instead). The
    /// copy stays inside this process for the length of that one job: nothing
    /// puts it in a response, a log or a file, and [`PayloadSigningKey`]'s own
    /// `Debug` keeps the private half out of a formatted record.
    ///
    /// `None` for the reason [`Self::payload_signing_key`] answers `None`: this
    /// profile's platform names no signature scheme the provider implements, so
    /// there is no key to sign with.
    pub fn signing_key_copy(&mut self) -> Option<PayloadSigningKey> {
        self.payload_signing_key().cloned()
    }
}
