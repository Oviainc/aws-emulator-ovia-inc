// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice service.
//!
//! The service has two resources in this slice:
//!
//! * the **benefit catalog** — service-owned, read-only data. The API models no
//!   `CreateBenefit`, so the catalogs and the benefits inside them are seeded here, at
//!   state init, and `ListBenefits` reports them from the store like any other read.
//! * the **benefit application** — created, updated, submitted and cancelled by the
//!   caller. Every application is stored exactly once, keyed by its
//!   `benappl-[0-9a-z]{14}` id inside the catalog it was created in, so each operation
//!   reads and writes the one record.
//!
//! Timestamps are stored as epoch MILLISECONDS (`i64`) and rendered into the modelled
//! `date-time` members with [`timestamp`] on the way out. Keeping an integer in the
//! store (rather than a `DateTime`) is what lets [`bump_millis`] guarantee an update is
//! *observably* later than the write before it even when both land inside one clock
//! tick.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};

use ::aws_smithy_types::{Document, Number};

use crate::ops::types::{Contact, FileDetail, FileInput, Tag};

/// This service's ARN namespace, from
/// `smithy_query(query="service_info")` -> `arnNamespace: partnercentral`.
pub const ARN_SERVICE: &str = "partnercentral";

/// The region an ARN carries when the request names none; the emulator's documented
/// default region.
pub const DEFAULT_REGION: &str = "us-east-1";

/// What a handler reports when the shared store cannot be reached. Each handler still
/// builds its own modelled error (the generated error enums are per-operation), so this
/// is the one part of that answer they share.
pub const STORE_UNAVAILABLE: &str =
    "the benefit application store is unavailable; retry the request";

/// The local name of the error shape this service denies an unauthorized caller with:
/// the modelled `com.amazonaws.partnercentralbenefits#AccessDeniedException`, which is
/// also what `smithy_server_runtime::OperationError::shape_name` reports for it and what
/// the wire carries as the error code.
pub const ACCESS_DENIED_SHAPE: &str = "AccessDeniedException";

/// The HTTP status this service serves `AccessDeniedException` with.
///
/// The model declares the shape `@error("client")` with `@httpError(403)`, but the
/// service answers its access denial with **400**: recorded behaviour (corpus
/// `simparity-wire-aws-v1`, fixture
/// `partnercentral-benefits-list-and-tag-validation-error-paths`, step `task#1`
/// `ListBenefits` and step `task#4` `ListTagsForResource`), where the body is the
/// modelled `AccessDeniedException` carrying the partner-entitlement sentence and the
/// status on the wire is 400. A status the model does not predict is settled by what the
/// service serves, so the recording is the authority for this one.
pub const ACCESS_DENIED_STATUS: u16 = 400;

/// The status this service serves a modelled error with, given the status that error's
/// own `@httpError` declares.
///
/// The model is the authority for every error shape but [`ACCESS_DENIED_SHAPE`], whose
/// served status is [`ACCESS_DENIED_STATUS`]. The choice is keyed on the error SHAPE and
/// made once for the whole service — `crate::scaffolded`'s `OperationError::http_status`
/// is its only caller — so every operation that can deny a caller answers the same
/// status rather than one handler differing from the next.
pub fn served_status(shape_name: &str, modelled: u16) -> u16 {
    match shape_name {
        ACCESS_DENIED_SHAPE => ACCESS_DENIED_STATUS,
        _ => modelled,
    }
}

/// What this service answers a caller whose account holds no active AWS partner
/// benefit: the entitlement gate's refusal, carried by the modelled
/// `AccessDeniedException` (served with [`ACCESS_DENIED_STATUS`], which is the status the
/// recordings show rather than the model's `@httpError`).
///
/// The gate is an account-level authorization stage, not a per-request computation, so
/// its wording is a fixed sentence rather than something built from the request. This
/// is the sentence the service answers with, recorded against the real API.
pub const PARTNER_ENTITLEMENT_REQUIRED: &str = "This action requires an active AWS \
     partner benefit. Register as an AWS partner if you haven't already, or contact AWS \
     partner central support.";

/// The name a `ResourceNotFoundException` message gives the catalog a request named.
///
/// The catalog is not a modelled `@resource` (no operation creates one); it is named by
/// the `Catalog` input member every operation carries, so that member's name is what the
/// message states. See [`not_found_message`].
pub const CATALOG_KIND: &str = "Catalog";

/// The name a `ResourceNotFoundException` message gives a benefit: the modelled resource
/// shape `com.amazonaws.partnercentralbenefits#Benefit`
/// (`smithy_query(query="shape",shape="Benefit")`).
pub const BENEFIT_KIND: &str = "Benefit";

/// The name a `ResourceNotFoundException` message gives a benefit application: the
/// modelled resource shape `com.amazonaws.partnercentralbenefits#BenefitApplication`
/// (`smithy_query(query="shape",shape="BenefitApplication")`).
pub const APPLICATION_KIND: &str = "BenefitApplication";

/// The prefix a benefit id carries, from the modelled `BenefitSummary$Id` pattern
/// `^ben-[0-9a-z]{14}$`.
pub const BENEFIT_ID_PREFIX: &str = "ben-";

/// The prefix a benefit-application id carries, from the modelled
/// `BenefitApplicationId` pattern `^benappl-[0-9a-z]{14}$`.
pub const APPLICATION_ID_PREFIX: &str = "benappl-";

/// How many characters follow the prefix in either id, from those same patterns.
pub const ID_BODY_LEN: usize = 14;

/// The characters an id body is built from: `[0-9a-z]`, from those same patterns.
const ID_ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// The `BenefitApplicationStatus` a freshly created application carries — the one the
/// enum names for an application the partner has not submitted yet.
pub const STATUS_PENDING_SUBMISSION: &str = "PENDING_SUBMISSION";

/// The `BenefitApplicationStatus` a submitted application carries while AWS reviews it.
pub const STATUS_IN_REVIEW: &str = "IN_REVIEW";

/// The terminal `BenefitApplicationStatus` of a cancelled application.
pub const STATUS_CANCELED: &str = "CANCELED";

/// The `BenefitApplicationStatus` a submitted application carries while AWS waits on the
/// partner. No operation in this slice puts an application into it — AWS's own review
/// workflow does — so it is named here only because [`RECALLABLE_STATUSES`] is defined
/// over the statuses a *submitted* application can carry, not over one example of them.
pub const STATUS_ACTION_REQUIRED: &str = "ACTION_REQUIRED";

/// The `BenefitApplicationStatus` of an application AWS's review approved. No operation
/// in this slice puts an application into it — AWS's own review workflow does — so it is
/// named here because [`TERMINAL_STATUSES`] is defined over every status AWS has finished
/// with, not over the one this emulator can reach.
pub const STATUS_APPROVED: &str = "APPROVED";

/// The `BenefitApplicationStatus` of an application AWS's review rejected. Reached the
/// same way as [`STATUS_APPROVED`], and terminal for the same reason.
pub const STATUS_REJECTED: &str = "REJECTED";

/// The statuses AWS has finished with: `APPROVED` and `REJECTED` end the review, and
/// `CANCELED` ends the application.
///
/// Nothing moves an application out of one of these — `SubmitBenefitApplication`,
/// `RecallBenefitApplication` and `CancelBenefitApplication` each refuse from them — so
/// no change to the application's content is accepted in one either (see
/// [`BenefitApplication::is_terminal`]). The six `BenefitApplicationStatus` values are
/// `crate::ops::types::BENEFIT_APPLICATION_STATUS_VALUES`.
pub const TERMINAL_STATUSES: &[&str] = &[STATUS_APPROVED, STATUS_REJECTED, STATUS_CANCELED];

