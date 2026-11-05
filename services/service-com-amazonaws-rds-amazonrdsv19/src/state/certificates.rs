// SPDX-License-Identifier: CC0-1.0

use super::*;

/// One CA certificate the service publishes for a DB instance's TLS endpoint.
///
/// Source: the certificate authorities Amazon RDS publishes in "Using SSL/TLS to
/// encrypt a connection to a DB instance" (Amazon RDS User Guide) and reports through
/// `DescribeCertificates`, each row carrying the `notBefore`/`notAfter` instants and the
/// SHA-1 fingerprint of the certificate that row NAMES. A validity endpoint is the
/// certificate's own instant, to the second: the window belongs to the certificate
/// material, so it is never a calendar day with the clock zeroed, and two authorities
/// signed at different moments report different windows.
#[derive(Debug, Clone, Copy)]
pub struct CaCertificate {
    /// `Certificate$CertificateIdentifier` — "The unique key that identifies a
    /// certificate."
    pub identifier: &'static str,
    /// `Certificate$CertificateType` — the type AWS reports for every published CA.
    pub certificate_type: &'static str,
    /// `Certificate$ValidFrom` — "The starting date from which the certificate is valid":
    /// the `notBefore` of the certificate this row names, as an ISO-8601 instant.
    pub valid_from: &'static str,
    /// `Certificate$ValidTill` — "The final date that the certificate continues to be
    /// valid": the `notAfter` of the certificate this row names, as an ISO-8601 instant.
    pub valid_till: &'static str,
    /// `Certificate$Thumbprint` — "The thumbprint of the certificate", the SHA-1
    /// fingerprint of the published authority's certificate, as the published catalogue
    /// carries it. EMPTY for an authority whose fingerprint this emulator does not hold:
    /// a fingerprint is a digest of certificate bytes, so one that is not published is
    /// reported as no `Thumbprint` member at all rather than as an invented digest.
    pub thumbprint: &'static str,
    /// The key this authority signs with, which the identifier itself names
    /// (`rds-ca-rsa2048-g1`, `rds-ca-ecc384-g1`).
    pub key: CertificateKey,
}

/// The public-key algorithm a published certificate authority signs with.
///
/// It is a property of the authority that an ENGINE's report reads twice: an engine
/// version publishes the authorities it can be launched under in one order — the
/// elliptic-curve authority, then the RSA ones from the longest key to the shortest — and
/// an engine whose endpoint serves no ECDSA certificate publishes none of the first kind
/// (see [`ca_certificates_offered_to`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertificateKey {
    /// An ECDSA authority, over a curve of this many bits.
    EllipticCurve(u32),
    /// An RSA authority, with a modulus of this many bits.
    Rsa(u32),
}

impl CertificateKey {
    /// The order an engine version publishes its supported authorities in: every
    /// elliptic-curve authority before every RSA one, and within a kind the longest key
    /// first.
    pub(super) fn publication_rank(self) -> (u8, std::cmp::Reverse<u32>) {
        match self {
            Self::EllipticCurve(bits) => (0, std::cmp::Reverse(bits)),
            Self::Rsa(bits) => (1, std::cmp::Reverse(bits)),
        }
    }
}

impl CaCertificate {
    /// This catalog entry as the modelled `Certificate`, with the ARN scoped to the
    /// region the request was signed for and the override members answered from
    /// `override_identifier` — the `CertificateIdentifier` the account's
    /// `ModifyCertificates` override names in that region, or `None` where it set none.
    pub fn to_wire(&self, region: &str, override_identifier: Option<&str>) -> Certificate {
        // "Indicates whether there is an override for the default certificate
        // identifier": true for the ONE certificate the override pins, false for every
        // other published certificate. The comparison is case-insensitive because
        // `ModifyCertificates`/`DescribeCertificates` accept the identifier in any case.
        let overridden =
            override_identifier.is_some_and(|pinned| pinned.eq_ignore_ascii_case(self.identifier));
        Certificate {
            certificate_identifier: Some(self.identifier.to_string()),
            certificate_type: Some(self.certificate_type.to_string()),
            // A fingerprint the catalogue does not carry is reported as an absent
            // member, never as an empty string: a client pinning a thumbprint must be
            // able to tell "not published" from a digest.
            thumbprint: (!self.thumbprint.is_empty()).then(|| self.thumbprint.to_string()),
            valid_from: catalog_instant(self.valid_from),
            valid_till: catalog_instant(self.valid_till),
            certificate_arn: Some(ca_certificate_arn(region, self.identifier)),
            customer_override: Some(overridden),
            // `CustomerOverrideValidTill`: "If there is an override for the default
            // certificate identifier, when the override expires." The override pins THIS
            // certificate, so it cannot outlive it: the override expires when the
            // certificate it names does, and a certificate under no override reports no
            // expiry at all.
            customer_override_valid_till: overridden
                .then(|| catalog_instant(self.valid_till))
                .flatten(),
        }
    }
}

/// The ARN of a published CA certificate: `arn:aws:rds:<region>::cert:<identifier>`.
///
/// The account field is EMPTY, unlike every other ARN this service builds (through
/// `emulator_server::emulator_arn`, which stamps the emulator's account id): a CA
/// certificate is published BY Amazon RDS rather than created by an account, so there is
/// no account that owns it to name — the same shape AWS gives the certificate ARNs
/// `DescribeCertificates` reports. It is still region-scoped, because the certificate
/// authority a DB instance is launched under is reported per Region.
pub fn ca_certificate_arn(region: &str, identifier: &str) -> String {
    format!("arn:aws:{ARN_NAMESPACE}:{region}::cert:{identifier}")
}

