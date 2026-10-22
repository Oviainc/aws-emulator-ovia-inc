// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-artifact-artifact service.
//!
//! AWS Artifact's report catalog is SERVICE-owned: the API has no `CreateReport`, so
//! every report a caller can see was published by AWS itself. This module therefore
//! holds the catalog as the state struct's [`Default`], which `engine::InMemoryEngine`
//! builds the first time any handler reaches the store. Seeding in `Default` — rather
//! than from the first read — is what keeps every report operation a pure reporting
//! operation: no `ListReports`, `GetReportMetadata` or `ListReportVersions` call writes
//! anything, so a read can never change what a later caller sees.
//!
//! The one thing a caller does create is a TERM GRANT: `GetTermForReport` mints the
//! acceptance receipt (`termToken`) that `GetReport` requires, and that token is
//! persisted here so `GetReport` can check it against the report and version it was
//! issued for.
//!
//! The CUSTOMER-AGREEMENT set is held the same way, and for the same reason: the API has
//! no operation that creates, accepts or terminates one — an account's agreements are
//! established outside this API — so the set a caller can see is persisted state built by
//! this struct's [`Default`], and `ListCustomerAgreements` only reads it. Unlike a report,
//! a customer-agreement belongs to the calling identity's ACCOUNT, which is what its ARN
//! and its `awsAccountId` name.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    ConflictException, CustomerAgreementSummary, ReportDetail, ReportSummary,
    ResourceNotFoundException, ValidationException,
};

/// The region a report ARN names when the request carries no SigV4 credential scope to
/// read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `resourceType` the modelled `ResourceNotFoundException` / `ConflictException`
/// report for a report id or version that names nothing.
pub const REPORT_RESOURCE_TYPE: &str = "Report";

/// The `resourceType` reported for a term grant that does not cover the request.
pub const TERM_RESOURCE_TYPE: &str = "Term";

/// The largest page every paginated operation of this service serves, and the page size
/// used when a request names no `maxResults`: every one of them takes its `maxResults`
/// from `MaxResultsAttribute`, modelled `@range(min: 1, max: 300)`.
pub const REPORT_PAGE_LIMIT: i32 = 300;

/// `CustomerAgreementState`'s value for an agreement that is in force. The one state
/// `ListCustomerAgreements` reports: it "List[s] active customer-agreements applicable to
/// calling identity".
pub const AGREEMENT_STATE_ACTIVE: &str = "ACTIVE";

/// `CustomerAgreementState`'s value for an agreement the customer terminated.
pub const AGREEMENT_STATE_CUSTOMER_TERMINATED: &str = "CUSTOMER_TERMINATED";

/// `CustomerAgreementState`'s value for an agreement AWS terminated.
pub const AGREEMENT_STATE_AWS_TERMINATED: &str = "AWS_TERMINATED";

/// `AgreementType`'s value for an agreement accepted as AWS publishes it.
pub const AGREEMENT_TYPE_DEFAULT: &str = "DEFAULT";

/// `AgreementType`'s value for a published agreement accepted with modifications.
pub const AGREEMENT_TYPE_MODIFIED: &str = "MODIFIED";

/// `AgreementType`'s value for an agreement negotiated for one customer.
pub const AGREEMENT_TYPE_CUSTOM: &str = "CUSTOM";

/// How many term grants the store holds before `GetTermForReport` releases the oldest.
///
/// AWS publishes no bound on outstanding acceptances, so this is the emulator's OWN
/// bound on its state rather than a claim about the service: it is what stops a caller
/// accepting terms in a loop from growing `term_tokens` without end. It is set far above
/// any workflow an emulator serves — a caller holds one grant per report version it
/// downloads — and the seeded catalog publishes far fewer versions than this.
pub const TERM_GRANT_CAPACITY: usize = 1_000;

/// `ValidationExceptionReason`'s value for a pagination or term token the service did
/// not issue.
pub const REASON_INVALID_TOKEN: &str = "invalidToken";

/// The host this emulator publishes report and term documents under. Nothing serves it;
/// it is the stable location a document URL points at, built from the stored object
/// path rather than from the request.
const DOCUMENT_HOST: &str = "artifact-documents";

/// One version of one AWS Artifact report, as this emulator persists it.
///
/// Every member of `ReportSummary` and `ReportDetail` that can differ between two
/// versions of the same report lives here; the members shared by all of a report's
/// versions live on [`Report`].
#[derive(Debug, Clone)]
pub struct ReportVersion {
    /// `VersionAttribute`, `@range(min: 1)`.
    pub version: i64,
    /// One of `PublishedState`'s values.
    pub state: String,
    /// One of `UploadState`'s values.
    pub upload_state: String,
    /// "The message associated with the current upload state."
    pub status_message: Option<String>,
    pub description: String,
    /// "Timestamp indicating the report resource effective start."
    pub period_start: SystemTime,
    /// "Timestamp indicating the report resource effective end."
    pub period_end: SystemTime,
    /// When AWS published this version — `createdAt` and, since a published version is
    /// never edited in place, `lastModifiedAt`.
    pub published_at: SystemTime,
    /// "Sequence number to enforce optimistic locking", `@range(min: 1)`.
    pub sequence_number: i64,
}