/// The statuses in which `UpdateBenefitApplication` rewrites an application's members.
///
/// An update is the partner editing its own DRAFT, so it is accepted only while the
/// application has not been submitted: `PENDING_SUBMISSION`. Once it is submitted the
/// record is the one AWS is reviewing, which is why the API models
/// `RecallBenefitApplication` (back to draft "for further modifications") and
/// `AmendBenefitApplication` (change named fields of a submitted application, with an
/// `AmendmentReason`); from every other status an update is refused with the modelled
/// `ConflictException`. See [`BenefitApplication::is_updatable`].
pub const UPDATABLE_STATUSES: &[&str] = &[STATUS_PENDING_SUBMISSION];

/// The statuses `RecallBenefitApplication` returns to draft.
///
/// A recall takes back an application that has been SUBMITTED and that AWS has not
/// finished with: `PENDING_SUBMISSION` was never submitted, and `APPROVED`, `REJECTED`
/// and `CANCELED` are terminal, so a recall from any of those is refused with the
/// modelled `ConflictException`. The six `BenefitApplicationStatus` values are
/// `crate::ops::types::BENEFIT_APPLICATION_STATUS_VALUES`.
pub const RECALLABLE_STATUSES: &[&str] = &[STATUS_IN_REVIEW, STATUS_ACTION_REQUIRED];

/// The two `BenefitStatus` values the model declares (see
/// `crate::ops::types::BENEFIT_STATUS_VALUES`).
pub const BENEFIT_STATUS_ACTIVE: &str = "ACTIVE";
/// See [`BENEFIT_STATUS_ACTIVE`].
pub const BENEFIT_STATUS_INACTIVE: &str = "INACTIVE";

/// The largest page either list operation builds, and the size applied when the request
/// names no `MaxResults`. It is the maximum of the modelled `MaxResults` range
/// (`range {min: 1, max: 100}`), which the centralized walk already refuses values
/// outside of.
pub const MAX_PAGE_SIZE: i32 = 100;

/// The most benefit applications this emulator holds in one catalog.
///
/// `CreateBenefitApplication` is the only operation that grows the store, so this is the
/// bound that keeps it finite. AWS publishes no quota for it, so the number is CHOSEN:
/// large enough that ordinary use never reaches it, small enough that a caller cannot
/// grow the process's memory without limit. `client_tokens` holds at most one entry per
/// application created, so it is bounded by the same count.
pub const MAX_APPLICATIONS_PER_CATALOG: usize = 500;

/// The most amendments one `AmendBenefitApplication` request may carry.
///
/// The modelled `AmendmentList` has no `@length`, so this is the bound on the work one
/// request drives — the loop that applies them runs under the store lock — and on how
/// much the amendment record below keeps. CHOSEN, not an AWS quota: far more fields than
/// one correction touches, while keeping one request's work and footprint finite.
pub const MAX_AMENDMENTS_PER_REQUEST: usize = 20;

/// The longest `Amendment$FieldPath` this service reads.
///
/// The member is an unconstrained `String`, so this is what keeps a path (and the record
/// of it below) from being arbitrarily long. CHOSEN: wider than any path into a document
/// this service stores, since [`MAX_DETAILS_DEPTH`] also caps the segments a path may
/// have.
pub const MAX_FIELD_PATH_LEN: usize = 256;

/// The longest value an amendment may write into `BenefitApplicationDetails`.
///
/// `Amendment$NewValue` is an unconstrained `String`, while the members a path can name
/// on the record itself carry the model's own `@length` (`BenefitApplicationName` 1..255,
/// `BenefitApplicationDescription` 1..1000). This is the matching bound for a leaf of
/// the document, which the model constrains not at all: the longest string member the
/// model lets a caller store on this resource.
pub const MAX_DETAILS_VALUE_LEN: usize = 1000;

/// The longest `AmendmentReason` this service stores, which is the modelled
/// `AmendBenefitApplicationInput$AmendmentReason` `@length(max: 1000)`
/// (`smithy_query(query="operation",operation="AmendBenefitApplication")`). The
/// amendment record below keeps the reason, so this is also the bound on the bytes one
/// record holds for it.
pub const MAX_AMENDMENT_REASON_LEN: usize = 1000;

/// The longest `ClientToken` this service stores, which is the modelled
/// `AmendBenefitApplicationInput$ClientToken` `@length(min: 1, max: 64)` (its
/// `@pattern` `^[a-zA-Z0-9-]{1,64}$` says the same). The amendment record keeps the
/// token, so this bounds the bytes one record holds for it.
pub const MAX_CLIENT_TOKEN_LEN: usize = 64;

/// The most amendments one benefit application keeps a record of.
///
/// `AmendBenefitApplication` is idempotent on its `ClientToken`, which it recognises by
/// looking the token up in this record, so the record is what bounds how far back a
/// retry is recognised. AWS publishes no retention for the token, so the number is
/// CHOSEN: more amendments than a retry window needs, while keeping the record of one
/// application finite however many times it is amended. Beyond it the oldest amendment
/// is dropped, and a retry of a token that old applies its amendments again.
///
/// With [`MAX_AMENDMENTS_PER_REQUEST`], [`MAX_FIELD_PATH_LEN`],
/// [`MAX_AMENDMENT_REASON_LEN`] and [`MAX_CLIENT_TOKEN_LEN`] — each enforced by
/// `AmendBenefitApplication` before the record is written — one application's amendment
/// record holds at most 20 x (64-byte token + 1000-byte reason + 20 x 256-byte path) of
/// caller text: bounded, like every other part of this store.
pub const MAX_AMENDMENT_HISTORY: usize = 20;

/// The most members a stored `BenefitApplicationDetails` document may hold, counted over
/// the whole tree.
///
/// The member is an unconstrained `Document` and `AmendBenefitApplication` can ADD a
/// member the document did not hold, so a count bound is what keeps repeated amendments
/// from growing one record without limit. Emulator resource guard, NOT an AWS quota.
pub const MAX_DETAILS_MEMBERS: usize = 256;

/// The most bytes of text (member names and string values) a stored
/// `BenefitApplicationDetails` document may carry. Emulator resource guard, NOT an AWS
/// quota; see [`MAX_DETAILS_MEMBERS`].
pub const MAX_DETAILS_BYTES: usize = 64 * 1024;

/// The most tags one resource of this service holds.
///
/// It is the model's own published bound on an application's tag set —
/// `CreateBenefitApplicationInput$Tags` is `length {max: 200}` (see
/// `smithy_query(query="shape",shape="CreateBenefitApplicationInput")`) — so a create
/// and a `TagResource` agree on how many tags one resource can carry.
/// `TagResourceRequest$tags` itself carries no length, so this is the bound that keeps a
/// resource's tag set finite; a merge that would take it past this is refused with the
/// modelled `ServiceQuotaExceededException`.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

/// The `ServiceQuotaExceededException$ResourceType` a tagging refusal reports: the
/// resource-type segment of the ARN whose tag set is full.
pub const QUOTA_RESOURCE_TYPE_APPLICATION: &str = APPLICATION_RESOURCE;

/// The `ServiceQuotaExceededException$QuotaCode` a tagging refusal reports.
///
/// The member is required and AWS publishes no quota code for this service's tag limit,
/// so the code NAMES the quota ([`MAX_TAGS_PER_RESOURCE`]) rather than imitating a
/// Service Quotas `L-########` identifier that no AWS rule produces.
pub const QUOTA_CODE_TAGS_PER_RESOURCE: &str = "TagsPerResource";

