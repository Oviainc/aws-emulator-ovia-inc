// SPDX-License-Identifier: CC0-1.0

//! What an agreement's ACCEPTED TERMS carry, end to end over HTTP.
//!
//! The rule under test: an accepted term is the proposal's term as the agreement
//! holds it. `GetAgreementTerms` names the three members that come from the
//! acceptance itself — the `TermType`, the `TermID` "which is common between
//! offer and agreement" and the `Configuration`, "the buyer/acceptor's selection
//! at the time of agreement creation" — and every OTHER payload member the
//! model declares for a term belongs to the offer's own term document: the
//! term's currency and rate cards, its price, duration, schedule, grants and
//! policy text, plus — for a validity term — the window the agreement itself
//! runs for. This emulator publishes that document and its rate card as DATA
//! (there is no agreement proposal in this service's model), so the terms an
//! agreement reports are the terms of THAT document, priced at the amounts its
//! own quote charged, and a member the document publishes nothing for is absent
//! rather than invented.
//!
//! The kind of a term is resolved from what the service holds, never guessed
//! from the shape of an id: the acceptor's `configuration` names it outright,
//! else the term of the same id on the agreement this request amends or replaces
//! does (one `TermID` across a chain is one term of one proposal), else the id
//! naming one of the kinds the document carries. A term none of them names is a
//! term of a proposal this emulator does not publish: it is reported as the kind
//! that asserts nothing — a legal term — carrying NONE of the document's
//! payload, so a caller can tell it from the legal term the document really
//! proposes.
//!
//! The rows differ in what the rule generalises over: every kind the
//! `AcceptedTerm` union models (`smithy_query(query="shape",
//! shape="AcceptedTerm")`), how the kind is named (by the configuration, by the
//! id, or carried over from a source agreement), which contract length the
//! acceptor selected and whether the rate card publishes it, which product
//! dimensions were bought and whether the rate card publishes them, and how many
//! terms one agreement holds. Every amount and id is read back from the call
//! that produced it, so no row can agree with a handler that invents one.
//!
//! The harness (`emulator`, `call`, `create`, `accept`, `agreement_over`,
//! `new_request`, `term`, `upfront_term`, `variable_payment_term`) is the one
//! `agreement_lifecycle` owns.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::agreement_lifecycle::{
    accept, agreement_over, call, create, emulator, new_request, term, upfront_term,
    variable_payment_term, Answer,
};

/// The currency every price of the offer this emulator publishes is quoted in,
/// spelled out here rather than imported so a term reporting another currency
/// fails these rows. `CurrencyCode` is `@length(3, 3)` with `^[A-Z]+$`.
const CURRENCY: &str = "USD";

/// The contract lengths the published rate card prices a configured prepaid
/// purchase at, each with how long the agreement then runs, in days.
///
/// SOURCE: the selectors the emulator's rate card publishes, which are the
/// ISO-8601 contract lengths contract pricing is bought in;
/// `ConfigurableUpfrontRateCardItem$selector` "differentiates between the
/// mutually exclusive rate cards in the same pricing term to be selected by the
/// buyer". Spelled out here so a handler publishing a different set fails.
const CONTRACT_LENGTHS: &[(&str, u64)] = &[
    ("P1M", 30),
    ("P12M", 365),
    ("P24M", 730),
    ("P36M", 1095),
];

/// The contract length an agreement runs for when its acceptor selected none the
/// rate card publishes.
const DEFAULT_CONTRACT_LENGTH: (&str, u64) = ("P12M", 365);

/// Seconds in a day, for turning a contract length into a window.
const SECONDS_PER_DAY: f64 = 86_400.0;

/// A product dimension the published rate card does NOT carry, so nothing
/// prices it and no rate card lists it. `BoundedString`'s `^(.)+$` admits it.
const UNPUBLISHED_DIMENSION: &str = "Widgets";

/// A contract length the published rate card does NOT carry.
const UNPUBLISHED_CONTRACT_LENGTH: &str = "P6M";

/// `DocumentItem$type` of the legal document the published offer proposes. Its
/// documented types are `CustomEula`, `CustomDsa`, `StandardEula` and
/// `StandardDsa`, and a standard one "is managed by AWS Marketplace", so it
/// carries a `version` and no `url`.
const STANDARD_EULA: &str = "StandardEula";

