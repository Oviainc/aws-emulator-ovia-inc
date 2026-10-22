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
//! COMPLIANCE INQUIRIES are the one thing a caller CREATES through this API:
//! `CreateComplianceInquiry` mints one, so the records live in the state struct rather
//! than in its [`Default`]. An inquiry carries its own clock-owed transition — AWS
//! answers the create with HTTP 202 and a non-terminal `PROCESSING` status — which
//! [`ComplianceInquiry::materialise`] applies from the stored due instant, so a reader
//! only ever materialises a change the clock had already made due.
//!
//! An inquiry EXPORT is the second thing a caller creates: `ExportComplianceInquiry`
//! produces a document from one inquiry, so each call mints an [`InquiryExport`] whose id
//! the presigned URL it answers with carries.
//!
//! The CUSTOMER-AGREEMENT set is held the same way as the catalog, and for the same
//! reason: the API has no operation that creates, accepts or terminates one — an
//! account's agreements are
//! established outside this API — so the set a caller can see is persisted state built by
//! this struct's [`Default`], and `ListCustomerAgreements` only reads it. Unlike a report,
//! a customer-agreement belongs to the calling identity's ACCOUNT, which is what its ARN
//! and its `awsAccountId` name.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use smithy_server_runtime::Violation;

use crate::ops::types::{
    ConflictException, CustomerAgreementSummary, InquiryDetail, InquirySummary, QuerySummary,
    ReportDetail, ReportSummary, ResourceNotFoundException, ValidationException,
    ValidationExceptionField,
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

/// The `resourceType` the modelled `ConflictException` reports for a compliance inquiry
/// id.
pub const INQUIRY_RESOURCE_TYPE: &str = "ComplianceInquiry";

/// How many compliance inquiries the store holds before `CreateComplianceInquiry`
/// refuses another.
///
/// AWS publishes no quota on compliance inquiries per account, so this is the emulator's
/// OWN bound on its state rather than a claim about the service: it is what stops a
/// caller submitting inquiries in a loop from growing `inquiries` without end. Nothing
/// is evicted to make room — an inquiry is a resource the caller created and a later
/// `GetComplianceInquiryMetadata` must still find it — so the create is refused instead.
pub const INQUIRY_CAPACITY: usize = 1_000;

/// How many `CreateComplianceInquiry` `clientToken` records are remembered for replay
/// before the oldest is forgotten. The same kind of emulator-owned bound as
/// [`INQUIRY_CAPACITY`]: AWS documents no lifetime for this token, so the records are
/// bounded by count rather than expired on a span this emulator would be inventing.
pub const INQUIRY_IDEMPOTENCY_CAPACITY: usize = 1_000;

/// How many compliance-inquiry EXPORTS the store holds before the oldest is released.
///
/// `ExportComplianceInquiry` produces a new document on every call, so the records would
/// otherwise grow without end. The same kind of emulator-owned bound as
/// [`TERM_GRANT_CAPACITY`]: AWS publishes no quota on exports, and the oldest export is
/// the one whose URL a caller is least likely to still be following, so it is released
/// first rather than refusing the call.
pub const INQUIRY_EXPORT_CAPACITY: usize = 1_000;

/// The file name an exported compliance-inquiry report carries in its document URL.
///
/// A JSON document, and named one: AWS publishes the export as an opaque S3 object whose
/// format the API does not state, and this emulator renders the one it CAN render from
/// what it holds — the inquiry, its queries and their statuses. Nothing here claims a
/// typeset report it did not produce.
pub const INQUIRY_EXPORT_OBJECT: &str = "compliance-inquiry-report.json";

/// The media type [`INQUIRY_EXPORT_OBJECT`] is served with.
pub const INQUIRY_EXPORT_CONTENT_TYPE: &str = "application/json";

/// `ValidationExceptionReason`'s value for a member whose value the service itself
/// refused — what `fieldList` names the member in.
pub const REASON_FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// How long after the create call an inquiry's processing is due to be over.
///
/// AWS answers `CreateComplianceInquiry` with HTTP 202 and a `PROCESSING` inquiry, so
/// the terminal status is owed by the CLOCK rather than by any later call. AWS publishes
/// no duration for it; this is the emulator's own span, chosen so a caller that polls
/// straight after the create still observes the non-terminal status the create returned.
pub const INQUIRY_PROCESSING_SPAN: Duration = Duration::from_millis(500);

/// `InquiryStatus`'s non-terminal value, which a created inquiry starts in.
pub const INQUIRY_STATUS_PROCESSING: &str = "PROCESSING";

/// `InquiryStatus`'s value for an inquiry whose processing is over.
pub const INQUIRY_STATUS_COMPLETED: &str = "COMPLETED";

/// `InquiryStatus`'s value for an inquiry whose processing could not be performed.
pub const INQUIRY_STATUS_FAILED: &str = "FAILED";

/// `InquiryStatusMessage`'s value for an inquiry that is still being processed.
pub const INQUIRY_MESSAGE_IN_PROGRESS: &str = "Compliance inquiry processing is in-progress.";

/// `InquiryStatusMessage`'s value for an inquiry that is over and every query of which
/// reached its terminal state without erroring.
pub const INQUIRY_MESSAGE_COMPLETE: &str = "Compliance inquiry processing is complete.";

/// `InquiryStatusMessage`'s value for an inquiry that is over with at least one query
/// that failed.
pub const INQUIRY_MESSAGE_COMPLETE_WITH_QUERY_ERRORS: &str =
    "Compliance inquiry processing is complete. One or more queries encountered errors \
     during processing.";

/// `InquiryStatusMessage`'s value for an inquiry whose processing itself failed.
pub const INQUIRY_MESSAGE_INTERNAL_ERROR: &str =
    "An internal error occurred while processing the inquiry. Try again at a later time.";

/// `QueryStatus`'s non-terminal value.
pub const QUERY_STATUS_PROCESSING: &str = "PROCESSING";

/// `QueryStatus`'s value for a query whose processing is over.
pub const QUERY_STATUS_COMPLETED: &str = "COMPLETED";

/// `QueryStatus`'s value for a query whose processing could not be performed.
pub const QUERY_STATUS_FAILED: &str = "FAILED";

/// `QueryStatusMessage`'s value for a query that is still being processed.
pub const QUERY_MESSAGE_IN_PROGRESS: &str = "Query processing is in-progress.";

/// `QueryStatusMessage`'s value for a query whose processing is over.
pub const QUERY_MESSAGE_COMPLETE: &str = "Query processing is complete.";

/// `QueryStatusMessage`'s value for a query whose processing could not be performed.
pub const QUERY_MESSAGE_INTERNAL_ERROR: &str =
    "An internal error occurred while processing the query. Try again at a later time.";

/// `InputSource`'s value for an inquiry submitted as a single query.
pub const INPUT_SOURCE_TEXT: &str = "TEXT";

/// `InputSource`'s value for an inquiry submitted as an uploaded document.
pub const INPUT_SOURCE_FILE: &str = "FILE";

/// `InquirySupportMode`'s default: "Defaults to AI_ONLY if not specified."
/// (`CreateComplianceInquiryRequest$supportMode`).
pub const SUPPORT_MODE_AI_ONLY: &str = "AI_ONLY";

/// `ReviewType`'s value for a response reviewed by the service's own model.
pub const REVIEW_TYPE_AI: &str = "AI";

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

/// What a caller submitted as an inquiry's content, as `InquiryContent`'s two union
/// members carry it.
///
/// The content is kept because it IS the created resource: the inquiry's queries are
/// enumerated from it, and `ExportComplianceInquiry` reports on it. It is `@sensitive`
/// in the model, so it is never written to a log or an error message.
#[derive(Debug, Clone)]
pub enum InquiryContentRecord {
    /// `InquiryContent$query` — one query, submitted as text.
    Text(String),
    /// `InquiryContent$fileContent` — an uploaded document, plus "the list of file
    /// sections/sheets to process" (`InquiryFileContent$fileSections`).
    File {
        sections: Vec<String>,
        content: Vec<u8>,
    },
}

impl InquiryContentRecord {
    /// The `InputSource` this content was submitted through.
    pub fn input_source(&self) -> &'static str {
        match self {
            Self::Text(_) => INPUT_SOURCE_TEXT,
            Self::File { .. } => INPUT_SOURCE_FILE,
        }
    }
}

