// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-acm-certificatemanager service.
//!
//! One `Certificate` record per ACM certificate, keyed by its ARN, plus the
//! idempotency index `RequestCertificate` consults. Every operation's response is
//! projected from these records — see [`certificate_detail`], [`certificate_summary`]
//! and [`certificate_search_result`] — so `DescribeCertificate`, `ListCertificates`
//! and `SearchCertificates` report what `RequestCertificate` stored rather than
//! echoing their own input.
//!
//! Beside them, one [`AcmeEndpointRecord`] per managed ACME endpoint, keyed by its
//! ARN, plus the idempotency index `CreateAcmeEndpoint` consults
//! ([`AcmeEndpointIdempotency`]). `DescribeAcmeEndpoint` projects that record through
//! [`acme_endpoint_detail`], and a freshly created endpoint is `CREATING` until the
//! transition the clock owes it falls due, exactly as a pending certificate is.
//!
//! Beside the certificates sits the account-level singleton
//! [`AccountConfiguration`]: `PutAccountConfiguration` writes it and
//! `GetAccountConfiguration` reports it, so that round trip reads one record too.
//!
//! A requested certificate does not stay `PENDING_VALIDATION`: ACM resolves its
//! validation on its own, so the record carries the outcome the clock owes it and the
//! instant it falls due ([`PendingValidation`]). Nothing sleeps for that instant —
//! [`ComAmazonawsAcmCertificatemanagerState::materialize_due`] applies whatever was
//! already due, and every handler calls it before reading or writing the store.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AcmCertificateMetadata, AcmeEndpoint, CertificateAuthority, CertificateDetail,
    CertificateMetadata, CertificateOptions, CertificateSearchResult, CertificateSummary,
    DistinguishedName, DomainValidation, ExpiryEventsConfiguration, ExtendedKeyUsage, GeneralName,
    HttpRedirect, KeyUsage, ResourceRecord, Tag, X509Attributes,
};

/// This service's SigV4 signing name, and the `acm` segment of every ARN it mints.
pub const SIGNING_NAME: &str = "acm";

/// The region used when a request carries no SigV4 credential scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `KeyAlgorithm` when `RequestCertificate` names none: "RSA is the default key
/// algorithm for ACM certificates" (`RequestCertificateRequest$KeyAlgorithm` doc),
/// and `RSA_2048` is the RSA size ACM issues.
pub const DEFAULT_KEY_ALGORITHM: &str = "RSA_2048";

/// `ValidationMethod=DNS`, the method whose evidence is a published DNS record: it is
/// the only one for which `DomainValidation` carries a `ResourceRecord`, the CNAME
/// "you add to your DNS database" (`ResourceRecord` doc).
pub const VALIDATION_METHOD_DNS: &str = "DNS";

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

/// `CertificateStatus` of a certificate whose revocation ACM has recorded:
/// `RevokedAt` and `RevocationReason` "exist only when the certificate status is
/// REVOKED" (`CertificateDetail$RevokedAt` / `$RevocationReason` docs), so the two
/// are written beside this status and nowhere else.
pub const STATUS_REVOKED: &str = "REVOKED";

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

/// How long after the domain-validation email was last sent ACM will send it again.
///
/// A DOCUMENTED span, not a chosen one: "you can request that the mail be resent
/// within 72 hours of requesting the ACM certificate. If more than 72 hours have
/// elapsed since your original request or since your last attempt to resend
/// validation mail, you must request a new certificate" (`ResendValidationEmail`
/// doc). Past it there is no mail left to resend, which is the invalid state
/// `ResendValidationEmail` reports.
pub const RESEND_VALIDATION_WINDOW: Duration = Duration::from_secs(72 * 60 * 60);

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

/// `CertificateExport` of a certificate whose owner opted in to export: "You can opt
/// in to allow the export of your certificates by specifying ENABLED. You cannot
/// update the value of Export after the the certificate is created"
/// (`CertificateOptions$Export` doc) — so this is fixed when the certificate is
/// created and is what decides whether it may ever be exported, and therefore
/// revoked.
pub const EXPORT_ENABLED: &str = "ENABLED";

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

/// `AcmeEndpointStatus` of an ACME endpoint ACM is still standing up: an endpoint is
/// "a managed ACME server with a unique endpoint URL" (`CreateAcmeEndpoint` doc), and
/// `AcmeEndpointStatus` models `CREATING` beside `ACTIVE` precisely because the server
/// is not serving the instant the call returns.
pub const ACME_ENDPOINT_STATUS_CREATING: &str = "CREATING";

