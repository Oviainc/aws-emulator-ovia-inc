// SPDX-License-Identifier: CC0-1.0

//! The value of a `SearchAgreements` FILTER is resolved against the closed set
//! the model gives its attribute, end to end over HTTP.
//!
//! `selector_resolution.rs` pins that rule for the selector MEMBERS (`catalog`,
//! `agreementType`, `agreementId`). The same rule reaches the value of a filter:
//! `FilterValue` is a pattern-constrained string
//! (`smithy_query(query="shape", shape="FilterValueList")`), so the boundary
//! validator checks only its shape, and a filter that selects on an attribute
//! the model gives a CLOSED value set for (`PartyType`, `AgreementType`, and the
//! enums `AgreementStatus`, `EndTimeBehaviorType`,
//! `EndTimeBehaviorReasonCode`) names nothing this service could hold when its
//! value lies outside that set. Such a value is refused with HTTP 400
//! `ValidationException` carrying the reason `ValidationExceptionReason` models
//! for that attribute — `INVALID_PARTY_TYPE`, `INVALID_AGREEMENT_TYPE`,
//! `INVALID_STATUS` — and the generic `INVALID_FILTER_VALUES` where it models
//! none, each naming the offending value's own request path. A value INSIDE the
//! set stays a filter: it selects whatever carries it, and an empty page is a
//! legitimate answer.
//!
//! The rows differ in what the rule generalises over: which attribute the filter
//! selects on, every value its published set carries (so the fix cannot be
//! "refuse everything"), the FORM of a value outside it (an unrelated word, the
//! right word in the wrong case, a prefix, a suffixed one, and the longest value
//! `FilterValue`'s `@length(1, 256)` admits), and what the store holds — each
//! case runs against an empty store and a store holding an agreement, since a
//! value checked only while scanning would answer an empty page instead.
//!
//! The harness (`emulator`, `call`, `agreement_over`, `term`) is the one
//! `agreement_lifecycle` owns.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use super::agreement_lifecycle::{agreement_over, call, emulator, term};

/// `PartyType`'s two values, as `SearchAgreementsInput$filters` documents them:
/// "The party type of the caller. Use `Proposer` or `Acceptor`."
const PUBLISHED_PARTY_TYPES: &[&str] = &["Proposer", "Acceptor"];

/// Every `agreementType` the model documents: "The type of agreement. Values are
/// `PurchaseAgreement` or `VendorInsightsAgreement`" (`smithy_query(query="doc",
/// shape="DescribeAgreementOutput", member="agreementType")`).
const PUBLISHED_AGREEMENT_TYPES: &[&str] = &["PurchaseAgreement", "VendorInsightsAgreement"];

/// Every value `AgreementStatus` carries
/// (`smithy_query(query="shape", shape="AgreementStatus")`), which is the closed
/// set the `Status` filter selects on — including the ones no flow of this crate
/// reaches, which are values the filter admits and selects nothing with.
const PUBLISHED_STATUSES: &[&str] = &[
    "ACTIVE",
    "ARCHIVED",
    "CANCELLED",
    "EXPIRED",
    "RENEWED",
    "REPLACED",
    "ROLLED_BACK",
    "SUPERSEDED",
    "TERMINATED",
];

/// Every value `EndTimeBehaviorType` carries
/// (`smithy_query(query="shape", shape="EndTimeBehaviorType")`).
const PUBLISHED_END_TIME_BEHAVIORS: &[&str] = &["EXPIRE", "RENEW", "REPLACE"];

/// Every value `EndTimeBehaviorReasonCode` carries
/// (`smithy_query(query="shape", shape="EndTimeBehaviorReasonCode")`).
const PUBLISHED_END_TIME_REASON_CODES: &[&str] = &[
    "ACCEPTOR_RENEW_OPTED_OUT",
    "NO_RENEWAL_TERM",
    "PROPOSER_RENEW_OPTED_OUT",
    "RENEWAL_LIMIT_EXHAUSTED",
];

/// One filter whose attribute has a closed value set: its name, that set, the
/// values outside it this suite sends, and the reason each of those is refused
/// with.
struct FilterValues {
    name: &'static str,
    published: &'static [&'static str],
    outside: Vec<String>,
    reason: &'static str,
}

/// Values outside a published set, spanning the forms a client gets wrong: an
/// unrelated word, the right word in the wrong case, a prefix of a published
/// value, a suffixed one, and the longest value `FilterValue` admits
/// (`@length(1, 256)`, `^[A-Za-z0-9.,+:/_-]+$`) so the refusal comes from the
/// value set and not from the length.
fn outside(unrelated: &str, wrong_case: &str, prefix: &str, suffixed: &str) -> Vec<String> {
    vec![
        unrelated.to_string(),
        wrong_case.to_string(),
        prefix.to_string(),
        suffixed.to_string(),
        "V".repeat(256),
    ]
}

