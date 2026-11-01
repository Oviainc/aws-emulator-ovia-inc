// SPDX-License-Identifier: CC0-1.0

//! What ACCEPTING an amendment does to the agreement it acts upon, end to end
//! over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the
//! same path a client takes.
//!
//! The rule under test: accepting an agreement request whose intent is to amend
//! applies the terms accepted in that request to the agreement it names instead
//! of creating one, so that agreement's accepted terms, the window they commit
//! it for, the value its quote prices and its last-update time become the
//! amendment's, and the call answers that agreement's own id. `Intent` states
//! it — "`AMEND` – Modifies an existing agreement with terms that are accepted
//! in the request", against "`NEW` – Creates a new agreement for terms in the
//! request" and "`REPLACE` – Creates a new agreement with accepted terms and
//! replaces the existing agreement" — and three reports follow from it:
//! `AcceptAgreementRequestOutput$agreementId` is "the agreement created or
//! modified by accepting the agreement request",
//! `AgreementViewSummary$lastUpdateTime` is "when the agreement was last
//! updated. An agreement is updated when any of its attributes or accepted terms
//! change. Amendments, renewals, and a party changing whether the agreement
//! renews are all examples", and `EstimatedCharges$agreementValue` "in case a
//! customer has amended an agreement, by purchasing more units of any dimension
//! … will include both the original cost as well as the added cost incurred due
//! to addition of new units".
//!
//! The rows span what the rule generalises over: what the amendment changes
//! (more units of a dimension the agreement holds, a dimension it does not hold,
//! fewer units than it holds, a different contract length, another term beside
//! the priced one, terms that commit nothing at all, and the very same terms
//! again), which moves the agreement's value up, down or not at all, and makes
//! its end date later, the same or absent.
//!
//! The harness (`emulator`, `call`, `create`, `accept`, `agreement_over`,
//! `describe`, `search`, `agreement_type_filter`, `term`, `upfront_term`) is the
//! one `agreement_lifecycle` owns.

use std::cmp::Ordering;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::agreement_lifecycle::{
    accept, agreement_over, agreement_type_filter, call, create, describe, emulator, search, term,
    upfront_term,
};

/// The `AgreementStatus` an accepted agreement carries, spelled out here rather
/// than imported from the crate so a handler that stores something else fails
/// these rows.
const ACTIVE: &str = "ACTIVE";

/// `EstimatedCharges$agreementValue` where the model states it outright: "this
/// is `0` if the accepted terms contain `UsageBasedPricingTerm` without
/// `ConfigurableUpfrontPricingTerm` or `RecurringPaymentTerm`", the case terms
/// that commit nothing reach.
const NO_VALUE: &str = "0";

/// An amount — units, a point and exactly two minor digits, as every money
/// member of this service spells one — counted in minor units, so two amounts
/// are compared as numbers rather than as text.
fn minor_units(case: &str, pointer: &str, value: &Value) -> u128 {
    let text = value
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: {pointer} must be an amount, got {value:?}"));
    // The documented `0` carries no minor digits; every priced amount does.
    let (units, minor) = text.split_once('.').unwrap_or((text, "0"));
    let units: u128 = units
        .parse()
        .unwrap_or_else(|_| panic!("case {case:?}: {pointer} must be money, got {text:?}"));
    let minor: u128 = minor
        .parse()
        .unwrap_or_else(|_| panic!("case {case:?}: {pointer} must be money, got {text:?}"));
    units * 100 + minor
}

/// The dimension keys and quantities one reported accepted term configures, in
/// the order it reports them — "the actual dimensions, prices, and units the
/// buyer has chosen to accept".
fn configured_dimensions(reported: &Value) -> Vec<(String, i64)> {
    reported["configurableUpfrontPricingTerm"]["configuration"]["dimensions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|dimension| {
            (
                dimension["dimensionKey"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                dimension["dimensionValue"].as_i64().unwrap_or_default(),
            )
        })
        .collect()
}

