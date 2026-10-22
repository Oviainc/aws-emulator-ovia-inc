// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-acm-certificatemanager service.
//!
//! One `Certificate` record per ACM certificate, keyed by its ARN, plus the
//! idempotency index `RequestCertificate` consults. Every operation's response is
//! projected from these records — see [`certificate_detail`] and
//! [`certificate_summary`] — so `DescribeCertificate` and `ListCertificates` report
//! what `RequestCertificate` stored rather than echoing their own input.
//!
//! A requested certificate does not stay `PENDING_VALIDATION`: ACM resolves its
//! validation on its own, so the record carries the outcome the clock owes it and the
//! instant it falls due ([`PendingValidation`]). Nothing sleeps for that instant —
//! [`ComAmazonawsAcmCertificatemanagerState::materialize_due`] applies whatever was
//! already due, and every handler calls it before reading or writing the store.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    CertificateDetail, CertificateOptions, CertificateSummary, DomainValidation, ExtendedKeyUsage,
    HttpRedirect, KeyUsage, ResourceRecord, Tag,
};

/// This service's SigV4 signing name, and the `acm` segment of every ARN it mints.
pub const SIGNING_NAME: &str = "acm";

/// The region used when a request carries no SigV4 credential scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `KeyAlgorithm` when `RequestCertificate` names none: "RSA is the default key
/// algorithm for ACM certificates" (`RequestCertificateRequest$KeyAlgorithm` doc),
/// and `RSA_2048` is the RSA size ACM issues.
pub const DEFAULT_KEY_ALGORITHM: &str = "RSA_2048";

/// `ValidationMethod=EMAIL`, the method whose evidence is a mailbox: it is the only
/// one for which `DomainValidation` carries `ValidationEmails` and
/// `ValidationDomain`, both documented as what "ACM used to send domain validation
/// emails".
pub const VALIDATION_METHOD_EMAIL: &str = "EMAIL";

/// `ValidationMethod` when `RequestCertificate` names none. ACM validates a public
/// certificate by email unless the request asks for DNS or HTTP validation.
pub const DEFAULT_VALIDATION_METHOD: &str = VALIDATION_METHOD_EMAIL;

/// `CertificateStatus`: "A certificate enters status PENDING_VALIDATION upon being
/// requested" (`CertificateDetail$Status` doc).
pub const STATUS_PENDING_VALIDATION: &str = "PENDING_VALIDATION";

/// `CertificateStatus`: "If validation succeeds, the certificate enters status
/// ISSUED" (`CertificateDetail$Status` doc).
pub const STATUS_ISSUED: &str = "ISSUED";

/// `CertificateStatus` of a request ACM could not complete: "A certificate enters
/// status PENDING_VALIDATION upon being requested, unless it fails for any of the
/// reasons given in the troubleshooting topic Certificate request fails"
/// (`CertificateDetail$Status` doc).
pub const STATUS_FAILED: &str = "FAILED";

/// `DomainStatus` of a domain whose ownership has not been proven yet.
pub const DOMAIN_STATUS_PENDING_VALIDATION: &str = "PENDING_VALIDATION";

/// `DomainStatus` of a domain whose ownership has been proven.
pub const DOMAIN_STATUS_SUCCESS: &str = "SUCCESS";

/// `DomainStatus` of a domain whose validation did not complete.
pub const DOMAIN_STATUS_FAILED: &str = "FAILED";

/// `FailureReason` of a public certificate request this emulator cannot carry
/// through: validating a domain name means proving control of it out of band — a DNS
/// record, a mailbox or an HTTP token on the domain itself — and this emulator makes
/// no outbound connection, so no requested domain can ever be proven here. ACM's
/// modelled reason for a request it cannot process without information it has to
/// obtain elsewhere is `ADDITIONAL_VERIFICATION_REQUIRED`, and it is what the
/// recorded AWS responses for unverifiable domains carry. `FailureReason` "exists
/// only when the certificate status is FAILED" (`CertificateDetail$FailureReason`
/// doc), so it is set only beside [`STATUS_FAILED`].
pub const FAILURE_REASON_ADDITIONAL_VERIFICATION_REQUIRED: &str =
    "ADDITIONAL_VERIFICATION_REQUIRED";

