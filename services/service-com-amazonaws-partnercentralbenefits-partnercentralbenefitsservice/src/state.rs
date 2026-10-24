// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice service.
//!
//! The service has two resources in this slice:
//!
//! * the **benefit catalog** — service-owned, read-only data. The API models no
//!   `CreateBenefit`, so the catalogs and the benefits inside them are seeded here, at
//!   state init, and `ListBenefits` reports them from the store like any other read. The
//!   **benefit allocations** AWS has granted are service-owned in the same way (the API
//!   models no `CreateBenefitAllocation`), so they are seeded beside the benefits they
//!   draw from and `ListBenefitAllocations` reports them from the store.
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

/// The name a `ResourceNotFoundException` message gives a benefit allocation: the
/// modelled resource shape `com.amazonaws.partnercentralbenefits#BenefitAllocation`
/// (`smithy_query(query="shape",shape="BenefitAllocation")`).
pub const ALLOCATION_KIND: &str = "BenefitAllocation";

/// The prefix a benefit id carries, from the modelled `BenefitSummary$Id` pattern
/// `^ben-[0-9a-z]{14}$`.
pub const BENEFIT_ID_PREFIX: &str = "ben-";

/// The prefix a benefit-application id carries, from the modelled
/// `BenefitApplicationId` pattern `^benappl-[0-9a-z]{14}$`.
pub const APPLICATION_ID_PREFIX: &str = "benappl-";

/// The prefix a benefit-allocation id carries, from the modelled `BenefitAllocationId`
/// pattern `^benalloc-[0-9a-z]{14}$`.
pub const ALLOCATION_ID_PREFIX: &str = "benalloc-";

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

/// The `FulfillmentType` of a benefit fulfilled by paying the partner money; one of the
/// three values the model declares for that enum (`CREDITS | CASH | ACCESS`, see
/// `crate::ops::types::FULFILLMENT_TYPE_VALUES`).
pub const FULFILLMENT_TYPE_CASH: &str = "CASH";
/// The `FulfillmentType` of a benefit fulfilled with AWS promotional credits.
pub const FULFILLMENT_TYPE_CREDITS: &str = "CREDITS";
/// The `FulfillmentType` of a benefit fulfilled by granting access rather than value.
pub const FULFILLMENT_TYPE_ACCESS: &str = "ACCESS";

/// The `BenefitAllocationStatus` of an allocation inside its term and not yet consumed;
/// one of the three values the model declares for that enum (`ACTIVE | INACTIVE |
/// FULFILLED`, see `crate::ops::types::BENEFIT_ALLOCATION_STATUS_VALUES`).
pub const ALLOCATION_STATUS_ACTIVE: &str = "ACTIVE";
/// The `BenefitAllocationStatus` of an allocation that is no longer usable — the status
/// an expiry leaves behind (see [`BenefitAllocation::expire_due`]).
pub const ALLOCATION_STATUS_INACTIVE: &str = "INACTIVE";
/// The `BenefitAllocationStatus` of an allocation whose benefit has been delivered in
/// full. Terminal: an expiry does not move it, because there is nothing left to expire.
pub const ALLOCATION_STATUS_FULFILLED: &str = "FULFILLED";

/// The `CurrencyCode` every monetary amount this service holds is denominated in; a value
/// of the model's own `CurrencyCode` enum (see `crate::ops::types::CURRENCY_CODE_VALUES`).
///
/// AWS partner funding benefits are granted in US dollars and no input member carries a
/// currency, so this is the one currency the store uses — and the one whose minor unit is
/// [`MINOR_UNITS_PER_UNIT`].
pub const ALLOCATION_CURRENCY: &str = "USD";

/// Minor units in one unit of [`ALLOCATION_CURRENCY`]: USD is an ISO-4217 currency with two
/// decimal digits, so an amount is stored as cents and rendered with two decimals (see
/// [`Money::amount_text`]).
pub const MINOR_UNITS_PER_UNIT: i64 = 100;

/// The amount a granted allocation is created with, in minor units of
/// [`ALLOCATION_CURRENCY`].
///
/// CHOSEN, not an AWS amount: AWS decides what it allocates when it fulfills a benefit and
/// this API models no member that carries the amount, so a grant needs a figure and this is
/// it. Nothing is issued against a fresh grant — the issuance is a later act of AWS's
/// fulfillment workflow — so a granted allocation reports this as allocated and zero as
/// issued.
pub const GRANTED_ALLOCATION_AMOUNT_MINOR: i64 = 25_000 * MINOR_UNITS_PER_UNIT;

/// The `StatusReason` an expired allocation reports (`BenefitAllocationSummary$StatusReason`:
/// "additional information explaining the current status of the benefit allocation").
///
/// AWS publishes no wording for this member, so the sentence is the emulator's own; what
/// is not chosen is the FACT it reports — the allocation's `ExpiresAt` has passed, which
/// the model documents as the instant it "expires and is no longer usable".
pub const ALLOCATION_EXPIRED_REASON: &str =
    "The allocation's term ended at its ExpiresAt and it is no longer usable.";

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

