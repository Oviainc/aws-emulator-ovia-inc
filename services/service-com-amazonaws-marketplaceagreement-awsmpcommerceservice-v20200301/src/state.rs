// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301 service.
//!
//! Two resources carry the agreement lifecycle this crate serves:
//!
//! * an **agreement request** — "a quote for the terms you want to accept"
//!   (`CreateAgreementRequest`), which captures the requested terms and the
//!   charges they imply;
//! * an **agreement** — what `AcceptAgreementRequest` finalises from one of
//!   those requests, and what `DescribeAgreement`, `GetAgreementTerms`,
//!   `SearchAgreements` and `CancelAgreement` report on and mutate.
//!
//! Every projection the model asks for (`DescribeAgreementOutput`,
//! `AgreementViewSummary`, `ChargeSummary`, `AcceptedTerm`) is computed from
//! these records, so the six implemented operations share exactly one copy of
//! each resource.
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

use std::collections::{BTreeMap, HashMap};
use std::time::SystemTime;

use sha2::{Digest, Sha256};

use crate::ops::types::{
    AcceptedTerm, Acceptor, AgreementViewSummary, ByolPricingTerm, ChargeSummary,
    ConfigurableUpfrontPricingTerm, ConflictException, DescribeAgreementOutput, EndTimeBehavior,
    EstimatedCharges, ExpectedCharge, Filter, FixedUpfrontPricingTerm, FreeTrialPricingTerm,
    InternalServerException, ItemizedCharge, LegalTerm, NetPaymentTerm, PaymentScheduleTerm,
    ProposalSummary, Proposer, PurchaseOrder, RecurringPaymentTerm, RenewalTerm, RequestedTerm,
    RequestedTermConfiguration, ResourceNotFoundException, SupportTerm, TaxConfiguration,
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

/// `ResourceType` values this service reports on a `ResourceNotFoundException`
/// or a `ConflictException` (`RESOURCE_TYPE_VALUES` in `types`).
pub const RESOURCE_TYPE_AGREEMENT: &str = "Agreement";
/// See [`RESOURCE_TYPE_AGREEMENT`].
pub const RESOURCE_TYPE_AGREEMENT_REQUEST: &str = "AgreementRequest";

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

/// The `quotaCode` reported with that refusal. Named for what it bounds: a real
/// quota code is an identifier Service Quotas issues, which this emulator does
/// not model and so must not invent one of.
pub const AGREEMENT_REQUEST_QUOTA_CODE: &str = "AgreementRequestsPerAccount";

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

/// `Timing`'s values (`TIMING_VALUES` in `types`), with the meaning
/// `ExpectedCharge$timing` documents: `ON_ACCEPTANCE` is "charged immediately
/// when the agreement request is accepted", `BILLING_PERIOD` is "charged on each
/// billing period" and `SCHEDULED` is "charged at a predetermined future date".
pub const TIMING_ON_ACCEPTANCE: &str = "ON_ACCEPTANCE";
/// See [`TIMING_ON_ACCEPTANCE`].
pub const TIMING_BILLING_PERIOD: &str = "BILLING_PERIOD";
/// See [`TIMING_ON_ACCEPTANCE`].
pub const TIMING_SCHEDULED: &str = "SCHEDULED";

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
pub const PARTY_TYPE_ACCEPTOR: &str = "Acceptor";
/// See [`PARTY_TYPE_ACCEPTOR`].
pub const PARTY_TYPE_PROPOSER: &str = "Proposer";

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
            // An entitlement is a License Manager licence granted by the
            // agreement; `GetAgreementEntitlements`, which is what provisions
            // and reports them, is still a stub, so none exists to report.
            entitlements: None,
            initial_agreement_id: Some(self.initial_agreement_id.clone()),
            end_time_behavior_type: behavior.as_ref().map(|b| b.r#type.clone()),
            end_time_behavior_reason_code: behavior.and_then(|b| b.reason_code),
        }
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

// ---------------------------------------------------------------------------
// Search filters
// ---------------------------------------------------------------------------

/// Why a `SearchAgreements` filter was refused: the `ValidationException`
/// `reason` to report, and the member it is about.
#[derive(Debug)]
pub struct FilterRefusal {
    pub reason: &'static str,
    pub detail: String,
}

/// The instant a filter value names, parsed as the RFC-3339 timestamp the
/// awsJson protocols carry a `Timestamp` as.
fn filter_instant(value: &str) -> Option<f64> {
    ::aws_smithy_types::DateTime::from_str(value, ::aws_smithy_types::date_time::Format::DateTime)
        .ok()
        .map(|instant| instant.as_secs_f64())
}

/// Seconds since the epoch, for comparing a stored instant with a filter's.
fn epoch_secs(instant: SystemTime) -> f64 {
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
        // caller's own account is the party the filter names. This emulator
        // serves one account, so a search as the acceptor finds the agreements
        // it accepted, and one as the proposer finds those it proposed — none,
        // until a proposer account is ever stored.
        "PartyType" => {
            let caller = Some(::engine::emulator_account_id());
            match value {
                PARTY_TYPE_ACCEPTOR => agreement.acceptor.account_id.as_deref() == caller,
                PARTY_TYPE_PROPOSER => agreement.proposer.account_id.as_deref() == caller,
                _ => false,
            }
        }
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
        // A License Manager licence granted by the agreement; see
        // `AgreementViewSummary$entitlements` above for why none exists yet.
        "LicenseArn" => false,
        _ => false,
    }
}

