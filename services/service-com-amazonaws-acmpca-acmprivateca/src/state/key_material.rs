// SPDX-License-Identifier: CC0-1.0

//! The key pair a private certificate authority signs with.
//!
//! A private CA is a key pair plus the certificate that vouches for it: the certification
//! request it publishes carries that key pair's PUBLIC key and is signed with its private
//! key, and every certificate and revocation list the CA puts out afterwards is signed with
//! the same private key. So the key pair is minted once, when the CA is created
//! ([`generate_key_pair`]), and lives on the CA's record for as long as the CA does.
//!
//! The key never leaves this process: the public half goes out inside the structures the
//! service hands back (`GetCertificateAuthorityCsr`, `GetCertificate`), and the private half
//! is only ever read here, to make a signature. Nothing writes it to a response, a log or a
//! file — [`CaKeyPair`]'s own `Debug` keeps it out of a formatted record as well.
//!
//! The algorithms are the `KeyAlgorithm` members the in-process cryptography provider can
//! mint and sign with: the RSA and the NIST-curve ones. For a member it cannot
//! ([`generate_key_pair`] answers `None`) the CA holds no key pair and the structures it
//! publishes carry the stand-in public key and signature `super` builds, as they did before
//! any CA held a key at all. One narrower limit sits beside it: a CA created with a curve
//! and a digest that is not that curve's own is given a real key pair, and the signatures it
//! makes are the stand-in, because the provider offers no algorithm for that pairing
//! ([`ecdsa_signing_algorithm`]).

use aws_lc_rs::encoding::{AsDer, Pkcs8V1Der, PublicKeyX509Der};
use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::rsa::KeySize;
use aws_lc_rs::signature::{
    EcdsaKeyPair, EcdsaSigningAlgorithm, KeyPair as _, RsaEncoding, RsaKeyPair,
    ECDSA_P256_SHA256_ASN1_SIGNING, ECDSA_P384_SHA384_ASN1_SIGNING,
    ECDSA_P521_SHA384_ASN1_SIGNING, ECDSA_P521_SHA512_ASN1_SIGNING, RSA_PKCS1_SHA256,
    RSA_PKCS1_SHA384, RSA_PKCS1_SHA512, RSA_PSS_SHA256, RSA_PSS_SHA384, RSA_PSS_SHA512,
};

use super::*;

/// The key pair one private CA holds.
///
/// Held as the two DER encodings the structures around it need: the public key as the
/// `SubjectPublicKeyInfo` a certification request and a certificate carry (RFC 5280 section
/// 4.1.2.7), and the private key as the PKCS#8 `PrivateKeyInfo` (RFC 5208) the signer reads
/// it back from.
#[derive(Clone)]
pub struct CaKeyPair {
    /// The `KeyAlgorithm` member this pair was minted for.
    key_algorithm: String,
    /// The public key, as a DER `SubjectPublicKeyInfo`.
    public_key_der: Vec<u8>,
    /// The private key, as a DER PKCS#8 `PrivateKeyInfo`.
    private_key_der: Vec<u8>,
}

/// Prints the pair WITHOUT its private key: the record this sits on derives `Debug`, and a
/// private key must not reach a log or an error message through it.
impl ::std::fmt::Debug for CaKeyPair {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        f.debug_struct("CaKeyPair")
            .field("key_algorithm", &self.key_algorithm)
            .field("public_key_der", &self.public_key_der)
            .finish_non_exhaustive()
    }
}

impl CaKeyPair {
    /// The public half, as the `SubjectPublicKeyInfo` a certification request or a
    /// certificate carries.
    pub fn public_key_info(&self) -> Option<SubjectPublicKeyInfoOwned> {
        SubjectPublicKeyInfoOwned::from_der(&self.public_key_der).ok()
    }

    /// The signature this key makes over `message` under the modelled `SigningAlgorithm`
    /// `signing_algorithm`, or `None` for a pairing it cannot make one with.
    ///
    /// The algorithm has to be one of this key's family — which is the rule
    /// `CreateCertificateAuthority` and `IssueCertificate` already hold a request to
    /// ([`algorithm_family`]) — and one the provider implements for this key's curve.
    pub fn signature(&self, signing_algorithm: &str, message: &[u8]) -> Option<Vec<u8>> {
        match algorithm_family(&self.key_algorithm)? {
            "RSA" => self.rsa_signature(rsa_encoding(signing_algorithm)?, message),
            "EC" => self.ecdsa_signature(
                ecdsa_signing_algorithm(&self.key_algorithm, signing_algorithm)?,
                message,
            ),
            _ => None,
        }
    }

    /// An RSA signature over `message` under `padding`.
    fn rsa_signature(
        &self,
        padding: &'static dyn RsaEncoding,
        message: &[u8],
    ) -> Option<Vec<u8>> {
        let key = RsaKeyPair::from_pkcs8(&self.private_key_der).ok()?;
        let mut signature = vec![0u8; key.public_modulus_len()];
        key.sign(padding, &SystemRandom::new(), message, &mut signature)
            .ok()?;
        Some(signature)
    }

    /// An ECDSA signature over `message`, as the DER `Ecdsa-Sig-Value` a certificate's
    /// signature field carries (RFC 5480 section 2.1.1).
    fn ecdsa_signature(
        &self,
        algorithm: &'static EcdsaSigningAlgorithm,
        message: &[u8],
    ) -> Option<Vec<u8>> {
        let key = EcdsaKeyPair::from_pkcs8(algorithm, &self.private_key_der).ok()?;
        let signature = key.sign(&SystemRandom::new(), message).ok()?;
        Some(signature.as_ref().to_vec())
    }
}