/// One AWS Artifact report: the members every version of it shares, plus its versions.
#[derive(Debug, Clone)]
pub struct Report {
    /// `ReportId`, `^report-[a-zA-Z0-9]{16}$`.
    pub id: String,
    pub name: String,
    pub series: String,
    pub category: String,
    pub company_name: String,
    pub product_name: String,
    /// One of `AcceptanceType`'s values: `EXPLICIT` requires click-through acceptance
    /// of the report's term, `PASSTHROUGH` does not.
    pub acceptance_type: String,
    /// The file name of the report document, used to build its URL.
    pub document_object: String,
    /// The file name of the term document, used to build its URL.
    pub term_object: String,
    /// Every published version, keyed by version number. A `BTreeMap` is what gives
    /// `ListReportVersions` the ascending total order its pagination token indexes into,
    /// and what makes "the latest version" the last entry.
    ///
    /// INVARIANT: never empty — a report exists because AWS published a version of it.
    pub versions: BTreeMap<i64, ReportVersion>,
}

impl Report {
    /// The version `requested` names, or the newest published one when it names none.
    ///
    /// TOTAL: `None` means "this report has no such version" — including the case where
    /// the request named none and the report holds none at all — so a caller answers it
    /// with a modelled error and never has to re-derive what was asked for.
    pub fn resolve(&self, requested: Option<i64>) -> Option<&ReportVersion> {
        match requested {
            Some(version) => self.versions.get(&version),
            None => self.versions.values().next_back(),
        }
    }

    /// The `ReportSummary` projection `ListReports` and `ListReportVersions` return for
    /// one version of this report.
    pub fn to_summary(&self, version: &ReportVersion, region: &str) -> ReportSummary {
        ReportSummary {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            state: Some(version.state.clone()),
            arn: Some(report_arn(region, &self.id)),
            version: Some(version.version),
            upload_state: Some(version.upload_state.clone()),
            description: Some(version.description.clone()),
            period_start: Some(::aws_smithy_types::DateTime::from(version.period_start)),
            period_end: Some(::aws_smithy_types::DateTime::from(version.period_end)),
            series: Some(self.series.clone()),
            category: Some(self.category.clone()),
            company_name: Some(self.company_name.clone()),
            product_name: Some(self.product_name.clone()),
            status_message: version.status_message.clone(),
            acceptance_type: Some(self.acceptance_type.clone()),
        }
    }

    /// The `ReportDetail` projection `GetReportMetadata` returns for one version of this
    /// report. Every member it shares with [`Report::to_summary`] takes the same value:
    /// both read the one stored record.
    pub fn to_detail(&self, version: &ReportVersion, region: &str) -> ReportDetail {
        ReportDetail {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            description: Some(version.description.clone()),
            period_start: Some(::aws_smithy_types::DateTime::from(version.period_start)),
            period_end: Some(::aws_smithy_types::DateTime::from(version.period_end)),
            created_at: Some(::aws_smithy_types::DateTime::from(version.published_at)),
            // A published version is never edited in place: AWS publishes a new version
            // instead, so the last modification of THIS version is its publication.
            last_modified_at: Some(::aws_smithy_types::DateTime::from(version.published_at)),
            // "Timestamp indicating when the report resource was deleted" — only a
            // deleted report carries one, and the catalog holds none.
            deleted_at: None,
            state: Some(version.state.clone()),
            arn: Some(report_arn(region, &self.id)),
            series: Some(self.series.clone()),
            category: Some(self.category.clone()),
            company_name: Some(self.company_name.clone()),
            product_name: Some(self.product_name.clone()),
            term_arn: Some(term_arn(region, &self.id, version.version)),
            version: Some(version.version),
            acceptance_type: Some(self.acceptance_type.clone()),
            sequence_number: Some(version.sequence_number),
            upload_state: Some(version.upload_state.clone()),
            status_message: version.status_message.clone(),
        }
    }
}

