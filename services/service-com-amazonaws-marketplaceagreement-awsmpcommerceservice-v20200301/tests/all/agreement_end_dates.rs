// SPDX-License-Identifier: CC0-1.0

//! WHEN an agreement ends, end to end over HTTP.
//!
//! The rule under test: an agreement runs until the duration its accepted terms
//! committed it for runs out. `ValidityTerm$agreementEndDate` states it for terms
//! that name no end date of their own — "If `AgreementEndDate` isn't provided,
//! the agreement end date is determined by the validity of individual terms" —
//! and a term states such a duration in its own member: a validity term's
//! `agreementDuration` ("the duration that the agreement remains active"), a
//! configured prepaid term's
//! `ConfigurableUpfrontPricingTermConfiguration$selectorValue` ("the length of
//! time for which the particular pricing/dimension is being purchased by the
//! acceptor") and a fixed prepaid term's `duration` ("contract duration for the
//! terms"). An agreement whose terms commit NO duration has no end date, which is
//! what `DescribeAgreementOutput$endTime` leaves null "for pay-as-you-go
//! agreements, which don't have end dates".
//!
//! Everything the model hangs off that end date follows from it:
//! `DescribeAgreementOutput$endTimeBehavior` is "present for every active
//! agreement that has an end date", `AgreementViewSummary` reports the same
//! behaviour as two members, the `BeforeEndTime`/`AfterEndTime`/
//! `EndTimeBehaviorType`/`EndTimeBehaviorReasonCode` filters of
//! `SearchAgreements` select on it, and `Sort$sortBy`'s default `EndTime` orders
//! by it.
//!
//! The rows differ in what the rule generalises over: which kind of term carries
//! the duration, which contract length the acceptor selected (every length the
//! rate card publishes, one it does not, and none at all), whether the acceptor
//! selected anything on the prepaid term, whether a renewal term and which
//! choice on it, and every term kind that commits no duration at all. The
//! harness (`emulator`, `accept`, `create`, `describe`, `search`, `new_request`,
//! `term`, `upfront_term`, `renewal_term`, `agreement_over`,
//! `with_agreement_type`) is the one `agreement_lifecycle` owns.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::agreement_lifecycle::{
    agreement_over, describe, emulator, renewal_term, search, term, upfront_term,
    with_agreement_type, Answer,
};

/// The contract lengths the published rate card prices a configured prepaid
/// purchase at, each with how long the agreement then runs, in days.
///
/// SOURCE: the selectors the emulator's rate card publishes
/// (`ConfigurableUpfrontRateCardItem$selector` "differentiates between the
/// mutually exclusive rate cards in the same pricing term to be selected by the
/// buyer"), spelled out here rather than imported so a handler publishing a
/// different set fails these rows.
const CONTRACT_LENGTHS: &[(&str, u64)] = &[
    ("P1M", 30),
    ("P12M", 365),
    ("P24M", 730),
    ("P36M", 1095),
];

/// The contract length an agreement runs for when its acceptor selected none the
/// rate card publishes.
const DEFAULT_CONTRACT_LENGTH: (&str, u64) = ("P12M", 365);

/// A contract length the published rate card does NOT carry, one step below its
/// shortest published one. `BoundedString`'s `^(.)+$` admits it.
const UNPUBLISHED_CONTRACT_LENGTH: &str = "P6M";

/// Seconds in a day, for turning a contract length into a window.
const SECONDS_PER_DAY: f64 = 86_400.0;

/// An instant no agreement this suite accepts can have reached, so a
/// `BeforeEndTime` bound at it is after every end date and an `AfterEndTime`
/// bound at it is after all of them.
const FAR_FUTURE: &str = "2999-01-01T00:00:00Z";

/// An instant before every agreement this suite accepts was signed.
const FAR_PAST: &str = "2000-01-01T00:00:00Z";

/// One row of the end-date table: the terms the acceptor accepted, and the end
/// date they commit the agreement to.
struct EndDate {
    case: &'static str,
    requested: Vec<Value>,
    /// How many days after its start the agreement ends, or `None` for terms
    /// that commit it for no duration at all.
    days: Option<u64>,
    /// `EndTimeBehavior$type` and `…$reasonCode` the agreement then reports, or
    /// `None` when it reports no behaviour because it has no end date.
    behavior: Option<(&'static str, Option<&'static str>)>,
}