/// `AcmeEndpointStatus` of an ACME endpoint that is serving: "After creation, ACME
/// clients can use the endpoint URL to automate certificate issuance"
/// (`CreateAcmeEndpoint` doc).
pub const ACME_ENDPOINT_STATUS_ACTIVE: &str = "ACTIVE";

/// The most ACME endpoint records this emulator stores, across every Region.
///
/// Not an AWS quota: ACM's own endpoint quota is per Region, and the Region of a
/// request is read off the caller's SigV4 credential scope, so a per-Region bound is a
/// bound the CALLER can partition by signing each request for another Region string.
/// This cap is keyed on nothing in the request, so it bounds
/// [`ComAmazonawsAcmCertificatemanagerState::acme_endpoints`] (and with it the
/// per-request `materialize_due` pass over those records) however the request varies.
/// `CreateAcmeEndpoint` checks it before inserting and answers its modelled quota error
/// past it.
pub const ACME_ENDPOINT_STORE_LIMIT: usize = 1_000;

/// How long after `CreateAcmeEndpoint` the endpoint reaches
/// [`ACME_ENDPOINT_STATUS_ACTIVE`].
///
/// A CHOSEN span, not a documented one: neither the model nor ACM's documentation
/// states how long standing an ACME endpoint up takes. It is never zero, so a caller
/// that describes the endpoint straight after creating it still observes `CREATING`,
/// and it is never slept on: the due instant is stored on the record and materialised
/// by whichever call next touches the store (see
/// [`ComAmazonawsAcmCertificatemanagerState::materialize_due`]).
pub const ACME_ENDPOINT_ACTIVATION_DELAY: Duration = Duration::from_millis(500);

/// The account-level configuration `PutAccountConfiguration` writes and
/// `GetAccountConfiguration` reports. One per account, so the state holds a
/// singleton rather than a map.
///
/// "The supported configuration option is DaysBeforeExpiry"
/// (`PutAccountConfiguration` doc), so that option is all this record carries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccountConfiguration {
    /// `ExpiryEventsConfiguration$DaysBeforeExpiry` as the last write carried it;
    /// `None` when that write named an `ExpiryEvents` with no `DaysBeforeExpiry`.
    pub days_before_expiry: Option<i32>,
}

/// The largest `DaysBeforeExpiry` ACM accepts. The model constrains this option
/// only from below (`PositiveInteger`, `@range(min: 1)`), but the service refuses
/// anything past the default notice window of 45 days with a `ValidationException`
/// saying so, so the accepted range is 1..=45.
pub const MAX_DAYS_BEFORE_EXPIRY: i32 = 45;

/// `ExpiryEventsConfiguration$DaysBeforeExpiry` for an account that has never
/// written one: "By default, accounts receive events starting 45 days before
/// certificate expiration" (`ExpiryEventsConfiguration$DaysBeforeExpiry` doc), so
/// that is the configuration such an account is running under and what
/// `GetAccountConfiguration` reports for it.
pub const DEFAULT_DAYS_BEFORE_EXPIRY: i32 = 45;

/// How long a `PutAccountConfiguration` idempotency token stays live: "Idempotency
/// tokens time out after one hour"
/// (`PutAccountConfigurationRequest$IdempotencyToken` doc). Past it the token names
/// no earlier request, so the next call carrying it is a new request.
pub const IDEMPOTENCY_TOKEN_TTL: Duration = Duration::from_secs(60 * 60);

/// One `PutAccountConfiguration` request ACM has already processed, kept so a call
/// repeating its idempotency token can be recognised as the same request — or, when
/// it asks for something else, refused as a different one.
#[derive(Debug, Clone)]
pub struct AccountConfigurationRequest {
    /// The `ExpiryEvents` that request carried, and `None` when it carried none.
    pub expiry_events: Option<AccountConfiguration>,
    /// When ACM processed it; the token times out [`IDEMPOTENCY_TOKEN_TTL`] later.
    pub processed_at: SystemTime,
}