fn filters() -> Vec<FilterValues> {
    vec![
        FilterValues {
            name: "PartyType",
            published: PUBLISHED_PARTY_TYPES,
            outside: outside("Bystander", "proposer", "Accept", "Acceptors"),
            reason: "INVALID_PARTY_TYPE",
        },
        FilterValues {
            name: "AgreementType",
            published: PUBLISHED_AGREEMENT_TYPES,
            outside: outside(
                "NotAType",
                "purchaseagreement",
                "VendorInsights",
                "PurchaseAgreements",
            ),
            reason: "INVALID_AGREEMENT_TYPE",
        },
        FilterValues {
            name: "Status",
            published: PUBLISHED_STATUSES,
            outside: outside("NOTASTATUS", "active", "ACTIV", "ACTIVATED"),
            reason: "INVALID_STATUS",
        },
        // `ValidationExceptionReason` models no reason naming either
        // end-time-behaviour attribute, so a value outside those sets is the
        // generic filter-value refusal.
        FilterValues {
            name: "EndTimeBehaviorType",
            published: PUBLISHED_END_TIME_BEHAVIORS,
            outside: outside("ROLL_OVER", "renew", "RENE", "RENEWED"),
            reason: "INVALID_FILTER_VALUES",
        },
        FilterValues {
            name: "EndTimeBehaviorReasonCode",
            published: PUBLISHED_END_TIME_REASON_CODES,
            outside: outside(
                "BUYER_CHANGED_MIND",
                "no_renewal_term",
                "NO_RENEWAL",
                "NO_RENEWAL_TERMS",
            ),
            reason: "INVALID_FILTER_VALUES",
        },
    ]
}

/// The request path of the single value the FIRST filter carries, as the
/// framework's own path rule spells it (`FilterValueList` is
/// `@length(min: 1, max: 1)`, so the value is that list's first element).
const FIRST_FILTER_VALUE_PATH: &str = "filters.1.member.values.1.member";

/// The filter list one row sends: the filter under test FIRST, so a refusal's
/// path names it, followed by the agreement-type filter
/// `SearchAgreementsInput$filters` requires of every search — unless the row is
/// about that filter itself, which already carries it.
fn filters_for(name: &str, value: &str) -> serde_json::Value {
    let under_test = json!({"name": name, "values": [value]});
    if name == "AgreementType" {
        json!([under_test])
    } else {
        json!([
            under_test,
            {"name": "AgreementType", "values": ["PurchaseAgreement"]},
        ])
    }
}

/// Two stores, so a value the handler only compared while scanning cannot pass:
/// one holding an agreement, and one holding nothing at all.
async fn stores() -> Vec<(&'static str, MultiServiceRouter)> {
    let held = emulator();
    agreement_over(&held, "ap-offer1", vec![term("eula.v1")]).await;
    vec![
        ("a store holding an agreement", held),
        ("an empty store", emulator()),
    ]
}

/// A filter value outside the closed set its attribute publishes is refused,
/// whatever the store holds.
#[tokio::test]
async fn a_filter_value_outside_its_published_set_is_refused() {
    for (store, router) in stores().await {
        for filter in filters() {
            for value in &filter.outside {
                let case = format!("filter {} {value:?} ({store})", filter.name);
                let answer = call(
                    &router,
                    "SearchAgreements",
                    json!({"filters": filters_for(filter.name, value)}),
                )
                .await;
                assert_eq!(
                    answer.status, 400,
                    "case {case:?}: expected HTTP 400, got {} with {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error_code(),
                    "ValidationException",
                    "case {case:?}: expected ValidationException, got {:?}",
                    answer.body
                );
                assert_eq!(
                    answer.reason(),
                    filter.reason,
                    "case {case:?}: expected reason {}, got {:?}",
                    filter.reason,
                    answer.body
                );
                assert_eq!(
                    answer.str_at("/fields/0/name"),
                    FIRST_FILTER_VALUE_PATH,
                    "case {case:?}: the refusal must name the offending value, got {:?}",
                    answer.body
                );
            }
        }
    }
}

/// Every value a published set carries is SERVED: the refusal above is of values
/// outside the set, not of the filter.
#[tokio::test]
async fn every_published_filter_value_is_served() {
    for (store, router) in stores().await {
        for filter in filters() {
            for value in filter.published {
                let answer = call(
                    &router,
                    "SearchAgreements",
                    json!({"filters": filters_for(filter.name, value)}),
                )
                .await;
                assert_eq!(
                    answer.status,
                    200,
                    "case filter {} {value:?} ({store}): a published value must be searched on, \
                     got {} with {:?}",
                    filter.name,
                    answer.status,
                    answer.body
                );
            }
        }
    }
}

/// A refused filter is refused wherever it sits in the list, and the refusal
/// names THAT element — so a caller with several filters can tell which one was
/// rejected.
#[tokio::test]
async fn the_refusal_names_the_filter_that_carried_the_value() {
    let router = emulator();
    agreement_over(&router, "ap-offer1", vec![term("eula.v1")]).await;
    let answer = call(
        &router,
        "SearchAgreements",
        json!({"filters": [
            {"name": "AgreementType", "values": ["PurchaseAgreement"]},
            {"name": "Status", "values": ["NOTASTATUS"]},
        ]}),
    )
    .await;
    assert_eq!(
        answer.status, 400,
        "a bad value in the second filter must be refused, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.reason(),
        "INVALID_STATUS",
        "the reason must name the attribute the second filter selects on, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str_at("/fields/0/name"),
        "filters.2.member.values.1.member",
        "the refusal must name the SECOND filter's value, got {:?}",
        answer.body
    );
}