/// One AWS Artifact customer-agreement resource, as this emulator persists it.
///
/// A customer-agreement is the calling identity's acceptance of an AWS-published
/// agreement; this record holds every member of `CustomerAgreementSummary` that can
/// differ between two of them.
#[derive(Debug, Clone)]
pub struct CustomerAgreement {
    /// `CustomerAgreementIdAttribute`, `^customer-agreement-[a-zA-Z0-9]{16}$`.
    pub id: String,
    /// "Name of the customer-agreement resource."
    pub name: String,
    /// "Description of the resource."
    pub description: String,
    /// The AWS-published agreement resource this customer-agreement represents, which
    /// its `agreementArn` names.
    pub agreement_id: String,
    /// One of `AgreementType`'s values ([`AGREEMENT_TYPE_DEFAULT`],
    /// [`AGREEMENT_TYPE_MODIFIED`], [`AGREEMENT_TYPE_CUSTOM`]).
    pub agreement_type: String,
    /// One of `CustomerAgreementState`'s values ([`AGREEMENT_STATE_ACTIVE`],
    /// [`AGREEMENT_STATE_CUSTOMER_TERMINATED`], [`AGREEMENT_STATE_AWS_TERMINATED`]).
    pub state: String,
    /// "Timestamp indicating when the agreement became effective."
    pub effective_start: SystemTime,
    /// "Timestamp indicating when the agreement was terminated" — so only a terminated
    /// agreement carries one.
    pub effective_end: Option<SystemTime>,
    /// "Terms required to accept the agreement resource." `AgreementTerms`, at most 10.
    pub acceptance_terms: Vec<String>,
    /// "Terms required to terminate the customer-agreement resource." `AgreementTerms`,
    /// at most 10.
    pub terminate_terms: Vec<String>,
}

impl CustomerAgreement {
    /// True when this agreement is in `CustomerAgreementState`'s `ACTIVE` state — the
    /// agreements `ListCustomerAgreements` reports.
    pub fn is_active(&self) -> bool {
        self.state == AGREEMENT_STATE_ACTIVE
    }

    /// The `CustomerAgreementSummary` projection `ListCustomerAgreements` returns.
    pub fn to_summary(&self, region: &str) -> CustomerAgreementSummary {
        CustomerAgreementSummary {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            arn: Some(customer_agreement_arn(region, &self.id)),
            agreement_arn: Some(agreement_arn(&self.agreement_id)),
            aws_account_id: Some(::engine::emulator_account_id().to_string()),
            // "ARN of the organization that owns the resource": this emulator presents a
            // single standalone account, so no organization owns the agreement and
            // naming one would claim an organization that does not exist.
            organization_arn: None,
            effective_start: Some(::aws_smithy_types::DateTime::from(self.effective_start)),
            effective_end: self.effective_end.map(::aws_smithy_types::DateTime::from),
            state: Some(self.state.clone()),
            description: Some(self.description.clone()),
            acceptance_terms: Some(self.acceptance_terms.clone()),
            terminate_terms: Some(self.terminate_terms.clone()),
            r#type: Some(self.agreement_type.clone()),
        }
    }
}

/// A term grant: the acceptance receipt `GetTermForReport` minted, and the report
/// version it covers. `GetReport` serves a document only for a token it finds here
/// whose report and version match the request.
#[derive(Debug, Clone)]
pub struct TermGrant {
    /// The `termToken` handed to the caller. Matches `ShortStringAttribute`'s
    /// `^[a-zA-Z0-9_\-\s]*$` within 1..256 bytes, which is what `GetReport`'s
    /// `termToken` member admits.
    pub token: String,
    pub report_id: String,
    pub report_version: i64,
    pub issued_at: SystemTime,
}

/// Per-service state for com-amazonaws-artifact-artifact. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsArtifactArtifactState {
    /// The service-owned report catalog, keyed by report id. A `BTreeMap` is what gives
    /// `ListReports` the stable total order its pagination token indexes into.
    pub reports: BTreeMap<String, Report>,
    /// Every term grant `GetTermForReport` has minted, keyed by its token.
    ///
    /// A `BTreeMap` rather than a hash map because the tokens are minted from a
    /// monotonic counter in fixed-width hex, so their lexicographic order IS their issue
    /// order: the first entry is always the oldest grant, which is what lets
    /// [`ComAmazonawsArtifactArtifactState::mint_term_grant`] hold the map to
    /// [`TERM_GRANT_CAPACITY`] without scanning it.
    pub term_tokens: BTreeMap<String, TermGrant>,
    /// The counter term tokens are minted from, so two calls never share a token.
    pub term_token_seq: u64,
    /// The calling account's customer-agreements, keyed by id. A `BTreeMap` is what
    /// gives `ListCustomerAgreements` the stable total order its pagination token
    /// indexes into.
    pub customer_agreements: BTreeMap<String, CustomerAgreement>,
}

impl Default for ComAmazonawsArtifactArtifactState {
    fn default() -> Self {
        Self {
            reports: seed_catalog(),
            term_tokens: BTreeMap::new(),
            term_token_seq: 0,
            customer_agreements: seed_customer_agreements(),
        }
    }
}