/// How long after `RequestCertificate` the validation outcome falls due.
///
/// A CHOSEN span, not a documented one: AWS takes minutes to hours ("ACM makes
/// repeated attempts to validate a certificate for 72 hours"), which no client could
/// poll through here. It is never zero, so a caller that reads straight after
/// requesting still observes `PENDING_VALIDATION`, and it is never slept on: the due
/// instant is stored on the record and materialised by whichever call next touches
/// the store (see [`ComAmazonawsAcmCertificatemanagerState::materialize_due`]).
pub const VALIDATION_SETTLE_DELAY: Duration = Duration::from_millis(500);

/// How long a certificate this emulator issues is valid for: ACM certificates are
/// valid for 13 months (395 days) (ACM user guide, "Managed certificate renewal"),
/// which is the span between the `NotBefore` and `NotAfter` of an issued record.
pub const CERTIFICATE_VALIDITY: Duration = Duration::from_secs(395 * 24 * 60 * 60);

/// The `KeyUsageName` values of the X.509 Key Usage extension of a TLS server
/// certificate ACM issues, which a certificate carries only once it has been issued.
pub const ISSUED_KEY_USAGES: &[&str] = &["DIGITAL_SIGNATURE", "KEY_ENCIPHERMENT"];

/// The `ExtendedKeyUsageName` values of the X.509 Extended Key Usage extension of a
/// TLS certificate ACM issues, with the same lifecycle as [`ISSUED_KEY_USAGES`].
pub const ISSUED_EXTENDED_KEY_USAGES: &[&str] = &[
    "TLS_WEB_SERVER_AUTHENTICATION",
    "TLS_WEB_CLIENT_AUTHENTICATION",
];

/// `CertificateType`: "For certificates provided by ACM, this value is
/// `AMAZON_ISSUED`" (`CertificateDetail$Type` doc).
pub const TYPE_AMAZON_ISSUED: &str = "AMAZON_ISSUED";

/// `RenewalEligibility`: "only exported private certificates can be renewed with the
/// RenewCertificate command" (`CertificateDetail$RenewalEligibility` doc), so a
/// public certificate ACM issues is not eligible.
pub const RENEWAL_INELIGIBLE: &str = "INELIGIBLE";

/// `CertificateKeyPairOrigin` of a certificate whose key pair ACM generated, which
/// is every certificate `RequestCertificate` creates.
pub const KEY_PAIR_ORIGIN_AWS_MANAGED: &str = "AWS_MANAGED";

/// `CertificateExport` when `Options.Export` is absent: export is opt-in
/// (`CertificateOptions$Export` doc).
pub const EXPORT_DISABLED: &str = "DISABLED";

/// `RecordType` of the DNS validation record: "Currently this can be `CNAME`"
/// (`ResourceRecord$Type` doc).
pub const RECORD_TYPE_CNAME: &str = "CNAME";

/// The zone ACM publishes DNS-validation CNAME targets under.
pub const ACM_VALIDATIONS_ZONE: &str = "acm-validations.aws";

/// The mailbox local-parts ACM sends domain-validation email to, beside the domain's
/// registered contacts, for `ValidationMethod=EMAIL`.
pub const VALIDATION_EMAIL_LOCAL_PARTS: &[&str] = &[
    "administrator",
    "hostmaster",
    "postmaster",
    "webmaster",
    "admin",
];

/// The path a CA fetches an HTTP domain-validation token from — the
/// `/.well-known/pki-validation/` location the CA/Browser Forum baseline
/// requirements define, which is where `HttpRedirect$RedirectFrom` points.
pub const HTTP_VALIDATION_PATH: &str = "/.well-known/pki-validation/";