/// One row of the payload table: the term requested, the union member and
/// `TermType` it comes back as, and exactly which payload members it carries.
struct TermCase {
    case: &'static str,
    requested: Value,
    member: &'static str,
    kind: &'static str,
    /// Every member the reported term must carry — and, since an absent member
    /// is not serialized at all, every member it may carry.
    ///
    /// SOURCE: the members each term shape declares,
    /// `smithy_query(query="shape", shape="LegalTerm,SupportTerm,RenewalTerm,…")`.
    present: &'static [&'static str],
    /// Members the model declares that this term reports no value for, each for
    /// a reason the model states.
    absent: &'static [&'static str],
}

fn term_cases() -> Vec<TermCase> {
    vec![
        TermCase {
            case: "the legal term the document proposes",
            requested: term("LegalTerm"),
            member: "legalTerm",
            kind: "LegalTerm",
            present: &["type", "id", "documents"],
            absent: &[],
        },
        TermCase {
            case: "an id naming no kind the document carries",
            requested: term("abc123term"),
            member: "legalTerm",
            kind: "LegalTerm",
            // Nothing resolved this term's kind, so the document publishes no
            // payload for it: the legal documents of the offer's own legal term
            // are not this term's.
            present: &["type", "id"],
            absent: &["documents"],
        },
        TermCase {
            case: "a support term",
            requested: term("SupportTerm"),
            member: "supportTerm",
            kind: "SupportTerm",
            present: &["type", "id", "refundPolicy"],
            absent: &[],
        },
        TermCase {
            case: "a renewal term, whose configuration names the kind",
            requested: json!({
                "id": "renew",
                "configuration": {"renewalTermConfiguration": {"enableAutoRenew": true}},
            }),
            member: "renewalTerm",
            kind: "RenewalTerm",
            present: &["type", "id", "configuration", "lockoutPeriod", "maxRenewals"],
            // "The field is null when the price doesn't change at renewal" and
            // "null when no terms change at renewal"; `adjustmentDeadline`
            // "applies only when `PriceIncrease` is a `PercentageRange`".
            absent: &["priceIncrease", "termTemplates", "adjustmentDeadline"],
        },
        TermCase {
            case: "usage-based pricing",
            requested: term("UsageBasedPricingTerm"),
            member: "usageBasedPricingTerm",
            kind: "UsageBasedPricingTerm",
            present: &["type", "id", "currencyCode", "rateCards"],
            absent: &[],
        },
        TermCase {
            case: "a configured prepaid purchase",
            requested: upfront_term("cfg", "P12M", &[("Users", 2)]),
            member: "configurableUpfrontPricingTerm",
            kind: "ConfigurableUpfrontPricingTerm",
            present: &["type", "id", "currencyCode", "rateCards", "configuration"],
            absent: &[],
        },
        TermCase {
            case: "a BYOL term, which models no payload beyond its identity",
            requested: term("ByolPricingTerm"),
            member: "byolPricingTerm",
            kind: "ByolPricingTerm",
            present: &["type", "id"],
            absent: &[],
        },
        TermCase {
            case: "a recurring payment term",
            requested: term("RecurringPaymentTerm"),
            member: "recurringPaymentTerm",
            kind: "RecurringPaymentTerm",
            present: &["type", "id", "currencyCode", "billingPeriod", "price"],
            absent: &[],
        },
        TermCase {
            case: "a validity term",
            requested: term("ValidityTerm"),
            member: "validityTerm",
            kind: "ValidityTerm",
            present: &[
                "type",
                "id",
                "agreementDuration",
                "agreementStartDate",
                "agreementEndDate",
            ],
            absent: &[],
        },
        TermCase {
            case: "a payment schedule term",
            requested: term("PaymentScheduleTerm"),
            member: "paymentScheduleTerm",
            kind: "PaymentScheduleTerm",
            present: &["type", "id", "currencyCode", "schedule"],
            absent: &[],
        },
        TermCase {
            case: "a free trial term",
            requested: term("FreeTrialPricingTerm"),
            member: "freeTrialPricingTerm",
            kind: "FreeTrialPricingTerm",
            present: &["type", "id", "duration", "grants"],
            absent: &[],
        },
        TermCase {
            case: "a fixed upfront term",
            requested: term("FixedUpfrontPricingTerm"),
            member: "fixedUpfrontPricingTerm",
            kind: "FixedUpfrontPricingTerm",
            present: &["type", "id", "currencyCode", "duration", "price", "grants"],
            absent: &[],
        },
        TermCase {
            case: "a variable payment term, whose configuration names the kind",
            requested: variable_payment_term("vpt"),
            member: "variablePaymentTerm",
            kind: "VariablePaymentTerm",
            present: &[
                "type",
                "id",
                "currencyCode",
                "maxTotalChargeAmount",
                "configuration",
            ],
            absent: &[],
        },
        TermCase {
            case: "a net payment term",
            requested: term("NetPaymentTerm"),
            member: "netPaymentTerm",
            kind: "NetPaymentTerm",
            present: &["type", "id", "paymentDuePeriod"],
            absent: &[],
        },
    ]
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

/// The one union member of an accepted term: its name and its payload.
fn union_member(term: &Value) -> (String, Value) {
    let object = term
        .as_object()
        .unwrap_or_else(|| panic!("an accepted term must be a union object: {term:?}"));
    assert_eq!(
        object.len(),
        1,
        "an accepted term carries exactly one union member: {term:?}"
    );
    let (member, payload) = object.iter().next().expect("one member");
    (member.clone(), payload.clone())
}

/// What `DescribeAgreement` reports for `agreement`.
async fn describe(router: &MultiServiceRouter, agreement: &str) -> Answer {
    let answer = call(
        router,
        "DescribeAgreement",
        json!({"agreementId": agreement}),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "describing {agreement} must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    answer
}

/// `amount` — units, a point and exactly two minor digits, as every money member
/// of this service spells one — counted in minor units, so two amounts are
/// compared and added as integers.
fn minor_units(case: &str, amount: &Value) -> u128 {
    let text = amount
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an amount, got {amount:?}"));
    let (units, minor) = text
        .split_once('.')
        .unwrap_or_else(|| panic!("case {case:?}: expected decimal money, got {text:?}"));
    assert_eq!(
        minor.len(),
        2,
        "case {case:?}: expected two minor digits, got {text:?}"
    );
    let parse = |part: &str| -> u128 {
        part.parse()
            .unwrap_or_else(|_| panic!("case {case:?}: expected digits, got {text:?}"))
    };
    parse(units) * 100 + parse(minor)
}

/// The member names a JSON object carries.
fn keys(value: &Value) -> BTreeSet<String> {
    value
        .as_object()
        .map(|object| object.keys().cloned().collect())
        .unwrap_or_default()
}

/// Every kind of term is reported with the payload the published offer document
/// carries for it — and only with members the model declares for that kind.
///
/// The rows span every member of the `AcceptedTerm` union, both ways a kind is
/// named (the configuration, the id), and the id that names none.
#[tokio::test]
async fn every_accepted_term_carries_the_payload_its_kind_models() {
    let router = emulator();
    for case in term_cases() {
        let requested_id = case.requested["id"].as_str().unwrap_or_default().to_string();
        let agreement =
            agreement_over(&router, "ap-document", vec![case.requested.clone()]).await;
        let reported = terms_of(&router, &agreement).await;
        assert_eq!(
            reported.len(),
            1,
            "case {:?}: one requested term becomes one accepted term, got {reported:?}",
            case.case
        );
        let (member, payload) = union_member(&reported[0]);
        assert_eq!(
            (member.as_str(), payload["type"].as_str(), payload["id"].as_str()),
            (case.member, Some(case.kind), Some(requested_id.as_str())),
            "case {:?}: expected the {} member carrying {} and the requested id, got {:?}",
            case.case,
            case.member,
            case.kind,
            reported[0]
        );
        let reported_members = keys(&payload);
        assert_eq!(
            reported_members,
            case.present
                .iter()
                .map(|member| (*member).to_string())
                .collect::<BTreeSet<String>>(),
            "case {:?}: expected exactly the members the document publishes, got {:?}",
            case.case,
            payload
        );
        for absent in case.absent {
            assert!(
                payload[*absent].is_null(),
                "case {:?}: {absent} carries no value, got {:?}",
                case.case,
                payload
            );
        }
        // No member the document publishes is reported empty: a list with no
        // entries would be a payload that says nothing.
        for member in case.present {
            let value = &payload[*member];
            assert!(
                !value.is_null() && value.as_array().is_none_or(|list| !list.is_empty()),
                "case {:?}: {member} must carry a value, got {:?}",
                case.case,
                payload
            );
        }
        if reported_members.contains("currencyCode") {
            assert_eq!(
                payload["currencyCode"].as_str(),
                Some(CURRENCY),
                "case {:?}: every price of the document is quoted in {CURRENCY}, got {:?}",
                case.case,
                payload
            );
        }
    }
}

/// The legal documents a legal term proposes are references to legal resources:
/// a standard contract, which carries the version AWS Marketplace manages it at
/// and no URL of its own.
#[tokio::test]
async fn the_legal_term_proposes_the_documents_the_offer_publishes() {
    let router = emulator();
    let agreement = agreement_over(&router, "ap-legal", vec![term("LegalTerm")]).await;
    let (_, payload) = union_member(&terms_of(&router, &agreement).await[0]);
    let documents = payload["documents"].as_array().cloned().unwrap_or_default();
    assert!(
        !documents.is_empty(),
        "the published legal term proposes at least one document, got {payload:?}"
    );
    for document in &documents {
        assert_eq!(
            document["type"].as_str(),
            Some(STANDARD_EULA),
            "case {:?}: expected the standard contract the offer proposes, got {document:?}",
            "a published legal document"
        );
        assert!(
            document["version"]
                .as_str()
                .is_some_and(|version| !version.is_empty()),
            "case {:?}: a standard contract carries the version it is managed at, got {document:?}",
            "a published legal document"
        );
        assert!(
            document["url"].is_null(),
            "case {:?}: a standard contract carries no URL of its own, got {document:?}",
            "a published legal document"
        );
    }
}

/// One row of the rate-card table: what was bought, and at what contract length.
struct Purchase {
    case: &'static str,
    selector: &'static str,
    dimensions: Vec<(&'static str, i32)>,
}

/// A configured prepaid term reports the seller's rate cards — one per contract
/// length the offer publishes — and the amount its own quote charged is those
/// published rates at the quantities the acceptor bought.
///
/// "The `RateCard` section will define the prices and dimensions defined by the
/// seller (proposer), whereas the `Configuration` section will define the actual
/// dimensions, prices, and units the buyer has chosen to accept"
/// (`Dimension$dimensionValue`).
///
/// The rows differ along the contract length, which dimensions were bought and
/// how many of them, and the quantity of each — including a dimension and a
/// length the published card does not carry, which no card lists and no quote
/// prices.
#[tokio::test]
async fn a_prepaid_term_reports_the_rate_cards_its_charge_was_priced_from() {
    let purchases = vec![
        Purchase {
            case: "one dimension at the shortest published length",
            selector: "P1M",
            dimensions: vec![("Users", 3)],
        },
        Purchase {
            case: "two dimensions at a year",
            selector: "P12M",
            dimensions: vec![("Users", 2), ("Storage", 5)],
        },
        Purchase {
            case: "three dimensions at two years",
            selector: "P24M",
            dimensions: vec![("Admins", 2), ("Nodes", 1), ("Storage", 100)],
        },
        Purchase {
            case: "dimensions the issue did not cite, at the longest length",
            selector: "P36M",
            dimensions: vec![("Hosts", 1), ("DataTransfer", 7)],
        },
        Purchase {
            case: "a dimension the published card does not carry",
            selector: "P12M",
            dimensions: vec![(UNPUBLISHED_DIMENSION, 4)],
        },
        Purchase {
            case: "a contract length the published card does not carry",
            selector: UNPUBLISHED_CONTRACT_LENGTH,
            dimensions: vec![("Users", 1)],
        },
    ];

    let router = emulator();
    for purchase in purchases {
        let case = purchase.case;
        let requested = upfront_term("cfg", purchase.selector, &purchase.dimensions);
        let created = create(&router, new_request("ap-rates", vec![requested])).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: creating the quote must succeed, got {} with {:?}",
            created.status, created.body
        );
        let charged = created.body["chargeSummary"]["expectedCharges"][0]["amount"].clone();
        let accepted = accept(
            &router,
            json!({"agreementRequestId": created.str_at("/agreementRequestId")}),
        )
        .await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: accepting must succeed, got {} with {:?}",
            accepted.status, accepted.body
        );
        let agreement = accepted.str_at("/agreementId");
        let (_, payload) = union_member(&terms_of(&router, &agreement).await[0]);

        // One card per published contract length, in the order the offer
        // publishes them, each selected by that length.
        let cards = payload["rateCards"].as_array().cloned().unwrap_or_default();
        let selectors: Vec<String> = cards
            .iter()
            .map(|card| card["selector"]["value"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            selectors,
            CONTRACT_LENGTHS
                .iter()
                .map(|(length, _)| (*length).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: expected one card per published contract length, got {payload:?}"
        );

        // The amount the quote charged is the published per-unit rate of every
        // dimension bought, at the selected length, times the quantity bought —
        // and no amount at all when one of them is not on the card, which
        // `ExpectedCharge$amount` ("only available when the charge amount is
        // known") states.
        let card = cards
            .iter()
            .find(|card| card["selector"]["value"].as_str() == Some(purchase.selector))
            .cloned()
            .unwrap_or(Value::Null);
        let mut expected: Option<u128> = Some(0);
        for (dimension, quantity) in &purchase.dimensions {
            let rate = card["rateCard"]
                .as_array()
                .and_then(|rates| {
                    rates
                        .iter()
                        .find(|rate| rate["dimensionKey"].as_str() == Some(*dimension))
                        .cloned()
                })
                .map(|rate| minor_units(case, &rate["price"]));
            assert_eq!(
                rate.is_some(),
                *dimension != UNPUBLISHED_DIMENSION && !card.is_null(),
                "case {case:?}: the card lists exactly the dimensions the offer prices, \
                 got {card:?}"
            );
            expected = expected
                .zip(rate)
                .map(|(total, rate)| total + rate * u128::from(quantity.unsigned_abs()));
        }
        match expected {
            Some(expected) => assert_eq!(
                minor_units(case, &charged),
                expected,
                "case {case:?}: the charge must be the card's rates at the quantities bought, \
                 got {charged:?} against {card:?}"
            ),
            None => assert!(
                charged.is_null(),
                "case {case:?}: an unpriced dimension leaves the charge without an amount, \
                 got {charged:?}"
            ),
        }
    }
}

/// A usage-based term reports the per-unit rates of a billing period, over the
/// same dimensions the seller's card prices, and a free-trial or prepaid term
/// reports a grant for each of them.
#[tokio::test]
async fn the_metered_rates_and_the_grants_cover_the_published_dimensions() {
    let router = emulator();
    let agreement = agreement_over(
        &router,
        "ap-metered",
        vec![
            term("UsageBasedPricingTerm"),
            term("FreeTrialPricingTerm"),
            upfront_term("cfg", "P12M", &[("Users", 1)]),
        ],
    )
    .await;
    let reported = terms_of(&router, &agreement).await;
    let payloads: Vec<(String, Value)> = reported.iter().map(union_member).collect();
    let find = |member: &str| -> Value {
        payloads
            .iter()
            .find(|(name, _)| name == member)
            .map(|(_, payload)| payload.clone())
            .unwrap_or_else(|| panic!("expected a {member} among {reported:?}"))
    };

    // The dimensions the seller's card prices, read from the prepaid term's own
    // card for the default contract length.
    let prepaid = find("configurableUpfrontPricingTerm");
    let published: BTreeSet<String> = prepaid["rateCards"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .find(|card| card["selector"]["value"].as_str() == Some(DEFAULT_CONTRACT_LENGTH.0))
        .and_then(|card| card["rateCard"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .map(|rate| rate["dimensionKey"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        published.len() > 1,
        "case {:?}: the published card prices several dimensions, got {published:?}",
        "the published dimensions"
    );

    let metered = find("usageBasedPricingTerm");
    let metered_cards = metered["rateCards"].as_array().cloned().unwrap_or_default();
    let metered_dimensions: BTreeSet<String> = metered_cards
        .iter()
        .flat_map(|card| card["rateCard"].as_array().cloned().unwrap_or_default())
        .map(|rate| rate["dimensionKey"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        metered_dimensions, published,
        "case {:?}: a metered rate is published for each priced dimension, got {metered:?}",
        "the metered rates"
    );
    for card in &metered_cards {
        for rate in card["rateCard"].as_array().cloned().unwrap_or_default() {
            // Every rate is money, so a caller can calculate with it.
            let _ = minor_units("the metered rates", &rate["price"]);
        }
    }

    let trial = find("freeTrialPricingTerm");
    let granted: BTreeSet<String> = trial["grants"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|grant| {
            // `GrantItem$maxQuantity` is `@range(min: 1)`: a grant of nothing is
            // not a grant.
            let quantity = grant["maxQuantity"].as_i64().unwrap_or_default();
            assert!(
                quantity >= 1,
                "case {:?}: a grant entitles at least one unit, got {grant:?}",
                "the published grants"
            );
            grant["dimensionKey"].as_str().unwrap_or_default().to_string()
        })
        .collect();
    assert_eq!(
        granted, published,
        "case {:?}: the offer grants capacity in each priced dimension, got {trial:?}",
        "the published grants"
    );
}

/// A term that carries ONE price reports the price its own quote charged, and a
/// payment-schedule term bills that price in installments that sum to it and
/// fall inside the agreement's own window.
///
/// `EstimatedCharges$agreementValue` calls the schedule's total "the sum of all
/// invoice charges in the payment schedule", and `ScheduleItem$chargeDate` is
/// "the date that the customer would pay the price defined in this payment
/// schedule term".
#[tokio::test]
async fn a_priced_term_reports_the_amount_its_quote_charged() {
    let cases: Vec<(&str, &str, &str)> = vec![
        (
            "a fixed upfront price",
            "FixedUpfrontPricingTerm",
            "fixedUpfrontPricingTerm",
        ),
        (
            "a recurring price, per billing period",
            "RecurringPaymentTerm",
            "recurringPaymentTerm",
        ),
    ];

    let router = emulator();
    for (case, kind, member) in cases {
        let created = create(&router, new_request("ap-priced", vec![term(kind)])).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: creating the quote must succeed, got {} with {:?}",
            created.status, created.body
        );
        let charged = created.body["chargeSummary"]["expectedCharges"][0]["amount"].clone();
        let accepted = accept(
            &router,
            json!({"agreementRequestId": created.str_at("/agreementRequestId")}),
        )
        .await;
        let agreement = accepted.str_at("/agreementId");
        let (reported_member, payload) = union_member(&terms_of(&router, &agreement).await[0]);
        assert_eq!(
            reported_member, member,
            "case {case:?}: expected the {member} union member, got {payload:?}"
        );
        assert_eq!(
            minor_units(case, &payload["price"]),
            minor_units(case, &charged),
            "case {case:?}: the term's price is the amount its quote charged, got {payload:?}"
        );
    }

    // The schedule: its installments sum to the amount the quote charged, and
    // every charge date falls in the agreement's own contract window, in order.
    let case = "an installment schedule";
    let created = create(
        &router,
        new_request(
            "ap-schedule",
            vec![term("PaymentScheduleTerm"), term("ValidityTerm")],
        ),
    )
    .await;
    let charged = created.body["chargeSummary"]["expectedCharges"][0]["amount"].clone();
    let accepted = accept(
        &router,
        json!({"agreementRequestId": created.str_at("/agreementRequestId")}),
    )
    .await;
    let agreement = accepted.str_at("/agreementId");
    let reported = terms_of(&router, &agreement).await;
    let payloads: Vec<(String, Value)> = reported.iter().map(union_member).collect();
    let schedule_term = payloads
        .iter()
        .find(|(member, _)| member == "paymentScheduleTerm")
        .map(|(_, payload)| payload.clone())
        .unwrap_or_else(|| panic!("case {case:?}: expected a payment schedule in {reported:?}"));
    let validity = payloads
        .iter()
        .find(|(member, _)| member == "validityTerm")
        .map(|(_, payload)| payload.clone())
        .unwrap_or_else(|| panic!("case {case:?}: expected a validity term in {reported:?}"));
    let schedule = schedule_term["schedule"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        schedule.len() > 1,
        "case {case:?}: a schedule bills in installments, got {schedule_term:?}"
    );
    let total: u128 = schedule
        .iter()
        .map(|installment| minor_units(case, &installment["chargeAmount"]))
        .sum();
    assert_eq!(
        total,
        minor_units(case, &charged),
        "case {case:?}: the installments must sum to the amount the quote charged, \
         got {schedule_term:?}"
    );
    let start = validity["agreementStartDate"].as_f64().unwrap_or_default();
    let end = validity["agreementEndDate"].as_f64().unwrap_or_default();
    let mut previous = f64::NEG_INFINITY;
    for installment in &schedule {
        let date = installment["chargeDate"].as_f64().unwrap_or_default();
        assert!(
            date > previous,
            "case {case:?}: the installments are billed in order, got {schedule:?}"
        );
        assert!(
            (start..end).contains(&date),
            "case {case:?}: an installment is billed inside the agreement's window \
             [{start}, {end}), got {installment:?}"
        );
        previous = date;
    }
}

/// One row of the validity table: what the acceptor selected, and the window the
/// agreement then runs for.
struct Validity {
    case: &'static str,
    /// The contract length the acceptor selected on a prepaid term, or `None`
    /// when the agreement carries no prepaid term at all.
    selector: Option<&'static str>,
    /// The length the agreement runs for, and how long that is in days.
    expected: (&'static str, u64),
}

/// A validity term reports the window the agreement itself runs for: it starts
/// at the agreement's signature time, runs for the contract length the acceptor
/// selected, and is the SAME window `DescribeAgreement` reports — an agreement
/// "ends on its pre-defined end date", and `endTime` "is null for pay-as-you-go
/// agreements, which don't have end dates".
///
/// The rows span every contract length the offer publishes, one it does not, and
/// an agreement whose terms select none.
#[tokio::test]
async fn a_validity_term_reports_the_window_the_agreement_runs_for() {
    let mut cases: Vec<Validity> = CONTRACT_LENGTHS
        .iter()
        .map(|(length, days)| Validity {
            case: "a published contract length",
            selector: Some(*length),
            expected: (*length, *days),
        })
        .collect();
    cases.push(Validity {
        case: "a contract length the rate card does not publish",
        selector: Some(UNPUBLISHED_CONTRACT_LENGTH),
        expected: DEFAULT_CONTRACT_LENGTH,
    });
    cases.push(Validity {
        case: "no prepaid term at all",
        selector: None,
        expected: DEFAULT_CONTRACT_LENGTH,
    });

    let router = emulator();
    for row in cases {
        let case = format!("{} {:?}", row.case, row.selector);
        let mut requested = vec![term("ValidityTerm")];
        if let Some(selector) = row.selector {
            requested.push(upfront_term("cfg", selector, &[("Users", 1)]));
        }
        let agreement = agreement_over(&router, "ap-validity", requested).await;
        let reported = terms_of(&router, &agreement).await;
        let (_, validity) = union_member(&reported[0]);
        assert_eq!(
            validity["agreementDuration"].as_str(),
            Some(row.expected.0),
            "case {case:?}: expected the length the acceptor selected, got {validity:?}"
        );
        let start = validity["agreementStartDate"].as_f64().unwrap_or_default();
        let end = validity["agreementEndDate"].as_f64().unwrap_or_default();
        let window = end - start;
        let expected_window = row.expected.1 as f64 * SECONDS_PER_DAY;
        assert!(
            (window - expected_window).abs() < 1.0,
            "case {case:?}: expected a window of {expected_window} seconds, got {window} \
             in {validity:?}"
        );

        // The agreement and its validity term report ONE window: the term is
        // what gives the agreement its end date.
        let described = describe(&router, &agreement).await;
        assert_eq!(
            described.body["startTime"].as_f64(),
            Some(start),
            "case {case:?}: the agreement starts when its validity term says, got {:?}",
            described.body
        );
        assert_eq!(
            described.body["endTime"].as_f64(),
            Some(end),
            "case {case:?}: the agreement ends when its validity term says, got {:?}",
            described.body
        );
        assert_eq!(
            described.str_at("/endTimeBehavior/type"),
            "EXPIRE",
            "case {case:?}: an agreement with an end date and no renewal term expires on it, \
             got {:?}",
            described.body
        );
    }

    // An agreement whose accepted terms carry no validity term has no end date
    // at all, so nothing invents one.
    let open_ended = agreement_over(&router, "ap-open", vec![term("LegalTerm")]).await;
    let described = describe(&router, &open_ended).await;
    assert!(
        described.body["endTime"].is_null(),
        "case {:?}: an agreement whose terms set no end date has none, got {:?}",
        "no validity term",
        described.body
    );
}

/// The kind of a term is resolved from what the service holds, and a term
/// carried over to an amendment or a replacement by id keeps the kind the
/// agreement it came from holds for it — the `TermID` is "common between offer
/// and agreement".
///
/// The rows differ along the intent that derives the new agreement and the kind
/// carried over, including a term id that names no kind at all: on its own such
/// an id is a legal term with no payload, and carried over it is the term the
/// source agreement holds.
#[tokio::test]
async fn a_carried_over_term_keeps_the_kind_the_source_agreement_holds() {
    let cases: Vec<(&str, &str, Value, &str, &str)> = vec![
        (
            "an amendment carries a configured prepaid term over",
            "AMEND",
            upfront_term("abc123term", "P12M", &[("Users", 2)]),
            "configurableUpfrontPricingTerm",
            "ConfigurableUpfrontPricingTerm",
        ),
        (
            "a replacement carries a variable payment term over",
            "REPLACE",
            variable_payment_term("xyz789term"),
            "variablePaymentTerm",
            "VariablePaymentTerm",
        ),
        (
            "an amendment carries a net payment term over",
            "AMEND",
            term("NetPaymentTerm"),
            "netPaymentTerm",
            "NetPaymentTerm",
        ),
    ];

    let router = emulator();
    for (case, intent, requested, member, kind) in cases {
        let term_id = requested["id"].as_str().unwrap_or_default().to_string();
        let source = agreement_over(&router, "ap-chain", vec![requested]).await;

        // The follow-on request names the SAME term id and carries no
        // configuration of its own, so nothing but the source agreement names
        // the kind.
        let mut follow_on = json!({
            "intent": intent,
            "sourceAgreementIdentifier": source,
            "requestedTerms": [{"id": term_id}],
        });
        if intent != "AMEND" {
            // "`agreementProposalIdentifier` … is required if the intent is not
            // `AMEND`".
            follow_on["agreementProposalIdentifier"] = json!("ap-chainnext");
        }
        let created = create(&router, follow_on).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the follow-on request must be created, got {} with {:?}",
            created.status, created.body
        );
        let accepted = accept(
            &router,
            json!({"agreementRequestId": created.str_at("/agreementRequestId")}),
        )
        .await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: accepting the follow-on request must succeed, got {} with {:?}",
            accepted.status, accepted.body
        );
        let derived = accepted.str_at("/agreementId");
        let reported = terms_of(&router, &derived).await;
        let (reported_member, payload) = union_member(&reported[0]);
        assert_eq!(
            (reported_member.as_str(), payload["type"].as_str()),
            (member, Some(kind)),
            "case {case:?}: the carried-over term keeps its kind, got {reported:?}"
        );
        assert_eq!(
            payload["id"].as_str(),
            Some(term_id.as_str()),
            "case {case:?}: the carried-over term keeps its id, got {reported:?}"
        );
    }
}

/// Reporting the terms changes nothing a later caller sees: two reads of the
/// same agreement answer the same terms, and the agreement's own description is
/// the same before and after.
#[tokio::test]
async fn reporting_the_terms_leaves_the_agreement_as_it_was() {
    let router = emulator();
    let agreement = agreement_over(
        &router,
        "ap-reports",
        vec![
            term("ValidityTerm"),
            term("FixedUpfrontPricingTerm"),
            upfront_term("cfg", "P24M", &[("Users", 2), ("Hosts", 1)]),
            variable_payment_term("vpt"),
            term("abc123term"),
        ],
    )
    .await;
    let before = describe(&router, &agreement).await;
    let first = terms_of(&router, &agreement).await;
    let second = terms_of(&router, &agreement).await;
    assert_eq!(
        first, second,
        "case {:?}: a report must answer the same terms twice, got {first:?} then {second:?}",
        "two reads"
    );
    assert_eq!(
        first.len(),
        5,
        "case {:?}: every requested term is reported, got {first:?}",
        "two reads"
    );
    let after = describe(&router, &agreement).await;
    assert_eq!(
        before.body, after.body,
        "case {:?}: reporting the terms must not change the agreement, got {:?} then {:?}",
        "two reads", before.body, after.body
    );
}