/// `EndTimeBehavior` for an agreement that has no renewal term at all.
const EXPIRES_UNRENEWED: Option<(&str, Option<&str>)> = Some(("EXPIRE", Some("NO_RENEWAL_TERM")));

fn end_date_cases() -> Vec<EndDate> {
    // One row per contract length the rate card publishes: the duration the
    // acceptor purchased is the one the agreement runs for.
    let mut cases: Vec<EndDate> = CONTRACT_LENGTHS
        .iter()
        .map(|(length, days)| EndDate {
            case: "a prepaid purchase at a published contract length",
            requested: vec![upfront_term("cfg", length, &[("Users", 2)])],
            days: Some(*days),
            behavior: EXPIRES_UNRENEWED,
        })
        .collect();
    cases.extend([
        EndDate {
            case: "a prepaid purchase at a length the rate card does not publish",
            requested: vec![upfront_term(
                "cfg",
                UNPUBLISHED_CONTRACT_LENGTH,
                &[("Users", 1)],
            )],
            days: Some(DEFAULT_CONTRACT_LENGTH.1),
            behavior: EXPIRES_UNRENEWED,
        },
        EndDate {
            case: "a validity term, which states the agreement's own duration",
            requested: vec![term("ValidityTerm")],
            days: Some(DEFAULT_CONTRACT_LENGTH.1),
            behavior: EXPIRES_UNRENEWED,
        },
        EndDate {
            case: "a fixed prepaid term, whose duration is the contract duration",
            requested: vec![term("FixedUpfrontPricingTerm")],
            days: Some(DEFAULT_CONTRACT_LENGTH.1),
            behavior: EXPIRES_UNRENEWED,
        },
        EndDate {
            case: "a validity term beside a prepaid purchase: one window, the purchased one",
            requested: vec![
                term("ValidityTerm"),
                upfront_term("cfg", "P24M", &[("Users", 3)]),
            ],
            days: Some(730),
            behavior: EXPIRES_UNRENEWED,
        },
        EndDate {
            case: "a prepaid purchase the acceptor asked to renew",
            requested: vec![
                upfront_term("cfg", "P36M", &[("Users", 1)]),
                renewal_term("renew", true),
            ],
            // "The field is null when the agreement renews."
            days: Some(1095),
            behavior: Some(("RENEW", None)),
        },
        EndDate {
            case: "a prepaid purchase the acceptor opted out of renewing",
            requested: vec![
                upfront_term("cfg", "P1M", &[("Users", 1)]),
                renewal_term("renew", false),
            ],
            days: Some(30),
            behavior: Some(("EXPIRE", Some("ACCEPTOR_RENEW_OPTED_OUT"))),
        },
        EndDate {
            case: "metered usage, which is charged on product usage and ends nothing",
            requested: vec![term("UsageBasedPricingTerm")],
            days: None,
            behavior: None,
        },
        EndDate {
            case: "a recurring payment term, which states a billing period and no purchase length",
            requested: vec![term("RecurringPaymentTerm")],
            days: None,
            behavior: None,
        },
        EndDate {
            case: "a free trial, whose duration is the trial's and not the agreement's",
            requested: vec![term("FreeTrialPricingTerm")],
            days: None,
            behavior: None,
        },
        EndDate {
            case: "a prepaid term the acceptor configured nothing on, so purchased no length",
            requested: vec![term("ConfigurableUpfrontPricingTerm")],
            days: None,
            behavior: None,
        },
        EndDate {
            case: "a renewal term alone, which decides what happens at an end date it does not set",
            requested: vec![renewal_term("renew", true)],
            days: None,
            behavior: None,
        },
        EndDate {
            case: "a legal, support and BYOL term, none of which states a window",
            requested: vec![
                term("LegalTerm"),
                term("SupportTerm"),
                term("ByolPricingTerm"),
            ],
            days: None,
            behavior: None,
        },
        EndDate {
            case: "a payment schedule and a net payment period, neither a purchase length",
            requested: vec![term("PaymentScheduleTerm"), term("NetPaymentTerm")],
            days: None,
            behavior: None,
        },
    ]);
    cases
}