/// The most benefit allocations this emulator holds in one catalog.
///
/// The catalog is seeded with [`SEEDED_ALLOCATIONS`] and grown only by
/// [`ComAmazonawsPartnercentralbenefitsPartnercentralbenefitsserviceState::grant_allocation`],
/// which no request from the wire reaches, so this is a store guard rather than a quota a
/// caller can push against. CHOSEN, like [`MAX_APPLICATIONS_PER_CATALOG`]: AWS publishes
/// no quota for allocations per partner.
pub const MAX_ALLOCATIONS_PER_CATALOG: usize = 500;

/// Milliseconds in a day, the unit the seeded allocation terms below are written in.
const MILLIS_PER_DAY: i64 = 24 * 60 * 60 * 1000;

/// The term a granted benefit allocation runs for, from `StartsAt` to `ExpiresAt`.
///
/// CHOSEN, not an AWS duration: AWS documents `ExpiresAt` ("the timestamp when the benefit
/// allocation expires and is no longer usable") but publishes no default term for an
/// allocation, and no input member carries one. A year is the term AWS partner funding
/// benefits are granted for in practice, and keeps a granted allocation inside its window
/// for the whole life of an emulator process.
pub const ALLOCATION_TERM_MILLIS: i64 = 365 * MILLIS_PER_DAY;

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

/// One benefit allocation the service has granted, before its per-catalog ids and its
/// timestamps are derived.
struct SeededAllocation {
    /// The `name` of the [`SEEDED_BENEFITS`] entry this allocation is based on; the
    /// allocation's `BenefitId`, fulfillment types, description and applicable benefits
    /// all come from that benefit.
    benefit_name: &'static str,
    /// `BenefitAllocationSummary$Name` (modelled length 1..255).
    name: &'static str,
    /// The `BenefitAllocationStatus` the allocation was GRANTED with. The term below may
    /// have closed since, in which case the first read materialises the expiry the clock
    /// already made due — see [`BenefitAllocation::expire_due`].
    status: &'static str,
    /// `BenefitAllocationSummary$StatusReason`, where the status has one to explain.
    status_reason: Option<&'static str>,
    /// The one `FulfillmentType` this allocation is fulfilled with, which must be one of
    /// the types its benefit offers (a benefit offering two is fulfilled with one of them).
    /// It is what decides which member of the modelled `FulfillmentDetails` union
    /// `GetBenefitAllocation` reports — see [`AllocationFulfillment`].
    fulfillment_type: &'static str,
    /// What AWS allocated, in minor units of [`ALLOCATION_CURRENCY`]; zero for an `ACCESS`
    /// allocation, which fulfills no money.
    allocated_amount_minor: i64,
    /// How much of [`Self::allocated_amount_minor`] has been issued (disbursed, or handed
    /// over as credit codes) as of state init. Never more than what was allocated.
    issued_amount_minor: i64,
    /// How long before state init the allocation was created, in days.
    created_days_ago: i64,
    /// The term, in days, from `StartsAt` (= creation) to `ExpiresAt`.
    term_days: i64,
}