/// The name and the single value one filter carries.
///
/// `FilterValueList` is modelled `@length(min: 1, max: 1)`, so a filter that
/// carries values carries exactly one; both members are optional, and a filter
/// missing either names nothing to select on.
fn filter_parts(filter: &Filter) -> Result<(&str, &str), FilterRefusal> {
    let name = filter.name.as_deref().ok_or_else(|| FilterRefusal {
        reason: "MISSING_FILTER_NAME",
        detail: "every filter must name the attribute it selects on".to_string(),
    })?;
    let value = filter
        .values
        .as_ref()
        .and_then(|values| values.first())
        .ok_or_else(|| FilterRefusal {
            reason: "MISSING_FILTER_VALUES",
            detail: format!("filter {name} must carry a value"),
        })?;
    Ok((name, value))
}

/// Refuse a filter list this service cannot interpret, BEFORE any agreement is
/// scanned — so a bad filter is refused the same way whatever the store holds,
/// including when it holds nothing.
pub fn validate_filters(filters: &[Filter]) -> Result<(), FilterRefusal> {
    for filter in filters {
        let (name, value) = filter_parts(filter)?;
        if !SEARCH_FILTER_NAMES.contains(&name) {
            return Err(FilterRefusal {
                reason: "INVALID_FILTER_NAME",
                detail: format!(
                    "filter {name:?} is not one of {}",
                    SEARCH_FILTER_NAMES.join(", ")
                ),
            });
        }
        if name == "PartyType" && !matches!(value, PARTY_TYPE_ACCEPTOR | PARTY_TYPE_PROPOSER) {
            return Err(FilterRefusal {
                reason: "INVALID_PARTY_TYPE",
                detail: format!(
                    "filter PartyType accepts {PARTY_TYPE_PROPOSER} or {PARTY_TYPE_ACCEPTOR}, not {value:?}"
                ),
            });
        }
        // Every `Before…`/`After…` filter selects on an instant, so its value
        // has to be one.
        if (name.starts_with("Before") || name.starts_with("After"))
            && filter_instant(value).is_none()
        {
            return Err(FilterRefusal {
                reason: "INVALID_FILTER_VALUES",
                detail: format!("filter {name} needs a date and time, not {value:?}"),
            });
        }
    }
    Ok(())
}