/// How this service renders a `Catalog` the request did not name, in the message of a
/// lookup that failed (see [`catalog_not_found_message`]).
pub const CATALOG_ABSENT: &str = "null";

/// The deepest `BenefitApplicationDetails` document this service stores.
///
/// The member is modelled as an unconstrained `Document`, so its nesting is entirely
/// caller-controlled and every walk over it is recursive. This is the bound that keeps
/// such a walk from exhausting the stack; a create or update carrying a deeper document
/// is refused with the modelled `ValidationException` rather than stored. Emulator
/// resource guard, NOT an AWS quota.
pub const MAX_DETAILS_DEPTH: usize = 32;

/// The catalogs this service serves.
///
/// PartnerCentral models `Catalog` as a required input on every operation and offers no
/// operation that creates one, so the set is fixed: `AWS` is the production catalog and
/// `Sandbox` the test catalog that PartnerCentral's APIs document. A request naming any
/// other catalog resolves to no catalog, and the handlers answer the modelled
/// `ResourceNotFoundException`.
pub const SEEDED_CATALOGS: &[&str] = &["AWS", "Sandbox"];

/// One benefit in the seeded catalog, before its per-catalog id is derived.
struct SeededBenefit {
    name: &'static str,
    description: &'static str,
    programs: &'static [&'static str],
    fulfillment_types: &'static [&'static str],
    status: &'static str,
    /// `BenefitRequestSchema` members: the field name and the JSON type a request for
    /// this benefit must put in `BenefitApplicationDetails`.
    schema: &'static [(&'static str, &'static str)],
}

/// The benefits every catalog is seeded with.
///
/// Each `fulfillment_types` entry is a value of the model's `FulfillmentType` enum
/// (`crate::ops::types::FULFILLMENT_TYPE_VALUES`) and each `status` a value of its
/// `BenefitStatus` enum (`crate::ops::types::BENEFIT_STATUS_VALUES`); a `programs` entry
/// satisfies the modelled `Program` constraint (len 1..255, `^[A-Za-z0-9_-]+$`). The set
/// deliberately spans every fulfillment type and both statuses, so a filtered
/// `ListBenefits` has something to include and something to exclude.
const SEEDED_BENEFITS: &[SeededBenefit] = &[
    SeededBenefit {
        name: "Marketing Development Funds",
        description: "Cash funding for partner-led marketing campaigns.",
        programs: &["AWS_PARTNER_NETWORK"],
        fulfillment_types: &["CASH"],
        status: BENEFIT_STATUS_ACTIVE,
        schema: &[("CampaignName", "string"), ("RequestedAmount", "number")],
    },
    SeededBenefit {
        name: "Promotional Credits",
        description: "AWS promotional credits for customer proofs of concept.",
        programs: &["ISV_ACCELERATE"],
        fulfillment_types: &["CREDITS"],
        status: BENEFIT_STATUS_ACTIVE,
        schema: &[("CustomerName", "string"), ("RequestedCredits", "number")],
    },
    SeededBenefit {
        name: "Partner Training Access",
        description: "Seats in AWS partner training and certification programs.",
        programs: &["AWS_PARTNER_NETWORK"],
        fulfillment_types: &["ACCESS"],
        status: BENEFIT_STATUS_ACTIVE,
        schema: &[("SeatCount", "number")],
    },
    SeededBenefit {
        name: "Migration Acceleration Funding",
        description: "Funding and credits for migration engagements.",
        programs: &["MIGRATION_ACCELERATION_PROGRAM"],
        fulfillment_types: &["CASH", "CREDITS"],
        status: BENEFIT_STATUS_ACTIVE,
        schema: &[("WorkloadName", "string"), ("RequestedAmount", "number")],
    },
    SeededBenefit {
        name: "Retired Innovation Sandbox",
        description: "A retired sandbox benefit, kept for applications that reference it.",
        programs: &["ISV_ACCELERATE"],
        fulfillment_types: &["ACCESS"],
        status: BENEFIT_STATUS_INACTIVE,
        schema: &[],
    },
];

/// File extension -> the `FileType` a `FileDetail` reports for it.
///
/// Every value is one of the model's own `FileType` enum values, whose catalog is
/// `crate::ops::types::FILE_TYPE_VALUES`
/// (`smithy_query(query="shape",shape="FileType")`): the enum is a list of media types,
/// so the file's type is the media type its extension names. An extension not listed
/// here leaves `FileType` absent rather than guessing one.
const FILE_TYPES_BY_EXTENSION: &[(&str, &str)] = &[
    ("csv", "text/csv"),
    ("doc", "application/msword"),
    (
        "docx",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    ),
    ("jpeg", "image/jpeg"),
    ("jpg", "image/jpeg"),
    ("pdf", "application/pdf"),
    ("png", "image/png"),
    (
        "pptx",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    ),
    ("svg", "image/svg+xml"),
    (
        "xlsx",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    ),
];

/// One benefit the service offers: seeded, read-only catalog data.
#[derive(Debug, Clone)]
pub struct Benefit {
    /// `ben-` + 14 `[0-9a-z]` characters, derived from the catalog and the benefit's
    /// name, so the same benefit keeps the same id across runs and two catalogs never
    /// share one.
    pub id: String,
    pub name: String,
    pub description: String,
    pub programs: Vec<String>,
    /// `FulfillmentType` values.
    pub fulfillment_types: Vec<String>,
    pub benefit_request_schema: Option<Document>,
    /// A `BenefitStatus` value.
    pub status: String,
}

/// One amendment request `AmendBenefitApplication` applied to a benefit application.
///
/// Kept so a replayed `ClientToken` applies nothing a second time, and as the record of
/// WHICH fields each amendment changed and why. No modelled output reports it: the model
/// binds no amendment member on any output shape, so it is state this service holds
/// rather than something it reports. The new values are not copied here — the fields
/// themselves hold them — which is also what keeps the record small.
#[derive(Debug, Clone)]
pub struct AppliedAmendment {
    /// The request's `ClientToken`.
    pub client_token: String,
    /// The request's `AmendmentReason`.
    pub reason: String,
    /// The `FieldPath` of each amendment applied, in the order they were applied; at
    /// most [`MAX_AMENDMENTS_PER_REQUEST`] of them, each at most
    /// [`MAX_FIELD_PATH_LEN`] bytes.
    pub field_paths: Vec<String>,
    /// When the amendment was applied, as epoch milliseconds.
    pub applied_at_millis: i64,
    /// The revision the amendment produced.
    pub revision: u64,
}

