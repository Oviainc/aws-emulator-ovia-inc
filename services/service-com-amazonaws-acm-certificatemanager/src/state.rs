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
//! [`acme_endpoint_detail`] and `ListAcmeEndpoints` through [`acme_endpoint_summary`],
//! and a freshly created endpoint is `CREATING` until the
//! transition the clock owes it falls due, exactly as a pending certificate is.
//!
//! Beside the endpoints, one [`AcmeAccountRecord`] per ACME account registered with
//! one of them, keyed by the (endpoint ARN, account URL) pair every ACME-account
//! operation addresses an account by. `ListAcmeAccounts` reports that endpoint's
//! records through [`acme_account_summary`], and deleting an endpoint drops the
//! accounts registered with it ([`ComAmazonawsAcmCertificatemanagerState::remove_acme_endpoint`]).
//!
//! Beside them, one [`AcmeDomainValidationRecord`] per domain validation an endpoint
//! holds, keyed by its own ARN — which is a PATH under the endpoint's ARN
//! ([`acme_domain_validation_arn`]), so one endpoint's validations are a contiguous
//! range ([`ComAmazonawsAcmCertificatemanagerState::acme_domain_validations_of`]) and
//! deleting the endpoint drops them with it. `DescribeAcmeDomainValidation` projects
//! that record through [`acme_domain_validation_detail`] and
//! `ListAcmeDomainValidations` through [`acme_domain_validation_summary`], including the
//! DNS record the service minted for it ([`acme_prevalidation_resource_record`]), and a freshly
//! created validation is `VALIDATING` until the transition the clock owes it falls due,
//! exactly as a freshly created endpoint is `CREATING`.
//!
//! Every URL those two records carry is minted here, on the Region's ACME enrollment
//! host ([`ACME_ENROLLMENT_HOST_LABEL`]) with the endpoint's own id in the path
//! ([`acme_endpoint_url`], [`acme_account_url`]) — and read back by
//! [`parse_acme_request_path`] and [`acme_enrollment_region`], so the ACME HTTP surface
//! `crate::service::acme_http` serves resolves a published URL to the record behind it.
//!
//! Beside the certificates sits the account-level singleton
//! [`AccountConfiguration`]: `PutAccountConfiguration` writes it and
//! `GetAccountConfiguration` reports it, so that round trip reads one record too.
//!
//! A requested certificate does not stay `PENDING_VALIDATION`: ACM resolves its
//! validation on its own, so the record carries the outcome the clock owes it and the
//! instant it falls due ([`PendingValidation`]), and — for a certificate it validates
//! by EMAIL — the instant the initial validation mail it owes falls due
//! ([`Certificate::pending_validation_email`]). Nothing sleeps for those instants —
//! [`ComAmazonawsAcmCertificatemanagerState::materialize_due`] applies whatever was
//! already due, and every handler calls it before reading or writing the store.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AcmCertificateMetadata, AcmeAccountSummary, AcmeDomainValidation, AcmeDomainValidationSummary,
    AcmeEndpoint,
    AcmeEndpointSummary,
    CertificateAuthority,
    CertificateDetail, CertificateMetadata, CertificateOptions, CertificateSearchResult,
    CertificateSummary, DistinguishedName, DnsPrevalidationDetails, DomainScope, DomainValidation,
    ExpiryEventsConfiguration,
    ExtendedKeyUsage, FailureDetails, GeneralName, HttpRedirect, KeyUsage, PrevalidationDetails,
    ResourceRecord, Tag, X509Attributes,
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
///
/// It sits ABOVE [`VALIDATION_EMAIL_DISPATCH_DELAY`] because the two owed transitions
/// happen in that order at AWS: ACM dispatches a request's validation mail first and
/// only then reaches an outcome for the request, so the compressed spans keep the same
/// order and each stays inside the observable band.
pub const VALIDATION_SETTLE_DELAY: Duration = Duration::from_millis(750);

/// How long after `RequestCertificate` the initial domain-validation EMAIL falls due.
///
/// A CHOSEN span, not a documented one: AWS dispatches the mail itself, shortly after
/// the request is accepted and well before the request reaches an outcome. It is never
/// zero, so a caller that resends straight after requesting still observes the state
/// where no mail has gone out yet, and it is never slept on: the due instant is stored
/// on the record ([`Certificate::pending_validation_email`]) and materialised by
/// whichever call next touches the store.
pub const VALIDATION_EMAIL_DISPATCH_DELAY: Duration = Duration::from_millis(250);

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

/// `CertificateTransparencyLoggingPreference` when `Options` names none: "All public
/// certificates are recorded in a certificate transparency log" and "certificate
/// transparency logging opt-out is no longer available" (`CertificateOptions` doc), so
/// a certificate that asked for nothing is logged.
pub const TRANSPARENCY_LOGGING_ENABLED: &str = "ENABLED";

/// `CertificateDetail$Issuer` ("the name of the certificate authority that issued and
/// signed the certificate") for a public certificate ACM issues: Amazon's own CA
/// signs it.
pub const ISSUER_AMAZON: &str = "Amazon";

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

/// `AcmeDomainValidationStatus` of a domain validation whose prevalidation ACM is
/// still working through: `AcmeDomainValidationStatus` models `VALIDATING` beside
/// `VALID` precisely because ACM has not finished proving domain ownership the instant
/// `CreateAcmeDomainValidation` returns
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#AcmeDomainValidationStatus")`
/// declares `VALIDATING | VALID | INVALID | DELETING`).
pub const ACME_DOMAIN_VALIDATION_STATUS_VALIDATING: &str = "VALIDATING";

/// `AcmeDomainValidationStatus` of a domain validation that authorizes its endpoint to
/// issue certificates for the domain: "domain validations authorize the endpoint to
/// issue certificates for specified domain names" (`CreateAcmeDomainValidation` doc).
pub const ACME_DOMAIN_VALIDATION_STATUS_VALID: &str = "VALID";

/// `AcmeDomainValidationStatus` of a domain validation ACM could not complete; it is
/// the status whose `FailureDetails` ("details about the failure, if the validation
/// failed", `AcmeDomainValidation$FailureDetails` doc) is set. No path in this emulator
/// reaches it yet: nothing here can fail a prevalidation, so a created validation
/// settles to [`ACME_DOMAIN_VALIDATION_STATUS_VALID`], and the record's
/// `failure_details` is what an operation that CAN fail one would write beside this
/// status.
pub const ACME_DOMAIN_VALIDATION_STATUS_INVALID: &str = "INVALID";

/// `PrevalidationType` of a domain validation configured with
/// `PrevalidationOptions$DnsPrevalidation`: the only member either union declares
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#PrevalidationType")` declares
/// `DNS_PREVALIDATION` alone).
pub const PREVALIDATION_TYPE_DNS: &str = "DNS_PREVALIDATION";

/// How long after `CreateAcmeDomainValidation` the validation reaches
/// [`ACME_DOMAIN_VALIDATION_STATUS_VALID`].
///
/// A CHOSEN span, not a documented one: neither the model nor ACM's documentation
/// states how long prevalidating a domain takes. It is never zero, so a caller that
/// describes the validation straight after creating it still observes `VALIDATING`, and
/// it is never slept on: the due instant is stored on the record and materialised by
/// whichever call next touches the store (see
/// [`ComAmazonawsAcmCertificatemanagerState::materialize_due`]).
pub const ACME_DOMAIN_VALIDATION_SETTLE_DELAY: Duration = Duration::from_millis(500);

/// The number of domain validations one ACME endpoint may hold.
///
/// Neither the model nor ACM's documentation states this quota, so the NUMBER is
/// chosen — generously, since a bound that is too low refuses a request ACM would
/// accept. What it encodes is that an endpoint's domain validations are a bounded
/// resource and that past the bound `CreateAcmeDomainValidation` answers the quota
/// error it models.
pub const ACME_DOMAIN_VALIDATION_LIMIT: usize = 500;

