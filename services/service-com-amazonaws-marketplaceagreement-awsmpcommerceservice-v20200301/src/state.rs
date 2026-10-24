// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301 service.
//!
//! Three resources carry the agreement lifecycle this crate serves:
//!
//! * an **agreement request** — "a quote for the terms you want to accept"
//!   (`CreateAgreementRequest`), which captures the requested terms and the
//!   charges they imply;
//! * an **agreement** — what `AcceptAgreementRequest` finalises from one of
//!   those requests, and what `DescribeAgreement`, `GetAgreementTerms`,
//!   `SearchAgreements` and `CancelAgreement` report on and mutate. Accepting it
//!   also provisions the **entitlements** it grants, which belong to the
//!   agreement rather than standing on their own and which
//!   `GetAgreementEntitlements` reports;
//! * a **payment request** — what `SendAgreementPaymentRequest` submits under an
//!   agreement's `VariablePaymentTerm`, and what
//!   `ListAgreementPaymentRequests` reports;
//! * a **billing adjustment request** — what
//!   `BatchCreateBillingAdjustmentRequest` submits against one invoice of an
//!   agreement, and what `GetBillingAdjustmentRequest` and
//!   `ListBillingAdjustmentRequests` report on.
//!
//! Every projection the model asks for (`DescribeAgreementOutput`,
//! `AgreementViewSummary`, `ChargeSummary`, `AcceptedTerm`,
//! `BillingAdjustmentSummary`) is computed from these records, so the
//! implemented operations share exactly one copy of each resource.
//!
//! ## What this emulator does NOT know
//!
//! An agreement request names terms that live in an *agreement proposal* (an
//! offer), which is a resource of a different AWS Marketplace API and is not in
//! this service's model or state. So the emulator holds no rate card, and no
//! money amount can be computed from a request: every amount member
//! (`ChargeSummary$newAgreementValueAfterTax`, `ExpectedCharge$amount`,
//! `ItemizedCharge$incrementalChargeAmount`, …) is optional in the model and is
//! left absent rather than invented. The one amount the model DOES define
//! without a rate card is `EstimatedCharges$agreementValue`, which documents
//! "This is `0` if the accepted terms contain `UsageBasedPricingTerm` without
//! `ConfigurableUpfrontPricingTerm` or `RecurringPaymentTerm`" — i.e. zero when
//! nothing in the terms commits the acceptor to an amount, and that is the case
//! this emulator reports.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    AcceptedTerm, Acceptor, AgreementCancellationRequestSummary, AgreementEntitlement,
    AgreementViewSummary, BatchCreateBillingAdjustmentRequestEntry, BillingAdjustmentSummary,
    ByolPricingTerm, Charge, ChargeSummary, ConfigurableUpfrontPricingTerm, ConflictException,
    DescribeAgreementOutput, EndTimeBehavior, Entitlement, EstimatedCharges, ExpectedCharge,
    Filter, FixedUpfrontPricingTerm, FreeTrialPricingTerm, GetBillingAdjustmentRequestOutput,
    InternalServerException, ItemizedCharge, LegalTerm, NetPaymentTerm, PaymentRequestSummary,
    PaymentScheduleTerm, ProposalSummary, Proposer, PurchaseOrder, RecurringPaymentTerm,
    RenewalTerm, RequestedTerm, RequestedTermConfiguration, Resource, ResourceNotFoundException,
    SendAgreementPaymentRequestInput, SupportTerm, TaxConfiguration, ThrottlingException,
    UsageBasedPricingTerm, ValidationException, ValidationExceptionField, ValidityTerm,
    VariablePaymentTerm,
};

/// The `agreementType` every agreement this slice mints carries.
///
/// `DescribeAgreementOutput$agreementType` documents two values,
/// "PurchaseAgreement or VendorInsightsAgreement", and `ChargeSummary` — the
/// output `CreateAgreementRequest` computes — says it "is applicable only when a
/// request is created for a `PurchaseAgreement`". The agreement-request flow
/// this crate serves is therefore the purchase one.
pub const AGREEMENT_TYPE: &str = "PurchaseAgreement";

/// The catalog an agreement is created in, and the one
/// `SearchAgreementsInput$catalog` selects.
///
/// AWS Marketplace exposes a single catalog, `AWSMarketplace`; the agreement
/// operations carry no catalog member, so every agreement is created in it.
pub const CATALOG: &str = "AWSMarketplace";

/// `Intent`'s three values (`INTENT_VALUES` in `types`), named where the
/// handlers branch on them.
pub const INTENT_NEW: &str = "NEW";
/// See [`INTENT_NEW`].
pub const INTENT_AMEND: &str = "AMEND";
/// See [`INTENT_NEW`].
pub const INTENT_REPLACE: &str = "REPLACE";

/// The `AgreementStatus` an accepted agreement carries, and the one
/// `CancelAgreement` moves it out of.
pub const STATUS_ACTIVE: &str = "ACTIVE";
/// The `AgreementStatus` `CancelAgreement` moves an agreement to.
pub const STATUS_CANCELLED: &str = "CANCELLED";
/// The `AgreementStatus` the agreement a `REPLACE` request acts upon carries
/// from then on: "REPLACED – The agreement was replaced using an agreement
/// replacement offer."
pub const STATUS_REPLACED: &str = "REPLACED";

// ---------------------------------------------------------------------------
// Entitlements
// ---------------------------------------------------------------------------

/// The `AgreementEntitlementStatus` values an entitlement of this emulator
/// reports (`AGREEMENT_ENTITLEMENT_STATUS_VALUES` in `types`), each the state
/// the agreement that provisioned it and the clock put it in — see
/// [`Agreement::entitlement_state`].
///
/// `FAILED` is the one modelled value none reports: every condition the model
/// gives a reason code for a failure (`INVALID_PAYMENT_INSTRUMENT`,
/// `ACCOUNT_SUSPENDED`, `INCOMPATIBLE_CURRENCY`, `PRODUCT_RESTRICTED`,
/// `UNSUPPORTED_OPERATION`) belongs to a payment instrument, an account state or
/// a product catalog this service neither models nor stores, so the emulator
/// never reports a provisioning failure for work nothing ran.
pub const ENTITLEMENT_PENDING: &str = "PENDING";
/// See [`ENTITLEMENT_PENDING`].
pub const ENTITLEMENT_SCHEDULED: &str = "SCHEDULED";
/// See [`ENTITLEMENT_PENDING`].
pub const ENTITLEMENT_PROVISIONED: &str = "PROVISIONED";
/// See [`ENTITLEMENT_PENDING`].
pub const ENTITLEMENT_DEPROVISIONED: &str = "DEPROVISIONED";

/// The `AgreementEntitlementStatusReasonCode` reported beside each of those
/// statuses — "provides more information about the status of an entitlement", so
/// it names WHY the entitlement is in that state
/// (`AGREEMENT_ENTITLEMENT_STATUS_REASON_CODE_VALUES` in `types`).
pub const ENTITLEMENT_REASON_PROVISIONING: &str = "PROVISIONING_IN_PROGRESS";
/// See [`ENTITLEMENT_REASON_PROVISIONING`].
pub const ENTITLEMENT_REASON_FUTURE_START_DATE: &str = "FUTURE_START_DATE";
/// See [`ENTITLEMENT_REASON_PROVISIONING`].
pub const ENTITLEMENT_REASON_AGREEMENT_ACTIVE: &str = "AGREEMENT_ACTIVE";
/// See [`ENTITLEMENT_REASON_PROVISIONING`].
pub const ENTITLEMENT_REASON_AGREEMENT_INACTIVE: &str = "AGREEMENT_INACTIVE";

/// How long an entitlement an acceptance provisioned stays `PENDING` before the
/// clock owes it `PROVISIONED`.
///
/// AWS publishes no provisioning time for an agreement's entitlements, so this
/// span is the emulator's OWN — the same kind of span as
/// [`BILLING_ADJUSTMENT_SETTLEMENT`]: non-zero, so a caller reading straight
/// after the accept still sees the `PROVISIONING_IN_PROGRESS` every entitlement
/// starts in, and short enough that a test can wait it out.
pub const ENTITLEMENT_PROVISIONING: Duration = Duration::from_millis(500);

/// The service segment of the ARN an entitlement's licence carries:
/// `AgreementEntitlement$licenseArn` is "the Amazon Resource Name (ARN) of the
/// AWS License Manager license associated with the entitlement".
pub const LICENSE_ARN_SERVICE: &str = "license-manager";

/// The region an ARN this service mints is in when the caller signed for none —
/// `::engine::emulator_arn`'s own documented default ("emulators default to
/// `us-east-1`").
pub const DEFAULT_REGION: &str = "us-east-1";

/// The region to mint a licence ARN in: the one the caller signed for, else
/// [`DEFAULT_REGION`].
///
/// Read from the SigV4 credential scope, the same signal the router routes on,
/// so a licence is minted in the region the client addressed.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `ResourceType` values this service reports on a `ResourceNotFoundException`
/// or a `ConflictException` (`RESOURCE_TYPE_VALUES` in `types`).
pub const RESOURCE_TYPE_AGREEMENT: &str = "Agreement";
/// See [`RESOURCE_TYPE_AGREEMENT`].
pub const RESOURCE_TYPE_AGREEMENT_REQUEST: &str = "AgreementRequest";
/// See [`RESOURCE_TYPE_AGREEMENT`].
pub const RESOURCE_TYPE_BILLING_ADJUSTMENT_REQUEST: &str = "BillingAdjustmentRequest";
/// See [`RESOURCE_TYPE_AGREEMENT`].
pub const RESOURCE_TYPE_PAYMENT_REQUEST: &str = "PaymentRequest";

/// The largest page the paginated operations serve, and the page they serve
/// when the request names no `maxResults`: `MaxResults` is modelled
/// `@range(min: 1, max: 50)`.
pub const MAX_RESULTS: i32 = 50;

/// How many agreement requests the store holds before `CreateAgreementRequest`
/// answers its modelled `ServiceQuotaExceededException`.
///
/// AWS Marketplace publishes no public number for this, so the bound is the
/// emulator's OWN bound on the store rather than a claim about an account's
/// quota: it is what stops state growing without end, and it is far above any
/// workflow an emulator serves.
pub const AGREEMENT_REQUEST_QUOTA: usize = 1_000;

/// How many billing adjustment requests the store holds before
/// `BatchCreateBillingAdjustmentRequest` stops accepting entries.
///
/// The same kind of bound as [`AGREEMENT_REQUEST_QUOTA`], and for the same
/// reason: AWS publishes no number, so this is the emulator's OWN bound on the
/// store — what stops the records growing without end across calls, however
/// many calls are made — rather than a claim about an account's quota. It is far
/// above any workflow an emulator serves. [`BILLING_ADJUSTMENT_BATCH_LIMIT`]
/// bounds ONE call; this bounds the store. Past it an entry is refused on
/// `BatchCreateBillingAdjustmentRequest`'s own per-entry `errors` channel, so a
/// call whose other entries still fit is answered rather than refused.
pub const BILLING_ADJUSTMENT_REQUEST_QUOTA: usize = 1_000;

/// The `quotaCode` reported with that refusal. Named for what it bounds: a real
/// quota code is an identifier Service Quotas issues, which this emulator does
/// not model and so must not invent one of.
pub const AGREEMENT_REQUEST_QUOTA_CODE: &str = "AgreementRequestsPerAccount";

/// How many payment requests the store holds before
/// `SendAgreementPaymentRequest` refuses to store another.
///
/// Like [`AGREEMENT_REQUEST_QUOTA`], this is the emulator's OWN bound on the
/// store rather than a claim about an account's quota (AWS Marketplace
/// publishes no number for it): it is what stops a caller growing state without
/// end, and it is far above any workflow an emulator serves. It also bounds the
/// idempotency lookup, which scans the stored requests for the `clientToken`
/// that minted one rather than holding a second, separately growing map.
///
/// `SendAgreementPaymentRequest` models no `ServiceQuotaExceededException`, so
/// reaching the bound is answered with the modelled `ThrottlingException`
/// ("Request was denied due to request throttling") — a retryable refusal that
/// stores nothing.
pub const PAYMENT_REQUEST_QUOTA: usize = 1_000;

/// How many agreement cancellation requests the store holds before
/// `SendAgreementCancellationRequest` refuses another.
///
/// Like [`AGREEMENT_REQUEST_QUOTA`] this is the emulator's OWN ceiling on the
/// store rather than a claim about an account's quota: it is what stops state
/// growing without end under repeated sends, and it is far above any workflow an
/// emulator serves. `SendAgreementCancellationRequest` models no
/// `ServiceQuotaExceededException`, so a send past the ceiling is refused with
/// its modelled `ThrottlingException` ([`store_full`]) — the service declining
/// to take more work — and nothing is stored.
///
/// This also bounds the [`ComAmazonawsMarketplaceagreementAwsmpcommerceserviceV20200301State::idempotency`]
/// map, whose entries are only ever written beside a record a create stored: the
/// map holds at most one key per stored resource, so every create's own ceiling
/// bounds it too.
pub const AGREEMENT_CANCELLATION_REQUEST_QUOTA: usize = 1_000;

/// The sigv4 signing name of this service, reported as the `serviceCode` of a
/// quota refusal.
pub const SERVICE_CODE: &str = "aws-marketplace";

/// `EstimatedCharges$agreementValue` / `ChargeSummary$newAgreementValue` for an
/// agreement whose accepted terms commit the acceptor to nothing.
///
/// `agreementValue` states this case for metered pricing: "This is `0` if the
/// accepted terms contain `UsageBasedPricingTerm` without
/// `ConfigurableUpfrontPricingTerm` or `RecurringPaymentTerm`" — i.e. zero when
/// no accepted term commits an amount. This emulator applies it to every such
/// terms set, which includes one carrying no pricing term at all (a legal term
/// only): there too the total known amount the acceptor has to pay is zero.
pub const ZERO_AGREEMENT_VALUE: &str = "0";