impl ComAmazonawsArtifactArtifactState {
    /// Mint and persist a term grant for one report version, returning its token.
    ///
    /// The counter advances on every call and the loop skips a token the store already
    /// holds, so no OUTSTANDING grant ever shares a token with another — which is what
    /// makes a grant the receipt for one acceptance. (Short of the counter's wrap, which
    /// 2^64 acceptances away is unreachable, the tokens also rise lexicographically, so
    /// the store's first entry is its oldest grant.)
    ///
    /// The store is held to [`TERM_GRANT_CAPACITY`] by releasing the oldest grants first,
    /// so a caller accepting terms in a loop cannot grow this map without end.
    pub fn mint_term_grant(
        &mut self,
        report_id: &str,
        report_version: i64,
        now: SystemTime,
    ) -> String {
        while self.term_tokens.len() >= TERM_GRANT_CAPACITY {
            if self.term_tokens.pop_first().is_none() {
                break;
            }
        }
        let token = loop {
            self.term_token_seq = self.term_token_seq.wrapping_add(1);
            let candidate = format!("term-{:016x}", self.term_token_seq);
            if !self.term_tokens.contains_key(&candidate) {
                break candidate;
            }
        };
        self.term_tokens.insert(
            token.clone(),
            TermGrant {
                token: token.clone(),
                report_id: report_id.to_string(),
                report_version,
                issued_at: now,
            },
        );
        token
    }
}

/// The ARN of a report resource.
///
/// AWS Artifact reports are service-owned, so the ARN carries the region the request was
/// signed for and an EMPTY account id — the form the model's own `ListReports` example
/// shows (`arn:aws:artifact:<region>::report/<reportId>`).
pub fn report_arn(region: &str, report_id: &str) -> String {
    format!("arn:aws:artifact:{region}::report/{report_id}")
}

/// The ARN of the term resource attached to one version of a report, in the same
/// service-owned form as [`report_arn`] (`arn:aws:artifact:<region>::term/<termId>`).
pub fn term_arn(region: &str, report_id: &str, version: i64) -> String {
    let suffix = report_id.strip_prefix("report-").unwrap_or(report_id);
    format!("arn:aws:artifact:{region}::term/term-{suffix}-v{version}")
}

/// The ARN of a customer-agreement resource.
///
/// A customer-agreement is ACCOUNT-owned — it is one account's acceptance of an
/// agreement, which is what its `awsAccountId` reports — so unlike [`report_arn`] its ARN
/// carries the account id beside the region the request was signed for.
pub fn customer_agreement_arn(region: &str, id: &str) -> String {
    format!(
        "arn:aws:artifact:{region}:{account}:customer-agreement/{id}",
        account = ::engine::emulator_account_id()
    )
}

/// The ARN of the AWS-published agreement resource a customer-agreement represents.
///
/// The agreement itself is published by AWS rather than owned by an account, and it is
/// not regional, so both fields are empty — the same service-owned ARN form [`report_arn`]
/// uses for a report, which carries no account id.
pub fn agreement_arn(agreement_id: &str) -> String {
    format!("arn:aws:artifact:::agreement/{agreement_id}")
}

/// The URL a caller downloads a stored document from.
///
/// `kind` is `report` or `term`, and `object` the stored file name, so the path is the
/// document's own location. The token the grant was issued under is carried as a query
/// parameter: it is the receipt that authorises the download, and it is what makes two
/// acceptances of the same report yield two distinct URLs. NO SigV4 signature is
/// attached — this emulator does not sign documents, and a fabricated signature would
/// claim a computation nothing performed.
fn document_url(
    region: &str,
    kind: &str,
    report_id: &str,
    version: i64,
    object: &str,
    term_token: &str,
) -> String {
    format!(
        "https://{DOCUMENT_HOST}.{region}.amazonaws.com/{kind}/{report_id}/v{version}/{object}\
         ?termToken={term_token}"
    )
}

/// The URL of the report document for one version, under the grant `term_token`.
pub fn report_document_url(
    region: &str,
    report: &Report,
    version: i64,
    term_token: &str,
) -> String {
    document_url(
        region,
        "report",
        &report.id,
        version,
        &report.document_object,
        term_token,
    )
}

/// The URL of the term document for one version, under the grant `term_token`.
pub fn term_document_url(region: &str, report: &Report, version: i64, term_token: &str) -> String {
    document_url(
        region,
        "term",
        &report.id,
        version,
        &report.term_object,
        term_token,
    )
}