/// One query within one compliance inquiry — the `QuerySummary` projection
/// `ListComplianceInquiryQueries` reports, and what `PutComplianceInquiryFeedback`'s
/// `queryIdentifier` names.
#[derive(Debug, Clone)]
pub struct InquiryQuery {
    /// "Sequential identifier of the query within the inquiry", 1-based.
    pub identifier: i32,
    /// "The actual query text", as the inquiry's content carried it.
    pub query: String,
    /// "Generated response to the query."
    ///
    /// `None` until something generates one, and NOTHING in this emulator does: a
    /// compliance answer is model-generated text and no such engine runs in-process, so
    /// this member stays absent rather than carrying prose nothing produced. Its absence
    /// is the honest signal; the query's own status still reports that its processing is
    /// over, because the query itself was received and recorded.
    pub response: Option<String>,
    /// One of `ReviewType`'s values — the review the inquiry was submitted for.
    pub review_type: Option<String>,
    /// One of `QueryStatus`'s values.
    pub status: String,
    /// One of `QueryStatusMessage`'s values.
    pub status_message: String,
    pub created_at: SystemTime,
}

impl InquiryQuery {
    /// The `QuerySummary` projection `ListComplianceInquiryQueries` reports.
    ///
    /// `citations` and `updatedResponseVersions` are absent rather than empty: a citation
    /// supports a GENERATED response and a response version records an EDIT to one, and
    /// this query has no response at all (see [`InquiryQuery::response`]), so there is
    /// nothing to cite and no version history to report.
    pub fn to_summary(&self) -> QuerySummary {
        QuerySummary {
            query_identifier: self.identifier,
            query: self.query.clone(),
            response: self.response.clone(),
            review_type: self.review_type.clone(),
            citations: None,
            status: self.status.clone(),
            status_message: self.status_message.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            updated_response_versions: None,
        }
    }
}