/// The `KeyAlgorithm` values `ListCertificates` reports when the request names no
/// `Includes.keyTypes`. A certificate of any other key algorithm is listed only when
/// the request asks for that algorithm by name.
///
/// Two sentences in this service's model describe the default, and they disagree on
/// `RSA_1024`. The member describing this very filter is the wider one and is what is
/// encoded here — read it verbatim with:
///
/// ```text
/// smithy_query(query="doc", shape="com.amazonaws.acm#Filters", member="keyTypes",
///     model_path="services/service-com-amazonaws-acm-certificatemanager/smithy-model/com-amazonaws-acm-certificatemanager.json")
/// ```
///
/// which answers "Default filtering returns only RSA_1024 and RSA_2048 certificates
/// that have at least one domain. To return other certificate types, provide the
/// desired type signatures in a comma-separated list." The operation's own doc
/// (`smithy_query(query="doc", shape="com.amazonaws.acm#ListCertificates")`) says
/// "Default filtering returns only RSA_2048 certificates"; taking the member sentence
/// means the narrower claim is never the reason a stored certificate is withheld,
/// which is the safer way to be wrong about a filter nobody asked for.
pub const DEFAULT_LISTED_KEY_ALGORITHMS: &[&str] = &["RSA_1024", "RSA_2048"];

/// The `CertificateKeyPairOrigin` values `ListCertificates` reports when the request
/// names no `CertificateKeyPairOrigins`: "Default filtering returns only certificates
/// with key pair origin of AWS_MANAGED and CUSTOMER_PROVIDED"
/// (`ListCertificatesRequest$CertificateKeyPairOrigins` doc), i.e. "by default, this
/// action does not return certificates with a CertificateKeyPairOrigin of ACME"
/// (`ListCertificates` doc).
pub const DEFAULT_LISTED_KEY_PAIR_ORIGINS: &[&str] = &["AWS_MANAGED", "CUSTOMER_PROVIDED"];

/// `SortBy=CREATED_AT`, the only sort key `ListCertificates` models.
pub const SORT_BY_CREATED_AT: &str = "CREATED_AT";

/// `SortOrder=DESCENDING`.
pub const SORT_ORDER_DESCENDING: &str = "DESCENDING";

/// The number of subject alternative names a `CertificateSummary` carries;
/// `HasAdditionalSubjectAlternativeNames` reports whether the certificate has more
/// (`CertificateSummary$HasAdditionalSubjectAlternativeNames` doc).
pub const SUMMARY_SAN_LIMIT: usize = 100;

/// The validation ACM is waiting on for one domain name in a certificate.
#[derive(Debug, Clone)]
pub struct DomainValidationRecord {
    /// The FQDN being validated.
    pub domain_name: String,
    /// A `DomainStatus` value.
    pub validation_status: String,
    /// A `ValidationMethod` value.
    pub validation_method: String,
    /// The domain ACM mails, for email validation.
    pub validation_domain: Option<String>,
    /// The mailboxes ACM mailed, for email validation.
    pub validation_emails: Vec<String>,
    /// The CNAME to publish, for DNS validation.
    pub resource_record: Option<ResourceRecord>,
    /// The URLs the CA fetches, for HTTP validation.
    pub http_redirect: Option<HttpRedirect>,
}

/// How the validation of a requested certificate resolves.
///
/// Decided by the call that starts the transition, from what the request asks for,
/// and applied when the transition falls due.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationOutcome {
    /// The certificate is issued: "If you are requesting a private certificate,
    /// domain validation is not required" (`RequestCertificate` doc), so there is
    /// nothing left to prove and the request completes.
    Issued,
    /// The request fails with [`FAILURE_REASON_ADDITIONAL_VERIFICATION_REQUIRED`]:
    /// "If you are requesting a public certificate, each domain name that you specify
    /// must be validated to verify that you own or control the domain"
    /// (`RequestCertificate` doc), and that proof is out of band.
    Failed,
}

/// The validation outcome the clock already owes a requested certificate.
///
/// Stored, never slept on: the instant it falls due is fixed by the call that started
/// it, so the outcome is owed at that instant whether or not anybody calls again.
#[derive(Debug, Clone)]
pub struct PendingValidation {
    /// When the outcome falls due.
    pub due_at: SystemTime,
    /// What falls due then.
    pub outcome: ValidationOutcome,
}