impl AccountConfigurationRequest {
    /// Whether this request's idempotency token is still live at `now`.
    pub fn token_is_live(&self, now: SystemTime) -> bool {
        now.duration_since(self.processed_at)
            .is_ok_and(|elapsed| elapsed < IDEMPOTENCY_TOKEN_TTL)
            // A record stamped in the future (a clock that stepped back) has not
            // timed out either.
            || self.processed_at > now
    }
}

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
    /// The validation domain the certificate was REQUESTED with for this name
    /// (`DomainValidationOption$ValidationDomain`), kept whatever method is in force.
    ///
    /// [`Self::validation_domain`] is reported only for email validation, because it
    /// is "the domain name that ACM used to send domain validation emails"; this is
    /// the configuration that choice came from, so a certificate migrated away from
    /// `EMAIL` and back mails the domain the request named rather than the covered
    /// name itself.
    pub requested_validation_domain: Option<String>,
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
    /// Whether the certificate has ever been exported. Only an exported certificate
    /// may be revoked (`RevokeCertificate` doc: "You can only revoke certificates
    /// that have been previously exported").
    pub exported: bool,
    /// When ACM last sent this certificate's domain-validation email: the instant the
    /// certificate was requested for a request that asked for email validation, moved
    /// forward by each `ResendValidationEmail`. `None` for a certificate ACM sends no
    /// validation mail for at all. It is what the documented
    /// [`RESEND_VALIDATION_WINDOW`] is measured from.
    pub validation_email_sent_at: Option<SystemTime>,
    /// When the certificate was revoked, which "exists only when the certificate
    /// status is REVOKED" (`CertificateDetail$RevokedAt` doc).
    pub revoked_at: Option<SystemTime>,
    /// A `RevocationReason` value — the reason the caller gave `RevokeCertificate` —
    /// which "exists only when the certificate status is REVOKED"
    /// (`CertificateDetail$RevocationReason` doc).
    pub revocation_reason: Option<String>,
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

    /// Whether export was allowed for this certificate when it was created: "You can
    /// opt in to allow the export of your certificates by specifying ENABLED. You
    /// cannot update the value of Export after the the certificate is created"
    /// (`CertificateOptions$Export` doc).
    pub fn export_allowed(&self) -> bool {
        export_option(self) == EXPORT_ENABLED
    }

    /// Whether this certificate is in the state `RevokeCertificate` applies to.
    ///
    /// "Revokes a public ACM certificate. You can only revoke certificates that have
    /// been previously exported" and "revoking a certificate is permanent"
    /// (`RevokeCertificate` doc): the export is what put the private key outside ACM,
    /// and a certificate whose revocation is already recorded has nothing left to
    /// revoke.
    pub fn is_revocable(&self) -> bool {
        self.export_allowed() && self.exported && !self.is_revoked()
    }

    /// Whether this certificate's revocation is already recorded.
    pub fn is_revoked(&self) -> bool {
        self.status == STATUS_REVOKED || self.revoked_at.is_some()
    }

    /// Record this certificate as revoked at `at` for `reason`.
    ///
    /// The validity window and the issuance members stay as issued — the certificate
    /// was issued and still carries those facts; what changes is its status and the
    /// two members that exist only beside it.
    pub fn revoke(&mut self, at: SystemTime, reason: &str) {
        self.status = STATUS_REVOKED.to_string();
        self.revoked_at = Some(at);
        self.revocation_reason = Some(reason.to_string());
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

/// The status transition the clock already owes an ACME endpoint.
///
/// Stored, never slept on: the instant it falls due is fixed by the call that started
/// the transition, so the endpoint reaches that status at that instant whether or not
/// anybody calls again.
#[derive(Debug, Clone)]
pub struct PendingAcmeEndpointStatus {
    /// When the transition falls due.
    pub due_at: SystemTime,
    /// The `AcmeEndpointStatus` the endpoint carries from then on.
    pub status: String,
}

impl PendingAcmeEndpointStatus {
    /// The transition owed to an endpoint created at `created_at`: it becomes
    /// [`ACME_ENDPOINT_STATUS_ACTIVE`] [`ACME_ENDPOINT_ACTIVATION_DELAY`] later.
    pub fn activating_at(created_at: SystemTime) -> Self {
        Self {
            due_at: created_at + ACME_ENDPOINT_ACTIVATION_DELAY,
            status: ACME_ENDPOINT_STATUS_ACTIVE.to_string(),
        }
    }
}

/// One stored ACME endpoint. Every field `DescribeAcmeEndpoint` reports is read from
/// here, so no response echoes its own request.
#[derive(Debug, Clone)]
pub struct AcmeEndpointRecord {
    /// `arn:aws:acm:<region>:<account>:acme-endpoint/<id>`.
    pub arn: String,
    /// The server-managed ACME directory URL ([`acme_endpoint_url`]).
    pub endpoint_url: String,
    /// An `AcmeEndpointStatus` value.
    pub status: String,
    /// "The reason the ACME endpoint failed, if applicable"
    /// (`AcmeEndpoint$FailureReason` doc), so it is set only beside a `FAILED` status.
    pub failure_reason: Option<String>,
    /// An `AcmeAuthorizationBehavior` value, as the create request configured it.
    pub authorization_behavior: String,
    /// An `AcmeContact` value, as the create request configured it.
    pub contact: String,
    /// The certificate authority the endpoint issues through.
    pub certificate_authority: CertificateAuthority,
    /// Tag key -> value applied to certificates issued through this endpoint.
    pub certificate_tags: BTreeMap<String, String>,
    /// Tag key -> value on the endpoint resource itself, as `ListTagsForResource`
    /// reports them.
    pub tags: BTreeMap<String, String>,
    /// The status transition the clock owes this endpoint. `None` once it has
    /// reached a status the clock owes nothing beyond.
    pub pending_status: Option<PendingAcmeEndpointStatus>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl AcmeEndpointRecord {
    /// Apply the status transition the clock already owes this endpoint at `now`.
    ///
    /// An endpoint ACM reports as `CREATING` is waiting on work ACM does on its own;
    /// the instant that work completes was fixed when the endpoint was created, so
    /// applying it here materialises a change the clock already made rather than a
    /// side effect of whoever happened to call.
    pub fn materialize_due(&mut self, now: SystemTime) {
        let Some(pending) = self
            .pending_status
            .clone()
            .filter(|pending| now >= pending.due_at)
        else {
            return;
        };
        self.pending_status = None;
        self.status = pending.status;
        // The endpoint changed at the instant the transition fell due, which is what
        // `UpdatedAt` ("the time at which the ACME endpoint was last updated") reports
        // — not the instant somebody happened to read it.
        self.updated_at = pending.due_at;
    }
}

/// What one `CreateAcmeEndpoint` idempotency token has already created.
#[derive(Debug, Clone)]
pub struct AcmeEndpointIdempotency {
    /// A canonical rendering of every parameter the endpoint was created from, so a
    /// re-sent token can be told apart from a changed request carrying the same token.
    pub fingerprint: String,
    /// The ARN the first call answered with.
    pub acme_endpoint_arn: String,
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
    /// ACME endpoint ARN -> record. A `BTreeMap`, so an unsorted `ListAcmeEndpoints`
    /// page and its `NextToken` walk one deterministic order.
    pub acme_endpoints: BTreeMap<String, AcmeEndpointRecord>,
    /// `CreateAcmeEndpoint` idempotency token -> what that token created. Separate
    /// from [`Self::idempotency`] because the two index different resources: a token
    /// resolves to the endpoint it created, and the two operations' tokens must not
    /// be able to collide.
    pub acme_endpoint_idempotency: HashMap<String, AcmeEndpointIdempotency>,
    /// The account's expiry-events configuration, as the last successful
    /// `PutAccountConfiguration` wrote it, and `None` until one has — when ACM is
    /// running the account on the documented default,
    /// [`DEFAULT_DAYS_BEFORE_EXPIRY`].
    pub account_configuration: Option<AccountConfiguration>,
    /// `PutAccountConfiguration` idempotency token -> the request it was first sent
    /// with, for as long as that token stays live. Repeating a token with the same
    /// request is the same request; repeating it with another is a conflict.
    pub account_configuration_requests: HashMap<String, AccountConfigurationRequest>,
}

impl ComAmazonawsAcmCertificatemanagerState {
    /// Drop every `PutAccountConfiguration` idempotency token that has timed out by
    /// `now`, so the index holds only tokens ACM would still recognise and cannot
    /// grow without bound.
    pub fn expire_account_configuration_tokens(&mut self, now: SystemTime) {
        self.account_configuration_requests
            .retain(|_, request| request.token_is_live(now));
    }

    /// Apply every validation outcome the clock already owes at `now`.
    ///
    /// Call it FIRST in every handler that locks the store, so each operation sees the
    /// state a caller would see after any other. One pass touches at most as many
    /// records as the account may hold certificates and ACME endpoints, and each is one
    /// instant comparison.
    pub fn materialize_due(&mut self, now: SystemTime) {
        for certificate in self.certificates.values_mut() {
            certificate.materialize_due(now);
        }
        for endpoint in self.acme_endpoints.values_mut() {
            endpoint.materialize_due(now);
        }
    }

    /// Drop `arn`'s ACME endpoint record and every idempotency token that resolves to
    /// it, so a replayed `CreateAcmeEndpoint` can never answer with an ARN that has no
    /// record behind it.
    pub fn remove_acme_endpoint(&mut self, arn: &str) -> Option<AcmeEndpointRecord> {
        let removed = self.acme_endpoints.remove(arn)?;
        self.acme_endpoint_idempotency
            .retain(|_, seen| seen.acme_endpoint_arn != arn);
        Some(removed)
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

/// `arn:aws:acm:<region>:<account>:acme-endpoint/<endpoint_id>`, the form
/// `AcmeEndpointArn`'s `@pattern` requires
/// (`^arn:aws[a-z-]*:acm:[a-z0-9-]+:[0-9]{12}:acme-endpoint/[a-zA-Z0-9-]+$`).
pub fn acme_endpoint_arn(region: &str, endpoint_id: &str) -> String {
    ::engine::emulator_arn(
        SIGNING_NAME,
        region,
        &format!("acme-endpoint/{endpoint_id}"),
    )
}

/// The ACME directory URL an endpoint serves on.
///
/// `AcmeEndpoint$EndpointUrl` is documented only as "The URL of the ACME endpoint",
/// and `CreateAcmeEndpoint` models no member a caller could propose one with, so what
/// matters is the invariant rather than AWS's exact host: the URL is SERVER-MANAGED
/// and derived from the endpoint's own id and Region, so it is unique per endpoint,
/// identical on every later read, and carries nothing the caller sent. It ends at the
/// `directory` resource, the single URL an ACME client is given and starts from
/// (RFC 8555 §7.1.1).
pub fn acme_endpoint_url(region: &str, endpoint_id: &str) -> String {
    format!("https://{endpoint_id}.acme.{region}.amazonaws.com/directory")
}

/// The `ResourceNotFoundException` message for an ACME endpoint ARN that no stored
/// record answers to, built from the ARN the request asked for.
///
/// `ResourceNotFoundException` carries one modelled member (`message: String`) and the
/// model documents no text for it, so the sentence follows the template every other
/// lookup in this service renders ([`certificate_not_found_message`]): one complete
/// sentence naming the requested ARN and terminated by a period.
pub fn acme_endpoint_not_found_message(acme_endpoint_arn: &str) -> String {
    format!("Could not find ACME endpoint {acme_endpoint_arn}.")
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

/// The `InvalidArnException` message for an ARN that is not an ACM certificate ARN,
/// built from the ARN the request asked for.
///
/// `InvalidArnException` carries one modelled member (`message: String`) and the model
/// documents no text for it, so the sentence comes from ACM's recorded behaviour: the
/// requested ARN followed by `is not a valid ARN`. Rendered here so every
/// certificate-lookup path reports the same sentence.
pub fn invalid_arn_message(certificate_arn: &str) -> String {
    format!("{certificate_arn} is not a valid ARN")
}

/// The `ResourceNotFoundException` message for an ACM certificate ARN that no stored
/// record answers to, built from the ARN the request asked for.
///
/// `ResourceNotFoundException` carries one modelled member (`message: String`) and the
/// model documents no text for it, so the sentence comes from ACM's recorded
/// behaviour: one complete sentence naming the requested ARN and terminated by a
/// period. Every operation that resolves a certificate ARN renders it here, so the
/// template cannot drift between the lookup paths.
pub fn certificate_not_found_message(certificate_arn: &str) -> String {
    format!("Could not find certificate {certificate_arn}.")
}

/// The `InvalidStateException` message for a request that needs an issued, valid
/// certificate and names one that is neither.
///
/// `InvalidStateException` carries one modelled member (`message: String`) and the
/// model documents no text for it, so the sentence comes from ACM's recorded
/// behaviour: ONE constant sentence for the whole refusal class, naming neither the
/// certificate nor the status it is in. Rendered here so every operation that requires
/// an issued certificate reports the same sentence.
pub const NOT_ISSUED_MESSAGE: &str = "The selected certificate is not Issued and/or Valid.";

/// The `ValidationException` message for an operation asked of a certificate export
/// was never allowed for, built from the ARN the request asked for.
///
/// Export is opt-in at creation (`CertificateOptions$Export` doc), and an operation
/// that only applies to an exported certificate — `RevokeCertificate`: "You can only
/// revoke certificates that have been previously exported" — cannot apply to one that
/// may not be exported at all. `ValidationException` carries one modelled member
/// (`message: String`) and the model documents no text for it, so the sentence comes
/// from ACM's recorded behaviour: `Export not allowed for certificate` followed by the
/// requested ARN. Rendered here so every export-eligibility path reports the same
/// sentence.
pub fn export_not_allowed_message(certificate_arn: &str) -> String {
    format!("Export not allowed for certificate {certificate_arn}")
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

/// A fingerprint of the parameters one request builds a record from, for an operation
/// that honours an `@idempotencyToken`: re-sending a token must answer with what it
/// already created, and the same token on a DIFFERENT request must be told apart from
/// it rather than answering 200 while the change is discarded.
///
/// The parts are hashed as GROUPS: each group behind its own part COUNT, and every part
/// inside it behind its own length. That is what makes the framing unambiguous — no
/// value can move between groups, or run into its neighbour, and leave the fingerprint
/// unchanged. A group marker pushed in-band as a part could not do that, since every
/// part is caller-controlled text.
///
/// Both operations that key an index on a token (`RequestCertificate`,
/// `CreateAcmeEndpoint`) render it here, so the framing lives in one place.
pub fn fingerprint_groups(groups: &[&[&str]]) -> String {
    use sha2::Digest;

    let mut hasher = sha2::Sha256::new();
    for group in groups {
        hasher.update(group.len().to_le_bytes());
        for part in *group {
            hasher.update(part.len().to_le_bytes());
            hasher.update(part.as_bytes());
        }
    }
    hex::encode(hasher.finalize())
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

/// The mailboxes ACM sends a domain's validation email to, at `mail_suffix`.
///
/// "ACM sends email to the the following five addresses:
/// admin@<suffix> administrator@<suffix> hostmaster@<suffix> postmaster@<suffix>
/// webmaster@<suffix>" (`ResendValidationEmailRequest$ValidationDomain` doc), i.e.
/// [`VALIDATION_EMAIL_LOCAL_PARTS`] at the validation domain. Rendered here so the
/// address set a `RequestCertificate` records and the one a `ResendValidationEmail`
/// records cannot drift apart.
pub fn validation_mailboxes(mail_suffix: &str) -> Vec<String> {
    VALIDATION_EMAIL_LOCAL_PARTS
        .iter()
        .map(|local_part| format!("{local_part}@{mail_suffix}"))
        .collect()
}

/// Whether two domain names name the same domain. DNS labels are case-insensitive,
/// so `Example.COM` and `example.com` are one name.
pub fn same_domain(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

/// Whether `validation_domain` may validate `domain_name`: the superdomain rule ACM
/// states for every `ValidationDomain` member it models
/// (`DomainValidationOption$ValidationDomain`: "This must be the same as the
/// `DomainName` value or a superdomain of the `DomainName` value";
/// `ResendValidationEmailRequest$ValidationDomain` says the same of `Domain`).
///
/// A superdomain is a DOT-BOUNDARY suffix: `example.com` validates
/// `testing.example.com`, while `ample.com` and `other.example.com` do not. The
/// comparison is case-insensitive (see [`same_domain`]), and a wildcard name is
/// validated through the domain it covers, which is the name ACM proves control of
/// ([`validation_target`]).
pub fn is_validation_domain_for(validation_domain: &str, domain_name: &str) -> bool {
    if same_domain(validation_domain, domain_name) {
        return true;
    }
    let candidate = validation_domain.to_ascii_lowercase();
    let target = validation_target(domain_name).to_ascii_lowercase();
    candidate == target
        || target
            .strip_suffix(&candidate)
            .is_some_and(|higher_labels| higher_labels.ends_with('.'))
}

/// The validation ACM is waiting on for one domain name of one certificate.
///
/// DNS validation publishes a CNAME, email validation names the mailboxes ACM wrote
/// to, and HTTP validation names the URL the CA fetches; `validation_method` decides
/// which. Every value is derived from the certificate's own id, so the same
/// `(certificate, domain, method)` always yields the same record — a later
/// `DescribeCertificate` reports exactly what was stored, and a certificate migrated
/// back to a method it already used reports the record it had.
///
/// Shared, because the method a certificate validates its domains by is set when the
/// certificate is requested and can be changed afterwards
/// (`CertificateOptions$ValidationMethod`), so both paths must mint the same record.
pub fn domain_validation_record(
    certificate_id: &str,
    domain_name: &str,
    validation_method: &str,
    validation_domain: Option<&str>,
) -> DomainValidationRecord {
    let target = validation_target(domain_name);
    let mut record = DomainValidationRecord {
        domain_name: domain_name.to_string(),
        validation_status: DOMAIN_STATUS_PENDING_VALIDATION.to_string(),
        validation_method: validation_method.to_string(),
        validation_domain: None,
        requested_validation_domain: validation_domain.map(str::to_string),
        validation_emails: Vec::new(),
        resource_record: None,
        http_redirect: None,
    };
    match validation_method {
        "DNS" => {
            let name = validation_label(certificate_id, domain_name, "dns-name");
            let value = validation_label(certificate_id, domain_name, "dns-value");
            record.resource_record = Some(ResourceRecord {
                name: format!("_{name}.{target}."),
                r#type: RECORD_TYPE_CNAME.to_string(),
                value: format!("_{value}.{ACM_VALIDATIONS_ZONE}."),
            });
        }
        "HTTP" => {
            let token = validation_label(certificate_id, domain_name, "http-token");
            let path = HTTP_VALIDATION_PATH;
            record.http_redirect = Some(HttpRedirect {
                redirect_from: Some(format!("http://{target}{path}{token}.txt")),
                redirect_to: Some(format!(
                    "http://{token}.{ACM_VALIDATIONS_ZONE}{path}{token}.txt"
                )),
            });
        }
        // EMAIL, and the default when no method was named.
        _ => {
            // The validation domain is the SUFFIX of the mailboxes ACM writes to, and
            // a mail suffix carries no wildcard label: a request that validates
            // `*.example.com` through itself is mailed at `example.com`, the domain
            // ACM proves control of.
            let mailed = validation_domain.map(validation_target).unwrap_or(target);
            record.validation_domain = Some(mailed.to_string());
            record.validation_emails = validation_mailboxes(mailed);
        }
    }
    record
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
        check_tag_not_reserved(tag)?;
        mapped.insert(tag.key.clone(), tag.value.clone().unwrap_or_default());
    }
    Ok(mapped)
}

/// `Ok(())` for a tag any caller may name, or the `InvalidTagException` message for
/// one it may not.
///
/// The `aws:` prefix is reserved on both halves of the pair, which is what
/// `InvalidTagException` names ("you cannot specify a tag value that begins with
/// aws:"). Shared by every operation that takes tags from a request, so the tagging
/// and the untagging paths refuse the same pairs with the same sentence.
pub fn check_tag_not_reserved(tag: &Tag) -> Result<(), String> {
    if tag.key.starts_with("aws:") || tag.value.as_deref().is_some_and(|v| v.starts_with("aws:")) {
        return Err(format!(
            "tag {} is reserved: neither a tag key nor its value may begin with aws:",
            tag.key
        ));
    }
    Ok(())
}

/// Drop the requested tags from a stored tag map, as ACM's untagging operations
/// define it: a requested pair that carries no value removes its key whatever value
/// the key holds, and one that carries a value removes the key only when the stored
/// value is that value (`RemoveTagsFromCertificate` doc: "If you do not specify the
/// value portion of the tag when calling this function, the tag will be removed
/// regardless of value. If you specify a value, the tag is removed only if it is
/// associated with the specified value").
///
/// Entries are applied in request order and a key the map does not hold is simply
/// absent afterwards, so removing is idempotent.
pub fn remove_tags(stored: &mut BTreeMap<String, String>, requested: &[Tag]) {
    for tag in requested {
        let matches = match tag.value.as_deref() {
            None => stored.contains_key(&tag.key),
            Some(value) => stored
                .get(&tag.key)
                .is_some_and(|stored_value| stored_value == value),
        };
        if matches {
            stored.remove(&tag.key);
        }
    }
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

/// The account's expiry-events configuration as the modelled
/// `ExpiryEventsConfiguration`: what `PutAccountConfiguration` stored, or the
/// documented default for an account that has written none.
pub fn expiry_events_configuration(
    stored: Option<&AccountConfiguration>,
) -> ExpiryEventsConfiguration {
    ExpiryEventsConfiguration {
        days_before_expiry: match stored {
            // A write that named an `ExpiryEvents` with no `DaysBeforeExpiry` left
            // the option unset, so the account is back on the default.
            Some(config) => config
                .days_before_expiry
                .or(Some(DEFAULT_DAYS_BEFORE_EXPIRY)),
            None => Some(DEFAULT_DAYS_BEFORE_EXPIRY),
        },
    }
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
        // Both "exist only when the certificate status is REVOKED"
        // (`CertificateDetail$RevokedAt` / `$RevocationReason` docs), and the record
        // only ever carries them together with that status.
        revoked_at: cert.revoked_at.map(to_date_time),
        revocation_reason: cert.revocation_reason.clone(),
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
        // Present only for a revoked certificate, as on `CertificateDetail`.
        revoked_at: cert.revoked_at.map(to_date_time),
        managed_by: cert.managed_by.clone(),
        certificate_key_pair_origin: Some(cert.key_pair_origin.clone()),
        ..CertificateSummary::default()
    }
}

/// A stored ACME endpoint as `DescribeAcmeEndpoint`'s `AcmeEndpoint`.
///
/// `FailureReason` exists only for an endpoint that failed
/// (`AcmeEndpoint$FailureReason` doc), which is what the record stores, and
/// `CertificateTags` is reported only when the endpoint carries tags to apply to the
/// certificates it issues — a create that named none leaves the member absent rather
/// than reporting an empty list (`TagList` has a minimum length of 1, so an empty list
/// is not a value the member can take).
pub fn acme_endpoint_detail(endpoint: &AcmeEndpointRecord) -> AcmeEndpoint {
    AcmeEndpoint {
        acme_endpoint_arn: Some(endpoint.arn.clone()),
        endpoint_url: Some(endpoint.endpoint_url.clone()),
        status: Some(endpoint.status.clone()),
        failure_reason: endpoint.failure_reason.clone(),
        authorization_behavior: Some(endpoint.authorization_behavior.clone()),
        contact: Some(endpoint.contact.clone()),
        certificate_authority: Some(endpoint.certificate_authority.clone()),
        certificate_tags: (!endpoint.certificate_tags.is_empty())
            .then(|| tag_list(&endpoint.certificate_tags)),
        created_at: Some(to_date_time(endpoint.created_at)),
        updated_at: Some(to_date_time(endpoint.updated_at)),
    }
}

/// A stored certificate as `SearchCertificates`' `CertificateSearchResult`: the same
/// record `certificate_detail` and `certificate_summary` project, split into the
/// X.509 attributes and the ACM metadata that operation's filters select on.
///
/// Members that belong to something this service never does stay absent, and are the
/// members the search filters correspondingly keep nothing on:
///
/// * `X509Attributes.Issuer` and `X509Attributes.SerialNumber` are minted by the
///   certificate authority that signs an X.509 certificate, and this emulator signs
///   none — the same reason [`certificate_detail`] reports no `Issuer` or `Serial`.
/// * `AcmCertificateMetadata.ImportedAt` dates an import, `RenewalStatus` belongs to
///   a renewal ACM has started, and the ACME members to a certificate obtained
///   through an ACME endpoint; no stored record carries any of them. `RevokedAt` is
///   reported, and is absent exactly when the record carries no revocation, which
///   "exists only when the certificate status is REVOKED"
///   (`CertificateDetail$RevokedAt` doc).
pub fn certificate_search_result(cert: &Certificate) -> CertificateSearchResult {
    CertificateSearchResult {
        certificate_arn: Some(cert.arn.clone()),
        x509_attributes: Some(X509Attributes {
            // The subject common name of an ACM certificate is the domain name the
            // certificate was requested for.
            subject: Some(DistinguishedName {
                common_name: Some(cert.domain_name.clone()),
                ..DistinguishedName::default()
            }),
            // The name set the certificate covers, each as the DNS name it is.
            subject_alternative_names: Some(
                cert.subject_alternative_names
                    .iter()
                    .map(|name| GeneralName::DnsName(name.clone()))
                    .collect(),
            ),
            key_algorithm: Some(cert.key_algorithm.clone()),
            // The X.509 key-usage extensions of an issued certificate; absent while
            // the record carries none.
            key_usages: (!cert.key_usages.is_empty()).then(|| cert.key_usages.clone()),
            extended_key_usages: (!cert.extended_key_usages.is_empty())
                .then(|| cert.extended_key_usages.clone()),
            not_before: cert.not_before.map(to_date_time),
            not_after: cert.not_after.map(to_date_time),
            ..X509Attributes::default()
        }),
        certificate_metadata: Some(CertificateMetadata::AcmCertificateMetadata(
            AcmCertificateMetadata {
                created_at: Some(to_date_time(cert.created_at)),
                exported: Some(cert.exported),
                in_use: Some(!cert.in_use_by.is_empty()),
                issued_at: cert.issued_at.map(to_date_time),
                // The revocation instant, which exists only beside the `REVOKED`
                // status the same record carries.
                revoked_at: cert.revoked_at.map(to_date_time),
                renewal_eligibility: Some(cert.renewal_eligibility.clone()),
                status: Some(cert.status.clone()),
                r#type: Some(cert.cert_type.clone()),
                export_option: Some(export_option(cert)),
                managed_by: cert.managed_by.clone(),
                validation_method: cert.validation_method.clone(),
                certificate_key_pair_origin: Some(cert.key_pair_origin.clone()),
                ..AcmCertificateMetadata::default()
            },
        )),
    }
}