/// The most ACME domain-validation records this emulator stores, across every endpoint.
///
/// The safety bound, for the same reason [`ACME_ENDPOINT_STORE_LIMIT`] is one: the
/// per-endpoint quota above is keyed on a parent the caller can create more of, so it
/// alone would not bound
/// [`ComAmazonawsAcmCertificatemanagerState::acme_domain_validations`] (nor the
/// per-request `materialize_due` pass over those records).
pub const ACME_DOMAIN_VALIDATION_STORE_LIMIT: usize = 10_000;

/// `AcmeAccountStatus` of a registered ACME account that may request and revoke
/// certificates: the status an account carries until it is deactivated or revoked
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#AcmeAccountStatus")`
/// declares `VALID | DEACTIVATED | REVOKED`).
pub const ACME_ACCOUNT_STATUS_VALID: &str = "VALID";

/// `AcmeAccountStatus` of an ACME account its own client deactivated (RFC 8555 §7.3.6
/// account deactivation), which ACM reports beside the status it reaches when ACM
/// revokes it.
pub const ACME_ACCOUNT_STATUS_DEACTIVATED: &str = "DEACTIVATED";

/// `AcmeAccountStatus` of an ACME account `RevokeAcmeAccount` has revoked, which
/// "prevent[s] it from requesting or revoking certificates" (`RevokeAcmeAccount` doc).
pub const ACME_ACCOUNT_STATUS_REVOKED: &str = "REVOKED";

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
    /// The domain ACM proves control of for this name — "the domain name that ACM
    /// used to send domain validation emails" — which is the name itself unless the
    /// request named another in `DomainValidationOptions`.
    pub validation_domain: Option<String>,
    /// The validation domain the certificate was REQUESTED with for this name
    /// (`DomainValidationOption$ValidationDomain`), kept whatever method is in force,
    /// so a certificate migrated away from `EMAIL` and back mails the domain the
    /// request named rather than the covered name itself.
    pub requested_validation_domain: Option<String>,
    /// The mailboxes ACM has sent a domain-validation email to: the five role-account
    /// addresses at [`Self::validation_domain`], for a name ACM validates BY EMAIL.
    /// Empty until the mail has actually gone out — the initial dispatch the clock owes
    /// a request ([`Certificate::pending_validation_email`]), or a
    /// `ResendValidationEmail` — and empty for a name validated by DNS or HTTP, where a
    /// mailbox list does not apply at all; [`domain_validation`] reports the difference
    /// from the record's own method.
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
    /// A `KeyAlgorithm` value, in the model enum's spelling (`RSA_2048`) — the
    /// spelling the request and the `Includes.keyTypes` filter use. The detail
    /// projection reports it through [`detail_key_algorithm`].
    pub key_algorithm: String,
    /// "The name of the entity that is associated with the public key contained in the
    /// certificate" (`CertificateDetail$Subject`): the subject distinguished name,
    /// which ACM fixes when it accepts the request — see
    /// [`subject_distinguished_name`].
    pub subject: Option<String>,
    /// The name of the CA that signs the certificate ([`issuer_name`]).
    pub issuer: Option<String>,
    /// The algorithm that CA signs with ([`signature_algorithm`]).
    pub signature_algorithm: Option<String>,
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
    /// initial dispatch fell due, moved forward by each `ResendValidationEmail`. `None`
    /// while no mail has gone out yet — a certificate ACM validates by DNS or HTTP, and
    /// one whose initial dispatch is still owed. It is what the documented
    /// [`RESEND_VALIDATION_WINDOW`] is measured from.
    pub validation_email_sent_at: Option<SystemTime>,
    /// When the INITIAL domain-validation email ACM owes this request falls due, for a
    /// certificate it validates by EMAIL. `None` once the mail has gone out, for a
    /// certificate ACM sends no validation mail for at all, and once the request has
    /// settled with the mail still owed — a request ACM has finished processing gets no
    /// validation mail afterwards.
    ///
    /// Stored, never slept on, exactly as [`Self::pending_validation`] is: the instant
    /// is fixed by the call that accepted the request, so the mail is owed at that
    /// instant whether or not anybody calls again.
    pub pending_validation_email: Option<SystemTime>,
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
    /// Apply everything the clock already owes this certificate at `now`: the initial
    /// domain-validation email, then the validation outcome.
    ///
    /// A certificate ACM reports as `PENDING_VALIDATION` is waiting on work ACM does
    /// on its own; the instant each piece of that work completes was fixed when the
    /// certificate was requested, so applying it here materialises a change the clock
    /// already made rather than a side effect of whoever happened to call.
    ///
    /// The two are applied in DUE ORDER, so the record passes through the same states
    /// it would have if somebody had called in between.
    pub fn materialize_due(&mut self, now: SystemTime) {
        if let Some(due) = self.pending_validation_email {
            // The outcome is applied first when it falls due first: a request ACM has
            // finished processing sends no validation mail afterwards.
            let outcome_first = self
                .pending_validation
                .as_ref()
                .is_some_and(|pending| pending.due_at < due);
            if now >= due && !outcome_first {
                self.pending_validation_email = None;
                self.dispatch_validation_email(due);
            }
        }
        let Some(pending) = self
            .pending_validation
            .clone()
            .filter(|pending| now >= pending.due_at)
        else {
            return;
        };
        self.pending_validation = None;
        // The request has reached an outcome, so no validation mail is owed for it any
        // more.
        self.pending_validation_email = None;
        match pending.outcome {
            ValidationOutcome::Issued => self.issue(pending.due_at),
            ValidationOutcome::Failed => {
                self.fail(FAILURE_REASON_ADDITIONAL_VERIFICATION_REQUIRED);
            }
        }
    }

    /// Record the domain-validation email ACM sent at `at` for every name this
    /// certificate validates BY EMAIL.
    ///
    /// "ACM sends email to the the following five addresses: admin@<suffix>
    /// administrator@<suffix> hostmaster@<suffix> postmaster@<suffix>
    /// webmaster@<suffix>" (`ResendValidationEmailRequest$ValidationDomain` doc), at
    /// each name's own validation domain — the suffix the record already carries. A
    /// name validated by DNS or HTTP gets no mail, so its mailbox list is left as it is.
    ///
    /// `at` is the instant the mail went out, which is what the documented
    /// [`RESEND_VALIDATION_WINDOW`] is measured from — not the instant somebody
    /// happened to read the record.
    pub fn dispatch_validation_email(&mut self, at: SystemTime) {
        for validation in &mut self.domain_validations {
            if validation.validation_method != VALIDATION_METHOD_EMAIL {
                continue;
            }
            let Some(mail_suffix) = validation.validation_domain.clone() else {
                continue;
            };
            validation.validation_emails = validation_mailboxes(&mail_suffix);
        }
        self.validation_email_sent_at = Some(at);
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
    /// validation emails" (`DomainValidation$ValidationEmails` doc) — a record of mail
    /// ACM HAS sent, which a request ending unproven does not retract, so each list is
    /// left exactly as the dispatches that wrote it left it. A request that failed
    /// before its mail went out never had one: the owed dispatch is dropped by
    /// [`Self::materialize_due`], so that list is still empty.
    fn fail(&mut self, reason: &str) {
        self.status = STATUS_FAILED.to_string();
        self.failure_reason = Some(reason.to_string());
        for validation in &mut self.domain_validations {
            validation.validation_status = DOMAIN_STATUS_FAILED.to_string();
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

/// The status transition the clock already owes an ACME domain validation.
///
/// Stored, never slept on, for the same reason [`PendingAcmeEndpointStatus`] is: the
/// instant it falls due is fixed by the call that started the prevalidation, so the
/// validation reaches that status at that instant whether or not anybody calls again.
#[derive(Debug, Clone)]
pub struct PendingAcmeDomainValidationStatus {
    /// When the transition falls due.
    pub due_at: SystemTime,
    /// The `AcmeDomainValidationStatus` the validation carries from then on.
    pub status: String,
}

impl PendingAcmeDomainValidationStatus {
    /// The transition owed to a validation created at `created_at`: its prevalidation
    /// completes [`ACME_DOMAIN_VALIDATION_SETTLE_DELAY`] later and it becomes
    /// [`ACME_DOMAIN_VALIDATION_STATUS_VALID`].
    pub fn validating_at(created_at: SystemTime) -> Self {
        Self {
            due_at: created_at + ACME_DOMAIN_VALIDATION_SETTLE_DELAY,
            status: ACME_DOMAIN_VALIDATION_STATUS_VALID.to_string(),
        }
    }
}

/// One stored ACME domain validation: the authorization of one ACME endpoint to issue
/// certificates for one domain name ("domain validations authorize the endpoint to
/// issue certificates for specified domain names", `CreateAcmeDomainValidation` doc).
///
/// Every field `DescribeAcmeDomainValidation` reports is read from here, so no response
/// echoes its own request — including the DNS record the caller must publish, which is
/// server-managed ([`acme_prevalidation_resource_record`]).
#[derive(Debug, Clone)]
pub struct AcmeDomainValidationRecord {
    /// `arn:aws:acm:<region>:<account>:acme-endpoint/<endpoint>/acme-domain-validation/<id>`
    /// ([`acme_domain_validation_arn`]).
    pub arn: String,
    /// The ACME endpoint this validation authorizes — the ARN prefix of [`Self::arn`],
    /// carried so the record reports it without re-parsing.
    pub acme_endpoint_arn: String,
    /// The domain name being validated, exactly as the create named it.
    pub domain_name: String,
    /// A `PrevalidationType` value: which `PrevalidationOptions` member the create
    /// carried.
    pub prevalidation_type: String,
    /// "The scope of domains covered by this prevalidation"
    /// (`DnsPrevalidationOptions$DomainScope` doc), as the create configured it.
    pub domain_scope: Option<DomainScope>,
    /// "The Route 53 hosted zone ID for DNS validation"
    /// (`DnsPrevalidationOptions$HostedZoneId` doc), as the create named it.
    pub hosted_zone_id: Option<String>,
    /// "The DNS resource record to create for domain validation"
    /// (`DnsPrevalidationDetails$ResourceRecord` doc) — minted by the service from this
    /// validation's own identity, never from anything the caller sent.
    pub resource_record: Option<ResourceRecord>,
    /// An `AcmeDomainValidationStatus` value.
    pub status: String,
    /// "Details about the failure, if the validation failed"
    /// (`AcmeDomainValidation$FailureDetails` doc), so it is set only beside an
    /// [`ACME_DOMAIN_VALIDATION_STATUS_INVALID`] status.
    pub failure_details: Option<FailureDetails>,
    /// Tag key -> value on the validation resource itself, as `ListTagsForResource`
    /// reports them.
    pub tags: BTreeMap<String, String>,
    /// The status transition the clock owes this validation. `None` once it has
    /// reached a status the clock owes nothing beyond.
    pub pending_status: Option<PendingAcmeDomainValidationStatus>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl AcmeDomainValidationRecord {
    /// Apply the status transition the clock already owes this validation at `now`.
    ///
    /// A validation ACM reports as `VALIDATING` is waiting on prevalidation work ACM
    /// does on its own; the instant that work completes was fixed when the validation
    /// was created, so applying it here materialises a change the clock already made
    /// rather than a side effect of whoever happened to call.
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
        // The validation changed at the instant the transition fell due, which is what
        // `UpdatedAt` ("the time at which the domain validation was last updated")
        // reports — not the instant somebody happened to read it.
        self.updated_at = pending.due_at;
    }
}

/// What one `CreateAcmeDomainValidation` idempotency token has already created.
#[derive(Debug, Clone)]
pub struct AcmeDomainValidationIdempotency {
    /// A canonical rendering of every parameter the validation was created from, so a
    /// re-sent token can be told apart from a changed request carrying the same token.
    pub fingerprint: String,
    /// The ARN the first call answered with.
    pub acme_domain_validation_arn: String,
}

/// One ACME account registered with one of this account's ACME endpoints.
///
/// "ACME accounts are created when clients use external account binding credentials
/// to register" (`ListAcmeAccounts` doc), i.e. by an ACME client's own `newAccount`
/// request to the endpoint's directory URL (RFC 8555 §7.3) — no operation in this
/// service's model creates one. So this map is written by whatever serves that
/// registration, and every ACME-account operation (`ListAcmeAccounts` today) REPORTS
/// these records rather than echoing its own request.
///
/// An account is identified by the key it registered with, which is why the record
/// carries the thumbprint of that key beside the `AccountUrl` the server assigned it.
#[derive(Debug, Clone)]
pub struct AcmeAccountRecord {
    /// The ACME endpoint the account is registered with: the ARN half of this
    /// record's key in [`ComAmazonawsAcmCertificatemanagerState::acme_accounts`].
    pub acme_endpoint_arn: String,
    /// The server-assigned URL of the account resource ([`acme_account_url`]), which
    /// is how every ACME-account operation addresses it.
    pub account_url: String,
    /// "The thumbprint of the public key associated with the ACME account"
    /// (`AcmeAccount$PublicKeyThumbprint` doc) — the account's identity.
    pub public_key_thumbprint: String,
    /// An `AcmeAccountStatus` value.
    pub status: String,
    /// The external account binding the registration presented, when it presented
    /// one (`AcmeAccount$AcmeExternalAccountBindingArn` doc).
    pub acme_external_account_binding_arn: Option<String>,
    /// The contact URIs the account registered with; empty when it registered none,
    /// which an endpoint whose `Contact` is `NOT_REQUIRED` admits.
    pub contacts: Vec<String>,
    pub created_at: SystemTime,
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
    /// (ACME endpoint ARN, account URL) -> the ACME account registered with that
    /// endpoint under that URL. Keyed by the pair because an ACME account is addressed
    /// by the two together (`DescribeAcmeAccount` and `RevokeAcmeAccount` both take
    /// `AcmeEndpointArn` + `AccountUrl`), and a `BTreeMap` on the pair so one
    /// endpoint's accounts are a contiguous range walked in one deterministic order —
    /// which is the order `ListAcmeAccounts` pages through
    /// ([`Self::acme_accounts_of`]).
    pub acme_accounts: BTreeMap<(String, String), AcmeAccountRecord>,
    /// ACME domain-validation ARN -> record. A `BTreeMap`, and every validation's ARN
    /// begins with its endpoint's ARN ([`acme_domain_validation_arn`]), so one
    /// endpoint's validations are a contiguous range walked in one deterministic order
    /// ([`Self::acme_domain_validations_of`]).
    pub acme_domain_validations: BTreeMap<String, AcmeDomainValidationRecord>,
    /// (ACME endpoint ARN, `CreateAcmeDomainValidation` idempotency token) -> what that
    /// token created. Keyed by the PAIR rather than a joined string, so no endpoint ARN
    /// and token can run together into another pair's key, and separate from the other
    /// token indexes because the three resolve to different resources.
    pub acme_domain_validation_idempotency:
        HashMap<(String, String), AcmeDomainValidationIdempotency>,
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
        for validation in self.acme_domain_validations.values_mut() {
            validation.materialize_due(now);
        }
    }

    /// The stored ACME endpoint the id `endpoint_id` names in `region`, or `None` when
    /// this account holds no such endpoint there.
    ///
    /// How a request to an endpoint's published ACME URL
    /// ([`acme_endpoint_url`]) resolves to the record behind it: the Region comes off
    /// the enrollment host the request addressed and the id off its path, and together
    /// they are the ARN this map is keyed by — so one Region's enrollment host serves
    /// only that Region's endpoints.
    pub fn acme_endpoint_in(&self, region: &str, endpoint_id: &str) -> Option<&AcmeEndpointRecord> {
        self.acme_endpoints
            .get(&acme_endpoint_arn(region, endpoint_id))
    }

    /// The tag map of the non-certificate ACM resource `arn` names, or `None` when
    /// this account holds no such resource.
    ///
    /// The generic tagging operations (`TagResource`, `UntagResource`,
    /// `ListTagsForResource`) apply to "all ACM resource types except the certificate
    /// resource type", so resolution walks the non-certificate resources this service
    /// persists — today the ACME endpoints, keyed by the same ARN the request names —
    /// and a well-formed ARN naming anything else has no record behind it (the caller
    /// answers `ResourceNotFoundException`). A resource type added to this struct
    /// later becomes taggable by being resolved here, not by another copy of the
    /// tagging rules.
    pub fn resource_tags(&self, arn: &str) -> Option<&BTreeMap<String, String>> {
        self.acme_endpoints
            .get(arn)
            .map(|endpoint| &endpoint.tags)
            .or_else(|| {
                self.acme_domain_validations
                    .get(arn)
                    .map(|validation| &validation.tags)
            })
    }

    /// [`Self::resource_tags`] for a caller that writes the map.
    pub fn resource_tags_mut(&mut self, arn: &str) -> Option<&mut BTreeMap<String, String>> {
        if let Some(endpoint) = self.acme_endpoints.get_mut(arn) {
            return Some(&mut endpoint.tags);
        }
        self.acme_domain_validations
            .get_mut(arn)
            .map(|validation| &mut validation.tags)
    }

    /// Drop `arn`'s ACME endpoint record and every idempotency token that resolves to
    /// it, so a replayed `CreateAcmeEndpoint` can never answer with an ARN that has no
    /// record behind it.
    pub fn remove_acme_endpoint(&mut self, arn: &str) -> Option<AcmeEndpointRecord> {
        let removed = self.acme_endpoints.remove(arn)?;
        self.acme_endpoint_idempotency
            .retain(|_, seen| seen.acme_endpoint_arn != arn);
        // An ACME account exists only on the endpoint it registered with, so the
        // endpoint's accounts go with it: "after deletion, the endpoint URL is no
        // longer accessible" (`DeleteAcmeEndpoint` doc), and an account left behind
        // would be reachable through an ARN with no record behind it.
        self.acme_accounts
            .retain(|(endpoint_arn, _), _| endpoint_arn != arn);
        // A domain validation authorizes ONE endpoint and its ARN is a path under that
        // endpoint's, so it cannot outlive the endpoint either: the deletion cascades to
        // the endpoint's validations and to the tokens that resolve to them.
        let validations: Vec<String> = self
            .acme_domain_validations_of(arn)
            .map(|validation| validation.arn.clone())
            .collect();
        for validation_arn in validations {
            self.remove_acme_domain_validation(&validation_arn);
        }
        Some(removed)
    }

    /// Every domain validation of `acme_endpoint_arn`, in the order
    /// `ListAcmeDomainValidations` pages through them (by validation ARN).
    ///
    /// A range scan over the key prefix every one of that endpoint's validation ARNs
    /// begins with, so the walk touches only that endpoint's validations however many
    /// other endpoints the store holds.
    pub fn acme_domain_validations_of<'a>(
        &'a self,
        acme_endpoint_arn: &'a str,
    ) -> impl Iterator<Item = &'a AcmeDomainValidationRecord> + 'a {
        let prefix = acme_domain_validation_arn_prefix(acme_endpoint_arn);
        self.acme_domain_validations
            .range(prefix.clone()..)
            .take_while(move |(arn, _)| arn.starts_with(&prefix))
            .map(|(_, validation)| validation)
    }

    /// The domain validation `acme_endpoint_arn` already holds for `domain_name`, if
    /// any.
    ///
    /// One endpoint authorizes one domain through ONE validation, and DNS labels are
    /// case-insensitive ([`same_domain`]), so the lookup is by domain rather than by the
    /// exact text the create carried.
    pub fn acme_domain_validation_for<'a>(
        &'a self,
        acme_endpoint_arn: &'a str,
        domain_name: &'a str,
    ) -> Option<&'a AcmeDomainValidationRecord> {
        self.acme_domain_validations_of(acme_endpoint_arn)
            .find(|validation| same_domain(&validation.domain_name, domain_name))
    }

    /// Drop `arn`'s domain-validation record and every idempotency token that resolves
    /// to it, so a replayed `CreateAcmeDomainValidation` can never answer with an ARN
    /// that has no record behind it.
    pub fn remove_acme_domain_validation(
        &mut self,
        arn: &str,
    ) -> Option<AcmeDomainValidationRecord> {
        let removed = self.acme_domain_validations.remove(arn)?;
        self.acme_domain_validation_idempotency
            .retain(|_, seen| seen.acme_domain_validation_arn != arn);
        Some(removed)
    }

    /// Every ACME account registered with `acme_endpoint_arn`, in the order
    /// `ListAcmeAccounts` pages through them (by account URL).
    ///
    /// A range scan over the key pair's first half, so the walk touches only that
    /// endpoint's accounts however many other endpoints the store holds.
    pub fn acme_accounts_of<'a>(
        &'a self,
        acme_endpoint_arn: &'a str,
    ) -> impl Iterator<Item = &'a AcmeAccountRecord> + 'a {
        self.acme_accounts
            .range((acme_endpoint_arn.to_string(), String::new())..)
            .take_while(move |((endpoint_arn, _), _)| endpoint_arn == acme_endpoint_arn)
            .map(|(_, account)| account)
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

/// The `acme-domain-validation` segment every ACME domain-validation ARN carries
/// between its endpoint's ARN and its own id, as `AcmeDomainValidationArn`'s
/// `@pattern` requires
/// (`^arn:aws[a-z-]*:acm:[a-z0-9-]+:[0-9]{12}:acme-endpoint/[a-zA-Z0-9-]+/acme-domain-validation/[a-zA-Z0-9-]+$`).
pub const ACME_DOMAIN_VALIDATION_SEGMENT: &str = "acme-domain-validation";

/// `arn:aws:acm:<region>:<account>:acme-endpoint/<endpoint>/acme-domain-validation/<id>`.
///
/// Built by extending the ENDPOINT's own ARN, which is what the pattern above says the
/// ARN of a domain validation is: a path under the endpoint it authorizes. So the two
/// ARNs cannot disagree about the Region or the account, and the store keyed by this
/// ARN groups one endpoint's validations into one contiguous range
/// ([`acme_domain_validation_arn_prefix`]).
pub fn acme_domain_validation_arn(acme_endpoint_arn: &str, validation_id: &str) -> String {
    format!(
        "{}{validation_id}",
        acme_domain_validation_arn_prefix(acme_endpoint_arn)
    )
}

/// The prefix every domain-validation ARN of `acme_endpoint_arn` begins with, and
/// nothing else does: the endpoint's ARN plus the
/// [`ACME_DOMAIN_VALIDATION_SEGMENT`] path. An endpoint id admits no `/`
/// (`[a-zA-Z0-9-]+`), so one endpoint's prefix is never a prefix of another's.
pub fn acme_domain_validation_arn_prefix(acme_endpoint_arn: &str) -> String {
    format!("{acme_endpoint_arn}/{ACME_DOMAIN_VALIDATION_SEGMENT}/")
}

/// The `ResourceNotFoundException` message for an ACME domain-validation ARN that no
/// stored record answers to, built from the ARN the request asked for.
///
/// `ResourceNotFoundException` carries one modelled member (`message: String`) and the
/// model documents no text for it, so the sentence follows the template every other
/// lookup in this service renders ([`certificate_not_found_message`]): one complete
/// sentence naming the requested ARN and terminated by a period.
pub fn acme_domain_validation_not_found_message(acme_domain_validation_arn: &str) -> String {
    format!("Could not find ACME domain validation {acme_domain_validation_arn}.")
}

/// The leading label of the host ACM serves its managed ACME endpoints on.
///
/// ACM publishes ONE enrollment host per Region and gives every endpoint in that
/// Region a URL on it: the recorded behaviour grades `AcmeEndpoint$EndpointUrl` as
/// being on `acm-acme-enroll.<region>.<dns suffix>`. The endpoint's own id therefore
/// identifies it in the PATH ([`acme_endpoint_base_url`]) and never as a DNS label —
/// an ACME client is configured with this URL as its ACME server, so a host of a form
/// the service does not publish is one the client's TLS/SNI, and any host-based
/// routing or allow-list built around the enrollment host, cannot match.
pub const ACME_ENROLLMENT_HOST_LABEL: &str = "acm-acme-enroll";

/// The DNS suffix of the AWS endpoint hosts this emulator publishes.
pub const AWS_DNS_SUFFIX: &str = "amazonaws.com";

/// The path prefix every ACME resource of every endpoint sits under, so one
/// enrollment host serves every endpoint in its Region and a path that is not an ACME
/// resource path is told apart from one that is.
pub const ACME_PATH_PREFIX: &str = "/acme/";

/// The ACME `directory` resource: the single URL an ACME client is configured with and
/// starts from, which answers the resource map (RFC 8555 §7.1.1).
pub const ACME_DIRECTORY_RESOURCE: &str = "directory";

/// The ACME `newNonce` resource: it answers a fresh anti-replay nonce in a
/// `Replay-Nonce` header (RFC 8555 §7.2).
pub const ACME_NEW_NONCE_RESOURCE: &str = "new-nonce";

/// The ACME `newAccount` resource, where a client registers an account (RFC 8555 §7.3).
pub const ACME_NEW_ACCOUNT_RESOURCE: &str = "new-account";

/// The ACME `newOrder` resource, where a client asks for a certificate (RFC 8555 §7.4).
pub const ACME_NEW_ORDER_RESOURCE: &str = "new-order";

/// The ACME `revokeCert` resource (RFC 8555 §7.6).
pub const ACME_REVOKE_CERTIFICATE_RESOURCE: &str = "revoke-cert";

/// The ACME `keyChange` resource, where an account rolls its key over (RFC 8555 §7.3.5).
pub const ACME_KEY_CHANGE_RESOURCE: &str = "key-change";

/// The resource-type segment of an ACME account's own URL ([`acme_account_url`]).
pub const ACME_ACCOUNT_RESOURCE: &str = "acct";

/// `acm-acme-enroll.<region>.amazonaws.com`, the host ACM serves the Region's ACME
/// endpoints on ([`ACME_ENROLLMENT_HOST_LABEL`]).
pub fn acme_enrollment_host(region: &str) -> String {
    format!("{ACME_ENROLLMENT_HOST_LABEL}.{region}.{AWS_DNS_SUFFIX}")
}

/// The Region whose ACME enrollment host `host` is, or `None` for any other host.
///
/// The inverse of [`acme_enrollment_host`], so the host this service PUBLISHES is the
/// host it recognises: the first label must be [`ACME_ENROLLMENT_HOST_LABEL`], the
/// second is the Region, and at least one further non-empty label must follow it (the
/// DNS suffix). The Region is held to the character set `AcmeEndpointArn`'s own
/// `@pattern` allows for it (`[a-z0-9-]+`), because it is then the Region half of the
/// ARN the request's endpoint record is looked up by.
///
/// Any userinfo and port are removed first, as they are part of the authority rather
/// than of the host name.
pub fn acme_enrollment_region(host: &str) -> Option<&str> {
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = host.split(':').next().unwrap_or(host);
    let mut labels = host.split('.');
    if labels.next() != Some(ACME_ENROLLMENT_HOST_LABEL) {
        return None;
    }
    let region = labels.next()?;
    let suffix: Vec<&str> = labels.collect();
    if suffix.is_empty() || suffix.iter().any(|label| label.is_empty()) {
        return None;
    }
    is_acme_region(region).then_some(region)
}

/// Whether `region` is a Region name an ACM ARN may carry
/// (`AcmeEndpointArn`'s `@pattern` spells the Region field `[a-z0-9-]+`).
fn is_acme_region(region: &str) -> bool {
    !region.is_empty()
        && region
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Whether `endpoint_id` is an endpoint id an ACM ACME endpoint ARN may carry
/// (`AcmeEndpointArn`'s `@pattern` spells the id `[a-zA-Z0-9-]+`).
pub fn is_acme_endpoint_id(endpoint_id: &str) -> bool {
    !endpoint_id.is_empty()
        && endpoint_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// The base URL of one endpoint's ACME surface:
/// `https://acm-acme-enroll.<region>.amazonaws.com/acme/<endpoint_id>`.
///
/// Every ACME resource of that endpoint hangs off it, which is why the endpoint's own
/// id sits here — in the path on the Region's one enrollment host — rather than in the
/// host name.
pub fn acme_endpoint_base_url(region: &str, endpoint_id: &str) -> String {
    format!(
        "https://{}{ACME_PATH_PREFIX}{endpoint_id}",
        acme_enrollment_host(region)
    )
}

/// The URL of one ACME resource of one endpoint: its base URL
/// ([`acme_endpoint_base_url`]) and the resource beneath it.
pub fn acme_resource_url(region: &str, endpoint_id: &str, resource: &str) -> String {
    format!("{}/{resource}", acme_endpoint_base_url(region, endpoint_id))
}

/// The ACME directory URL an endpoint serves on.
///
/// `AcmeEndpoint$EndpointUrl` is documented only as "The URL of the ACME endpoint",
/// and neither `CreateAcmeEndpoint` nor `UpdateAcmeEndpoint` models a member a caller
/// could propose one with, so the URL is SERVER-MANAGED: it is derived from the
/// endpoint's own id and Region, is unique per endpoint, is identical on every later
/// read, and carries nothing the caller sent. It is minted on the Region's ACME
/// enrollment host ([`ACME_ENROLLMENT_HOST_LABEL`]) and ends at the `directory`
/// resource, the single URL an ACME client is given and starts from (RFC 8555 §7.1.1)
/// — which is the URL this service SERVES (`crate::service::acme_http`).
pub fn acme_endpoint_url(region: &str, endpoint_id: &str) -> String {
    acme_resource_url(region, endpoint_id, ACME_DIRECTORY_RESOURCE)
}

/// The URL of the ACME account resource an endpoint assigned a registration.
///
/// `AcmeAccount$AccountUrl` is documented only as "The URL of the ACME account", and
/// nothing in this service's model lets a caller propose one, so what matters is the
/// invariant: the URL is SERVER-ASSIGNED, sits under the same endpoint base URL as the
/// endpoint's own directory URL ([`acme_endpoint_url`]), and is unique per account. It
/// names the `account` resource an ACME server hands back in the `Location` header of
/// a successful `newAccount` (RFC 8555 §7.3), which the client then uses as its
/// account URL.
pub fn acme_account_url(region: &str, endpoint_id: &str, account_id: &str) -> String {
    acme_resource_url(
        region,
        endpoint_id,
        &format!("{ACME_ACCOUNT_RESOURCE}/{account_id}"),
    )
}

/// What a request path on an ACME enrollment host names: the endpoint it addresses,
/// and the resource of that endpoint beneath it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcmeRequestPath {
    /// The endpoint id in the path, which is the id half of the endpoint's ARN.
    pub endpoint_id: String,
    /// Everything after the endpoint id: `directory`, `new-nonce`, `acct/<id>`, …
    pub resource: String,
}

/// The endpoint and resource an enrollment-host path names, or `None` when the path is
/// not one this service publishes.
///
/// The inverse of [`acme_resource_url`]: the paths recognised are exactly the paths
/// minted, so a URL this service hands out is a URL it can resolve back to an endpoint
/// record.
pub fn parse_acme_request_path(path: &str) -> Option<AcmeRequestPath> {
    let rest = path.strip_prefix(ACME_PATH_PREFIX)?;
    let (endpoint_id, resource) = rest.split_once('/')?;
    if !is_acme_endpoint_id(endpoint_id) || resource.is_empty() {
        return None;
    }
    Some(AcmeRequestPath {
        endpoint_id: endpoint_id.to_string(),
        resource: resource.to_string(),
    })
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

/// The resource part of an ACM ARN
/// (`arn:<partition>:acm:<region>:<account>:<type>/<id>`), or `None` when `arn` is not
/// an ACM ARN at all.
///
/// Every ACM resource is addressed by an ARN of that one form, so the framing is parsed
/// in one place and each resource type only names its own `<type>/` prefix.
fn acm_arn_resource(arn: &str) -> Option<&str> {
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
    Some(resource)
}

/// The id in an ACM ARN whose resource type is `resource_type`, or `None` when `arn`
/// names something else.
fn acm_resource_id<'arn>(arn: &'arn str, resource_type: &str) -> Option<&'arn str> {
    let id = acm_arn_resource(arn)?.strip_prefix(resource_type)?;
    let id = id.strip_prefix('/')?;
    (!id.is_empty() && !id.contains('/')).then_some(id)
}

/// The certificate id in an ACM certificate ARN, or `None` when `arn` names
/// something that is not one (the caller answers `InvalidArnException`).
///
/// The model's `@pattern` already refused anything that is not an `acm` ARN before a
/// handler runs; what is left to check here is that the resource is a `certificate`.
pub fn certificate_id_from_arn(arn: &str) -> Option<&str> {
    acm_resource_id(arn, "certificate")
}

/// The endpoint id in an ACME endpoint ARN, or `None` when `arn` names something that
/// is not one.
///
/// The counterpart of [`certificate_id_from_arn`] for the other resource type this
/// service persists, and what tells an opaque `ListAcmeEndpoints` pagination token this
/// service issued apart from one it did not.
pub fn acme_endpoint_id_from_arn(arn: &str) -> Option<&str> {
    acm_resource_id(arn, "acme-endpoint")
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

/// The `ResourceNotFoundException` message for an ACM resource ARN that names no
/// resource this account holds, built from the ARN the request asked for.
///
/// Rendered by the operations that address an ACM resource by type-agnostic ARN
/// (`TagResource`, `UntagResource`, `ListTagsForResource`), which is why it names a
/// "resource" rather than one type. `ResourceNotFoundException` carries one modelled
/// member (`message: String`) and the model documents no text for it, so the sentence
/// follows the template every other lookup in this service renders
/// ([`certificate_not_found_message`]): one complete sentence naming the requested ARN
/// and terminated by a period.
pub fn resource_not_found_message(resource_arn: &str) -> String {
    format!("Could not find resource {resource_arn}.")
}

/// The `ValidationException` message for a CERTIFICATE ARN handed to one of the
/// generic resource-tagging operations, naming the certificate-specific operation to
/// use instead.
///
/// Those operations are documented for "all ACM resource types except the certificate
/// resource type", each pointing at its certificate counterpart
/// (`TagResource` -> `AddTagsToCertificate`, `UntagResource` ->
/// `RemoveTagsFromCertificate`, `ListTagsForResource` -> `ListTagsForCertificate`), so
/// a certificate ARN is input the operation's own constraints do not admit — which is
/// what `ValidationException` ("the supplied input failed to satisfy constraints")
/// names. Rendered here so all three refusals read the same.
pub fn certificate_resource_message(resource_arn: &str, certificate_operation: &str) -> String {
    format!(
        "{resource_arn} names a certificate, which this operation does not apply to; \
         use {certificate_operation} instead"
    )
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

/// The refusal sentence for a pagination token the service did not issue.
///
/// A `NextToken` is opaque and belongs to the page that issued it, so a token this
/// service cannot read as one of its own names no position to resume from. The
/// sentence comes from ACM's recorded behaviour: one constant sentence for the whole
/// class, naming neither the token nor what was wrong with it — a caller cannot
/// repair an opaque value, it can only restart the walk. Rendered here so every
/// paginated read that answers this class reports the same sentence.
pub const INVALID_NEXT_TOKEN_MESSAGE: &str = "Invalid next token.";

/// The byte that separates the two halves of a parent-scoped pagination cursor
/// ([`encode_page_cursor`]). It is outside every ARN pattern this service mints, and
/// outside a domain name or an `https` URL, so it cannot occur inside either value it
/// joins.
pub const PAGE_CURSOR_SEPARATOR: char = '\u{1f}';

/// The opaque `NextToken` for the position a page of one parent resource's
/// sub-resources stopped at.
///
/// It carries the PARENT the page was cut from as well as the position in it, so a
/// token issued for one parent cannot resume another's list, where the same position
/// would name a different place. Hex-encoded, so the token is opaque to the caller and
/// satisfies the `^[\u0009\u000A\u000D\u0020-\u00FF]*$` pattern a `NextToken` member
/// carries whatever the two values contain.
///
/// Rendered here so every paginated read that walks a parent's sub-resources in one
/// key's order issues ONE token form, with [`decode_page_cursor`] as its only reader.
/// (The certificate listings page in an order built from several keys at once, so they
/// frame their own cursors.)
pub fn encode_page_cursor(parent: &str, position: &str) -> String {
    hex::encode(format!("{parent}{PAGE_CURSOR_SEPARATOR}{position}"))
}

/// The position inside a `NextToken` [`encode_page_cursor`] issued for `parent`, or
/// `None` when the token did not come from it for that parent — which every caller
/// refuses with [`INVALID_NEXT_TOKEN_MESSAGE`].
///
/// Every step is fallible on purpose: the token is caller-supplied, so no value read
/// out of it may panic a handler or silently resume somebody else's list.
pub fn decode_page_cursor(token: &str, parent: &str) -> Option<String> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    let (named_parent, position) = decoded.split_once(PAGE_CURSOR_SEPARATOR)?;
    (named_parent == parent && !position.is_empty()).then(|| position.to_string())
}

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

/// The mailbox local-parts ACM sends a domain-validation email to: "ACM sends email
/// to the the following five addresses: admin@<suffix> administrator@<suffix>
/// hostmaster@<suffix> postmaster@<suffix> webmaster@<suffix>"
/// (`ResendValidationEmailRequest$ValidationDomain` doc).
///
/// A certificate's `ValidationEmails` lists the addresses ACM HAS written to, so these
/// are rendered onto a record when mail goes out — the initial dispatch the clock owes
/// a request ([`Certificate::dispatch_validation_email`]) or a
/// `ResendValidationEmail` — never by the describe that reports it.
pub const VALIDATION_EMAIL_LOCAL_PARTS: &[&str] = &[
    "admin",
    "administrator",
    "hostmaster",
    "postmaster",
    "webmaster",
];

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
        // The domain whose control is being proven, whatever the method proves it
        // with: the one the request named for this name, else the name itself. It is
        // also the SUFFIX of the mailboxes ACM writes to, and a mail suffix carries no
        // wildcard label — a certificate that validates `*.example.com` through itself
        // is mailed at `example.com`, the domain ACM proves control of.
        validation_domain: Some(
            validation_domain
                .map(validation_target)
                .unwrap_or(target)
                .to_string(),
        ),
        requested_validation_domain: validation_domain.map(str::to_string),
        // Minting a validation sends no mail: the mailbox list names the messages ACM
        // HAS sent, which is what [`Certificate::dispatch_validation_email`] and
        // `ResendValidationEmail` record.
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
        // EMAIL, and the default when no method was named: the validation domain ACM
        // mails is already on the record, and the mailboxes it wrote to are recorded
        // by the call that writes them.
        _ => {}
    }
    record
}