// ---------------------------------------------------------------------------
// Billing adjustment requests
// ---------------------------------------------------------------------------

/// `BillingAdjustmentStatus`'s values (`BILLING_ADJUSTMENT_STATUS_VALUES` in
/// `types`), named where the billing-adjustment handlers branch on them.
///
/// `PENDING` is what a submitted request carries while AWS Marketplace is still
/// processing it, `COMPLETED` once the adjustment has been applied, and
/// `VALIDATION_FAILED` when the submitted adjustment did not pass validation —
/// the one status `GetBillingAdjustmentRequestOutput$statusMessage` is
/// "populated only when" ([`BILLING_ADJUSTMENT_VALIDATION_FAILED`]).
pub const BILLING_ADJUSTMENT_PENDING: &str = "PENDING";
/// See [`BILLING_ADJUSTMENT_PENDING`].
pub const BILLING_ADJUSTMENT_COMPLETED: &str = "COMPLETED";
/// See [`BILLING_ADJUSTMENT_PENDING`]. No stored request reaches this status:
/// `BatchCreateBillingAdjustmentRequest` documents an `errors` list "for entries
/// that failed validation", so an entry this emulator cannot accept is reported
/// on that per-entry channel at submit time and no record is created for it. The
/// checks AWS can only make later (against the invoice being adjusted) need the
/// invoice, which this service neither models nor stores.
pub const BILLING_ADJUSTMENT_VALIDATION_FAILED: &str = "VALIDATION_FAILED";

/// How long a submitted billing adjustment request stays `PENDING` before the
/// clock owes it its terminal `COMPLETED`.
///
/// AWS publishes no processing time for a billing adjustment, so this span is
/// the emulator's OWN: it is non-zero, so a caller reading straight after the
/// submit still sees the `PENDING` the submit reported, and short enough that a
/// test can wait it out.
pub const BILLING_ADJUSTMENT_SETTLEMENT: Duration = Duration::from_millis(500);

/// How many entries one `BatchCreateBillingAdjustmentRequest` call accepts
/// before it answers its modelled `ValidationException` with reason
/// `BILLING_ADJUSTMENTS_LIMIT_EXCEEDED`.
///
/// `BatchCreateBillingAdjustmentRequestEntryList` carries no `@length`, and AWS
/// publishes no public number, so the bound is the emulator's OWN bound on one
/// batch rather than a claim about an account's quota: it is what stops one call
/// growing state without end.
pub const BILLING_ADJUSTMENT_BATCH_LIMIT: usize = 50;

/// `BillingAdjustmentErrorCode`'s values (`BILLING_ADJUSTMENT_ERROR_CODE_VALUES`
/// in `types`), as `BatchCreateBillingAdjustmentError$code` reports them for the
/// entry that failed.
pub const BILLING_ADJUSTMENT_ERROR_VALIDATION: &str = "VALIDATION_EXCEPTION";
/// See [`BILLING_ADJUSTMENT_ERROR_VALIDATION`].
pub const BILLING_ADJUSTMENT_ERROR_NOT_FOUND: &str = "RESOURCE_NOT_FOUND_EXCEPTION";
/// See [`BILLING_ADJUSTMENT_ERROR_VALIDATION`].
pub const BILLING_ADJUSTMENT_ERROR_CONFLICT: &str = "CONFLICT_EXCEPTION";

/// The message `BatchCreateBillingAdjustmentError$message` carries for an entry
/// whose `agreementId` names no agreement.
///
/// A per-entry error carries only its code, its message and the entry's
/// `clientToken`, so the message is the service's own sentence for the code —
/// the same one whatever entry hit it — and the entry it is about is the one its
/// `clientToken` names.
pub const BILLING_ADJUSTMENT_AGREEMENT_NOT_FOUND_MESSAGE: &str =
    "Provided agreement does not exist.";

/// Whether `amount` is the positive amount `PositiveAmountUpto8Decimals` is
/// named for.
///
/// That shape's `^[0-9]*(\.[0-9]{0,8})?$` — which the boundary already enforced —
/// also admits amounts that are not positive (`""`, `"0"`, `"0.00"`), so
/// positivity is the one part of the constraint left for a handler: the value
/// has to carry a non-zero digit.
pub fn is_positive_amount(amount: &str) -> bool {
    amount.chars().any(|digit| digit.is_ascii_digit() && digit != '0')
}

/// `Timing`'s values (`TIMING_VALUES` in `types`), with the meaning
/// `ExpectedCharge$timing` documents: `ON_ACCEPTANCE` is "charged immediately
/// when the agreement request is accepted", `BILLING_PERIOD` is "charged on each
/// billing period" and `SCHEDULED` is "charged at a predetermined future date".
pub const TIMING_ON_ACCEPTANCE: &str = "ON_ACCEPTANCE";
/// See [`TIMING_ON_ACCEPTANCE`].
pub const TIMING_BILLING_PERIOD: &str = "BILLING_PERIOD";
/// See [`TIMING_ON_ACCEPTANCE`].
pub const TIMING_SCHEDULED: &str = "SCHEDULED";

/// The revision every charge this store holds reports as `Charge$revision`.
///
/// `ChargeRevision` is modelled `@range(min: 1)`
/// (`smithy_query(query="shape", shape="ChargeRevision")`), so a charge's first
/// revision is 1. A charge is minted once — by the quote whose requested terms
/// imply it ([`ComAmazonawsMarketplaceagreementAwsmpcommerceserviceV20200301State::charge_summary`])
/// — and no operation of this service revises one: what changes afterwards is
/// the purchase-order association recorded ON the charge, which is a member of
/// the charge's report rather than a new revision of the charge. So every charge
/// this store holds is at its first revision.
pub const INITIAL_CHARGE_REVISION: i64 = 1;

/// Every term kind `AcceptedTerm` models, spelled as the term's own `type`
/// member spells it (`GetAgreementTerms` documents "`TermType` – The type of
/// term, such as `LegalTerm`, `RenewalTerm`, or `ConfigurableUpfrontPricingTerm`").
///
/// This is the catalog [`term_kind`] reads a requested term's kind from.
pub const TERM_KINDS: &[&str] = &[
    "LegalTerm",
    "SupportTerm",
    "RenewalTerm",
    "UsageBasedPricingTerm",
    "ConfigurableUpfrontPricingTerm",
    "ByolPricingTerm",
    "RecurringPaymentTerm",
    "ValidityTerm",
    "PaymentScheduleTerm",
    "FreeTrialPricingTerm",
    "FixedUpfrontPricingTerm",
    "VariablePaymentTerm",
    "NetPaymentTerm",
];

/// The term kind a requested term with neither a configuration nor a
/// kind-naming id is accepted as.
///
/// A `RequestedTerm` names a term of an agreement proposal, which this service
/// neither models nor stores, so the only things the emulator can read a term's
/// kind from are the request's own `configuration` union member and its `id`.
/// When neither names a kind, the term is accepted as a legal term: the kind
/// that takes no configuration and incurs no charge, so reporting it asserts
/// nothing the emulator does not hold.
pub const DEFAULT_TERM_KIND: &str = "LegalTerm";

/// The attributes `Sort$sortBy` documents: "which can be `EndTime`, `StartTime`,
/// or `LastUpdateTime`".
pub const SORT_BY_VALUES: &[&str] = &["EndTime", "StartTime", "LastUpdateTime"];

/// `Sort$sortBy`'s documented default ("The default value is `EndTime`").
pub const DEFAULT_SORT_BY: &str = "EndTime";

/// The sort attributes `Sort$sortBy` restricts to one party type: "`StartTime`
/// and `LastUpdateTime` are supported only when `PartyType` is `Proposer`"
/// (repeated on `SearchAgreementsInput$sort`). `EndTime` carries no such
/// restriction, so it is not listed here.
pub const PROPOSER_ONLY_SORT_BY: &[&str] = &["StartTime", "LastUpdateTime"];

/// `Sort$sortOrder`'s documented default ("The default value is `ASCENDING`").
pub const SORT_ORDER_ASCENDING: &str = "ASCENDING";

/// The filter names `SearchAgreementsInput$filters` documents, in the order the
/// model lists them. A filter naming anything else is refused.
pub const SEARCH_FILTER_NAMES: &[&str] = &[
    "ResourceIdentifier",
    "ResourceType",
    "PartyType",
    "AcceptorAccountId",
    "OfferId",
    "Status",
    "BeforeEndTime",
    "AfterEndTime",
    "BeforeStartTime",
    "AfterStartTime",
    "BeforeLastUpdateTime",
    "AfterLastUpdateTime",
    "AgreementType",
    "OfferSetId",
    "EndTimeBehaviorType",
    "EndTimeBehaviorReasonCode",
    "InitialAgreementId",
    "LicenseArn",
];

/// `PartyType`'s two values, as `SearchAgreementsInput$filters` documents them:
/// "The party type of the caller. Use `Proposer` or `Acceptor`."
///
/// `ListAgreementPaymentRequestsInput$partyType` names the same two with the
/// roles spelled out: "Use `Proposer` to list payment requests where you are the
/// seller, or `Acceptor` to list payment requests where you are the buyer."
pub const PARTY_TYPE_ACCEPTOR: &str = "Acceptor";
/// See [`PARTY_TYPE_ACCEPTOR`].
pub const PARTY_TYPE_PROPOSER: &str = "Proposer";

/// The `PaymentRequestStatus` a payment request is created in:
/// `SendAgreementPaymentRequest` documents "The payment request is created in
/// `PENDING_APPROVAL` status, at which point the buyer can accept or reject
/// it."
pub const PAYMENT_REQUEST_STATUS_PENDING_APPROVAL: &str = "PENDING_APPROVAL";

/// The `PaymentRequestStatus` an approved payment request carries, and the only
/// one `PaymentRequestSummary$chargeId` is reported for ("This field is only
/// present for approved payment requests").
pub const PAYMENT_REQUEST_STATUS_APPROVED: &str = "APPROVED";

/// Every value a `partyType` admits. `PartyType` is modelled a plain string
/// (`@length(1, 32)`, `^[A-Za-z]+$`), not an enum, so the boundary validator
/// cannot refuse a party type that is neither of these — the handler does, with
/// [`validation_exception`] and the modelled `INVALID_PARTY_TYPE` reason.
pub const PARTY_TYPE_VALUES: &[&str] = &[PARTY_TYPE_PROPOSER, PARTY_TYPE_ACCEPTOR];

/// The `AgreementCancellationRequestStatus` a cancellation request is created
/// in: "The cancellation request is created in PENDING_APPROVAL status, at which
/// point the buyer can review it"
/// (`SendAgreementCancellationRequest`).
///
/// What moves it on is the buyer's own call
/// (`AcceptAgreementCancellationRequest`, `RejectAgreementCancellationRequest`)
/// or the seller's `CancelAgreementCancellationRequest` — never the clock, so a
/// stored request stays `PENDING_APPROVAL` until one of those runs.
pub const CANCELLATION_STATUS_PENDING_APPROVAL: &str = "PENDING_APPROVAL";

/// `ResourceType` for a cancellation request, reported on a
/// `ResourceNotFoundException` or a `ConflictException` about one.
pub const RESOURCE_TYPE_AGREEMENT_CANCELLATION_REQUEST: &str = "AgreementCancellationRequest";

// ---------------------------------------------------------------------------
// Pagination tokens
// ---------------------------------------------------------------------------

/// Characters in a `nextToken` this service mints: a 16-hex-digit digest of the
/// query the page belongs to, then the 8-hex-digit offset the next page starts
/// at.
const PAGE_TOKEN_DIGEST_LEN: usize = 16;
/// See [`PAGE_TOKEN_DIGEST_LEN`].
const PAGE_TOKEN_OFFSET_LEN: usize = 8;
/// See [`PAGE_TOKEN_DIGEST_LEN`].
const PAGE_TOKEN_LEN: usize = PAGE_TOKEN_DIGEST_LEN + PAGE_TOKEN_OFFSET_LEN;

/// The digest of the query a page belongs to — the agreement id for
/// `GetAgreementTerms`, the whole search for `SearchAgreements`.
///
/// Hashing is what makes the token opaque rather than secret: it carries no
/// resource name, and it BINDS the token to the query it was minted for, so a
/// token cannot resume a different agreement's terms or a different search. It
/// is not a capability and guards nothing — the same caller could issue the
/// query it belongs to directly.
fn scope_digest(scope: &str) -> String {
    let digest = Sha256::digest(scope.as_bytes());
    hex::encode(digest)[..PAGE_TOKEN_DIGEST_LEN].to_string()
}

/// The `nextToken` that resumes `scope` at `offset`.
///
/// Every character is a hex digit, so the token satisfies `NextToken`'s
/// `^[a-zA-Z0-9+/=_-]+$` and its 8192-character bound.
pub fn page_token(scope: &str, offset: usize) -> String {
    format!(
        "{digest}{offset:0width$x}",
        digest = scope_digest(scope),
        width = PAGE_TOKEN_OFFSET_LEN
    )
}

/// The offset `token` resumes `scope` at, or `None` when it is not a token this
/// service minted for that scope.
pub fn page_offset(scope: &str, token: &str) -> Option<usize> {
    if !token.is_ascii() || token.len() != PAGE_TOKEN_LEN {
        return None;
    }
    let (digest, offset) = token.split_at(PAGE_TOKEN_DIGEST_LEN);
    if digest != scope_digest(scope) {
        return None;
    }
    usize::from_str_radix(offset, 16).ok()
}

