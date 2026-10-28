// SPDX-License-Identifier: CC0-1.0

//! What an agreement does at its end date, as each projection of it reports it,
//! end to end over HTTP.
//!
//! The rule under test: the behaviour at an agreement's end date is computed
//! from the end date and the accepted terms the agreement stores, and each
//! projection of it is withheld exactly under the nullness rule its OWN model
//! member states. `AgreementViewSummary$endTimeBehaviorType` and
//! `…$endTimeBehaviorReasonCode` state one null case — the field is "null for
//! agreements that have no end date, because those agreements never reach an end
//! time" — so `SearchAgreements` reports both for every agreement that HAS an
//! end date, whatever status it has since reached, and its
//! `EndTimeBehaviorType`/`EndTimeBehaviorReasonCode` filters select on exactly
//! those reported values. `DescribeAgreementOutput$endTimeBehavior` states a
//! further null case of its own — "It is also not present for an agreement that
//! is no longer active" — and is the only projection that applies it. Leaving
//! `ACTIVE` changes neither the stored end date nor the accepted terms, so it
//! changes nothing the summary computes.
//!
//! The rows differ along what the rule generalises over: the status the
//! agreement carries and the path that put it there (still active, a direct
//! cancel, an approved cancellation request, a replacement), the behaviour its
//! accepted terms commit it to (expiring with each reason code those terms can
//! give, renewing), and whether its terms commit an end date at all.
//!
//! The harness (`emulator`, `accept`, `create`, `cancel`, `describe`, `search`,
//! `term`, `upfront_term`, `renewal_term`, `agreement_over`) is the one
//! `agreement_lifecycle` owns; `summary_of`, `selected` and `ids` are
//! `agreement_end_dates`'; `cancellation_over` is
//! `agreement_cancellation_requests`'; the published value sets are
//! `selector_values`'.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::agreement_cancellation_requests::cancellation_over;
use crate::agreement_end_dates::{ids, selected, summary_of};
use crate::agreement_lifecycle::{
    accept, agreement_over, call, cancel, create, describe, emulator, renewal_term, term,
    upfront_term,
};
use crate::selector_values::{PUBLISHED_END_TIME_BEHAVIORS, PUBLISHED_END_TIME_REASON_CODES};

/// Where an agreement's status stands, and for a terminal one the path that put
/// it there: every way an agreement of this service leaves `ACTIVE`, plus
/// staying active as the baseline the others are compared against.
#[derive(Debug, Clone, Copy)]
enum Standing {
    /// Still `ACTIVE`: nothing was done to it after acceptance.
    Active,
    /// The acceptor cancelled it directly (`CancelAgreement`).
    Cancelled,
    /// The buyer approved the seller's cancellation request, whose approval is
    /// what cancels the agreement.
    CancellationApproved,
    /// A replacement agreement superseded it (`AcceptAgreementRequest` on a
    /// `REPLACE` request naming it as the source).
    Replaced,
}

/// Every standing a row is driven through.
const STANDINGS: [Standing; 4] = [
    Standing::Active,
    Standing::Cancelled,
    Standing::CancellationApproved,
    Standing::Replaced,
];

impl Standing {
    /// The `AgreementStatus` the agreement reports in this standing, from
    /// `smithy_query(query="shape", shape="AgreementStatus")`: "CANCELLED – The
    /// agreement was cancelled" and "REPLACED – The agreement was replaced using
    /// an agreement replacement offer".
    fn reported(self) -> &'static str {
        match self {
            Self::Active => "ACTIVE",
            Self::Cancelled | Self::CancellationApproved => "CANCELLED",
            Self::Replaced => "REPLACED",
        }
    }

    fn is_active(self) -> bool {
        matches!(self, Self::Active)
    }
}

/// Put `agreement` into `standing`, by the path that standing names.
async fn put_into(router: &MultiServiceRouter, agreement: &str, standing: Standing) {
    match standing {
        Standing::Active => {}
        Standing::Cancelled => {
            let cancelled = cancel(router, agreement).await;
            assert_eq!(
                cancelled.status, 200,
                "case {standing:?}: cancelling {agreement} must succeed, got {} with {:?}",
                cancelled.status, cancelled.body
            );
        }
        Standing::CancellationApproved => {
            let request_id = cancellation_over(router, agreement, "OTHER").await;
            let approved = call(
                router,
                "AcceptAgreementCancellationRequest",
                json!({
                    "agreementId": agreement,
                    "agreementCancellationRequestId": request_id,
                }),
            )
            .await;
            assert_eq!(
                approved.status, 200,
                "case {standing:?}: approving {request_id} must succeed, got {} with {:?}",
                approved.status, approved.body
            );
        }
        Standing::Replaced => {
            let created = create(
                router,
                json!({
                    "intent": "REPLACE",
                    "agreementProposalIdentifier": "ap-replacement",
                    "sourceAgreementIdentifier": agreement,
                    "requestedTerms": [term("eula.v2")],
                }),
            )
            .await;
            assert_eq!(
                created.status, 200,
                "case {standing:?}: the replacement request must be created, got {} with {:?}",
                created.status, created.body
            );
            let accepted = accept(
                router,
                json!({"agreementRequestId": created.str_at("/agreementRequestId")}),
            )
            .await;
            assert_eq!(
                accepted.status, 200,
                "case {standing:?}: accepting the replacement must succeed, got {} with {:?}",
                accepted.status, accepted.body
            );
        }
    }
}