/// The DNS record the caller publishes to prove ownership of `domain_name` for one
/// ACME domain validation: "the DNS resource record to create for domain validation"
/// (`DnsPrevalidationDetails$ResourceRecord` doc).
///
/// SERVER-MANAGED, like the CNAME a certificate's DNS validation publishes
/// ([`domain_validation_record`]) and in the same form: a `_<label>` prefixed name
/// under the domain being validated, pointing at a `_<label>` name in
/// [`ACM_VALIDATIONS_ZONE`]. Both labels are derived from the validation's OWN id
/// through [`validation_label`], so repeated reads of one validation report the same
/// record, two validations of the same domain differ, and no part of the record comes
/// from anything the caller sent.
pub fn acme_prevalidation_resource_record(
    validation_id: &str,
    domain_name: &str,
) -> ResourceRecord {
    let target = validation_target(domain_name);
    let name = validation_label(validation_id, domain_name, "dns-name");
    let value = validation_label(validation_id, domain_name, "dns-value");
    ResourceRecord {
        name: format!("_{name}.{target}."),
        r#type: RECORD_TYPE_CNAME.to_string(),
        value: format!("_{value}.{ACM_VALIDATIONS_ZONE}."),
    }
}

/// The tag-key prefix Amazon Web Services reserves for its own tags: neither half of
/// a pair a caller names may begin with it (`InvalidTagException`: "you cannot specify
/// a tag value that begins with aws:").
pub const RESERVED_TAG_PREFIX: &str = "aws:";