/// The page of `items` starting at `offset`, and the token that resumes after
/// it — absent on the last page.
///
/// Shared by `GetAgreementTerms` and `SearchAgreements`: both paginate a list
/// they have already ordered, which is the only thing the token indexes into.
///
/// `None` when `offset` is past the end of the list: a token is minted only
/// while items remain, so such an offset names a position this list never had
/// and is a bad token rather than an empty page.
pub fn paginate<T: Clone>(
    items: &[T],
    scope: &str,
    offset: usize,
    page_size: usize,
) -> Option<(Vec<T>, Option<String>)> {
    if offset > 0 && offset >= items.len() {
        return None;
    }
    let end = offset.saturating_add(page_size).min(items.len());
    let page = items.get(offset..end).unwrap_or_default().to_vec();
    let next = (end < items.len()).then(|| page_token(scope, end));
    Some((page, next))
}

/// The page size a request asks for: its `maxResults`, or the largest page the
/// model admits when it names none.
pub fn page_size(max_results: Option<i32>) -> usize {
    let requested = max_results.unwrap_or(MAX_RESULTS);
    debug_assert!(
        (1..=MAX_RESULTS).contains(&requested),
        "maxResults {requested} should have been refused at the boundary"
    );
    usize::try_from(requested.clamp(1, MAX_RESULTS)).expect("a page size in 1..=50 fits in usize")
}

// ---------------------------------------------------------------------------
// Terms
// ---------------------------------------------------------------------------

/// The kind of term `requested` names.
///
/// The requested `configuration` names it outright — each union member belongs
/// to exactly one term kind — and otherwise the term's `id` is accepted as
/// naming one of [`TERM_KINDS`]. See [`DEFAULT_TERM_KIND`] for why a term that
/// names neither is a legal term.
pub fn term_kind(requested: &RequestedTerm) -> &'static str {
    match requested.configuration {
        Some(RequestedTermConfiguration::configurableUpfrontPricingTermConfiguration(_)) => {
            "ConfigurableUpfrontPricingTerm"
        }
        Some(RequestedTermConfiguration::renewalTermConfiguration(_)) => "RenewalTerm",
        Some(RequestedTermConfiguration::variablePaymentTermConfiguration(_)) => {
            "VariablePaymentTerm"
        }
        None => TERM_KINDS
            .iter()
            .copied()
            .find(|kind| *kind == requested.id)
            .unwrap_or(DEFAULT_TERM_KIND),
    }
}

/// The accepted term a requested term becomes.
///
/// `GetAgreementTerms` documents what an accepted term carries: the `TermType`,
/// the `TermID` "which is common between offer and agreement", and the
/// `Configuration` — "the buyer/acceptor's selection at the time of agreement
/// creation". The payload members beyond those (a EULA document, a rate card, a
/// price) belong to the proposal this emulator does not hold, and stay absent.
pub fn accepted_term(requested: &RequestedTerm) -> AcceptedTerm {
    let kind = term_kind(requested);
    let r#type = Some(kind.to_string());
    let id = Some(requested.id.clone());
    let configuration = requested.configuration.clone();
    match kind {
        "SupportTerm" => AcceptedTerm::supportTerm(SupportTerm {
            r#type,
            id,
            ..Default::default()
        }),
        "RenewalTerm" => AcceptedTerm::renewalTerm(RenewalTerm {
            r#type,
            id,
            configuration: match configuration {
                Some(RequestedTermConfiguration::renewalTermConfiguration(cfg)) => Some(cfg),
                _ => None,
            },
            ..Default::default()
        }),
        "UsageBasedPricingTerm" => AcceptedTerm::usageBasedPricingTerm(UsageBasedPricingTerm {
            r#type,
            id,
            ..Default::default()
        }),
        "ConfigurableUpfrontPricingTerm" => {
            AcceptedTerm::configurableUpfrontPricingTerm(ConfigurableUpfrontPricingTerm {
                r#type,
                id,
                configuration: match configuration {
                    Some(
                        RequestedTermConfiguration::configurableUpfrontPricingTermConfiguration(
                            cfg,
                        ),
                    ) => Some(cfg),
                    _ => None,
                },
                ..Default::default()
            })
        }
        "ByolPricingTerm" => AcceptedTerm::byolPricingTerm(ByolPricingTerm { r#type, id }),
        "RecurringPaymentTerm" => AcceptedTerm::recurringPaymentTerm(RecurringPaymentTerm {
            r#type,
            id,
            ..Default::default()
        }),
        "ValidityTerm" => AcceptedTerm::validityTerm(ValidityTerm {
            r#type,
            id,
            ..Default::default()
        }),
        "PaymentScheduleTerm" => AcceptedTerm::paymentScheduleTerm(PaymentScheduleTerm {
            r#type,
            id,
            ..Default::default()
        }),
        "FreeTrialPricingTerm" => AcceptedTerm::freeTrialPricingTerm(FreeTrialPricingTerm {
            r#type,
            id,
            ..Default::default()
        }),
        "FixedUpfrontPricingTerm" => {
            AcceptedTerm::fixedUpfrontPricingTerm(FixedUpfrontPricingTerm {
                r#type,
                id,
                ..Default::default()
            })
        }
        "VariablePaymentTerm" => AcceptedTerm::variablePaymentTerm(VariablePaymentTerm {
            r#type,
            id,
            configuration: match configuration {
                Some(RequestedTermConfiguration::variablePaymentTermConfiguration(cfg)) => {
                    Some(cfg)
                }
                _ => None,
            },
            ..Default::default()
        }),
        "NetPaymentTerm" => AcceptedTerm::netPaymentTerm(NetPaymentTerm {
            r#type,
            id,
            ..Default::default()
        }),
        // `DEFAULT_TERM_KIND`, and the kind a `LegalTerm` id names.
        _ => AcceptedTerm::legalTerm(LegalTerm {
            r#type,
            id,
            ..Default::default()
        }),
    }
}

/// When a term of `kind` is charged, or `None` for a kind that carries no
/// charge at all (a legal, support, validity, renewal or BYOL term).
pub fn charge_timing(kind: &str) -> Option<&'static str> {
    match kind {
        // Prepaid: "charged immediately when the agreement request is accepted".
        "ConfigurableUpfrontPricingTerm" | "FixedUpfrontPricingTerm" => {
            Some(TIMING_ON_ACCEPTANCE)
        }
        // Metered and recurring pricing: "charged on each billing period".
        "UsageBasedPricingTerm" | "RecurringPaymentTerm" => Some(TIMING_BILLING_PERIOD),
        // An installment schedule: "charged at a predetermined future date".
        "PaymentScheduleTerm" | "VariablePaymentTerm" => Some(TIMING_SCHEDULED),
        _ => None,
    }
}

/// True for a term kind that commits the acceptor to an amount the proposal's
/// rate card prices.
///
/// `EstimatedCharges$agreementValue` names these: the total is the contract
/// value when the terms carry a `ConfigurableUpfrontPricingTerm` or
/// `FixedUpfrontPricingTerm`, the payment-schedule total when they carry a
/// `PaymentScheduleTerm`, and zero for usage-based pricing without a
/// `ConfigurableUpfrontPricingTerm` or `RecurringPaymentTerm`.
pub fn commits_amount(kind: &str) -> bool {
    matches!(
        kind,
        "ConfigurableUpfrontPricingTerm"
            | "FixedUpfrontPricingTerm"
            | "RecurringPaymentTerm"
            | "PaymentScheduleTerm"
    )
}

/// The quantity each dimension of `terms` is already bought at, summed over
/// every accepted configurable-upfront term.
///
/// This is what `ItemizedCharge$oldQuantity` reports for an amendment: "The
/// existing quantity for this dimension from the source agreement."
fn dimension_quantities(terms: &[AcceptedTerm]) -> HashMap<String, i32> {
    let mut quantities: HashMap<String, i32> = HashMap::new();
    for term in terms {
        if let AcceptedTerm::configurableUpfrontPricingTerm(pricing) = term {
            if let Some(configuration) = pricing.configuration.as_ref() {
                for dimension in &configuration.dimensions {
                    // The quantities come from the request, and one request may
                    // name the same dimension twice, so the sum is bounded
                    // rather than left to overflow `i32`: a quantity cannot be
                    // larger than the largest the member can carry.
                    let total = quantities
                        .entry(dimension.dimension_key.clone())
                        .or_default();
                    *total = total.saturating_add(dimension.dimension_value);
                }
            }
        }
    }
    quantities
}

// ---------------------------------------------------------------------------
// Records
// ---------------------------------------------------------------------------

/// One agreement request — "a quote for the terms you want to accept" — as this
/// emulator persists it.
#[derive(Debug, Clone)]
pub struct AgreementRequest {
    /// `^ar-[A-Za-z0-9]+$`, minted by
    /// [`ComAmazonawsMarketplaceagreementAwsmpcommerceserviceV20200301State::mint_agreement_request_id`].
    pub id: String,
    /// One of `Intent`'s values.
    pub intent: String,
    /// The terms the request asked for, verbatim: the accepted terms of the
    /// agreement are derived from these.
    pub requested_terms: Vec<RequestedTerm>,
    pub source_agreement_identifier: Option<String>,
    pub agreement_proposal_identifier: Option<String>,
    pub tax_configuration: Option<TaxConfiguration>,
    /// The charges the requested terms imply, computed once at create time so a
    /// replayed `clientToken` answers with the same quote.
    pub charge_summary: ChargeSummary,
    pub client_token: Option<String>,
    /// The agreement `AcceptAgreementRequest` finalised from this request; a
    /// second accept of the same request is a conflict.
    pub accepted_agreement_id: Option<String>,
    pub created_at: SystemTime,
}

/// One entitlement an acceptance provisioned, as this emulator persists it.
///
/// What is STORED is the entitlement's IDENTITY — the resource it was
/// provisioned to, the licence it granted and the instant provisioning is due to
/// finish. Its `status` and `statusReasonCode` are COMPUTED from the agreement
/// it belongs to and the clock ([`Agreement::entitlement_state`]), so cancelling
/// or replacing the agreement moves every entitlement it granted without a
/// second copy of the agreement's own state to keep in step.
#[derive(Debug, Clone)]
pub struct AgreementEntitlementRecord {
    /// "The resource that the entitlement is provisioned to, such as a product"
    /// — one of the resources the agreement's proposal names, and absent for an
    /// agreement whose proposal names none. The proposal is a resource of a
    /// different AWS Marketplace API (see the module docs), so its products are
    /// known only for an agreement derived from one that already carried them;
    /// the member is left absent rather than naming an invented product.
    pub resource: Option<Resource>,
    /// The AWS License Manager licence this entitlement granted, minted once
    /// when the entitlement was provisioned so every later report — this
    /// agreement's entitlements, `AgreementViewSummary$entitlements` and the
    /// `LicenseArn` search filter — names the same licence.
    pub license_arn: String,
    /// When the `PENDING` this entitlement was provisioned in is due to become
    /// `PROVISIONED`: the acceptance instant plus [`ENTITLEMENT_PROVISIONING`].
    /// Stored rather than slept for.
    pub provisioned_at: SystemTime,
}

/// One agreement, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct Agreement {
    /// `^[A-Za-z0-9_/-]+$`, minted by
    /// [`ComAmazonawsMarketplaceagreementAwsmpcommerceserviceV20200301State::mint_agreement_id`].
    pub id: String,
    /// "When an agreement isn't derived from another agreement, its
    /// `InitialAgreementId` is its own `AgreementId`" — otherwise the source
    /// agreement's own initial id, so a chain traces back to its first link.
    pub initial_agreement_id: String,
    pub agreement_request_id: String,
    pub agreement_type: String,
    pub catalog: String,
    pub acceptor: Acceptor,
    pub proposer: Proposer,
    /// One of `AgreementStatus`'s values.
    pub status: String,
    pub start_time: SystemTime,
    /// "The field is null for pay-as-you-go agreements, which don't have end
    /// dates." The requested terms this emulator accepts carry no validity
    /// dates, so an agreement it mints has no end date.
    pub end_time: Option<SystemTime>,
    pub acceptance_time: SystemTime,
    pub proposal_summary: ProposalSummary,
    pub accepted_terms: Vec<AcceptedTerm>,
    /// The purchase orders the accept call associated with this agreement's
    /// charges.
    pub purchase_orders: Vec<PurchaseOrder>,
    /// The entitlements accepting this agreement provisioned — one per resource
    /// its proposal names, and the single one the agreement grants when the
    /// proposal names none (see
    /// [`ComAmazonawsMarketplaceagreementAwsmpcommerceserviceV20200301State::provision_entitlements`]).
    pub entitlements: Vec<AgreementEntitlementRecord>,
    pub updated_at: SystemTime,
}

impl Agreement {
    /// Whether the acceptor asked for auto-renewal, as the accepted renewal
    /// term's `enableAutoRenew` records it.
    fn auto_renew(&self) -> Option<bool> {
        self.accepted_terms.iter().find_map(|term| match term {
            AcceptedTerm::renewalTerm(renewal) => Some(
                renewal
                    .configuration
                    .as_ref()
                    .is_some_and(|configuration| configuration.enable_auto_renew),
            ),
            _ => None,
        })
    }