/// One compliance inquiry a caller created, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct ComplianceInquiry {
    /// `InquiryId`, `^compliance-inquiry-[a-zA-Z0-9]{16}$`.
    pub id: String,
    pub arn: String,
    /// "Title of the inquiry" (`CreateComplianceInquiryRequest$name`).
    pub name: String,
    /// One of `InquiryStatus`'s values.
    pub status: String,
    /// One of `InquiryStatusMessage`'s values.
    pub status_message: String,
    /// One of `InquirySupportMode`'s values, defaulted to [`SUPPORT_MODE_AI_ONLY`] when
    /// the request named none.
    pub support_mode: String,
    pub content: InquiryContentRecord,
    /// The queries this inquiry owns, by ascending `queryIdentifier`.
    pub queries: Vec<InquiryQuery>,
    pub created_at: SystemTime,
    /// "Timestamp indicating when the resource was last modified."
    pub updated_at: SystemTime,
    /// When this inquiry's processing is due to be over, while it still is.
    ///
    /// `None` once the inquiry is in a terminal status. Stored rather than slept on:
    /// [`materialise`](Self::materialise) applies the transition the clock already owed.
    pub processing_due_at: Option<SystemTime>,
    pub tags: BTreeMap<String, String>,
}

impl ComplianceInquiry {
    /// Apply the transition the CLOCK owes this inquiry, if `now` has reached its due
    /// instant. A no-op otherwise, and a no-op for an inquiry already terminal, so
    /// calling it on every read is idempotent.
    ///
    /// The terminal status is DERIVED from the queries': an inquiry is over when its
    /// queries are, and the inquiry-level message says whether any of them errored.
    ///
    /// * a TEXT inquiry's one query is enumerated — its text IS the submitted query, so
    ///   nothing had to be read or generated to know it — and its processing is over at
    ///   the due instant, so it reaches `COMPLETED` and the inquiry reaches the modelled
    ///   clean-completion message. NO RESPONSE is reported for it: a compliance answer is
    ///   model-generated text and no such engine runs in-process, so the `response` member
    ///   stays absent (see [`InquiryQuery::response`]) rather than carrying prose nothing
    ///   produced. The absent member is the honest signal, not a query-level error that
    ///   never happened;
    /// * a FILE inquiry's queries would have to be read out of the uploaded document,
    ///   which this emulator does not parse, so its processing never got as far as a
    ///   query and the inquiry ends in the modelled FAILED state instead;
    /// * a query that did reach `FAILED` is what selects the inquiry-level
    ///   "One or more queries encountered errors" message, so the two enum values are
    ///   told apart by the queries rather than one of them being unreachable.
    pub fn materialise(&mut self, now: SystemTime) {
        let Some(due) = self.processing_due_at else {
            return;
        };
        if now < due {
            return;
        }
        self.processing_due_at = None;
        // The transition was owed AT `due`, so that is when the resource changed —
        // not the instant some caller happened to look.
        self.updated_at = due;
        for query in &mut self.queries {
            if query.status == QUERY_STATUS_PROCESSING {
                query.status = QUERY_STATUS_COMPLETED.to_string();
                query.status_message = QUERY_MESSAGE_COMPLETE.to_string();
            }
        }
        let any_query_failed = self
            .queries
            .iter()
            .any(|query| query.status == QUERY_STATUS_FAILED);
        if self.queries.is_empty() {
            self.status = INQUIRY_STATUS_FAILED.to_string();
            self.status_message = INQUIRY_MESSAGE_INTERNAL_ERROR.to_string();
        } else if any_query_failed {
            self.status = INQUIRY_STATUS_COMPLETED.to_string();
            self.status_message = INQUIRY_MESSAGE_COMPLETE_WITH_QUERY_ERRORS.to_string();
        } else {
            self.status = INQUIRY_STATUS_COMPLETED.to_string();
            self.status_message = INQUIRY_MESSAGE_COMPLETE.to_string();
        }
    }