/// One stored benefit application: the single source of truth for every application
/// operation.
#[derive(Debug, Clone)]
pub struct BenefitApplication {
    pub id: String,
    pub arn: String,
    pub catalog: String,
    /// The `ben-...` id of the benefit this application requests.
    pub benefit_id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    /// `FulfillmentType` values, as requested on the create.
    pub fulfillment_types: Vec<String>,
    pub benefit_application_details: Option<Document>,
    /// The partner programs this application belongs to: the programs of the benefit it
    /// requests, not a caller-supplied value — the model binds no `Programs` member on
    /// any application input.
    pub programs: Vec<String>,
    /// A `BenefitApplicationStatus` value.
    pub status: String,
    /// The workflow stage. AWS sets it as its own review workflow advances and no input
    /// member carries one, so it stays absent in this emulator.
    pub stage: Option<String>,
    pub status_reason: Option<String>,
    pub status_reason_codes: Vec<String>,
    pub created_at_millis: i64,
    pub updated_at_millis: i64,
    /// The optimistic-concurrency revision, rendered on the wire by
    /// [`BenefitApplication::revision_text`].
    pub revision: u64,
    /// The revision the create minted. Replaying the create's `ClientToken` answers with
    /// THIS revision, so a retry sees the first call's response even if the application
    /// has been updated since.
    pub created_revision: u64,
    /// The `ClientToken` of the recall that put this application back into
    /// `PENDING_SUBMISSION`, if one did and it is still in draft.
    ///
    /// `RecallBenefitApplication` documents its `ClientToken` as what makes the recall
    /// idempotent, so a retry carrying the token of the recall still in effect answers
    /// success and changes nothing. One token per application, so it is bounded by the
    /// application count; a fresh submission makes the application recallable again, at
    /// which point the token no longer matches a recall in effect.
    pub recall_client_token: Option<String>,
    pub associated_resources: Vec<String>,
    pub partner_contacts: Vec<Contact>,
    pub file_details: Vec<FileDetail>,
    pub tags: BTreeMap<String, String>,
    /// The amendments applied to this application, oldest first, at most
    /// [`MAX_AMENDMENT_HISTORY`] of them.
    pub amendments: Vec<AppliedAmendment>,
}

impl BenefitApplication {
    /// The current revision as the modelled `Revision` string.
    pub fn revision_text(&self) -> String {
        self.revision.to_string()
    }

    /// The revision the create returned, as the modelled `Revision` string.
    pub fn created_revision_text(&self) -> String {
        self.created_revision.to_string()
    }

    /// Advance `UpdatedAt` past the previous write, without changing the revision.
    ///
    /// The revision is the optimistic-concurrency token `UpdateBenefitApplication`
    /// checks, so only a change to the application's own content moves it (see
    /// [`BenefitApplication::revise`]); a status transition the caller asked for records
    /// only the time.
    pub fn touch(&mut self, now_millis: i64) {
        self.updated_at_millis = bump_millis(self.updated_at_millis, now_millis);
    }

    /// Whether a recall may take this application back to draft: true exactly for the
    /// statuses in [`RECALLABLE_STATUSES`].
    pub fn is_recallable(&self) -> bool {
        RECALLABLE_STATUSES.contains(&self.status.as_str())
    }

    /// Whether `UpdateBenefitApplication` may rewrite this application's members: true
    /// exactly for the statuses in [`UPDATABLE_STATUSES`].
    pub fn is_updatable(&self) -> bool {
        UPDATABLE_STATUSES.contains(&self.status.as_str())
    }

    /// Whether AWS has finished with this application: true exactly for the statuses in
    /// [`TERMINAL_STATUSES`]. No change to its content is accepted in one.
    pub fn is_terminal(&self) -> bool {
        TERMINAL_STATUSES.contains(&self.status.as_str())
    }

    /// Whether `client_token` is the token of the recall that is still in effect on this
    /// application — i.e. a retry of that same recall, which must change nothing.
    ///
    /// Takes the token itself, never an `Option`, so a request that carries none cannot
    /// be read as a retry of a recall that never happened: the caller passes a token only
    /// when the request has one.
    pub fn is_recall_retry(&self, client_token: &str) -> bool {
        self.status == STATUS_PENDING_SUBMISSION
            && self.recall_client_token.as_deref() == Some(client_token)
    }

    /// Record a content change: bump the revision and advance `UpdatedAt`.
    ///
    /// Any content change also ENDS a recall's idempotency window: the record the recall
    /// left has moved on, so replaying that recall must not answer as a retry of it. A
    /// recall itself records its token after calling this.
    pub fn revise(&mut self, now_millis: i64) {
        self.recall_client_token = None;
        self.revision = self.revision.saturating_add(1);
        self.touch(now_millis);
    }

    /// The amendment `client_token` already applied to this application, if this
    /// application still holds a record of it.
    pub fn applied_amendment(&self, client_token: &str) -> Option<&AppliedAmendment> {
        self.amendments
            .iter()
            .find(|applied| applied.client_token == client_token)
    }

    /// Record one applied amendment, dropping the oldest once the record is full.
    pub fn record_amendment(&mut self, applied: AppliedAmendment) {
        self.amendments.push(applied);
        if self.amendments.len() > MAX_AMENDMENT_HISTORY {
            self.amendments.remove(0);
        }
    }

    /// A list member, omitted from the wire when nothing was ever stored for it.
    pub fn optional_list<T: Clone>(values: &[T]) -> Option<Vec<T>> {
        (!values.is_empty()).then(|| values.to_vec())
    }

    /// `BenefitApplicationSummary$BenefitApplicationDetails`, which the model types as
    /// `Attributes` (`map<String, String>`) while the full record carries a `Document`.
    ///
    /// The summary therefore reports the stored document's top-level members, each
    /// rendered by [`document_text`]. A document that is not an object has no top-level
    /// members, so the summary omits the member rather than inventing one.
    pub fn summary_details(&self) -> Option<HashMap<String, String>> {
        let Document::Object(members) = self.benefit_application_details.as_ref()? else {
            return None;
        };
        Some(
            members
                .iter()
                .map(|(name, value)| (name.clone(), document_text(value)))
                .collect(),
        )
    }

    /// Whether this application carries at least one of `arns` in its associated
    /// resources. An empty filter matches everything.
    pub fn matches_resource_arns(&self, arns: &[String]) -> bool {
        arns.is_empty()
            || arns
                .iter()
                .any(|wanted| self.associated_resources.contains(wanted))
    }
}

/// One benefit catalog: its seeded benefits, the applications created in it, and the
/// `ClientToken`s those creates were made with.
#[derive(Debug, Clone, Default)]
pub struct Catalog {
    /// Benefits by `ben-...` id, in one stable order so `ListBenefits` can page.
    pub benefits: BTreeMap<String, Benefit>,
    /// Applications by `benappl-...` id, in one stable order so
    /// `ListBenefitApplications` can page.
    pub applications: BTreeMap<String, BenefitApplication>,
    /// `ClientToken` -> the application id the create it completed minted.
    pub client_tokens: BTreeMap<String, String>,
}

impl Catalog {
    /// The catalog named `catalog`, with [`SEEDED_BENEFITS`] in it.
    fn seeded(catalog: &str) -> Self {
        let benefits = SEEDED_BENEFITS
            .iter()
            .map(|seed| {
                let id = benefit_id(catalog, seed.name);
                let benefit = Benefit {
                    id: id.clone(),
                    name: seed.name.to_string(),
                    description: seed.description.to_string(),
                    programs: seed.programs.iter().map(|p| (*p).to_string()).collect(),
                    fulfillment_types: seed
                        .fulfillment_types
                        .iter()
                        .map(|t| (*t).to_string())
                        .collect(),
                    benefit_request_schema: benefit_request_schema(seed.schema),
                    status: seed.status.to_string(),
                };
                (id, benefit)
            })
            .collect();
        Self {
            benefits,
            applications: BTreeMap::new(),
            client_tokens: BTreeMap::new(),
        }
    }

    /// The id of the benefit `identifier` names, accepting either form the modelled
    /// `BenefitId` pattern allows (`^(arn:.+|ben-[0-9a-z]{14})$`): the bare id, or a
    /// benefit ARN of this catalog. `None` when this catalog holds no such benefit.
    pub fn resolve_benefit_id(&self, catalog: &str, identifier: &str) -> Option<String> {
        let id = if identifier.starts_with("arn:") {
            let (arn_catalog, arn_id) = parse_resource_arn(identifier, BENEFIT_RESOURCE)?;
            if arn_catalog != catalog {
                return None;
            }
            arn_id.to_string()
        } else {
            identifier.to_string()
        };
        self.benefits.contains_key(&id).then_some(id)
    }
}

