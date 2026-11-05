// SPDX-License-Identifier: CC0-1.0

//! Stored SSL/TLS certificates: what the account requested for a set of domain
//! names, how its validation stands, and when it may be attached to a
//! distribution.

use super::*;

/// The `resourceType` a certificate and its operations report, from the modelled
/// `ResourceType` enum.
pub const RESOURCE_TYPE_CERTIFICATE: &str = "Certificate";

/// The `resourceType` a content delivery network distribution reports, from the
/// modelled `ResourceType` enum — the value the
/// `LightsailDistribution$resourceType` documentation names ("The Lightsail
/// resource type (`Distribution`)").
pub const RESOURCE_TYPE_DISTRIBUTION: &str = "Distribution";

/// The resource portion a certificate ARN carries before its id. Mirrors the
/// `Instance/<uuid>` form the modelled `Instance$arn` documentation shows, with
/// this resource's own `ResourceType` value.
pub const CERTIFICATE_ARN_RESOURCE_PREFIX: &str = "Certificate/";

/// The `CertificateStatus` a certificate this account just requested is in, from
/// the modelled enum: Lightsail has to validate the domain before the
/// certificate authority issues it.
pub const CERTIFICATE_STATUS_PENDING_VALIDATION: &str = "PENDING_VALIDATION";

/// The `CertificateStatus` of a validated certificate, from the modelled enum.
/// `AttachCertificateToDistribution`'s `certificateName` documentation says
/// "Only certificates with a status of ISSUED can be attached to a
/// distribution".
pub const CERTIFICATE_STATUS_ISSUED: &str = "ISSUED";

/// The `CertificateDomainValidationStatus` of a domain whose validation is still
/// outstanding, from the modelled enum.
pub const DOMAIN_VALIDATION_STATUS_PENDING: &str = "PENDING_VALIDATION";

/// The `CertificateDomainValidationStatus` of a validated domain, from the
/// modelled enum.
pub const DOMAIN_VALIDATION_STATUS_SUCCESS: &str = "SUCCESS";

/// The algorithm a Lightsail certificate's key pair is generated with, as
/// `Certificate$keyAlgorithm` reports it.
///
/// A Lightsail SSL/TLS certificate is an AWS Certificate Manager certificate,
/// and AWS documents `RSA_2048` as the key algorithm ACM requests a certificate
/// with unless the caller asks for another — and Lightsail's `CreateCertificate`
/// takes no key-algorithm parameter, so there is no other to ask for.
pub const CERTIFICATE_KEY_ALGORITHM: &str = "RSA_2048";

/// The certificate authority that issued the certificate, as
/// `Certificate$issuerCA` reports it: Amazon's own, because Lightsail requests
/// the certificate from AWS Certificate Manager rather than from a CA the caller
/// names.
pub const CERTIFICATE_ISSUER_CA: &str = "Amazon";

/// How long an issued certificate stays valid: `notAfter` is `notBefore` plus
/// this span.
///
/// A DOCUMENTED duration, not a chosen one: AWS documents that "ACM
/// certificates are valid for 13 months (395 days)", and a Lightsail
/// certificate is an ACM certificate.
pub const CERTIFICATE_VALIDITY_MILLIS: i64 = 395 * 24 * 60 * 60 * 1_000;

/// The most alternate domains one certificate may carry.
///
/// A DOCUMENTED limit, not a guard: the modelled
/// `CreateCertificate$subjectAlternativeNames` documentation says "You can
/// specify a maximum of nine alternate domains (in addition to the primary
/// domain name)".
pub const MAX_SUBJECT_ALTERNATIVE_NAMES: usize = 9;