/// The region this request was signed for — what a report ARN and a document URL name.
///
/// The scope's region is only honoured when it has an AWS region's SHAPE
/// (`<partition>-<area>[-<area>]-<number>`, e.g. `us-east-1`, `ap-southeast-3`): the value
/// is interpolated into an ARN and a URL host, so an arbitrary header value must not be
/// able to shape either. Anything else falls back to [`DEFAULT_REGION`].
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_shaped(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// True when `value` has an AWS region's shape: lowercase alphabetic segments joined by
/// `-`, ending in a digit group, at least three segments.
fn is_region_shaped(value: &str) -> bool {
    let mut segments = value.split('-').rev();
    let Some(number) = segments.next() else {
        return false;
    };
    if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let words: Vec<&str> = segments.collect();
    words.len() >= 2
        && words
            .iter()
            .all(|word| !word.is_empty() && word.bytes().all(|b| b.is_ascii_lowercase()))
}

/// True when `value` satisfies `ReportId`'s `^report-[a-zA-Z0-9]{16}$`.
///
/// Two callers need it where the boundary validator cannot be the judge: a `nextToken`,
/// which `ListReports` carries a report id in and the model does not constrain, and the
/// front door in [`crate::service`], which decides whether a request names a resource
/// at all BEFORE that validator runs.
pub fn is_report_id(value: &str) -> bool {
    is_id_shaped(value, "report-")
}

/// True when `value` satisfies `CustomerAgreementIdAttribute`'s
/// `^customer-agreement-[a-zA-Z0-9]{16}$`.
///
/// As with [`is_report_id`], this is for a value the model does not constrain: the
/// `nextToken` `ListCustomerAgreements` carries a customer-agreement id in.
pub fn is_customer_agreement_id(value: &str) -> bool {
    is_id_shaped(value, "customer-agreement-")
}

/// True when `value` is `prefix` followed by 16 alphanumerics — the shape every
/// `<resource>-[a-zA-Z0-9]{16}` id attribute in this model declares.
fn is_id_shaped(value: &str, prefix: &str) -> bool {
    let Some(suffix) = value.strip_prefix(prefix) else {
        return false;
    };
    suffix.len() == 16 && suffix.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// True when `value` satisfies `InquiryId`'s `^compliance-inquiry-[a-zA-Z0-9]{16}$` —
/// the id of a compliance inquiry, as every `complianceInquiryId` member targets one.
///
/// Spelled the same way as [`is_report_id`], and for the same caller: the front door
/// judges the target of a request before the boundary validator sees the member.
pub fn is_inquiry_id(value: &str) -> bool {
    let Some(suffix) = value.strip_prefix("compliance-inquiry-") else {
        return false;
    };
    suffix.len() == 16 && suffix.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// The separator a `ListReportVersions` pagination token joins its report id and its
/// resume version with. `:` is a query-safe sub-delimiter, and it cannot appear in a
/// `ReportId`, so the split is unambiguous.
const VERSIONS_TOKEN_SEPARATOR: char = ':';

/// The `ListReportVersions` pagination token that resumes `report_id` at `version`.
///
/// The token carries the report it was cut for, not just an offset, so a token issued for
/// one report cannot be spent as a resume point on another and a number a client made up
/// is not mistaken for a token the service issued.
pub fn report_versions_token(report_id: &str, version: i64) -> String {
    format!("{report_id}{VERSIONS_TOKEN_SEPARATOR}{version}")
}

/// The version a `ListReportVersions` token resumes at, when it is a token THIS service
/// issued for `report_id`. `None` for anything else.
pub fn parse_report_versions_token(token: &str, report_id: &str) -> Option<i64> {
    let (id, version) = token.split_once(VERSIONS_TOKEN_SEPARATOR)?;
    if id != report_id {
        return None;
    }
    version.parse::<i64>().ok().filter(|version| *version >= 1)
}

/// The `ResourceNotFoundException` payload for a report id the catalog does not hold.
pub fn report_not_found(report_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("Report {report_id} not found"),
        resource_id: report_id.to_string(),
        resource_type: REPORT_RESOURCE_TYPE.to_string(),
    }
}

/// The `ResourceNotFoundException` payload for a version the named report never had.
pub fn report_version_not_found(report_id: &str, version: i64) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("Report {report_id} has no version {version}"),
        resource_id: report_id.to_string(),
        resource_type: REPORT_RESOURCE_TYPE.to_string(),
    }
}

/// The modelled `ValidationException` for a token this service never issued, carrying
/// `ValidationExceptionReason`'s `invalidToken`.
pub fn invalid_token(message: impl Into<String>) -> ValidationException {
    ValidationException {
        message: message.into(),
        reason: REASON_INVALID_TOKEN.to_string(),
        field_list: None,
    }
}

/// The modelled `ConflictException` for a term grant that exists but covers a different
/// report version than the one requested.
pub fn grant_does_not_cover(token: &str, grant: &TermGrant) -> ConflictException {
    ConflictException {
        message: format!(
            "the term token was accepted for {} version {}, not for the requested report version",
            grant.report_id, grant.report_version
        ),
        resource_id: token.to_string(),
        resource_type: TERM_RESOURCE_TYPE.to_string(),
    }
}