/// One row of the behaviour table: the terms the acceptor accepted, and the
/// `type`/`reasonCode` the behaviour at the end date they commit then has — or
/// `None` for terms that commit no end date at all, the one case the summary
/// members are null in.
struct Behavior {
    case: &'static str,
    proposal: &'static str,
    requested: Vec<Value>,
    expected: Option<(&'static str, Option<&'static str>)>,
}

fn behavior_cases() -> Vec<Behavior> {
    vec![
        Behavior {
            case: "a prepaid purchase with no renewal term",
            proposal: "ap-noRenewalTerm",
            requested: vec![upfront_term("cfg", "P12M", &[("Users", 2)])],
            expected: Some(("EXPIRE", Some("NO_RENEWAL_TERM"))),
        },
        Behavior {
            case: "a prepaid purchase whose acceptor opted out of renewing",
            proposal: "ap-optedOut",
            requested: vec![
                upfront_term("cfg", "P1M", &[("Users", 1)]),
                renewal_term("renew", false),
            ],
            expected: Some(("EXPIRE", Some("ACCEPTOR_RENEW_OPTED_OUT"))),
        },
        Behavior {
            case: "a prepaid purchase the acceptor asked to renew",
            proposal: "ap-renewing",
            requested: vec![
                upfront_term("cfg", "P24M", &[("Users", 1)]),
                renewal_term("renew", true),
            ],
            expected: Some(("RENEW", None)),
        },
        Behavior {
            case: "metered usage, which commits no end date at all",
            proposal: "ap-openEnded",
            requested: vec![term("UsageBasedPricingTerm")],
            expected: None,
        },
    ]
}

/// The `AgreementViewSummary` reports the behaviour at the end date for every
/// agreement that has an end date, whatever status the agreement has reached,
/// while `DescribeAgreement` additionally withholds it from an agreement that is
/// no longer active.
#[tokio::test]
async fn the_two_projections_of_the_end_time_behavior_follow_their_own_null_rules() {
    for row in behavior_cases() {
        for standing in STANDINGS {
            // One emulator per case, so the search below is about this row's
            // agreement and the replacement path cannot leak into another row.
            let router = emulator();
            let agreement = agreement_over(&router, row.proposal, row.requested.clone()).await;
            put_into(&router, &agreement, standing).await;
            let case = format!("{} {:?}", row.case, standing);

            let described = describe(&router, &agreement).await;
            assert_eq!(
                described.status, 200,
                "case {case:?}: describing {agreement} must succeed, got {} with {:?}",
                described.status, described.body
            );
            assert_eq!(
                described.body["status"].as_str(),
                Some(standing.reported()),
                "case {case:?}: expected status {}, got {:?}",
                standing.reported(),
                described.body
            );

            // "This field is present for every active agreement that has an end
            // date… It is also not present for an agreement that is no longer
            // active."
            match row.expected.filter(|_| standing.is_active()) {
                Some((kind, reason)) => {
                    assert_eq!(
                        described.body.pointer("/endTimeBehavior/type"),
                        Some(&json!(kind)),
                        "case {case:?}: expected endTimeBehavior {kind}, got {:?}",
                        described.body
                    );
                    assert_eq!(
                        described
                            .body
                            .pointer("/endTimeBehavior/reasonCode")
                            .and_then(Value::as_str),
                        reason,
                        "case {case:?}: expected reason code {reason:?}, got {:?}",
                        described.body
                    );
                }
                None => assert!(
                    described.body["endTimeBehavior"].is_null(),
                    "case {case:?}: describe reports no behaviour for an agreement that has no \
                     end date or is no longer active, got {:?}",
                    described.body
                ),
            }

            // "The field is null for agreements that have no end date": the ONE
            // null case of the summary's two members, so the behaviour is
            // reported here for every standing.
            let summary = summary_of(&router, &agreement).await;
            assert_eq!(
                summary["endTime"],
                described.body["endTime"],
                "case {case:?}: the summary must report the end date the describe does ({:?}), \
                 got {summary:?}",
                described.body["endTime"]
            );
            assert_eq!(
                summary["endTimeBehaviorType"].as_str(),
                row.expected.map(|(kind, _)| kind),
                "case {case:?}: expected endTimeBehaviorType {:?}, got {summary:?}",
                row.expected.map(|(kind, _)| kind)
            );
            assert_eq!(
                summary["endTimeBehaviorReasonCode"].as_str(),
                row.expected.and_then(|(_, reason)| reason),
                "case {case:?}: expected endTimeBehaviorReasonCode {:?}, got {summary:?}",
                row.expected.and_then(|(_, reason)| reason)
            );
        }
    }
}