/// The alternate domains an account's SSL/TLS certificate covers for the
/// `domainName`/`subjectAlternativeNames` pair a request asked for, or the
/// refusal message that list earned.
///
/// The rule is [`requested_alternative_domain_names`]' — the one owner of it
/// for every certificate this store holds — with this certificate kind's own
/// member name and its own documented maximum: a name the list repeats, and a
/// name that already IS the certificate's primary domain, add no domain to the
/// certificate, so the documented maximum is applied to what the de-duping
/// leaves and what is STORED is that reduced list.
///
/// The primary domain is validated by the caller, which has its own
/// `domainName` refusal to answer with.
pub fn certificate_subject_alternative_names(
    domain_name: &str,
    requested: Vec<String>,
) -> Result<Vec<String>, String> {
    requested_alternative_domain_names(
        "subjectAlternativeNames",
        domain_name,
        requested,
        MAX_SUBJECT_ALTERNATIVE_NAMES,
    )
}

/// The most SSL/TLS certificates this emulator will hold.
///
/// Emulator resource guard, NOT an AWS quota: a create that would exceed it is
/// refused with the modelled `InvalidInputException`.
pub const MAX_CERTIFICATES: usize = 500;

/// How many certificates one `GetCertificates` page carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation, only that the
/// response carries a `nextPageToken` when more certificates remain.
pub const CERTIFICATE_PAGE_SIZE: usize = 10;