/// The page size a paginated operation of this service serves for `max_results`.
///
/// `MaxResultsAttribute` is modelled `@range(min: 1, max: 300)` and the boundary
/// validator refused anything outside it before a handler ran, so the only case left is
/// an absent value: serve the largest page the model allows. The `clamp` states that
/// invariant rather than correcting a caller — a value reaching here outside the range is
/// a validator bug, which the debug assertion surfaces instead of silently repairing.
pub fn page_size(max_results: Option<i32>) -> usize {
    let requested = max_results.unwrap_or(REPORT_PAGE_LIMIT);
    debug_assert!(
        (1..=REPORT_PAGE_LIMIT).contains(&requested),
        "maxResults {requested} should have been refused at the boundary"
    );
    // Clamped into 1..=300 first, so the conversion cannot fail; it is still written
    // without a panicking combinator, because nothing a request carries may abort a
    // handler. The floor is the fallback for the same reason.
    usize::try_from(requested.clamp(1, REPORT_PAGE_LIMIT)).unwrap_or(1)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsArtifactArtifactState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsArtifactArtifactState>)
}

// ---------------------------------------------------------------------------
// The seeded, service-owned catalog.
// ---------------------------------------------------------------------------

/// Seconds since the Unix epoch at 2023-01-01T00:00:00Z.
const T2023H1: u64 = 1_672_531_200;
/// Seconds since the Unix epoch at 2023-07-01T00:00:00Z.
const T2023H2: u64 = 1_688_169_600;
/// Seconds since the Unix epoch at 2024-01-01T00:00:00Z.
const T2024H1: u64 = 1_704_067_200;
/// Seconds since the Unix epoch at 2024-07-01T00:00:00Z.
const T2024H2: u64 = 1_719_792_000;
/// Seconds since the Unix epoch at 2025-01-01T00:00:00Z.
const T2025H1: u64 = 1_735_689_600;
/// Seconds since the Unix epoch at 2025-07-01T00:00:00Z.
const T2025H2: u64 = 1_751_328_000;
/// Seconds since the Unix epoch at 2026-01-01T00:00:00Z.
const T2026H1: u64 = 1_767_225_600;

/// The message `UploadState` `COMPLETE` carries — one of `StatusMessage`'s documented
/// upload messages.
const UPLOAD_COMPLETE_MESSAGE: &str = "Document upload is complete.";

/// One seeded report version, before it becomes a [`ReportVersion`].
struct SeedVersion {
    version: i64,
    description: &'static str,
    period_start: u64,
    period_end: u64,
    published_at: u64,
    sequence_number: i64,
}

/// One seeded report, before it becomes a [`Report`].
struct SeedReport {
    id: &'static str,
    name: &'static str,
    series: &'static str,
    category: &'static str,
    acceptance_type: &'static str,
    document_object: &'static str,
    term_object: &'static str,
    versions: &'static [SeedVersion],
}

/// Every report in the catalog publishes under one company and one product.
const COMPANY_NAME: &str = "Amazon Web Services";
/// See [`COMPANY_NAME`].
const PRODUCT_NAME: &str = "AWS";

