// SPDX-License-Identifier: CC0-1.0

//! What makes a certificate presented to a private CA that CA's OWN certificate.
//!
//! `ImportCertificateAuthorityCertificate` is the one call that gives a private CA the
//! certificate it signs with, and a CA signs with the key pair IT holds — so the
//! certificate it takes has to be the one that was issued for the certification request it
//! published ([`super::derive_csr_pem`]): the same subject, over the same public key.
//! A certificate for another entity, or over another key, is the modelled mismatch
//! ([`CertificateMismatch`]), and importing it would leave the CA holding a certificate
//! whose private key this service does not have.
//!
//! The shape check above it belongs to the same surface: the certificate member of a
//! request is published with a PEM shape ([`CERTIFICATE_PEM_PATTERN`]), and a body outside
//! that shape is refused by the request-validation stage before anything tries to read a
//! certificate out of it.

use std::sync::LazyLock;

use regex::Regex;

use super::*;

/// The wire name of the member a CA certificate is presented in, as a refusal of that
/// member's own shape names it.
pub const CERTIFICATE_MEMBER: &str = "certificate";

/// The PEM shape the certificate member of a request is published with: the `CERTIFICATE`
/// armour of RFC 7468 section 5 around a base64 body of full 64-character lines and a
/// shorter last one, with either line ending and an optional final break.
///
/// The pinned model constrains the member only by length (`CertificateBodyBlob`, a blob of
/// 1..32768 bytes), while the front door publishes this expression for it — which is why a
/// body outside the shape is stated by the request-validation stage, quoting the
/// expression, rather than by the handler's own certificate decode: the decode's modelled
/// failure ([`MALFORMED_CERTIFICATE_MESSAGE`]) is for a body of the right shape that is not
/// a certificate.
pub const CERTIFICATE_PEM_PATTERN: &str = concat!(
    r"-----BEGIN CERTIFICATE-----\r?\n",
    r"([A-Za-z0-9/+]{64}\r?\n)*[A-Za-z0-9/+]{1,64}={0,2}\r?\n",
    r"-----END CERTIFICATE-----(\r?\n)?"
);

/// The expression above, anchored so it is matched against the WHOLE member value, compiled
/// once for the process rather than per request.
static CERTIFICATE_PEM_SHAPE: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(&format!("^(?:{CERTIFICATE_PEM_PATTERN})$")).ok());

/// Whether `value` is the published PEM shape of the certificate member.
///
/// A member whose bytes are not text at all is not of the shape either: the expression is
/// over characters, and nothing outside it can satisfy it.
///
/// An expression that could not be compiled admits every value, so the shape stage would
/// pass everything to the certificate decode behind it rather than refusing a request this
/// service cannot judge. The expression is a constant of this crate, never caller input, and
/// the suite's shape rows fail loudly if it stops matching
/// (`tests/all/ca_certificate_import.rs`).
pub fn is_published_certificate_pem(value: &[u8]) -> bool {
    let Some(shape) = CERTIFICATE_PEM_SHAPE.as_ref() else {
        return true;
    };
    ::std::str::from_utf8(value).is_ok_and(|text| shape.is_match(text))
}

/// What tells a presented CA certificate apart from the certification request the CA
/// published, when something does.
///
/// Both are `CertificateMismatchException`, and a caller tells them apart by the sentence:
/// one says the certificate is for another entity, the other that it is over another key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertificateMismatch {
    /// The certificate's subject is not the subject of the CA's certification request.
    Subject,
    /// The subject matches, and the certificate's `SubjectPublicKeyInfo` is not the one the
    /// CA's certification request published.
    PublicKey,
}

impl CertificateMismatch {
    /// What Amazon Web Services Private CA answers for this mismatch: one sentence naming
    /// the field of the certificate that diverges from the certification request, wholly
    /// fixed text that interpolates no value from the request or the store.
    pub fn message(self) -> &'static str {
        match self {
            Self::Subject => {
                "The certificate subject contains a different subject than the CA \
                 certificate signing request."
            }
            Self::PublicKey => {
                "The certificate subject public key info field contains a different public \
                 key or key algorithm than the CA certificate signing request."
            }
        }
    }
}

impl CertificateAuthorityRecord {
    /// The certification request this CA published ([`super::derive_csr_pem`]), decoded.
    ///
    /// `None` only if the request this service wrote cannot be read back, which is the same
    /// condition under which the CA could not publish one at all.
    pub fn own_certification_request(&self) -> Option<CertificationRequest> {
        let der = parse_pem_block(&self.csr_pem, PEM_LABEL_CSR)?;
        parse_certification_request(&der)
    }

    /// What tells `certificate` apart from this CA's own published certification request,
    /// when something does — and `None` when the certificate is the one that request asked
    /// for, so the CA may take it.
    ///
    /// The subject is judged first and the public key second, so a certificate that is for
    /// another entity is reported as that rather than as the key difference that follows
    /// from it.
    pub fn certificate_mismatch(
        &self,
        certificate: &Certificate,
    ) -> Option<CertificateMismatch> {
        let request = self.own_certification_request()?;
        if !same_distinguished_name(&request.info.subject, &certificate.tbs_certificate.subject)
        {
            return Some(CertificateMismatch::Subject);
        }
        if !same_public_key(
            &request.info.public_key,
            &certificate.tbs_certificate.subject_public_key_info,
        ) {
            return Some(CertificateMismatch::PublicKey);
        }
        None
    }
}

/// Whether two X.500 names name the same entity.
///
/// Compared as the attributes they carry rather than as the DER they were written in: the
/// same attribute types with the same values, each value matched insensitively to case and
/// to runs of whitespace (the string preparation RFC 4518 sections 2.2 and 2.6.1 defines
/// for a name comparison), and independent of which relative distinguished name an
/// attribute was grouped into. Two writers of one subject choose that grouping and that
/// order differently, and a certificate issued for a certification request is for the same
/// entity either way.
fn same_distinguished_name(left: &Name, right: &Name) -> bool {
    comparable_name(left) == comparable_name(right)
}

/// A name as the set of attributes it carries, each attribute type with its prepared value,
/// in an order that is the same for every writing of one name.
fn comparable_name(name: &Name) -> Vec<(String, String)> {
    let mut attributes: Vec<(String, String)> = name
        .0
        .iter()
        .flat_map(|rdn| rdn.0.iter())
        .map(|attribute| {
            (
                attribute.oid.to_string(),
                comparable_value(attribute.value.value()),
            )
        })
        .collect();
    attributes.sort();
    attributes
}

/// An attribute value prepared for comparison: its characters case-folded, with every run
/// of whitespace read as one space and none at either end.
fn comparable_value(value: &[u8]) -> String {
    String::from_utf8_lossy(value)
        .split_whitespace()
        .collect::<Vec<&str>>()
        .join(" ")
        .to_lowercase()
}

/// Whether two `SubjectPublicKeyInfo` structures are the same public key: the same
/// algorithm, the same algorithm parameters and the same key bits, which is exactly what
/// their DER encodings say.
fn same_public_key(left: &SubjectPublicKeyInfoOwned, right: &SubjectPublicKeyInfoOwned) -> bool {
    match (left.to_der(), right.to_der()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}