/// What `DescribeAgreement` reports for `agreement`, which must succeed.
async fn described(router: &MultiServiceRouter, agreement: &str) -> Answer {
    let answer = describe(router, agreement).await;
    assert_eq!(
        answer.status, 200,
        "describing {agreement} must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    answer
}

/// The `AgreementViewSummary` `SearchAgreements` reports for `agreement`.
async fn summary_of(router: &MultiServiceRouter, agreement: &str) -> Value {
    let answer = search(router, json!({"filters": with_agreement_type(&json!([]))})).await;
    assert_eq!(
        answer.status, 200,
        "searching for {agreement} must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["agreementViewSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|summary| summary["agreementId"].as_str() == Some(agreement))
        .unwrap_or_else(|| panic!("the search must report {agreement}, got {:?}", answer.body))
}

/// The agreement ids one search selects, as a set so a row says WHICH agreements
/// a filter selects without restating the order the sort owns.
async fn selected(router: &MultiServiceRouter, filters: Value) -> BTreeSet<String> {
    let answer = search(router, json!({"filters": with_agreement_type(&filters)})).await;
    assert_eq!(
        answer.status, 200,
        "the search must be served, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["agreementViewSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| {
            summary["agreementId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// `instant`, in epoch seconds, as the RFC-3339 timestamp a filter value spells
/// an instant with.
fn filter_instant(instant: f64) -> String {
    ::aws_smithy_types::DateTime::from_secs_f64(instant)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .expect("an instant inside the agreement's own window is RFC-3339 spellable")
}

/// A set of the ids given, for comparing against what a filter selected.
fn ids(agreements: &[&String]) -> BTreeSet<String> {
    agreements.iter().map(|id| (*id).clone()).collect()
}

/// An agreement ends when the duration its accepted terms commit it for runs
/// out, and only then: the end date is the agreement's start plus that duration,
/// and an agreement whose terms commit none has no end date at all.
///
/// `DescribeAgreement` and the `AgreementViewSummary` of `SearchAgreements` are
/// both checked, so the behaviour the end date implies is reported in both
/// projections or in neither.
#[tokio::test]
async fn an_agreement_ends_when_the_duration_its_terms_commit_runs_out() {
    for row in end_date_cases() {
        let case = format!("{} {:?}", row.case, row.days);
        // One emulator per row, so the search below reports this row's
        // agreement only.
        let router = emulator();
        let agreement = agreement_over(&router, "ap-enddates", row.requested.clone()).await;
        let answer = described(&router, &agreement).await;
        let body = &answer.body;
        let start = body["startTime"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: an agreement must report a start, got {body:?}"));

        match row.days {
            Some(days) => {
                let end = body["endTime"].as_f64().unwrap_or_else(|| {
                    panic!("case {case:?}: expected an endTime {days} days after the start, got {body:?}")
                });
                let window = end - start;
                let expected = days as f64 * SECONDS_PER_DAY;
                assert!(
                    (window - expected).abs() < 1.0,
                    "case {case:?}: expected a window of {expected} seconds, got {window} in \
                     {body:?}"
                );
            }
            None => assert!(
                body["endTime"].is_null(),
                "case {case:?}: terms that commit no duration leave the agreement with no end \
                 date, got {body:?}"
            ),
        }

        // "This field is present for every active agreement that has an end
        // date. It is not present for an agreement that has no end date."
        match row.behavior {
            Some((kind, reason)) => {
                assert_eq!(
                    answer.str_at("/endTimeBehavior/type"),
                    kind,
                    "case {case:?}: expected endTimeBehavior {kind}, got {body:?}"
                );
                assert_eq!(
                    body.pointer("/endTimeBehavior/reasonCode")
                        .and_then(Value::as_str),
                    reason,
                    "case {case:?}: expected reason code {reason:?}, got {body:?}"
                );
            }
            None => assert!(
                body["endTimeBehavior"].is_null(),
                "case {case:?}: an agreement with no end date reports no behaviour at it, got \
                 {body:?}"
            ),
        }

        // The search projection reports the same end date and the same
        // behaviour, as two flat members.
        let summary = summary_of(&router, &agreement).await;
        assert_eq!(
            summary["endTime"], body["endTime"],
            "case {case:?}: the summary must report the end date the describe does ({:?}), got \
             {summary:?}",
            body["endTime"]
        );
        assert_eq!(
            summary["endTimeBehaviorType"].as_str(),
            row.behavior.map(|(kind, _)| kind),
            "case {case:?}: expected endTimeBehaviorType {:?}, got {summary:?}",
            row.behavior.map(|(kind, _)| kind)
        );
        assert_eq!(
            summary["endTimeBehaviorReasonCode"].as_str(),
            row.behavior.and_then(|(_, reason)| reason),
            "case {case:?}: expected endTimeBehaviorReasonCode {:?}, got {summary:?}",
            row.behavior.and_then(|(_, reason)| reason)
        );
    }
}

/// The end-date filters of `SearchAgreements` select on the stored end date and
/// on the behaviour it implies: `BeforeEndTime`/`AfterEndTime` compare against
/// the end date, `EndTimeBehaviorType`/`EndTimeBehaviorReasonCode` against the
/// behaviour, and an agreement with no end date is selected by none of them.
///
/// The bounds are a far-past and a far-future instant plus one taken from the
/// agreements themselves — between the shortest and the longest purchase — so a
/// row distinguishes the end dates rather than merely their presence.
#[tokio::test]
async fn the_end_date_filters_select_on_the_stored_end_date() {
    let router = emulator();
    let short = agreement_over(
        &router,
        "ap-short",
        vec![
            upfront_term("cfg", "P1M", &[("Users", 1)]),
            renewal_term("renew", false),
        ],
    )
    .await;
    let long = agreement_over(
        &router,
        "ap-long",
        vec![upfront_term("cfg", "P36M", &[("Users", 2)])],
    )
    .await;
    let renewing = agreement_over(
        &router,
        "ap-renewing",
        vec![
            upfront_term("cfg", "P24M", &[("Users", 1)]),
            renewal_term("renew", true),
        ],
    )
    .await;
    let open_ended = agreement_over(&router, "ap-open", vec![term("UsageBasedPricingTerm")]).await;

    let short_end = described(&router, &short).await.body["endTime"]
        .as_f64()
        .expect("the shorter purchase has an end date");
    let long_end = described(&router, &long).await.body["endTime"]
        .as_f64()
        .expect("the longer purchase has an end date");
    // An instant inside the window of the longer purchase and past the end of
    // the shorter one.
    let between = filter_instant((short_end + long_end) / 2.0);

    let end_dated = ids(&[&short, &long, &renewing]);
    let cases: Vec<(String, Value, BTreeSet<String>)> = vec![
        (
            "every end date is before the far future".to_string(),
            json!([{"name": "BeforeEndTime", "values": [FAR_FUTURE]}]),
            end_dated.clone(),
        ),
        (
            "and none of them before the far past".to_string(),
            json!([{"name": "BeforeEndTime", "values": [FAR_PAST]}]),
            BTreeSet::new(),
        ),
        (
            "every end date is after the far past".to_string(),
            json!([{"name": "AfterEndTime", "values": [FAR_PAST]}]),
            end_dated.clone(),
        ),
        (
            "and none of them after the far future".to_string(),
            json!([{"name": "AfterEndTime", "values": [FAR_FUTURE]}]),
            BTreeSet::new(),
        ),
        (
            "only the shortest purchase ends before an instant inside the longest".to_string(),
            json!([{"name": "BeforeEndTime", "values": [between.clone()]}]),
            ids(&[&short]),
        ),
        (
            "the longer purchases end after it".to_string(),
            json!([{"name": "AfterEndTime", "values": [between.clone()]}]),
            ids(&[&long, &renewing]),
        ),
        (
            "both bounds are an AND over one window".to_string(),
            json!([
                {"name": "AfterEndTime", "values": [between.clone()]},
                {"name": "BeforeEndTime", "values": [FAR_FUTURE]},
            ]),
            ids(&[&long, &renewing]),
        ),
        (
            "the agreements that expire on their end date".to_string(),
            json!([{"name": "EndTimeBehaviorType", "values": ["EXPIRE"]}]),
            ids(&[&short, &long]),
        ),
        (
            "the one that renews at it".to_string(),
            json!([{"name": "EndTimeBehaviorType", "values": ["RENEW"]}]),
            ids(&[&renewing]),
        ),
        (
            "the one whose terms carry no renewal term at all".to_string(),
            json!([{"name": "EndTimeBehaviorReasonCode", "values": ["NO_RENEWAL_TERM"]}]),
            ids(&[&long]),
        ),
        (
            "the one whose acceptor opted out of renewing".to_string(),
            json!([{"name": "EndTimeBehaviorReasonCode", "values": ["ACCEPTOR_RENEW_OPTED_OUT"]}]),
            ids(&[&short]),
        ),
        (
            "a reason code no agreement reports".to_string(),
            json!([{"name": "EndTimeBehaviorReasonCode", "values": ["RENEWAL_LIMIT_EXHAUSTED"]}]),
            BTreeSet::new(),
        ),
        (
            "behaviour and end time are an AND".to_string(),
            json!([
                {"name": "EndTimeBehaviorType", "values": ["EXPIRE"]},
                {"name": "AfterEndTime", "values": [between]},
            ]),
            ids(&[&long]),
        ),
    ];

    for (case, filters, expected) in cases {
        let reported = selected(&router, filters).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
        assert!(
            !reported.contains(&open_ended),
            "case {case:?}: an agreement with no end date matches no end-date filter, got \
             {reported:?}"
        );
    }
}

/// `Sort$sortBy`'s documented default orders a search by the agreements' end
/// dates — "The default value is `EndTime`" — ascending by default, with an
/// agreement that has no end date last, and reversed when the caller asks for
/// `DESCENDING`.
///
/// The agreements are accepted in an order unrelated to their lengths, so an
/// order that merely followed the agreement ids fails this test.
#[tokio::test]
async fn the_default_sort_orders_a_search_by_the_end_dates() {
    let router = emulator();
    // Accepted shortest-last, so creation order is the reverse of end-date
    // order for the purchases that have one.
    let two_years = agreement_over(
        &router,
        "ap-2y",
        vec![upfront_term("cfg", "P24M", &[("Users", 1)])],
    )
    .await;
    let open_ended = agreement_over(&router, "ap-open", vec![term("UsageBasedPricingTerm")]).await;
    let three_years = agreement_over(
        &router,
        "ap-3y",
        vec![upfront_term("cfg", "P36M", &[("Users", 1)])],
    )
    .await;
    let one_month = agreement_over(
        &router,
        "ap-1m",
        vec![upfront_term("cfg", "P1M", &[("Users", 1)])],
    )
    .await;

    let ascending = vec![
        one_month.clone(),
        two_years.clone(),
        three_years.clone(),
        open_ended.clone(),
    ];
    // The whole comparison is reversed, which puts the agreement that has no
    // end date first.
    let descending: Vec<String> = ascending.iter().rev().cloned().collect();

    for (case, sort, expected) in [
        ("the documented default", json!({}), ascending.clone()),
        (
            "the default attribute, named outright",
            json!({"sortBy": "EndTime"}),
            ascending.clone(),
        ),
        (
            "the default attribute and order, both named",
            json!({"sortBy": "EndTime", "sortOrder": "ASCENDING"}),
            ascending.clone(),
        ),
        (
            "newest end date first",
            json!({"sortBy": "EndTime", "sortOrder": "DESCENDING"}),
            descending.clone(),
        ),
    ] {
        let answer = search(
            &router,
            json!({"sort": sort, "filters": with_agreement_type(&json!([]))}),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        let reported: Vec<String> = answer.body["agreementViewSummaries"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|summary| {
                summary["agreementId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?} from {:?}",
            answer.body
        );
    }
}