    /// What happens to this agreement at its end date.
    ///
    /// `DescribeAgreementOutput$endTimeBehavior` states when the member is
    /// reported at all: "This field is present for every active agreement that
    /// has an end date. It is not present for an agreement that has no end
    /// date… It is also not present for an agreement that is no longer active."
    /// The behaviour itself follows `EndTimeBehavior`: `RENEW` when the accepted
    /// renewal term enables auto-renewal, and otherwise `EXPIRE` with the reason
    /// the terms give — `NO_RENEWAL_TERM` when they carry no renewal term at
    /// all, `ACCEPTOR_RENEW_OPTED_OUT` when they carry one the acceptor turned
    /// off.
    pub fn end_time_behavior(&self) -> Option<EndTimeBehavior> {
        self.end_time?;
        if self.status != STATUS_ACTIVE {
            return None;
        }
        Some(match self.auto_renew() {
            Some(true) => EndTimeBehavior {
                r#type: "RENEW".to_string(),
                // "The field is null when the agreement renews."
                reason_code: None,
                // The offer the agreement would renew from belongs to the
                // proposal this emulator does not hold.
                renewal_summary: None,
            },
            Some(false) => EndTimeBehavior {
                r#type: "EXPIRE".to_string(),
                reason_code: Some("ACCEPTOR_RENEW_OPTED_OUT".to_string()),
                renewal_summary: None,
            },
            None => EndTimeBehavior {
                r#type: "EXPIRE".to_string(),
                reason_code: Some("NO_RENEWAL_TERM".to_string()),
                renewal_summary: None,
            },
        })
    }

    /// The agreement's estimated cost.
    ///
    /// Reported only where the model defines it without a rate card: zero when
    /// no accepted term commits the acceptor to an amount (see
    /// [`ZERO_AGREEMENT_VALUE`]). When a term does commit one, the amount is
    /// priced by the proposal's rate card, which this emulator does not hold, so
    /// the member is left absent rather than invented — and so is
    /// `currencyCode`, which the same rate card names.
    pub fn estimated_charges(&self) -> EstimatedCharges {
        let committed = self
            .accepted_terms
            .iter()
            .any(|term| commits_amount(accepted_term_kind(term)));
        EstimatedCharges {
            currency_code: None,
            agreement_value: (!committed).then(|| ZERO_AGREEMENT_VALUE.to_string()),
        }
    }

    /// The provisioning state of one entitlement of this agreement at `now`:
    /// its `AgreementEntitlementStatus` and the `…StatusReasonCode` that says
    /// why it is in that state.
    ///
    /// An entitlement is granted BY the agreement, so its state follows the
    /// agreement's own and the clock: an agreement that is no longer `ACTIVE`
    /// entitles its acceptor to nothing (`DEPROVISIONED`, `AGREEMENT_INACTIVE`),
    /// one that has not started yet has its entitlement waiting for that start
    /// date (`SCHEDULED`, `FUTURE_START_DATE`), and one that has started is
    /// provisioning until the instant the clock owes the entitlement its licence
    /// (`PENDING`, `PROVISIONING_IN_PROGRESS`), and provisioned from then on
    /// (`PROVISIONED`, `AGREEMENT_ACTIVE`).
    pub fn entitlement_state(
        &self,
        record: &AgreementEntitlementRecord,
        now: SystemTime,
    ) -> (&'static str, &'static str) {
        if self.status != STATUS_ACTIVE {
            (
                ENTITLEMENT_DEPROVISIONED,
                ENTITLEMENT_REASON_AGREEMENT_INACTIVE,
            )
        } else if now < self.start_time {
            (ENTITLEMENT_SCHEDULED, ENTITLEMENT_REASON_FUTURE_START_DATE)
        } else if now < record.provisioned_at {
            (ENTITLEMENT_PENDING, ENTITLEMENT_REASON_PROVISIONING)
        } else {
            (
                ENTITLEMENT_PROVISIONED,
                ENTITLEMENT_REASON_AGREEMENT_ACTIVE,
            )
        }
    }

    /// The `GetAgreementEntitlements` projection of this agreement's
    /// entitlements at `now`, in the order they were provisioned in.
    ///
    /// Two of `AgreementEntitlement`'s members are left absent rather than
    /// invented, the way the module docs leave every amount absent:
    ///
    /// * `type` — "the type of entitlement". `EntitlementType` is a plain
    ///   string, the model declares no values for it and AWS documents none, so
    ///   there is no value to report rather than make up.
    /// * `registrationToken` — "a short-lived token required by acceptors to
    ///   register their account with the product provider". It is a capability
    ///   redeemed with the provider, outside this service's model: no operation
    ///   of this API consumes one, so a token this emulator minted would grant
    ///   nothing and verify nothing.
    pub fn to_entitlements(&self, now: SystemTime) -> Vec<AgreementEntitlement> {
        self.entitlements
            .iter()
            .map(|record| {
                let (status, reason_code) = self.entitlement_state(record, now);
                AgreementEntitlement {
                    resource: record.resource.clone(),
                    r#type: None,
                    registration_token: None,
                    status: Some(status.to_string()),
                    status_reason_code: Some(reason_code.to_string()),
                    license_arn: Some(record.license_arn.clone()),
                }
            })
            .collect()
    }

    /// The `DescribeAgreement` projection of this agreement.
    pub fn to_description(&self) -> DescribeAgreementOutput {
        DescribeAgreementOutput {
            agreement_id: Some(self.id.clone()),
            acceptor: Some(self.acceptor.clone()),
            proposer: Some(self.proposer.clone()),
            start_time: Some(::aws_smithy_types::DateTime::from(self.start_time)),
            end_time: self.end_time.map(::aws_smithy_types::DateTime::from),
            acceptance_time: Some(::aws_smithy_types::DateTime::from(self.acceptance_time)),
            agreement_type: Some(self.agreement_type.clone()),
            estimated_charges: Some(self.estimated_charges()),
            proposal_summary: Some(self.proposal_summary.clone()),
            status: Some(self.status.clone()),
            initial_agreement_id: Some(self.initial_agreement_id.clone()),
            end_time_behavior: self.end_time_behavior(),
        }
    }

    /// The `SearchAgreements` projection of this agreement.
    pub fn to_view_summary(&self) -> AgreementViewSummary {
        let behavior = self.end_time_behavior();
        AgreementViewSummary {
            agreement_id: Some(self.id.clone()),
            acceptance_time: Some(::aws_smithy_types::DateTime::from(self.acceptance_time)),
            start_time: Some(::aws_smithy_types::DateTime::from(self.start_time)),
            end_time: self.end_time.map(::aws_smithy_types::DateTime::from),
            last_update_time: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
            agreement_type: Some(self.agreement_type.clone()),
            acceptor: Some(self.acceptor.clone()),
            proposer: Some(self.proposer.clone()),
            proposal_summary: Some(self.proposal_summary.clone()),
            status: Some(self.status.clone()),
            // "A list of entitlements associated with the agreement": an
            // `Entitlement` carries only the licence of each entitlement the
            // acceptance provisioned; `GetAgreementEntitlements` reports the
            // same entitlements in full. Absent, not empty, for an agreement
            // that granted none.
            entitlements: (!self.entitlements.is_empty()).then(|| {
                self.entitlements
                    .iter()
                    .map(|record| Entitlement {
                        license_arn: Some(record.license_arn.clone()),
                    })
                    .collect()
            }),
            initial_agreement_id: Some(self.initial_agreement_id.clone()),
            end_time_behavior_type: behavior.as_ref().map(|b| b.r#type.clone()),
            end_time_behavior_reason_code: behavior.and_then(|b| b.reason_code),
        }
    }

    /// The accepted `VariablePaymentTerm` of this agreement that `term_id`
    /// names, or `None` when no accepted term of that kind carries that id.
    ///
    /// `SendAgreementPaymentRequest` is defined over exactly this term — "an
    /// agreement that includes a `VariablePaymentTerm`" — and
    /// `SendAgreementPaymentRequestInput$termId` names "the unique identifier of
    /// the `VariablePaymentTerm` for the agreement".
    pub fn variable_payment_term(&self, term_id: &str) -> Option<&VariablePaymentTerm> {
        self.accepted_terms.iter().find_map(|term| match term {
            AcceptedTerm::variablePaymentTerm(variable)
                if variable.id.as_deref() == Some(term_id) =>
            {
                Some(variable)
            }
            _ => None,
        })
    }

    /// The instant `sort_by` orders this agreement by, absent when the
    /// agreement has no value for that attribute.
    pub fn sort_key(&self, sort_by: &str) -> Option<SystemTime> {
        match sort_by {
            "StartTime" => Some(self.start_time),
            "LastUpdateTime" => Some(self.updated_at),
            // `DEFAULT_SORT_BY`.
            _ => self.end_time,
        }
    }
}

/// One payment request — what a seller submits to a buyer "for a specific
/// charge amount for an agreement that includes a `VariablePaymentTerm`" — as
/// this emulator persists it.
///
/// `ListAgreementPaymentRequests` reports these, and every member it projects is
/// read from this record or from the agreement it names; nothing is echoed from
/// the listing call's own input.
#[derive(Debug, Clone)]
pub struct PaymentRequest {
    /// `^pr-[a-zA-Z0-9]+$`, minted by
    /// [`ComAmazonawsMarketplaceagreementAwsmpcommerceserviceV20200301State::mint_payment_request_id`].
    pub id: String,
    /// The agreement the request is for; the agreement itself stays the single
    /// copy of its own type, catalog and parties.
    pub agreement_id: String,
    /// The `VariablePaymentTerm` of that agreement the request was sent under.
    pub term_id: String,
    pub name: String,
    pub description: Option<String>,
    pub charge_amount: String,
    /// "The currency code for the charge amount": the currency of the accepted
    /// term, which is `VariablePaymentTerm$currencyCode`. The term belongs to an
    /// agreement proposal this emulator does not hold (see the module docs), so
    /// an accepted term carries no currency and neither does a payment request
    /// sent under it.
    pub currency_code: Option<String>,
    /// One of `PaymentRequestStatus`'s values.
    pub status: String,
    /// "The unique identifier of the charge created after the payment request is
    /// approved. This field is only present for approved payment requests" —
    /// approving one is `AcceptAgreementPaymentRequest`, still a scaffold stub,
    /// so no stored request carries a charge yet.
    pub charge_id: Option<String>,
    /// The account that submitted the request, which is its proposer: "Allows
    /// sellers (proposers) to submit a payment request to buyers (acceptors)".
    pub proposer_account_id: String,
    /// The `clientToken` the send carried, if any.
    pub client_token: Option<String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl PaymentRequest {
    /// Whether this request is the one `input` asks to send — every member the
    /// request carries beside its `clientToken` is the same.
    ///
    /// A send replaying the same token with the same details reports this
    /// request; one reusing it for different details is a conflict, as the
    /// sibling `BillingAdjustmentRequest::matches_entry` treats the same case.
    pub fn matches_send(&self, input: &SendAgreementPaymentRequestInput) -> bool {
        self.agreement_id == input.agreement_id
            && self.term_id == input.term_id
            && self.name == input.name
            && self.charge_amount == input.charge_amount
            && self.description == input.description
    }

    /// The `ListAgreementPaymentRequests` projection of this payment request.
    pub fn to_summary(&self) -> PaymentRequestSummary {
        PaymentRequestSummary {
            payment_request_id: Some(self.id.clone()),
            agreement_id: Some(self.agreement_id.clone()),
            status: Some(self.status.clone()),
            name: Some(self.name.clone()),
            // Only an approved request has a charge; see `charge_id`.
            charge_id: self
                .charge_id
                .clone()
                .filter(|_| self.status == PAYMENT_REQUEST_STATUS_APPROVED),
            charge_amount: Some(self.charge_amount.clone()),
            currency_code: self.currency_code.clone(),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
        }
    }
}

/// Whether the caller is the `party_type` party of `request`.
///
/// `ListAgreementPaymentRequestsInput$partyType` states the two roles: "Use
/// `Proposer` to list payment requests where you are the seller, or `Acceptor`
/// to list payment requests where you are the buyer." The buyer is the acceptor
/// of the agreement the request was sent for, which [`caller_is_party`] decides
/// — so `agreement` is the stored agreement `request.agreement_id` names, and a
/// request whose agreement is gone has no buyer to match. The seller is the
/// account that submitted the payment request, which this service DOES record
/// (`SendAgreementPaymentRequest` is the seller's own call), unlike the proposer
/// of an agreement.
///
/// A `party_type` that is neither value matches nothing; the handler refuses it
/// before any request is scanned.
pub fn payment_request_party_matches(
    request: &PaymentRequest,
    agreement: Option<&Agreement>,
    party_type: &str,
) -> bool {
    match party_type {
        PARTY_TYPE_PROPOSER => request.proposer_account_id == ::engine::emulator_account_id(),
        PARTY_TYPE_ACCEPTOR => {
            agreement.is_some_and(|agreement| caller_is_party(agreement, PARTY_TYPE_ACCEPTOR))
        }
        _ => false,
    }
}

/// Whether the caller is the party `party_type` names on `agreement`.
///
/// A party type is "the party type of the caller", so it selects by the account
/// the agreement records for that side. This emulator serves one account, which
/// `AcceptAgreementRequest` records as the acceptor of every agreement it mints;
/// the proposer is the seller who published the proposal, whose account the
/// emulator holds no record of. So `Acceptor` selects the agreements this
/// account accepted, and `Proposer` selects those it proposed — none, until a
/// proposer account is ever stored. A party type that is neither value selects
/// nothing; a handler refuses it where it is a request member, with
/// [`validation_exception`] and the modelled `INVALID_PARTY_TYPE` reason.
///
/// Shared by `SearchAgreements`' `PartyType` filter and by every operation that
/// takes a `partyType` of its own.
pub fn caller_is_party(agreement: &Agreement, party_type: &str) -> bool {
    let caller = Some(::engine::emulator_account_id());
    match party_type {
        PARTY_TYPE_ACCEPTOR => agreement.acceptor.account_id.as_deref() == caller,
        PARTY_TYPE_PROPOSER => agreement.proposer.account_id.as_deref() == caller,
        _ => false,
    }
}

/// One agreement cancellation request, as this emulator persists it.
///
/// `SendAgreementCancellationRequest` creates it over an active agreement and
/// every later report of it (`ListAgreementCancellationRequests`,
/// `GetAgreementCancellationRequest`) is computed from this one record.
#[derive(Debug, Clone)]
pub struct AgreementCancellationRequest {
    /// `^acr-[a-zA-Z0-9]+$`, minted by
    /// [`ComAmazonawsMarketplaceagreementAwsmpcommerceserviceV20200301State::mint_agreement_cancellation_request_id`].
    pub id: String,
    /// The agreement the cancellation was asked for.
    pub agreement_id: String,
    /// The agreement's own `agreementType` and `catalog`, copied at create time:
    /// a summary reports "the type of agreement" and "the catalog in which the
    /// agreement was created", which are properties of that agreement and not of
    /// the cancelling call.
    pub agreement_type: String,
    /// See [`AgreementCancellationRequest::agreement_type`].
    pub catalog: String,
    /// One of `AgreementCancellationRequestStatus`'s values; see
    /// [`CANCELLATION_STATUS_PENDING_APPROVAL`].
    pub status: String,
    /// One of `AgreementCancellationRequestReasonCode`'s values, as the modelled
    /// enum member the create call carried.
    pub reason_code: String,
    /// The optional free-text reason the create call carried.
    pub description: Option<String>,
    /// The `clientToken` the create call carried, if any.
    pub client_token: Option<String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl AgreementCancellationRequest {
    /// The `ListAgreementCancellationRequests` projection of this request.
    pub fn to_summary(&self) -> AgreementCancellationRequestSummary {
        AgreementCancellationRequestSummary {
            agreement_cancellation_request_id: Some(self.id.clone()),
            agreement_id: Some(self.agreement_id.clone()),
            status: Some(self.status.clone()),
            reason_code: Some(self.reason_code.clone()),
            agreement_type: Some(self.agreement_type.clone()),
            catalog: Some(self.catalog.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
        }
    }
}

/// The kind of an accepted term, as its own `type` member spells it.
pub fn accepted_term_kind(term: &AcceptedTerm) -> &str {
    let r#type = match term {
        AcceptedTerm::legalTerm(t) => t.r#type.as_deref(),
        AcceptedTerm::supportTerm(t) => t.r#type.as_deref(),
        AcceptedTerm::renewalTerm(t) => t.r#type.as_deref(),
        AcceptedTerm::usageBasedPricingTerm(t) => t.r#type.as_deref(),
        AcceptedTerm::configurableUpfrontPricingTerm(t) => t.r#type.as_deref(),
        AcceptedTerm::byolPricingTerm(t) => t.r#type.as_deref(),
        AcceptedTerm::recurringPaymentTerm(t) => t.r#type.as_deref(),
        AcceptedTerm::validityTerm(t) => t.r#type.as_deref(),
        AcceptedTerm::paymentScheduleTerm(t) => t.r#type.as_deref(),
        AcceptedTerm::freeTrialPricingTerm(t) => t.r#type.as_deref(),
        AcceptedTerm::fixedUpfrontPricingTerm(t) => t.r#type.as_deref(),
        AcceptedTerm::variablePaymentTerm(t) => t.r#type.as_deref(),
        AcceptedTerm::netPaymentTerm(t) => t.r#type.as_deref(),
    };
    r#type.unwrap_or(DEFAULT_TERM_KIND)
}

/// One billing adjustment request: what `BatchCreateBillingAdjustmentRequest`
/// stored for one entry, and the one record `GetBillingAdjustmentRequest` and
/// `ListBillingAdjustmentRequests` report on.
///
/// `agreement_type` and `catalog` are read from the agreement the request
/// adjusts rather than from the submitting call, which carries neither: a
/// summary reports the agreement's own.
#[derive(Debug, Clone)]
pub struct BillingAdjustmentRequest {
    /// `BillingAdjustmentRequestId`'s `^ba-[a-zA-Z0-9]+$`.
    pub id: String,
    pub agreement_id: String,
    pub agreement_type: String,
    pub catalog: String,
    pub original_invoice_id: String,
    pub adjustment_amount: String,
    pub currency_code: String,
    pub adjustment_reason_code: String,
    pub description: Option<String>,
    /// The entry's `clientToken`, which is what makes a replayed submit report
    /// the request it already created.
    pub client_token: String,
    pub status: String,
    /// "This field is populated only when the status is `VALIDATION_FAILED`."
    pub status_message: Option<String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// When the `PENDING` this request was submitted in is due to become
    /// `COMPLETED`: `created_at` plus [`BILLING_ADJUSTMENT_SETTLEMENT`]. Stored
    /// rather than slept for, and read by [`ComAmazonawsMarketplaceagreementAwsmpcommerceserviceV20200301State::settle_billing_adjustments`].
    pub completes_at: SystemTime,
}

impl BillingAdjustmentRequest {
    /// Whether this request is the one `entry` asks to create — every member the
    /// entry carries beside its `clientToken` is the same.
    ///
    /// A submit replaying the same token with the same details reports this
    /// request; one reusing it for different details is a conflict.
    pub fn matches_entry(&self, entry: &BatchCreateBillingAdjustmentRequestEntry) -> bool {
        self.agreement_id == entry.agreement_id
            && self.original_invoice_id == entry.original_invoice_id
            && self.adjustment_amount == entry.adjustment_amount
            && self.currency_code == entry.currency_code
            && self.adjustment_reason_code == entry.adjustment_reason_code
            && self.description == entry.description
    }