impl PendingValidation {
    /// The outcome owed to a certificate requested at `requested_at`, due
    /// [`VALIDATION_SETTLE_DELAY`] later.
    pub fn starting_at(requested_at: SystemTime, outcome: ValidationOutcome) -> Self {
        Self {
            due_at: requested_at + VALIDATION_SETTLE_DELAY,
            outcome,
        }
    }
}

/// One stored ACM certificate. Every field a response reports is read from here.
#[derive(Debug, Clone)]
pub struct Certificate {
    /// `arn:aws:acm:<region>:<account>:certificate/<id>`.
    pub arn: String,
    pub domain_name: String,
    /// `domain_name` first, then the requested SANs, deduplicated.
    pub subject_alternative_names: Vec<String>,
    /// A `CertificateStatus` value.
    pub status: String,
    /// A `CertificateType` value.
    pub cert_type: String,
    /// A `KeyAlgorithm` value.
    pub key_algorithm: String,
    /// A `CertificateKeyPairOrigin` value.
    pub key_pair_origin: String,
    /// A `ValidationMethod` value, for a certificate ACM has to validate.
    pub validation_method: Option<String>,
    /// One entry per name in `subject_alternative_names`.
    pub domain_validations: Vec<DomainValidationRecord>,
    pub options: Option<CertificateOptions>,
    pub certificate_authority_arn: Option<String>,
    /// A `CertificateManagedBy` value.
    pub managed_by: Option<String>,
    /// A `RenewalEligibility` value.
    pub renewal_eligibility: String,
    /// A `FailureReason` value, which "exists only when the certificate status is
    /// FAILED" (`CertificateDetail$FailureReason` doc).
    pub failure_reason: Option<String>,
    /// The validation outcome the clock owes this certificate, while it is still
    /// `PENDING_VALIDATION`. `None` once the certificate has reached a terminal
    /// status — there is nothing left for the clock to do.
    pub pending_validation: Option<PendingValidation>,
    /// ARNs of the resources using this certificate.
    pub in_use_by: Vec<String>,
    /// The `KeyUsageName` values of the certificate's X.509 Key Usage extensions.
    /// Those extensions exist only once the certificate has been issued, so this is
    /// empty while it is pending — which is what the `Includes.keyUsage` filter
    /// compares against.
    pub key_usages: Vec<String>,
    /// The `ExtendedKeyUsageName` values of the certificate's X.509 Extended Key
    /// Usage extensions, with the same lifecycle as [`Self::key_usages`].
    pub extended_key_usages: Vec<String>,
    /// Whether the certificate has ever been exported.
    pub exported: bool,
    pub created_at: SystemTime,
    pub issued_at: Option<SystemTime>,
    pub not_before: Option<SystemTime>,
    pub not_after: Option<SystemTime>,
    /// Tag key -> value, as `ListTagsForCertificate` reports them.
    pub tags: BTreeMap<String, String>,
}

impl Certificate {
    /// Apply the validation outcome the clock already owes this certificate at `now`.
    ///
    /// A certificate ACM reports as `PENDING_VALIDATION` is waiting on work ACM does
    /// on its own; the instant that work completes was fixed when the certificate was
    /// requested, so applying it here materialises a change the clock already made
    /// rather than a side effect of whoever happened to call.
    pub fn materialize_due(&mut self, now: SystemTime) {
        let Some(pending) = self
            .pending_validation
            .clone()
            .filter(|pending| now >= pending.due_at)
        else {
            return;
        };
        self.pending_validation = None;
        match pending.outcome {
            ValidationOutcome::Issued => self.issue(pending.due_at),
            ValidationOutcome::Failed => {
                self.fail(FAILURE_REASON_ADDITIONAL_VERIFICATION_REQUIRED);
            }
        }
    }