/// The catalog this emulator serves: five reports of three series, with one, two and
/// three published versions, under both `AcceptanceType` values.
///
/// Every value satisfies the member's modelled constraints — `id` matches `ReportId`'s
/// `^report-[a-zA-Z0-9]{16}$`, and `name`, `series` and `category` match
/// `ShortStringAttribute`'s `^[a-zA-Z0-9_\-\s]*$`.
const SEED: &[SeedReport] = &[
    SeedReport {
        id: "report-cloudsecalliance",
        name: "Cloud Security Alliance CAIQ",
        series: "CSA",
        category: "Certifications and Attestations",
        acceptance_type: "PASSTHROUGH",
        document_object: "CSA-CAIQ.pdf",
        term_object: "CSA-CAIQ-Term.txt",
        versions: &[SeedVersion {
            version: 1,
            description: "Cloud Security Alliance Consensus Assessments Initiative Questionnaire",
            period_start: T2024H1,
            period_end: T2025H1,
            published_at: T2025H1,
            sequence_number: 1,
        }],
    },
    SeedReport {
        id: "report-iso270012022cert",
        name: "ISO 27001 2022 Certification",
        series: "ISO",
        category: "Certifications and Attestations",
        acceptance_type: "PASSTHROUGH",
        document_object: "ISO-27001-2022.pdf",
        term_object: "ISO-27001-2022-Term.txt",
        versions: &[SeedVersion {
            version: 1,
            description: "ISO/IEC 27001:2022 certificate covering AWS infrastructure services",
            period_start: T2024H1,
            period_end: T2026H1,
            published_at: T2024H1,
            sequence_number: 1,
        }],
    },
    SeedReport {
        id: "report-pcidss4aoc000001",
        name: "PCI DSS v4 AOC",
        series: "PCI",
        category: "Certifications and Attestations",
        acceptance_type: "EXPLICIT",
        document_object: "PCI-DSS-v4-AOC.pdf",
        term_object: "PCI-DSS-v4-AOC-Term.txt",
        versions: &[
            SeedVersion {
                version: 1,
                description: "PCI DSS v4 Attestation of Compliance for calendar year 2023",
                period_start: T2023H1,
                period_end: T2024H1,
                published_at: T2024H1,
                sequence_number: 1,
            },
            SeedVersion {
                version: 2,
                description: "PCI DSS v4 Attestation of Compliance for calendar year 2024",
                period_start: T2024H1,
                period_end: T2025H1,
                published_at: T2025H1,
                sequence_number: 4,
            },
        ],
    },
    SeedReport {
        id: "report-soc1type2report1",
        name: "SOC 1 Type 2",
        series: "SOC",
        category: "Certifications and Attestations",
        acceptance_type: "EXPLICIT",
        document_object: "SOC1-Type2.pdf",
        term_object: "SOC1-Type2-Term.txt",
        versions: &[
            SeedVersion {
                version: 1,
                description: "SOC 1 Type 2 report for the first half of 2023",
                period_start: T2023H1,
                period_end: T2023H2,
                published_at: T2023H2,
                sequence_number: 1,
            },
            SeedVersion {
                version: 2,
                description: "SOC 1 Type 2 report for the second half of 2023",
                period_start: T2023H2,
                period_end: T2024H1,
                published_at: T2024H1,
                sequence_number: 3,
            },
            SeedVersion {
                version: 3,
                description: "SOC 1 Type 2 report for the first half of 2024",
                period_start: T2024H1,
                period_end: T2024H2,
                published_at: T2024H2,
                sequence_number: 7,
            },
        ],
    },
    SeedReport {
        id: "report-soc2type2report1",
        name: "SOC 2 Type 2",
        series: "SOC",
        category: "Certifications and Attestations",
        acceptance_type: "EXPLICIT",
        document_object: "SOC2-Type2.pdf",
        term_object: "SOC2-Type2-Term.txt",
        versions: &[
            SeedVersion {
                version: 1,
                description: "SOC 2 Type 2 report for the first half of 2024",
                period_start: T2024H1,
                period_end: T2024H2,
                published_at: T2024H2,
                sequence_number: 2,
            },
            SeedVersion {
                version: 2,
                description: "SOC 2 Type 2 report for the second half of 2024",
                period_start: T2024H2,
                period_end: T2025H1,
                published_at: T2025H2,
                sequence_number: 5,
            },
        ],
    },
];

/// An instant from its seconds since the Unix epoch.
fn at(seconds: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
}