/// The resource-type segment a benefit ARN carries.
pub const BENEFIT_RESOURCE: &str = "benefit";
/// The resource-type segment a benefit-application ARN carries.
pub const APPLICATION_RESOURCE: &str = "benefit-application";

/// Per-service state for com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice. A handler reaches it through [`service_state`].
///
/// BOUNDED: `CreateBenefitApplication` refuses a create that would take a catalog past
/// [`MAX_APPLICATIONS_PER_CATALOG`], the catalog set is fixed by [`SEEDED_CATALOGS`],
/// `arns` and `client_tokens` carry at most one entry per stored application, and one
/// application records at most [`MAX_AMENDMENT_HISTORY`] amendments — so no caller can
/// grow this store without limit.
#[derive(Debug)]
pub struct ComAmazonawsPartnercentralbenefitsPartnercentralbenefitsserviceState {
    /// Every catalog, keyed by the `Catalog` input string.
    pub catalogs: BTreeMap<String, Catalog>,
    /// The AWS accounts that hold an active AWS partner benefit — the whole input to the
    /// entitlement gate [`entitlement_refusal`] applies ahead of every operation.
    ///
    /// EMPTY at state init, and deliberately so: being a registered AWS partner is a
    /// property of an account that this API cannot establish — it models no partner
    /// registration — so no account is entitled until something outside the API records
    /// one. Seeding an entitled account here would make the gate unobservable, which is
    /// the defect this field exists to close.
    partner_accounts: BTreeSet<String>,
    /// Application ARN -> (catalog, application id), so an `Identifier` given as an ARN
    /// resolves to the same record as the bare id.
    pub arns: HashMap<String, (String, String)>,
    /// Monotonic counter feeding [`Self::mint_application_id`]; it is what makes a minted
    /// id unique even within one clock tick.
    id_sequence: u64,
}

impl Default for ComAmazonawsPartnercentralbenefitsPartnercentralbenefitsserviceState {
    /// The benefit catalog is service-owned — the API models no `CreateBenefit` — so it
    /// is seeded here, where the engine first touches this service's state.
    fn default() -> Self {
        Self {
            catalogs: SEEDED_CATALOGS
                .iter()
                .map(|name| ((*name).to_string(), Catalog::seeded(name)))
                .collect(),
            partner_accounts: BTreeSet::new(),
            arns: HashMap::new(),
            id_sequence: 0,
        }
    }
}

impl ComAmazonawsPartnercentralbenefitsPartnercentralbenefitsserviceState {
    /// Whether `account` holds an active AWS partner benefit, which is what the
    /// entitlement gate admits a request on.
    pub fn has_active_partner_benefit(&self, account: &str) -> bool {
        self.partner_accounts.contains(account)
    }

    /// Record an active AWS partner benefit for `account`, so the gate admits its
    /// requests. `true` when this call is what recorded it.
    ///
    /// No modelled operation reaches this: the API has none that registers a partner, so
    /// entitlement arrives from outside the API — which in this emulator means an
    /// in-process caller holding the shared engine (a test standing in for an entitled
    /// partner account). Nothing on the wire can grant itself the entitlement.
    pub fn grant_partner_entitlement(&mut self, account: &str) -> bool {
        self.partner_accounts.insert(account.to_string())
    }

    /// The application `identifier` names inside `catalog`, accepting either form the
    /// modelled `BenefitApplicationIdentifier` allows (`^(arn:.+|benappl-[0-9a-z]{14})$`).
    ///
    /// An ARN must match a stored ARN exactly, so an ARN naming another partition,
    /// region or account resolves to nothing rather than to the application that happens
    /// to share its id segment — and an ARN whose catalog is not the one the request
    /// named resolves to nothing either.
    pub fn resolve_application_id(&self, catalog: &str, identifier: &str) -> Option<String> {
        if identifier.starts_with("arn:") {
            return self
                .arns
                .get(identifier)
                .filter(|(arn_catalog, _)| arn_catalog == catalog)
                .map(|(_, id)| id.clone());
        }
        self.catalogs
            .get(catalog)
            .filter(|stored| stored.applications.contains_key(identifier))
            .map(|_| identifier.to_string())
    }

    /// The application `identifier` names, for a reporting operation.
    pub fn application(&self, catalog: &str, identifier: &str) -> Option<&BenefitApplication> {
        let id = self.resolve_application_id(catalog, identifier)?;
        self.catalogs.get(catalog)?.applications.get(&id)
    }

    /// The application `identifier` names, for an operation the spec says mutates it.
    ///
    /// `id`, `arn` and `catalog` are this record's identity — the store is keyed on the
    /// id and `arns` on the ARN — so none of them may be reassigned through this handle.
    /// No operation needs to: the model binds no member that moves an application
    /// between catalogs.
    pub fn application_mut(
        &mut self,
        catalog: &str,
        identifier: &str,
    ) -> Option<&mut BenefitApplication> {
        let id = self.resolve_application_id(catalog, identifier)?;
        self.catalogs.get_mut(catalog)?.applications.get_mut(&id)
    }

    /// Store a newly created application and index it by ARN and by `ClientToken`.
    ///
    /// `None` when the catalog does not exist, which a handler has already refused.
    pub fn insert_application(&mut self, client_token: String, application: BenefitApplication) {
        self.arns.insert(
            application.arn.clone(),
            (application.catalog.clone(), application.id.clone()),
        );
        if let Some(catalog) = self.catalogs.get_mut(&application.catalog) {
            catalog
                .client_tokens
                .insert(client_token, application.id.clone());
            catalog
                .applications
                .insert(application.id.clone(), application);
        }
    }

    /// The resource a tagging operation's `resourceArn` names, for a report of its tags.
    ///
    /// The tagging operations (`TagResource`, `UntagResource`, `ListTagsForResource`)
    /// name their resource by ARN alone — they bind no `Catalog` member — so the lookup
    /// goes through the same ARN index [`Self::resolve_application_id`] uses, and an ARN
    /// naming another partition, region or account resolves to nothing rather than to
    /// the resource that happens to share its id segment.
    ///
    /// The benefit application is the one resource of this service that holds a tag set:
    /// it is the only shape with a `Tags` member (`CreateBenefitApplicationInput$Tags`),
    /// while a benefit and a benefit allocation carry none on any input or output. An
    /// ARN of any other resource type therefore resolves to no taggable resource.
    pub fn taggable_resource(&self, resource_arn: &str) -> Option<&BenefitApplication> {
        let (catalog, id) = self.arns.get(resource_arn)?;
        self.catalogs.get(catalog)?.applications.get(id)
    }

    /// The resource a tagging operation's `resourceArn` names, for a call that changes
    /// its tag set. Resolved exactly as [`Self::taggable_resource`].
    pub fn taggable_resource_mut(
        &mut self,
        resource_arn: &str,
    ) -> Option<&mut BenefitApplication> {
        let (catalog, id) = self.arns.get(resource_arn)?.clone();
        self.catalogs.get_mut(&catalog)?.applications.get_mut(&id)
    }