    /// `BillingAdjustmentSummary`, as `ListBillingAdjustmentRequests` reports
    /// this request.
    pub fn to_summary(&self) -> BillingAdjustmentSummary {
        BillingAdjustmentSummary {
            billing_adjustment_request_id: self.id.clone(),
            original_invoice_id: self.original_invoice_id.clone(),
            adjustment_amount: self.adjustment_amount.clone(),
            currency_code: self.currency_code.clone(),
            status: self.status.clone(),
            agreement_id: self.agreement_id.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            updated_at: ::aws_smithy_types::DateTime::from(self.updated_at),
            agreement_type: self.agreement_type.clone(),
            catalog: self.catalog.clone(),
        }
    }

    /// The whole request, as `GetBillingAdjustmentRequest` reports it.
    pub fn to_detail(&self) -> GetBillingAdjustmentRequestOutput {
        GetBillingAdjustmentRequestOutput {
            billing_adjustment_request_id: self.id.clone(),
            agreement_id: self.agreement_id.clone(),
            adjustment_reason_code: self.adjustment_reason_code.clone(),
            description: self.description.clone(),
            original_invoice_id: self.original_invoice_id.clone(),
            adjustment_amount: self.adjustment_amount.clone(),
            currency_code: self.currency_code.clone(),
            status: self.status.clone(),
            status_message: self.status_message.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            updated_at: ::aws_smithy_types::DateTime::from(self.updated_at),
        }
    }
}

/// The scope a `ListBillingAdjustmentRequests` page token belongs to: the whole
/// query, so a token cannot resume a different listing.
///
/// Every part is length-prefixed by [`query_scope`], so no value a caller
/// supplies can imitate the delimiters and make two different listings hash
/// alike.
pub fn billing_adjustment_scope(
    agreement_id: Option<&str>,
    status: Option<&str>,
    catalog: Option<&str>,
    agreement_type: Option<&str>,
    created_after: Option<f64>,
    created_before: Option<f64>,
) -> String {
    let instant = |bound: Option<f64>| bound.map(|secs| format!("{secs:?}")).unwrap_or_default();
    query_scope(&[
        agreement_id.unwrap_or_default(),
        status.unwrap_or_default(),
        catalog.unwrap_or_default(),
        agreement_type.unwrap_or_default(),
        &instant(created_after),
        &instant(created_before),
    ])
}

// ---------------------------------------------------------------------------
// Search filters
// ---------------------------------------------------------------------------

/// Why a `SearchAgreements` filter was refused: the `ValidationException`
/// `reason` to report, and the dotted request path of the member it is about —
/// which element of `filters` carried it, and which member of that element.
#[derive(Debug)]
pub struct FilterRefusal {
    pub reason: &'static str,
    pub path: String,
}

/// The dotted request path of one member of the `index`-th element of the
/// request's `filters` list — `filters.1.member.name` for the FIRST filter's
/// `name`.
///
/// Spelled through the framework's own path rule
/// ([`::aws_smithy_schema_server::constraints::path`]), so a path this service
/// builds for a rule the model cannot express reads exactly like one the
/// boundary validator builds for a modelled constraint: the list member, the
/// element's 1-based position, the step into the element, then the member of it
/// that failed.
fn filter_member_path(index: usize, member: &str) -> String {
    use ::aws_smithy_schema_server::constraints::path;
    path::join(&[
        path::member("filters"),
        path::element(index + 1),
        path::member(member),
    ])
}

/// The dotted request path of the single value one filter carries —
/// `filters.1.member.values.1.member`. `FilterValueList` is modelled
/// `@length(min: 1, max: 1)`, so the offending value is always that list's
/// first element.
fn filter_value_path(index: usize) -> String {
    use ::aws_smithy_schema_server::constraints::path;
    path::join(&[filter_member_path(index, "values"), path::element(1)])
}

/// The instant a filter value names, parsed as the RFC-3339 timestamp the
/// awsJson protocols carry a `Timestamp` as.
fn filter_instant(value: &str) -> Option<f64> {
    ::aws_smithy_types::DateTime::from_str(value, ::aws_smithy_types::date_time::Format::DateTime)
        .ok()
        .map(|instant| instant.as_secs_f64())
}

/// `<byte length>:<value>`, which no value can forge: the part every page-token
/// scope is built from.
fn counted(value: &str) -> String {
    format!("{}:{value}", value.len())
}

/// Seconds since the epoch, for comparing a stored instant with a filter's or
/// with a `createdAfter`/`createdBefore` bound.
pub fn epoch_secs(instant: SystemTime) -> f64 {
    ::aws_smithy_types::DateTime::from(instant).as_secs_f64()
}

/// Whether `agreement` matches the one filter `name`/`value` names.
///
/// The names are the ones `SearchAgreementsInput$filters` documents
/// ([`SEARCH_FILTER_NAMES`]); each is matched against the member of the stored
/// agreement it names. A filter on something the emulator holds no value for
/// matches nothing — it does not match everything.
///
/// The filter itself is refused by [`validate_filters`] before any agreement is
/// scanned, so a name or value this cannot interpret answers `false` here
/// rather than repeating that refusal.
fn matches_filter(agreement: &Agreement, name: &str, value: &str) -> bool {
    let before =
        |stored: Option<SystemTime>| match (stored.map(epoch_secs), filter_instant(value)) {
            (Some(stored), Some(bound)) => stored < bound,
            _ => false,
        };
    let after = |stored: Option<SystemTime>| match (stored.map(epoch_secs), filter_instant(value))
    {
        (Some(stored), Some(bound)) => stored > bound,
        _ => false,
    };
    let behavior = agreement.end_time_behavior();

    match name {
        "ResourceIdentifier" => agreement
            .proposal_summary
            .resources
            .iter()
            .flatten()
            .any(|resource| resource.id.as_deref() == Some(value)),
        "ResourceType" => agreement
            .proposal_summary
            .resources
            .iter()
            .flatten()
            .any(|resource| resource.r#type.as_deref() == Some(value)),
        // "The party type of the caller": the agreement matches when the
        // caller's own account is the party the filter names.
        "PartyType" => caller_is_party(agreement, value),
        "AcceptorAccountId" => agreement.acceptor.account_id.as_deref() == Some(value),
        "OfferId" => agreement.proposal_summary.offer_id.as_deref() == Some(value),
        "OfferSetId" => agreement.proposal_summary.offer_set_id.as_deref() == Some(value),
        "Status" => agreement.status == value,
        "AgreementType" => agreement.agreement_type == value,
        "InitialAgreementId" => agreement.initial_agreement_id == value,
        "BeforeEndTime" => before(agreement.end_time),
        "AfterEndTime" => after(agreement.end_time),
        "BeforeStartTime" => before(Some(agreement.start_time)),
        "AfterStartTime" => after(Some(agreement.start_time)),
        "BeforeLastUpdateTime" => before(Some(agreement.updated_at)),
        "AfterLastUpdateTime" => after(Some(agreement.updated_at)),
        "EndTimeBehaviorType" => behavior.is_some_and(|b| b.r#type == value),
        "EndTimeBehaviorReasonCode" => {
            behavior.is_some_and(|b| b.reason_code.as_deref() == Some(value))
        }
        // "A License Manager licence granted by the agreement": the licence of
        // one of the entitlements accepting the agreement provisioned.
        "LicenseArn" => agreement
            .entitlements
            .iter()
            .any(|entitlement| entitlement.license_arn == value),
        _ => false,
    }
}

/// The name and the single value one filter carries.
///
/// `FilterValueList` is modelled `@length(min: 1, max: 1)`, so a filter that
/// carries values carries exactly one; both members are optional, and a filter
/// missing either names nothing to select on.
fn filter_parts(index: usize, filter: &Filter) -> Result<(&str, &str), FilterRefusal> {
    let name = filter.name.as_deref().ok_or_else(|| FilterRefusal {
        reason: "MISSING_FILTER_NAME",
        path: filter_member_path(index, "name"),
    })?;
    let value = filter
        .values
        .as_ref()
        .and_then(|values| values.first())
        .ok_or_else(|| FilterRefusal {
            reason: "MISSING_FILTER_VALUES",
            path: filter_member_path(index, "values"),
        })?;
    Ok((name, value))
}

/// Refuse a filter list this service cannot interpret, BEFORE any agreement is
/// scanned — so a bad filter is refused the same way whatever the store holds,
/// including when it holds nothing.
/// Each refusal names the element that carried the offending member and the
/// member of it that failed, so a caller with several filters can tell WHICH one
/// was rejected ([`filter_member_path`]).
pub fn validate_filters(filters: &[Filter]) -> Result<(), FilterRefusal> {
    for (index, filter) in filters.iter().enumerate() {
        let (name, value) = filter_parts(index, filter)?;
        if !SEARCH_FILTER_NAMES.contains(&name) {
            return Err(FilterRefusal {
                reason: "INVALID_FILTER_NAME",
                path: filter_member_path(index, "name"),
            });
        }
        if name == "PartyType" && !PARTY_TYPE_VALUES.contains(&value) {
            return Err(FilterRefusal {
                reason: "INVALID_PARTY_TYPE",
                path: filter_value_path(index),
            });
        }
        // Every `Before…`/`After…` filter selects on an instant, so its value
        // has to be one.
        if (name.starts_with("Before") || name.starts_with("After"))
            && filter_instant(value).is_none()
        {
            return Err(FilterRefusal {
                reason: "INVALID_FILTER_VALUES",
                path: filter_value_path(index),
            });
        }
    }
    Ok(())
}

/// The party type a search is scoped to: the value of its `PartyType` filter,
/// or `None` when it names none.
///
/// `Sort$sortBy` restricts two of its attributes to one party type, so the
/// attribute a search may sort on is decided by this scope rather than by the
/// agreements the search happens to select. Call [`validate_filters`] first: a
/// `PartyType` value it would refuse is not a scope.
pub fn party_type_scope(filters: &[Filter]) -> Option<&str> {
    filters
        .iter()
        .enumerate()
        .find_map(|(index, filter)| match filter_parts(index, filter) {
            Ok(("PartyType", value)) => Some(value),
            _ => None,
        })
}

/// Whether `agreement` matches every filter the search named.
///
/// Several filters are an AND, which is what makes the documented combinations
/// (`AgreementType` + `Status`, …) select. Call [`validate_filters`] first: a
/// filter it would refuse matches nothing here.
pub fn matches_filters(agreement: &Agreement, filters: &[Filter]) -> bool {
    filters.iter().enumerate().all(|(index, filter)| {
        filter_parts(index, filter).is_ok_and(|(name, value)| matches_filter(agreement, name, value))
    })
}

/// The scope a page token belongs to: every part of the query that selects what
/// the page indexes into, each part length-prefixed by [`counted`] so no value a
/// caller supplies can imitate the delimiters and make two different queries
/// hash alike.
///
/// The one spelling of that rule: [`search_scope`] and
/// [`billing_adjustment_scope`] both build their scope from it.
pub fn query_scope(parts: &[&str]) -> String {
    parts.iter().copied().map(counted).collect()
}

/// The scope a `SearchAgreements` page token belongs to: the whole query, so a
/// token cannot resume a different search.
pub fn search_scope(
    catalog: Option<&str>,
    filters: &[Filter],
    sort_by: &str,
    sort_order: &str,
) -> String {
    let mut pairs: Vec<String> = filters
        .iter()
        .map(|filter| {
            query_scope(&[
                filter.name.as_deref().unwrap_or_default(),
                filter
                    .values
                    .as_ref()
                    .and_then(|values| values.first())
                    .map(String::as_str)
                    .unwrap_or_default(),
            ])
        })
        .collect();
    pairs.sort();
    query_scope(&[
        catalog.unwrap_or_default(),
        sort_by,
        sort_order,
        &pairs.join(""),
    ])
}

/// The scope a `ListAgreementPaymentRequests` page token belongs to: every
/// parameter that selects which payment requests the list reports, so a token
/// minted for one list cannot resume another.
pub fn payment_request_scope(
    party_type: &str,
    agreement_id: Option<&str>,
    agreement_type: Option<&str>,
    catalog: Option<&str>,
    status: Option<&str>,
) -> String {
    format!(
        "{}{}{}{}{}",
        counted(party_type),
        counted(agreement_id.unwrap_or_default()),
        counted(agreement_type.unwrap_or_default()),
        counted(catalog.unwrap_or_default()),
        counted(status.unwrap_or_default())
    )
}

/// The scope a `ListAgreementCharges` page token belongs to: every parameter
/// that selects which charges the list reports, so a token minted for one list
/// cannot resume another.
///
/// Built from [`query_scope`] like every other scope in this module, so no value
/// a caller supplies can imitate the delimiters and make two different listings
/// hash alike.
pub fn charge_scope(
    agreement_id: Option<&str>,
    agreement_type: Option<&str>,
    catalog: Option<&str>,
) -> String {
    query_scope(&[
        agreement_id.unwrap_or_default(),
        agreement_type.unwrap_or_default(),
        catalog.unwrap_or_default(),
    ])
}

// ---------------------------------------------------------------------------
// Purchase orders
// ---------------------------------------------------------------------------

/// The request member every operation carrying purchase-order associations
/// names the list with (`AcceptAgreementRequestInput$purchaseOrders`,
/// `UpdatePurchaseOrdersInput$purchaseOrders`).
const PURCHASE_ORDERS_MEMBER: &str = "purchaseOrders";

/// The `ValidationExceptionReason` for a list that associates one charge twice
/// (`smithy_query(query="shape", shape="ValidationExceptionReason")` lists
/// `DUPLICATE_CHARGES`).
///
/// Spelled out rather than resolved from a member the way [`member_reason`]
/// resolves the rest: no single element of such a list is the wrong one, so the
/// reason is about the list.
pub const REASON_DUPLICATE_CHARGES: &str = "DUPLICATE_CHARGES";

/// Why one purchase-order association was refused: the `ValidationException`
/// `reason` to report, and the dotted request path of the member it is about —
/// which element of the request's `purchaseOrders` list carried it, and which
/// member of that element.
#[derive(Debug)]
pub struct PurchaseOrderRefusal {
    pub reason: String,
    pub path: String,
}

/// The dotted request path of one member of the `index`-th element of a
/// request's `purchaseOrders` list — `purchaseOrders.1.member.chargeId` for the
/// FIRST order's `chargeId`.
///
/// Spelled through the framework's own path rule
/// ([`::aws_smithy_schema_server::constraints::path`]), exactly as
/// [`filter_member_path`] is, so a path this service builds for a rule the model
/// cannot express reads like one the boundary validator builds for a modelled
/// constraint.
pub fn purchase_order_member_path(index: usize, member: &str) -> String {
    use ::aws_smithy_schema_server::constraints::path;
    path::join(&[
        path::member(PURCHASE_ORDERS_MEMBER),
        path::element(index + 1),
        path::member(member),
    ])
}

/// The ids of the charges a quote minted, which is the set a purchase order's
/// `chargeId` has to name.
///
/// `ChargeSummary$expectedCharges` carries one `ExpectedCharge` per charge the
/// requested terms imply, and `ExpectedCharge$id` is the "Unique identifier of
/// the charge for a given agreement" — so these ids, and no others, are the
/// charges of the agreement that summary belongs to.
pub fn expected_charge_ids(summary: &ChargeSummary) -> BTreeSet<&str> {
    summary
        .expected_charges
        .iter()
        .flatten()
        .filter_map(|charge| charge.id.as_deref())
        .collect()
}

/// Refuse a purchase-order association list that cannot be resolved against
/// `charges`, BEFORE anything is stored — so a list naming a charge that does
/// not exist associates nothing.
///
/// `PurchaseOrder` is "information about a purchase order association to a
/// charge within an agreement": `chargeId` is "the unique identifier of the
/// charge to associate the purchase order with" and `purchaseOrderReference` is
/// "the purchase order reference to associate with the charge". An association
/// therefore resolves only when
///
/// * the charge it names is one of the agreement's own charges — otherwise
///   there is nothing to associate with (`INVALID_CHARGE_ID`);
/// * no earlier element of the same list named that charge already — a charge
///   carries ONE purchase order reference, so two elements naming it ask for two
///   ([`REASON_DUPLICATE_CHARGES`]);
/// * it carries the reference the association exists to record — an element
///   without one associates nothing (`MISSING_PURCHASE_ORDER_REFERENCE`).
///
/// Each refusal names the element that carried the offending member and the
/// member of it that failed ([`purchase_order_member_path`]), so a caller with
/// several orders can tell WHICH one was rejected, and the member-derived
/// reasons come from that member through [`member_reason`] — so a rule this
/// service enforces is reported under the same reason the boundary validator
/// reports a modelled constraint on the same member under.
pub fn validate_purchase_orders(
    orders: &[PurchaseOrder],
    charges: &BTreeSet<&str>,
) -> Result<(), PurchaseOrderRefusal> {
    let mut associated: BTreeSet<&str> = BTreeSet::new();
    for (index, order) in orders.iter().enumerate() {
        let charge_path = purchase_order_member_path(index, "chargeId");
        if !charges.contains(order.charge_id.as_str()) {
            return Err(PurchaseOrderRefusal {
                reason: member_reason(&charge_path, RefusalKind::Invalid),
                path: charge_path,
            });
        }
        if !associated.insert(order.charge_id.as_str()) {
            return Err(PurchaseOrderRefusal {
                reason: REASON_DUPLICATE_CHARGES.to_string(),
                path: charge_path,
            });
        }
        if order.purchase_order_reference.is_none() {
            let reference_path = purchase_order_member_path(index, "purchaseOrderReference");
            return Err(PurchaseOrderRefusal {
                reason: member_reason(&reference_path, RefusalKind::Missing),
                path: reference_path,
            });
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Modelled failures
// ---------------------------------------------------------------------------

/// The source of the `requestId` every error body of this service carries.
///
/// Process-wide rather than a field of the store: an error is also answered
/// where the store is not reached at all — the boundary refusal
/// ([`validation_exception_for`], raised from `crate::service` before any
/// handler runs) and [`store_unavailable`] — and a refused READ must not write
/// to the state it refused.
static ERROR_REQUEST_IDS: AtomicU64 = AtomicU64::new(1);

/// The `requestId` for ONE error body: "The unique identifier associated with
/// the error" (`smithy_query(query="shape", shape="ValidationException")`),
/// which every exception shape this service models carries
/// (`ValidationException`, `ResourceNotFoundException`, `ConflictException`,
/// `ThrottlingException`, `AccessDeniedException`, `InternalServerException`).
///
/// `RequestId` is `@length(min: 1, max: 128)` `^[A-Za-z0-9-]+$`
/// (`smithy_query(query="shape", shape="RequestId")`), so the value is rendered
/// as a 36-character hyphenated UUID — the form AWS request identifiers take.
/// The low bits are a monotonic counter rather than randomness, which makes the
/// value unique for the life of the process without deriving anything from the
/// request it reports on: nothing a caller sends can predict or collide with
/// another error's id.
pub fn next_request_id() -> String {
    let minted = ERROR_REQUEST_IDS.fetch_add(1, Ordering::Relaxed);
    format!("00000000-0000-4000-8000-{minted:012x}")
}

/// The `ValidationExceptionReason` for a refusal this service cannot place on a
/// named member of the request: the modelled catch-all.
pub const REASON_OTHER: &str = "OTHER";

/// `ValidationException`'s own documentation, which is what the envelope states
/// when the reason names no member ("The input fails to satisfy the constraints
/// specified by the service.", `smithy_query(query="shape",
/// shape="ValidationException")`).
pub const VALIDATION_MESSAGE_FALLBACK: &str =
    "The input fails to satisfy the constraints specified by the service.";

/// What a `ValidationExceptionField` states about the member it names.
///
/// Both sentences are fixed: `name` already says WHICH member failed and
/// `reason` says what about it was wrong, and a refusal must not echo a
/// request-chosen value back into the response (the boundary validator caps its
/// own echo for the same reason). So the entry states only that the member is
/// not acceptable — invalid when it carried a value, absent when it carried
/// none, which is the distinction `ValidationExceptionReason`'s own
/// `INVALID_…`/`MISSING_…` pairs draw.
///
/// The WORDING is this service's own and is deliberately one sentence per
/// kind: no rule the model or the documentation states produces this text, and
/// the recorded AWS responses do not agree with each other on it — a recorded
/// refusal of a `partyType` on one listing operation states the sentence
/// without a closing full stop while the recorded refusal of the SAME member on
/// a sibling listing operation states it with one. So there is nothing to
/// derive and no per-member wording to guess; the machine-readable part a
/// client branches on is `reason` and `fields[].name`, which this service does
/// state per member.
pub const FIELD_INVALID: &str = "Member is invalid.";
/// See [`FIELD_INVALID`].
pub const FIELD_MISSING: &str = "Member must not be null.";

/// How a member was wrong: it carried a value the service does not accept, or
/// it carried none at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalKind {
    /// The member carried a value outside what is accepted.
    Invalid,
    /// The member was absent from the request.
    Missing,
}

impl RefusalKind {
    /// The `ValidationExceptionReason` prefix this kind reports under.
    fn reason_prefix(self) -> &'static str {
        match self {
            Self::Invalid => "INVALID",
            Self::Missing => "MISSING",
        }
    }

    /// The `ValidationExceptionField$message` this kind states.
    fn field_message(self) -> &'static str {
        match self {
            Self::Invalid => FIELD_INVALID,
            Self::Missing => FIELD_MISSING,
        }
    }

    /// The kind `reason` reports, read back off the reason a handler chose, so a
    /// refusal raised with an explicit reason states the same field message a
    /// refusal derived from a violation does.
    fn of_reason(reason: &str) -> Self {
        if reason.starts_with("MISSING_") {
            Self::Missing
        } else {
            Self::Invalid
        }
    }
}

/// `lowerCamel` as the `SCREAMING_SNAKE` word a `ValidationExceptionReason`
/// value is spelled in.
fn screaming_snake(member: &str) -> String {
    let mut out = String::with_capacity(member.len() + 4);
    for (position, character) in member.char_indices() {
        if character.is_uppercase() && position > 0 {
            out.push('_');
        }
        out.extend(character.to_uppercase());
    }
    out
}

/// A container member named in the singular, which is how a reason names the
/// element of a list (`filters` -> `FILTER_NAME`, not `FILTERS_NAME`).
fn singular(member: &str) -> &str {
    member.strip_suffix('s').unwrap_or(member)
}

/// The member-name segments of a dotted request path, dropping the positions and
/// collection steps the path spells between them
/// ([`::aws_smithy_schema_server::constraints::path`]).
fn path_members(path: &str) -> Vec<&str> {
    path.split('.')
        .filter(|segment| *segment != "member" && !segment.chars().all(|c| c.is_ascii_digit()))
        .collect()
}

/// The `ValidationExceptionReason` that names the member at `path` failing in
/// `kind`.
///
/// `ValidationExceptionReason` names its members: every value is
/// `INVALID_<MEMBER>` or `MISSING_<MEMBER>` for a member of some operation's
/// input (`INVALID_AGREEMENT_ID`, `MISSING_FILTER_VALUES`,
/// `INVALID_SORT_BY`, …, `smithy_query(query="shape",
/// shape="ValidationExceptionReason")`). So the reason for a refused member is
/// resolved FROM the member: the longest tail of the path's member names that
/// spells a modelled value wins — which is what distinguishes the `name` of a
/// `filters` element (`INVALID_FILTER_NAME`) from a top-level `name`
/// (`INVALID_NAME`) — and a member no value names reports the modelled
/// catch-all [`REASON_OTHER`].
pub fn member_reason(path: &str, kind: RefusalKind) -> String {
    let members = path_members(path);
    let prefix = kind.reason_prefix();
    for take in (1..=members.len()).rev() {
        let tail = &members[members.len() - take..];
        // A container is named in the singular by a reason, so both spellings
        // are tried, the singular first: `filters.1.member.name` is
        // `INVALID_FILTER_NAME`, while `filters` alone is `INVALID_FILTERS`.
        for shorten_containers in [true, false] {
            let stem: Vec<String> = tail
                .iter()
                .enumerate()
                .map(|(position, member)| {
                    let container = position + 1 < tail.len();
                    screaming_snake(if shorten_containers && container {
                        singular(member)
                    } else {
                        member
                    })
                })
                .collect();
            let candidate = format!("{prefix}_{}", stem.join("_"));
            if crate::ops::types::VALIDATION_EXCEPTION_REASON_VALUES
                .contains(&candidate.as_str())
            {
                return candidate;
            }
        }
    }
    REASON_OTHER.to_string()
}

/// The `message` a `ValidationException` carrying `reason` states.
///
/// One sentence per reason, built from the reason itself — so the member the
/// request got wrong is what the sentence names, and every refusal of the same
/// member reads alike whether the boundary validator or a handler found it. A
/// reason that names no member states `ValidationException`'s own
/// documentation.
pub fn validation_message(reason: &str) -> String {
    /// `INVALID_AGREEMENT_ID` → `agreement id`.
    fn words(stem: &str) -> String {
        stem.to_lowercase().replace('_', " ")
    }

    if let Some(stem) = reason.strip_prefix("INVALID_") {
        format!("Provided {} is invalid.", words(stem))
    } else if let Some(stem) = reason.strip_prefix("MISSING_") {
        format!("Provided {} is missing.", words(stem))
    } else {
        VALIDATION_MESSAGE_FALLBACK.to_string()
    }
}

/// A `ValidationException` naming the member that was wrong and why: the
/// modelled `reason` a client branches on, and one `fields` entry naming the
/// offending member's dotted request path.
///
/// `reason` is one of `ValidationExceptionReason`'s values
/// (`VALIDATION_EXCEPTION_REASON_VALUES` in `types`); `path` is the member's
/// path, which for a member of a list element carries the element's position
/// (`filters.1.member.name`).
pub fn validation_exception(reason: &str, path: impl Into<String>) -> ValidationException {
    ValidationException {
        request_id: Some(next_request_id()),
        message: Some(validation_message(reason)),
        reason: Some(reason.to_string()),
        fields: Some(vec![ValidationExceptionField {
            name: path.into(),
            message: RefusalKind::of_reason(reason).field_message().to_string(),
        }]),
    }
}

/// The `ValidationException` for constraint violations the boundary validator
/// found in the request, BEFORE any handler ran.
///
/// Every modelled constraint — a required member left out, a value outside a
/// declared length, range, pattern or value set, at any depth of the input —
/// arrives here as a [`::smithy_server_runtime::Violation`] carrying the
/// offending member's dotted path, and is stated in this service's own modelled
/// envelope: `reason` from the FIRST violation's member (the exception carries
/// one reason, and that is the member it is about) and one `fields` entry per
/// offending MEMBER. A member that misses two constraints at once (an empty
/// string is both shorter than its minimum and outside its pattern) is one
/// member and is named once: the entry says which member to fix, so a second
/// identical entry carries nothing. The count is already bounded by the
/// validator's own walk, which stops collecting at its `MAX_VIOLATIONS`.
pub fn validation_exception_for(
    violations: &[::smithy_server_runtime::Violation],
) -> ValidationException {
    let reason = violations
        .first()
        .map(|violation| member_reason(&violation.path, violation_kind(violation)))
        .unwrap_or_else(|| REASON_OTHER.to_string());
    let mut named: Vec<&str> = Vec::with_capacity(violations.len());
    let mut fields: Vec<ValidationExceptionField> = Vec::with_capacity(violations.len());
    for violation in violations {
        if named.contains(&violation.path.as_str()) {
            continue;
        }
        named.push(&violation.path);
        fields.push(ValidationExceptionField {
            name: violation.path.clone(),
            message: violation_kind(violation).field_message().to_string(),
        });
    }
    ValidationException {
        request_id: Some(next_request_id()),
        message: Some(validation_message(&reason)),
        reason: Some(reason),
        fields: Some(fields),
    }
}

/// Whether a violation is about a member that carried no value at all.
fn violation_kind(violation: &::smithy_server_runtime::Violation) -> RefusalKind {
    match violation.kind {
        ::smithy_server_runtime::ViolationKind::MissingRequiredMember => RefusalKind::Missing,
        _ => RefusalKind::Invalid,
    }
}

/// The HTTP status this service answers a reference to an id that names no
/// resource with, for EVERY resource it holds.
///
/// `ResourceNotFoundException` is the shape such a reference is answered with —
/// it is the one that carries `resourceId` and `resourceType`, naming what was
/// looked for — and the model declares it `@httpError(404)`. The status this
/// service actually sends with that body is 400: the recorded AWS responses for
/// `AWSMPCommerceService_v20200301` answer an unknown agreement and an unknown
/// agreement request with HTTP 400 carrying the `ResourceNotFoundException`
/// body, so the model's `@httpError` understates what the wire does. The status
/// belongs to the SERVICE and not to those two resources — one condition cannot
/// read as two status classes depending on which of this service's resources was
/// looked for — so every lookup in this crate answers through [`not_found`].
/// Only the STATUS is this service's; the body and the error code stay the
/// model's, which is exactly what
/// [`crate::scaffolded::Scaffolded::ModeledWithStatus`] renders.
pub const NOT_FOUND_STATUS: u16 = 400;

/// Lift a not-found refusal into an operation's error channel at
/// [`NOT_FOUND_STATUS`], so every lookup that found nothing answers the one
/// status this service answers them with.
pub fn not_found<E>(error: E) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::modeled_with_status(error, NOT_FOUND_STATUS)
}

/// How this service words a reference to a resource that does not exist, for
/// the resource noun its message names.
///
/// One sentence per resource kind, naming the KIND and not the id — the id the
/// caller supplied is reported in `resourceId`, which is the modelled place for
/// it, so nothing about the request is lost.
fn does_not_exist(resource: &str) -> String {
    format!("Provided {resource} does not exist.")
}

/// How this service states a request whose filters cannot hold together with
/// the rest of the request.
///
/// Not derived from a member the way [`validation_message`] derives its
/// sentence: no single member of such a request is the wrong one, so the
/// sentence is about the combination.
pub const UNSUPPORTED_FILTER_COMBINATION_MESSAGE: &str =
    "Provided combination of filters is not supported.";

/// The `ValidationExceptionReason` for filters that are each acceptable but
/// cannot hold together with the rest of the request
/// (`smithy_query(query="shape", shape="ValidationExceptionReason")` lists
/// `UNSUPPORTED_FILTERS`).
pub const REASON_UNSUPPORTED_FILTERS: &str = "UNSUPPORTED_FILTERS";

/// A `ValidationException` for a request whose members are each acceptable but
/// cannot hold together — the modelled `UNSUPPORTED_FILTERS` reason.
///
/// `path` is the member whose pairing with the filters is unsupported, so a
/// client still reads WHICH part of the request to change from `fields`, while
/// `message` states the combination rather than one bad member.
pub fn unsupported_filter_combination(path: impl Into<String>) -> ValidationException {
    ValidationException {
        request_id: Some(next_request_id()),
        message: Some(UNSUPPORTED_FILTER_COMBINATION_MESSAGE.to_string()),
        reason: Some(REASON_UNSUPPORTED_FILTERS.to_string()),
        fields: Some(vec![ValidationExceptionField {
            name: path.into(),
            message: FIELD_INVALID.to_string(),
        }]),
    }
}

/// The `ThrottlingException` a create answers when its store already holds the
/// most records the emulator keeps: the request is declined, and nothing is
/// stored, rather than letting state grow without end.
pub fn store_full(resource_type: &str, quota: usize) -> ThrottlingException {
    ThrottlingException {
        request_id: Some(next_request_id()),
        message: Some(format!(
            "this emulator already holds the maximum of {quota} {resource_type} records; \
             release some before creating another"
        )),
    }
}

/// The `ResourceNotFoundException` for an agreement id that names no agreement.
pub fn agreement_not_found(id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        request_id: Some(next_request_id()),
        message: Some(does_not_exist("agreement")),
        resource_id: Some(id.to_string()),
        resource_type: Some(RESOURCE_TYPE_AGREEMENT.to_string()),
    }
}

/// The `ResourceNotFoundException` for an agreement request id that names none.
pub fn agreement_request_not_found(id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        request_id: Some(next_request_id()),
        message: Some(does_not_exist("agreement request")),
        resource_id: Some(id.to_string()),
        resource_type: Some(RESOURCE_TYPE_AGREEMENT_REQUEST.to_string()),
    }
}

/// The `ResourceNotFoundException` for an agreement and billing adjustment
/// request id pair that names no stored request.
pub fn billing_adjustment_not_found(id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        request_id: Some(next_request_id()),
        message: Some(does_not_exist("billing adjustment request")),
        resource_id: Some(id.to_string()),
        resource_type: Some(RESOURCE_TYPE_BILLING_ADJUSTMENT_REQUEST.to_string()),
    }
}

/// A `ConflictException` for an operation the resource's current state refuses.
pub fn conflict(resource_type: &str, id: &str, detail: impl Into<String>) -> ConflictException {
    ConflictException {
        request_id: Some(next_request_id()),
        message: Some(detail.into()),
        resource_id: Some(id.to_string()),
        resource_type: Some(resource_type.to_string()),
    }
}

/// The audit record a state-changing operation owes: which operation ran, what
/// it did, the resource it did it to, and the access key id the caller
/// presented (an identifier, never a secret).
///
/// Emitted under the `audit` target so a deployment can route these records
/// apart from the emulator's own diagnostics.
pub fn audit(operation: &str, outcome: &str, resource: &str, parts: &::http::request::Parts) {
    ::tracing::info!(
        target: "audit",
        operation,
        outcome,
        resource,
        principal = %::engine::extract_akid(&parts.headers),
        "marketplace agreement state change"
    );
}

/// The `InternalServerException` a handler answers when the engine does not
/// carry this service's store — never a freshly built engine.
pub fn store_unavailable() -> InternalServerException {
    InternalServerException {
        request_id: Some(next_request_id()),
        message: Some("the marketplace agreement store is unavailable".to_string()),
    }
}

// ---------------------------------------------------------------------------
// The store
// ---------------------------------------------------------------------------

/// The key [`ComAmazonawsMarketplaceagreementAwsmpcommerceserviceV20200301State::idempotency`]
/// holds an operation's `clientToken` under.
///
/// A `clientToken` is unique within the create that minted it, not across the
/// service, so the operation's own name is part of the key. The separator is a
/// character `ClientToken`'s `^[a-zA-Z0-9-]+$` forbids, so no token a caller
/// supplies can forge another operation's key.
pub fn idempotency_key(operation: &str, client_token: &str) -> String {
    format!("{operation}#{client_token}")
}

/// Per-service state for com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsMarketplaceagreementAwsmpcommerceserviceV20200301State {
    /// Every agreement request, keyed by its `^ar-[A-Za-z0-9]+$` id.
    pub agreement_requests: BTreeMap<String, AgreementRequest>,
    /// Every agreement, keyed by its id. A `BTreeMap` is what gives
    /// `SearchAgreements` a stable total order to page over: ids are minted from
    /// a zero-padded counter, so they sort in creation order.
    pub agreements: BTreeMap<String, Agreement>,
    /// Every payment request, keyed by its `^pr-[a-zA-Z0-9]+$` id. A `BTreeMap`
    /// gives `ListAgreementPaymentRequests` the same stable total order to page
    /// over that `agreements` gives `SearchAgreements`. Bounded by
    /// [`PAYMENT_REQUEST_QUOTA`], which `SendAgreementPaymentRequest` checks
    /// before it stores one.
    pub payment_requests: BTreeMap<String, PaymentRequest>,
    /// Every agreement cancellation request, keyed by its
    /// `^acr-[a-zA-Z0-9]+$` id. A `BTreeMap` is what gives
    /// `ListAgreementCancellationRequests` a stable total order to page over:
    /// ids are minted from a zero-padded counter, so they sort in creation
    /// order.
    pub agreement_cancellation_requests: BTreeMap<String, AgreementCancellationRequest>,
    /// Every billing adjustment request, keyed by its `^ba-[a-zA-Z0-9]+$` id.
    /// A `BTreeMap` is what gives `ListBillingAdjustmentRequests` a stable total
    /// order to page over: ids are minted from a zero-padded counter, so they
    /// sort in creation order.
    pub billing_adjustment_requests: BTreeMap<String, BillingAdjustmentRequest>,
    /// An `@idempotencyToken` `clientToken` to the resource it created, so a
    /// replayed create answers with the first resource instead of minting a
    /// second one. `CreateAgreementRequest` keys it by the token itself;
    /// `SendAgreementCancellationRequest` keys it by [`idempotency_key`], which
    /// no `ClientToken` (`^[a-zA-Z0-9-]+$`) can imitate, so the two creates
    /// cannot collide on one token.
    pub idempotency: HashMap<String, String>,
    /// The counter every id this service mints is drawn from.
    pub id_sequence: u64,
}