/// A new key pair for a CA created with `key_algorithm`, or `None` for a member the
/// in-process provider cannot mint.
///
/// Called once per CA, by `CreateCertificateAuthority`: the pair is what the CA's published
/// certification request is over, so re-minting it would change the request a caller has
/// already taken away to be signed.
pub fn generate_key_pair(key_algorithm: &str) -> Option<CaKeyPair> {
    // The `KeyAlgorithm` members, from `KEY_ALGORITHM_VALUES` in `ops::types`: the RSA
    // sizes and the three NIST curves are the ones the provider mints; `SM2` and the
    // `ML_DSA_*` members are the ones it does not.
    let (public_key_der, private_key_der) = match key_algorithm {
        "RSA_2048" => rsa_key_pair(KeySize::Rsa2048)?,
        "RSA_3072" => rsa_key_pair(KeySize::Rsa3072)?,
        "RSA_4096" => rsa_key_pair(KeySize::Rsa4096)?,
        "EC_prime256v1" => ecdsa_key_pair(&ECDSA_P256_SHA256_ASN1_SIGNING)?,
        "EC_secp384r1" => ecdsa_key_pair(&ECDSA_P384_SHA384_ASN1_SIGNING)?,
        "EC_secp521r1" => ecdsa_key_pair(&ECDSA_P521_SHA512_ASN1_SIGNING)?,
        _ => return None,
    };
    Some(CaKeyPair {
        key_algorithm: key_algorithm.to_string(),
        public_key_der,
        private_key_der,
    })
}

/// A fresh RSA pair of `size`, as (`SubjectPublicKeyInfo`, PKCS#8) DER.
fn rsa_key_pair(size: KeySize) -> Option<(Vec<u8>, Vec<u8>)> {
    let key = RsaKeyPair::generate(size).ok()?;
    let public: PublicKeyX509Der = AsDer::as_der(key.public_key()).ok()?;
    let private: Pkcs8V1Der = AsDer::as_der(&key).ok()?;
    Some((public.as_ref().to_vec(), private.as_ref().to_vec()))
}

/// A fresh ECDSA pair on the curve `algorithm` names, as (`SubjectPublicKeyInfo`, PKCS#8)
/// DER.
fn ecdsa_key_pair(
    algorithm: &'static EcdsaSigningAlgorithm,
) -> Option<(Vec<u8>, Vec<u8>)> {
    let private = EcdsaKeyPair::generate_pkcs8(algorithm, &SystemRandom::new()).ok()?;
    let key = EcdsaKeyPair::from_pkcs8(algorithm, private.as_ref()).ok()?;
    let public: PublicKeyX509Der = AsDer::as_der(key.public_key()).ok()?;
    Some((public.as_ref().to_vec(), private.as_ref().to_vec()))
}

/// The RSA padding a modelled `SigningAlgorithm` names: PKCS#1 v1.5 for the `WITHRSA`
/// members (RFC 8017 section 8.2), PSS for the `WITHRSA_PSS` ones (section 8.1), each with
/// the digest the member names.
fn rsa_encoding(signing_algorithm: &str) -> Option<&'static dyn RsaEncoding> {
    Some(match signing_algorithm {
        "SHA256WITHRSA" => &RSA_PKCS1_SHA256,
        "SHA384WITHRSA" => &RSA_PKCS1_SHA384,
        "SHA512WITHRSA" => &RSA_PKCS1_SHA512,
        "SHA256WITHRSA_PSS" => &RSA_PSS_SHA256,
        "SHA384WITHRSA_PSS" => &RSA_PSS_SHA384,
        "SHA512WITHRSA_PSS" => &RSA_PSS_SHA512,
        _ => return None,
    })
}

/// The ECDSA signing algorithm a `KeyAlgorithm` curve and a `SigningAlgorithm` digest name
/// together, or `None` for a pairing the provider implements no algorithm for.
///
/// Every curve/digest pairing the in-process provider offers is listed; the ones it does
/// not offer are the pairings of a curve with a digest that is not its own (`EC_prime256v1`
/// with `SHA384WITHECDSA`, say), which `CreateCertificateAuthority` accepts because the
/// rule it holds a configuration to is the algorithm FAMILY. Such a CA holds a real key
/// pair, publishes a real public key and issues certificates naming the algorithm the
/// caller asked for, and the signature on them is the stand-in: it is the one limit of this
/// module beside the key algorithms it mints no pair for at all (see the module docs), and
/// it is why nothing in this emulator verifies a signature it did not make.
fn ecdsa_signing_algorithm(
    key_algorithm: &str,
    signing_algorithm: &str,
) -> Option<&'static EcdsaSigningAlgorithm> {
    Some(match (key_algorithm, signing_algorithm) {
        ("EC_prime256v1", "SHA256WITHECDSA") => &ECDSA_P256_SHA256_ASN1_SIGNING,
        ("EC_secp384r1", "SHA384WITHECDSA") => &ECDSA_P384_SHA384_ASN1_SIGNING,
        ("EC_secp521r1", "SHA384WITHECDSA") => &ECDSA_P521_SHA384_ASN1_SIGNING,
        ("EC_secp521r1", "SHA512WITHECDSA") => &ECDSA_P521_SHA512_ASN1_SIGNING,
        _ => return None,
    })
}