    /// A `benappl-[0-9a-z]{14}` id no stored application is using.
    ///
    /// The sequence number makes two ids minted in the same millisecond differ, and the
    /// mixing step keeps them from being consecutive, so a caller cannot derive the next
    /// id from the last.
    pub fn mint_application_id(&mut self, now_millis: i64) -> String {
        for _ in 0..1024 {
            self.id_sequence = self.id_sequence.wrapping_add(1);
            let seed = splitmix64((now_millis as u64) ^ self.id_sequence.rotate_left(17));
            let candidate = format!("{APPLICATION_ID_PREFIX}{}", id_body(seed));
            if !self.holds_application(&candidate) {
                return candidate;
            }
        }
        // Bounded fallback, so a pathological clock can never spin here: the
        // zero-padded decimals are also `[0-9a-z]{14}`, and there are always more of
        // them than there are stored applications.
        (0u64..=self.application_count() as u64)
            .map(|n| format!("{APPLICATION_ID_PREFIX}{n:014}"))
            .find(|candidate| !self.holds_application(candidate))
            .unwrap_or_else(|| format!("{APPLICATION_ID_PREFIX}{:014}", self.application_count()))
    }

    fn holds_application(&self, id: &str) -> bool {
        self.catalogs
            .values()
            .any(|catalog| catalog.applications.contains_key(id))
    }

    fn application_count(&self) -> usize {
        self.catalogs
            .values()
            .map(|catalog| catalog.applications.len())
            .sum()
    }
}

/// The `BenefitRequestSchema` document for a seeded benefit's field list, or `None` when
/// the benefit declares no fields.
///
/// A JSON-Schema-shaped object: `{"type":"object","properties":{"<field>":{"type":"<t>"}}}`.
fn benefit_request_schema(fields: &[(&str, &str)]) -> Option<Document> {
    if fields.is_empty() {
        return None;
    }
    let properties: HashMap<String, Document> = fields
        .iter()
        .map(|(name, json_type)| {
            let mut property = HashMap::new();
            property.insert(
                "type".to_string(),
                Document::String((*json_type).to_string()),
            );
            ((*name).to_string(), Document::Object(property))
        })
        .collect();
    let mut schema = HashMap::new();
    schema.insert("type".to_string(), Document::String("object".to_string()));
    schema.insert("properties".to_string(), Document::Object(properties));
    Some(Document::Object(schema))
}

/// A document rendered as one string, for the `map<String, String>` the application
/// summary models: a string is itself, a scalar is its literal text, and a nested array
/// or object is its compact JSON text.
pub fn document_text(value: &Document) -> String {
    match value {
        Document::String(text) => text.clone(),
        other => document_json(other, MAX_DETAILS_DEPTH),
    }
}

/// `value` as compact JSON text, with the levels this walk may still descend.
///
/// Bounded by `budget` so the recursion cannot exhaust the stack on a caller-supplied
/// document; a create or update already refused anything deeper than
/// [`MAX_DETAILS_DEPTH`], so the exhausted arm is unreachable for a stored document.
fn document_json(value: &Document, budget: usize) -> String {
    let Some(next) = budget.checked_sub(1) else {
        return "null".to_string();
    };
    match value {
        Document::Null => "null".to_string(),
        Document::Bool(flag) => flag.to_string(),
        Document::Number(number) => match number {
            Number::PosInt(n) => n.to_string(),
            Number::NegInt(n) => n.to_string(),
            Number::Float(n) => n.to_string(),
        },
        Document::String(text) => {
            // `serde_json` is this crate's JSON writer; a string is the one scalar that
            // needs escaping.
            ::serde_json::Value::String(text.clone()).to_string()
        }
        Document::Array(items) => {
            let rendered: Vec<String> = items
                .iter()
                .map(|item| document_json(item, next))
                .collect();
            format!("[{}]", rendered.join(","))
        }
        Document::Object(members) => {
            // Sorted, so the rendering of one document is stable between reads.
            let mut rendered: Vec<String> = members
                .iter()
                .map(|(name, member)| {
                    format!(
                        "{}:{}",
                        ::serde_json::Value::String(name.clone()),
                        document_json(member, next)
                    )
                })
                .collect();
            rendered.sort();
            format!("{{{}}}", rendered.join(","))
        }
    }
}

/// Why a supplied or amended `BenefitApplicationDetails` cannot be stored, or `None`
/// when it can.
///
/// Three rules, each an emulator resource guard on a member the model leaves entirely
/// caller-controlled: the document must nest no deeper than [`MAX_DETAILS_DEPTH`], hold
/// no more than [`MAX_DETAILS_MEMBERS`] members over the whole tree, and carry no more
/// than [`MAX_DETAILS_BYTES`] of text. Every operation that writes the member applies
/// this — the create and the update to the document they were handed,
/// `AmendBenefitApplication` to the document its amendments produced — each rendering
/// the refusal as its own modelled `ValidationException`.
pub fn details_rejection(details: Option<&Document>) -> Option<String> {
    let document = details?;
    if !document_fits(document, MAX_DETAILS_DEPTH) {
        return Some(format!(
            "BenefitApplicationDetails nests deeper than the {MAX_DETAILS_DEPTH} levels this service accepts"
        ));
    }
    // Counted only after the depth check, so this walk's recursion is already bounded.
    let (members, bytes) = document_weight(document);
    if members > MAX_DETAILS_MEMBERS {
        return Some(format!(
            "BenefitApplicationDetails holds more than the {MAX_DETAILS_MEMBERS} members this service stores"
        ));
    }
    (bytes > MAX_DETAILS_BYTES).then(|| {
        format!(
            "BenefitApplicationDetails carries more than the {MAX_DETAILS_BYTES} bytes of text this service stores"
        )
    })
}

/// How many members and how many bytes of text `value` holds, over the whole tree: the
/// two sizes [`details_rejection`] bounds.
///
/// A member is an object member or an array element; the bytes counted are the member
/// names and the string values, which is all of the document a caller chooses the SIZE
/// of (a number or a boolean has none).
fn document_weight(value: &Document) -> (usize, usize) {
    match value {
        Document::Null | Document::Bool(_) | Document::Number(_) => (0, 0),
        Document::String(text) => (0, text.len()),
        Document::Array(items) => items.iter().fold((0, 0), |(members, bytes), item| {
            let (item_members, item_bytes) = document_weight(item);
            (members + 1 + item_members, bytes + item_bytes)
        }),
        Document::Object(entries) => {
            entries
                .iter()
                .fold((0, 0), |(members, bytes), (name, member)| {
                    let (member_members, member_bytes) = document_weight(member);
                    (
                        members + 1 + member_members,
                        bytes + name.len() + member_bytes,
                    )
                })
        }
    }
}

/// Whether `value` nests no deeper than `budget` levels.
fn document_fits(value: &Document, budget: usize) -> bool {
    let Some(next) = budget.checked_sub(1) else {
        return false;
    };
    match value {
        Document::Array(items) => items.iter().all(|item| document_fits(item, next)),
        Document::Object(members) => members.values().all(|member| document_fits(member, next)),
        _ => true,
    }
}

/// The stored `FileDetail` for one supplied `FileInput`.
///
/// `FileURI` and `BusinessUseCase` are the caller's own values; `FileName` and `FileType`
/// are derived from the URI (see [`file_name_of`] and [`file_type_of`]); `CreatedBy` and
/// `CreatedAt` record who attached the file and when. `FileStatus`/`FileStatusReason`
/// stay absent: nothing in this emulator processes an attachment, so there is no status
/// to report.
pub fn file_detail(input: &FileInput, created_by: &str, now_millis: i64) -> FileDetail {
    FileDetail {
        file_uri: input.file_uri.clone(),
        business_use_case: input.business_use_case.clone(),
        file_name: file_name_of(&input.file_uri),
        file_status: None,
        file_status_reason: None,
        file_type: file_type_of(&input.file_uri),
        created_by: Some(created_by.to_string()),
        created_at: Some(timestamp(now_millis)),
    }
}