impl ComAmazonawsMarketplaceagreementAwsmpcommerceserviceV20200301State {
    /// The next value of the shared id counter.
    fn next_sequence(&mut self) -> u64 {
        self.id_sequence = self.id_sequence.wrapping_add(1);
        self.id_sequence
    }

    /// Mint an unused agreement request id matching `AgreementRequestId`'s
    /// `^ar-[A-Za-z0-9]+$`.
    pub fn mint_agreement_request_id(&mut self) -> String {
        loop {
            let id = format!("ar-{:016x}", self.next_sequence());
            if !self.agreement_requests.contains_key(&id) {
                return id;
            }
        }
    }

    /// Mint an unused cancellation request id matching
    /// `AgreementCancellationRequestId`'s `^acr-[a-zA-Z0-9]+$`.
    pub fn mint_agreement_cancellation_request_id(&mut self) -> String {
        loop {
            let id = format!("acr-{:016x}", self.next_sequence());
            if !self.agreement_cancellation_requests.contains_key(&id) {
                return id;
            }
        }
    }

    /// Mint an unused agreement id matching `ResourceId`'s
    /// `^[A-Za-z0-9_/-]+$`.
    pub fn mint_agreement_id(&mut self) -> String {
        loop {
            let id = format!("agmt-{:016x}", self.next_sequence());
            if !self.agreements.contains_key(&id) {
                return id;
            }
        }
    }