    /// Record this certificate as issued at `at`: every domain is proven, the
    /// validity window opens, and the X.509 extensions an issued certificate carries
    /// come into existence.
    fn issue(&mut self, at: SystemTime) {
        self.status = STATUS_ISSUED.to_string();
        self.failure_reason = None;
        for validation in &mut self.domain_validations {
            validation.validation_status = DOMAIN_STATUS_SUCCESS.to_string();
        }
        self.issued_at = Some(at);
        self.not_before = Some(at);
        self.not_after = Some(at + CERTIFICATE_VALIDITY);
        self.key_usages = ISSUED_KEY_USAGES
            .iter()
            .map(|name| (*name).to_string())
            .collect();
        self.extended_key_usages = ISSUED_EXTENDED_KEY_USAGES
            .iter()
            .map(|name| (*name).to_string())
            .collect();
    }

    /// Record this certificate's request as failed for `reason`: no domain was
    /// proven, and nothing was issued, so the issuance members stay absent.
    ///
    /// `ValidationEmails` is "a list of email addresses that ACM used to send domain
    /// validation emails" (`DomainValidation$ValidationEmails` doc). A request that
    /// ends with ownership still unproven sent none, so the list is dropped with the
    /// validation it belonged to.
    fn fail(&mut self, reason: &str) {
        self.status = STATUS_FAILED.to_string();
        self.failure_reason = Some(reason.to_string());
        for validation in &mut self.domain_validations {
            validation.validation_status = DOMAIN_STATUS_FAILED.to_string();
            validation.validation_emails.clear();
        }
    }
}

/// Per-service state for com-amazonaws-acm-certificatemanager. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsAcmCertificatemanagerState {
    /// Certificate ARN -> record. A `BTreeMap`, so an unsorted `ListCertificates`
    /// page and its `NextToken` walk one deterministic order.
    pub certificates: BTreeMap<String, Certificate>,
    /// Idempotency key (token plus a fingerprint of the request parameters) ->
    /// certificate ARN, so re-sending one `RequestCertificate` returns the
    /// certificate it already created instead of making another.
    pub idempotency: HashMap<String, String>,
}

impl ComAmazonawsAcmCertificatemanagerState {
    /// Apply every validation outcome the clock already owes at `now`.
    ///
    /// Call it FIRST in every handler that locks the store, so each operation sees the
    /// state a caller would see after any other. One pass touches at most as many
    /// records as the account may hold certificates, and each is one instant
    /// comparison.
    pub fn materialize_due(&mut self, now: SystemTime) {
        for certificate in self.certificates.values_mut() {
            certificate.materialize_due(now);
        }
    }

    /// Drop `arn`'s record and every idempotency entry that resolves to it, so a
    /// replayed `RequestCertificate` can never answer with an ARN that has no
    /// record behind it.
    pub fn remove_certificate(&mut self, arn: &str) -> Option<Certificate> {
        let removed = self.certificates.remove(arn)?;
        self.idempotency.retain(|_, stored| stored != arn);
        Some(removed)
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAcmCertificatemanagerState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAcmCertificatemanagerState>)
}

/// The region to mint ARNs in: the one the caller signed for, else [`DEFAULT_REGION`].
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `arn:aws:acm:<region>:<account>:certificate/<certificate_id>`.
pub fn certificate_arn(region: &str, certificate_id: &str) -> String {
    ::engine::emulator_arn(
        SIGNING_NAME,
        region,
        &format!("certificate/{certificate_id}"),
    )
}

/// The certificate id in an ACM certificate ARN, or `None` when `arn` names
/// something that is not one (the caller answers `InvalidArnException`).
///
/// The model's `@pattern` already refused anything that is not an `acm` ARN before a
/// handler runs; what is left to check here is that the resource is a `certificate`.
pub fn certificate_id_from_arn(arn: &str) -> Option<&str> {
    let mut fields = arn.split(':');
    let (
        Some("arn"),
        Some(partition),
        Some(SIGNING_NAME),
        Some(_region),
        Some(account),
        Some(resource),
        None,
    ) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    )
    else {
        return None;
    };
    if partition.is_empty() || !account.bytes().all(|b| b.is_ascii_digit()) || account.is_empty() {
        return None;
    }
    let id = resource.strip_prefix("certificate/")?;
    (!id.is_empty() && !id.contains('/')).then_some(id)
}