/// The last path segment of `uri`, which is the file's name, with any query or fragment
/// removed. `None` when the URI names no segment, or one outside the modelled
/// `FileDetail$FileName` length (1..255).
pub fn file_name_of(uri: &str) -> Option<String> {
    let path = uri
        .split(['?', '#'])
        .next()
        .unwrap_or(uri)
        .trim_end_matches('/');
    let name = path.rsplit('/').next()?;
    (1..=255).contains(&name.len()).then(|| name.to_string())
}

/// The `FileType` the extension of `uri` names, or `None` for an extension
/// [`FILE_TYPES_BY_EXTENSION`] does not list.
pub fn file_type_of(uri: &str) -> Option<String> {
    let name = file_name_of(uri)?;
    let extension = name.rsplit_once('.')?.1.to_ascii_lowercase();
    FILE_TYPES_BY_EXTENSION
        .iter()
        .find(|(candidate, _)| *candidate == extension)
        .map(|(_, file_type)| (*file_type).to_string())
}

/// The id of the benefit named `name` in `catalog`: `ben-` + 14 `[0-9a-z]` characters
/// derived from both, so the id is stable across runs and distinct per catalog.
fn benefit_id(catalog: &str, name: &str) -> String {
    let seed = fnv1a64(format!("{catalog}/{name}").as_bytes());
    format!("{BENEFIT_ID_PREFIX}{}", id_body(seed))
}

/// [`ID_BODY_LEN`] characters drawn from [`ID_ALPHABET`] by mixing `seed`.
fn id_body(seed: u64) -> String {
    let mut state = seed;
    (0..ID_BODY_LEN)
        .map(|_| {
            state = splitmix64(state);
            ID_ALPHABET[(state % ID_ALPHABET.len() as u64) as usize] as char
        })
        .collect()
}

/// FNV-1a, a standard 64-bit non-cryptographic hash. Used only to derive a seeded
/// benefit's id from its name, so it must be STABLE across processes — which rules out
/// `std`'s `DefaultHasher`, whose output is not specified.
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// SplitMix64, a standard 64-bit mixing function: a bijection on `u64`, so distinct
/// inputs always give distinct outputs. Used only to scatter minted identifiers.
fn splitmix64(input: u64) -> u64 {
    let mut z = input.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The current wall-clock instant as epoch milliseconds.
pub fn now_millis() -> i64 {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(since_epoch.as_millis()).unwrap_or(i64::MAX)
}

/// The new `UpdatedAt` for a record last written at `previous`.
///
/// `now` unless the clock has not advanced since `previous`, in which case one
/// millisecond past `previous` — so an update is always observably later than the write
/// before it, which is what a caller comparing two reads relies on.
pub fn bump_millis(previous: i64, now: i64) -> i64 {
    now.max(previous.saturating_add(1))
}

/// Render a stored epoch-millisecond instant as the modelled `date-time` timestamp.
pub fn timestamp(millis: i64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(millis)
}

/// The region this request was signed for, for the ARNs the response carries.
///
/// A signed AWS SDK request names it in the SigV4 credential scope; an unsigned one
/// names none, so [`DEFAULT_REGION`] stands in. The scope is caller-controlled text and
/// the value lands inside an ARN whose modelled pattern allows only
/// `[a-z]{2}(-gov)?-[a-z]+-\d` there, so anything else is refused here and the default
/// used instead: a generated ARN can never carry text the model does not admit.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| is_modelled_region(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Whether `region` is a value the modelled `Arn` pattern admits in its region field:
/// `([a-z]{2}(-gov)?-[a-z]+-\d{1})?`.
fn is_modelled_region(region: &str) -> bool {
    let mut fields: Vec<&str> = region.split('-').collect();
    let Some(digit) = fields.pop() else {
        return false;
    };
    if digit.len() != 1 || !digit.as_bytes()[0].is_ascii_digit() {
        return false;
    }
    let lowercase_alpha =
        |field: &str| !field.is_empty() && field.bytes().all(|b| b.is_ascii_lowercase());
    match fields.as_slice() {
        [prefix, area] => prefix.len() == 2 && lowercase_alpha(prefix) && lowercase_alpha(area),
        [prefix, gov, area] => {
            prefix.len() == 2 && lowercase_alpha(prefix) && *gov == "gov" && lowercase_alpha(area)
        }
        _ => false,
    }
}

/// The principal that attached a file, for `FileDetail$CreatedBy`.
///
/// Derived from the request's own SigV4 access key id, so the member reports who called
/// rather than a fabricated identity. That id is caller-controlled text, so it is first
/// reduced to the characters an access key id can hold; the rendering then satisfies the
/// member's modelled 1..255 length by construction, whatever the header said.
pub fn caller_principal(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    let raw = ::engine::extract_akid(&ctx.parts().headers);
    let filtered: String = raw
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(64)
        .collect();
    let akid = if filtered.is_empty() {
        "default"
    } else {
        filtered.as_str()
    };
    ::engine::emulator_arn("iam", "", &format!("user/{akid}"))
}

/// The ARN of the benefit application `id` of `catalog`, in `region`.
pub fn application_arn(region: &str, catalog: &str, id: &str) -> String {
    resource_arn(region, catalog, APPLICATION_RESOURCE, id)
}

/// The ARN of the benefit `id` of `catalog`, in `region`.
pub fn benefit_arn(region: &str, catalog: &str, id: &str) -> String {
    resource_arn(region, catalog, BENEFIT_RESOURCE, id)
}

/// `arn:aws:partnercentral:{region}:{account}:catalog/{catalog}/{resource}/{id}`.
///
/// The `catalog/...` resource shape is the one the modelled
/// `ListTagsForResource$resourceArn` pattern declares for this service
/// (`^arn:[\w+=/,.@-]+:partnercentral:[\w+=/,.@-]*:[0-9]{12}:catalog/([a-zA-Z]+)/[\w+=,.@-]+(/[\w+=,.@-]+)*$`),
/// so one ARN form serves both the resource members and the tagging operations.
fn resource_arn(region: &str, catalog: &str, resource: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("catalog/{catalog}/{resource}/{id}"),
    )
}

/// The `(catalog, id)` an ARN of this service's `resource` type names, or `None` when
/// `arn` is not one.
pub fn parse_resource_arn<'a>(arn: &'a str, resource: &str) -> Option<(&'a str, &'a str)> {
    let mut fields = arn.splitn(6, ':');
    let (prefix, _partition, service, _region, _account, tail) = (
        fields.next()?,
        fields.next()?,
        fields.next()?,
        fields.next()?,
        fields.next()?,
        fields.next()?,
    );
    if prefix != "arn" || service != ARN_SERVICE {
        return None;
    }
    let mut segments = tail.split('/');
    if segments.next()? != "catalog" {
        return None;
    }
    let catalog = segments.next()?;
    if segments.next()? != resource {
        return None;
    }
    let id = segments.next()?;
    (segments.next().is_none()).then_some((catalog, id))
}