/// The benefit allocations every catalog is seeded with.
///
/// The allocation is service-owned data like the benefit catalog it draws from: AWS grants
/// one when it fulfills a partner's benefit, and this API models no operation that creates
/// one, so the set is seeded here and every read reports it from the store.
///
/// Each `status` is a value of the model's `BenefitAllocationStatus` enum
/// (`crate::ops::types::BENEFIT_ALLOCATION_STATUS_VALUES`, i.e. `ACTIVE | INACTIVE |
/// FULFILLED`), each `fulfillment_type` a value of its `FulfillmentType` enum
/// (`crate::ops::types::FULFILLMENT_TYPE_VALUES`) that the named benefit offers, and each
/// `benefit_name` names a row of [`SEEDED_BENEFITS`]. The set deliberately spans every
/// `FulfillmentType` (`CASH`, `CREDITS`, `ACCESS`, and a benefit carrying two of them,
/// fulfilled with the second), an allocation inside its term, one already fulfilled, one
/// whose term closed before state init, an allocation partly issued and one with nothing
/// issued yet — so a filtered `ListBenefitAllocations` has something to include and
/// something to exclude whichever filter it is given, and `GetBenefitAllocation` has one
/// allocation per member of the modelled `FulfillmentDetails` union to report.
const SEEDED_ALLOCATIONS: &[SeededAllocation] = &[
    SeededAllocation {
        benefit_name: "Marketing Development Funds",
        name: "Marketing Development Funds allocation",
        status: ALLOCATION_STATUS_ACTIVE,
        status_reason: None,
        fulfillment_type: FULFILLMENT_TYPE_CASH,
        allocated_amount_minor: 25_000 * MINOR_UNITS_PER_UNIT,
        // Part of the funding has been disbursed, so this allocation carries an issuance.
        issued_amount_minor: 10_000 * MINOR_UNITS_PER_UNIT,
        created_days_ago: 30,
        term_days: 365,
    },
    SeededAllocation {
        benefit_name: "Promotional Credits",
        name: "Promotional Credits allocation",
        status: ALLOCATION_STATUS_FULFILLED,
        status_reason: Some("The allocated credits were issued in full."),
        fulfillment_type: FULFILLMENT_TYPE_CREDITS,
        allocated_amount_minor: 5_000 * MINOR_UNITS_PER_UNIT,
        // Issued in full, which is what `FULFILLED` says: the credit code carries all of it.
        issued_amount_minor: 5_000 * MINOR_UNITS_PER_UNIT,
        created_days_ago: 60,
        term_days: 365,
    },
    SeededAllocation {
        benefit_name: "Partner Training Access",
        name: "Partner Training Access allocation",
        status: ALLOCATION_STATUS_ACTIVE,
        status_reason: None,
        fulfillment_type: FULFILLMENT_TYPE_ACCESS,
        // An `ACCESS` allocation fulfills a privilege, not an amount.
        allocated_amount_minor: 0,
        issued_amount_minor: 0,
        // Granted more than a term ago, so its `ExpiresAt` is already past: the expiry is
        // what moves it to `INACTIVE`, rather than a status seeded as already moved.
        created_days_ago: 400,
        term_days: 365,
    },
    SeededAllocation {
        benefit_name: "Migration Acceleration Funding",
        name: "Migration Acceleration Funding allocation",
        status: ALLOCATION_STATUS_ACTIVE,
        status_reason: None,
        // The benefit offers `CASH` and `CREDITS`; this allocation is fulfilled with the
        // second of them, so the type is the allocation's own choice rather than the
        // benefit's first.
        fulfillment_type: FULFILLMENT_TYPE_CREDITS,
        allocated_amount_minor: 10_000 * MINOR_UNITS_PER_UNIT,
        // Nothing issued yet, so the credit-code list is empty.
        issued_amount_minor: 0,
        created_days_ago: 10,
        term_days: 365,
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

/// A monetary amount as the store holds it: minor units of its currency, so an amount is
/// an integer in the store and is rendered into the modelled `MonetaryValue$Amount` once,
/// on the way out (see [`Self::amount_text`]).
#[derive(Debug, Clone)]
pub struct Money {
    /// The amount in minor units (cents, for [`ALLOCATION_CURRENCY`]). Never negative:
    /// [`Self::new`] is the only constructor and clamps at zero.
    minor_units: i64,
    /// A `CurrencyCode` value.
    pub currency_code: String,
}

impl Money {
    /// `minor_units` of [`ALLOCATION_CURRENCY`]. A negative figure is not an amount this
    /// service allocates, so it clamps to zero rather than rendering a signed amount.
    pub fn new(minor_units: i64) -> Self {
        Self {
            minor_units: minor_units.max(0),
            currency_code: ALLOCATION_CURRENCY.to_string(),
        }
    }

    /// This amount in minor units of [`Self::currency_code`].
    pub fn minor_units(&self) -> i64 {
        self.minor_units
    }

    /// The modelled `MonetaryValue$Amount` text: the units, then the currency's minor units
    /// as its decimal digits (`"25000.00"` for 2_500_000 cents of USD).
    pub fn amount_text(&self) -> String {
        let units = self.minor_units / MINOR_UNITS_PER_UNIT;
        let fraction = self.minor_units % MINOR_UNITS_PER_UNIT;
        format!("{units}.{fraction:02}")
    }

    /// This amount as the modelled `MonetaryValue`.
    pub fn to_monetary_value(&self) -> crate::ops::types::MonetaryValue {
        crate::ops::types::MonetaryValue {
            amount: self.amount_text(),
            currency_code: self.currency_code.clone(),
        }
    }
}

/// One issuance of value against a benefit allocation: the record the modelled
/// `IssuanceDetail` reports.
#[derive(Debug, Clone)]
pub struct Issuance {
    /// The issuance's own identifier, derived from the allocation's id so it is stable
    /// across reads and distinct per allocation (see [`issuance_id`]).
    pub id: String,
    /// What this issuance handed over.
    pub amount: Money,
    /// When it was processed.
    pub issued_at_millis: i64,
}

/// One AWS promotional credit code issued against a benefit allocation: the record the
/// modelled `CreditCode` reports.
#[derive(Debug, Clone)]
pub struct AllocatedCreditCode {
    /// The AWS account the code can be applied to: the one account this emulator presents.
    pub aws_account_id: String,
    /// The code itself, derived from the allocation's id and this code's index in the
    /// allocation (see [`credit_code_text`]).
    pub code: String,
    /// The code's value.
    pub value: Money,
    /// A `BenefitAllocationStatus` value: `ACTIVE` while the code can still be redeemed,
    /// `FULFILLED` once it has been, `INACTIVE` once it has expired.
    pub status: String,
    pub issued_at_millis: i64,
    /// When the code expires and can no longer be redeemed.
    pub expires_at_millis: i64,
}

impl AllocatedCreditCode {
    /// Materialise the expiry the clock has already made due on this code, exactly as
    /// [`BenefitAllocation::expire_due`] does for the allocation: a code past its own
    /// `ExpiresAt` can no longer be redeemed, so it is `INACTIVE` whether or not anybody
    /// has looked at it. A redeemed (`FULFILLED`) code is terminal.
    fn expire_due(&mut self, now_millis: i64) -> bool {
        if self.status != ALLOCATION_STATUS_ACTIVE || now_millis < self.expires_at_millis {
            return false;
        }
        self.status = ALLOCATION_STATUS_INACTIVE.to_string();
        true
    }
}

/// How a benefit allocation is fulfilled, as the store holds it.
///
/// The allocation's `FulfillmentType` decides the shape of this record, which is what the
/// modelled `FulfillmentDetails` union is: one member per kind of fulfillment
/// (`smithy_query(query="shape",shape="FulfillmentDetails")`). `CASH` is fulfilled by
/// disbursing money (`DisbursementDetails`, "direct payments or reimbursements"), `CREDITS`
/// by issuing AWS promotional credits (`CreditDetails`), and `ACCESS` by granting a
/// privilege (`AccessDetails`, "service permissions or feature access"), whose description
/// is the allocation's own — so the variant carries nothing.
///
/// The union's fourth member, `ConsumableDetails` ("usage quotas or service limits"), is
/// produced by no `FulfillmentType` the model declares, so nothing in this store holds one;
/// a fulfillment type with no record here leaves `FulfillmentDetail` absent rather than
/// reporting a kind of fulfillment that did not happen.
#[derive(Debug, Clone)]
pub enum AllocationFulfillment {
    /// `CASH`: what has been disbursed so far, and the issuance that disbursed it (absent
    /// while nothing has been).
    Disbursement {
        disbursed: Money,
        issuance: Option<Issuance>,
    },
    /// `CREDITS`: what was allocated, what has been issued as credit codes, and those
    /// codes (empty while nothing has been issued).
    Credits {
        allocated: Money,
        issued: Money,
        codes: Vec<AllocatedCreditCode>,
    },
    /// `ACCESS`: the privilege itself, described by the allocation's `Description`.
    Access,
}

impl AllocationFulfillment {
    /// The fulfillment record of an allocation of `fulfillment_type` that was granted
    /// `allocated_minor` and has had `issued_minor` of it issued, or `None` for a
    /// fulfillment type this store holds no record shape for.
    ///
    /// `allocation_id` seeds the issuance id and the credit codes, and the allocation's own
    /// `issued_at`/`expires_at` are what an issuance and a code carry: a code expires with
    /// the allocation it was issued against.
    fn new(
        fulfillment_type: &str,
        allocated_minor: i64,
        issued_minor: i64,
        allocation_id: &str,
        issued_at_millis: i64,
        expires_at_millis: i64,
    ) -> Option<Self> {
        let issued = Money::new(issued_minor.clamp(0, allocated_minor.max(0)));
        match fulfillment_type {
            FULFILLMENT_TYPE_CASH => Some(Self::Disbursement {
                issuance: (issued.minor_units() > 0).then(|| Issuance {
                    id: issuance_id(allocation_id),
                    amount: issued.clone(),
                    issued_at_millis,
                }),
                disbursed: issued,
            }),
            FULFILLMENT_TYPE_CREDITS => Some(Self::Credits {
                allocated: Money::new(allocated_minor),
                // One issuance, so one code carrying what has been issued; nothing issued
                // means no codes rather than a zero-valued one.
                codes: Vec::from_iter((issued.minor_units() > 0).then(|| AllocatedCreditCode {
                    aws_account_id: ::engine::emulator_account_id().to_string(),
                    code: credit_code_text(allocation_id, 0),
                    value: issued.clone(),
                    // A code is handed over REDEEMABLE: nothing in this emulator
                    // redeems one, and its own expiry is what moves it to
                    // `INACTIVE` (see [`AllocatedCreditCode::expire_due`]).
                    status: ALLOCATION_STATUS_ACTIVE.to_string(),
                    issued_at_millis,
                    expires_at_millis,
                })),
                issued,
            }),
            FULFILLMENT_TYPE_ACCESS => Some(Self::Access),
            _ => None,
        }
    }
}

/// One benefit allocation the service has granted: the record every allocation operation
/// reads.
///
/// Granted by AWS when it fulfills a benefit for a partner — this API models no operation
/// that creates one — so the record arrives from [`SEEDED_ALLOCATIONS`] or from
/// [`ComAmazonawsPartnercentralbenefitsPartnercentralbenefitsserviceState::grant_allocation`],
/// and every operation reports it from here. The ARN is NOT stored: it carries the region
/// the request was signed for, so it is built per read with [`allocation_arn`], exactly as
/// a benefit's is.
#[derive(Debug, Clone)]
pub struct BenefitAllocation {
    /// `benalloc-` + 14 `[0-9a-z]` characters, from the modelled `BenefitAllocationId`.
    pub id: String,
    pub catalog: String,
    /// The `ben-...` id of the benefit this allocation is based on.
    pub benefit_id: String,
    /// The `benappl-...` id of the application that resulted in this allocation, where one
    /// did. Absent on an allocation AWS granted directly, which is what every seeded
    /// allocation is: the model makes the member optional for exactly that case.
    pub benefit_application_id: Option<String>,
    pub name: String,
    pub description: Option<String>,
    /// A `BenefitAllocationStatus` value.
    pub status: String,
    pub status_reason: Option<String>,
    /// `FulfillmentType` values: the ones the benefit this allocation is based on offers.
    pub fulfillment_types: Vec<String>,
    /// The one `FulfillmentType` this allocation is fulfilled with, which is always one of
    /// [`Self::fulfillment_types`]: a benefit may offer two, and the allocation is fulfilled
    /// with one of them.
    pub fulfillment_type: String,
    /// How this allocation is fulfilled: the amounts, issuances and credit codes behind
    /// [`Self::fulfillment_type`]. `None` for a fulfillment type this store holds no record
    /// shape for — see [`AllocationFulfillment`].
    pub fulfillment: Option<AllocationFulfillment>,
    /// The `ben-...` ids of the benefits this allocation can be applied to.
    pub applicable_benefit_ids: Vec<String>,
    pub created_at_millis: i64,
    pub updated_at_millis: i64,
    /// When the allocation becomes active and usable.
    pub starts_at_millis: i64,
    /// When the allocation expires and is no longer usable.
    pub expires_at_millis: i64,
}

impl BenefitAllocation {
    /// Whether this allocation's term has closed at `now_millis` — `ExpiresAt` is the
    /// instant the model documents it "expires and is no longer usable" at, so the term is
    /// closed from that instant on.
    pub fn has_expired(&self, now_millis: i64) -> bool {
        now_millis >= self.expires_at_millis
    }

    /// Materialise the expiry the clock has already made due, and report whether this call
    /// is what materialised it.
    ///
    /// An `ACTIVE` allocation past its `ExpiresAt` is no longer usable, so it is
    /// `INACTIVE` whether or not anybody has looked at it: the change was owed at that
    /// instant, which is why a reporting operation may apply it. `FULFILLED` is terminal —
    /// the benefit was delivered, so there is nothing left to expire — and an allocation
    /// already `INACTIVE` has nowhere to move. Each credit code the allocation has issued
    /// expires on the same rule, against its own `ExpiresAt` (see
    /// [`AllocatedCreditCode::expire_due`]).
    pub fn expire_due(&mut self, now_millis: i64) -> bool {
        let mut changed = false;
        if self.status == ALLOCATION_STATUS_ACTIVE && self.has_expired(now_millis) {
            self.status = ALLOCATION_STATUS_INACTIVE.to_string();
            self.status_reason = Some(ALLOCATION_EXPIRED_REASON.to_string());
            changed = true;
        }
        if let Some(AllocationFulfillment::Credits { codes, .. }) = self.fulfillment.as_mut() {
            for code in codes.iter_mut() {
                changed |= code.expire_due(now_millis);
            }
        }
        if !changed {
            return false;
        }
        self.updated_at_millis = bump_millis(self.updated_at_millis, now_millis);
        true
    }
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

    /// A list member, omitted from the wire when nothing was ever stored for it. The one
    /// implementation is the free [`optional_list`], which the allocation summaries reach
    /// directly.
    pub fn optional_list<T: Clone>(values: &[T]) -> Option<Vec<T>> {
        optional_list(values)
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

/// One benefit catalog: its seeded benefits and allocations, the applications created in
/// it, and the `ClientToken`s those creates were made with.
#[derive(Debug, Clone, Default)]
pub struct Catalog {
    /// Benefits by `ben-...` id, in one stable order so `ListBenefits` can page.
    pub benefits: BTreeMap<String, Benefit>,
    /// Applications by `benappl-...` id, in one stable order so
    /// `ListBenefitApplications` can page.
    pub applications: BTreeMap<String, BenefitApplication>,
    /// Allocations by `benalloc-...` id, in one stable order so
    /// `ListBenefitAllocations` can page.
    pub allocations: BTreeMap<String, BenefitAllocation>,
    /// `ClientToken` -> the application id the create it completed minted.
    pub client_tokens: BTreeMap<String, String>,
}

impl Catalog {
    /// The catalog named `catalog`, with [`SEEDED_BENEFITS`] and the
    /// [`SEEDED_ALLOCATIONS`] drawn from them in it, as of `seeded_at_millis`.
    fn seeded(catalog: &str, seeded_at_millis: i64) -> Self {
        let benefits: BTreeMap<String, Benefit> = SEEDED_BENEFITS
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
        // Each seeded allocation names the benefit it is based on, so a row whose benefit
        // is not in the catalog — or one naming a fulfillment type that benefit does not
        // offer — seeds nothing rather than an allocation no read can make sense of.
        let allocations = SEEDED_ALLOCATIONS
            .iter()
            .filter_map(|seed| {
                let benefit = benefits
                    .values()
                    .find(|benefit| benefit.name == seed.benefit_name)?;
                if !benefit
                    .fulfillment_types
                    .iter()
                    .any(|offered| offered == seed.fulfillment_type)
                {
                    return None;
                }
                let created_at_millis =
                    seeded_at_millis - seed.created_days_ago * MILLIS_PER_DAY;
                let expires_at_millis = created_at_millis + seed.term_days * MILLIS_PER_DAY;
                let id = allocation_id(catalog, seed.name);
                let allocation = BenefitAllocation {
                    id: id.clone(),
                    catalog: catalog.to_string(),
                    benefit_id: benefit.id.clone(),
                    // Seeded allocations are granted directly, not through an
                    // application, so they carry no `BenefitApplicationId`.
                    benefit_application_id: None,
                    name: seed.name.to_string(),
                    description: Some(benefit.description.clone()),
                    status: seed.status.to_string(),
                    status_reason: seed.status_reason.map(str::to_string),
                    fulfillment_types: benefit.fulfillment_types.clone(),
                    fulfillment_type: seed.fulfillment_type.to_string(),
                    fulfillment: AllocationFulfillment::new(
                        seed.fulfillment_type,
                        seed.allocated_amount_minor,
                        seed.issued_amount_minor,
                        &id,
                        created_at_millis,
                        expires_at_millis,
                    ),
                    applicable_benefit_ids: applicable_benefit_ids(&benefits, benefit),
                    created_at_millis,
                    // Nothing has changed a seeded allocation since it was granted; an
                    // expiry that falls due is what moves this on.
                    updated_at_millis: created_at_millis,
                    starts_at_millis: created_at_millis,
                    expires_at_millis,
                };
                Some((id, allocation))
            })
            .collect();
        Self {
            benefits,
            applications: BTreeMap::new(),
            allocations,
            client_tokens: BTreeMap::new(),
        }
    }

    /// The id of the benefit `identifier` names, accepting either form the modelled
    /// `BenefitId` pattern allows (`^(arn:.+|ben-[0-9a-z]{14})$`): the bare id, or a
    /// benefit ARN of this catalog. `None` when this catalog holds no such benefit.
    pub fn resolve_benefit_id(&self, catalog: &str, identifier: &str) -> Option<String> {
        let id = resolve_identifier(catalog, identifier, BENEFIT_RESOURCE)?;
        self.benefits.contains_key(&id).then_some(id)
    }

    /// The id of the benefit allocation `identifier` names, accepting either form the
    /// modelled `BenefitAllocationIdentifier` pattern allows
    /// (`^(arn:.+|benalloc-[0-9a-z]{14})$`): the bare id, or an allocation ARN of this
    /// catalog. `None` when this catalog holds no such allocation — which is also the
    /// answer for an ARN of another catalog, another service or another resource type.
    pub fn resolve_allocation_id(&self, catalog: &str, identifier: &str) -> Option<String> {
        let id = resolve_identifier(catalog, identifier, ALLOCATION_RESOURCE)?;
        self.allocations.contains_key(&id).then_some(id)
    }
}

/// The resource id `identifier` names inside `catalog`, for a resource of `resource`'s
/// type: `identifier` itself when it is a bare id, or the ARN's id segment when it is an
/// ARN of that resource type IN that catalog.
///
/// One construction for every identifier member whose modelled pattern admits both forms
/// (`^(arn:.+|<prefix>-[0-9a-z]{14})$`), so an ARN naming another catalog, another service
/// or another resource type resolves to nothing rather than to the record that happens to
/// share its id segment. Whether a record of that id EXISTS is the caller's own check.
fn resolve_identifier(catalog: &str, identifier: &str, resource: &str) -> Option<String> {
    if !identifier.starts_with("arn:") {
        return Some(identifier.to_string());
    }
    let (arn_catalog, arn_id) = parse_resource_arn(identifier, resource)?;
    (arn_catalog == catalog).then(|| arn_id.to_string())
}

/// The `ben-...` ids of the benefits an allocation based on `base` can be applied to: the
/// catalog's `ACTIVE` benefits that share at least one partner program with it, and `base`
/// itself whatever its status.
///
/// `ApplicableBenefitIds` is "the identifiers of the benefits applicable for this
/// allocation", and what makes a benefit applicable is the program the allocation was
/// granted under; a benefit that is no longer offered cannot be applied for, so an
/// `INACTIVE` one is not applicable — except the allocation's own benefit, which the
/// allocation already applies to however the catalog has moved on since. Walked over the
/// catalog's id-keyed map, so the list is in one stable order.
fn applicable_benefit_ids(benefits: &BTreeMap<String, Benefit>, base: &Benefit) -> Vec<String> {
    benefits
        .values()
        .filter(|candidate| {
            candidate.id == base.id
                || (candidate.status == BENEFIT_STATUS_ACTIVE
                    && candidate
                        .programs
                        .iter()
                        .any(|program| base.programs.contains(program)))
        })
        .map(|candidate| candidate.id.clone())
        .collect()
}

/// The resource-type segment a benefit ARN carries.
pub const BENEFIT_RESOURCE: &str = "benefit";
/// The resource-type segment a benefit-application ARN carries.
pub const APPLICATION_RESOURCE: &str = "benefit-application";
/// The resource-type segment a benefit-allocation ARN carries, from the modelled
/// `BenefitAllocation` resource's own ARN template
/// (`catalog/{CatalogName}/benefit-allocation/benalloc-{BenefitAllocationId}`, see
/// `smithy_query(query="shape",shape="BenefitAllocation")`).
pub const ALLOCATION_RESOURCE: &str = "benefit-allocation";

/// Per-service state for com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice. A handler reaches it through [`service_state`].
///
/// BOUNDED: `CreateBenefitApplication` refuses a create that would take a catalog past
/// [`MAX_APPLICATIONS_PER_CATALOG`], the catalog set is fixed by [`SEEDED_CATALOGS`],
/// `arns` and `client_tokens` carry at most one entry per stored application, one
/// application records at most [`MAX_AMENDMENT_HISTORY`] amendments, and a catalog holds
/// at most [`MAX_ALLOCATIONS_PER_CATALOG`] allocations — so no caller can grow this store
/// without limit.
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
            catalogs: {
                // One instant for every catalog, so two catalogs' seeded allocations
                // carry the same timestamps rather than drifting by the seeding loop.
                let seeded_at_millis = now_millis();
                SEEDED_CATALOGS
                    .iter()
                    .map(|name| ((*name).to_string(), Catalog::seeded(name, seeded_at_millis)))
                    .collect()
            },
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

    /// The allocation `identifier` names inside `catalog`, for a reporting operation.
    ///
    /// Resolved as [`Catalog::resolve_allocation_id`] does, so both forms the modelled
    /// `BenefitAllocationIdentifier` admits name the same record. `None` when the catalog
    /// does not exist or holds no such allocation.
    pub fn allocation(&self, catalog: &str, identifier: &str) -> Option<&BenefitAllocation> {
        let stored = self.catalogs.get(catalog)?;
        let id = stored.resolve_allocation_id(catalog, identifier)?;
        stored.allocations.get(&id)
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

    /// Record an allocation AWS granted for a benefit, and report the id it minted.
    ///
    /// `None` when the catalog does not exist, when it holds no benefit `benefit_identifier`
    /// names, when `application_identifier` names no application of it, or when the catalog
    /// already holds [`MAX_ALLOCATIONS_PER_CATALOG`] allocations. Both identifiers accept
    /// either form their modelled patterns allow (a bare id or an ARN), resolved exactly as
    /// every operation resolves them.
    ///
    /// No modelled operation reaches this, and deliberately so: granting an allocation is
    /// what AWS's own fulfillment workflow does once it approves a partner's application,
    /// and this API models no operation that creates one — just as it models no partner
    /// registration, which is why [`Self::grant_partner_entitlement`] is reached the same
    /// way. So a grant arrives from OUTSIDE the API, which in this emulator means an
    /// in-process caller holding the shared engine; nothing on the wire can grant itself an
    /// allocation.
    ///
    /// The allocation runs for [`ALLOCATION_TERM_MILLIS`] from `now_millis`, and its
    /// description, fulfillment types and applicable benefits are the benefit's own rather
    /// than values the caller supplies. It is fulfilled with the FIRST type its benefit
    /// offers — a benefit offering two is fulfilled with one of them and nothing here names
    /// which — for [`GRANTED_ALLOCATION_AMOUNT_MINOR`], of which nothing has been issued
    /// yet.
    pub fn grant_allocation(
        &mut self,
        catalog: &str,
        benefit_identifier: &str,
        application_identifier: Option<&str>,
        name: &str,
        now_millis: i64,
    ) -> Option<String> {
        let benefit_id = self
            .catalogs
            .get(catalog)?
            .resolve_benefit_id(catalog, benefit_identifier)?;
        let benefit_application_id = match application_identifier {
            Some(identifier) => Some(self.resolve_application_id(catalog, identifier)?),
            None => None,
        };
        if self.catalogs.get(catalog)?.allocations.len() >= MAX_ALLOCATIONS_PER_CATALOG {
            return None;
        }
        let stored = self.catalogs.get(catalog)?;
        let benefit = stored.benefits.get(&benefit_id)?.clone();
        let applicable_benefit_ids = applicable_benefit_ids(&stored.benefits, &benefit);
        let id = self.mint_allocation_id(now_millis);
        let fulfillment_type = benefit.fulfillment_types.first().cloned();
        let expires_at_millis = now_millis.saturating_add(ALLOCATION_TERM_MILLIS);
        let allocation = BenefitAllocation {
            id: id.clone(),
            catalog: catalog.to_string(),
            benefit_id,
            benefit_application_id,
            name: name.to_string(),
            description: Some(benefit.description.clone()),
            status: ALLOCATION_STATUS_ACTIVE.to_string(),
            status_reason: None,
            fulfillment_types: benefit.fulfillment_types.clone(),
            fulfillment: fulfillment_type.as_deref().and_then(|fulfillment_type| {
                AllocationFulfillment::new(
                    fulfillment_type,
                    GRANTED_ALLOCATION_AMOUNT_MINOR,
                    0,
                    &id,
                    now_millis,
                    expires_at_millis,
                )
            }),
            fulfillment_type: fulfillment_type.unwrap_or_default(),
            applicable_benefit_ids,
            created_at_millis: now_millis,
            updated_at_millis: now_millis,
            starts_at_millis: now_millis,
            expires_at_millis,
        };
        self.catalogs
            .get_mut(catalog)?
            .allocations
            .insert(id.clone(), allocation);
        Some(id)
    }

    /// Materialise, in `catalog`, every allocation expiry `now_millis` has already made
    /// due — see [`BenefitAllocation::expire_due`] for why a reporting operation may.
    pub fn expire_allocations_due(&mut self, catalog: &str, now_millis: i64) {
        if let Some(catalog) = self.catalogs.get_mut(catalog) {
            for allocation in catalog.allocations.values_mut() {
                allocation.expire_due(now_millis);
            }
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
    pub fn mint_application_id(&mut self, now_millis: i64) -> String {
        self.mint_id(
            APPLICATION_ID_PREFIX,
            now_millis,
            Self::holds_application,
            Self::application_count,
        )
    }

    /// A `benalloc-[0-9a-z]{14}` id no stored allocation is using.
    pub fn mint_allocation_id(&mut self, now_millis: i64) -> String {
        self.mint_id(
            ALLOCATION_ID_PREFIX,
            now_millis,
            Self::holds_allocation,
            Self::allocation_count,
        )
    }

    /// An id of `prefix`'s kind that no stored record of that kind is using, where `holds`
    /// answers whether one is and `count` how many there are.
    ///
    /// The sequence number makes two ids minted in the same millisecond differ, and the
    /// mixing step keeps them from being consecutive, so a caller cannot derive the next
    /// id from the last.
    fn mint_id(
        &mut self,
        prefix: &str,
        now_millis: i64,
        holds: fn(&Self, &str) -> bool,
        count: fn(&Self) -> usize,
    ) -> String {
        for _ in 0..1024 {
            self.id_sequence = self.id_sequence.wrapping_add(1);
            let seed = splitmix64((now_millis as u64) ^ self.id_sequence.rotate_left(17));
            let candidate = format!("{prefix}{}", id_body(seed));
            if !holds(self, &candidate) {
                return candidate;
            }
        }
        // Bounded fallback, so a pathological clock can never spin here: the
        // zero-padded decimals are also `[0-9a-z]{14}`, and there are always more of
        // them than there are stored records.
        (0u64..=count(self) as u64)
            .map(|n| format!("{prefix}{n:014}"))
            .find(|candidate| !holds(self, candidate))
            .unwrap_or_else(|| format!("{prefix}{:014}", count(self)))
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

    fn holds_allocation(&self, id: &str) -> bool {
        self.catalogs
            .values()
            .any(|catalog| catalog.allocations.contains_key(id))
    }

    fn allocation_count(&self) -> usize {
        self.catalogs
            .values()
            .map(|catalog| catalog.allocations.len())
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

/// The id of the seeded allocation named `name` in `catalog`, derived from both exactly as
/// a seeded benefit's is (see [`benefit_id`]), so it too is stable across runs and distinct
/// per catalog. A GRANTED allocation's id is minted instead, by
/// [`ComAmazonawsPartnercentralbenefitsPartnercentralbenefitsserviceState::mint_allocation_id`].
fn allocation_id(catalog: &str, name: &str) -> String {
    let seed = fnv1a64(format!("{catalog}/{name}").as_bytes());
    format!("{ALLOCATION_ID_PREFIX}{}", id_body(seed))
}

/// The `IssuanceDetail$IssuanceId` of the issuance made against the allocation `id`.
///
/// The member is an unconstrained `String` and AWS publishes no format for it, so the id is
/// DERIVED from the allocation's own id: stable across reads, and distinct per allocation
/// because the allocation ids are.
fn issuance_id(id: &str) -> String {
    format!("issuance-{}", id_body(fnv1a64(id.as_bytes())))
}

/// The `CreditCode$AwsCreditCode` of code `index` of the allocation `id`.
///
/// The member is an unconstrained `String` and AWS publishes no format for a promotional
/// credit code, so the code is DERIVED from the allocation's id and the code's index within
/// it: [`ID_BODY_LEN`] upper-case characters, stable across reads and distinct per code.
fn credit_code_text(id: &str, index: usize) -> String {
    let seed = fnv1a64(format!("{id}/{index}").as_bytes());
    id_body(seed).to_ascii_uppercase()
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

/// The ARN of the benefit allocation `id` of `catalog`, in `region`.
pub fn allocation_arn(region: &str, catalog: &str, id: &str) -> String {
    resource_arn(region, catalog, ALLOCATION_RESOURCE, id)
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
/// `resource` is [`CATALOG_KIND`], [`BENEFIT_KIND`], [`APPLICATION_KIND`] or
/// [`ALLOCATION_KIND`].
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

/// A list member, omitted from the wire when nothing was ever stored for it.
pub fn optional_list<T: Clone>(values: &[T]) -> Option<Vec<T>> {
    (!values.is_empty()).then(|| values.to_vec())
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