/// A wall-clock instant as the modelled `TStamp`.
pub fn to_date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_and_nanos(
        i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
        since_epoch.subsec_nanos(),
    )
}

/// The validation label ACM publishes for one `(certificate, domain, purpose)`:
/// 32 lowercase hex characters, the shape of the labels in the `ResourceRecord` doc
/// (`_a79865eb4cd1a6ab990a45779b4e0b96`).
///
/// Derived from the certificate's own id, so repeated describes of one certificate
/// report the same record while two certificates for the same domain differ.
pub fn validation_label(certificate_id: &str, domain_name: &str, purpose: &str) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(certificate_id.as_bytes());
    hasher.update([0u8]);
    hasher.update(domain_name.as_bytes());
    hasher.update([0u8]);
    hasher.update(purpose.as_bytes());
    let digest = hasher.finalize();
    hex::encode(&digest[..16])
}

/// A domain name with a leading wildcard label removed: ACM validates
/// `*.example.com` by proving control of `example.com`.
pub fn validation_target(domain_name: &str) -> &str {
    domain_name.strip_prefix("*.").unwrap_or(domain_name)
}

/// The request's tags as the key -> value map the certificate record stores, or the
/// message explaining why a tag was refused.
///
/// A tag key or value beginning with `aws:` is reserved, which is what
/// `InvalidTagException` names ("you cannot specify a tag value that begins with
/// aws:"). Later values win for a repeated key, so one key maps to one value.
pub fn tag_map(tags: &[Tag]) -> Result<BTreeMap<String, String>, String> {
    let mut mapped = BTreeMap::new();
    for tag in tags {
        if tag.key.starts_with("aws:") || tag.value.as_deref().is_some_and(|v| v.starts_with("aws:"))
        {
            return Err(format!(
                "tag {} is reserved: neither a tag key nor its value may begin with aws:",
                tag.key
            ));
        }
        mapped.insert(tag.key.clone(), tag.value.clone().unwrap_or_default());
    }
    Ok(mapped)
}

/// The stored tags as the modelled `Tag` list, in key order.
pub fn tag_list(tags: &BTreeMap<String, String>) -> Vec<Tag> {
    tags.iter()
        .map(|(key, value)| Tag {
            key: key.clone(),
            value: Some(value.clone()),
        })
        .collect()
}

/// `CertificateExport` for a certificate: `ENABLED` only when its options opted in.
pub fn export_option(cert: &Certificate) -> String {
    cert.options
        .as_ref()
        .and_then(|options| options.export.clone())
        .unwrap_or_else(|| EXPORT_DISABLED.to_string())
}

/// One stored validation as the modelled `DomainValidation`.
///
/// `ValidationEmails` is "a list of email addresses that ACM used to send domain
/// validation emails" (`DomainValidation$ValidationEmails` doc), so it belongs to a
/// domain validated BY EMAIL: it is reported for such a domain — as the empty list
/// once no mailbox was used, which is a validation that never ran — and is absent for
/// a domain validated by DNS or HTTP, where ACM sends no mail at all.
fn domain_validation(record: &DomainValidationRecord) -> DomainValidation {
    DomainValidation {
        domain_name: record.domain_name.clone(),
        validation_emails: (!record.validation_emails.is_empty()
            || record.validation_method == VALIDATION_METHOD_EMAIL)
            .then(|| record.validation_emails.clone()),
        validation_domain: record.validation_domain.clone(),
        validation_status: Some(record.validation_status.clone()),
        resource_record: record.resource_record.clone(),
        http_redirect: record.http_redirect.clone(),
        validation_method: Some(record.validation_method.clone()),
    }
}