/// The ARN of the certificate `id`. A certificate, like the distribution it is
/// attached to, lives in [`DISTRIBUTION_REGION_NAME`] — "only certificates
/// created in the us-east-1 Amazon Web Services Region can be attached to
/// Lightsail distributions" — so that is the Region its ARN carries.
pub fn certificate_arn(id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        DISTRIBUTION_REGION_NAME,
        &format!("{CERTIFICATE_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The serial number of the certificate minted as `id`, as
/// `Certificate$serialNumber` reports it.
///
/// An X.509 serial number is printed as colon-separated hex byte pairs, and this
/// one is built from the certificate's own minted id — so it identifies that one
/// certificate and is the same on every later read. An odd trailing digit is
/// dropped, so every group is a whole byte; a minted uuid carries 32 hex digits,
/// which the assertion below pins.
pub fn certificate_serial_number(id: &str) -> String {
    let mut digits: Vec<char> = id.chars().filter(char::is_ascii_hexdigit).collect();
    debug_assert!(
        digits.len() >= 2,
        "a certificate id carries at least one hex byte, got {id:?}"
    );
    digits.truncate(digits.len() - digits.len() % 2);
    digits
        .chunks(2)
        .map(|pair| pair.iter().collect::<String>())
        .collect::<Vec<String>>()
        .join(":")
}

/// Why the certificate a request named is not one the account may secure
/// traffic with.
///
/// The rule is the same whichever operation records the reference — the create
/// that names a certificate on a new distribution, the attach that sets one
/// later, the container service that keys its public domain names by one, or the
/// attach that puts one of a load balancer's certificates in front of its HTTPS
/// traffic — so it is owned here, and each handler only renders the refusal in
/// its own error channel. See
/// [`ComAmazonawsLightsailLightsail20161128State::issued_certificate`] for the
/// certificates of the account and [`StoredLoadBalancer::attach_certificate`]
/// for the ones a load balancer holds.
#[derive(Debug, Clone)]
pub enum CertificateNotUsable {
    /// This account holds no certificate of that name: Lightsail's
    /// `DoesNotExist` envelope, built through
    /// [`crate::scaffolded::Scaffolded::resource_does_not_exist`] with
    /// [`RESOURCE_TYPE_CERTIFICATE`].
    DoesNotExist,
    /// The certificate exists but the certificate authority has not issued it,
    /// so it secures nothing. The variant carries the status the record is in —
    /// a `CertificateStatus`, or the `LoadBalancerTlsCertificateStatus` of a
    /// certificate a load balancer holds — which is what the refusing
    /// operation's message names: see
    /// [`certificate_not_issued_for_distribution`],
    /// [`certificate_not_issued_for_load_balancer`] and
    /// [`certificate_not_issued_for_public_domain_names`].
    NotIssued { status: &'static str },
}

/// Lightsail's message for a request that would make a certificate the
/// certificate authority has not issued the one a distribution serves HTTPS
/// traffic with: "Only certificates with a status of ISSUED can be attached to a
/// distribution" (the modelled
/// `AttachCertificateToDistribution$certificateName` documentation).
pub fn certificate_not_issued_for_distribution(name: &str, status: &str) -> String {
    format!(
        "The certificate {name} is {status}: only certificates with a status of \
         {CERTIFICATE_STATUS_ISSUED} can be attached to a distribution."
    )
}

/// Lightsail's message for a request that would make a certificate the
/// certificate authority has not issued the one a load balancer terminates
/// HTTPS with: "Once you create and validate your certificate, you can attach
/// it to your load balancer" (the modelled
/// `AttachLoadBalancerTlsCertificate` documentation).
pub fn certificate_not_issued_for_load_balancer(name: &str, status: &str) -> String {
    format!(
        "The certificate {name} is {status}: once you create and validate your certificate, \
         you can attach it to your load balancer."
    )
}

/// Lightsail's message for a request that would key a container service's public
/// domain names by a certificate the certificate authority has not issued: "You
/// must create and validate an SSL/TLS certificate before you can use public
/// domain names with your container service" (the modelled
/// `CreateContainerServiceRequest$publicDomainNames` documentation).
pub fn certificate_not_issued_for_public_domain_names(name: &str, status: &str) -> String {
    format!(
        "The certificate {name} is {status}: you must create and validate an SSL/TLS \
         certificate before you can use public domain names with your container service."
    )
}

/// One stored SSL/TLS certificate: the request the caller made, the members the
/// service assigned, and the validation state it is in.
#[derive(Debug, Clone)]
pub struct StoredCertificate {
    /// The certificate name the request named, kept verbatim — it is this
    /// record's store key and the name every certificate operation addresses it
    /// by.
    pub name: String,
    pub arn: String,
    pub support_code: String,
    pub created_at_ms: i64,
    /// Where the certificate lives — always [`DISTRIBUTION_REGION_NAME`], for
    /// the reason that constant gives.
    ///
    /// Kept on the record rather than re-derived: the modelled `Certificate`
    /// carries no location member, but every operation that LOGS about this
    /// certificate has to locate its entry where the resource is, and this is
    /// where that location comes from.
    pub location: StoredLocation,
    pub tags: Vec<Tag>,
    /// The domain the certificate is for, kept as the request asked.
    pub domain_name: String,
    /// The alternate domains and subdomains the certificate also covers, as
    /// [`alternative_domain_names`] reduced them — every writer of this field
    /// goes through that one owner (see
    /// [`certificate_subject_alternative_names`]), so a repeat and the primary
    /// domain are never among them — in the order the request first named
    /// them.
    pub subject_alternative_names: Vec<String>,
    pub serial_number: String,
    /// The current `CertificateStatus`.
    pub status: &'static str,
    /// The status the clock owes this certificate, while one is outstanding.
    pub due: Option<DueNamedState>,
    /// The instant the certificate authority issues this certificate — the same
    /// instant the clock owes it [`CERTIFICATE_STATUS_ISSUED`] — which is what
    /// `issuedAt`, `notBefore` and `notAfter` are reported from once it is
    /// issued.
    pub issued_at_ms: i64,
}

impl StoredCertificate {
    /// Whether the certificate authority has issued this certificate.
    pub fn is_issued(&self) -> bool {
        self.status == CERTIFICATE_STATUS_ISSUED
    }

    /// Every domain this certificate covers: the domain it is for, then each
    /// alternate domain it also covers.
    ///
    /// The alternate domains went through [`alternative_domain_names`] before
    /// they were stored, so none of them repeats and none of them is the
    /// primary domain — this list is the set of domains the certificate secures
    /// traffic for, with its primary domain first.
    ///
    /// This is what a distribution the certificate is attached to reports as its
    /// `alternativeDomainNames`: "After the certificate is attached, your
    /// distribution accepts HTTPS traffic for all of the domains that are
    /// associated with the certificate."
    pub fn domain_names(&self) -> Vec<String> {
        let mut names = vec![self.domain_name.clone()];
        names.extend(self.subject_alternative_names.iter().cloned());
        names
    }

    /// This certificate as the attachment a distribution serves HTTPS traffic
    /// through: the name the distribution reports as its `certificateName`,
    /// together with every domain this certificate covers.
    ///
    /// Read off ONE certificate at once, so the two halves of the attachment
    /// cannot come from different places, and applied by
    /// [`StoredDistribution::attach_certificate`].
    pub fn distribution_attachment(&self) -> DistributionCertificateAttachment {
        DistributionCertificateAttachment {
            certificate_name: self.name.clone(),
            alternative_domain_names: self.domain_names(),
        }
    }

    /// This record as the modelled `CertificateSummary`.
    ///
    /// `in_use_resource_count` is how many resources the certificate is attached
    /// to, which only the whole store knows — see
    /// [`ComAmazonawsLightsailLightsail20161128State::certificate_in_use_resource_count`].
    ///
    /// `include_details` is `GetCertificates`'s own
    /// `includeCertificateDetails`: without it "the response will include only
    /// the certificate Amazon Resource Name (ARN), certificate name, domain name,
    /// and tags".
    pub fn view(&self, in_use_resource_count: i32, include_details: bool) -> CertificateSummary {
        CertificateSummary {
            certificate_arn: Some(self.arn.clone()),
            certificate_name: Some(self.name.clone()),
            domain_name: Some(self.domain_name.clone()),
            certificate_detail: include_details.then(|| self.detail(in_use_resource_count)),
            tags: Some(self.tags.clone()),
        }
    }

    /// This record as the modelled `Certificate`, the full detail of the
    /// certificate.
    pub fn detail(&self, in_use_resource_count: i32) -> Certificate {
        let issued = self.is_issued();
        Certificate {
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            domain_name: Some(self.domain_name.clone()),
            status: Some(self.status.to_string()),
            serial_number: Some(self.serial_number.clone()),
            subject_alternative_names: Some(self.subject_alternative_names.clone()),
            // One record per domain the certificate covers. This emulator owns
            // no DNS zone to publish a validation CNAME in, so a record carries
            // the domain it is about and the state its validation is in rather
            // than a `resourceRecord` nobody could create.
            domain_validation_records: Some(
                self.domain_names()
                    .into_iter()
                    .map(|domain_name| DomainValidationRecord {
                        domain_name: Some(domain_name),
                        validation_status: Some(
                            if issued {
                                DOMAIN_VALIDATION_STATUS_SUCCESS
                            } else {
                                DOMAIN_VALIDATION_STATUS_PENDING
                            }
                            .to_string(),
                        ),
                        ..Default::default()
                    })
                    .collect(),
            ),
            in_use_resource_count,
            key_algorithm: Some(CERTIFICATE_KEY_ALGORITHM.to_string()),
            created_at: Some(timestamp(self.created_at_ms)),
            // Nothing was issued while the certificate was still being
            // validated, so none of the issuance members is reported until it
            // is: a certificate has no issuer, no validity window and no issue
            // date before the certificate authority gives it one.
            issued_at: issued.then(|| timestamp(self.issued_at_ms)),
            issuer_ca: issued.then(|| CERTIFICATE_ISSUER_CA.to_string()),
            not_before: issued.then(|| timestamp(self.issued_at_ms)),
            not_after: issued
                .then(|| timestamp(self.issued_at_ms.saturating_add(CERTIFICATE_VALIDITY_MILLIS))),
            tags: Some(self.tags.clone()),
            support_code: Some(self.support_code.clone()),
            ..Default::default()
        }
    }
}