/// The published CA certificate `identifier` names, or `None` when no published
/// certificate carries it.
///
/// `DescribeCertificates$CertificateIdentifier`: "This parameter isn't case-sensitive",
/// so the match ignores case — and `ModifyCertificates` resolves the identifier it is
/// given through this same lookup, so the two operations cannot disagree about which
/// identifiers exist.
pub fn ca_certificate(identifier: &str) -> Option<&'static CaCertificate> {
    CA_CERTIFICATES
        .iter()
        .find(|certificate| certificate.identifier.eq_ignore_ascii_case(identifier))
}

/// Every CA certificate `DescribeCertificates` reports, in the order the service
/// publishes them: the strongest authority first — every elliptic-curve authority before
/// every RSA one, and within a kind the longest key first ([`CertificateKey`]).
///
/// The rows are the authorities the service PROVIDES, each carrying its own certificate's
/// `notBefore`/`notAfter` and SHA-1 fingerprint. An authority whose certificate has
/// expired is no longer provided for a database's TLS endpoint and so is not published at
/// all — the retired 2019 authority, whose certificate expired in August 2024, is not a
/// row here; an authority AWS publishes or retires is a row added or removed.
pub const CA_CERTIFICATES: &[CaCertificate] = &[
    CaCertificate {
        identifier: "rds-ca-ecc384-g1",
        certificate_type: "CA",
        valid_from: "2021-05-24T22:06:59Z",
        valid_till: "2121-05-24T23:06:59Z",
        thumbprint: "2ee3dcc06e50192559b13929e73484354f23387d",
        key: CertificateKey::EllipticCurve(384),
    },
    CaCertificate {
        identifier: "rds-ca-rsa4096-g1",
        certificate_type: "CA",
        valid_from: "2021-05-24T22:03:20Z",
        valid_till: "2121-05-24T23:03:20Z",
        thumbprint: "19da4f2af579a8ae1f6a0fa77aa5befd874b4cab",
        key: CertificateKey::Rsa(4096),
    },
    CaCertificate {
        identifier: "rds-ca-rsa2048-g1",
        certificate_type: "CA",
        valid_from: "2021-05-24T21:59:00Z",
        valid_till: "2061-05-24T22:59:00Z",
        thumbprint: "7c40cb42714b6fdb2b296f9bbd0e8bb364436a76",
        key: CertificateKey::Rsa(2048),
    },
];

/// The CA certificate identifiers an engine version can be launched with:
/// `DBEngineVersion$SupportedCACertificateIdentifiers`, "a list of the supported CA
/// certificate identifiers".
///
/// Read from the ONE certificate catalogue this service reports through
/// `DescribeCertificates` ([`CA_CERTIFICATES`]), so the two reports cannot name different
/// certificates — minus any entry whose `ValidTill` has passed: a caller picks the
/// `CACertificateIdentifier` of a new instance out of this list, and an expired
/// certificate is not something it can be launched with. The order is the one an engine
/// version publishes its authorities in ([`CertificateKey::publication_rank`]), which the
/// catalogue is itself held in, so dropping the expired entries cannot reorder what is
/// left.
///
/// Which of these a given engine publishes is [`ca_certificates_offered_to`]: this is the
/// whole set, for an engine offered every authority.
pub fn supported_ca_certificate_identifiers() -> Vec<String> {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_secs() as i64);
    let mut launchable: Vec<&CaCertificate> = CA_CERTIFICATES
        .iter()
        .filter(|certificate| {
            catalog_instant(certificate.valid_till).is_some_and(|till| till.secs() > now)
        })
        .collect();
    launchable.sort_by_key(|certificate| certificate.key.publication_rank());
    launchable
        .into_iter()
        .map(|certificate| certificate.identifier.to_string())
        .collect()
}

/// The authorities among `published` that a version of `engine` can be launched under, in
/// the order `published` holds them.
///
/// The supported-authority list is a property of the ENGINE, not of the catalogue: the
/// Aurora engines publish the service's elliptic-curve authority beside the RSA ones,
/// while an engine whose endpoint serves only an RSA certificate publishes only those —
/// which is what the recorded wire reports for the DB instance engines and for the
/// Amazon DocumentDB cluster engine. A caller reads this list to choose the
/// `CACertificateIdentifier` of a new instance or cluster, so naming an authority the
/// engine cannot be launched under would be naming a value its create refuses.
pub fn ca_certificates_offered_to(engine: &str, published: &[String]) -> Vec<String> {
    published
        .iter()
        .filter(|identifier| {
            ca_certificate(identifier).is_some_and(|certificate| match certificate.key {
                CertificateKey::EllipticCurve(_) => is_aurora(engine),
                CertificateKey::Rsa(_) => true,
            })
        })
        .cloned()
        .collect()
}

/// One of the catalogs' fixed ISO-8601 instants as the modelled timestamp shapes carry
/// it.
///
/// The input is a literal in a catalog, never caller data, so a value that is not an
/// instant is a typo in that catalog: it panics at the first read rather than
/// silently dropping a timestamp member from the response.
///
/// `pub` because a reference catalog an OPERATION owns carries the same kind of fixed
/// date — the engine-version support lifecycles
/// (`crate::ops::describe_db_major_engine_versions`) are published as calendar days —
/// and the parse and its panic belong in one place for every catalog in this service.
pub fn catalog_timestamp(iso: &str) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_str(iso, ::aws_smithy_types::date_time::Format::DateTime)
        .expect("every catalog instant in this service is an ISO-8601 literal")
}

/// [`catalog_timestamp`] for an OPTIONAL timestamp member, which is how the catalogs
/// above project `ValidFrom`/`ValidTill`.
pub(super) fn catalog_instant(iso: &str) -> Option<::aws_smithy_types::DateTime> {
    Some(catalog_timestamp(iso))
}