/// Whether `agreement` matches every filter the search named.
///
/// Several filters are an AND, which is what makes the documented combinations
/// (`AgreementType` + `Status`, …) select. Call [`validate_filters`] first: a
/// filter it would refuse matches nothing here.
pub fn matches_filters(agreement: &Agreement, filters: &[Filter]) -> bool {
    filters.iter().all(|filter| {
        filter_parts(filter).is_ok_and(|(name, value)| matches_filter(agreement, name, value))
    })
}

/// The scope a `SearchAgreements` page token belongs to: the whole query, so a
/// token cannot resume a different search.
/// Every part is length-prefixed, so no value a caller supplies can imitate the
/// delimiters and make two different searches hash alike.
pub fn search_scope(
    catalog: Option<&str>,
    filters: &[Filter],
    sort_by: &str,
    sort_order: &str,
) -> String {
    /// `<byte length>:<value>`, which no value can forge.
    fn counted(value: &str) -> String {
        format!("{}:{value}", value.len())
    }

    let mut pairs: Vec<String> = filters
        .iter()
        .map(|filter| {
            format!(
                "{}{}",
                counted(filter.name.as_deref().unwrap_or_default()),
                counted(
                    filter
                        .values
                        .as_ref()
                        .and_then(|values| values.first())
                        .map(String::as_str)
                        .unwrap_or_default()
                )
            )
        })
        .collect();
    pairs.sort();
    format!(
        "{}{}{}{}",
        counted(catalog.unwrap_or_default()),
        counted(sort_by),
        counted(sort_order),
        counted(&pairs.join(""))
    )
}

// ---------------------------------------------------------------------------
// Modelled failures
// ---------------------------------------------------------------------------

/// A `ValidationException` naming the member that was wrong and why.
///
/// `reason` is one of `ValidationExceptionReason`'s values
/// (`VALIDATION_EXCEPTION_REASON_VALUES` in `types`), which is what a client
/// branches on.
pub fn validation_exception(
    reason: &str,
    member: &str,
    detail: impl Into<String>,
) -> ValidationException {
    let detail = detail.into();
    ValidationException {
        request_id: None,
        message: Some(format!("{member}: {detail}")),
        reason: Some(reason.to_string()),
        fields: Some(vec![ValidationExceptionField {
            name: member.to_string(),
            message: detail,
        }]),
    }
}

/// The `ResourceNotFoundException` for an agreement id that names no agreement.
pub fn agreement_not_found(id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        request_id: None,
        message: Some(format!("agreement {id} not found")),
        resource_id: Some(id.to_string()),
        resource_type: Some(RESOURCE_TYPE_AGREEMENT.to_string()),
    }
}

/// The `ResourceNotFoundException` for an agreement request id that names none.
pub fn agreement_request_not_found(id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        request_id: None,
        message: Some(format!("agreement request {id} not found")),
        resource_id: Some(id.to_string()),
        resource_type: Some(RESOURCE_TYPE_AGREEMENT_REQUEST.to_string()),
    }
}

/// A `ConflictException` for an operation the resource's current state refuses.
pub fn conflict(resource_type: &str, id: &str, detail: impl Into<String>) -> ConflictException {
    ConflictException {
        request_id: None,
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
        request_id: None,
        message: Some("the marketplace agreement store is unavailable".to_string()),
    }
}

// ---------------------------------------------------------------------------
// The store
// ---------------------------------------------------------------------------

/// Per-service state for com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsMarketplaceagreementAwsmpcommerceserviceV20200301State {
    /// Every agreement request, keyed by its `^ar-[A-Za-z0-9]+$` id.
    pub agreement_requests: BTreeMap<String, AgreementRequest>,
    /// Every agreement, keyed by its id. A `BTreeMap` is what gives
    /// `SearchAgreements` a stable total order to page over: ids are minted from
    /// a zero-padded counter, so they sort in creation order.
    pub agreements: BTreeMap<String, Agreement>,
    /// A `CreateAgreementRequest` `clientToken` to the agreement request it
    /// created, so a replayed create answers with the first request's id
    /// instead of minting a second quote.
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