    /// The `InquirySummary` projection `CreateComplianceInquiry` and
    /// `ListComplianceInquiries` return.
    pub fn to_summary(&self) -> InquirySummary {
        InquirySummary {
            arn: self.arn.clone(),
            name: self.name.clone(),
            id: self.id.clone(),
            status: self.status.clone(),
            status_message: self.status_message.clone(),
            input_source: self.content.input_source().to_string(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
        }
    }

    /// The `InquiryDetail` projection `GetComplianceInquiryMetadata` returns. Every
    /// member it shares with [`to_summary`](Self::to_summary) takes the same value: both
    /// read the one stored record.
    pub fn to_detail(&self) -> InquiryDetail {
        InquiryDetail {
            arn: self.arn.clone(),
            name: self.name.clone(),
            id: self.id.clone(),
            status: self.status.clone(),
            status_message: self.status_message.clone(),
            input_source: self.content.input_source().to_string(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
            support_mode: Some(self.support_mode.clone()),
        }
    }

    /// The inquiry's tags, in the `TagsMap` shape every operation returns them in.
    pub fn tags_map(&self) -> ::std::collections::HashMap<String, String> {
        self.tags
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }

    /// The DOCUMENT `ExportComplianceInquiry` exports: this inquiry, and the queries
    /// `covered` names, as the bytes a caller downloads from the URL that export
    /// published.
    ///
    /// Every value in it is read off this stored record — the inquiry's metadata and its
    /// queries with their statuses — so the document reports what the emulator holds and
    /// nothing else. In particular a query carries the `response` it has, which is null
    /// while nothing has generated one, rather than text no model wrote.
    pub fn export_document(&self, covered: &[i32], include_citations: bool) -> Vec<u8> {
        let queries: Vec<::serde_json::Value> = self
            .queries
            .iter()
            .filter(|query| covered.contains(&query.identifier))
            .map(|query| {
                let mut entry = ::serde_json::json!({
                    "queryIdentifier": query.identifier,
                    "query": query.query,
                    "response": query.response,
                    "reviewType": query.review_type,
                    "status": query.status,
                    "statusMessage": query.status_message,
                    "createdAt": rendered_timestamp(query.created_at),
                });
                if include_citations {
                    // "When true, include citations in the exported document." A citation
                    // cites the source a generated response drew on, so a query with no
                    // response has none: the member is present and EMPTY rather than
                    // filled with references nothing read.
                    entry["citations"] = ::serde_json::json!([]);
                }
                entry
            })
            .collect();
        let document = ::serde_json::json!({
            "complianceInquiryId": self.id,
            "arn": self.arn,
            "name": self.name,
            "status": self.status,
            "statusMessage": self.status_message,
            "supportMode": self.support_mode,
            "inputSource": self.content.input_source(),
            "createdAt": rendered_timestamp(self.created_at),
            "updatedAt": rendered_timestamp(self.updated_at),
            "includeCitations": include_citations,
            "queryIdentifiers": covered,
            "queries": queries,
            "tags": self.tags,
        });
        // Pretty-printed: the document is what a caller downloads and reads, not a wire
        // payload. A value that will not render leaves the document empty rather than
        // half-written, which the download then reports as the server error it is.
        ::serde_json::to_vec_pretty(&document).unwrap_or_default()
    }
}

/// One timestamp as the service renders every `TimestampAttribute` on the wire — RFC 3339
/// date-time, which is the `@timestampFormat` the model declares for them.
fn rendered_timestamp(value: SystemTime) -> String {
    ::aws_smithy_types::DateTime::from(value)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

/// What one remembered `CreateComplianceInquiry` `clientToken` replays: the inquiry the
/// first call with that token created, and the fingerprint of the request that created
/// it, so the same token on a DIFFERENT request is a conflict rather than a replay.
#[derive(Debug, Clone)]
pub struct InquiryCreateRecord {
    pub inquiry_id: String,
    pub fingerprint: String,
}

/// One export `ExportComplianceInquiry` produced: the document it minted for an inquiry,
/// and the options it was exported under.
///
/// The record is what makes the presigned URL a locator for something this emulator
/// persists rather than a string assembled per request: the URL carries the export's id,
/// and this is the export that id names.
#[derive(Debug, Clone)]
pub struct InquiryExport {
    /// The minted export id, which the document URL carries.
    pub id: String,
    /// The inquiry this document was exported from.
    pub inquiry_id: String,
    /// The `queryIdentifiers` the export covers, ascending and without repeats: the ones
    /// the request named, or every query the inquiry owns when it named none.
    pub query_identifiers: Vec<i32>,
    /// `ExportComplianceInquiryRequest$includeCitations`, defaulted to false when the
    /// request named no value.
    pub include_citations: bool,
    pub exported_at: SystemTime,
    /// The exported document itself, as [`ComplianceInquiry::export_document`] rendered
    /// it when this export was taken.
    ///
    /// Held rather than re-rendered on download, because an export is a SNAPSHOT: the
    /// document a caller follows the URL to is the one the export produced, whatever has
    /// become of the inquiry since.
    pub document: Vec<u8>,
}

/// What a request to a published export-document URL names: the inquiry the document was
/// exported from, the export that produced it, and the object being fetched.
///
/// The path is the one [`inquiry_export_url`] builds:
/// `/compliance-inquiry/<inquiryId>/export/<exportId>/<object>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportDocumentTarget {
    pub inquiry_id: String,
    pub export_id: String,
    pub object: String,
}

/// The export document a request path names, or `None` when the path is not one this
/// service publishes.
pub fn parse_export_document_path(path: &str) -> Option<ExportDocumentTarget> {
    let rest = path.strip_prefix("/compliance-inquiry/")?;
    let mut segments = rest.split('/');
    let (Some(inquiry_id), Some("export"), Some(export_id), Some(object), None) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return None;
    };
    if inquiry_id.is_empty() || export_id.is_empty() || object.is_empty() {
        return None;
    }
    Some(ExportDocumentTarget {
        inquiry_id: inquiry_id.to_string(),
        export_id: export_id.to_string(),
        object: object.to_string(),
    })
}

/// True when `host` is the hostname this service publishes its document URLs under —
/// `<DOCUMENT_HOST>.<region>.amazonaws.com`, as [`inquiry_export_url`] builds it.
///
/// Recognising it is what routes a request to a URL this service HANDED OUT back to this
/// service: the host's first label names no service, so the framework's route key cannot
/// name one from it.
pub fn is_document_host(host: &str) -> bool {
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = host.split(':').next().unwrap_or(host);
    let Some(rest) = host
        .strip_prefix(DOCUMENT_HOST)
        .and_then(|rest| rest.strip_prefix('.'))
    else {
        return false;
    };
    rest.strip_suffix(".amazonaws.com")
        .is_some_and(is_region_shaped)
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
    /// Every compliance inquiry `CreateComplianceInquiry` has created, keyed by its id.
    ///
    /// A `BTreeMap` because the ids are minted from a monotonic counter in fixed-width
    /// hex, so their lexicographic order IS their creation order: that is the stable
    /// total order `ListComplianceInquiries` can index a pagination token into.
    pub inquiries: BTreeMap<String, ComplianceInquiry>,
    /// The counter inquiry ids are minted from, so two creates never share an id.
    pub inquiry_seq: u64,
    /// `CreateComplianceInquiry` `clientToken` -> what it replays. Bounded by
    /// [`INQUIRY_IDEMPOTENCY_CAPACITY`].
    pub inquiry_create_tokens: BTreeMap<String, InquiryCreateRecord>,
    /// The order [`inquiry_create_tokens`](Self::inquiry_create_tokens) were recorded in,
    /// so the oldest is the one forgotten once the map is full. The tokens are
    /// caller-chosen, so their own order says nothing about when they arrived.
    pub inquiry_create_token_order: ::std::collections::VecDeque<String>,
    /// The calling account's customer-agreements, keyed by id. A `BTreeMap` is what
    /// gives `ListCustomerAgreements` the stable total order its pagination token
    /// indexes into.
    pub customer_agreements: BTreeMap<String, CustomerAgreement>,
    /// Every document `ExportComplianceInquiry` has exported, keyed by its export id.
    ///
    /// A `BTreeMap` for the same reason as [`term_tokens`](Self::term_tokens): the ids
    /// are minted from a monotonic counter in fixed-width hex, so the first entry is
    /// always the oldest export, which is what lets
    /// [`ComAmazonawsArtifactArtifactState::record_inquiry_export`] hold the map to
    /// [`INQUIRY_EXPORT_CAPACITY`] without scanning it.
    pub inquiry_exports: BTreeMap<String, InquiryExport>,
    /// The counter export ids are minted from, so two exports never share an id.
    pub inquiry_export_seq: u64,
}

impl Default for ComAmazonawsArtifactArtifactState {
    fn default() -> Self {
        Self {
            reports: seed_catalog(),
            term_tokens: BTreeMap::new(),
            term_token_seq: 0,
            inquiries: BTreeMap::new(),
            inquiry_seq: 0,
            inquiry_create_tokens: BTreeMap::new(),
            inquiry_create_token_order: ::std::collections::VecDeque::new(),
            customer_agreements: seed_customer_agreements(),
            inquiry_exports: BTreeMap::new(),
            inquiry_export_seq: 0,
        }
    }
}

impl ComAmazonawsArtifactArtifactState {
    /// Whether the inquiry store is below [`INQUIRY_CAPACITY`].
    pub fn accepts_another_inquiry(&self) -> bool {
        self.inquiries.len() < INQUIRY_CAPACITY
    }