    /// Mint an unused payment request id matching `PaymentRequestId`'s
    /// `^pr-[a-zA-Z0-9]+$`.
    pub fn mint_payment_request_id(&mut self) -> String {
        loop {
            let id = format!("pr-{:016x}", self.next_sequence());
            if !self.payment_requests.contains_key(&id) {
                return id;
            }
        }
    }

    /// Mint an unused billing adjustment request id matching
    /// `BillingAdjustmentRequestId`'s `^ba-[a-zA-Z0-9]+$`.
    pub fn mint_billing_adjustment_request_id(&mut self) -> String {
        loop {
            let id = format!("ba-{:016x}", self.next_sequence());
            if !self.billing_adjustment_requests.contains_key(&id) {
                return id;
            }
        }
    }

    /// Mint the ARN of the AWS License Manager licence one entitlement grants.
    ///
    /// The licence id carries License Manager's own `l-` prefix, and the ARN is
    /// built through [`::engine::emulator_arn`], so its account segment is the
    /// emulator's canonical one and its region is the one the accepting caller
    /// signed for. Every ARN this mints satisfies `AwsArn`'s pattern, whose
    /// resource part admits the `license:<id>` form License Manager uses.
    fn mint_license_arn(&mut self, region: &str) -> String {
        let license_id = format!("l-{:016x}", self.next_sequence());
        ::engine::emulator_arn(
            LICENSE_ARN_SERVICE,
            region,
            &format!("license:{license_id}"),
        )
    }