/// A stored certificate as `DescribeCertificate`'s `CertificateDetail`.
///
/// Members that exist only once a certificate has been issued (`Serial`, `Subject`,
/// `Issuer`, `SignatureAlgorithm`, the key usages, `IssuedAt`, `NotBefore` /
/// `NotAfter`) stay absent until the record carries them.
pub fn certificate_detail(cert: &Certificate) -> CertificateDetail {
    CertificateDetail {
        certificate_arn: Some(cert.arn.clone()),
        domain_name: Some(cert.domain_name.clone()),
        subject_alternative_names: Some(cert.subject_alternative_names.clone()),
        managed_by: cert.managed_by.clone(),
        domain_validation_options: (!cert.domain_validations.is_empty())
            .then(|| cert.domain_validations.iter().map(domain_validation).collect()),
        created_at: Some(to_date_time(cert.created_at)),
        issued_at: cert.issued_at.map(to_date_time),
        status: Some(cert.status.clone()),
        not_before: cert.not_before.map(to_date_time),
        not_after: cert.not_after.map(to_date_time),
        key_algorithm: Some(cert.key_algorithm.clone()),
        in_use_by: Some(cert.in_use_by.clone()),
        // The X.509 key-usage extensions of an issued certificate; absent while the
        // record carries none.
        key_usages: (!cert.key_usages.is_empty()).then(|| {
            cert.key_usages
                .iter()
                .map(|name| KeyUsage {
                    name: Some(name.clone()),
                })
                .collect()
        }),
        extended_key_usages: (!cert.extended_key_usages.is_empty()).then(|| {
            cert.extended_key_usages
                .iter()
                .map(|name| ExtendedKeyUsage {
                    name: Some(name.clone()),
                    // The extension's object identifier, which only a parsed
                    // certificate carries.
                    oid: None,
                })
                .collect()
        }),
        r#type: Some(cert.cert_type.clone()),
        certificate_authority_arn: cert.certificate_authority_arn.clone(),
        renewal_eligibility: Some(cert.renewal_eligibility.clone()),
        // "This value exists only when the certificate status is FAILED"
        // (`CertificateDetail$FailureReason` doc), which is what the record stores.
        failure_reason: cert.failure_reason.clone(),
        options: cert.options.clone(),
        certificate_key_pair_origin: Some(cert.key_pair_origin.clone()),
        ..CertificateDetail::default()
    }
}

/// A stored certificate as `ListCertificates`' `CertificateSummary`.
///
/// `SubjectAlternativeNameSummaries` carries at most [`SUMMARY_SAN_LIMIT`] names and
/// `HasAdditionalSubjectAlternativeNames` says whether the certificate has more, as
/// that member's doc defines.
pub fn certificate_summary(cert: &Certificate) -> CertificateSummary {
    let total_sans = cert.subject_alternative_names.len();
    let shown = total_sans.min(SUMMARY_SAN_LIMIT);
    CertificateSummary {
        certificate_arn: Some(cert.arn.clone()),
        domain_name: Some(cert.domain_name.clone()),
        subject_alternative_name_summaries: Some(
            cert.subject_alternative_names[..shown].to_vec(),
        ),
        has_additional_subject_alternative_names: Some(total_sans > shown),
        status: Some(cert.status.clone()),
        r#type: Some(cert.cert_type.clone()),
        key_algorithm: Some(cert.key_algorithm.clone()),
        // The X.509 key-usage extension names of an issued certificate; absent while
        // the record carries none.
        key_usages: (!cert.key_usages.is_empty()).then(|| cert.key_usages.clone()),
        extended_key_usages: (!cert.extended_key_usages.is_empty())
            .then(|| cert.extended_key_usages.clone()),
        export_option: Some(export_option(cert)),
        in_use: Some(!cert.in_use_by.is_empty()),
        exported: Some(cert.exported),
        renewal_eligibility: Some(cert.renewal_eligibility.clone()),
        not_before: cert.not_before.map(to_date_time),
        not_after: cert.not_after.map(to_date_time),
        created_at: Some(to_date_time(cert.created_at)),
        issued_at: cert.issued_at.map(to_date_time),
        managed_by: cert.managed_by.clone(),
        certificate_key_pair_origin: Some(cert.key_pair_origin.clone()),
        ..CertificateSummary::default()
    }
}