    /// The TAGGABLE resource one `resourceArn` names, or `None` when this account holds
    /// no such resource.
    ///
    /// A compliance inquiry is the only resource of this service that can carry tags: it
    /// is the only one a caller creates, and the only one whose modelled shapes have a
    /// `tags` member at all (`CreateComplianceInquiryRequest$tags` — "Tags to associate
    /// with the compliance inquiry resource" — and the `tags` of every inquiry
    /// projection; no report, term or customer-agreement shape declares one). So a
    /// `resourceArn` naming anything else names nothing this store holds.
    ///
    /// The ARN is compared WHOLE against the one [`inquiry_arn`] built for the stored
    /// record rather than parsed for an id, which is what makes the partition, the
    /// service, the region and the account segments all have to be the ones this
    /// emulator issued the resource under: an ARN of another account, another region or
    /// another service names no resource here, whatever id it carries.
    pub fn tagged_resource(&self, resource_arn: &str) -> Option<&ComplianceInquiry> {
        self.inquiries
            .values()
            .find(|inquiry| inquiry.arn == resource_arn)
    }

    /// Mint an unused `InquiryId`.
    ///
    /// The counter advances on every call and the loop skips an id the store already
    /// holds, so no two inquiries ever share one. The 16 lower-case hex digits satisfy
    /// `InquiryId`'s `^compliance-inquiry-[a-zA-Z0-9]{16}$`, and they rise with the
    /// counter, so the store's key order is its creation order.
    pub fn mint_inquiry_id(&mut self) -> String {
        loop {
            self.inquiry_seq = self.inquiry_seq.wrapping_add(1);
            let candidate = format!("compliance-inquiry-{:016x}", self.inquiry_seq);
            if !self.inquiries.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// Remember that `token` created `inquiry_id` from the request `fingerprint`,
    /// forgetting the oldest remembered token once [`INQUIRY_IDEMPOTENCY_CAPACITY`] are
    /// held — so a caller sending a fresh `clientToken` on every request cannot grow this
    /// map without end.
    pub fn record_inquiry_create(&mut self, token: String, inquiry_id: &str, fingerprint: &str) {
        let record = InquiryCreateRecord {
            inquiry_id: inquiry_id.to_string(),
            fingerprint: fingerprint.to_string(),
        };
        if self
            .inquiry_create_tokens
            .insert(token.clone(), record)
            .is_none()
        {
            self.inquiry_create_token_order.push_back(token);
        }
        while self.inquiry_create_token_order.len() > INQUIRY_IDEMPOTENCY_CAPACITY {
            if let Some(oldest) = self.inquiry_create_token_order.pop_front() {
                self.inquiry_create_tokens.remove(&oldest);
            }
        }
    }

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

    /// Mint and persist one compliance-inquiry export, returning its id.
    ///
    /// Every call produces a NEW document — that is what exporting is — so the id is
    /// fresh on every call and two exports of one inquiry are two records with two URLs.
    /// The map is held to [`INQUIRY_EXPORT_CAPACITY`] by releasing the oldest export
    /// first, so a caller exporting in a loop cannot grow it without end.
    pub fn record_inquiry_export(
        &mut self,
        inquiry_id: &str,
        query_identifiers: Vec<i32>,
        include_citations: bool,
        document: Vec<u8>,
        now: SystemTime,
    ) -> String {
        while self.inquiry_exports.len() >= INQUIRY_EXPORT_CAPACITY {
            if self.inquiry_exports.pop_first().is_none() {
                break;
            }
        }
        let id = loop {
            self.inquiry_export_seq = self.inquiry_export_seq.wrapping_add(1);
            let candidate = format!("export-{:016x}", self.inquiry_export_seq);
            if !self.inquiry_exports.contains_key(&candidate) {
                break candidate;
            }
        };
        self.inquiry_exports.insert(
            id.clone(),
            InquiryExport {
                id: id.clone(),
                inquiry_id: inquiry_id.to_string(),
                query_identifiers,
                include_citations,
                exported_at: now,
                document,
            },
        );
        id
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

/// The URL of the document one compliance-inquiry export produced.
///
/// Built from the stored export — the inquiry it was exported from and the export's own
/// id — so each export of an inquiry has its own URL and the URL names the record it can
/// be traced back to. As with [`report_document_url`], NO SigV4 signature is attached:
/// this emulator does not sign documents, and a fabricated signature would claim a
/// computation nothing performed.
pub fn inquiry_export_url(region: &str, inquiry_id: &str, export_id: &str) -> String {
    format!(
        "https://{DOCUMENT_HOST}.{region}.amazonaws.com/compliance-inquiry/{inquiry_id}\
         /export/{export_id}/{INQUIRY_EXPORT_OBJECT}"
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

/// The separator a SCOPED pagination token — one cut for the sub-collection of ONE
/// resource, as `ListReportVersions` and `ListComplianceInquiryQueries` both serve —
/// joins the owning resource's id and the resume point with. `:` is a query-safe
/// sub-delimiter, and it cannot appear in a `ReportId` or an `InquiryId`, so the split is
/// unambiguous.
const SCOPED_TOKEN_SEPARATOR: char = ':';

/// The scoped pagination token that resumes the sub-collection of `scope` at `resume_at`.
///
/// ONE spelling for every scoped token this service issues, so the two paginated
/// sub-collections cannot drift apart: the token carries the resource it was cut for, not
/// just an offset, which is what stops a token issued for one resource being spent as a
/// resume point on another and a number a client made up being mistaken for a token the
/// service issued.
fn scoped_token<T: ::std::fmt::Display>(scope: &str, resume_at: T) -> String {
    format!("{scope}{SCOPED_TOKEN_SEPARATOR}{resume_at}")
}

/// The resume point a scoped token names, when it is a token THIS service issued for
/// `scope`. `None` for anything else — a token cut for another resource, a value that is
/// not a resume point at all, or one below the first position the sub-collection numbers
/// from (every sequence this service pages is 1-based).
fn parse_scoped_token<T>(token: &str, scope: &str) -> Option<T>
where
    T: ::std::str::FromStr + PartialOrd + From<u8>,
{
    let (id, resume_at) = token.split_once(SCOPED_TOKEN_SEPARATOR)?;
    if id != scope {
        return None;
    }
    resume_at
        .parse::<T>()
        .ok()
        .filter(|resume_at| *resume_at >= T::from(1u8))
}

/// The `ListReportVersions` pagination token that resumes `report_id` at `version`.
pub fn report_versions_token(report_id: &str, version: i64) -> String {
    scoped_token(report_id, version)
}

/// The version a `ListReportVersions` token resumes at, when it is a token THIS service
/// issued for `report_id`. `None` for anything else.
pub fn parse_report_versions_token(token: &str, report_id: &str) -> Option<i64> {
    parse_scoped_token(token, report_id)
}

/// The `ListComplianceInquiryQueries` pagination token that resumes `inquiry_id` at
/// `identifier`.
///
/// Scoped to the inquiry it was cut for, for the same reason as
/// [`report_versions_token`]: a query identifier is a small sequential number, so an
/// unscoped token would be indistinguishable from one a client made up, and a token issued
/// for one inquiry would be spendable as a resume point on another.
pub fn inquiry_queries_token(inquiry_id: &str, identifier: i32) -> String {
    scoped_token(inquiry_id, identifier)
}

/// The `queryIdentifier` a `ListComplianceInquiryQueries` token resumes at, when it is a
/// token THIS service issued for `inquiry_id`. `None` for anything else.
pub fn parse_inquiry_queries_token(token: &str, inquiry_id: &str) -> Option<i32> {
    parse_scoped_token(token, inquiry_id)
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

/// The ARN of a compliance inquiry.
///
/// Unlike a report, an inquiry is CALLER-owned, so its ARN carries the account id this
/// emulator presents, through `engine::emulator_arn`.
pub fn inquiry_arn(region: &str, inquiry_id: &str) -> String {
    ::engine::emulator_arn("artifact", region, &format!("compliance-inquiry/{inquiry_id}"))
}

// There is deliberately no not-found payload for a compliance inquiry id: an id the
// account holds no inquiry for is answered with the front door's 401
// `UnauthorizedException` (`crate::scaffolded::Scaffolded::unauthorized`) rather than the
// modelled 404, exactly as a `reportId` the catalog does not publish is — the gateway
// authorizes the target before the state is consulted, so the absence of a resource the
// caller may not read is never confirmed.

/// The modelled `ConflictException` for a `clientToken` replayed on a request that is
/// not the one it created an inquiry for.
///
/// An `@idempotencyToken` identifies ONE call: honouring it on different content would
/// either hide the second submission or silently replace the first, so the request is
/// refused instead.
pub fn inquiry_token_mismatch(inquiry_id: &str) -> ConflictException {
    ConflictException {
        message: "the clientToken was already used to create a compliance inquiry from \
                  different content"
            .to_string(),
        resource_id: inquiry_id.to_string(),
        resource_type: INQUIRY_RESOURCE_TYPE.to_string(),
    }
}

/// The modelled `ValidationException` for a token this service never issued, carrying
/// `ValidationExceptionReason`'s `invalidToken`.
///
/// The message is the service's own fixed refusal text ([`INVALID_TOKEN_MESSAGE`]), not a
/// sentence describing which check failed: which token member was refused, and why the
/// emulator could not place it, is information about the SERVER, and a client branches on
/// the error code and the reason rather than on prose. The token itself is not echoed
/// either — the request already carried it.
pub fn invalid_token() -> ValidationException {
    ValidationException {
        message: INVALID_TOKEN_MESSAGE.to_string(),
        reason: REASON_INVALID_TOKEN.to_string(),
        field_list: None,
    }
}

/// The modelled `ValidationException` for a request whose `queryIdentifiers` name queries
/// the targeted inquiry does not own.
///
/// The clause is built from the REQUEST — the member that carried the value and the
/// requirement it failed — in the same validation envelope
/// [`constraint_violation_message`] renders for a modelled constraint, and without
/// echoing the refused identifiers back: the request already carried them.
pub fn unknown_query_identifiers() -> ValidationException {
    ValidationException {
        message: format!("1 validation error detected: {UNKNOWN_QUERY_IDENTIFIER_MESSAGE}"),
        reason: REASON_FIELD_VALIDATION_FAILED.to_string(),
        field_list: Some(vec![ValidationExceptionField {
            name: "queryIdentifiers".to_string(),
            message: UNKNOWN_QUERY_IDENTIFIER_MESSAGE.to_string(),
        }]),
    }
}

/// The clause [`unknown_query_identifiers`] states, in both the message and the
/// `fieldList` entry: the member path and the requirement its value failed.
pub const UNKNOWN_QUERY_IDENTIFIER_MESSAGE: &str =
    "Value at 'queryIdentifiers' failed to satisfy constraint: every query identifier must \
     name a query of the compliance inquiry";

/// The `ValidationException` message every token refusal in this service states: a token
/// (a `nextToken` or a `termToken`) this service did not issue is refused with this one
/// fixed text, which neither names the check that rejected it nor echoes the token.
pub const INVALID_TOKEN_MESSAGE: &str = "The provided token is invalid.";

/// The `ValidationException` message this service states for a request the boundary
/// validator refused against the model's own constraints.
///
/// The AWS validation envelope — `N validation error(s) detected: <clause>; …`, each
/// clause naming the member path and the requirement it failed — with the REJECTED VALUE
/// left out of the clause: a present-but-invalid value is reported as `Value at '<path>'
/// …`, while an absent required member keeps the `Value null at '<path>' …` the framework
/// renders, because absence IS the fact the clause is stating. Returns `None` when there
/// is nothing to state, so the caller keeps the framework's default wording.
///
/// The requirement half of every clause comes from the framework's own rendering of the
/// violation, so a constraint it learns to check cannot drift from this wording; only the
/// echo of the value is removed.
pub fn constraint_violation_message(violations: &[Violation]) -> Option<String> {
    let clauses: Vec<String> = violations.iter().map(clause_without_rejected_value).collect();
    match clauses.len() {
        0 => None,
        1 => Some(format!("1 validation error detected: {}", clauses[0])),
        n => Some(format!(
            "{n} validation errors detected: {}",
            clauses.join("; ")
        )),
    }
}

/// One violation's clause with the rejected value removed from it.
///
/// The framework renders `Value '<shown>' at '<path>' failed to satisfy constraint: …`;
/// the value is known here, so the echo is removed by name rather than by parsing the
/// sentence. A clause that does not start with the echo (an absent member's `Value null`,
/// or a wording the framework changes) is kept exactly as the framework rendered it.
fn clause_without_rejected_value(violation: &Violation) -> String {
    let rendered = violation.message();
    let Some(shown) = violation.value.as_deref() else {
        return rendered;
    };
    match rendered.strip_prefix(&format!("Value '{shown}' at ")) {
        Some(rest) => format!("Value at {rest}"),
        None => rendered,
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