/// The `EndTimeBehaviorType` and `EndTimeBehaviorReasonCode` filters select on
/// the values the summary reports, so they select an agreement that is no longer
/// active exactly as they select an active one — including in combination with
/// the `Status` filter, which is what narrows a search to one status.
#[tokio::test]
async fn the_end_time_behavior_filters_select_an_inactive_agreement_too() {
    let router = emulator();
    let cancelled_expiring = agreement_over(
        &router,
        "ap-cancelledExpiring",
        vec![upfront_term("cfg", "P12M", &[("Users", 1)])],
    )
    .await;
    let cancelled_opted_out = agreement_over(
        &router,
        "ap-cancelledOptedOut",
        vec![
            upfront_term("cfg", "P1M", &[("Users", 1)]),
            renewal_term("renew", false),
        ],
    )
    .await;
    let cancelled_renewing = agreement_over(
        &router,
        "ap-cancelledRenewing",
        vec![
            upfront_term("cfg", "P24M", &[("Users", 1)]),
            renewal_term("renew", true),
        ],
    )
    .await;
    let cancelled_open_ended = agreement_over(
        &router,
        "ap-cancelledOpen",
        vec![term("UsageBasedPricingTerm")],
    )
    .await;
    let active_expiring = agreement_over(
        &router,
        "ap-activeExpiring",
        vec![upfront_term("cfg", "P36M", &[("Users", 1)])],
    )
    .await;

    for agreement in [
        &cancelled_expiring,
        &cancelled_opted_out,
        &cancelled_renewing,
        &cancelled_open_ended,
    ] {
        put_into(&router, agreement, Standing::Cancelled).await;
    }

    let expiring = ids(&[
        &cancelled_expiring,
        &cancelled_opted_out,
        &active_expiring,
    ]);
    let cases: Vec<(&str, Value, BTreeSet<String>)> = vec![
        (
            "every agreement that expires at its end date, whatever its status",
            json!([{"name": "EndTimeBehaviorType", "values": ["EXPIRE"]}]),
            expiring.clone(),
        ),
        (
            "the cancelled agreement that renews at its end date",
            json!([{"name": "EndTimeBehaviorType", "values": ["RENEW"]}]),
            ids(&[&cancelled_renewing]),
        ),
        (
            "expiring, narrowed to the cancelled agreements",
            json!([
                {"name": "Status", "values": ["CANCELLED"]},
                {"name": "EndTimeBehaviorType", "values": ["EXPIRE"]},
            ]),
            ids(&[&cancelled_expiring, &cancelled_opted_out]),
        ),
        (
            "renewing, narrowed to the cancelled agreements",
            json!([
                {"name": "Status", "values": ["CANCELLED"]},
                {"name": "EndTimeBehaviorType", "values": ["RENEW"]},
            ]),
            ids(&[&cancelled_renewing]),
        ),
        (
            "expiring, narrowed to the one that is still active",
            json!([
                {"name": "Status", "values": ["ACTIVE"]},
                {"name": "EndTimeBehaviorType", "values": ["EXPIRE"]},
            ]),
            ids(&[&active_expiring]),
        ),
        (
            "the agreements whose terms carry no renewal term at all",
            json!([{"name": "EndTimeBehaviorReasonCode", "values": ["NO_RENEWAL_TERM"]}]),
            ids(&[&cancelled_expiring, &active_expiring]),
        ),
        (
            "the cancelled agreement whose acceptor opted out of renewing",
            json!([{"name": "EndTimeBehaviorReasonCode", "values": ["ACCEPTOR_RENEW_OPTED_OUT"]}]),
            ids(&[&cancelled_opted_out]),
        ),
        (
            "a reason code no agreement here reports",
            json!([{"name": "EndTimeBehaviorReasonCode", "values": ["RENEWAL_LIMIT_EXHAUSTED"]}]),
            BTreeSet::new(),
        ),
        (
            "a behaviour type no agreement here reports",
            json!([{"name": "EndTimeBehaviorType", "values": ["REPLACE"]}]),
            BTreeSet::new(),
        ),
        (
            "the status filter alone, which selects on the status and not on the behaviour",
            json!([{"name": "Status", "values": ["CANCELLED"]}]),
            ids(&[
                &cancelled_expiring,
                &cancelled_opted_out,
                &cancelled_renewing,
                &cancelled_open_ended,
            ]),
        ),
    ];

    for (case, filters, expected) in cases {
        let reported = selected(&router, filters).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }

    // An agreement with no end date reaches no end time, so no value of either
    // behaviour filter selects it — cancelling it does not change that.
    for (name, values) in [
        ("EndTimeBehaviorType", PUBLISHED_END_TIME_BEHAVIORS),
        ("EndTimeBehaviorReasonCode", PUBLISHED_END_TIME_REASON_CODES),
    ] {
        for value in values {
            let reported = selected(&router, json!([{"name": name, "values": [value]}])).await;
            assert!(
                !reported.contains(&cancelled_open_ended),
                "case {name:?}={value:?}: an agreement with no end date is selected by no \
                 behaviour filter, got {reported:?}"
            );
        }
    }
}