/// AWS's "not found" message: the resource that was looked up, then the identifier the
/// request named it with, backtick-quoted — e.g. ``Catalog `Sandbox` not found``.
///
/// One construction for every lookup this service does, so a client reading the message
/// learns WHICH resource was missing from the one place the sentence is written.
/// `resource` is [`CATALOG_KIND`], [`BENEFIT_KIND`] or [`APPLICATION_KIND`].
pub fn not_found_message(resource: &str, identifier: &str) -> String {
    format!("{resource} `{identifier}` not found")
}

/// The message a request refused because of the application's STATUS carries: the
/// application, the status it is in, and the thing that status does not allow — e.g.
/// ``benefit application benappl-… is IN_REVIEW, so it cannot be updated``.
///
/// One construction for every status refusal this service makes (`submitted`, `recalled`,
/// `updated`, `amended`, `canceled`), so a client reading a `ConflictException` learns
/// the current status from any of them in the same words. `action` is the past participle
/// of the refused change.
pub fn status_conflict_message(id: &str, status: &str, action: &str) -> String {
    format!("benefit application {id} is {status}, so it cannot be {action}")
}

/// The entitlement gate: `Some(..)` when the calling account holds no active AWS partner
/// benefit, and the request must be refused with the modelled `AccessDeniedException`
/// (served with [`ACCESS_DENIED_STATUS`]) instead of being served.
///
/// Every operation applies this BEFORE it looks any resource up, so an un-entitled
/// caller is told it is not entitled rather than whether a resource exists; the
/// modelled constraints are refused earlier still, by the centralized request walk, so
/// a malformed request is a `ValidationException` whether the caller is entitled or not.
///
/// The two tag-MUTATING operations (`TagResource`, `UntagResource`) are the one recorded
/// exception and do not apply it — see `crate::ops::tag_resource`.
///
/// The emulator presents exactly one account (`engine::EMULATOR_ACCOUNT_ID`), so that is
/// the account every request is made from and the only one the gate can be asked about.
/// A store that cannot be reached answers `None` — unknown is not "denied", and the
/// handler's own store check turns it into the modelled server error.
pub fn entitlement_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<crate::ops::types::AccessDeniedException> {
    let state = service_state(engine)?;
    let entitled = state
        .lock()
        .has_active_partner_benefit(::engine::emulator_account_id());
    (!entitled).then(|| crate::ops::types::AccessDeniedException {
        message: PARTNER_ENTITLEMENT_REQUIRED.to_string(),
    })
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsPartnercentralbenefitsPartnercentralbenefitsserviceState>>
{
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsPartnercentralbenefitsPartnercentralbenefitsserviceState>)
}

/// The tags a create supplied, as the store keeps them.
pub fn tag_map(tags: Option<&Vec<Tag>>) -> BTreeMap<String, String> {
    tags.map(|tags| {
        tags.iter()
            .map(|tag| (tag.key.clone(), tag.value.clone()))
            .collect()
    })
    .unwrap_or_default()
}

/// The tag set `additions` leaves behind when merged onto `current`: a key `additions`
/// carries takes its value there (`TagResource` "adds or updates tags"), and every other
/// key keeps the value it had.
pub fn merged_tags(
    current: &BTreeMap<String, String>,
    additions: &[Tag],
) -> BTreeMap<String, String> {
    let mut merged = current.clone();
    for tag in additions {
        merged.insert(tag.key.clone(), tag.value.clone());
    }
    merged
}

/// A stored tag set as the modelled `Tags` list, in tag-key order.
///
/// A resource carrying no tags reports an EMPTY list rather than omitting the member: the
/// listing reports the set it found, and an empty set is a fact about the resource.
pub fn tag_list(tags: &BTreeMap<String, String>) -> Vec<Tag> {
    tags.iter()
        .map(|(key, value)| Tag {
            key: key.clone(),
            value: value.clone(),
        })
        .collect()
}

/// The `ResourceNotFoundException` message a failed catalog lookup carries, built from
/// the `Catalog` the request named — the one construction [`not_found_message`] writes.
///
/// `None` is the tagging operations' case — they bind no `Catalog` member — and renders
/// as [`CATALOG_ABSENT`], which is what the service reports for a tagging call whose
/// `resourceArn` resolves to no resource: recorded behaviour (corpus
/// `simparity-behavioral-aws-v1`, units
/// `partnercentral-benefits.tag-resource-malformed-arn-rejected` and
/// `partnercentral-benefits.untag-resource-malformed-arn-rejected`), where a well-formed
/// tagging call naming a resource that does not exist is answered ``(
/// "ResourceNotFoundException", "Catalog `null` not found")``. The absent catalog is the
/// SERVICE's own wording for a path that carries no catalog, not an unset emulator value
/// rendered by accident.
pub fn catalog_not_found_message(catalog: Option<&str>) -> String {
    not_found_message(CATALOG_KIND, catalog.unwrap_or(CATALOG_ABSENT))
}

/// Whether `values` carries at least one entry of `filter`. An empty or absent filter
/// matches everything, which is what an omitted list filter means on the wire.
pub fn matches_any(filter: Option<&Vec<String>>, values: &[String]) -> bool {
    match filter {
        None => true,
        Some(filter) if filter.is_empty() => true,
        Some(filter) => filter.iter().any(|wanted| values.contains(wanted)),
    }
}

/// The `ValidationExceptionReason` a refusal of one input member carries; a value of the
/// model's own `ValidationExceptionReason` enum (see
/// `crate::ops::types::VALIDATION_EXCEPTION_REASON_VALUES`).
pub const VALIDATION_REASON_FIELD: &str = "fieldValidationFailed";

/// The `ValidationExceptionField$Code` for a member whose value is not one this service
/// accepts; a value of the model's own `ValidationExceptionErrorCode` enum (see
/// `crate::ops::types::VALIDATION_EXCEPTION_ERROR_CODE_VALUES`).
pub const VALIDATION_CODE_INVALID_VALUE: &str = "INVALID_VALUE";

/// A one-member `FieldList` for a `ValidationException`, so a client can see WHICH
/// member it must fix rather than only the message.
pub fn validation_field(
    name: &str,
    message: &str,
    code: &str,
) -> Vec<crate::ops::types::ValidationExceptionField> {
    vec![crate::ops::types::ValidationExceptionField {
        name: name.to_string(),
        message: message.to_string(),
        code: Some(code.to_string()),
    }]
}

/// The page size a list operation uses, from the `MaxResults` the request asked for.
///
/// `MaxResults` is modelled `range(1..100)`, so the centralized walk is the enforcement
/// point and has already refused anything outside it; the modelled maximum stands in when
/// the request names none. The clamp is belt-and-braces for that unreachable case, so a
/// page can never be zero-sized (an empty page carrying a `NextToken` would make a paging
/// caller loop forever).
pub fn page_size(max_results: Option<i32>) -> usize {
    let requested = max_results.unwrap_or(MAX_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE);
    usize::try_from(requested).unwrap_or(1)
}

/// A pagination token naming the record the NEXT page starts at.
///
/// The modelled `NextToken` is opaque, so the id is hex-encoded rather than handed back
/// in the clear — opaque to a caller, not secret: hex is trivially reversible and the
/// token carries nothing beyond a record id. The characters it produces sit well inside
/// the member's modelled 1..2048 length.
pub fn encode_page_token(id: &str) -> String {
    ::hex::encode(id)
}

/// The record id a pagination token names, or `None` when the token is not one this
/// service issued for a record of `prefix`'s kind.
pub fn decode_page_token(token: &str, prefix: &str) -> Option<String> {
    let decoded = ::hex::decode(token)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())?;
    let body = decoded.strip_prefix(prefix)?;
    (body.len() == ID_BODY_LEN
        && body
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()))
    .then_some(decoded)
}