/// The number of tags one ACM resource may carry, from the `TagList` length bound the
/// model declares (`len 1..50`): it is both the most pairs one request may name and
/// the most a resource holds, since `ListTagsForResource` and
/// `ListTagsForCertificate` report the stored map through that same list.
pub const TAG_LIMIT: usize = 50;

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
    check_tag_key_not_reserved(&tag.key)?;
    if tag
        .value
        .as_deref()
        .is_some_and(|value| value.starts_with(RESERVED_TAG_PREFIX))
    {
        return Err(reserved_tag_message(&tag.key));
    }
    Ok(())
}

/// `Ok(())` for a tag key any caller may name, or the reserved-tag message for one it
/// may not.
///
/// The operation that removes tags BY KEY (`UntagResource`) carries no value to check,
/// so the key half of the rule is reachable on its own; it is the same rule and the
/// same sentence either way.
pub fn check_tag_key_not_reserved(key: &str) -> Result<(), String> {
    if key.starts_with(RESERVED_TAG_PREFIX) {
        return Err(reserved_tag_message(key));
    }
    Ok(())
}

/// The message a refused reserved tag reports, naming the key of the offending pair.
fn reserved_tag_message(key: &str) -> String {
    format!("tag {key} is reserved: neither a tag key nor its value may begin with {RESERVED_TAG_PREFIX}")
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

/// Drop the requested KEYS from a stored tag map, as ACM's key-addressed untagging
/// operation (`UntagResource`, whose `TagKeys` carries keys alone) defines it: a named
/// key is removed whatever value it holds, and a key the map does not hold leaves the
/// map as it was — so removing is idempotent.
pub fn remove_tag_keys(stored: &mut BTreeMap<String, String>, keys: &[String]) {
    for key in keys {
        stored.remove(key);
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

/// A certificate's `CertificateOptions` as a response reports them: every preference
/// the certificate carries, defaulted where the request named none, so a client that
/// set one can read it back and one that set nothing sees what ACM applied.
///
/// `ValidationMethod` is reported from the certificate's own options: it is the
/// preference a request or an `UpdateCertificateOptions` named there, and a
/// certificate whose options never named one reports none — the method each name is
/// being validated by is reported per domain, in `DomainValidation$ValidationMethod`.
pub fn certificate_options(cert: &Certificate) -> CertificateOptions {
    CertificateOptions {
        certificate_transparency_logging_preference: Some(
            cert.options
                .as_ref()
                .and_then(|options| options.certificate_transparency_logging_preference.clone())
                .unwrap_or_else(|| TRANSPARENCY_LOGGING_ENABLED.to_string()),
        ),
        export: Some(export_option(cert)),
        validation_method: cert
            .options
            .as_ref()
            .and_then(|options| options.validation_method.clone()),
    }
}

/// `CertificateDetail$Subject`: the subject distinguished name of a certificate whose
/// common name is `common_name`, in the `CN=<name>` form an X.509 subject is written
/// in.
pub fn subject_distinguished_name(common_name: &str) -> String {
    format!("CN={common_name}")
}

/// `CertificateDetail$Issuer`: the name of the CA that signs the certificate.
///
/// A public certificate ACM issues is signed by Amazon's own CA ([`ISSUER_AMAZON`]).
/// One ACM issues from a private CA is signed by that CA, whose subject name lives in
/// Private CA's own store and not in this service's, so no issuer is reported for it.
pub fn issuer_name(certificate_authority_arn: Option<&str>) -> Option<String> {
    certificate_authority_arn
        .is_none()
        .then(|| ISSUER_AMAZON.to_string())
}

/// `CertificateDetail$SignatureAlgorithm`: "the algorithm that was used to sign the
/// certificate", as the `<hash>WITH<key family>` spelling AWS reports it in, or `None`
/// for a key algorithm of no family this service knows how to sign with — an absent
/// member rather than a wrong one.
///
/// ACM signs with SHA-256, so the stored `KeyAlgorithm`'s family picks the rest:
/// `SHA256WITHRSA` for an RSA key pair and `SHA256WITHECDSA` for an elliptic-curve
/// one. Both spellings are the sibling Private CA model's `SigningAlgorithm` values
/// (`smithy_query(query="shape", shape="com.amazonaws.acmpca#SigningAlgorithm")`).
///
/// The family is the part of the `KeyAlgorithm` BEFORE its separator (`EC` of
/// `EC_prime256v1`), matched in full, so neither a name that merely begins with those
/// letters nor a family the model adds later is reported as one of these two.
pub fn signature_algorithm(key_algorithm: &str) -> Option<String> {
    // The family is the part before the first separator (`EC` of `EC_prime256v1`),
    // and a name with no separator is its own family.
    let family = match key_algorithm
        .split_once('_')
        .map(|(family, _)| family)
        .unwrap_or(key_algorithm)
    {
        "RSA" => "RSA",
        "EC" => "ECDSA",
        _ => return None,
    };
    Some(format!("SHA256WITH{family}"))
}

/// `KeyAlgorithm` as a certificate's detail reports it: the X.509 algorithm
/// descriptor, whose family and parameter are joined by a HYPHEN (`RSA-2048`,
/// `EC-prime256v1`), rather than the model enum's underscored name (`RSA_2048`) that
/// a request and the `Includes.keyTypes` filter are written in.
pub fn detail_key_algorithm(key_algorithm: &str) -> String {
    key_algorithm.replacen('_', "-", 1)
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
/// Every member the certificate's own state decides is reported: the X.509 identity
/// ACM fixed when it accepted the request (`Subject`, `Issuer`,
/// `SignatureAlgorithm`), the key-usage extension lists (empty, as lists, while the
/// certificate is still pending) and the full `Options`. Members that come into
/// existence only with the certificate itself (`Serial`, `IssuedAt`, `NotBefore` /
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
        key_algorithm: Some(detail_key_algorithm(&cert.key_algorithm)),
        subject: cert.subject.clone(),
        issuer: cert.issuer.clone(),
        signature_algorithm: cert.signature_algorithm.clone(),
        in_use_by: Some(cert.in_use_by.clone()),
        // The X.509 key-usage extensions the certificate carries. A pending
        // certificate has none yet, which is the EMPTY list, not a missing member.
        key_usages: Some(
            cert.key_usages
                .iter()
                .map(|name| KeyUsage {
                    name: Some(name.clone()),
                })
                .collect(),
        ),
        extended_key_usages: Some(
            cert.extended_key_usages
                .iter()
                .map(|name| ExtendedKeyUsage {
                    name: Some(name.clone()),
                    // The extension's object identifier, which only a parsed
                    // certificate carries.
                    oid: None,
                })
                .collect(),
        ),
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
        options: Some(certificate_options(cert)),
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
        // The summary reports the algorithm in the model enum's spelling — the one a
        // request's `Includes.keyTypes` is written in and this very list is filtered
        // on. Only the certificate's DETAIL carries the X.509 descriptor spelling
        // (see [`detail_key_algorithm`]).
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

/// A stored ACME endpoint as `ListAcmeEndpoints`' `AcmeEndpointSummary`.
///
/// `AcmeEndpointSummary` declares exactly the members `AcmeEndpoint` does, so one
/// endpoint is reported identically whether it is read one at a time or listed: the
/// summary is projected from the same record, member for member, through
/// [`acme_endpoint_detail`] — including the two members that are present only when the
/// record carries them (`FailureReason`, `CertificateTags`).
pub fn acme_endpoint_summary(endpoint: &AcmeEndpointRecord) -> AcmeEndpointSummary {
    let detail = acme_endpoint_detail(endpoint);
    AcmeEndpointSummary {
        acme_endpoint_arn: detail.acme_endpoint_arn,
        endpoint_url: detail.endpoint_url,
        status: detail.status,
        failure_reason: detail.failure_reason,
        authorization_behavior: detail.authorization_behavior,
        contact: detail.contact,
        certificate_authority: detail.certificate_authority,
        certificate_tags: detail.certificate_tags,
        created_at: detail.created_at,
        updated_at: detail.updated_at,
    }
}

/// A stored ACME domain validation as `DescribeAcmeDomainValidation`'s
/// `AcmeDomainValidation`.
///
/// Every member is read off the record: `PrevalidationDetails` reports the scope and
/// hosted zone the create configured BESIDE the resource record the service minted
/// ([`acme_prevalidation_resource_record`]), which is the one member of the three the
/// caller cannot have sent. `FailureDetails` exists only for a validation that failed
/// ("details about the failure, if the validation failed",
/// `AcmeDomainValidation$FailureDetails` doc), which is what the record stores.
pub fn acme_domain_validation_detail(
    validation: &AcmeDomainValidationRecord,
) -> AcmeDomainValidation {
    AcmeDomainValidation {
        acme_domain_validation_arn: Some(validation.arn.clone()),
        acme_endpoint_arn: Some(validation.acme_endpoint_arn.clone()),
        domain_name: Some(validation.domain_name.clone()),
        prevalidation_type: Some(validation.prevalidation_type.clone()),
        prevalidation_details: Some(PrevalidationDetails::DnsPrevalidation(
            DnsPrevalidationDetails {
                domain_scope: validation.domain_scope.clone(),
                hosted_zone_id: validation.hosted_zone_id.clone(),
                resource_record: validation.resource_record.clone(),
            },
        )),
        status: Some(validation.status.clone()),
        failure_details: validation.failure_details.clone(),
        created_at: Some(to_date_time(validation.created_at)),
        updated_at: Some(to_date_time(validation.updated_at)),
    }
}

/// A stored ACME domain validation as `ListAcmeDomainValidations`'
/// `AcmeDomainValidationSummary`.
///
/// `AcmeDomainValidationSummary` declares exactly the members `AcmeDomainValidation`
/// does, so one validation is reported identically whether it is read one at a time or
/// listed: the summary is projected from the same record, member for member, through
/// [`acme_domain_validation_detail`] — including `FailureDetails`, which is present only
/// for a validation that failed.
pub fn acme_domain_validation_summary(
    validation: &AcmeDomainValidationRecord,
) -> AcmeDomainValidationSummary {
    let detail = acme_domain_validation_detail(validation);
    AcmeDomainValidationSummary {
        acme_domain_validation_arn: detail.acme_domain_validation_arn,
        acme_endpoint_arn: detail.acme_endpoint_arn,
        domain_name: detail.domain_name,
        prevalidation_type: detail.prevalidation_type,
        prevalidation_details: detail.prevalidation_details,
        status: detail.status,
        failure_details: detail.failure_details,
        created_at: detail.created_at,
        updated_at: detail.updated_at,
    }
}

/// A stored ACME account as `ListAcmeAccounts`' `AcmeAccountSummary`.
///
/// Every member is read off the record: the summary and `DescribeAcmeAccount`'s
/// `AcmeAccount` carry the same members, so both project this one record and cannot
/// disagree. `AcmeExternalAccountBindingArn` is present only for an account that
/// registered against a binding, and `Contacts` only for one that registered contact
/// information — "the contact information for the ACME account"
/// (`AcmeAccountSummary$Contacts` doc), which an endpoint whose `Contact` is
/// `NOT_REQUIRED` need not have.
pub fn acme_account_summary(account: &AcmeAccountRecord) -> AcmeAccountSummary {
    AcmeAccountSummary {
        account_url: Some(account.account_url.clone()),
        public_key_thumbprint: Some(account.public_key_thumbprint.clone()),
        status: Some(account.status.clone()),
        created_at: Some(to_date_time(account.created_at)),
        acme_external_account_binding_arn: account.acme_external_account_binding_arn.clone(),
        contacts: (!account.contacts.is_empty()).then(|| account.contacts.clone()),
    }
}

/// A stored certificate as `SearchCertificates`' `CertificateSearchResult`: the same
/// record `certificate_detail` and `certificate_summary` project, split into the
/// X.509 attributes and the ACM metadata that operation's filters select on.
///
/// Every member here is the value the FILTER on that member compares, so a
/// certificate a filter kept reports the value that kept it — a client that
/// re-reads the attribute it filtered on sees what it asked for, and this
/// projection and the filter cannot drift into two vocabularies for one attribute.
///
/// Members that belong to something this service never does stay absent, and are the
/// members the search filters correspondingly keep nothing on:
///
/// * `X509Attributes.Issuer` is the issuing CA's distinguished NAME, and
///   `X509Attributes.SerialNumber` the serial that CA assigns when it signs; this
///   emulator signs no X.509 certificate, so it holds neither. (The detail's own
///   `Issuer` member is a plain CA name, which [`issuer_name`] does report.)
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
            // A search result reports each attribute as the value the FILTER on that
            // attribute selects on, so a certificate a filter kept reports the value
            // that kept it: `X509AttributeFilter.KeyAlgorithm` compares the stored
            // `KeyAlgorithm` in the model enum's spelling (`EC_prime256v1`), which is
            // therefore the spelling reported here. Only the certificate's DETAIL
            // carries the X.509 descriptor spelling (see [`detail_key_algorithm`]),
            // which no filter is written in.
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