    /// The entitlements accepting an agreement provisions, each with its own
    /// freshly minted licence and the instant its provisioning is due to finish.
    ///
    /// `AgreementEntitlement` "represents an entitlement associated with an
    /// agreement, including the provisioning status, resource, and type", and
    /// `Resource` is "the resource that the entitlement is provisioned to, such
    /// as a product" — so an agreement grants one entitlement per resource its
    /// proposal names. An agreement whose proposal names no resource still
    /// entitles its acceptor to what it accepted, so it provisions the single
    /// entitlement it grants, with `resource` absent rather than an invented
    /// product (see [`AgreementEntitlementRecord::resource`]).
    pub fn provision_entitlements(
        &mut self,
        resources: Option<&[Resource]>,
        region: &str,
        now: SystemTime,
    ) -> Vec<AgreementEntitlementRecord> {
        let provisioned_at = now + ENTITLEMENT_PROVISIONING;
        let provisioned_to: Vec<Option<Resource>> = match resources {
            Some(resources) if !resources.is_empty() => {
                resources.iter().cloned().map(Some).collect()
            }
            _ => vec![None],
        };
        provisioned_to
            .into_iter()
            .map(|resource| AgreementEntitlementRecord {
                resource,
                license_arn: self.mint_license_arn(region),
                provisioned_at,
            })
            .collect()
    }

    /// The payment request `token` already sent, if any.
    ///
    /// `SendAgreementPaymentRequestInput$clientToken` is modelled
    /// `@idempotencyToken`, so a replayed token names the request it first
    /// minted. The token lives on the record itself rather than in a second map,
    /// so the two can never diverge and the lookup is bounded by the same
    /// [`PAYMENT_REQUEST_QUOTA`] that bounds the store.
    pub fn payment_request_for_token(&self, token: &str) -> Option<&PaymentRequest> {
        self.payment_requests
            .values()
            .find(|request| request.client_token.as_deref() == Some(token))
    }

    /// The stored billing adjustment request submitted under `token`, if any.
    ///
    /// The token is held on the request itself rather than in a second index, so
    /// there is exactly one copy of each request to read.
    pub fn billing_adjustment_by_client_token(
        &self,
        token: &str,
    ) -> Option<&BillingAdjustmentRequest> {
        self.billing_adjustment_requests
            .values()
            .find(|request| request.client_token == token)
    }

    /// Materialise every billing adjustment status change the clock has already
    /// made due by `now`.
    ///
    /// A request is submitted `PENDING` and AWS Marketplace processes it; the
    /// processing is owed at `completes_at` whether or not anybody calls, so a
    /// reporting operation materialises it before it reports. `updated_at`
    /// becomes the instant the change was DUE, never the instant it was
    /// observed, so repeated reads report the same value.
    pub fn settle_billing_adjustments(&mut self, now: SystemTime) {
        for request in self.billing_adjustment_requests.values_mut() {
            if request.status == BILLING_ADJUSTMENT_PENDING && now >= request.completes_at {
                request.status = BILLING_ADJUSTMENT_COMPLETED.to_string();
                request.updated_at = request.completes_at;
            }
        }
    }

    /// The ids of the charges `agreement` holds.
    ///
    /// A charge belongs to the agreement whose quote minted it: the charges of
    /// an agreement are the `ChargeSummary$expectedCharges` of the agreement
    /// request it was accepted from ([`expected_charge_ids`]), which is the one
    /// copy of them this store keeps — nothing re-derives a charge id, so a
    /// charge cannot be reported under two identities.
    pub fn charge_ids_of<'a>(&'a self, agreement: &Agreement) -> BTreeSet<&'a str> {
        self.agreement_requests
            .get(&agreement.agreement_request_id)
            .map(|request| expected_charge_ids(&request.charge_summary))
            .unwrap_or_default()
    }

    /// Every charge this store holds, mapped to the id of the agreement holding
    /// it.
    ///
    /// Charge ids are minted from the store's single counter
    /// ([`Self::mint_charge_id`]), so no charge belongs to two agreements and
    /// this map IS the resolution of a `chargeId` a request names to the
    /// agreement whose charge it is — what `UpdatePurchaseOrders` needs, since
    /// its input names charges without naming an agreement.
    pub fn charge_owners(&self) -> BTreeMap<String, String> {
        let mut owners: BTreeMap<String, String> = BTreeMap::new();
        for agreement in self.agreements.values() {
            for charge_id in self.charge_ids_of(agreement) {
                owners.insert(charge_id.to_string(), agreement.id.clone());
            }
        }
        owners
    }

    /// The charges of `agreement`, in the order its quote minted them, as
    /// `ListAgreementCharges` reports them.
    ///
    /// Every member is read from the one copy of the resource it belongs to:
    /// the charge's identity and timing from the quote's own `ExpectedCharge`,
    /// the agreement's id and type from the agreement, and the purchase order
    /// reference from the association recorded on that agreement's charge —
    /// absent while the charge carries none, which is what "The purchase order
    /// reference associated with the charge, if any" states.
    pub fn charges_of(&self, agreement: &Agreement) -> Vec<Charge> {
        let Some(request) = self.agreement_requests.get(&agreement.agreement_request_id) else {
            return Vec::new();
        };
        request
            .charge_summary
            .expected_charges
            .iter()
            .flatten()
            .filter_map(|expected| {
                let id = expected.id.as_deref()?;
                Some(Charge {
                    id: Some(id.to_string()),
                    revision: Some(INITIAL_CHARGE_REVISION),
                    agreement_id: Some(agreement.id.clone()),
                    agreement_type: Some(agreement.agreement_type.clone()),
                    purchase_order_reference: agreement
                        .purchase_orders
                        .iter()
                        .find(|order| order.charge_id == id)
                        .and_then(|order| order.purchase_order_reference.clone()),
                    // The amount and the currency it is priced in come from the
                    // proposal's rate card, which this emulator does not hold
                    // (see the module docs), so both are read from the quote's
                    // own charge rather than invented — and the quote carries
                    // neither.
                    currency_code: request.charge_summary.currency_code.clone(),
                    amount: expected.amount.clone(),
                    // "available only when the charge date is known", which is
                    // exactly when the quote's expected charge carries one.
                    time: expected.time,
                })
            })
            .collect()
    }

    /// Mint a charge id for an expected charge of a quote. `ExpectedCharge$id`
    /// is a `ResourceId`; AWS Marketplace charge ids carry the `ch-` prefix the
    /// payment-request operations model (`^ch-[a-zA-Z0-9]+$`).
    fn mint_charge_id(&mut self) -> String {
        format!("ch-{:016x}", self.next_sequence())
    }

    /// The charges `requested_terms` imply, as `CreateAgreementRequest` returns
    /// them.
    ///
    /// Every member is computed from the request and the store: one expected
    /// charge per requested term that is charged at all (with the timing its
    /// kind implies), one itemized charge per dimension the acceptor configured
    /// (with the quantity it asked for, and the quantity the source agreement
    /// already holds for an amendment), and the agreement value where the model
    /// defines it without a rate card. See the module docs for why no amount is
    /// reported.
    pub fn charge_summary(
        &mut self,
        requested_terms: &[RequestedTerm],
        intent: &str,
        source_agreement_id: Option<&str>,
    ) -> ChargeSummary {
        // "This value is 0 for NEW intent"; an amendment or replacement reads
        // the quantities the source agreement already holds.
        let old_quantities = source_agreement_id
            .filter(|_| intent != INTENT_NEW)
            .and_then(|id| self.agreements.get(id))
            .map(|source| dimension_quantities(&source.accepted_terms))
            .unwrap_or_default();

        let mut expected_charges: Vec<ExpectedCharge> = Vec::new();
        let mut itemized_charges: Vec<ItemizedCharge> = Vec::new();
        for requested in requested_terms {
            let kind = term_kind(requested);
            let Some(timing) = charge_timing(kind) else {
                continue;
            };
            let charge_id = self.mint_charge_id();
            expected_charges.push(ExpectedCharge {
                id: Some(charge_id.clone()),
                // "The date and time when the charge is due to be invoiced.
                // This is available only when the charge date is known" — a
                // quote has not been accepted yet, so no charge date is.
                time: None,
                amount: None,
                amount_after_tax: None,
                timing: Some(timing.to_string()),
                estimated_taxes: None,
            });
            if let Some(RequestedTermConfiguration::configurableUpfrontPricingTermConfiguration(
                configuration,
            )) = requested.configuration.as_ref()
            {
                for dimension in &configuration.dimensions {
                    itemized_charges.push(ItemizedCharge {
                        dimension_key: Some(dimension.dimension_key.clone()),
                        new_quantity: Some(dimension.dimension_value),
                        old_quantity: Some(
                            old_quantities
                                .get(&dimension.dimension_key)
                                .copied()
                                .unwrap_or_default(),
                        ),
                        charge_reference: Some(charge_id.clone()),
                        incremental_charge_amount: None,
                    });
                }
            }
        }

        let committed = requested_terms
            .iter()
            .any(|requested| commits_amount(term_kind(requested)));
        ChargeSummary {
            currency_code: None,
            new_agreement_value: (!committed).then(|| ZERO_AGREEMENT_VALUE.to_string()),
            new_agreement_value_after_tax: None,
            expected_charges: (!expected_charges.is_empty()).then_some(expected_charges),
            estimated_taxes: None,
            itemized_charges: (!itemized_charges.is_empty()).then_some(itemized_charges),
            invoicing_entity: None,
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMarketplaceagreementAwsmpcommerceserviceV20200301State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMarketplaceagreementAwsmpcommerceserviceV20200301State>)
}