/// Build the catalog from [`SEED`]. Deterministic: the same records, in the same order,
/// every time the state is constructed.
fn seed_catalog() -> BTreeMap<String, Report> {
    SEED.iter()
        .map(|seed| {
            let report = Report {
                id: seed.id.to_string(),
                name: seed.name.to_string(),
                series: seed.series.to_string(),
                category: seed.category.to_string(),
                company_name: COMPANY_NAME.to_string(),
                product_name: PRODUCT_NAME.to_string(),
                acceptance_type: seed.acceptance_type.to_string(),
                document_object: seed.document_object.to_string(),
                term_object: seed.term_object.to_string(),
                versions: seed
                    .versions
                    .iter()
                    .map(|version| {
                        (
                            version.version,
                            ReportVersion {
                                version: version.version,
                                state: "PUBLISHED".to_string(),
                                upload_state: "COMPLETE".to_string(),
                                status_message: Some(UPLOAD_COMPLETE_MESSAGE.to_string()),
                                description: version.description.to_string(),
                                period_start: at(version.period_start),
                                period_end: at(version.period_end),
                                published_at: at(version.published_at),
                                sequence_number: version.sequence_number,
                            },
                        )
                    })
                    .collect(),
            };
            (report.id.clone(), report)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The seeded customer-agreement set.
// ---------------------------------------------------------------------------

/// One seeded customer-agreement, before it becomes a [`CustomerAgreement`].
struct SeedAgreement {
    id: &'static str,
    name: &'static str,
    agreement_id: &'static str,
    description: &'static str,
    agreement_type: &'static str,
    state: &'static str,
    effective_start: u64,
    effective_end: Option<u64>,
    acceptance_terms: &'static [&'static str],
    terminate_terms: &'static [&'static str],
}

/// The customer-agreement set this emulator serves for its one account.
///
/// The API has no operation that creates, accepts or terminates a customer-agreement, so
/// this set is the account's established agreements rather than anything a caller made:
/// five records spanning every `AgreementType` and every `CustomerAgreementState`, so the
/// state rule `ListCustomerAgreements` applies is observable. Every value satisfies its
/// member's modelled constraints — `id` matches `CustomerAgreementIdAttribute`'s
/// `^customer-agreement-[a-zA-Z0-9]{16}$`, and each name, description and term matches
/// `LongStringAttribute`'s `^[^<>]*$` within 1..2048 bytes.
const AGREEMENT_SEED: &[SeedAgreement] = &[
    SeedAgreement {
        id: "customer-agreement-baaagreement0001",
        name: "AWS Business Associate Addendum",
        agreement_id: "AWS-BAA",
        description: "HIPAA Business Associate Addendum to the AWS Customer Agreement",
        agreement_type: AGREEMENT_TYPE_DEFAULT,
        state: AGREEMENT_STATE_ACTIVE,
        effective_start: T2024H1,
        effective_end: None,
        acceptance_terms: &[
            "Acceptance by an authorized representative of the account",
            "Designation of the AWS services used to process protected health information",
        ],
        terminate_terms: &["Written notice of termination by an authorized representative"],
    },
    SeedAgreement {
        id: "customer-agreement-cjisagreement001",
        name: "AWS CJIS Security Addendum",
        agreement_id: "AWS-CJIS",
        description: "Criminal Justice Information Services Security Addendum",
        agreement_type: AGREEMENT_TYPE_DEFAULT,
        state: AGREEMENT_STATE_CUSTOMER_TERMINATED,
        effective_start: T2023H1,
        effective_end: Some(T2024H2),
        acceptance_terms: &["Acceptance by an authorized representative of the account"],
        terminate_terms: &["Written notice of termination by an authorized representative"],
    },
    SeedAgreement {
        id: "customer-agreement-gdprsccagreement",
        name: "AWS GDPR Data Processing Addendum",
        agreement_id: "AWS-GDPR-DPA",
        description: "Data Processing Addendum with the EU standard contractual clauses",
        agreement_type: AGREEMENT_TYPE_MODIFIED,
        state: AGREEMENT_STATE_ACTIVE,
        effective_start: T2024H2,
        effective_end: None,
        acceptance_terms: &[
            "Acceptance by an authorized representative of the account",
            "Identification of the data exporter and the data importer",
            "Selection of the standard contractual clauses module that applies",
        ],
        terminate_terms: &[
            "Written notice of termination by an authorized representative",
            "Completion of the deletion or return of personal data",
        ],
    },
    SeedAgreement {
        id: "customer-agreement-ndaagreement0001",
        name: "AWS Nondisclosure Agreement",
        agreement_id: "AWS-NDA",
        description: "Mutual nondisclosure agreement negotiated for this customer",
        agreement_type: AGREEMENT_TYPE_CUSTOM,
        state: AGREEMENT_STATE_ACTIVE,
        effective_start: T2025H1,
        effective_end: None,
        acceptance_terms: &["Signature by an authorized representative of the account"],
        terminate_terms: &["Expiry of the confidentiality period"],
    },
    SeedAgreement {
        id: "customer-agreement-pcidssagreement1",
        name: "AWS PCI DSS Merchant Addendum",
        agreement_id: "AWS-PCI-DSS",
        description: "Payment Card Industry Data Security Standard merchant addendum",
        agreement_type: AGREEMENT_TYPE_MODIFIED,
        state: AGREEMENT_STATE_AWS_TERMINATED,
        effective_start: T2023H2,
        effective_end: Some(T2025H1),
        acceptance_terms: &["Acceptance by an authorized representative of the account"],
        terminate_terms: &["Written notice of termination by AWS"],
    },
];

/// Build the customer-agreement set from [`AGREEMENT_SEED`]. Deterministic: the same
/// records, in the same order, every time the state is constructed.
fn seed_customer_agreements() -> BTreeMap<String, CustomerAgreement> {
    AGREEMENT_SEED
        .iter()
        .map(|seed| {
            let agreement = CustomerAgreement {
                id: seed.id.to_string(),
                name: seed.name.to_string(),
                description: seed.description.to_string(),
                agreement_id: seed.agreement_id.to_string(),
                agreement_type: seed.agreement_type.to_string(),
                state: seed.state.to_string(),
                effective_start: at(seed.effective_start),
                effective_end: seed.effective_end.map(at),
                acceptance_terms: seed
                    .acceptance_terms
                    .iter()
                    .map(|term| (*term).to_string())
                    .collect(),
                terminate_terms: seed
                    .terminate_terms
                    .iter()
                    .map(|term| (*term).to_string())
                    .collect(),
            };
            (agreement.id.clone(), agreement)
        })
        .collect()
}