/// The accepted terms `agreement` reports, in the order `GetAgreementTerms`
/// reports them.
async fn terms_of(router: &MultiServiceRouter, agreement: &str) -> Vec<Value> {
    let answer = call(
        router,
        "GetAgreementTerms",
        json!({"agreementId": agreement}),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "reporting the terms of {agreement} must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["acceptedTerms"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The `AgreementViewSummary` `SearchAgreements` reports for every agreement the
/// store holds, keyed by agreement id.
async fn summaries(router: &MultiServiceRouter) -> Vec<Value> {
    let found = search(router, json!({"filters": [agreement_type_filter()]})).await;
    assert_eq!(
        found.status, 200,
        "searching must succeed, got {} with {:?}",
        found.status, found.body
    );
    found.body["agreementViewSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The one summary `SearchAgreements` reports for `agreement`.
async fn summary_of(router: &MultiServiceRouter, agreement: &str) -> Value {
    summaries(router)
        .await
        .into_iter()
        .find(|summary| summary["agreementId"].as_str() == Some(agreement))
        .unwrap_or_else(|| panic!("{agreement} must be searchable"))
}

/// The `lastUpdateTime` `SearchAgreements` reports for `agreement`.
async fn last_update_time(router: &MultiServiceRouter, agreement: &str) -> f64 {
    let summary = summary_of(router, agreement).await;
    summary["lastUpdateTime"]
        .as_f64()
        .unwrap_or_else(|| panic!("{agreement} must report a lastUpdateTime, got {summary:?}"))
}

/// The charge ids `ListAgreementCharges` reports for `agreement`.
async fn charge_ids(router: &MultiServiceRouter, agreement: &str) -> Vec<String> {
    let listed = call(
        router,
        "ListAgreementCharges",
        json!({"agreementId": agreement}),
    )
    .await;
    assert_eq!(
        listed.status, 200,
        "listing the charges of {agreement} must succeed, got {} with {:?}",
        listed.status, listed.body
    );
    listed.body["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|charge| charge["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The `ExpectedCharge` ids one quote minted, in the order it reports them.
fn quoted_charge_ids(quote: &Value) -> Vec<String> {
    quote["chargeSummary"]["expectedCharges"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|charge| charge["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Quote an amendment of `source` over `terms`, answering what the create said.
///
/// "`agreementProposalIdentifier` … is required if the intent is not `AMEND`",
/// so an amendment names only the agreement it acts upon.
async fn quote_amendment(
    router: &MultiServiceRouter,
    case: &str,
    source: &str,
    terms: Vec<Value>,
) -> crate::agreement_lifecycle::Answer {
    let quoted = create(
        router,
        json!({
            "intent": "AMEND",
            "sourceAgreementIdentifier": source,
            "requestedTerms": terms,
        }),
    )
    .await;
    assert_eq!(
        quoted.status, 200,
        "case {case:?}: quoting the amendment must succeed, got {} with {:?}",
        quoted.status, quoted.body
    );
    quoted
}

/// What one row amends an agreement to, and what the rule then implies about
/// the agreement's value and end date.
struct AmendmentCase {
    case: &'static str,
    /// The terms the amendment accepts.
    terms: fn() -> Vec<Value>,
    /// How the amended agreement's value compares with the value it had before:
    /// buying more units raises it, buying fewer lowers it, and terms that buy
    /// the same units leave it where it was.
    value: Ordering,
    /// Whether the amended agreement still has an end date at all: "the field
    /// is null for pay-as-you-go agreements, which don't have end dates", so an
    /// amendment whose terms commit no duration leaves it with none.
    ends: bool,
    /// Whether that end date is the one it had before, which it is while the
    /// amendment commits the agreement for the same contract length — the window
    /// runs from the agreement's own start date, which an amendment does not
    /// move.
    same_end: bool,
}

/// The terms the agreement every row starts from was accepted over: a prepaid
/// purchase of four Users for one year.
fn source_terms() -> Vec<Value> {
    vec![upfront_term("cfg", "P12M", &[("Users", 4)])]
}

fn amendment_cases() -> Vec<AmendmentCase> {
    vec![
        AmendmentCase {
            case: "more units of a dimension the agreement holds",
            terms: || vec![upfront_term("cfg", "P12M", &[("Users", 9)])],
            value: Ordering::Greater,
            ends: true,
            same_end: true,
        },
        AmendmentCase {
            case: "a dimension the agreement does not hold",
            terms: || vec![upfront_term("cfg", "P12M", &[("Users", 4), ("Storage", 100)])],
            value: Ordering::Greater,
            ends: true,
            same_end: true,
        },
        AmendmentCase {
            case: "fewer units than the agreement holds",
            terms: || vec![upfront_term("cfg", "P12M", &[("Users", 1)])],
            value: Ordering::Less,
            ends: true,
            same_end: true,
        },
        AmendmentCase {
            case: "a longer contract length",
            terms: || vec![upfront_term("cfg", "P24M", &[("Users", 4)])],
            value: Ordering::Greater,
            ends: true,
            same_end: false,
        },
        AmendmentCase {
            case: "another term beside the priced one",
            terms: || {
                vec![
                    upfront_term("cfg", "P12M", &[("Users", 4)]),
                    term("SupportTerm"),
                ]
            },
            value: Ordering::Equal,
            ends: true,
            same_end: true,
        },
        AmendmentCase {
            case: "terms that commit nothing at all",
            terms: || vec![term("eula.v2")],
            value: Ordering::Less,
            ends: false,
            same_end: false,
        },
        AmendmentCase {
            case: "the very same terms again",
            terms: source_terms,
            value: Ordering::Equal,
            ends: true,
            same_end: true,
        },
    ]
}

/// Accepting an amendment modifies the agreement it names: that agreement
/// reports the terms the amendment accepted, the end date they commit it for,
/// the value its quote priced and the acceptance instant as its
/// `lastUpdateTime` — and the accept answers that agreement rather than minting
/// a second one, so the chain still holds exactly one agreement.
#[tokio::test]
async fn accepting_an_amendment_applies_it_to_the_agreement_it_names() {
    for row in amendment_cases() {
        let case = row.case;
        let router = emulator();
        let source = agreement_over(&router, "ap-amended", source_terms()).await;

        let before = describe(&router, &source).await;
        let before_value = minor_units(
            case,
            "estimatedCharges.agreementValue",
            &before.body["estimatedCharges"]["agreementValue"],
        );
        let before_terms = terms_of(&router, &source).await;
        let before_update = last_update_time(&router, &source).await;
        assert_eq!(
            configured_dimensions(&before_terms[0]),
            vec![("Users".to_string(), 4)],
            "case {case:?}: the agreement must start from the terms it was accepted over, \
             got {before_terms:?}"
        );

        let requested = (row.terms)();
        let quoted = quote_amendment(&router, case, &source, requested.clone()).await;
        let accepted = accept(
            &router,
            json!({"agreementRequestId": quoted.str_at("/agreementRequestId")}),
        )
        .await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: accepting the amendment must succeed, got {} with {:?}",
            accepted.status, accepted.body
        );
        assert_eq!(
            accepted.str_at("/agreementId"),
            source,
            "case {case:?}: an amendment modifies an existing agreement, so the accept answers \
             that agreement, got {:?}",
            accepted.body
        );

        // Nothing was minted: the store still holds the one agreement, which is
        // still the only link of its chain and still active.
        let held: Vec<String> = summaries(&router)
            .await
            .iter()
            .map(|summary| summary["agreementId"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            held,
            vec![source.clone()],
            "case {case:?}: an amendment creates no agreement, so the store still holds only \
             {source}, got {held:?}"
        );
        let summary = summary_of(&router, &source).await;
        assert_eq!(
            summary["status"].as_str(),
            Some(ACTIVE),
            "case {case:?}: the amended agreement is still active, got {summary:?}"
        );
        assert_eq!(
            summary["initialAgreementId"].as_str(),
            Some(source.as_str()),
            "case {case:?}: the amended agreement still starts its own chain, got {summary:?}"
        );

        // Its accepted terms are the terms the amendment accepted: one per
        // requested term, by the ids the request named, carrying the
        // configuration it asked for.
        let after_terms = terms_of(&router, &source).await;
        let reported_ids: Vec<String> = after_terms
            .iter()
            .map(|reported| {
                reported
                    .as_object()
                    .and_then(|object| object.values().next())
                    .map(|payload| payload["id"].as_str().unwrap_or_default().to_string())
                    .unwrap_or_default()
            })
            .collect();
        let requested_ids: Vec<String> = requested
            .iter()
            .map(|term| term["id"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            reported_ids, requested_ids,
            "case {case:?}: the amended agreement holds the terms accepted in the request, \
             got {after_terms:?}"
        );
        let expected_dimensions: Vec<(String, i64)> = requested[0]["configuration"]
            ["configurableUpfrontPricingTermConfiguration"]["dimensions"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|dimension| {
                (
                    dimension["dimensionKey"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    dimension["dimensionValue"].as_i64().unwrap_or_default(),
                )
            })
            .collect();
        assert_eq!(
            configured_dimensions(&after_terms[0]),
            expected_dimensions,
            "case {case:?}: the amended agreement reports the quantities the amendment \
             configured, got {after_terms:?}"
        );

        // Its value is what the amendment's own quote priced the whole
        // agreement at, and it moved the way the row's amendment implies.
        let after = describe(&router, &source).await;
        assert_eq!(
            after.body["estimatedCharges"]["agreementValue"],
            quoted.body["chargeSummary"]["newAgreementValue"],
            "case {case:?}: the amended agreement is worth what its amendment quoted, got {:?}",
            after.body["estimatedCharges"]
        );
        let after_value = minor_units(
            case,
            "estimatedCharges.agreementValue",
            &after.body["estimatedCharges"]["agreementValue"],
        );
        assert_eq!(
            after_value.cmp(&before_value),
            row.value,
            "case {case:?}: the value must be {:?} than the {before_value} it was, got \
             {after_value}",
            row.value
        );
        if !row.ends {
            assert_eq!(
                after.body["estimatedCharges"]["agreementValue"].as_str(),
                Some(NO_VALUE),
                "case {case:?}: terms that commit nothing are worth {NO_VALUE}, got {:?}",
                after.body["estimatedCharges"]
            );
        }

        // Its window: the agreement was modified, not restarted, so it keeps
        // its start and acceptance instants, and its end date is the one the
        // amended terms commit it for.
        assert_eq!(
            after.body["startTime"], before.body["startTime"],
            "case {case:?}: an amendment does not restart the agreement, got {:?}",
            after.body
        );
        assert_eq!(
            after.body["acceptanceTime"], before.body["acceptanceTime"],
            "case {case:?}: an amendment does not re-date the acceptance, got {:?}",
            after.body
        );
        assert_eq!(
            after.body["endTime"].is_null(),
            !row.ends,
            "case {case:?}: the amended agreement must {} an end date, got {:?}",
            if row.ends { "have" } else { "have no" },
            after.body
        );
        if row.ends {
            let moved = after.body["endTime"] != before.body["endTime"];
            assert_eq!(
                !moved,
                row.same_end,
                "case {case:?}: the end date must {} the {:?} it was, got {:?}",
                if row.same_end { "stay at" } else { "move off" },
                before.body["endTime"],
                after.body["endTime"]
            );
        }

        // "An agreement is updated when any of its attributes or accepted terms
        // change. Amendments … are all examples."
        let after_update = last_update_time(&router, &source).await;
        assert!(
            after_update > before_update,
            "case {case:?}: accepting the amendment stamps lastUpdateTime past {before_update}, \
             got {after_update}"
        );
    }
}

/// The charges an amendment quoted are charges OF the agreement it modified,
/// beside the ones that agreement was already reporting — an amendment adds to
/// an agreement rather than creating a second one to carry them.
#[tokio::test]
async fn an_amendment_adds_its_charges_to_the_agreement_it_modified() {
    let case = "an amendment of a prepaid purchase";
    let router = emulator();

    let quoted_source = create(
        &router,
        json!({
            "intent": "NEW",
            "agreementProposalIdentifier": "ap-charges",
            "requestedTerms": source_terms(),
        }),
    )
    .await;
    assert_eq!(
        quoted_source.status, 200,
        "case {case:?}: quoting the purchase must succeed, got {} with {:?}",
        quoted_source.status, quoted_source.body
    );
    let accepted = accept(
        &router,
        json!({"agreementRequestId": quoted_source.str_at("/agreementRequestId")}),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "case {case:?}: accepting the purchase must succeed, got {} with {:?}",
        accepted.status, accepted.body
    );
    let agreement = accepted.str_at("/agreementId");

    let quoted_amendment = quote_amendment(
        &router,
        case,
        &agreement,
        vec![upfront_term("cfg", "P12M", &[("Users", 9)])],
    )
    .await;
    let amended = accept(
        &router,
        json!({"agreementRequestId": quoted_amendment.str_at("/agreementRequestId")}),
    )
    .await;
    assert_eq!(
        amended.status, 200,
        "case {case:?}: accepting the amendment must succeed, got {} with {:?}",
        amended.status, amended.body
    );

    // Both quotes' charges are reported, in the order they were quoted, and
    // each of the amendment's is a charge the earlier quote did not hold.
    let mut expected = quoted_charge_ids(&quoted_source.body);
    let added = quoted_charge_ids(&quoted_amendment.body);
    assert!(
        !expected.is_empty() && !added.is_empty(),
        "case {case:?}: both quotes must price a charge, got {expected:?} and {added:?}"
    );
    assert!(
        added.iter().all(|id| !expected.contains(id)),
        "case {case:?}: the amendment mints its own charges, got {added:?} against {expected:?}"
    );
    expected.extend(added);
    let reported = charge_ids(&router, &agreement).await;
    assert_eq!(
        reported, expected,
        "case {case:?}: the amended agreement reports the charges of both its quotes, \
         got {reported:?}"
    );

    // Each of those charges is billed by its own invoice: the purchase was
    // invoiced on acceptance and so was the amendment, whose charge the earlier
    // invoice cannot have billed.
    let listed = call(
        &router,
        "ListAgreementInvoiceLineItems",
        json!({"agreementId": agreement, "groupBy": "INVOICE_ID"}),
    )
    .await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: listing the invoices must succeed, got {} with {:?}",
        listed.status, listed.body
    );
    let groups = listed.body["agreementInvoiceLineItemGroupSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        groups.len(),
        2,
        "case {case:?}: the acceptance of the purchase and the acceptance of the amendment each \
         issue an invoice, got {:?}",
        listed.body
    );
    let billed = minor_units(
        case,
        "agreementInvoiceLineItemGroupSummaries[1].pricingCurrencyAmount.amount",
        &groups[1]["pricingCurrencyAmount"]["amount"],
    );
    let charged = minor_units(
        case,
        "chargeSummary.expectedCharges[0].amount",
        &quoted_amendment.body["chargeSummary"]["expectedCharges"][0]["amount"],
    );
    assert_eq!(
        billed, charged,
        "case {case:?}: the amendment's invoice bills what its quote charged ({charged}), \
         got {billed}"
    );
}

/// An amendment is quoted and applied against the agreement AS IT STANDS, so a
/// second amendment measures against the quantities the first one left: the
/// agreement accumulates its amendments rather than each one re-amending the
/// terms it was accepted over.
#[tokio::test]
async fn a_second_amendment_is_measured_against_the_first() {
    let case = "two amendments of one agreement";
    let router = emulator();
    let agreement = agreement_over(&router, "ap-twice", source_terms()).await;

    // The quantities each step leaves the agreement holding, and what the step
    // after it must therefore report as the existing quantity.
    let steps = [(9, 4), (12, 9)];
    for (new_quantity, old_quantity) in steps {
        let quoted = quote_amendment(
            &router,
            case,
            &agreement,
            vec![upfront_term("cfg", "P12M", &[("Users", new_quantity)])],
        )
        .await;
        let reported: Vec<(i64, i64)> = quoted.body["chargeSummary"]["itemizedCharges"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|item| {
                (
                    item["newQuantity"].as_i64().unwrap_or_default(),
                    item["oldQuantity"].as_i64().unwrap_or_default(),
                )
            })
            .collect();
        assert_eq!(
            reported,
            vec![(i64::from(new_quantity), i64::from(old_quantity))],
            "case {case:?}: amending to {new_quantity} Users is quoted against the \
             {old_quantity} the agreement holds, got {:?}",
            quoted.body["chargeSummary"]
        );

        let accepted = accept(
            &router,
            json!({"agreementRequestId": quoted.str_at("/agreementRequestId")}),
        )
        .await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: accepting the amendment to {new_quantity} Users must succeed, got {} \
             with {:?}",
            accepted.status, accepted.body
        );
        assert_eq!(
            accepted.str_at("/agreementId"),
            agreement,
            "case {case:?}: each amendment modifies the same agreement, got {:?}",
            accepted.body
        );

        let held = terms_of(&router, &agreement).await;
        assert_eq!(
            configured_dimensions(&held[0]),
            vec![("Users".to_string(), i64::from(new_quantity))],
            "case {case:?}: the agreement holds the {new_quantity} Users the amendment bought, \
             got {held:?}"
        );
    }

    // One agreement all along: two amendments mint none.
    let held: Vec<String> = summaries(&router)
        .await
        .iter()
        .map(|summary| summary["agreementId"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        held,
        vec![agreement.clone()],
        "case {case:?}: two amendments create no agreement, got {held:?}"
    );
}
